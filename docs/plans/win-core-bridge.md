# ProxyPilot Windows — план 1: ядро и мост

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** рабочий локальный HTTP-CONNECT прокси на Rust, который уводит трафик наружу через SOCKS5, вышестоящий HTTP-прокси или напрямую, и умеет менять маршрут на лету не разрывая живые соединения.

**Architecture:** два крейта в воркспейсе. `proxypilot-core` — чистая логика без ввода-вывода (выбор маршрута, bypass-матчер, конфиг), тестируется табличными тестами. `proxypilot-bridge` — асинхронный мост на tokio: разбор запроса, коннекторы, атомарная смена маршрута через `ArcSwap`. Плюс тонкий бинарь для ручной проверки через `curl -x`.

**Tech Stack:** Rust 2021, tokio, arc-swap, serde + toml, thiserror.

**Spec:** [`docs/superpowers/specs/2026-08-30-proxypilot-windows-rust-design.md`](../specs/2026-08-30-proxypilot-windows-rust-design.md)

**Размещение:** код живёт в `win/` внутри этого репозитория, рядом с существующими `bin/` (macOS CLI) и `app/` (macOS приложение). Спека и план — в `docs/superpowers/`. Если позже решим вынести в отдельный репозиторий, каталог самодостаточен и переносится целиком.

## Global Constraints

Требования из спеки, действующие для **всех** задач плана:

