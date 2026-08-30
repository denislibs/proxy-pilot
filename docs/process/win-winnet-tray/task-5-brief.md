### Task 5: Пробы живости апстримов

**Files:**
- Create: `win/crates/bridge/src/probe.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Reachability` из `core::mode`.
- Produces: `Prober::new(ttl: Duration, timeout: Duration)`, `Prober::health(&self, up: &Upstreams) -> Health`, `Prober::invalidate(&self)`.

**Почему с кэшем, но без машинерии macOS.** Проба нужна для решения `auto` и для индикаторов в UI. Кэш — чтобы не дёргать сеть на каждый чих. Но вся защита от дребезга (асимметричные TTL, повторные пробы, подтверждение перехода) **не переносится**: она защищала от разрыва соединений при перезапуске моста, а перезапуска больше нет.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/probe.rs`:

```rust
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
            .health(&Upstreams { socks: Some(live), http: Some("127.0.0.1:1".into()) })
            .await;
        assert_eq!(h.socks, Reachability::Up);
        assert_eq!(h.http, Reachability::Down);
    }

    #[tokio::test]
    async fn an_unconfigured_upstream_is_unknown_not_down() {
        // Разница смысловая: «не задан» и «задан, но мёртв» по-разному
        // выглядят в UI и по-разному объясняются пользователю.
        let p = Prober::new(Duration::from_secs(30), Duration::from_secs(1));
        let h = p.health(&Upstreams { socks: None, http: None }).await;
        assert_eq!(h.socks, Reachability::Unknown);
        assert_eq!(h.http, Reachability::Unknown);
    }

    #[tokio::test]
    async fn the_result_is_cached_within_the_ttl() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let p = Prober::new(Duration::from_secs(30), Duration::from_secs(1));
        let up = Upstreams { socks: Some(addr.clone()), http: None };

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
        let h = p.health(&Upstreams { socks: Some("10.255.255.1:9".into()), http: None }).await;
        assert_eq!(h.socks, Reachability::Down);
        assert!(started.elapsed() < Duration::from_secs(2), "проба обязана уложиться в таймаут");
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge probe`
Expected: FAIL — `Prober` не определён.

- [ ] **Step 3: Написать реализацию**

Вставь в начало `probe.rs`:

```rust
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
    socks: Reachability,
    http: Reachability,
}

pub struct Prober {
    ttl: Duration,
    timeout: Duration,
    cache: Mutex<Option<Cached>>,
}

impl Prober {
    pub fn new(ttl: Duration, timeout: Duration) -> Self {
        Self { ttl, timeout, cache: Mutex::new(None) }
    }

    /// Сбросить кэш — например, когда пользователь сменил адреса.
    pub fn invalidate(&self) {
        *self.cache.lock().expect("отравленный мьютекс кэша проб") = None;
    }

    pub async fn health(&self, up: &Upstreams) -> Health {
        if let Some(c) = self.cache.lock().expect("отравленный мьютекс кэша проб").as_ref() {
            if c.at.elapsed() < self.ttl {
                return Health { socks: c.socks, http: c.http };
            }
        }

        let socks = self.probe(up.socks.as_deref()).await;
        let http = self.probe(up.http.as_deref()).await;

        *self.cache.lock().expect("отравленный мьютекс кэша проб") =
            Some(Cached { at: Instant::now(), socks, http });
        Health { socks, http }
    }

    async fn probe(&self, addr: Option<&str>) -> Reachability {
        // «Не задан» и «задан, но мёртв» — разные вещи и по-разному
        // объясняются пользователю.
        let Some(addr) = addr else { return Reachability::Unknown };
        match tokio::time::timeout(self.timeout, TcpStream::connect(addr)).await {
            Ok(Ok(_)) => Reachability::Up,
            _ => Reachability::Down,
        }
    }
}
```

- [ ] **Step 4: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): пробы живости апстримов с кэшем"
```

---

