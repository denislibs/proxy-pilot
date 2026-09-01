//! Оркестрация фоновой проверки обновлений: сеть → сравнение версий →
//! скачивание → проверка подписи → откладывание к следующему запуску.
//!
//! **Не блокирует старт.** [`run`] — `async fn`, а единственная блокирующая
//! работа (сетевые вызовы [`super::source::UpdateSource`], запись файла)
//! уходит в `tokio::task::spawn_blocking`, обёрнутый `tokio::time::timeout`:
//! зависшая сеть роняет проверку по таймауту, а не поток исполнителя.
//! Вызывающий (`main.rs`) обязан звать `run` через `tokio::spawn`, а не
//! `block_on` — сама функция границу «не блокировать» держит только внутри
//! себя, снаружи её легко нарушить неправильным вызовом, и тест
//! `a_hanging_source_does_not_block_the_caller_beyond_the_timeout` проверяет
//! именно внутреннюю границу.
//!
//! **Установка только при подтверждённой подписи.** [`stage`] качает файл во
//! временный путь и переименовывает его в «отложенный к установке» ТОЛЬКО
//! если `verify` (в проде — [`super::verify::verify_authenticode`]) вернула
//! `Ok`. Любой другой исход [`stage`] — файл удаляется, ничего не
//! откладывается, [`CheckOutcome::RefusedUnsigned`] — не «установлено без
//! проверки» и не «молча пропущено» (докблок `super::verify`).
//!
//! **Выключатель — только здесь.** `enabled` гасит любое обращение к сети в
//! самом начале [`run`] (тест `disabled_check_never_touches_the_source`);
//! ничего похожего нет и не может быть у `verify` — единственный тумблер
//! продукта относится к ЭТОЙ проверке, не к подписи уже скачанного файла.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::source::{ReleaseInfo, UpdateSource};
use super::version::{self, Decision};

/// Сколько ждём сетевую часть целиком (запрос к API плюс, если найдено
/// обновление, скачивание файла), прежде чем считать сеть недоступной.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const PARTIAL_NAME: &str = "staged.exe.partial";
/// Имя файла в каталоге обновлений, чьё присутствие И ЕСТЬ отметка
/// «обновление отложено к следующему запуску» ([`super::install`]). Второй,
/// отдельный файл-маркер не заведён нарочно: два источника истины (файл +
/// маркер) могут разойтись, а один — не может.
pub const STAGED_NAME: &str = "staged.exe";

/// Итог одной проверки — то, что покажет страница настроек буквально.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Тумблер выключен — до сети не дошло вовсе.
    Disabled,
    UpToDate,
    /// Собственная версия новее опубликованного тега.
    CurrentIsNewer,
    /// Опубликованный тег не разобрался как версия.
    Unrecognized,
    /// Опубликован только предрелиз — не предлагается к установке.
    PublishedIsPrerelease(String),
    /// Сеть недоступна, GitHub ответил неожиданным кодом, или проверка не
    /// уложилась в срок. НЕ путать с [`CheckOutcome::UpToDate`] — это
    /// разные исходы, и приёмка задачи прямо требует не путать один с
    /// другим на экране.
    Failed(String),
    /// Найдено обновление, файл скачан, подпись подтверждена — отложено к
    /// следующему запуску ([`super::install::apply_pending_update`]).
    StagedForNextLaunch {
        tag: String,
    },
    /// Найдено обновление, но подпись отсутствует или неверна — файл
    /// удалён, установка НЕ произошла.
    RefusedUnsigned {
        tag: String,
        reason: String,
    },
}

/// Функция проверки подписи как параметр, а не жёстко вшитый вызов
/// [`super::verify::verify_authenticode`]: тесты этого файла подставляют
/// управляемую заглушку (принять/отказать по требованию теста), а не гоняют
/// настоящий `WinVerifyTrust` на каждый прогон — тот путь уже покрыт
/// собственными тестами `super::verify` на реальных файлах системы. Простой
/// указатель на функцию, а не `Box<dyn Fn>`: подмена нужна только на
/// «всегда OK» / «всегда отказ», без захвата состояния.
pub type Verifier = fn(&Path) -> Result<(), String>;

