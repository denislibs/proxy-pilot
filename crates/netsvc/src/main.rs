//! Вход Service Control Manager и главный цикл службы `ProxyPilotNetProfile`.
//!
//! Ничего в этом файле не проверяется автотестами и не может быть проверено
//! иначе, чем запуском настоящей службы диспетчером Windows: `main` либо
//! получает управление от `StartServiceCtrlDispatcherW` (а это происходит,
//! только когда процесс запущен именно как служба SCM), либо сразу отказывает
//! с понятной ошибкой в консоли, если запущен руками. Вся логика решения уже
//! проверена в библиотечных модулях крейта:
//! - `proxypilot_core::netprofile::decide_profile` — что делать (задача 5);
//! - `netsh_cmd` — какими командами это выразить;
//! - `safety` — когда откатывать в DHCP;
//! - `profile`/`state` — откуда брать данные;
//! - `exec` — как исполнить `netsh` и как проверить, что он действительно
//!   исполнился.
//!
//! Этот файл только связывает их с реальным вводом-выводом: чтением сети
//! через NLM, применением через `exec::run_netsh_batch` и проверкой шлюза
//! ICMP-пингом. Контроллер сессии прямо запрещает как устанавливать эту
//! службу, так и исполнять `netsh interface ipv4 set address`/`set
//! dnsservers` на машине разработки (`CLAUDE.md`, «Живые проверки, которые
//! не делает агент») — поэтому ни разу не запускался и не мог быть запущен
//! в этой сессии. Первый живой прогон — дело человека, в офисной сети (см.
//! отчёт задачи).
//!
//! ## Три правила, добавленные ревью round 2, без которых цикл ниже был
//! бы классом тихих отказов
//!
//! 1. **Ничего не считается применённым, пока не перечитано.** И
//!    применение статики, и откат в DHCP заканчиваются повторным чтением
//!    `adapter::current_ipv4_config` — код возврата `netsh` (уже
//!    проверяемый `exec::run_netsh`) доказывает только то, что сам процесс
//!    отчитался нулём, а не то, что адаптер действительно несёт нужное
//!    состояние.
//! 2. **`state::AppliedState` не перезаписывается, пока действие не
//!    подтверждено.** Если откат в DHCP не подтверждён, запись «это наша
//!    статика» остаётся как есть — иначе следующий цикл видит адаптер как
//!    ЧУЖУЮ статику и по `foreign_static_address_is_never_reset` (задача
//!    5) не трогает её никогда: машина застряла бы на офисном адресе с
//!    мёртвым шлюзом навсегда, а не «до следующей успешной попытки».
//! 3. **Латч против дребезга** (`check_and_maintain_latch`): без него
//!    единственный ICMP-пакет, потерянный корпоративным шлюзом (обычное
//!    дело), даёт откат → тик таймера → снова в офисе на DHCP → снова
//!    `SetStatic` → снова откат, вечно, на живом адаптере.

use std::ffi::c_void;
use std::net::Ipv4Addr;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use proxypilot_core::config::Config;
use proxypilot_core::mode::ConnectedNetwork;
use proxypilot_core::netprofile::{decide_profile, AdapterConfig, ProfileAction};
use proxypilot_netsvc::adapter::{
    self, current_ipv4_config, find_office_adapter, gather_from_nlm, CurrentIpv4Config,
};
use proxypilot_netsvc::exec;
use proxypilot_netsvc::netsh_cmd::commands_for_action;
use proxypilot_netsvc::profile::ServiceProfile;
use proxypilot_netsvc::safety::{evaluate_gateway, SafetyNetOutcome};
use proxypilot_netsvc::state::{self, AppliedState};
use proxypilot_netsvc::{profile, SERVICE_NAME};
use proxypilot_winnet::com::ComGuard;
use proxypilot_winnet::networks::list_connected;
use tracing::{error, info, warn};
use windows::core::{Error as WinError, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_CALL_NOT_IMPLEMENTED, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY, IP_SUCCESS,
};
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_CONTROL_INTERROGATE,
    SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOPPED,
    SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};

