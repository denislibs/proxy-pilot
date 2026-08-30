# ProxyPilot Windows — план 2: опознание сети и трей

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** превратить консольный мост в приложение Windows: оно само понимает, в какой сети находится, само выбирает маршрут, само прописывается системным прокси и живёт иконкой в трее.

**Architecture:** третий крейт `proxypilot-winnet` — всё, что говорит с Windows: NLM (опознание сети и события её смены), настройки прокси в реестре, пробы живости апстримов. Поверх — супервизор, который на событие смены сети пересчитывает решение `core::decide` и делает `router.set()`; мост при этом не перезапускается и живые соединения не рвутся. Трей живёт на главном потоке с оконным циклом сообщений, tokio-рантайм моста — на своих потоках, связь между ними — уже существующий `Router` через `ArcSwap`.

**Tech Stack:** Rust 2021, `windows` (windows-rs) для NLM/реестра/WinINET, `tray-icon` + `winit` для трея, `tracing` + `tracing-appender` для логов, `directories` для путей.

**Spec:** [`docs/superpowers/specs/2026-08-30-proxypilot-windows-rust-design.md`](../specs/2026-08-30-proxypilot-windows-rust-design.md) — этот план реализует разделы 4.3, 6 и 10, плюс UI-часть 11.1.

**Предыдущий план:** [`2026-08-30-proxypilot-win-core-bridge.md`](2026-08-30-proxypilot-win-core-bridge.md) — крейты `proxypilot-core` (чистая логика) и `proxypilot-bridge` (мост). Они уже работают и проверены на реальной сети.

## Global Constraints

Действуют для **всех** задач плана. Первые пять унаследованы из плана 1 и остаются в силе.

- Слушатель моста привязывается **строго к `127.0.0.1`**, никогда к `0.0.0.0`.
- **Смена маршрута не трогает установленные соединения.** Маршрут читается один раз на соединение, до набора апстрима, и дальше на пути данных `Router::get()` не вызывается. У него ровно одна не-тестовая точка вызова — это инвариант, а не совпадение.
- **Слушатель привязывается один раз и не перепривязывается, пока живы соединения.** Смена порта из настроек обязана требовать явного перезапуска моста, а не тихо ронять туннели. Это прямое следствие предыдущего пункта, и его легко нарушить по неосторожности.
- **Молчаливого перехода на direct для отдельного соединения нет** — ошибка набора апстрима отдаётся клиенту как `502`.
- Ядро (`proxypilot-core`) остаётся без ввода-вывода и без платформенных зависимостей. Всё, что говорит с Windows, живёт в `proxypilot-winnet`.
- **Ядро не требует прав администратора.** Настройки прокси лежат в `HKCU`, чтение NLM прав не требует, слушатель на loopback не вызывает диалога брандмауэра. Ни одна задача этого плана не должна вводить запрос UAC.
- Rust edition 2021, `rust-version = "1.75"`.
- CI обязан проходить `cargo test --all`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- Комментарии объясняют **почему**, по-русски, в стиле существующих файлов.

## Входные пункты из финального ревью плана 1

Финальное ревью ветки плана 1 назвало четыре вещи, которые обязаны быть сделаны здесь, а не позже. Каждая закреплена за задачей:

- **`tracing`-фасад до начала работы над треем** — пока это две строки на функцию; после того как UI начнёт зависеть от состояния, инструментировать горячий путь станет дорого. → Задача 1.
- **Валидация `max_connections` при загрузке конфига из файла.** `Semaphore::new` паникует при значении выше `MAX_PERMITS`; сегодня это недостижимо, потому что конфиг из файла не читается, и становится достижимым ровно в этом плане. → Задача 2.
- **`Place::network`** из спеки 4.1 — добавить, пока у `Place` один не-тестовый конструктор. → Задача 4.
- **Инвариант «слушатель привязывается один раз»** — записать в код и в комментарий супервизора. → Задача 8.

---

### Task 1: Логи и диагностика

**Files:**
- Modify: `win/Cargo.toml`
- Modify: `win/crates/bridge/Cargo.toml`
- Create: `win/crates/bridge/src/log.rs`
- Modify: `win/crates/bridge/src/lib.rs`
- Modify: `win/crates/bridge/src/serve.rs`
- Modify: `win/crates/bridge/src/connector.rs`

**Interfaces:**
- Consumes: ничего из предыдущих задач этого плана.
- Produces: `log::init(dir: Option<&Path>) -> Option<tracing_appender::non_blocking::WorkerGuard>` — настраивает подписчика; при `None` пишет только в stderr. Все последующие задачи пользуются макросами `tracing::{info, warn, error, debug}` напрямую.

**Почему первой.** Через мост идёт весь трафик машины, и сейчас он не пишет ни строчки: каждая ошибка гасится через `let _ = …`. Пока файлов немного и UI ещё нет, инструментирование — две строки на функцию. После трея это станет правкой всего горячего пути.

- [ ] **Step 1: Добавить зависимости**

В `win/Cargo.toml`, в `[workspace.dependencies]`:

```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
tracing-appender = "0.2"
```

В `win/crates/bridge/Cargo.toml`, в `[dependencies]`:

```toml
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
tracing-appender = { workspace = true }
```

- [ ] **Step 2: Написать падающий тест**

`win/crates/bridge/src/log.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_defaults_to_info_and_honours_the_env_var() {
        // Без переменной — info: в бою нужен спокойный лог.
        assert_eq!(filter_directive(None), "proxypilot=info");
        // С переменной — что попросили, чтобы можно было поднять уровень
        // на месте, не пересобирая.
        assert_eq!(filter_directive(Some("proxypilot=debug")), "proxypilot=debug");
        // Пустая переменная — не считается заданной.
        assert_eq!(filter_directive(Some("")), "proxypilot=info");
    }

    #[test]
    fn log_file_name_is_stable() {
        // Имя должно быть предсказуемым: на него смотрит doctor и человек,
        // которого просят прислать лог.
        assert_eq!(LOG_FILE_PREFIX, "proxypilot");
    }
}
```