- Слушатель моста привязывается **строго к `127.0.0.1`**, никогда к `0.0.0.0` — иначе получается открытый прокси для всей локальной сети (спека 5.1).
- Таймаут набора апстрима — **3 с**, настраиваемый (спека 5.6). Именно ради этого мы ушли от gost, где он зашит в 15 с.
- Таймаут чтения заголовков запроса — **10 с** (спека 5.6).
- **Таймаута простоя нет.** Туннель может легитимно молчать: long-poll, websocket, ssh. Полагаемся на TCP (спека 5.6).
- Предел одновременных соединений — **512**, сверх лимита клиент получает `503` (спека 5.6).
- **Молчаливого перехода на direct для отдельного соединения нет.** Ошибка набора апстрима — это `502` клиенту, а не тихий обход: тихий обход означал бы утечку трафика мимо выбранного маршрута (спека 5.7).
- SOCKS5-коннектор передаёт апстриму **имя хоста**, а не разрешённый адрес (семантика `socks5h`) — внутренние имена должен резолвить апстрим (спека 5.4).
- **Авторизация на апстриме не поддерживается** в этой версии. Продукт не хранит секретов вообще, и это свойство сохраняем (спека 5.4).
- Обычный HTTP (не CONNECT) обслуживается **без keep-alive**: ответ получает `Connection: close` (спека 5.3).
- Смена маршрута **не трогает установленные соединения** — они доживают по старому пути; новый маршрут действует только для новых (спека 5.5). Это центральное свойство, ради которого писался свой мост.
- Rust edition 2021, `rust-version = "1.75"`.
- CI обязан проходить `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.

---

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

### Task 2: Bypass-матчер

**Files:**
- Create: `win/crates/core/src/bypass.rs`
- Modify: `win/crates/core/src/lib.rs`

**Interfaces:**
- Consumes: ничего из предыдущих задач.
- Produces: `BypassList::parse(list: &str) -> BypassList`, `BypassList::matches(&self, host: &str) -> bool`. Мост вызывает `matches` для каждого соединения.

- [ ] **Step 1: Написать падающий тест**

`win/crates/core/src/bypass.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "localhost,127.0.0.1,::1,.local,192.168.0.0/16,10.0.0.0/8,git.company.kz";

    fn list() -> BypassList {
        BypassList::parse(LIST)
    }

    #[test]
    fn exact_hostname_matches() {
        assert!(list().matches("localhost"));
        assert!(list().matches("git.company.kz"));
    }

    #[test]
    fn exact_hostname_is_case_insensitive() {
        assert!(list().matches("LocalHost"));
        assert!(list().matches("GIT.Company.KZ"));
    }

    #[test]
    fn dot_suffix_matches_subdomains_only() {
        assert!(list().matches("printer.local"));
        assert!(list().matches("a.b.local"));
        // сам суффикс без метки слева — не совпадение
        assert!(!list().matches("local"));
    }

    #[test]
    fn ip_literal_matches() {
        assert!(list().matches("127.0.0.1"));
        assert!(list().matches("::1"));
    }

    #[test]
    fn cidr_matches_addresses_inside() {
        assert!(list().matches("203.0.113.246"));
        assert!(list().matches("10.20.30.40"));
    }

    #[test]
    fn cidr_does_not_match_outside() {
        assert!(!list().matches("172.16.0.1"));
        assert!(!list().matches("8.8.8.8"));
    }

    #[test]
    fn cidr_never_matches_a_hostname() {
        // Имя не адрес: «192.168.0.0/16» не должно ловить «example.com».
        assert!(!list().matches("example.com"));
        assert!(!list().matches("api.anthropic.com"));
    }

    #[test]
    fn empty_and_blank_entries_are_ignored() {
        let l = BypassList::parse("localhost, ,,  ,127.0.0.1");
        assert!(l.matches("localhost"));
        assert!(!l.matches("anything.else"));
    }

    #[test]
    fn empty_list_matches_nothing() {
        let l = BypassList::parse("");
        assert!(!l.matches("localhost"));
    }

    #[test]
    fn bracketed_ipv6_host_is_unwrapped() {
        // В CONNECT адрес приходит как [::1]:443
        assert!(list().matches("[::1]"));
    }

    #[test]
    fn zero_prefix_cidr_matches_every_ipv4() {
        // /0 не должен паниковать на сдвиге на 32
        let l = BypassList::parse("0.0.0.0/0");
        assert!(l.matches("8.8.8.8"));
        assert!(!l.matches("example.com"));
    }

    #[test]
    fn full_prefix_cidr_matches_single_address() {
        let l = BypassList::parse("203.0.113.246/32");
        assert!(l.matches("203.0.113.246"));
        assert!(!l.matches("203.0.113.247"));
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-core bypass`
Expected: FAIL — `BypassList` не определён.

- [ ] **Step 3: Написать минимальную реализацию**

Вставь в начало `win/crates/core/src/bypass.rs`:

```rust
//! Какие адреса идут мимо апстрима.
//!
//! Правило живёт здесь, в мосте, а не в клиентах — и это осознанно.
//! Node/Bun и python-requests не понимают CIDR (только точное имя или
//! суффикс с точкой), а часть приложений вообще перетирает NO_PROXY своим
//! списком. Мост — единственное место, где список соблюдается гарантированно.

use std::net::{IpAddr, Ipv4Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    /// точное имя хоста, в нижнем регистре
    Exact(String),
    /// суффикс с ведущей точкой, в нижнем регистре: ".local"
    Suffix(String),
    /// IPv4-подсеть: адрес сети и длина префикса
    Cidr4 { net: u32, bits: u32 },
    /// конкретный адрес
    Ip(IpAddr),
}

#[derive(Debug, Clone, Default)]
pub struct BypassList {
    entries: Vec<Entry>,
}

impl BypassList {
    /// Разбирает список через запятую. Нераспознанные элементы трактуются
    /// как имена хостов — молча игнорировать их было бы хуже.
    pub fn parse(list: &str) -> Self {
        let mut entries = Vec::new();
        for raw in list.split(',') {
            let e = raw.trim();
            if e.is_empty() {
                continue;
            }
            if let Some((net, bits)) = e.split_once('/') {
                if let (Ok(ip), Ok(b)) = (net.parse::<Ipv4Addr>(), bits.parse::<u32>()) {
                    if b <= 32 {
                        entries.push(Entry::Cidr4 { net: u32::from(ip), bits: b });
                        continue;
                    }
                }
            }
            if let Ok(ip) = e.parse::<IpAddr>() {
                entries.push(Entry::Ip(ip));
                continue;
            }
            if let Some(sfx) = e.strip_prefix('.') {
                entries.push(Entry::Suffix(format!(".{}", sfx.to_ascii_lowercase())));
                continue;
            }
            entries.push(Entry::Exact(e.to_ascii_lowercase()));
        }
        Self { entries }
    }

    /// `host` — имя или адрес без порта. Скобки вокруг IPv6 снимаются.
    pub fn matches(&self, host: &str) -> bool {
        let h = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let ip = h.parse::<IpAddr>().ok();

        self.entries.iter().any(|e| match e {
            Entry::Exact(s) => h == *s,
            Entry::Suffix(s) => h.ends_with(s.as_str()),
            Entry::Ip(a) => ip == Some(*a),
            Entry::Cidr4 { net, bits } => match ip {
                Some(IpAddr::V4(v4)) => {
                    // сдвиг на 32 — паника в debug, поэтому /0 отдельно
                    let mask = if *bits == 0 { 0 } else { u32::MAX << (32 - bits) };
                    (u32::from(v4) & mask) == (*net & mask)
                }
                _ => false,
            },
        })
    }
}
```

`win/crates/core/src/lib.rs`:

```rust
pub mod bypass;
pub mod mode;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-core`
Expected: PASS, 22 теста.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/core
git commit -m "feat(win): bypass-матчер — имя, суффикс, CIDR, IP"
```

---

### Task 3: Конфигурация

**Files:**
- Create: `win/crates/core/src/config.rs`
- Modify: `win/crates/core/src/lib.rs`

**Interfaces:**
- Consumes: `Mode` из задачи 1.
- Produces: `Config` с полями `bridge_port: u16`, `mode: Mode`, `socks_upstream: Option<String>`, `http_upstream: Option<String>`, `no_proxy: String`, `dial_timeout_ms: u64`, `head_timeout_ms: u64`, `max_connections: usize`; `Config::default()`, `Config::from_toml(&str) -> Result<Config, ConfigError>`, `Config::to_toml(&self) -> String`, `Config::upstreams(&self) -> Upstreams`, `validate_upstream(&str) -> bool`.

- [ ] **Step 1: Написать падающий тест**

`win/crates/core/src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Mode;

    #[test]
    fn defaults_match_the_spec() {
        let c = Config::default();
        assert_eq!(c.bridge_port, 3129);
        assert_eq!(c.mode, Mode::Auto);
        assert_eq!(c.dial_timeout_ms, 3000);
        assert_eq!(c.head_timeout_ms, 10_000);
        assert_eq!(c.max_connections, 512);
        assert!(c.socks_upstream.is_none());
        assert!(c.http_upstream.is_none());
    }

    #[test]
    fn default_no_proxy_covers_local_ranges() {
        let c = Config::default();
        for host in ["localhost", "127.0.0.1", "printer.local", "203.0.113.1", "10.1.2.3"] {
            assert!(
                crate::bypass::BypassList::parse(&c.no_proxy).matches(host),
                "{host} должен быть в bypass по умолчанию"
            );
        }
    }

    #[test]
    fn roundtrip_through_toml_preserves_everything() {
        let mut c = Config::default();
        c.socks_upstream = Some("203.0.113.10:9999".into());
        c.http_upstream = Some("203.0.113.10:3128".into());
        c.mode = Mode::Socks;
        c.bridge_port = 3130;

        let parsed = Config::from_toml(&c.to_toml()).expect("должен разобраться");
        assert_eq!(parsed.socks_upstream, c.socks_upstream);
        assert_eq!(parsed.http_upstream, c.http_upstream);
        assert_eq!(parsed.mode, c.mode);
        assert_eq!(parsed.bridge_port, c.bridge_port);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // Конфиг мог прийти от версии постарше — недостающее берём из дефолтов.
        let c = Config::from_toml("bridge_port = 3131").expect("должен разобраться");
        assert_eq!(c.bridge_port, 3131);
        assert_eq!(c.mode, Mode::Auto);
        assert_eq!(c.max_connections, 512);
    }

    #[test]
    fn broken_toml_is_an_error_not_a_panic() {
        assert!(Config::from_toml("это не toml =").is_err());
    }

    #[test]
    fn upstream_format_is_validated() {
        assert!(validate_upstream("203.0.113.10:9999"));
        assert!(validate_upstream("proxy.company.kz:3128"));
        assert!(!validate_upstream("203.0.113.10"));
        assert!(!validate_upstream("203.0.113.10:"));
        assert!(!validate_upstream("203.0.113.10:0"));
        assert!(!validate_upstream("203.0.113.10:70000"));
        assert!(!validate_upstream(""));
    }

    #[test]
    fn upstreams_view_is_built_from_config() {
        let mut c = Config::default();
        c.socks_upstream = Some("10.0.0.2:9999".into());
        let u = c.upstreams();
        assert_eq!(u.socks.as_deref(), Some("10.0.0.2:9999"));
        assert!(u.http.is_none());
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-core config`
Expected: FAIL — `Config` не определён.

- [ ] **Step 3: Написать минимальную реализацию**

Вставь в начало `win/crates/core/src/config.rs`:

```rust
//! Конфигурация.
//!
//! TOML, а не KEY=VALUE как на macOS: тот формат был продиктован тем, что
//! файл читал шелл. Здесь этого ограничения нет, а свойство безопасности
//! («конфиг разбирается, но никогда не исполняется») достаётся бесплатно.

use serde::{Deserialize, Serialize};

use crate::mode::{Mode, Upstreams};

pub const DEFAULT_NO_PROXY: &str = "localhost,127.0.0.1,::1,.local,\
169.254.0.0/16,192.168.0.0/16,10.0.0.0/8,172.16.0.0/12";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub bridge_port: u16,
    pub mode: Mode,
    pub socks_upstream: Option<String>,
    pub http_upstream: Option<String>,
    pub no_proxy: String,
    pub dial_timeout_ms: u64,
    pub head_timeout_ms: u64,
    pub max_connections: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bridge_port: 3129,
            mode: Mode::Auto,
            socks_upstream: None,
            http_upstream: None,
            no_proxy: DEFAULT_NO_PROXY.to_string(),
            dial_timeout_ms: 3000,
            head_timeout_ms: 10_000,
            max_connections: 512,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("не разобрался конфиг: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("не сериализовался конфиг: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("Config всегда сериализуем")
    }

    pub fn upstreams(&self) -> Upstreams {
        Upstreams {
            socks: self.socks_upstream.clone(),
            http: self.http_upstream.clone(),
        }
    }
}

/// Апстрим задаётся как `host:port`. Порт обязателен и должен быть валидным:
/// пустая строка означает «режим выключен» и проверяется отдельно вызывающим.
pub fn validate_upstream(s: &str) -> bool {
    let Some((host, port)) = s.rsplit_once(':') else {
        return false;
    };
    if host.is_empty() {
        return false;
    }
    matches!(port.parse::<u16>(), Ok(p) if p > 0)
}
```

Добавь в `mode.rs` производные для сериализации — без них `Config` не соберётся. Замени строку `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` над `enum Mode` на:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
```

`win/crates/core/src/lib.rs`:

```rust
pub mod bypass;
pub mod config;
pub mod mode;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-core`
Expected: PASS, 29 тестов.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/core
git commit -m "feat(win): конфигурация в TOML с валидацией апстримов"
```

---

### Task 4: Держатель маршрута (атомарная смена)

**Files:**
- Create: `win/crates/bridge/Cargo.toml`
- Create: `win/crates/bridge/src/lib.rs`
- Create: `win/crates/bridge/src/router.rs`
- Modify: `win/Cargo.toml` (вернуть `crates/bridge` в `members`)

**Interfaces:**
- Consumes: `Route` из задачи 1.
- Produces: `Router::new(Route) -> Router`, `Router::get(&self) -> Arc<Route>`, `Router::set(&self, Route)`.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/router.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn returns_the_route_it_was_built_with() {
        let r = Router::new(Route::Direct);
        assert_eq!(*r.get(), Route::Direct);
    }

    #[test]
    fn set_replaces_the_route_for_later_readers() {
        let r = Router::new(Route::Direct);
        r.set(Route::Socks("10.0.0.2:9999".into()));
        assert_eq!(*r.get(), Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn a_handle_taken_before_set_keeps_the_old_route() {
        // Это и есть свойство, ради которого писался свой мост: соединение
        // взяло маршрут в момент установки и доживает по нему, что бы ни
        // переключили потом.
        let r = Router::new(Route::Socks("old:1080".into()));
        let held = r.get();
        r.set(Route::Direct);
        assert_eq!(*held, Route::Socks("old:1080".into()));
        assert_eq!(*r.get(), Route::Direct);
    }

    #[test]
    fn is_shareable_across_threads() {
        let r = Arc::new(Router::new(Route::Direct));
        let r2 = Arc::clone(&r);
        let t = std::thread::spawn(move || {
            r2.set(Route::Http("p:3128".into()));
        });
        t.join().unwrap();
        assert_eq!(*r.get(), Route::Http("p:3128".into()));
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: FAIL — крейта `proxypilot-bridge` ещё нет.

- [ ] **Step 3: Написать минимальную реализацию**

`win/crates/bridge/Cargo.toml`:

```toml
[package]
name = "proxypilot-bridge"
edition.workspace = true
rust-version.workspace = true
version.workspace = true

[dependencies]
proxypilot-core = { path = "../core" }
tokio = { workspace = true }
arc-swap = { workspace = true }
thiserror = { workspace = true }
```

Верни `crates/bridge` в `members` в `win/Cargo.toml`.

Вставь в начало `win/crates/bridge/src/router.rs`:

```rust
//! Текущий маршрут моста.
//!
//! Смена маршрута обязана быть атомарной и НЕ трогать установленные
//! соединения: каждое соединение читает маршрут один раз, в момент приёма,
//! и дальше живёт с этим значением. Именно поэтому в macOS-версии был нужен
//! трёхуровневый антифлаппинг — там смена режима перезапускала gost и рвала
//! всё живое. Здесь рвать нечего.

use std::sync::Arc;

use arc_swap::ArcSwap;
use proxypilot_core::mode::Route;

#[derive(Debug)]
pub struct Router {
    current: ArcSwap<Route>,
}

impl Router {
    pub fn new(route: Route) -> Self {
        Self { current: ArcSwap::from_pointee(route) }
    }

    /// Снимок маршрута. Держатель снимка не заметит последующих `set`.
    pub fn get(&self) -> Arc<Route> {
        self.current.load_full()
    }

    pub fn set(&self, route: Route) {
        self.current.store(Arc::new(route));
    }
}
```

`win/crates/bridge/src/lib.rs`:

```rust
pub mod router;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 4 теста.

- [ ] **Step 5: Коммит**

```bash
git add win/Cargo.toml win/crates/bridge
git commit -m "feat(win): держатель маршрута с атомарной сменой"
```

---

### Task 5: Разбор заголовка запроса

**Files:**
- Create: `win/crates/bridge/src/http.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: ничего.
- Produces: `Head { method: String, target: String, version: String, headers: Vec<(String, String)>, leftover: Vec<u8> }`, `read_head<R: AsyncRead + Unpin>(&mut R, max: usize) -> Result<Head, HeadError>`, `split_host_port(&str, u16) -> Option<(String, u16)>`, `Head::is_connect(&self) -> bool`.

**Критично:** читая заголовок, мы почти всегда захватываем лишние байты — клиенты присылают TLS ClientHello сразу за `CONNECT`, не дожидаясь ответа. Эти байты обязаны сохраниться в `leftover` и уйти в апстрим первыми, иначе рукопожатие TLS сломается. Это самая частая ошибка в самописных прокси.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/http.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    async fn head_of(input: &[u8]) -> Result<Head, HeadError> {
        let mut cursor = std::io::Cursor::new(input.to_vec());
        read_head(&mut cursor, 8192).await
    }

    #[tokio::test]
    async fn parses_connect() {
        let h = head_of(b"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: api.anthropic.com:443\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(h.method, "CONNECT");
        assert_eq!(h.target, "api.anthropic.com:443");
        assert_eq!(h.version, "HTTP/1.1");
        assert!(h.is_connect());
        assert_eq!(h.headers.len(), 1);
        assert!(h.leftover.is_empty());
    }

    #[tokio::test]
    async fn keeps_bytes_that_follow_the_head() {
        // Клиент шлёт TLS ClientHello, не дожидаясь нашего 200. Потерять эти
        // байты — сломать рукопожатие.
        let h = head_of(b"CONNECT h:443 HTTP/1.1\r\n\r\n\x16\x03\x01ABC")
            .await
            .unwrap();
        assert_eq!(h.leftover, b"\x16\x03\x01ABC");
    }

    #[tokio::test]
    async fn parses_absolute_form_request() {
        let h = head_of(b"GET http://example.com/a?b=1 HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(h.method, "GET");
        assert_eq!(h.target, "http://example.com/a?b=1");
        assert!(!h.is_connect());
    }

    #[tokio::test]
    async fn header_names_keep_value_spacing_trimmed() {
        let h = head_of(b"CONNECT h:443 HTTP/1.1\r\nProxy-Connection:   keep-alive  \r\n\r\n")
            .await
            .unwrap();
        assert_eq!(h.headers[0].0, "Proxy-Connection");
        assert_eq!(h.headers[0].1, "keep-alive");
    }

    #[tokio::test]
    async fn oversized_head_is_rejected() {
        let mut big = b"CONNECT h:443 HTTP/1.1\r\n".to_vec();
        big.extend(std::iter::repeat(b'x').take(9000));
        let mut cursor = std::io::Cursor::new(big);
        assert!(matches!(read_head(&mut cursor, 4096).await, Err(HeadError::TooLarge)));
    }

    #[tokio::test]
    async fn truncated_input_is_an_error() {
        assert!(matches!(
            head_of(b"CONNECT h:443 HTTP/1.1\r\n").await,
            Err(HeadError::Truncated)
        ));
    }

    #[tokio::test]
    async fn garbage_request_line_is_an_error() {
        assert!(matches!(head_of(b"nonsense\r\n\r\n").await, Err(HeadError::Malformed)));
    }

    #[tokio::test]
    async fn parses_a_response_status_line_too() {
        // Тот же разбор используется для ответа вышестоящего HTTP-прокси на
        // наш CONNECT (задача 7). Разбор позиционный, поэтому у ответа
        // «HTTP/1.1 200 OK» версия попадает в method, а код — в target.
        let h = head_of(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
        assert_eq!(h.method, "HTTP/1.1");
        assert_eq!(h.target, "200");
    }

    #[test]
    fn splits_host_and_port() {
        assert_eq!(split_host_port("h:443", 80), Some(("h".into(), 443)));
        assert_eq!(split_host_port("h", 80), Some(("h".into(), 80)));
    }

    #[test]
    fn splits_bracketed_ipv6() {
        assert_eq!(split_host_port("[::1]:443", 80), Some(("::1".into(), 443)));
        assert_eq!(split_host_port("[::1]", 80), Some(("::1".into(), 80)));
    }

    #[test]
    fn rejects_bad_port() {
        assert_eq!(split_host_port("h:0", 80), None);
        assert_eq!(split_host_port("h:99999", 80), None);
        assert_eq!(split_host_port("h:abc", 80), None);
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge http`
Expected: FAIL — `read_head` не определён.

- [ ] **Step 3: Написать минимальную реализацию**

Добавь в `win/crates/bridge/Cargo.toml` в `[dev-dependencies]`:

```toml
[dev-dependencies]
tokio = { workspace = true, features = ["rt", "macros", "io-util"] }
```

Вставь в начало `win/crates/bridge/src/http.rs`:

```rust
//! Разбор заголовка запроса от клиента.
//!
//! Нам нужны только строка запроса и заголовки — тело мы не интерпретируем,
//! а переливаем. Ключевая тонкость: читая заголовок, мы почти всегда
//! захватываем часть следующих байтов (клиенты шлют TLS ClientHello сразу
//! за CONNECT, не дожидаясь ответа). Они сохраняются в `leftover` и уходят
//! в апстрим первыми.

use tokio::io::{AsyncRead, AsyncReadExt};

const TERMINATOR: &[u8] = b"\r\n\r\n";

#[derive(Debug, thiserror::Error)]
pub enum HeadError {
    #[error("заголовок больше допустимого")]
    TooLarge,
    #[error("соединение закрылось до конца заголовка")]
    Truncated,
    #[error("нечитаемая строка запроса")]
    Malformed,
    #[error("ошибка чтения: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct Head {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
    /// байты, прочитанные сверх заголовка
    pub leftover: Vec<u8>,
}

impl Head {
    pub fn is_connect(&self) -> bool {
        self.method.eq_ignore_ascii_case("CONNECT")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub async fn read_head<R: AsyncRead + Unpin>(r: &mut R, max: usize) -> Result<Head, HeadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    loop {
        if let Some(pos) = find_terminator(&buf) {
            let body_at = pos + TERMINATOR.len();
            let leftover = buf[body_at..].to_vec();
            let mut head = parse(&buf[..pos])?;
            head.leftover = leftover;
            return Ok(head);
        }
        if buf.len() >= max {
            return Err(HeadError::TooLarge);
        }
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Err(HeadError::Truncated);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn find_terminator(buf: &[u8]) -> Option<usize> {
    buf.windows(TERMINATOR.len()).position(|w| w == TERMINATOR)
}

/// Разбор позиционный и намеренно годится и для строки ЗАПРОСА
/// (`CONNECT host:443 HTTP/1.1`), и для строки ОТВЕТА
/// (`HTTP/1.1 200 OK`) — последняя нужна задаче 7, чтобы прочитать ответ
/// вышестоящего HTTP-прокси на наш CONNECT. У ответа версия оказывается
/// в `method`, а код статуса — в `target`.
fn parse(head: &[u8]) -> Result<Head, HeadError> {
    let text = std::str::from_utf8(head).map_err(|_| HeadError::Malformed)?;
    let mut lines = text.split("\r\n");

    let first_line = lines.next().ok_or(HeadError::Malformed)?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().ok_or(HeadError::Malformed)?.to_string();
    let target = parts.next().ok_or(HeadError::Malformed)?.to_string();
    let version = parts.next().unwrap_or("").to_string();
    // «HTTP/x» обязан присутствовать: в запросе третьим полем, в ответе первым
    if !method.starts_with("HTTP/") && !version.starts_with("HTTP/") {
        return Err(HeadError::Malformed);
    }

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':').ok_or(HeadError::Malformed)?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }

    Ok(Head { method, target, version, headers, leftover: Vec::new() })
}

/// Разбирает `host:port`. IPv6 приходит в скобках: `[::1]:443`.
/// Порт 0 и вне диапазона отвергаются.
pub fn split_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => parse_port(p)?,
            None if tail.is_empty() => default_port,
            None => return None,
        };
        return Some((host.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Some((host.to_string(), parse_port(port)?)),
        Some(_) => None,
        None => Some((s.to_string(), default_port)),
    }
}

fn parse_port(s: &str) -> Option<u16> {
    match s.parse::<u16>() {
        Ok(p) if p > 0 => Some(p),
        _ => None,
    }
}
```

`win/crates/bridge/src/lib.rs`:

```rust
pub mod http;
pub mod router;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 15 тестов.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): разбор заголовка запроса с сохранением хвоста"
```

---

### Task 6: Клиент SOCKS5

**Files:**
- Create: `win/crates/bridge/src/socks5.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: ничего.
- Produces: `socks5_handshake<S: AsyncRead + AsyncWrite + Unpin>(&mut S, host: &str, port: u16) -> Result<(), Socks5Error>`, `Socks5Error`.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/socks5.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Фальшивый SOCKS5-сервер: отвечает заранее заданными байтами и
    /// возвращает всё, что ему прислал клиент.
    async fn fake_server(reply_greeting: Vec<u8>, reply_connect: Vec<u8>) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut b = [0u8; 3];
            s.read_exact(&mut b).await.unwrap();
            seen.extend_from_slice(&b);
            s.write_all(&reply_greeting).await.unwrap();
            if reply_greeting.get(1) == Some(&0x00) {
                let mut hdr = [0u8; 5];
                s.read_exact(&mut hdr).await.unwrap();
                seen.extend_from_slice(&hdr);
                let mut rest = vec![0u8; hdr[4] as usize + 2];
                s.read_exact(&mut rest).await.unwrap();
                seen.extend_from_slice(&rest);
                s.write_all(&reply_connect).await.unwrap();
            }
            seen
        });
        (addr, h)
    }

    #[tokio::test]
    async fn sends_hostname_not_resolved_address() {
        // socks5h: резолвить должен апстрим — иначе внутренние имена
        // офиса не разрешатся на нашей стороне.
        let (addr, h) = fake_server(vec![0x05, 0x00], vec![0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        socks5_handshake(&mut s, "git.company.kz", 443).await.unwrap();

        let seen = h.await.unwrap();
        assert_eq!(&seen[0..3], &[0x05, 0x01, 0x00]);
        assert_eq!(&seen[3..8], &[0x05, 0x01, 0x00, 0x03, 14]);
        assert_eq!(&seen[8..22], b"git.company.kz");
        assert_eq!(&seen[22..24], &443u16.to_be_bytes());
    }

    #[tokio::test]
    async fn accepts_ipv4_bound_address_in_reply() {
        let (addr, _h) = fake_server(vec![0x05, 0x00], vec![0x05, 0x00, 0x00, 0x01, 1, 2, 3, 4, 0, 80]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(socks5_handshake(&mut s, "h", 80).await.is_ok());
    }

    #[tokio::test]
    async fn accepts_domain_bound_address_in_reply() {
        let mut reply = vec![0x05, 0x00, 0x00, 0x03, 3];
        reply.extend_from_slice(b"abc");
        reply.extend_from_slice(&[0, 80]);
        let (addr, _h) = fake_server(vec![0x05, 0x00], reply).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(socks5_handshake(&mut s, "h", 80).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_server_demanding_auth() {
        // Мы не храним секретов, поэтому апстрим с логином не поддерживаем —
        // но обязаны сказать это внятно, а не зависнуть.
        let (addr, _h) = fake_server(vec![0x05, 0x02], vec![]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(matches!(
            socks5_handshake(&mut s, "h", 80).await,
            Err(Socks5Error::AuthRequired(0x02))
        ));
    }

    #[tokio::test]
    async fn surfaces_refusal_code() {
        let (addr, _h) = fake_server(vec![0x05, 0x00], vec![0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(matches!(
            socks5_handshake(&mut s, "h", 80).await,
            Err(Socks5Error::Refused(0x05))
        ));
    }

    #[tokio::test]
    async fn rejects_non_socks5_greeting() {
        let (addr, _h) = fake_server(vec![0x04, 0x00], vec![]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(matches!(
            socks5_handshake(&mut s, "h", 80).await,
            Err(Socks5Error::BadVersion(0x04))
        ));
    }

    #[tokio::test]
    async fn rejects_overlong_hostname() {
        let (addr, _h) = fake_server(vec![0x05, 0x00], vec![]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        let long = "a".repeat(256);
        assert!(matches!(
            socks5_handshake(&mut s, &long, 80).await,
            Err(Socks5Error::HostTooLong)
        ));
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge socks5`
Expected: FAIL — `socks5_handshake` не определён.

- [ ] **Step 3: Написать минимальную реализацию**

Вставь в начало `win/crates/bridge/src/socks5.rs`:

```rust
//! Клиентская сторона SOCKS5 — ровно столько, сколько нужно мосту.
//!
//! Адрес назначения передаётся ИМЕНЕМ (ATYP=0x03), а не разрешённым
//! адресом: резолвить должен апстрим, иначе внутренние имена офиса не
//! разрешатся на нашей стороне. Это семантика `socks5h`.
//!
//! Авторизация не поддерживается сознательно: продукт не хранит секретов.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, thiserror::Error)]
pub enum Socks5Error {
    #[error("апстрим ответил версией {0:#04x}, а не SOCKS5")]
    BadVersion(u8),
    #[error("апстрим требует авторизацию (метод {0:#04x}), а мы её не поддерживаем")]
    AuthRequired(u8),
    #[error("апстрим отказал, код {0:#04x}")]
    Refused(u8),
    #[error("апстрим вернул неизвестный тип адреса {0:#04x}")]
    BadAtyp(u8),
    #[error("имя хоста длиннее 255 байт")]
    HostTooLong,
    #[error("ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn socks5_handshake<S>(s: &mut S, host: &str, port: u16) -> Result<(), Socks5Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let host_bytes = host.as_bytes();
    if host_bytes.len() > 255 {
        return Err(Socks5Error::HostTooLong);
    }

    // приветствие: версия 5, один метод, «без авторизации»
    s.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut greeting = [0u8; 2];
    s.read_exact(&mut greeting).await?;
    if greeting[0] != 0x05 {
        return Err(Socks5Error::BadVersion(greeting[0]));
    }
    if greeting[1] != 0x00 {
        return Err(Socks5Error::AuthRequired(greeting[1]));
    }

    // запрос CONNECT с именем хоста
    let mut req = Vec::with_capacity(7 + host_bytes.len());
    req.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8]);
    req.extend_from_slice(host_bytes);
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await?;

    let mut reply = [0u8; 4];
    s.read_exact(&mut reply).await?;
    if reply[0] != 0x05 {
        return Err(Socks5Error::BadVersion(reply[0]));
    }
    if reply[1] != 0x00 {
        return Err(Socks5Error::Refused(reply[1]));
    }

    // адрес привязки нам не нужен, но вычитать его обязаны — иначе он
    // окажется в потоке данных и испортит первое же чтение
    match reply[3] {
        0x01 => {
            let mut skip = [0u8; 6];
            s.read_exact(&mut skip).await?;
        }
        0x04 => {
            let mut skip = [0u8; 18];
            s.read_exact(&mut skip).await?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut skip = vec![0u8; len[0] as usize + 2];
            s.read_exact(&mut skip).await?;
        }
        other => return Err(Socks5Error::BadAtyp(other)),
    }

    Ok(())
}
```

`win/crates/bridge/src/lib.rs`:

```rust
pub mod http;
pub mod router;
pub mod socks5;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 22 теста.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): клиент SOCKS5 с передачей имени хоста"
```

---

### Task 7: Коннекторы

**Files:**
- Create: `win/crates/bridge/src/connector.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Route` (задача 1), `socks5_handshake` (задача 6), `read_head`/`split_host_port` (задача 5).
- Produces: `ConnectError`, `connect_via(route: &Route, host: &str, port: u16, dial: Duration) -> Result<TcpStream, ConnectError>`.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/connector.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn echo_server() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut b = [0u8; 4];
            let _ = s.read_exact(&mut b).await;
            let _ = s.write_all(b"pong").await;
        });
        addr
    }

    #[tokio::test]
    async fn direct_connects_to_origin() {
        let addr = echo_server().await;
        let (host, port) = crate::http::split_host_port(&addr, 80).unwrap();
        let mut s = connect_via(&Route::Direct, &host, port, Duration::from_secs(3))
            .await
            .unwrap();
        s.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        s.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn dial_timeout_is_honoured() {
        // Слушатель принимает соединение и молчит: рукопожатие SOCKS5 никогда
        // не завершится, и сработать обязан наш таймаут. Ровно эта проблема
        // была с зашитыми в gost 15 секундами.
        //
        // Недостижимый адрес вроде 10.255.255.1 для проверки не годится:
        // поведение зависит от ОС, Windows может мгновенно ответить «сеть
        // недоступна», и тест поймал бы Origin вместо Timeout.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let _silent = tokio::spawn(async move {
            let _accepted = l.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let started = std::time::Instant::now();
        let r = connect_via(
            &Route::Socks(addr),
            "example.com",
            443,
            Duration::from_millis(300),
        )
        .await;
        let elapsed = started.elapsed();
        assert!(matches!(&r, Err(ConnectError::Timeout)), "получили: {r:?}");
        assert!(elapsed < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn refused_upstream_reports_error() {
        // порт 1 на loopback закрыт
        let r = connect_via(
            &Route::Socks("127.0.0.1:1".into()),
            "example.com",
            443,
            Duration::from_secs(2),
        )
        .await;
        assert!(matches!(r, Err(ConnectError::Upstream { .. })));
    }

    #[tokio::test]
    async fn http_upstream_sends_connect_and_accepts_200() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let seen = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 128];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });

        let s = connect_via(&Route::Http(addr), "example.com", 443, Duration::from_secs(2)).await;
        assert!(s.is_ok());
        let request = seen.await.unwrap();
        assert!(request.starts_with("CONNECT example.com:443 HTTP/1.1\r\n"));
        assert!(request.contains("Host: example.com:443\r\n"));
    }

    #[tokio::test]
    async fn http_upstream_non_200_is_an_error() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 128];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
        });

        let r = connect_via(&Route::Http(addr), "example.com", 443, Duration::from_secs(2)).await;
        assert!(matches!(r, Err(ConnectError::UpstreamStatus(403))));
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge connector`
Expected: FAIL — `connect_via` не определён.

- [ ] **Step 3: Написать минимальную реализацию**

Вставь в начало `win/crates/bridge/src/connector.rs`:

```rust
//! Набор соединения до назначения по выбранному маршруту.
//!
//! Таймаут набора — наш и настраиваемый. В macOS-версии он был зашит
//! в gost и равен 15 секундам, из-за чего каждый запрос к недоступному
//! апстриму вешал браузер на эти 15 секунд; половина логики режимов там
//! существовала только чтобы в такой апстрим не смотреть.

use std::time::Duration;

use proxypilot_core::mode::Route;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::http::{read_head, HeadError};
use crate::socks5::{socks5_handshake, Socks5Error};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("не уложились в таймаут набора")]
    Timeout,
    #[error("апстрим {addr} недоступен: {source}")]
    Upstream { addr: String, source: std::io::Error },
    #[error("не удалось соединиться с {host}:{port}: {source}")]
    Origin { host: String, port: u16, source: std::io::Error },
    #[error("апстрим-прокси ответил статусом {0}")]
    UpstreamStatus(u16),
    #[error("апстрим-прокси ответил неразборчиво: {0}")]
    UpstreamReply(#[from] HeadError),
    #[error(transparent)]
    Socks(#[from] Socks5Error),
}

pub async fn connect_via(
    route: &Route,
    host: &str,
    port: u16,
    dial: Duration,
) -> Result<TcpStream, ConnectError> {
    match tokio::time::timeout(dial, connect_inner(route, host, port)).await {
        Err(_) => Err(ConnectError::Timeout),
        Ok(r) => r,
    }
}

async fn connect_inner(route: &Route, host: &str, port: u16) -> Result<TcpStream, ConnectError> {
    match route {
        Route::Direct => TcpStream::connect((host, port))
            .await
            .map_err(|source| ConnectError::Origin { host: host.to_string(), port, source }),

        Route::Socks(addr) => {
            let mut s = dial_upstream(addr).await?;
            socks5_handshake(&mut s, host, port).await?;
            Ok(s)
        }

        Route::Http(addr) => {
            let mut s = dial_upstream(addr).await?;
            let target = format_target(host, port);
            let request = format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n\r\n"
            );
            s.write_all(request.as_bytes()).await.map_err(|source| ConnectError::Upstream {
                addr: addr.clone(),
                source,
            })?;

            let head = read_head(&mut s, 8192).await?;
            let status: u16 = head
                .target // в ответе на месте target стоит код статуса
                .parse()
                .map_err(|_| ConnectError::UpstreamStatus(0))?;
            if status != 200 {
                return Err(ConnectError::UpstreamStatus(status));
            }
            Ok(s)
        }
    }
}

async fn dial_upstream(addr: &str) -> Result<TcpStream, ConnectError> {
    TcpStream::connect(addr)
        .await
        .map_err(|source| ConnectError::Upstream { addr: addr.to_string(), source })
}

/// IPv6 в строке запроса обязан быть в скобках.
fn format_target(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
```

Ответ вышестоящего прокси читается тем же `read_head`: разбор позиционный, поэтому у строки `HTTP/1.1 200 Connection established` версия попадает в `method`, а код статуса — в `target`. Задача 5 это уже учитывает и покрыла тестом `parses_a_response_status_line_too`, менять `http.rs` не нужно.

`win/crates/bridge/src/lib.rs`:

```rust
pub mod connector;
pub mod http;
pub mod router;
pub mod socks5;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 27 тестов.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): коннекторы direct/socks5/http со своим таймаутом набора"
```

---

### Task 8: Обслуживание соединения — CONNECT

**Files:**
- Create: `win/crates/bridge/src/serve.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Router` (4), `read_head`/`split_host_port` (5), `connect_via` (7), `BypassList` (2).
- Produces: `Limits { dial: Duration, head: Duration, max_connections: usize }`, `Shared { router: Arc<Router>, bypass: Arc<BypassList>, limits: Limits }`, `serve(listener: TcpListener, shared: Arc<Shared>) -> std::io::Result<()>`, `handle(stream: TcpStream, shared: Arc<Shared>)`.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/serve.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::bypass::BypassList;
    use proxypilot_core::mode::Route;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Origin, отвечающий "pong" на любые 4 байта.
    async fn origin() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                tokio::spawn(async move {
                    let mut b = [0u8; 4];
                    if s.read_exact(&mut b).await.is_ok() {
                        let _ = s.write_all(b"pong").await;
                    }
                });
            }
        });
        addr
    }

    async fn bridge_with(route: Route, no_proxy: &str) -> (String, Arc<Shared>) {
        let shared = Arc::new(Shared {
            router: Arc::new(Router::new(route)),
            bypass: Arc::new(BypassList::parse(no_proxy)),
            limits: Limits {
                dial: Duration::from_secs(2),
                head: Duration::from_secs(2),
                max_connections: 512,
            },
        });
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let s2 = Arc::clone(&shared);
        tokio::spawn(async move { serve(l, s2).await });
        (addr, shared)
    }

    async fn connect_through(bridge: &str, target: &str) -> (TcpStream, String) {
        let mut c = TcpStream::connect(bridge).await.unwrap();
        c.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = vec![0u8; 128];
        let n = c.read(&mut buf).await.unwrap();
        (c, String::from_utf8_lossy(&buf[..n]).to_string())
    }

    #[tokio::test]
    async fn connect_direct_tunnels_bytes() {
        let target = origin().await;
        let (bridge, _) = bridge_with(Route::Direct, "").await;
        let (mut c, reply) = connect_through(&bridge, &target).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");

        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn bypassed_host_goes_direct_even_with_upstream_set() {
        // Апстрим заведомо мёртвый: если bypass не сработает, будет 502.
        let target = origin().await;
        let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "127.0.0.1").await;
        let (mut c, reply) = connect_through(&bridge, &target).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn dead_upstream_yields_502_not_a_hang() {
        let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "").await;
        let (_c, reply) = connect_through(&bridge, "example.com:443").await;
        assert!(reply.starts_with("HTTP/1.1 502"), "получили: {reply}");
    }

    #[tokio::test]
    async fn malformed_request_yields_400() {
        let (bridge, _) = bridge_with(Route::Direct, "").await;
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(b"nonsense\r\n\r\n").await.unwrap();
        let mut buf = vec![0u8; 128];
        let n = c.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 400"));
    }

    #[tokio::test]
    async fn changing_route_does_not_disturb_an_open_tunnel() {
        // ЦЕНТРАЛЬНОЕ СВОЙСТВО. Ради него писался свой мост: в macOS-версии
        // смена режима перезапускала gost и рвала всё живое, из-за чего там
        // понадобился трёхуровневый антифлаппинг.
        let target = origin().await;
        let (bridge, shared) = bridge_with(Route::Direct, "").await;
        let (mut c, reply) = connect_through(&bridge, &target).await;
        assert!(reply.starts_with("HTTP/1.1 200"));

        // переключаем маршрут на заведомо мёртвый апстрим
        shared.router.set(Route::Socks("127.0.0.1:1".into()));

        // уже открытый туннель обязан продолжать работать
        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");

        // а новое соединение — уже по новому маршруту, то есть 502
        let (_c2, reply2) = connect_through(&bridge, "example.com:443").await;
        assert!(reply2.starts_with("HTTP/1.1 502"), "получили: {reply2}");
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge serve`
Expected: FAIL — `serve`, `Shared`, `Limits` не определены.

- [ ] **Step 3: Написать минимальную реализацию**

Вставь в начало `win/crates/bridge/src/serve.rs`:

```rust
//! Приём и обслуживание клиентских соединений.
//!
//! Маршрут читается ОДИН РАЗ на соединение, в момент приёма. Дальше
//! соединение живёт с этим значением, что бы ни переключили потом —
//! именно поэтому смена маршрута не рвёт живой трафик.

use std::sync::Arc;
use std::time::Duration;

use proxypilot_core::bypass::BypassList;
use proxypilot_core::mode::Route;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use crate::connector::connect_via;
use crate::http::{read_head, split_host_port, Head};
use crate::router::Router;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub dial: Duration,
    pub head: Duration,
    pub max_connections: usize,
}

#[derive(Debug)]
pub struct Shared {
    pub router: Arc<Router>,
    pub bypass: Arc<BypassList>,
    pub limits: Limits,
}

pub async fn serve(listener: TcpListener, shared: Arc<Shared>) -> std::io::Result<()> {
    let permits = Arc::new(Semaphore::new(shared.limits.max_connections));
    loop {
        let (stream, _) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        let permits = Arc::clone(&permits);

        tokio::spawn(async move {
            // Превышение лимита — честный отказ, а не молчаливое исчерпание
            // ресурсов: клиент должен узнать, что произошло.
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                let mut s = stream;
                let _ = respond(&mut s, 503, "too many connections").await;
                return;
            };
            handle(stream, shared).await;
            drop(permit);
        });
    }
}

pub async fn handle(mut client: TcpStream, shared: Arc<Shared>) {
    let head = match tokio::time::timeout(
        shared.limits.head,
        read_head(&mut client, 16 * 1024),
    )
    .await
    {
        Err(_) => {
            let _ = respond(&mut client, 408, "request head timed out").await;
            return;
        }
        Ok(Err(e)) => {
            let _ = respond(&mut client, 400, &format!("bad request: {e}")).await;
            return;
        }
        Ok(Ok(h)) => h,
    };

    if head.is_connect() {
        handle_connect(client, head, shared).await;
    } else {
        let _ = respond(&mut client, 501, "plain HTTP not implemented yet").await;
    }
}

async fn handle_connect(mut client: TcpStream, head: Head, shared: Arc<Shared>) {
    let Some((host, port)) = split_host_port(&head.target, 443) else {
        let _ = respond(&mut client, 400, "bad CONNECT target").await;
        return;
    };

    // Снимок маршрута на всё время жизни соединения.
    let route = pick_route(&host, &shared);

    let mut upstream = match connect_via(&route, &host, port, shared.limits.dial).await {
        Ok(s) => s,
        Err(e) => {
            // Тихого перехода на direct здесь нет и быть не должно: это была
            // бы утечка трафика мимо выбранного маршрута. Клиент получает
            // внятную ошибку, решение о смене маршрута принимает core.
            let _ = respond(&mut client, 502, &format!("upstream: {e}")).await;
            return;
        }
    };

    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }

    // Байты, захваченные вместе с заголовком (обычно TLS ClientHello),
    // обязаны уйти первыми — иначе рукопожатие сломается.
    if !head.leftover.is_empty() && upstream.write_all(&head.leftover).await.is_err() {
        return;
    }

    // Таймаута простоя нет сознательно: long-poll, websocket и ssh молчат
    // законно. Полагаемся на TCP.
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

fn pick_route(host: &str, shared: &Shared) -> Route {
    if shared.bypass.matches(host) {
        Route::Direct
    } else {
        (*shared.router.get()).clone()
    }
}

async fn respond(s: &mut TcpStream, code: u16, reason: &str) -> std::io::Result<()> {
    let body = format!("proxypilot: {reason}\n");
    let head = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status_text(code),
        body.len()
    );
    s.write_all(head.as_bytes()).await?;
    s.write_all(body.as_bytes()).await?;
    s.flush().await
}

fn status_text(code: u16) -> &'static str {
    match code {
        400 => "Bad Request",
        408 => "Request Timeout",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    }
}
```

`win/crates/bridge/src/lib.rs`:

```rust
pub mod connector;
pub mod http;
pub mod router;
pub mod serve;
pub mod socks5;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 32 теста. Особенно проверь, что проходит `changing_route_does_not_disturb_an_open_tunnel` — это главный тест плана.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): обслуживание CONNECT с bypass, лимитом и 502 вместо тихого обхода"
```

---

### Task 9: Обычный HTTP

**Files:**
- Modify: `win/crates/bridge/src/serve.rs`

**Interfaces:**
- Consumes: всё из задачи 8.
- Produces: `handle_plain(client: TcpStream, head: Head, shared: Arc<Shared>)`, вызывается из `handle` вместо ответа 501.

- [ ] **Step 1: Написать падающий тест**

Добавь в блок `mod tests` в `serve.rs`:

```rust
    /// Origin, отвечающий фиксированным HTTP-ответом и запоминающий запрос.
    async fn http_origin() -> (String, tokio::task::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 512];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });
        (addr, h)
    }

    #[tokio::test]
    async fn plain_http_is_forwarded_in_origin_form() {
        let (origin_addr, seen) = http_origin().await;
        let (bridge, _) = bridge_with(Route::Direct, "").await;

        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(
            format!("GET http://{origin_addr}/path?q=1 HTTP/1.1\r\nHost: {origin_addr}\r\nProxy-Connection: keep-alive\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();

        let mut buf = vec![0u8; 256];
        let n = c.read(&mut buf).await.unwrap();
        let reply = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "получили: {reply}");
        assert!(reply.ends_with("hi"));

        let request = seen.await.unwrap();
        // origin-form, а не absolute-form
        assert!(request.starts_with("GET /path?q=1 HTTP/1.1\r\n"), "origin увидел: {request}");
        // hop-by-hop заголовок не должен просочиться
        assert!(!request.to_ascii_lowercase().contains("proxy-connection"));
        // v1 работает без keep-alive
        assert!(request.contains("Connection: close"));
    }

    #[tokio::test]
    async fn plain_http_with_dead_upstream_yields_502() {
        let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "").await;
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 256];
        let n = c.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 502"));
    }

    #[tokio::test]
    async fn non_absolute_target_yields_400() {
        let (bridge, _) = bridge_with(Route::Direct, "").await;
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(b"GET /just/a/path HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 256];
        let n = c.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 400"));
    }
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge serve::tests::plain`
Expected: FAIL — сейчас отдаётся 501.

- [ ] **Step 3: Написать минимальную реализацию**

В `handle` замени ветку `else` на вызов `handle_plain`:

```rust
    if head.is_connect() {
        handle_connect(client, head, shared).await;
    } else {
        handle_plain(client, head, shared).await;
    }
```

Добавь в `serve.rs`:

```rust
/// Обычный HTTP (не CONNECT), запрос в absolute-form.
///
/// Keep-alive сознательно не поддерживается: подавляющая часть трафика —
/// HTTPS через CONNECT, и усложнять первую версию ради http:// не стоит.
/// Каждый запрос обслуживается в отдельном соединении к origin.
async fn handle_plain(mut client: TcpStream, head: Head, shared: Arc<Shared>) {
    let Some((host, port, path)) = split_absolute(&head.target) else {
        let _ = respond(&mut client, 400, "expected an absolute-form URL").await;
        return;
    };

    let route = pick_route(&host, &shared);

    let mut upstream = match connect_via(&route, &host, port, shared.limits.dial).await {
        Ok(s) => s,
        Err(e) => {
            let _ = respond(&mut client, 502, &format!("upstream: {e}")).await;
            return;
        }
    };

    let mut request = format!("{} {} {}\r\n", head.method, path, head.version);
    for (name, value) in &head.headers {
        if is_hop_by_hop(name) {
            continue;
        }
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");

    if upstream.write_all(request.as_bytes()).await.is_err() {
        let _ = respond(&mut client, 502, "upstream write failed").await;
        return;
    }
    if !head.leftover.is_empty() && upstream.write_all(&head.leftover).await.is_err() {
        return;
    }

    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

/// `http://host:port/path?q` → (host, port, "/path?q")
fn split_absolute(target: &str) -> Option<(String, u16, String)> {
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("HTTP://"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = split_host_port(authority, 80)?;
    Some((host, port, path.to_string()))
}

/// Заголовки, относящиеся к соединению с прокси, а не к запросу.
fn is_hop_by_hop(name: &str) -> bool {
    const NAMES: [&str; 8] = [
        "connection",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "keep-alive",
        "te",
        "trailer",
        "upgrade",
    ];
    let lower = name.to_ascii_lowercase();
    NAMES.contains(&lower.as_str())
}
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 35 тестов.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): обычный HTTP — origin-form, без hop-by-hop, без keep-alive"
```

---

### Task 10: Бинарь и ручная проверка

**Files:**
- Create: `win/crates/bridge/src/main.rs`
- Modify: `win/crates/bridge/Cargo.toml`

**Interfaces:**
- Consumes: `Config` (3), `decide` (1), `Router` (4), `serve`/`Shared`/`Limits` (8).
- Produces: исполняемый `proxypilot-bridge` с аргументами `--port`, `--socks`, `--http`, `--mode`, `--no-proxy`.

- [ ] **Step 1: Написать падающий тест**

Добавь `win/crates/bridge/tests/cli.rs`:

```rust
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_proxypilot-bridge")
}

#[test]
fn rejects_an_invalid_upstream() {
    let out = Command::new(bin())
        .args(["--socks", "нет-порта"])
        .output()
        .expect("бинарь должен запускаться");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("host:port"));
}

#[test]
fn prints_usage_on_help() {
    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--port"));
    assert!(text.contains("--socks"));
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge --test cli`
Expected: FAIL — бинарной цели нет.

- [ ] **Step 3: Написать минимальную реализацию**

Добавь в `win/crates/bridge/Cargo.toml`:

```toml
[[bin]]
name = "proxypilot-bridge"
path = "src/main.rs"
```

`win/crates/bridge/src/main.rs`:

```rust
//! Тонкий запускатель моста — для ручной проверки и как основа будущего
//! приложения. Разбор аргументов свой: одна зависимость меньше, а флагов
//! здесь пять.

use std::sync::Arc;
use std::time::Duration;

use proxypilot_core::bypass::BypassList;
use proxypilot_core::config::{validate_upstream, Config};
use proxypilot_core::mode::{decide, Health, Mode, Place, Reachability};
use proxypilot_bridge::router::Router;
use proxypilot_bridge::serve::{serve, Limits, Shared};
use tokio::net::TcpListener;

const USAGE: &str = "\
proxypilot-bridge — локальный HTTP-CONNECT мост

  --port <N>          порт моста (по умолчанию 3129)
  --socks <host:port> апстрим SOCKS5
  --http <host:port>  апстрим HTTP-прокси
  --mode <режим>      socks | http | direct | auto (по умолчанию auto)
  --no-proxy <список> адреса мимо апстрима, через запятую
  --help              эта справка

Клиенты ходят на http://127.0.0.1:<порт>. Смена маршрута не разрывает
установленные соединения.
";

fn main() {
    if let Err(e) = run() {
        eprintln!("proxypilot-bridge: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut cfg = Config::default();
    // Сначала в вектор, потом по индексу: замыкание-хелпер над итератором
    // здесь спорит с заимствованием, а так всё прямолинейно.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;

    while i < args.len() {
        let flag = args[i].as_str();
        // значение следующего аргумента, с внятной ошибкой если его нет
        let mut next = || {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("у {flag} нет значения"))
        };
        match flag {
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--port" => {
                cfg.bridge_port = next()?
                    .parse()
                    .map_err(|_| "порт: число 1..65535".to_string())?;
                i += 1;
            }
            "--socks" => {
                cfg.socks_upstream = Some(next()?);
                i += 1;
            }
            "--http" => {
                cfg.http_upstream = Some(next()?);
                i += 1;
            }
            "--no-proxy" => {
                cfg.no_proxy = next()?;
                i += 1;
            }
            "--mode" => {
                cfg.mode = match next()?.as_str() {
                    "socks" => Mode::Socks,
                    "http" => Mode::Http,
                    "direct" => Mode::Direct,
                    "auto" => Mode::Auto,
                    other => return Err(format!("неизвестный режим: {other}")),
                };
                i += 1;
            }
            other => return Err(format!("неизвестный аргумент: {other}")),
        }
        i += 1;
    }

    for (name, value) in [("--socks", &cfg.socks_upstream), ("--http", &cfg.http_upstream)] {
        if let Some(v) = value {
            if !validate_upstream(v) {
                return Err(format!("{name}: нужен формат host:port, получено «{v}»"));
            }
        }
    }

    // В этом плане мы ещё не умеем определять сеть — считаем, что мы в офисе,
    // и что заданные апстримы живы. Опознание сети и проба живости придут
    // в плане 2 вместе с модулем winnet.
    let health = Health {
        socks: if cfg.socks_upstream.is_some() { Reachability::Up } else { Reachability::Down },
        http: if cfg.http_upstream.is_some() { Reachability::Up } else { Reachability::Down },
    };
    let decision = decide(cfg.mode, &cfg.upstreams(), Place { in_office: true }, health);

    let shared = Arc::new(Shared {
        router: Arc::new(Router::new(decision.route.clone())),
        bypass: Arc::new(BypassList::parse(&cfg.no_proxy)),
        limits: Limits {
            dial: Duration::from_millis(cfg.dial_timeout_ms),
            head: Duration::from_millis(cfg.head_timeout_ms),
            max_connections: cfg.max_connections,
        },
    });

    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        // Строго loopback: на 0.0.0.0 это был бы открытый прокси для всей
        // локальной сети.
        let addr = format!("127.0.0.1:{}", cfg.bridge_port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("не занять {addr}: {e}"))?;
        println!("мост слушает http://{addr}, маршрут: {:?}", decision.route);
        serve(listener, shared).await.map_err(|e| e.to_string())
    })
}
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 37 тестов.

