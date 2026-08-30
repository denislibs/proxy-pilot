### Task 1: Каркас воркспейса и выбор маршрута

**Files:**
- Create: `win/Cargo.toml`
- Create: `win/crates/core/Cargo.toml`
- Create: `win/crates/core/src/lib.rs`
- Create: `win/crates/core/src/mode.rs`
- Create: `win/.gitignore`

**Interfaces:**
- Consumes: ничего, это первая задача.
- Produces: `Mode`, `Route`, `Reachability`, `Upstreams`, `Health`, `Place`, `Decision`, `decide(mode: Mode, up: &Upstreams, place: Place, health: Health) -> Decision`. Все последующие задачи опираются на эти типы.

- [ ] **Step 1: Создать каркас воркспейса**

`win/Cargo.toml`:

```toml
[workspace]
members = ["crates/core", "crates/bridge"]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.75"
version = "0.1.0"

[workspace.dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "net", "io-util", "macros", "time", "sync"] }
arc-swap = "1"
serde = { version = "1", features = ["derive"] }
toml = "0.8"
thiserror = "2"
```

`win/.gitignore`:

```
/target
```

`win/crates/core/Cargo.toml`:

```toml
[package]
name = "proxypilot-core"
edition.workspace = true
rust-version.workspace = true
version.workspace = true

[dependencies]
serde = { workspace = true }
toml = { workspace = true }
thiserror = { workspace = true }
```

`win/crates/core/src/lib.rs`:

```rust
pub mod mode;
```

Крейт `bridge` пока не существует, поэтому временно убери его из `members`, оставив только `crates/core`. Вернёшь в задаче 4.

- [ ] **Step 2: Написать падающий тест**

`win/crates/core/src/mode.rs`:

```rust
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
            Place { in_office: false },
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
            Place { in_office: true },
            health(Reachability::Up, Reachability::Up),
        );
        assert_eq!(d.route, Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn auto_in_office_falls_back_to_http() {
        let d = decide(
            Mode::Auto,
            &ups(),
            Place { in_office: true },
            health(Reachability::Down, Reachability::Up),
        );
        assert_eq!(d.route, Route::Http("10.0.0.2:3128".into()));
    }

    #[test]
    fn auto_in_office_with_everything_dead_is_direct() {
        let d = decide(
            Mode::Auto,
            &ups(),
            Place { in_office: true },
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
            Place { in_office: true },
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
            Place { in_office: true },
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
            Place { in_office: false },
            health(Reachability::Up, Reachability::Up),
        );
        assert_eq!(d.route, Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn unconfigured_upstream_is_never_chosen() {
        let up = Upstreams { socks: None, http: None };
        let d = decide(
            Mode::Socks,
            &up,
            Place { in_office: true },
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
            Place { in_office: true },
            health(Reachability::Unknown, Reachability::Unknown),
        );
        assert_eq!(d.route, Route::Direct);
    }

    #[test]
    fn direct_mode_is_direct() {
        let d = decide(
            Mode::Direct,
            &ups(),
            Place { in_office: true },
            health(Reachability::Up, Reachability::Up),
        );
        assert_eq!(d.route, Route::Direct);
        assert!(!d.demoted);
    }
}
```

- [ ] **Step 3: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-core`
Expected: FAIL — компиляция не проходит, `Upstreams`, `decide` и остальные типы не определены.

- [ ] **Step 4: Написать минимальную реализацию**

Вставь в начало `win/crates/core/src/mode.rs`, перед блоком `#[cfg(test)]`:

```rust
//! Выбор маршрута: чистая функция от режима, места и живости апстримов.
//!
//! Здесь нет ни таймеров, ни кэшей, ни сети — вся защита от «дребезга»,
//! которая была в macOS-версии, существовала только потому, что смена режима
//! перезапускала внешний процесс и рвала соединения. Свой мост меняет
//! маршрут атомарно, поэтому решать можно каждый раз заново.

/// Сохранённое предпочтение пользователя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Default)]
pub struct Upstreams {
    pub socks: Option<String>,
    pub http: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Health {
    pub socks: Reachability,
    pub http: Reachability,
}

#[derive(Debug, Clone, Copy)]
pub struct Place {
    pub in_office: bool,
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
        Mode::Direct => Decision { route: Route::Direct, demoted: false },

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
            Decision { route, demoted: false }
        }

        Mode::Socks => match socks {
            Some(addr) => Decision { route: Route::Socks(addr), demoted: false },
            None => Decision { route: Route::Direct, demoted: true },
        },

        Mode::Http => match http {
            Some(addr) => Decision { route: Route::Http(addr), demoted: false },
            None => Decision { route: Route::Direct, demoted: true },
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
```

- [ ] **Step 5: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-core`
Expected: PASS, 10 тестов.

- [ ] **Step 6: Коммит**

```bash
git add win/Cargo.toml win/.gitignore win/crates/core
git commit -m "feat(win): каркас воркспейса и выбор маршрута"
```

---