- [ ] **Step 3: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge log`
Expected: FAIL — `filter_directive` и `LOG_FILE_PREFIX` не определены.

- [ ] **Step 4: Написать реализацию**

Вставь в начало `win/crates/bridge/src/log.rs`:

```rust
//! Логи.
//!
//! Компонент несёт весь трафик машины, поэтому «не работает» без лога
//! неотличимо от «работает медленно». Уровень по умолчанию — info: в бою
//! нужен спокойный лог, который не крутит диск. Ежедневная ротация, потому
//! что на macOS-версии её нет и файл там растёт бесконечно.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

pub const LOG_FILE_PREFIX: &str = "proxypilot";
pub const ENV_VAR: &str = "PROXYPILOT_LOG";

/// Какой фильтр применить: переменная окружения, иначе info.
pub fn filter_directive(env: Option<&str>) -> String {
    match env {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => "proxypilot=info".to_string(),
    }
}

/// Настраивает подписчика. Возвращает страж, который обязан жить столько же,
/// сколько процесс: при его сбросе неотправленные строки теряются.
///
/// `dir` = None — только stderr (так работает CLI-режим и тесты).
pub fn init(dir: Option<&Path>) -> Option<WorkerGuard> {
    let env = std::env::var(ENV_VAR).ok();
    let filter = EnvFilter::new(filter_directive(env.as_deref()));

    match dir {
        None => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .init();
            None
        }
        Some(dir) => {
            let appender = tracing_appender::rolling::daily(dir, LOG_FILE_PREFIX);
            let (writer, guard) = tracing_appender::non_blocking(appender);
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(writer)
                .init();
            Some(guard)
        }
    }
}
```

- [ ] **Step 5: Инструментировать горячий путь**

В `serve.rs` заменить молчаливые `let _ = …` на логирование там, где теряется информация. Точечно, без шума на каждое соединение:

- в руке ошибки `accept`: `warn!(error = %e, consecutive, "ошибка приёма соединения");`
- перед ответом `502`: `warn!(%host, port, error = %e, "апстрим недоступен");`
- перед ответом `503`: `warn!(limit = shared.limits.max_connections, "предел соединений исчерпан");`
- перед ответом `400`/`408`: `debug!(error = %e, "некорректный запрос клиента");`
- в `serve` при старте: `info!(%addr, "мост слушает");`

Успешные соединения **не логируются на уровне info** — это путь каждого запроса браузера, и info-строка на каждый превратит лог в мусор. Для них `debug!`.

В `connector.rs`, в `connect_via`, при ошибке: `debug!(route = ?route, %host, port, error = %e, "не удалось соединиться");` — уровень debug, потому что вызывающий уже логирует это как warn с большим контекстом, и дублировать в info незачем.

- [ ] **Step 6: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: всё зелёное, число тестов выросло на 2.

- [ ] **Step 7: Коммит**

```bash
git add win/Cargo.toml win/crates/bridge
git commit -m "feat(win): логи с ротацией и инструментирование горячего пути"
```

---

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

### Task 3: Крейт winnet и опознание сетей через NLM

**Files:**
- Create: `win/crates/winnet/Cargo.toml`
- Create: `win/crates/winnet/src/lib.rs`
- Create: `win/crates/winnet/src/com.rs`
- Create: `win/crates/winnet/src/networks.rs`
- Modify: `win/Cargo.toml`

**Interfaces:**
- Consumes: ничего.
- Produces: `NetworkSnapshot { id: String, name: String, connected: bool, category: NetworkCategory, internet: bool }`, `NetworkCategory { Public, Private, Domain, Unknown }`, `list_connected() -> Result<Vec<NetworkSnapshot>, WinNetError>`, `ComGuard`.

**Почему это ядро всего плана.** macOS-версия не имеет понятия «сеть как объект» и вынуждена строить эвристику: перебрать интерфейсы, взять адрес и шлюз, сверить строковые префиксы, убедиться, что шлюз отвечает за физическим интерфейсом, пингануть, откатиться на ARP. Полторы сотни строк, и вся конструкция существует ради одного — чтобы поднятый VPN не сошёл за офис. Windows держит реестр сетей: у каждой есть GUID, имя, категория и состояние. Сравнение по GUID не подделывается туннелем, не ломается при смене подсети и не требует ни ICMP, ни ARP. Вся эвристическая половина не портируется.

- [ ] **Step 1: Создать крейт**

`win/crates/winnet/Cargo.toml`:

```toml
[package]
name = "proxypilot-winnet"
edition.workspace = true
rust-version.workspace = true
version.workspace = true

[target.'cfg(windows)'.dependencies]
windows = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_System_Com",
    "Win32_System_Variant",
    "Win32_Networking_NetworkListManager",
] }

[dependencies]
thiserror = { workspace = true }
tracing = { workspace = true }
```

В `win/Cargo.toml`: добавить `"crates/winnet"` в `members` и `windows = "0.58"` в `[workspace.dependencies]`.

- [ ] **Step 2: Написать падающий тест**

`win/crates/winnet/src/networks.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_maps_every_documented_value() {
        assert_eq!(NetworkCategory::from_raw(0), NetworkCategory::Public);
        assert_eq!(NetworkCategory::from_raw(1), NetworkCategory::Private);
        assert_eq!(NetworkCategory::from_raw(2), NetworkCategory::Domain);
        // Неизвестное значение не должно паниковать: Windows может завести
        // новую категорию, и падать из-за этого мы не обязаны.
        assert_eq!(NetworkCategory::from_raw(99), NetworkCategory::Unknown);
    }

    #[test]
    fn guid_is_formatted_in_the_canonical_braced_form() {
        // Этот идентификатор пользователь увидит в конфиге и, возможно,
        // сверит с `Get-NetConnectionProfile`. Форма обязана совпадать.
        let g = windows::core::GUID::from_u128(0x1234_5678_90ab_cdef_1234_5678_90ab_cdef);
        let s = format_guid(&g);
        assert!(s.starts_with('{') && s.ends_with('}'), "получили: {s}");
        assert_eq!(s.len(), 38);
        assert_eq!(s, s.to_uppercase(), "канонично — верхний регистр");
    }

    #[cfg(windows)]
    #[test]
    fn listing_connected_networks_does_not_fail_on_a_real_machine() {
        // Смоук: на живой машине вызов обязан отработать. Список может быть
        // пустым (машина без сети) — это не ошибка.
        let _guard = crate::com::ComGuard::new().expect("COM должен подняться");
        let nets = list_connected().expect("перечисление сетей не должно падать");
        for n in &nets {
            assert!(!n.id.is_empty(), "у сети обязан быть идентификатор");
            assert!(n.connected, "list_connected отдаёт только подключённые");
        }
    }
}
```

- [ ] **Step 3: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-winnet`
Expected: FAIL — крейта и его типов нет.