- [ ] **Step 5: Проверить руками**

В одном окне:

```bash
cd win && cargo run -p proxypilot-bridge -- --mode direct
```

В другом:

```bash
curl -x http://127.0.0.1:3129 -sS -o /dev/null -w '%{http_code}\n' https://api.anthropic.com/v1/messages
```

Expected: `401` или `405` — то есть до сервиса дошли (без ключа он так и отвечает). Код `000` означает, что мост не отработал.

Проверь и обычный HTTP:

```bash
curl -x http://127.0.0.1:3129 -sS -o /dev/null -w '%{http_code}\n' http://example.com/
```

Expected: `200`.

- [ ] **Step 6: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): бинарь моста с разбором аргументов"
```

---

### Task 11: CI

**Files:**
- Create: `.github/workflows/win.yml`

**Interfaces:**
- Consumes: весь воркспейс.
- Produces: проверку на каждый push и PR.

- [ ] **Step 1: Написать конфигурацию**

`.github/workflows/win.yml`:

```yaml
name: Windows build

on:
  push:
    branches: [main]
    paths: ["win/**", ".github/workflows/win.yml"]
  pull_request:
    paths: ["win/**", ".github/workflows/win.yml"]
  workflow_dispatch:

defaults:
  run:
    working-directory: win

jobs:
  check:
    name: Тесты и линтеры
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt

      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: win

      - name: Форматирование
        run: cargo fmt --check

      - name: Клиппи
        run: cargo clippy --all-targets -- -D warnings

      - name: Тесты
        run: cargo test --all

      - name: Сборка релиза
        run: cargo build --release -p proxypilot-bridge

      - uses: actions/upload-artifact@v4
        with:
          name: proxypilot-bridge
          path: win/target/release/proxypilot-bridge.exe
          retention-days: 14
```

- [ ] **Step 2: Проверить локально то же, что проверяет CI**

Run: `cd win && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all`
Expected: всё зелёное. Если `clippy` ругается — чини, а не глуши `#[allow]`.

- [ ] **Step 3: Коммит**

```bash
git add .github/workflows/win.yml
git commit -m "ci(win): тесты, клиппи, формат и сборка релиза"
```

---

## Что этот план НЕ делает

Осознанно вне объёма, приходит следующими планами:

- **План 2:** `winnet` — опознание офиса через NLM, события смены сети, системный прокси в реестре, проба живости апстримов; трей и переключение режимов. После него это приложение Windows, а не консольная утилита.
- **План 3:** окно настроек, `bench`, `doctor`, VPN, служба статического IP, подпись и поставка.

Из спеки в этот план сознательно не вошли: авторизация на апстриме (5.4), keep-alive для обычного HTTP (5.3), IPv6-апстримы (13), собственный резолвер (2.2 — это была болезнь macOS).
