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