/// Полная проверка: сеть → сравнение версий → (если найдено) скачивание и
/// подпись. `current_version` — `CARGO_PKG_VERSION` вызывающего.
pub async fn run(
    current_version: String,
    enabled: bool,
    source: Arc<dyn UpdateSource>,
    update_dir: PathBuf,
    verify: Verifier,
    timeout: Duration,
) -> CheckOutcome {
    if !enabled {
        return CheckOutcome::Disabled;
    }
    let handle = tokio::task::spawn_blocking(move || {
        run_sync(&current_version, source.as_ref(), &update_dir, verify)
    });
    match tokio::time::timeout(timeout, handle).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_join_error)) => {
            CheckOutcome::Failed("проверка обновлений завершилась паникой".to_string())
        }
        // Таймаут НЕ отменяет уже запущенный `spawn_blocking`: поток
        // блокирующего пула доработает сам по себе (или всё же дозвонится
        // позже), но вызывающий этой функции его больше не ждёт — старт
        // приложения не задерживается сетью ни на секунду сверх этого
        // предела.
        Err(_) => CheckOutcome::Failed(format!(
            "проверка обновлений не уложилась в {timeout:?} — сеть недоступна или медленная"
        )),
    }
}

fn run_sync(
    current: &str,
    source: &dyn UpdateSource,
    update_dir: &Path,
    verify: Verifier,
) -> CheckOutcome {
    let release = match source.latest_release() {
        Ok(r) => r,
        Err(e) => return CheckOutcome::Failed(e),
    };
    match version::decide(current, &release.tag) {
        Decision::UpToDate => CheckOutcome::UpToDate,
        Decision::CurrentIsNewer => CheckOutcome::CurrentIsNewer,
        Decision::Unrecognized => CheckOutcome::Unrecognized,
        Decision::PublishedIsPrerelease(v) => {
            CheckOutcome::PublishedIsPrerelease(format_version(&v))
        }
        Decision::Available(_) => stage(source, update_dir, &release, verify),
    }
}

fn format_version(v: &version::Version) -> String {
    match &v.pre {
        Some(pre) => format!("v{}.{}.{}-{pre}", v.major, v.minor, v.patch),
        None => format!("v{}.{}.{}", v.major, v.minor, v.patch),
    }
}