/// Как часто пересчитывать решение просто по времени — та же подстраховка,
/// что и в приложении (`app::REEVALUATE_PERIOD`): подписка на NLM может не
/// подняться вовсе или пропустить смену, которая не двигает агрегат
/// связности (докблок `winnet::events`).
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Пауза перед первым пингом шлюза после применения статики. Сразу после
/// `netsh` адрес может быть ещё не «устоявшимся» (ARP для нового адреса,
/// внутренние задержки стека) — пинг вслепую через 0 мс рискует принять
/// «ещё не устоялось» за «шлюз не отвечает» (ревью round 2, Critical №4).
const GATEWAY_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// Сколько раз пробовать ICMP-эхо, прежде чем признать шлюз недостижимым.
/// Один потерянный пакет — обычное дело для корпоративного шлюза; решение
/// «откатывать в DHCP» не должно зависеть от одного broadcast'а ARP.
const GATEWAY_PING_ATTEMPTS: u32 = 3;
const GATEWAY_PING_RETRY_DELAY: Duration = Duration::from_secs(1);
const GATEWAY_PING_TIMEOUT: Duration = Duration::from_secs(2);

/// `wait_hint`, с которым `STOP_PENDING` сообщается SCM: щедрый запас на
/// худший случай (несколько команд `netsh`, каждая до
/// `exec::NETSH_TIMEOUT`, плюс отстойник и несколько попыток пинга). Это
/// одна отметка без наращивания чекпоинта — упрощение, а не полноценная
/// периодическая отчётность о прогрессе остановки; честно об этом в
/// отчёте задачи.
const STOP_PENDING_WAIT_HINT_MS: u32 = 30_000;