- [ ] **Step 4: Написать реализацию**

`win/crates/winnet/src/com.rs`:

```rust
//! Инициализация COM.
//!
//! NLM — COM-объект, и до первого обращения поток обязан войти в апартамент.
//! Держим это стражем, а не свободной функцией: `CoUninitialize` обязан
//! вызваться на том же потоке и ровно столько же раз, сколько `CoInitialize`.

use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};

use crate::WinNetError;

/// Пока жив — поток в апартаменте. Сбрасывать вручную не нужно.
pub struct ComGuard;

impl ComGuard {
    pub fn new() -> Result<Self, WinNetError> {
        // SAFETY: вызов на текущем потоке, парный CoUninitialize — в Drop.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok()?;
        }
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: парный вызов к CoInitializeEx на том же потоке.
        unsafe { CoUninitialize() };
    }
}
```

`win/crates/winnet/src/networks.rs` (перед блоком тестов):

```rust
//! Опознание сети через Network List Manager.
//!
//! Windows помнит каждую сеть, которую видела: у неё есть GUID, имя,
//! категория (Public/Private/Domain) и состояние. Сравнение по GUID —
//! то, чего у macOS-версии не было и ради чего там пришлось городить
//! эвристику из адреса, шлюза, ping и ARP: GUID не подделывается поднятым
//! туннелем и не меняется при смене подсети.

use windows::core::GUID;
use windows::Win32::Networking::NetworkListManager::{
    INetwork, INetworkListManager, NetworkListManager, NLM_ENUM_NETWORK_CONNECTED,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use crate::WinNetError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkCategory {
    Public,
    Private,
    Domain,
    Unknown,
}

impl NetworkCategory {
    pub fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Public,
            1 => Self::Private,
            2 => Self::Domain,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSnapshot {
    /// GUID в канонической форме `{XXXXXXXX-XXXX-...}` — то, что попадёт
    /// в конфиг и что человек может сверить с `Get-NetConnectionProfile`.
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub category: NetworkCategory,
    pub internet: bool,
}

pub fn format_guid(g: &GUID) -> String {
    format!("{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3,
        g.data4[0], g.data4[1], g.data4[2], g.data4[3],
        g.data4[4], g.data4[5], g.data4[6], g.data4[7])
}

/// Подключённые сейчас сети. Вызывающий обязан держать живым `ComGuard`.
pub fn list_connected() -> Result<Vec<NetworkSnapshot>, WinNetError> {
    // SAFETY: COM инициализирован вызывающим (ComGuard), интерфейсы
    // освобождаются самим windows-rs по Drop.
    unsafe {
        let manager: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
        let enumerator = manager.GetNetworks(NLM_ENUM_NETWORK_CONNECTED)?;

        let mut out = Vec::new();
        loop {
            let mut item = [None::<INetwork>; 1];
            let mut fetched = 0u32;
            enumerator.Next(&mut item, &mut fetched)?;
            if fetched == 0 {
                break;
            }
            let Some(net) = item[0].as_ref() else { break };

            let id = format_guid(&net.GetNetworkId()?);
            let name = net.GetName()?.to_string();
            let connected = net.IsConnected()?.as_bool();
            let internet = net.IsConnectedToInternet()?.as_bool();
            let category = NetworkCategory::from_raw(net.GetCategory()?.0);

            out.push(NetworkSnapshot { id, name, connected, category, internet });
        }
        Ok(out)
    }
}
```

`win/crates/winnet/src/lib.rs`:

```rust
//! Всё, что говорит с Windows: опознание сети, настройки прокси, события.
//!
//! Вынесено отдельным крейтом сознательно: `proxypilot-core` обязан
//! оставаться без платформенных зависимостей, а `proxypilot-bridge` —
//! переносимым (он говорит только на tokio).

pub mod com;
pub mod networks;

#[derive(Debug, thiserror::Error)]
pub enum WinNetError {
    #[error("ошибка Windows: {0}")]
    Windows(#[from] windows::core::Error),
}
```

- [ ] **Step 5: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS. Смоук-тест на живой машине обязан перечислить хотя бы одну сеть, если машина в сети.

- [ ] **Step 6: Ручная проверка — вывести реальные сети**

Добавь временный пример или воспользуйся тестом с `-- --nocapture`, чтобы увидеть свои настоящие сети и их GUID. Запиши GUID своей текущей сети в отчёт: он понадобится при настройке офиса.

Run: `cd win && cargo test -p proxypilot-winnet -- --nocapture listing_connected`

- [ ] **Step 7: Коммит**

```bash
git add win/Cargo.toml win/crates/winnet
git commit -m "feat(win): крейт winnet и опознание сетей через NLM"
```

---

### Task 4: Место в конфиге и `Place::network`

**Files:**
- Modify: `win/crates/core/src/mode.rs`
- Modify: `win/crates/core/src/config.rs`

**Interfaces:**
- Consumes: `Place`, `Config` из плана 1; `NetworkSnapshot` концептуально (без зависимости — `core` остаётся платформонезависимым, идентификатор передаётся строкой).
- Produces: `Place { in_office: bool, network: Option<String> }`, `Config::office_networks: Vec<OfficeNetwork>`, `OfficeNetwork { id: String, name: String }`, `Config::place_for(&self, connected_ids: &[String]) -> Place`.

