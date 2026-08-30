//! Выбор маршрута: чистая функция от режима, места и живости апстримов.
//!
//! Здесь нет ни таймеров, ни кэшей, ни сети — вся защита от «дребезга»,
//! которая была в macOS-версии, существовала только потому, что смена режима
//! перезапускала внешний процесс и рвала соединения. Свой мост меняет
//! маршрут атомарно, поэтому решать можно каждый раз заново.

/// Сохранённое предпочтение пользователя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Socks,
    Http,
    Direct,
    Auto,
}

/// Фактический выход, выбранный на текущий момент.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// адрес апстрима в форме `host:port`
    Socks(String),
    /// адрес апстрима в форме `host:port`
    Http(String),
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    Up,
    Down,
    /// ещё не проверяли
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Upstreams {
    pub socks: Option<String>,
    pub http: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Health {
    pub socks: Reachability,
    pub http: Reachability,
}

/// Подключённая сейчас сеть — ровно то, что нужно для решения и для UI.
///
/// Простые данные, без единой платформенной зависимости: на Windows их
/// заполняет `winnet::NetworkSnapshot`, но `core` про него не знает и знать
/// не должен (спека, раздел 3). Категория и признак интернета сюда не
/// переносятся сознательно: решение принимается по GUID (спека 2.3), а
/// лишнее поле в модели — приглашение начать решать по нему.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedNetwork {
    /// GUID в канонической форме, как его отдаёт NLM.
    pub id: String,
    /// Имя сети, каким его показывает Windows.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub in_office: bool,
    /// Идентификатор сети, по которой принято решение. Нужен, чтобы UI мог
    /// показать «сейчас: Офис», а лог — объяснить, почему выбран маршрут.
    pub network: Option<String>,
    /// Имя той же сети. Отдельным полем, а не через `ConnectedNetwork`:
    /// имя — только для показа, в сравнении оно не участвует (см.
    /// `OfficeNetwork::name`), и связывать их в одну структуру значило бы
    /// намекать, что по имени тоже можно решать. Спека 6.1: окну настроек
    /// нужна кнопка «эта сеть — офис», а рядом с голым GUID её не подписать.
    pub network_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub route: Route,
    /// Закреплённый режим оказался недоступен и мы временно работаем иначе.
    /// Сохранённое предпочтение при этом не меняется — оно вернётся само,
    /// как только апстрим оживёт. Показывать это в UI обязательно: молчаливый
    /// обход выглядит как «галочка стоит, а трафик идёт мимо».
    pub demoted: bool,
}

pub fn decide(mode: Mode, up: &Upstreams, place: Place, health: Health) -> Decision {
    let socks = usable(&up.socks, health.socks);
    let http = usable(&up.http, health.http);

    match mode {
        Mode::Direct => Decision {
            route: Route::Direct,
            demoted: false,
        },

        // Прокси имеет смысл там, где он стоит на пути — в офисе. Снаружи
        // он тоже отвечает (через туннель), но маршрут через него был бы
        // кругом: до офисных адресов трафик и так идёт в туннель, а мимо
        // моста — по bypass-списку.
        Mode::Auto => {
            let route = if !place.in_office {
                Route::Direct
            } else if let Some(addr) = socks {
                Route::Socks(addr)
            } else if let Some(addr) = http {
                Route::Http(addr)
            } else {
                Route::Direct
            };
            Decision {
                route,
                demoted: false,
            }
        }

        Mode::Socks => match socks {
            Some(addr) => Decision {
                route: Route::Socks(addr),
                demoted: false,
            },
            None => Decision {
                route: Route::Direct,
                demoted: true,
            },
        },

        Mode::Http => match http {
            Some(addr) => Decision {
                route: Route::Http(addr),
                demoted: false,
            },
            None => Decision {
                route: Route::Direct,
                demoted: true,
            },
        },
    }
}

/// Апстрим годится, только если он задан И проверен живым.
/// `Unknown` — это «ещё не пробовали», решать на нём нельзя.
fn usable(addr: &Option<String>, health: Reachability) -> Option<String> {
    match (addr, health) {
        (Some(a), Reachability::Up) => Some(a.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ups() -> Upstreams {
        Upstreams {
            socks: Some("10.0.0.2:9999".into()),
            http: Some("10.0.0.2:3128".into()),
        }
    }
    fn health(socks: Reachability, http: Reachability) -> Health {
        Health { socks, http }
    }

    #[test]
    fn auto_outside_office_is_always_direct() {
        // Снаружи офисный прокси тоже отвечает (через VPN), но гонять через
        // него весь веб значит делать круг через офис. Спека 4.2.
        let d = decide(
            Mode::Auto,
            &ups(),
            Place {
                in_office: false,
                network: None,
                network_name: None,
            },
            health(Reachability::Up, Reachability::Up),
        );
        assert_eq!(d.route, Route::Direct);
        assert!(!d.demoted);
    }

    #[test]
    fn auto_in_office_prefers_socks() {
        let d = decide(
            Mode::Auto,
            &ups(),
            Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health(Reachability::Up, Reachability::Up),
        );
        assert_eq!(d.route, Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn auto_in_office_falls_back_to_http() {
        let d = decide(
            Mode::Auto,
            &ups(),
            Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health(Reachability::Down, Reachability::Up),
        );
        assert_eq!(d.route, Route::Http("10.0.0.2:3128".into()));
    }

    #[test]
    fn auto_in_office_with_everything_dead_is_direct() {
        let d = decide(
            Mode::Auto,
            &ups(),
            Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health(Reachability::Down, Reachability::Down),
        );
        assert_eq!(d.route, Route::Direct);
        assert!(!d.demoted);
    }

    #[test]
    fn pinned_socks_demotes_to_direct_when_dead() {
        // Пользователь не остаётся без сети, но факт понижения виден в UI.
        let d = decide(
            Mode::Socks,
            &ups(),
            Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health(Reachability::Down, Reachability::Up),
        );
        assert_eq!(d.route, Route::Direct);
        assert!(d.demoted);
    }

    #[test]
    fn pinned_http_demotes_to_direct_when_dead() {
        let d = decide(
            Mode::Http,
            &ups(),
            Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health(Reachability::Up, Reachability::Down),
        );
        assert_eq!(d.route, Route::Direct);
        assert!(d.demoted);
    }

    #[test]
    fn pinned_mode_ignores_place() {
        // Закреплённый режим — воля пользователя, место значения не имеет.
        let d = decide(
            Mode::Socks,
            &ups(),
            Place {
                in_office: false,
                network: None,
                network_name: None,
            },
            health(Reachability::Up, Reachability::Up),
        );
        assert_eq!(d.route, Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn unconfigured_upstream_is_never_chosen() {
        let up = Upstreams {
            socks: None,
            http: None,
        };
        let d = decide(
            Mode::Socks,
            &up,
            Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health(Reachability::Up, Reachability::Up),
        );
        assert_eq!(d.route, Route::Direct);
        assert!(d.demoted);
    }

    #[test]
    fn unknown_reachability_counts_as_unusable() {
        // Unknown значит «ещё не пробовали». Решать на нём нельзя.
        let d = decide(
            Mode::Auto,
            &ups(),
            Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health(Reachability::Unknown, Reachability::Unknown),
        );
        assert_eq!(d.route, Route::Direct);
    }

    #[test]
    fn direct_mode_is_direct() {
        let d = decide(
            Mode::Direct,
            &ups(),
            Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health(Reachability::Up, Reachability::Up),
        );
        assert_eq!(d.route, Route::Direct);
        assert!(!d.demoted);
    }
}