fn main() {
    // Консоль есть только если процесс запущен руками (не диспетчером
    // служб) — то есть уже в состоянии, которого контроллер сессии не
    // допускает никогда. Сообщение существует ради человека, который решит
    // проверить сборку так, а не ради этой сессии.
    let name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR::from_raw(name.as_ptr() as *mut u16),
            lpServiceProc: Some(ffi_service_main),
        },
        // Терминатор массива — обязателен по контракту
        // `StartServiceCtrlDispatcherW`, оба поля нулевые.
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR::null(),
            lpServiceProc: None,
        },
    ];

    // SAFETY: `table` — живой массив из двух корректно заполненных записей
    // (докблок функции про терминатор), `name` живёт до конца этого вызова
    // (сам вызов блокируется до остановки службы, дольше `main` буфер не
    // нужен). Вызов либо не возвращается, пока служба не остановлена, либо
    // сразу отказывает, если процесс не был запущен диспетчером служб — это
    // не отказ этого кода, а единственный законный способ запустить службу
    // не так, как задумано.
    if let Err(e) = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } {
        eprintln!(
            "{SERVICE_NAME}: не запущено диспетчером служб Windows ({e}). \
             Эта программа — не консольный инструмент; установка и запуск \
             делаются командами `install-service`/`uninstall-service` \
             приложения, с правами администратора."
        );
        std::process::exit(1);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Что будит главный цикл: смена сети, тик таймера или просьба остановиться.
enum LoopEvent {
    Network,
    Tick,
    Stop,
}

/// Канал, которым `ffi_control_handler` (вызывается SCM на СВОЁМ потоке)
/// будит главный цикл, крутящийся в `run_loop` на другом потоке.
/// `Mutex`, а не голый `Sender`, ради `Sync`: `static` обязана быть
/// потокобезопасной, а `mpsc::Sender` сам по себе `Sync` не гарантирован.
static LOOP_TX: OnceLock<Mutex<Option<Sender<LoopEvent>>>> = OnceLock::new();

/// Обёртка над `SERVICE_STATUS_HANDLE`, которую можно положить в `static`.
///
/// SAFETY: `SERVICE_STATUS_HANDLE` — непрозрачный дескриптор SCM; и
/// `RegisterServiceCtrlHandlerExW`, и `SetServiceStatus` документированы
/// как вызываемые с ним с любого потока процесса (это и есть весь смысл
/// пары этих функций — служба обязана уметь сообщить статус из потока,
/// не совпадающего с тем, что его зарегистрировал). Сам дескриптор не
/// адресует память процесса напрямую — пересылка его значения между
/// потоками безопасна ровно в тех пределах, которые документирует Win32.
struct SendableStatusHandle(SERVICE_STATUS_HANDLE);
unsafe impl Send for SendableStatusHandle {}
unsafe impl Sync for SendableStatusHandle {}

/// Дескриптор статуса, сохранённый сразу после регистрации — нужен
/// `ffi_control_handler`, чтобы сообщить `STOP_PENDING` немедленно, с
/// постороннего потока, не дожидаясь, пока главный цикл сам дочитает канал
/// (ревью round 2, Important №6).
static STATUS_HANDLE: OnceLock<Mutex<Option<SendableStatusHandle>>> = OnceLock::new();

fn store_status_handle(handle: SERVICE_STATUS_HANDLE) {
    *STATUS_HANDLE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("мьютекс дескриптора статуса не отравлен — паник в критической секции не бывает") =
        Some(SendableStatusHandle(handle));
}

fn report_status_from_any_thread(state: SERVICE_STATUS_CURRENT_STATE, wait_hint_ms: u32) {
    let Some(lock) = STATUS_HANDLE.get() else {
        return;
    };
    let Ok(guard) = lock.lock() else {
        return;
    };
    let Some(handle) = guard.as_ref() else {
        return;
    };
    let _ = report_status(handle.0, state, wait_hint_ms);
}

/// Единственная точка входа, которую вызывает `StartServiceCtrlDispatcherW`
/// на выделенном для этой службы потоке.
///
/// SAFETY: `unsafe extern "system"` здесь — требование самой сигнатуры
/// `LPSERVICE_MAIN_FUNCTIONW`, под которую SCM ищет функцию по указателю в
/// `SERVICE_TABLE_ENTRYW` (`main`, где эта запись строится). Тело не
/// разыменовывает `_argc`/`_argv` вовсе (аргументы командной строки службы
/// этому коду не нужны).
///
/// Паника внутри `run_loop` оборачивается `catch_unwind` (ревью round 2,
/// Important №10): необработанная паника, вышедшая наружу через границу
/// FFI, — неопределённое поведение по правилам Rust и на практике
/// аварийно останавливает процесс, не оставив SCM ни единого шанса узнать
/// статус. `report_status(..., SERVICE_STOPPED, ...)` в конце вызывается
/// БЕЗУСЛОВНО, что бы ни случилось выше (паника или обычный выход из
/// цикла) — раньше (до ревью round 2) ранний `Err` внутри могло привести
/// к тому, что `STOPPED` не сообщался вовсе, и служба выглядела бы для SCM
/// зависшей в `START_PENDING`/`RUNNING`.
unsafe extern "system" fn ffi_service_main(_argc: u32, _argv: *mut PWSTR) {
    // Подписчик логов — до первой же строки лога: если он не поднимется,
    // молчание отсюда неотличимо от «служба ничего не делает».
    let log_dir = profile::program_data_dir().join("ProxyPilot").join("logs");
    let _log_guard = init_logging(&log_dir);

    let service_name = wide(SERVICE_NAME);
    // SAFETY: `service_name` жива до конца этого вызова; `ffi_control_handler`
    // — статическая функция, живущая весь процесс; контекст не нужен —
    // канал управления идёт через `LOOP_TX`, а не через контекст обработчика.
    let handle = match unsafe {
        RegisterServiceCtrlHandlerExW(
            PCWSTR::from_raw(service_name.as_ptr()),
            Some(ffi_control_handler),
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            error!(
                error = %e,
                "не удалось зарегистрировать обработчик управления службой — \
                 завершаюсь, не сообщив SCM ни одного статуса"
            );
            return;
        }
    };
    store_status_handle(handle);

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_loop(handle)));
    if outcome.is_err() {
        error!("главный цикл службы запаниковал — сообщаю SCM STOPPED и завершаюсь");
    }
    if let Err(e) = report_status(handle, SERVICE_STOPPED, 0) {
        error!(error = %e, "не удалось сообщить SCM финальный статус STOPPED");
    }
}