**Почему сейчас.** Финальное ревью плана 1: у `Place` сейчас один не-тестовый конструктор, и добавить поле дёшево; после появления супервизора и трея это станет ломающей правкой в десятке мест.

- [ ] **Step 1: Написать падающий тест**

Добавь в `mod tests` в `config.rs`:

```rust
    fn office_cfg() -> Config {
        let mut c = Config::default();
        c.office_networks = vec![
            OfficeNetwork { id: "{AAAA0000-0000-0000-0000-000000000001}".into(), name: "Офис".into() },
            OfficeNetwork { id: "{AAAA0000-0000-0000-0000-000000000002}".into(), name: "Офис-2".into() },
        ];
        c
    }

    #[test]
    fn place_is_office_when_a_connected_network_matches() {
        let p = office_cfg().place_for(&["{AAAA0000-0000-0000-0000-000000000002}".to_string()]);
        assert!(p.in_office);
        assert_eq!(p.network.as_deref(), Some("{AAAA0000-0000-0000-0000-000000000002}"));
    }

    #[test]
    fn place_is_not_office_for_an_unknown_network() {
        let p = office_cfg().place_for(&["{BBBB0000-0000-0000-0000-000000000000}".to_string()]);
        assert!(!p.in_office);
        assert_eq!(p.network.as_deref(), Some("{BBBB0000-0000-0000-0000-000000000000}"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        // GUID из реестра и из конфига могут отличаться регистром — это
        // один и тот же идентификатор, и различать их было бы ловушкой.
        let p = office_cfg().place_for(&["{aaaa0000-0000-0000-0000-000000000001}".to_string()]);
        assert!(p.in_office);
    }

    #[test]
    fn no_network_at_all_is_not_office() {
        let p = office_cfg().place_for(&[]);
        assert!(!p.in_office);
        assert!(p.network.is_none());
    }

    #[test]
    fn without_configured_offices_nothing_is_office() {
        // Пустой список — «мы не знаем, где находимся». Считать это офисом
        // означало бы гнать весь трафик через прокси в любой сети.
        let p = Config::default().place_for(&["{AAAA0000-0000-0000-0000-000000000001}".to_string()]);
        assert!(!p.in_office);
    }

    #[test]
    fn several_connected_networks_office_wins() {
        // Ноутбук может быть одновременно в Wi-Fi и в доке по кабелю.
        // Если хоть одна из них офисная — мы в офисе.
        let p = office_cfg().place_for(&[
            "{CCCC0000-0000-0000-0000-000000000000}".to_string(),
            "{AAAA0000-0000-0000-0000-000000000001}".to_string(),
        ]);
        assert!(p.in_office);
        assert_eq!(p.network.as_deref(), Some("{AAAA0000-0000-0000-0000-000000000001}"));
    }
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-core place`
Expected: FAIL — `OfficeNetwork` и `place_for` не определены.

- [ ] **Step 3: Написать реализацию**

В `mode.rs` заменить `Place`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub in_office: bool,
    /// Идентификатор сети, по которой принято решение. Нужен, чтобы UI мог
    /// показать «сейчас: Офис», а лог — объяснить, почему выбран маршрут.
    pub network: Option<String>,
}
```

`Place` перестаёт быть `Copy` — поправь места, где он копировался.

В `config.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeNetwork {
    /// GUID сети в канонической форме, как его отдаёт NLM.
    pub id: String,
    /// Человекочитаемое имя — только для UI, в сравнении не участвует:
    /// пользователь может переименовать сеть, а идентификатор останется.
    #[serde(default)]
    pub name: String,
}

impl Config {
    /// Где мы, судя по списку подключённых сетей.
    ///
    /// Пустой список офисов означает «не знаем» и трактуется как «не офис»:
    /// считать иначе значило бы гнать весь трафик через прокси в любой сети.
    pub fn place_for(&self, connected_ids: &[String]) -> Place {
        let office = connected_ids.iter().find(|id| {
            self.office_networks
                .iter()
                .any(|o| o.id.eq_ignore_ascii_case(id))
        });
        match office {
            Some(id) => Place { in_office: true, network: Some(id.clone()) },
            None => Place { in_office: false, network: connected_ids.first().cloned() },
        }
    }
}
```

И поле в `Config` (плюс `office_networks: Vec::new()` в `Default`):

```rust
    #[serde(default)]
    pub office_networks: Vec<OfficeNetwork>,
```

- [ ] **Step 4: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/core
git commit -m "feat(win): офисные сети в конфиге и Place::network"
```

---

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

### Task 6: Системный прокси в реестре

**Files:**
- Create: `win/crates/winnet/src/sysproxy.rs`
- Modify: `win/crates/winnet/src/lib.rs`
- Modify: `win/crates/winnet/Cargo.toml`

**Interfaces:**
- Consumes: `WinNetError`.
- Produces: `SysProxy { enabled: bool, server: String, bypass: String }`, `read() -> Result<SysProxy, WinNetError>`, `apply(&SysProxy) -> Result<(), WinNetError>`, `to_bypass_string(no_proxy: &str) -> String`.

**Почему мы этим управляем сами.** На macOS пользователю предписано выставить прокси руками один раз. Здесь прав администратора не нужно — это `HKCU`, — поэтому приложение делает это само. Взамен появляется обязанность: **при падении процесса в реестре останется указатель на мёртвый слушатель, и пользователь останется без сети вообще** — отказ хуже того, который мы лечим. Поэтому прежнее значение сохраняется в конфиг ДО записи в реестр, и восстанавливается при следующем старте (спека 6.3).

Что этим не покрывается и должно быть честно сказано в UI: **WinHTTP** (`netsh winhttp`, контекст служб, нужен администратор), **Firefox** (свои настройки мимо WinINET), и приложения, читающие `HTTP_PROXY` из окружения.

- [ ] **Step 1: Добавить фичи windows-rs**

В `win/crates/winnet/Cargo.toml`, в список features: `"Win32_System_Registry"`, `"Win32_Networking_WinInet"`.

