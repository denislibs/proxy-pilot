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