fn init_logging(dir: &std::path::Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    if std::fs::create_dir_all(dir).is_err() {
        return None;
    }
    let appender = tracing_appender::rolling::daily(dir, "netsvc");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("proxypilot_netsvc=info"))
        .with_ansi(false)
        .with_writer(writer)
        .try_init();
    Some(guard)
}

#[derive(Debug, thiserror::Error)]
enum ServiceError {
    #[error("не удалось сообщить SCM о статусе службы: {0}")]
    SetStatus(WinError),
}

/// Регистрирует фоновые потоки, поднимает статус `RUNNING` и крутит
/// главный цикл до просьбы остановиться. Не возвращает `Result` и не
/// пробрасывает отказы наружу через `?` — вызывающая сторона
/// (`ffi_service_main`) обязана дойти до конца и сообщить `STOPPED`
/// независимо от того, что случилось здесь; отказы логируются на месте.
fn run_loop(handle: SERVICE_STATUS_HANDLE) {
    let (tx, rx) = channel::<LoopEvent>();
    *LOOP_TX
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("мьютекс канала не отравлен — паник в критической секции не бывает") =
        Some(tx.clone());

    if let Err(e) = report_status(handle, SERVICE_START_PENDING, 0) {
        warn!(error = %e, "не удалось сообщить SCM статус START_PENDING — продолжаю всё равно");
    }

    // Мост «поток подписки NLM» → «наш общий канал». `watch_network_changes`
    // сама поднимает выделенный поток с циклом сообщений (докблок
    // `winnet::events`) и не требует рантайма tokio для этого; здесь только
    // откачиваем её канал блокирующим чтением на СВОЁМ потоке — тем же
    // приёмом, которым бы это делал синхронный вызывающий без tokio.
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("proxypilot-netsvc-netwatch".to_owned())
            .spawn(move || bridge_network_events(tx))
            .ok();
    }
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("proxypilot-netsvc-poll".to_owned())
            .spawn(move || poll_thread(tx))
            .ok();
    }

    if let Err(e) = report_status(handle, SERVICE_RUNNING, 0) {
        warn!(error = %e, "не удалось сообщить SCM статус RUNNING — продолжаю всё равно");
    }
    info!("служба ProxyPilotNetProfile запущена");

    for event in rx.iter() {
        match event {
            LoopEvent::Stop => break,
            LoopEvent::Network | LoopEvent::Tick => {
                if let Err(e) = run_cycle() {
                    warn!(error = %e, "цикл применения профиля не выполнился");
                }
            }
        }
    }

    info!("служба ProxyPilotNetProfile останавливается");
}

fn report_status(
    handle: SERVICE_STATUS_HANDLE,
    state: SERVICE_STATUS_CURRENT_STATE,
    wait_hint_ms: u32,
) -> Result<(), ServiceError> {
    let controls_accepted = if state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING {
        0
    } else {
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls_accepted,
        dwWin32ExitCode: NO_ERROR.0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint_ms,
    };
    // SAFETY: `handle` получен от `RegisterServiceCtrlHandlerExW` и,
    // согласно документации Win32, годен для `SetServiceStatus` с любого
    // потока процесса, пока служба жива; `status` — живая локальная
    // переменная на весь вызов.
    unsafe { SetServiceStatus(handle, &status) }.map_err(ServiceError::SetStatus)
}

