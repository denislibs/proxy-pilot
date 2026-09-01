//! Единственное место в крейте, которое реально запускает `netsh` — с
//! таймаутом и проверкой кода возврата и вывода.
//!
//! И служба (`main.rs::apply_action`), и `install::uninstall` (откат в DHCP
//! перед удалением, ревью round 2) делят этот код, а не дублируют его: обе
//! стороны обязаны одинаково узнавать об отказе `netsh`, а не расходиться в
//! том, что считается успехом.
//!
//! Ревью round 2 (задача 6), Critical №1: раньше вызывающий код проверял
//! только то, запустился ли процесс (`Command::status()` возвращает
//! `Ok(ExitStatus)` даже когда `netsh` сам отказал — неверное имя
//! адаптера, адаптер пропал, отказано в доступе). Служба работает от
//! LocalSystem без интерактивной консоли — стандартный вывод `netsh`
//! просто исчезает, и отказ становится не просто тихим, а буквально
//! невидимым никаким способом, кроме чтения кода возврата и текста
//! ошибки явно, что и делает эта функция.
//!
//! Ревью round 2, Important №6: `netsh` может зависнуть (например, служба
//! `wmiApSrv`, к которой она иногда стучится, сама не отвечает); цикл
//! службы синхронный и однопоточный (докблок `main.rs`), поэтому зависшая
//! команда останавливает вообще всё, включая обработку `SERVICE_CONTROL_STOP`
//! до следующего пробуждения канала. Таймаут — единственное, что не даёт
//! этому стать зависанием навсегда.

use std::process::{Command, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use tracing::error;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

/// Сколько ждать одну команду `netsh`, прежде чем принудительно её
/// завершить. `netsh` обычно отвечает за миллисекунды; 10 секунд — щедрый
/// запас на медленную машину, но не бесконечность.
pub const NETSH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum NetshError {
    #[error("не удалось запустить netsh: {0}")]
    Spawn(std::io::Error),
    #[error("не удалось создать поток ожидания netsh: {0}")]
    WaiterThread(std::io::Error),
    #[error("netsh не завершился за {0:?} — процесс принудительно остановлен")]
    TimedOut(Duration),
    #[error("netsh завершился с кодом {code:?}: {stderr}")]
    NonZeroExit { code: Option<i32>, stderr: String },
}

/// Запускает одну команду `netsh`, ждёт её с таймаутом, проверяет код
/// возврата. `Ok(())` означает именно то, что сказано в докблоке модуля:
/// процесс запустился, завершился в срок и отчитался кодом `0`. Любое
/// другое сочетание — `Err` с текстом, который стоит логировать вызывающей
/// стороне вместе с самой командой (эта функция текст самой команды не
/// знает и не логирует).
pub fn run_netsh(cmd: &mut Command) -> Result<(), NetshError> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = cmd.spawn().map_err(NetshError::Spawn)?;
    let pid = child.id();

    // `wait_with_output` блокирует и вычитывает стандартные потоки за нас
    // (без этого пайпы могли бы забиться и создать взаимную блокировку,
    // если бы мы читали и ждали раздельно) — но блокирует именно ЭТОТ
    // поток, поэтому она уезжает в отдельный, а здесь мы ждём результат с
    // таймаутом через канал.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("proxypilot-netsvc-netsh-wait".to_owned())
        .spawn(move || {
            let _ = tx.send(child.wait_with_output());
        })
        .map_err(NetshError::WaiterThread)?;

    match rx.recv_timeout(NETSH_TIMEOUT) {
        Ok(Ok(output)) => {
            if output.status.success() {
                Ok(())
            } else {
                Err(NetshError::NonZeroExit {
                    code: output.status.code(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                })
            }
        }
        Ok(Err(e)) => Err(NetshError::Spawn(e)),
        Err(RecvTimeoutError::Timeout) => {
            kill_process(pid);
            Err(NetshError::TimedOut(NETSH_TIMEOUT))
        }
        // Отправитель пропал, не отправив ничего — поток ожидания сам
        // запаниковал. Процесс всё ещё может быть жив; на всякий случай
        // тоже останавливаем его, а не оставляем висеть незамеченным.
        Err(RecvTimeoutError::Disconnected) => {
            kill_process(pid);
            Err(NetshError::TimedOut(NETSH_TIMEOUT))
        }
    }
}

/// Принудительно останавливает процесс по PID после таймаута. Ошибку
/// глушим сознательно: процесс мог уже сам завершиться в микроскопическом
/// окне между истечением таймаута и этим вызовом — это не отказ, а
/// нормальная гонка, и настаивать здесь больше не на чем.
fn kill_process(pid: u32) {
    // SAFETY: `pid` — идентификатор процесса, который эта же функция только
    // что запустила (`Command::spawn`); `OpenProcess` сама проверяет, жив
    // ли он ещё, и возвращает ошибку, если нет, а не портит память.
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(handle, 1);
            // SAFETY: `handle` получен от `OpenProcess` строкой выше и
            // больше нигде не используется.
            let _ = CloseHandle(handle);
        }
    }
}

/// Прогоняет пачку команд по очереди, останавливаясь на первой неудаче:
/// команды в пачке (адрес, затем DNS) как правило зависят друг от друга по
/// смыслу — выполнять DNS-команду для адреса, который не встал, значило бы
/// маскировать первый отказ вторым. Возвращает `true`, только если
/// абсолютно все команды пачки завершились успехом.
pub fn run_netsh_batch(cmds: Vec<Command>) -> bool {
    for mut cmd in cmds {
        match run_netsh(&mut cmd) {
            Ok(()) => {}
            Err(e) => {
                error!(error = %e, command = ?cmd, "команда netsh не выполнилась");
                return false;
            }
        }
    }
    true
}