- [ ] **Step 2: Написать падающий тест**

`win/crates/winnet/src/sysproxy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_string_uses_semicolons_and_keeps_local_token() {
        // WinINET разделяет точкой с запятой, а не запятой, и понимает
        // особый токен <local> для адресов без точки.
        let s = to_bypass_string("localhost,127.0.0.1,.local,192.168.0.0/16");
        assert!(s.contains(';'), "получили: {s}");
        assert!(!s.contains(','), "запятых остаться не должно: {s}");
        assert!(s.contains("<local>"), "локальные имена без точки: {s}");
    }

    #[test]
    fn bypass_string_converts_dot_suffix_to_wildcard() {
        // «.local» в нашем формате — суффикс; WinINET ждёт «*.local».
        let s = to_bypass_string(".local");
        assert!(s.contains("*.local"), "получили: {s}");
    }

    #[test]
    fn bypass_string_skips_empty_entries() {
        let s = to_bypass_string("localhost,,  ,127.0.0.1");
        assert!(!s.contains(";;"), "получили: {s}");
    }

    #[cfg(windows)]
    #[test]
    fn reading_current_settings_does_not_fail() {
        // Смоук на живой машине: ключ существует всегда, даже когда прокси
        // выключен. Ничего не меняем — только читаем.
        let s = read().expect("HKCU Internet Settings обязан читаться");
        // enabled может быть любым; проверяем лишь, что структура заполнена
        let _ = (s.enabled, s.server.len(), s.bypass.len());
    }
}
```

- [ ] **Step 3: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-winnet sysproxy`
Expected: FAIL — модуля нет.

- [ ] **Step 4: Написать реализацию**

```rust
//! Системные настройки прокси (WinINET).
//!
//! Живут в HKCU, поэтому прав администратора не нужно и приложение
//! управляет ими само — в отличие от macOS-версии, где это делал человек.
//!
//! Плата за это — обязанность прибраться. Если процесс упадёт, в реестре
//! останется указатель на мёртвый слушатель, и пользователь окажется без
//! сети вообще: отказ хуже того, который мы лечим. Поэтому прежнее значение
//! сохраняется в конфиг ДО записи сюда и восстанавливается при старте.
//!
//! Что этим НЕ покрывается: WinHTTP (контекст служб, нужен администратор),
//! Firefox (свои настройки), и приложения, читающие HTTP_PROXY из окружения.

use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinInet::{
    InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ,
};

use crate::WinNetError;

const SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SysProxy {
    pub enabled: bool,
    /// `127.0.0.1:3129` либо пусто
    pub server: String,
    /// список исключений в формате WinINET (через `;`)
    pub bypass: String,
}

/// Наш список исключений → формат WinINET.
///
/// Отличия от нашего: разделитель `;`, суффикс пишется как `*.local`,
/// и есть особый токен `<local>` — адреса без точки в имени.
pub fn to_bypass_string(no_proxy: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for raw in no_proxy.split(',') {
        let e = raw.trim();
        if e.is_empty() {
            continue;
        }
        if let Some(sfx) = e.strip_prefix('.') {
            parts.push(format!("*.{sfx}"));
        } else {
            parts.push(e.to_string());
        }
    }
    parts.push("<local>".to_string());
    parts.join(";")
}

pub fn read() -> Result<SysProxy, WinNetError> { /* RegOpenKeyExW(KEY_READ) + RegQueryValueExW */ }

pub fn apply(p: &SysProxy) -> Result<(), WinNetError> {
    /* RegSetValueExW ProxyEnable/ProxyServer/ProxyOverride, затем:
       InternetSetOptionW(None, INTERNET_OPTION_SETTINGS_CHANGED, None, 0)
       InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0)
       Без этих двух вызовов уже запущенные приложения продолжат ходить
       по старым настройкам до перезапуска. */
}
```

Реализацию `read`/`apply` пиши целиком — оставленные заглушки в этом плане только чтобы не диктовать построчно обвязку `RegQueryValueExW` с её двойным вызовом за размером буфера. Требования: `ProxyEnable` — `REG_DWORD` 0/1, `ProxyServer` и `ProxyOverride` — `REG_SZ` в UTF-16 с завершающим нулём; ключ открывается с `KEY_READ`/`KEY_WRITE` и закрывается через `RegCloseKey` в любом случае, включая ошибочный путь.

- [ ] **Step 5: Ручная проверка — и обязательно вернуть как было**

Прочитай текущие настройки, применить свои, убедиться в System Settings → Network → Proxy, что значение изменилось, **вернуть исходное**. Приложи в отчёт вывод «до», «после» и «вернули».

- [ ] **Step 6: Прогнать тесты и линтеры, закоммитить**

```bash
git add win/crates/winnet
git commit -m "feat(win): системный прокси через реестр и InternetSetOption"
```

---

### Task 7: События смены сети

**Files:**
- Create: `win/crates/winnet/src/events.rs`
- Modify: `win/crates/winnet/src/lib.rs`
- Modify: `win/crates/winnet/Cargo.toml`

**Interfaces:**
- Consumes: `ComGuard` (задача 3), `WinNetError`.
- Produces: `NetworkChange` (перечисление), `watch_network_changes() -> Result<tokio::sync::mpsc::Receiver<NetworkChange>, WinNetError>`, `debounce(rx, window) -> Receiver<NetworkChange>`.

**Устройство.** NLM отдаёт события по классическому паттерну точек подключения: создать `NetworkListManager`, запросить у него `IConnectionPointContainer`, найти точку по IID `INetworkListManagerEvents`, вызвать `Advise` с объектом-приёмником. Приёмник реализуется макросом `#[implement]` из windows-rs. Готового примера на Rust в открытом виде нет — писать аккуратно, сверяясь с документацией интерфейсов.

Поток, на котором сделан `Advise`, обязан быть в апартаменте и **крутить цикл сообщений**: COM доставляет события апартаментного объекта через оконные сообщения, и без цикла приёмник просто не вызовется. Поэтому подписка живёт на своём выделенном потоке, а наружу отдаёт `tokio::sync::mpsc`.