/// Обработчик управления SCM — вызывается на СВОЁМ потоке, отдельном от
/// `run_loop`.
///
/// SAFETY: `unsafe extern "system"` — требование сигнатуры
/// `LPHANDLER_FUNCTION_EX`, под которую SCM вызывает обработчик по
/// указателю, переданному в `RegisterServiceCtrlHandlerExW`. `_event_data`
/// и `_context` — сырые указатели, но не разыменовываются здесь ни разу
/// (управление не использует ни расширенные данные события, ни контекст —
/// канал общения с главным циклом идёт через `LOOP_TX`, статус — через
/// `STATUS_HANDLE`, ни то ни другое не через них). Тело обязано не
/// паниковать и не блокироваться надолго — оба вызова внутри
/// (`report_status_from_any_thread`, `tx.send`) неблокирующие или почти
/// мгновенные.
unsafe extern "system" fn ffi_control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            // Ревью round 2, Important №6: сообщаем SCM `STOP_PENDING`
            // немедленно, с этого потока, а не ждём, пока главный цикл сам
            // дочитает канал — он в этот момент вполне может сидеть внутри
            // `netsh` с таймаутом до `exec::NETSH_TIMEOUT` и не заметить
            // просьбу ещё несколько секунд. Без этого шага `sc stop` видел
            // бы `RUNNING` вплоть до самого момента остановки.
            report_status_from_any_thread(SERVICE_STOP_PENDING, STOP_PENDING_WAIT_HINT_MS);
            if let Some(lock) = LOOP_TX.get() {
                if let Ok(guard) = lock.lock() {
                    if let Some(tx) = guard.as_ref() {
                        let _ = tx.send(LoopEvent::Stop);
                    }
                }
            }
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}