/// Качает ассет, проверяет подпись, и только при успехе откладывает файл к
/// следующему запуску. Любой другой исход убирает временный файл за собой —
/// каталог обновлений не должен копить недокачанные или отвергнутые файлы.
fn stage(
    source: &dyn UpdateSource,
    update_dir: &Path,
    release: &ReleaseInfo,
    verify: Verifier,
) -> CheckOutcome {
    if let Err(e) = std::fs::create_dir_all(update_dir) {
        return CheckOutcome::Failed(format!("не создать {}: {e}", update_dir.display()));
    }
    let partial = update_dir.join(PARTIAL_NAME);
    if let Err(e) = source.download(&release.asset_url, &partial) {
        let _ = std::fs::remove_file(&partial);
        return CheckOutcome::Failed(format!("скачивание обновления не удалось: {e}"));
    }
    match verify(&partial) {
        Ok(()) => {
            let staged = update_dir.join(STAGED_NAME);
            if let Err(e) = std::fs::rename(&partial, &staged) {
                let _ = std::fs::remove_file(&partial);
                return CheckOutcome::Failed(format!(
                    "скачанный файл прошёл проверку подписи, но не отложился: {e}"
                ));
            }
            CheckOutcome::StagedForNextLaunch {
                tag: release.tag.clone(),
            }
        }
        Err(reason) => {
            // Подпись отсутствует или неверна — файл убирается, ничего не
            // откладывается. Это и есть правило «не установлено без
            // проверки, не пропущено молча»: молчания здесь нет —
            // `RefusedUnsigned` несёт причину дальше, до страницы настроек.
            let _ = std::fs::remove_file(&partial);
            CheckOutcome::RefusedUnsigned {
                tag: release.tag.clone(),
                reason,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn always_ok(_: &Path) -> Result<(), String> {
        Ok(())
    }

    fn always_refuses(_: &Path) -> Result<(), String> {
        Err("тестовый отказ подписи".to_string())
    }

    struct FakeSource {
        release: Result<ReleaseInfo, String>,
        download_bytes: Result<Vec<u8>, String>,
        release_calls: Arc<AtomicBool>,
        download_calls: Arc<AtomicBool>,
        /// Реальная задержка ПЕРЕД ответом — единственный способ честно
        /// проверить, что таймаут `run` не ждёт вечно: настоящий поток
        /// `spawn_blocking` не отменяется по требованию, а «не блокирует
        /// вызывающего дольше срока» — это именно про то, что `run`
        /// возвращается вовремя, даже если поток где-то там всё ещё спит.
        stall: Duration,
    }

    impl UpdateSource for FakeSource {
        fn latest_release(&self) -> Result<ReleaseInfo, String> {
            self.release_calls.store(true, Ordering::SeqCst);
            if !self.stall.is_zero() {
                std::thread::sleep(self.stall);
            }
            self.release.clone()
        }

        fn download(&self, _url: &str, dest: &Path) -> Result<(), String> {
            self.download_calls.store(true, Ordering::SeqCst);
            match &self.download_bytes {
                Ok(bytes) => std::fs::write(dest, bytes).map_err(|e| e.to_string()),
                Err(e) => Err(e.clone()),
            }
        }
    }

    fn fake(release: Result<ReleaseInfo, String>) -> (Arc<FakeSource>, Arc<AtomicBool>) {
        let release_calls = Arc::new(AtomicBool::new(false));
        let src = Arc::new(FakeSource {
            release,
            download_bytes: Ok(b"fake-exe-bytes".to_vec()),
            release_calls: Arc::clone(&release_calls),
            download_calls: Arc::new(AtomicBool::new(false)),
            stall: Duration::ZERO,
        });
        (src, release_calls)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("proxypilot-test-update-check")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn disabled_check_never_touches_the_source() {
        let (src, called) = fake(Err("сеть не должна была спрашиваться".to_string()));
        let outcome = run(
            "1.0.0".to_string(),
            false,
            src,
            temp_dir("disabled"),
            always_ok,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(outcome, CheckOutcome::Disabled);
        assert!(
            !called.load(Ordering::SeqCst),
            "выключенная проверка не должна была спрашивать источник"
        );
    }

    #[tokio::test]
    async fn a_network_failure_is_reported_as_failed_not_up_to_date() {
        let (src, _) = fake(Err("нет соединения".to_string()));
        let outcome = run(
            "1.0.0".to_string(),
            true,
            src,
            temp_dir("net-fail"),
            always_ok,
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(outcome, CheckOutcome::Failed(_)), "{outcome:?}");
        assert_ne!(
            outcome,
            CheckOutcome::UpToDate,
            "отказ сети не должен читаться как «всё в порядке»"
        );
    }

    #[tokio::test]
    async fn a_hanging_source_does_not_block_the_caller_beyond_the_timeout() {
        // Сеть может быть недоступна ровно тогда, когда прокси и нужен —
        // это и есть требование приёмки. Источник «зависает» на секунду
        // реального времени; таймаут короче на два порядка — вызывающий
        // обязан получить ответ по таймауту, а не дождаться источника.
        let release_calls = Arc::new(AtomicBool::new(false));
        let src = Arc::new(FakeSource {
            release: Ok(ReleaseInfo {
                tag: "v9.9.9".to_string(),
                asset_url: "https://example.internal/x.exe".to_string(),
            }),
            download_bytes: Ok(Vec::new()),
            release_calls: Arc::clone(&release_calls),
            download_calls: Arc::new(AtomicBool::new(false)),
            stall: Duration::from_secs(1),
        });

        let started = std::time::Instant::now();
        let outcome = run(
            "1.0.0".to_string(),
            true,
            src,
            temp_dir("hanging"),
            always_ok,
            Duration::from_millis(50),
        )
        .await;
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(500),
            "вызывающий прождал {elapsed:?} — таймаут не сработал"
        );
        assert!(matches!(outcome, CheckOutcome::Failed(_)), "{outcome:?}");
        assert_ne!(outcome, CheckOutcome::UpToDate);
    }

    #[tokio::test]
    async fn an_up_to_date_current_version_is_reported_as_such() {
        let (src, _) = fake(Ok(ReleaseInfo {
            tag: "v1.0.0".to_string(),
            asset_url: "https://example.internal/x.exe".to_string(),
        }));
        let outcome = run(
            "1.0.0".to_string(),
            true,
            src,
            temp_dir("up-to-date"),
            always_ok,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(outcome, CheckOutcome::UpToDate);
    }

    #[tokio::test]
    async fn a_prerelease_is_reported_and_never_downloaded() {
        let release_calls = Arc::new(AtomicBool::new(false));
        let download_calls = Arc::new(AtomicBool::new(false));
        let src = Arc::new(FakeSource {
            release: Ok(ReleaseInfo {
                tag: "v2.0.0-rc.1".to_string(),
                asset_url: "https://example.internal/x.exe".to_string(),
            }),
            download_bytes: Ok(Vec::new()),
            release_calls,
            download_calls: Arc::clone(&download_calls),
            stall: Duration::ZERO,
        });
        let outcome = run(
            "1.0.0".to_string(),
            true,
            src,
            temp_dir("prerelease"),
            always_ok,
            Duration::from_secs(5),
        )
        .await;
        assert!(
            matches!(outcome, CheckOutcome::PublishedIsPrerelease(_)),
            "{outcome:?}"
        );
        assert!(
            !download_calls.load(Ordering::SeqCst),
            "предрелиз не должен качаться вовсе"
        );
    }

    #[tokio::test]
    async fn a_valid_signature_stages_the_update_for_the_next_launch() {
        let dir = temp_dir("staged-ok");
        let (src, _) = fake(Ok(ReleaseInfo {
            tag: "v9.0.0".to_string(),
            asset_url: "https://example.internal/x.exe".to_string(),
        }));
        let outcome = run(
            "1.0.0".to_string(),
            true,
            src,
            dir.clone(),
            always_ok,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            outcome,
            CheckOutcome::StagedForNextLaunch {
                tag: "v9.0.0".to_string()
            }
        );
        assert!(dir.join(STAGED_NAME).exists(), "файл обязан быть отложен");
        assert!(
            !dir.join(PARTIAL_NAME).exists(),
            "временный файл обязан быть убран"
        );
    }

    #[tokio::test]
    async fn an_invalid_signature_refuses_the_update_and_leaves_nothing_staged() {
        // Приёмка задачи: отказ на неверной подписи — файл не отложен, не
        // установлен без проверки, не пропущен молча.
        let dir = temp_dir("staged-bad-sig");
        let (src, _) = fake(Ok(ReleaseInfo {
            tag: "v9.0.0".to_string(),
            asset_url: "https://example.internal/x.exe".to_string(),
        }));
        let outcome = run(
            "1.0.0".to_string(),
            true,
            src,
            dir.clone(),
            always_refuses,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            outcome,
            CheckOutcome::RefusedUnsigned {
                tag: "v9.0.0".to_string(),
                reason: "тестовый отказ подписи".to_string()
            }
        );
        assert!(
            !dir.join(STAGED_NAME).exists(),
            "неподписанный файл не должен становиться отложенным обновлением"
        );
        assert!(
            !dir.join(PARTIAL_NAME).exists(),
            "временный файл обязан быть убран"
        );
    }

    #[tokio::test]
    async fn a_download_failure_stages_nothing() {
        let dir = temp_dir("download-fail");
        let release_calls = Arc::new(AtomicBool::new(false));
        let src = Arc::new(FakeSource {
            release: Ok(ReleaseInfo {
                tag: "v9.0.0".to_string(),
                asset_url: "https://example.internal/x.exe".to_string(),
            }),
            download_bytes: Err("тестовый обрыв соединения".to_string()),
            release_calls,
            download_calls: Arc::new(AtomicBool::new(false)),
            stall: Duration::ZERO,
        });
        let outcome = run(
            "1.0.0".to_string(),
            true,
            src,
            dir.clone(),
            always_ok,
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(outcome, CheckOutcome::Failed(_)), "{outcome:?}");
        assert!(!dir.join(STAGED_NAME).exists());
    }
}
