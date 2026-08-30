### Task 2: Конфиг на диске

**Files:**
- Modify: `win/Cargo.toml`
- Modify: `win/crates/core/Cargo.toml`
- Modify: `win/crates/core/src/config.rs`

**Interfaces:**
- Consumes: `Config` из плана 1.
- Produces: `Config::path() -> Option<PathBuf>`, `Config::load() -> Result<Config, ConfigError>`, `Config::load_from(&Path)`, `Config::save(&self) -> Result<(), ConfigError>`, `Config::validate(&self) -> Result<(), ConfigError>`; новые варианты `ConfigError::Io`, `ConfigError::Invalid(String)`, `ConfigError::NoConfigDir`.

**Почему сейчас.** До этого момента конфиг существовал только в памяти и заполнялся флагами командной строки. Как только он читается с диска, значения становятся недоверенными, и `Semaphore::new(max_connections)` получает возможность запаниковать на старте — финальное ревью плана 1 назвало это как условие: валидировать в том же коммите, где появляется чтение файла.

- [ ] **Step 1: Добавить зависимость**

В `win/Cargo.toml`, в `[workspace.dependencies]`: `directories = "5"`.
В `win/crates/core/Cargo.toml`, в `[dependencies]`: `directories = { workspace = true }`.

- [ ] **Step 2: Написать падающий тест**

Добавь в блок `mod tests` в `config.rs`:

```rust
    #[test]
    fn validate_rejects_a_port_below_the_privileged_range() {
        let mut c = Config::default();
        c.bridge_port = 80;
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_an_absurd_connection_limit() {
        // Semaphore::new паникует выше MAX_PERMITS. Конфиг правится руками,
        // значит значение недоверенное, и падать надо внятной ошибкой при
        // загрузке, а не паникой при старте моста.
        let mut c = Config::default();
        c.max_connections = usize::MAX;
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_a_zero_connection_limit() {
        let mut c = Config::default();
        c.max_connections = 0;
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_a_malformed_upstream() {
        let mut c = Config::default();
        c.socks_upstream = Some("нет-порта".into());
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_accepts_the_defaults() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn load_from_a_missing_file_yields_defaults() {
        // Первый запуск: файла нет, это не ошибка.
        let dir = std::env::temp_dir().join("proxypilot-test-missing");
        let path = dir.join("nope.toml");
        let c = Config::load_from(&path).expect("отсутствие файла — не ошибка");
        assert_eq!(c, Config::default());
    }

    #[test]
    fn save_then_load_roundtrips_through_a_real_file() {
        let dir = std::env::temp_dir().join("proxypilot-test-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.socks_upstream = Some("203.0.113.10:9999".into());
        c.bridge_port = 3130;
        c.save_to(&path).expect("должно сохраниться");

        let back = Config::load_from(&path).expect("должно прочитаться");
        assert_eq!(back, c);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_from_an_invalid_file_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join("proxypilot-test-invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "max_connections = 0\n").unwrap();
        assert!(Config::load_from(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
```

- [ ] **Step 3: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-core config`
Expected: FAIL — `validate`, `load_from`, `save_to` не определены.

- [ ] **Step 4: Написать реализацию**

Добавь в `config.rs`:

```rust
use std::path::{Path, PathBuf};

/// Верхний предел на число соединений. Tokio паникует, если запросить
/// у семафора больше `Semaphore::MAX_PERMITS`; конфиг правится руками,
/// поэтому значение обязано проверяться при загрузке, а не при старте моста.
const MAX_CONNECTIONS_CEILING: usize = 65_536;

impl Config {
    /// `%APPDATA%\ProxyPilot\config.toml`
    pub fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "ProxyPilot")
            .map(|d| d.config_dir().join("config.toml"))
    }

    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::path().ok_or(ConfigError::NoConfigDir)?;
        Self::load_from(&path)
    }

    /// Отсутствие файла — это первый запуск, а не ошибка: возвращаем дефолты.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(ConfigError::Io(e)),
        };
        let cfg = Self::from_toml(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::path().ok_or(ConfigError::NoConfigDir)?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(ConfigError::Io)?;
        }
        std::fs::write(path, self.to_toml()).map_err(ConfigError::Io)
    }

    /// Значения из файла недоверенные. Каждая проверка здесь соответствует
    /// месту, которое иначе упало бы позже и непонятнее.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.bridge_port < 1024 {
            return Err(ConfigError::Invalid(format!(
                "bridge_port {}: порт ниже 1024 требует прав администратора",
                self.bridge_port
            )));
        }
        if self.max_connections == 0 || self.max_connections > MAX_CONNECTIONS_CEILING {
            return Err(ConfigError::Invalid(format!(
                "max_connections {}: допустимо от 1 до {MAX_CONNECTIONS_CEILING}",
                self.max_connections
            )));
        }
        for (name, value) in [
            ("socks_upstream", &self.socks_upstream),
            ("http_upstream", &self.http_upstream),
        ] {
            if let Some(v) = value {
                if !validate_upstream(v) {
                    return Err(ConfigError::Invalid(format!(
                        "{name} «{v}»: нужен формат host:port"
                    )));
                }
            }
        }
        Ok(())
    }
}
```

И расширь `ConfigError`:

```rust
    #[error("не прочитался файл конфига: {0}")]
    Io(#[from] std::io::Error),
    #[error("недопустимое значение в конфиге: {0}")]
    Invalid(String),
    #[error("не нашёл каталог конфигурации пользователя")]
    NoConfigDir,
```

- [ ] **Step 5: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS, тестов на 8 больше.

- [ ] **Step 6: Коммит**

```bash
git add win/Cargo.toml win/crates/core
git commit -m "feat(win): конфиг на диске с валидацией недоверенных значений"
```

---