**Запасной канал.** Если подписка не поднялась (нет прав, сломан COM, экзотическая сборка Windows) — `NotifyIpInterfaceChange` из IP Helper. Он грубее (реагирует на изменения адресов, а не на смену профиля сети), но лучше, чем ничего. Опрос по таймеру не используется: на macOS он был вынужденным, здесь есть настоящие события. Отказ подписки логируется как `warn` — молча деградировать нельзя.

**Дребезг.** Одно физическое переключение Wi-Fi порождает пачку событий. Схлопываем окном в 2 секунды: супервизор пересчитывает решение целиком, и десять пересчётов подряд ничего не добавляют, зато десять записей в логе мешают читать.

- [ ] **Step 1: Написать падающий тест на схлопывание**

`win/crates/winnet/src/events.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_burst_collapses_to_one_event() {
        // Одно переключение Wi-Fi даёт пачку событий; наружу должно уйти одно.
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut out = debounce(rx, Duration::from_millis(100));

        for _ in 0..5 {
            tx.send(NetworkChange::Connectivity).await.unwrap();
        }
        drop(tx);

        assert!(out.recv().await.is_some(), "первое событие обязано пройти");
        assert!(out.recv().await.is_none(), "остальные — схлопнуться");
    }

    #[tokio::test]
    async fn events_further_apart_than_the_window_both_pass() {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut out = debounce(rx, Duration::from_millis(50));

        tx.send(NetworkChange::Connectivity).await.unwrap();
        assert!(out.recv().await.is_some());

        tokio::time::sleep(Duration::from_millis(120)).await;
        tx.send(NetworkChange::NetworkPropertyChanged).await.unwrap();
        drop(tx);
        assert!(out.recv().await.is_some(), "после окна событие обязано пройти");
    }

    #[tokio::test]
    async fn closing_the_source_closes_the_output() {
        let (tx, rx) = tokio::sync::mpsc::channel::<NetworkChange>(1);
        let mut out = debounce(rx, Duration::from_millis(10));
        drop(tx);
        assert!(out.recv().await.is_none());
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-winnet events`
Expected: FAIL — `NetworkChange` и `debounce` не определены.

- [ ] **Step 3: Реализовать схлопывание**

```rust
//! События смены сети.
//!
//! NLM отдаёт их через точку подключения: создать NetworkListManager,
//! запросить IConnectionPointContainer, найти точку по IID
//! INetworkListManagerEvents, вызвать Advise с приёмником.
//!
//! Поток, сделавший Advise, обязан крутить цикл сообщений: COM доставляет
//! события апартаментного объекта оконными сообщениями, и без цикла приёмник
//! просто не вызовется. Отсюда выделенный поток и канал наружу.

use std::time::Duration;

use tokio::sync::mpsc::{channel, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkChange {
    Connectivity,
    NetworkAdded,
    NetworkPropertyChanged,
}

/// Схлопывает пачку событий в одно.
///
/// Первое событие проходит сразу — реагировать надо быстро; всё, что пришло
/// в течение окна после него, отбрасывается.
pub fn debounce(mut rx: Receiver<NetworkChange>, window: Duration) -> Receiver<NetworkChange> {
    let (tx, out) = channel(8);
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if tx.send(ev).await.is_err() {
                return;
            }
            // Дожёвываем хвост пачки, ничего не пересылая.
            let deadline = tokio::time::Instant::now() + window;
            loop {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some(_)) => continue,
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
        }
    });
    out
}
```

- [ ] **Step 4: Реализовать подписку**

Приёмник через `#[implement(INetworkListManagerEvents)]`, метод `ConnectivityChanged` шлёт `NetworkChange::Connectivity` в канал (`try_send`, чтобы не блокировать COM-поток; переполнение канала означает, что супервизор ещё не разгрёб прошлое событие, и терять новое безопасно — решение всё равно пересчитывается целиком).

Порядок: `ComGuard` → `CoCreateInstance(&NetworkListManager)` → `.cast::<IConnectionPointContainer>()` → `FindConnectionPoint(&INetworkListManagerEvents::IID)` → `Advise(&sink)`. Полученный `cookie` хранится и отдаётся в `Unadvise` при завершении. Затем цикл `GetMessage`/`DispatchMessage` до сигнала остановки.

При любой ошибке на этом пути: `warn!` с текстом и переход на `NotifyIpInterfaceChange`.

- [ ] **Step 5: Ручная проверка**

Запустить, физически переключить Wi-Fi (или включить/выключить адаптер), увидеть в логе ровно одну строку о смене сети, а не пачку. Приложить вывод лога в отчёт.

- [ ] **Step 6: Коммит**

```bash
git add win/crates/winnet
git commit -m "feat(win): события смены сети через NLM со схлопыванием пачки"
```

---

### Task 8: Супервизор

**Files:**
- Create: `win/crates/bridge/src/supervisor.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Router` (план 1), `Prober` (задача 5), `Config`/`place_for` (задачи 2, 4), `core::decide`.
- Produces: `NetworkSource` (трейт), `Supervisor::new(router, prober, config, source)`, `Supervisor::reevaluate(&self) -> AppState`, `Supervisor::run(self, events)`, `AppState { mode, route, demoted, place, health, port }`.

**Что делает.** На старте и на каждое схлопнутое событие смены сети: список подключённых сетей → `config.place_for` → `prober.health` → `core::decide` → **если решение изменилось, `router.set()`**. Мост при этом не трогается вообще.

**Трейт ради тестируемости.** `NetworkSource` с одним синхронным методом `connected_ids() -> Result<Vec<String>, ...>` — NLM синхронен, и async здесь ничего не даёт. В бою — реализация поверх `winnet::list_connected`, в тестах — подставная. Это единственное место, где нужен трейт: остальное чистые функции.