fn bridge_network_events(tx: Sender<LoopEvent>) {
    let com = ComGuard::new();
    if let Err(e) = &com {
        warn!(error = %e, "COM на потоке подписки не поднялся — служба работает только по таймеру");
        return;
    }
    match proxypilot_winnet::events::watch_network_changes() {
        Ok(mut rx) => {
            while rx.blocking_recv().is_some() {
                if tx.send(LoopEvent::Network).is_err() {
                    return;
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "подписка на смену сети не поднялась — служба работает только по таймеру")
        }
    }
}

fn poll_thread(tx: Sender<LoopEvent>) {
    loop {
        std::thread::sleep(POLL_INTERVAL);
        if tx.send(LoopEvent::Tick).is_err() {
            return;
        }
    }
}

/// Сеть (по GUID NLM), на которой последняя попытка применить статику
/// закончилась откатом из-за недостижимого шлюза. Только в памяти
/// процесса, намеренно не на диске: перезапуск службы — обычный способ
/// человека сказать «дайте ещё одну попытку», и он обязан работать без
/// правки файлов руками. Сброс от смены сетевой идентичности (см.
/// `check_and_maintain_latch`) работает и без персистентности — тот же
/// эффект получается снова, если машина уходит и возвращается.
static LATCHED_NETWORK: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn set_latch(network_id: &str) {
    *LATCHED_NETWORK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("мьютекс латча не отравлен — паник в критической секции не бывает") =
        Some(network_id.to_string());
}

/// `true`, если латч сейчас блокирует применение статики на сети
/// `current_network`. Заодно снимает латч, если сеть другая (в том числе
/// `None`, то есть мы вообще ушли с сети) — «до смены сетевой идентичности»
/// в буквальном смысле: как только машина замечена на ДРУГОЙ сети (пусть
/// даже на секунду), прошлый отказ относится к прошлой сессии подключения,
/// и следующая попытка на офисной сети снова заслуживает шанса.
fn check_and_maintain_latch(current_network: Option<&str>) -> bool {
    let mut guard = LATCHED_NETWORK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("мьютекс латча не отравлен — паник в критической секции не бывает");
    match (guard.as_deref(), current_network) {
        (Some(latched), Some(cur)) if latched.eq_ignore_ascii_case(cur) => true,
        _ => {
            *guard = None;
            false
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CycleError {
    #[error("COM на рабочем потоке не поднялся: {0}")]
    Com(proxypilot_winnet::WinNetError),
    #[error("не прочитать список подключённых сетей: {0}")]
    Networks(#[from] proxypilot_winnet::WinNetError),
    #[error("не прочитать profile.toml: {0}")]
    Profile(#[from] proxypilot_netsvc::profile::ProfileError),
    #[error("не собрать сопоставление NLM-адаптеров: {0}")]
    Adapter(#[from] proxypilot_netsvc::adapter::AdapterError),
}

/// Один проход: прочитать профиль и текущую сеть, принять решение
/// (`decide_profile`, уже полностью проверен задачей 5) и применить его.
///
/// Единственное место в крейте, которое реально пингует шлюз — см.
/// `gateway_reachable`/`gateway_reachable_with_retries` ниже. Исполнение
/// самого `netsh` теперь в `exec::run_netsh_batch` (общее с
/// `install::uninstall`, ревью round 2). Всё остальное в этой функции —
/// уже проверенные библиотечные вызовы.
fn run_cycle() -> Result<(), CycleError> {
    // SAFETY-обоснование живёт у `ComGuard::new()` самой — она сама
    // безопасна вызывать многократно (см. её докблок в `winnet::com`).
    let _com = ComGuard::new().map_err(CycleError::Com)?;

    let service_profile: ServiceProfile = profile::load_from(&profile::path())?;
    if service_profile.net_profile.office_ip.is_none() {
        // Профиль не настроен — decide_profile всё равно вернула бы
        // LeaveAlone на каждой ветке ниже, но читать сеть и NLM ради этого
        // незачем: самый частый случай (никто не настроил статику) не
        // должен стоить ни одного лишнего вызова.
        return Ok(());
    }

    let connected = list_connected()?;
    let cfg_for_place = Config {
        office_networks: service_profile.office_networks.clone(),
        ..Config::default()
    };
    let place = cfg_for_place.place_for(
        &connected
            .into_iter()
            .map(|n| ConnectedNetwork {
                id: n.id,
                name: n.name,
            })
            .collect::<Vec<_>>(),
    );

    // Латч против дребезга (докблок модуля, правило 3) — проверяется и
    // обслуживается на КАЖДОМ цикле, даже вне офиса: обслуживание здесь —
    // это именно снятие латча при смене сети, и оно обязано случиться, как
    // только мы замечены не на латченной сети, а не только когда мы снова
    // в офисе.
    let latched = check_and_maintain_latch(place.network.as_deref());
    if place.in_office && latched {
        return Ok(());
    }

    let applied = state::load_from(&state::path());

    // Адаптер берётся из NLM, только пока мы ещё в офисе — как только сеть
    // покинута, NLM больше не отдаёт то подключение вовсе (докблок
    // `state::AppliedState::iface_guid`). Вне офиса единственный источник —
    // то, что мы сами записали в прошлый раз.
    let iface_guid = if place.in_office {
        let adapters = gather_from_nlm()?;
        place
            .network
            .as_deref()
            .and_then(|net_id| find_office_adapter(&adapters, net_id))
            .map(str::to_owned)
    } else {
        applied.iface_guid.clone()
    };

    let Some(iface_guid) = iface_guid else {
        // Не в офисе, и раньше статику мы не ставили — управлять нечем.
        return Ok(());
    };

    let Some(current) = current_ipv4_config(&iface_guid)? else {
        // Адаптер, на который мы когда-то ставили статику, физически исчез
        // (докстанция отключена и т.п.) — сети применить некуда.
        return Ok(());
    };

    let adapter_config = to_adapter_config(&current, &applied);
    let action = decide_profile(
        place.in_office,
        &service_profile.net_profile,
        &adapter_config,
    );

    apply_action(&iface_guid, place.network.as_deref(), &action);
    Ok(())
}

fn to_adapter_config(current: &CurrentIpv4Config, applied: &AppliedState) -> AdapterConfig {
    AdapterConfig {
        dhcp: current.dhcp,
        addr: current.addr,
        set_by_us: state::set_by_us(applied, current.addr, current.mask),
        dns: current.dns.clone(),
        mask: current.mask,
    }
}

/// GUID адаптера → текущее дружественное имя, для самой команды `netsh`.
/// `None` (с записью в лог) означает «адаптер по этому GUID сейчас не
/// найден» или «не удалось прочитать список адаптеров» — оба случая
/// одинаково обрывают текущий цикл: применять статику или откатывать её
/// не на что.
fn resolve_alias(iface_guid: &str) -> Option<String> {
    match adapter::friendly_name_for_guid(iface_guid) {
        Ok(Some(alias)) => Some(alias),
        Ok(None) => {
            error!(iface_guid, "адаптер по GUID не найден — пропускаю цикл");
            None
        }
        Err(e) => {
            error!(error = %e, iface_guid, "не удалось определить псевдоним адаптера по GUID — пропускаю цикл");
            None
        }
    }
}

/// Исполняет решение `decide_profile` и обновляет собственную память
/// службы (`state::AppliedState`) так, чтобы она отражала то, что реально
/// подтверждено применённым — не то, что должно было применяться и не то,
/// о чём `netsh` только отчитался кодом возврата (докблок модуля, правила
/// 1 и 2).
fn apply_action(iface_guid: &str, network_id: Option<&str>, action: &ProfileAction) {
    match action {
        ProfileAction::LeaveAlone => {}
        ProfileAction::SetStatic {
            ip, mask, gateway, ..
        } => {
            let Some(alias) = resolve_alias(iface_guid) else {
                return;
            };

            if !exec::run_netsh_batch(commands_for_action(&alias, action)) {
                error!(
                    iface_guid, alias = %alias,
                    "применение статики не выполнилось целиком или частично — состояние не \
                     изменено, адаптер мог остаться в неопределённом виде; проверьте вручную"
                );
                return;
            }

            // Верификация (ревью round 2, Critical №3): не доверяем
            // только коду возврата `netsh` — перечитываем реальное
            // состояние адаптера и убеждаемся, что он действительно несёт
            // то, что мы просили, прежде чем записать владение.
            match current_ipv4_config(iface_guid) {
                Ok(Some(cfg)) if cfg.addr == Some(*ip) && cfg.mask == Some(*mask) => {}
                Ok(_) => {
                    error!(
                        iface_guid, alias = %alias,
                        "netsh отчитался об успехе, но адаптер не несёт запрошенный адрес — \
                         состояние не записано"
                    );
                    return;
                }
                Err(e) => {
                    error!(
                        error = %e, iface_guid,
                        "не перечитать состояние адаптера после применения статики — \
                         состояние не записано"
                    );
                    return;
                }
            }

            let new_state = AppliedState {
                ip: Some(*ip),
                mask: Some(*mask),
                iface_guid: Some(iface_guid.to_string()),
            };
            if let Err(e) = state::save_to(&state::path(), &new_state) {
                warn!(error = %e, "не удалось записать applied.toml — служба забудет, что статику поставила она");
            }

            // `gateway_reachable_with_retries` сама делает отстойник и
            // несколько попыток пинга внутри одного логического ответа —
            // `evaluate_gateway` берёт `FnOnce`, но повтор нужен ДО
            // единственного ответа, а не вместо него (докблок модуля).
            let (outcome, rollback) =
                evaluate_gateway(&alias, *gateway, gateway_reachable_with_retries, |msg| {
                    warn!("{msg}")
                });
            if let SafetyNetOutcome::RolledBack = outcome {
                let rollback_ran = exec::run_netsh_batch(rollback);
                let rollback_verified = rollback_ran
                    && matches!(current_ipv4_config(iface_guid), Ok(Some(cfg)) if cfg.dhcp);

                if rollback_verified {
                    if let Err(e) = state::save_to(&state::path(), &AppliedState::default()) {
                        warn!(error = %e, "не удалось записать applied.toml после отката в DHCP");
                    }
                } else {
                    // Ревью round 2, Critical №2: НЕ трогаем state —
                    // запись «это наша статика» остаётся как есть. Если бы
                    // мы сбросили её в default здесь не подтвердив откат,
                    // decide_profile на следующем цикле увидела бы адаптер
                    // как ЧУЖУЮ статику и по правилу
                    // foreign_static_address_is_never_reset (задача 5) не
                    // тронула бы её никогда — то есть машина застряла бы
                    // на офисном адресе с мёртвым шлюзом навсегда, а не «до
                    // следующей успешной попытки».
                    error!(
                        iface_guid, alias = %alias,
                        "откат в DHCP не подтверждён — статика могла остаться на адаптере; \
                         сделайте вручную: netsh interface ipv4 set address name=\"{alias}\" \
                         source=dhcp && netsh interface ipv4 set dnsservers name=\"{alias}\" \
                         source=dhcp. Служба продолжит пытаться сама на следующих циклах."
                    );
                }
                if let Some(net_id) = network_id {
                    set_latch(net_id);
                }
            }
        }
        ProfileAction::SetDhcp => {
            let Some(alias) = resolve_alias(iface_guid) else {
                return;
            };
            if exec::run_netsh_batch(commands_for_action(&alias, action)) {
                if let Err(e) = state::save_to(&state::path(), &AppliedState::default()) {
                    warn!(error = %e, "не удалось записать applied.toml после возврата в DHCP");
                }
            } else {
                error!(
                    iface_guid, alias = %alias,
                    "возврат в DHCP не выполнился — состояние не изменено, служба повторит \
                     попытку на следующем цикле"
                );
            }
        }
    }
}

/// Отстойник плюс несколько попыток ICMP-эха — единственная точка проверки
/// достижимости во всём крейте, вызываемая как `is_reachable` у
/// `safety::evaluate_gateway`. `safety::evaluate_gateway` тестируется
/// подменой этой функции замыканием (докблок `safety.rs`); сама она не
/// тестируется по той же причине, что и `exec::run_netsh` — контроллер
/// сессии запрещает слать настоящие пакеты шлюзу с машины разработки в
/// рамках этой задачи (шлюза, который вообще имел бы смысл пинговать,
/// здесь и нет).
fn gateway_reachable_with_retries(gateway: Ipv4Addr) -> bool {
    std::thread::sleep(GATEWAY_SETTLE_DELAY);
    for attempt in 0..GATEWAY_PING_ATTEMPTS {
        if gateway_reachable(gateway, GATEWAY_PING_TIMEOUT) {
            return true;
        }
        if attempt + 1 < GATEWAY_PING_ATTEMPTS {
            std::thread::sleep(GATEWAY_PING_RETRY_DELAY);
        }
    }
    false
}

/// Один ICMP-запрос-ответ шлюзу, с таймаутом.
fn gateway_reachable(gateway: Ipv4Addr, timeout: Duration) -> bool {
    // SAFETY: `IcmpCreateFile` не принимает аргументов; возвращает
    // валидный хендл или ошибку — обе ветки обработаны ниже.
    let handle = match unsafe { IcmpCreateFile() } {
        Ok(h) => h,
        Err(e) => {
            warn!(error = %e, "IcmpCreateFile не удался — шлюз считается недостижимым");
            return false;
        }
    };

    let dest = u32::from_ne_bytes(gateway.octets());
    let request: [u8; 0] = [];
    let mut reply_buf = vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + 8];

    // SAFETY: `handle` — валидный хендл, только что созданный выше;
    // `request` — пустой (но валидный) буфер нулевой длины; `reply_buf` —
    // живой буфер, размером заведомо больше `ICMP_ECHO_REPLY` (плюс запас
    // на данные ответа), что и требует документация `IcmpSendEcho`.
    let replies = unsafe {
        IcmpSendEcho(
            handle,
            dest,
            request.as_ptr().cast::<c_void>(),
            0,
            None,
            reply_buf.as_mut_ptr().cast::<c_void>(),
            reply_buf.len() as u32,
            timeout.as_millis() as u32,
        )
    };

    // SAFETY: `handle` больше не используется после этой точки — обе ветки
    // ниже возвращают управление сразу после закрытия.
    let _ = unsafe { IcmpCloseHandle(handle) };

    if replies == 0 {
        return false;
    }
    // SAFETY: `replies > 0` означает, что `IcmpSendEcho` записала в начало
    // `reply_buf` хотя бы одну `ICMP_ECHO_REPLY` — буфер выделен под её
    // размер (плюс запас) строкой выше.
    let reply = unsafe { &*reply_buf.as_ptr().cast::<ICMP_ECHO_REPLY>() };
    reply.Status == IP_SUCCESS
}
