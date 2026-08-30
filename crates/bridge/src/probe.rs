//! Проверка живости апстримов.
//!
//! Нужна для решения `auto` и для индикаторов в UI. Кэш — чтобы не дёргать
//! сеть на каждое обращение.
//!
//! Чего здесь СОЗНАТЕЛЬНО нет: асимметричных TTL, повторных проб и
//! подтверждения перехода. Вся эта машинерия в macOS-версии защищала от
//! одного — смена решения перезапускала внешний прокси и рвала живые
//! соединения. Здесь маршрут меняется атомарно, рвать нечего, и решать
//! можно каждый раз заново.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use proxypilot_core::mode::{Health, Reachability, Upstreams};
use tokio::net::TcpStream;

struct Cached {
    at: Instant,
    /// Адреса, которые ПРОБОВАЛИ. Без них кэш ответил бы «жив» про новый
    /// адрес на основании пробы старого — это не устаревший ответ, а ответ
    /// про другое.
    probed: Upstreams,
    socks: Reachability,
    http: Reachability,
}

struct State {
    cache: Option<Cached>,
    /// Растёт на каждый invalidate. Проба, начатая до сброса, не вправе
    /// записать результат поверх: иначе invalidate можно молча отменить
    /// пробой, которая в этот момент была в полёте.
    generation: u64,
}

pub struct Prober {
    ttl: Duration,
    timeout: Duration,
    state: Mutex<State>,
}

impl Prober {
    pub fn new(ttl: Duration, timeout: Duration) -> Self {
        Self {
            ttl,
            timeout,
            state: Mutex::new(State {
                cache: None,
                generation: 0,
            }),
        }
    }

    /// Сбросить кэш — например, когда пользователь сменил адреса.
    pub fn invalidate(&self) {
        let mut s = self.state.lock().expect("отравленный мьютекс кэша проб");
        s.cache = None;
        s.generation = s.generation.wrapping_add(1);
    }

    pub async fn health(&self, up: &Upstreams) -> Health {
        let generation = {
            let s = self.state.lock().expect("отравленный мьютекс кэша проб");
            if let Some(c) = s.cache.as_ref() {
                if c.probed == *up && c.at.elapsed() < self.ttl {
                    return Health {
                        socks: c.socks,
                        http: c.http,
                    };
                }
            }
            s.generation
        };

        let (socks, http) = tokio::join!(
            self.probe(up.socks.as_deref()),
            self.probe(up.http.as_deref())
        );

        let mut s = self.state.lock().expect("отравленный мьютекс кэша проб");
        if s.generation == generation {
            s.cache = Some(Cached {
                at: Instant::now(),
                probed: up.clone(),
                socks,
                http,
            });
        }

        Health { socks, http }
    }

    async fn probe(&self, addr: Option<&str>) -> Reachability {
        // «Не задан» и «задан, но мёртв» — разные вещи и по-разному
        // объясняются пользователю.
        let Some(addr) = addr else {
            return Reachability::Unknown;
        };
        match tokio::time::timeout(self.timeout, TcpStream::connect(addr)).await {
            Ok(Ok(_)) => Reachability::Up,
            _ => Reachability::Down,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::mode::Upstreams;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn a_live_listener_is_up_and_a_closed_port_is_down() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = l.local_addr().unwrap().to_string();

        let p = Prober::new(Duration::from_secs(30), Duration::from_secs(1));
        let h = p
            .health(&Upstreams {
                socks: Some(live),
                http: Some("127.0.0.1:1".into()),
            })
            .await;
        assert_eq!(h.socks, Reachability::Up);
        assert_eq!(h.http, Reachability::Down);
    }

    #[tokio::test]
    async fn an_unconfigured_upstream_is_unknown_not_down() {
        // Разница смысловая: «не задан» и «задан, но мёртв» по-разному
        // выглядят в UI и по-разному объясняются пользователю.
        let p = Prober::new(Duration::from_secs(30), Duration::from_secs(1));
        let h = p
            .health(&Upstreams {
                socks: None,
                http: None,
            })
            .await;
        assert_eq!(h.socks, Reachability::Unknown);
        assert_eq!(h.http, Reachability::Unknown);
    }

    #[tokio::test]
    async fn the_result_is_cached_within_the_ttl() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let p = Prober::new(Duration::from_secs(30), Duration::from_secs(1));
        let up = Upstreams {
            socks: Some(addr.clone()),
            http: None,
        };

        assert_eq!(p.health(&up).await.socks, Reachability::Up);
        drop(l); // слушателя больше нет, но кэш ещё жив
        assert_eq!(p.health(&up).await.socks, Reachability::Up);

        p.invalidate();
        assert_eq!(p.health(&up).await.socks, Reachability::Down);
    }

    #[tokio::test]
    async fn a_silent_address_is_down_within_the_timeout() {
        let started = std::time::Instant::now();
        let p = Prober::new(Duration::from_secs(30), Duration::from_millis(200));
        let h = p
            .health(&Upstreams {
                socks: Some("10.255.255.1:9".into()),
                http: None,
            })
            .await;
        assert_eq!(h.socks, Reachability::Down);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "проба обязана уложиться в таймаут"
        );
    }

    #[tokio::test]
    async fn a_changed_address_is_not_answered_from_the_old_cache() {
        // Кэш обязан помнить, ЧТО он пробовал. Иначе смена адреса в конфиге
        // без invalidate даёт ответ про старый адрес, поданный как текущий.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = l.local_addr().unwrap().to_string();
        let p = Prober::new(Duration::from_secs(30), Duration::from_millis(300));

        let up_live = Upstreams {
            socks: Some(live),
            http: None,
        };
        assert_eq!(p.health(&up_live).await.socks, Reachability::Up);

        // Тот же Prober, тот же TTL — но адрес другой и мёртвый.
        let up_dead = Upstreams {
            socks: Some("127.0.0.1:1".into()),
            http: None,
        };
        assert_eq!(p.health(&up_dead).await.socks, Reachability::Down);
    }
}
