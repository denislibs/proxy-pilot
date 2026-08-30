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
///
/// Установка подписчика — не паникующая: `try_init` вместо `init`. Второй
/// вызов (или любой другой код, уже поставивший глобальный подписчик раньше
/// нас) не должен ронять процесс на старте — это была бы деградация хуже,
/// чем сам факт отсутствия лога.
pub fn init(dir: Option<&Path>) -> Option<WorkerGuard> {
    let env = std::env::var(ENV_VAR).ok();
    let filter = EnvFilter::new(filter_directive(env.as_deref()));

    match dir {
        None => {
            // Подписчик уже есть — значит, логи и так куда-то идут; сообщать
            // об этом некуда, поскольку канала для сообщения (того самого
            // подписчика) у нас с чистого листа и не было.
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .try_init();
            None
        }
        Some(dir) => {
            let appender = tracing_appender::rolling::daily(dir, LOG_FILE_PREFIX);
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(writer)
                .try_init();
            Some(guard)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_defaults_to_info_and_honours_the_env_var() {
        // Без переменной — info: в бою нужен спокойный лог.
        assert_eq!(filter_directive(None), "proxypilot=info");
        // С переменной — что попросили, чтобы можно было поднять уровень
        // на месте, не пересобирая.
        assert_eq!(
            filter_directive(Some("proxypilot=debug")),
            "proxypilot=debug"
        );
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