- [ ] **Step 1: Написать падающий тест**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct FakeNet(std::sync::Mutex<Vec<String>>);
    impl NetworkSource for FakeNet {
        fn connected_ids(&self) -> Result<Vec<String>, SupervisorError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn office_config(socks: &str) -> Config {
        let mut c = Config::default();
        c.socks_upstream = Some(socks.to_string());
        c.mode = Mode::Auto;
        c.office_networks = vec![OfficeNetwork { id: "{OFFICE}".into(), name: "Офис".into() }];
        c
    }

    #[tokio::test]
    async fn in_the_office_with_a_live_socks_the_route_becomes_socks() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();

        let router = Arc::new(Router::new(Route::Direct));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config(&addr),
            Box::new(FakeNet(Mutex::new(vec!["{OFFICE}".into()]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Socks(addr.clone()));
        assert_eq!(*router.get(), Route::Socks(addr));
        assert!(state.place.in_office);
    }

    #[tokio::test]
    async fn outside_the_office_the_route_is_direct_even_with_a_live_upstream() {
        // Правило спеки 4.2 дословно: снаружи офисный прокси тоже отвечает
        // (через туннель), но маршрут через него был бы кругом через офис.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();

        let router = Arc::new(Router::new(Route::Socks(addr.clone())));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config(&addr),
            Box::new(FakeNet(Mutex::new(vec!["{HOME}".into()]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Direct);
        assert_eq!(*router.get(), Route::Direct);
        assert!(!state.place.in_office);
    }

    #[tokio::test]
    async fn an_unchanged_decision_does_not_touch_the_router() {
        // Лишний set безвреден для соединений, но маскирует ошибки в логике
        // и засоряет лог. Решение не изменилось — значит и трогать нечего.
        let router = Arc::new(Router::new(Route::Direct));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config("127.0.0.1:1"),
            Box::new(FakeNet(Mutex::new(vec!["{HOME}".into()]))),
        );

        let before = router.get();
        sup.reevaluate().await;
        let after = router.get();
        assert!(Arc::ptr_eq(&before, &after), "router.set вызывать не следовало");
    }

    #[tokio::test]
    async fn a_dead_pinned_upstream_is_reported_as_demoted() {
        let router = Arc::new(Router::new(Route::Direct));
        let mut cfg = office_config("127.0.0.1:1");
        cfg.mode = Mode::Socks;
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_millis(200)),
            cfg,
            Box::new(FakeNet(Mutex::new(vec!["{OFFICE}".into()]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Direct);
        assert!(state.demoted, "понижение обязано быть видно в состоянии");
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge supervisor`
Expected: FAIL — `Supervisor` не определён.

- [ ] **Step 3: Написать реализацию**

Ключевой комментарий, который обязан быть в файле:

```rust
//! Супервизор: пересчёт маршрута при смене обстановки.
//!
//! ИНВАРИАНТ. Слушатель привязывается один раз за жизнь процесса и не
//! перепривязывается. Супервизор меняет ТОЛЬКО маршрут — через router.set(),
//! который не касается установленных соединений. Смена порта требует
//! перезапуска моста и обязана быть явным действием пользователя: тихая
//! перепривязка убьёт то самое свойство, ради которого продукт переписан.
```

`reevaluate` возвращает `AppState` — его же читает трей, чтобы нарисовать иконку и меню, не дублируя логику.

- [ ] **Step 4: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): супервизор — пересчёт маршрута на смену сети"
```

---

### Task 9: Трей и приложение

**Files:**
- Create: `win/crates/app/Cargo.toml`, `win/crates/app/src/main.rs`, `win/crates/app/src/tray.rs`, `win/crates/app/src/icons.rs`
- Modify: `win/Cargo.toml`

**Interfaces:**
- Consumes: `Supervisor`, `AppState`, `Router`, `serve`, `Config`, `sysproxy`, `watch_network_changes`, `log::init`.
- Produces: исполняемое приложение `proxypilot`.

**Устройство процесса.** `tray-icon` (0.24 на момент написания — проверь актуальную версию) требует оконный цикл сообщений на том же потоке, где создана иконка, а на Windows это практически главный поток. tokio-рантайм моста поднимается отдельно.

```
главный поток              tokio runtime
─────────────              ─────────────
цикл сообщений      ←──→   serve()      (мост)
трей + меню                supervisor   (пересчёт маршрута)
      │                          │
      └───── Router (ArcSwap) ───┘
```

Существующая архитектура ложится сюда без единой правки в мосте — прямое следствие того, что маршрут уже живёт в атомарной ячейке. Трей вызывает `router.set()` из главного потока, мост видит новое значение на следующем соединении, живые туннели не замечают ничего.

**Иконка** отражает активный маршрут: SOCKS5 · HTTP · напрямую · мост не запущен · не настроено. Иконки строятся из сырых RGBA через `Icon::from_rgba` — рисуем программно, чтобы не тащить файлы ресурсов в первую версию.

**Меню** повторяет macOS-версию: заголовок с адресом моста и текущим состоянием (в том числе «SOCKS5 недоступен → работаем напрямую», когда `demoted`), переключение режимов через `CheckMenuItem` с индикаторами доступности, копирование адреса, выход. Секции сети и туннеля появятся в плане 3.

**Обязательное поведение при выходе.** Восстановить системный прокси в то состояние, что было до запуска, — иначе выход из приложения оставляет машину без сети. То же самое при старте: если в реестре стоит наш адрес, а моста нет, значит прошлый процесс убили — либо поднимаем мост, либо возвращаем сохранённое значение (спека 6.3).

- [ ] **Step 1: Написать падающий тест на чистые части**

Тестируется то, что не требует окна: выбор иконки по состоянию, формирование строки заголовка меню, решение о восстановлении прокси при старте.

```rust
    #[test]
    fn icon_reflects_the_active_route() {
        assert_eq!(icon_for(&state(Route::Socks("x:1".into()), false)), IconKind::Socks);
        assert_eq!(icon_for(&state(Route::Http("x:1".into()), false)), IconKind::Http);
        assert_eq!(icon_for(&state(Route::Direct, false)), IconKind::Direct);
    }

    #[test]
    fn header_names_the_bridge_and_the_route() {
        let h = header_text(&state(Route::Direct, false));
        assert!(h.contains("127.0.0.1:3129"), "получили: {h}");
    }

    #[test]
    fn header_explains_a_demotion_rather_than_hiding_it() {
        // Спека 4.2: молчаливый обход выглядит как «галочка стоит на SOCKS,
        // а трафик идёт мимо».
        let mut s = state(Route::Direct, true);
        s.mode = Mode::Socks;
        let h = header_text(&s);
        assert!(h.contains("недоступен"), "получили: {h}");
    }

    #[test]
    fn stale_registry_pointing_at_us_without_a_bridge_is_detected() {
        // Прошлый процесс убили: в реестре наш адрес, моста нет.
        assert!(is_stale_pointer(&SysProxy {
            enabled: true,
            server: "127.0.0.1:3129".into(),
            bypass: String::new(),
        }, 3129));
        assert!(!is_stale_pointer(&SysProxy {
            enabled: true,
            server: "10.0.0.2:3128".into(),
            bypass: String::new(),
        }, 3129));
    }
```

- [ ] **Step 2: Проверить падение, затем реализовать.**

- [ ] **Step 3: Ручная проверка — обязательная и подробная**

Запустить приложение. Проверить по пунктам и приложить в отчёт:
1. иконка появилась в трее;
2. `curl -x http://127.0.0.1:3129 https://example.com/` возвращает 200;
3. переключение режима из меню меняет иконку и продолжает работать;
4. системный прокси в System Settings → Network → Proxy указывает на нас;
5. **выход из меню возвращает системный прокси в исходное состояние** — проверить в тех же настройках;
6. после выхода `curl https://example.com/` без `-x` по-прежнему работает, то есть машина не осталась без сети.

- [ ] **Step 4: Коммит**

```bash
git add win/Cargo.toml win/crates/app
git commit -m "feat(win): приложение в трее с переключением режимов"
```

---

### Task 10: Сборка и CI

**Files:**
- Modify: `win/crates/app/src/main.rs`, `.github/workflows/win.yml`

- [ ] **Step 1:** Добавить `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` — в релизе консольного окна быть не должно, в отладочной сборке оно нужно для логов.
- [ ] **Step 2:** В CI добавить `cargo build --release -p proxypilot-app` и выгрузку `proxypilot.exe` артефактом рядом с существующим `proxypilot-bridge.exe`.
- [ ] **Step 3:** Прогнать все три проверки локально, закоммитить: `ci(win): сборка приложения`.

Автозапуск, подпись и обновления — план 3.

---

### Task 8: Супервизор

**Files:**
- Create: `win/crates/bridge/src/supervisor.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Router`, `Prober`, `Config`, `decide`, `list_connected`, `watch_network_changes`.
- Produces: `Supervisor::new(...)`, `Supervisor::run(self)`, `Supervisor::state() -> AppState`, `AppState { mode, route, demoted, place, health, port }`.

**Что делает.** На старте и на каждое событие смены сети: спросить NLM список подключённых сетей → `config.place_for` → `prober.health` → `core::decide` → **если решение изменилось, `router.set()`**. Мост при этом не трогается.

**Инвариант, который здесь надо записать и в код, и в комментарий:**

```rust
// Слушатель привязывается один раз за жизнь процесса и не перепривязывается.
// Супервизор меняет ТОЛЬКО маршрут — через router.set(), который не касается
// установленных соединений. Смена порта требует перезапуска моста и обязана
// быть явным действием пользователя: тихая перепривязка убьёт то самое
// свойство, ради которого продукт переписан.
```

- [ ] **Step 1: Тесты на чистую часть** — таблица «место + живость → ожидаемый вызов `router.set` или его отсутствие», с подставными источником сетей и пробером.
- [ ] **Step 2: Тест «решение не изменилось — `set` не вызывается»** — лишний `set` безвреден, но он маскирует ошибки в логике и мешает логу.
- [ ] **Step 3: Реализация.**
- [ ] **Step 4: Коммит** — `feat(win): супервизор — пересчёт маршрута на смену сети`.

---

### Task 9: Трей

**Files:**
- Create: `win/crates/app/Cargo.toml`, `win/crates/app/src/main.rs`, `win/crates/app/src/tray.rs`
- Modify: `win/Cargo.toml`

**Устройство процесса.** `tray-icon` требует оконный цикл сообщений на своём потоке, а на Windows — практически главный поток. tokio-рантайм моста поднимается отдельно. Связь — уже существующий `Router` через `ArcSwap` плюс канал команд.

```
главный поток            tokio runtime
─────────────            ─────────────
цикл сообщений    ←──→   serve()  (мост)
трей + меню              supervisor (пересчёт маршрута)
      │                        │
      └──── Router (ArcSwap) ──┘
```

Существующая архитектура ложится сюда без единой правки в мосте — это прямое следствие того, что маршрут уже живёт в атомарной ячейке.

**Иконка** отражает активный маршрут: SOCKS5 · HTTP · напрямую · мост не запущен · не настроено.

**Меню** повторяет macOS-версию: заголовок с адресом моста и текущим состоянием, переключение режимов с индикаторами доступности, копирование адреса, выход. Секции сети и туннеля появятся в плане 3.

**Обязательное поведение при выходе:** восстановить системный прокси в то состояние, что было до запуска. Без этого выход из приложения оставляет машину без сети.

- [ ] Шаги по обычному циклу; ручная проверка обязательна — запустить, увидеть иконку, переключить режим, проверить `curl`, выйти и убедиться, что настройки прокси вернулись.

---

### Task 10: Сборка приложения и CI

- Приложение собирается как `windows_subsystem = "windows"` (без консольного окна), CLI-бинарь моста остаётся для отладки.
- CI: добавить сборку `proxypilot-app`, оставить существующие три проверки.
- Автозапуск и обновления — план 3.

---

## Что этот план НЕ делает

Уходит в план 3: окно настроек, `bench` и `doctor`, VPN через OpenVPN GUI, служба статического IP, автозапуск, подпись Authenticode, упаковка и обновления.
