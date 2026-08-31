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
//! - `profile`/`state` — откуда брать данные.
//!
//! Этот файл только связывает их с реальным вводом-выводом: чтением сети
//! через NLM, исполнением `netsh` и проверкой шлюза ICMP-пингом. Контроллер
//! сессии прямо запрещает как устанавливать эту службу, так и исполнять
//! `netsh interface ipv4 set address`/`set dnsservers` на машине разработки
//! (`CLAUDE.md`, «Живые проверки, которые не делает агент») — поэтому ни
//! разу не запускался и не мог быть запущен в этой сессии. Первый живой
//! прогон — дело человека, в офисной сети (см. отчёт задачи).

use std::ffi::c_void;
use std::net::Ipv4Addr;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use proxypilot_core::config::Config;
use proxypilot_core::netprofile::{decide_profile, AdapterConfig, ProfileAction};
use proxypilot_netsvc::adapter::{
    current_ipv4_config, find_office_adapter, gather_from_nlm, CurrentIpv4Config,
};
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
    SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};

/// Как часто пересчитывать решение просто по времени — та же подстраховка,
/// что и в приложении (`app::REEVALUATE_PERIOD`): подписка на NLM может не
/// подняться вовсе или пропустить смену, которая не двигает агрегат
/// связности (докблок `winnet::events`).
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Таймаут ICMP-эха при проверке шлюза (спека 7.3).
const GATEWAY_PING_TIMEOUT: Duration = Duration::from_secs(2);

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
/// будит главный цикл, крутящийся в `service_main` на другом потоке.
/// `Mutex`, а не голый `Sender`, ради `Sync`: `static` обязана быть
/// потокобезопасной, а `mpsc::Sender` сам по себе `Sync` не гарантирован.
static LOOP_TX: OnceLock<Mutex<Option<Sender<LoopEvent>>>> = OnceLock::new();

/// Единственная точка входа, которую вызывает `StartServiceCtrlDispatcherW`
/// на выделенном для этой службы потоке.
///
/// SAFETY: `unsafe extern "system"` здесь — требование самой сигнатуры
/// `LPSERVICE_MAIN_FUNCTIONW`, под которую SCM ищет функцию по указателю в
/// `SERVICE_TABLE_ENTRYW` (`main`, где эта запись строится). Тело не
/// разыменовывает `_argc`/`_argv` вовсе (аргументы командной строки службы
/// этому коду не нужны) и обязано не паниковать: паника через границу FFI
/// — неопределённое поведение, а `run_service` внутри и так возвращает
/// `Result`, а не паникует на ожидаемых отказах.
unsafe extern "system" fn ffi_service_main(_argc: u32, _argv: *mut PWSTR) {
    // Подписчик логов — до первой же строки лога: если он не поднимется,
    // молчание отсюда неотличимо от «служба ничего не делает».
    let log_dir = profile::program_data_dir().join("ProxyPilot").join("logs");
    let _log_guard = init_logging(&log_dir);

    if let Err(e) = run_service() {
        error!(error = %e, "служба завершилась с ошибкой");
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
    #[error("не удалось зарегистрировать обработчик управления службой: {0}")]
    RegisterHandler(WinError),
    #[error("не удалось сообщить SCM о статусе службы: {0}")]
    SetStatus(WinError),
}

/// Регистрирует обработчик, поднимает статус RUNNING и крутит главный цикл
/// до просьбы остановиться, затем сообщает STOPPED.
fn run_service() -> Result<(), ServiceError> {
    let (tx, rx) = channel::<LoopEvent>();
    *LOOP_TX
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("мьютекс канала не отравлен — паник в критической секции не бывает") =
        Some(tx.clone());

    let service_name = wide(SERVICE_NAME);
    // SAFETY: `service_name` жива до конца этого вызова; `ffi_control_handler`
    // — статическая функция, живущая весь процесс; контекст не нужен —
    // канал управления идёт через `LOOP_TX`, а не через контекст обработчика.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(
            PCWSTR::from_raw(service_name.as_ptr()),
            Some(ffi_control_handler),
            None,
        )
    }
    .map_err(ServiceError::RegisterHandler)?;

    report_status(handle, SERVICE_START_PENDING, 0)?;

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

    report_status(handle, SERVICE_RUNNING, 0)?;
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
    report_status(handle, SERVICE_STOPPED, 0)?;
    Ok(())
}

fn report_status(
    handle: SERVICE_STATUS_HANDLE,
    state: windows::Win32::System::Services::SERVICE_STATUS_CURRENT_STATE,
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
    // SAFETY: `handle` получен от `RegisterServiceCtrlHandlerExW` этим же
    // вызовом `run_service` и ещё живёт (мы на том же потоке, до его
    // возврата); `status` — живая локальная переменная на весь вызов.
    unsafe { SetServiceStatus(handle, &status) }.map_err(ServiceError::SetStatus)
}

/// Обработчик управления SCM — вызывается на СВОЁМ потоке, отдельном от
/// `run_service`. Единственное, что он делает — просит главный цикл
/// остановиться; сам статус STOPPED сообщает `run_service`, когда цикл
/// в самом деле завершится, а не этот обработчик заранее.
///
/// SAFETY: `unsafe extern "system"` — требование сигнатуры
/// `LPHANDLER_FUNCTION_EX`, под которую SCM вызывает обработчик по
/// указателю, переданному в `RegisterServiceCtrlHandlerExW`. `_event_data`
/// и `_context` — сырые указатели, но не разыменовываются здесь ни разу
/// (управление не использует ни расширенные данные события, ни контекст —
/// канал общения с главным циклом идёт через `LOOP_TX`, не через них).
/// Тело обязано не паниковать и не блокироваться надолго по той же причине,
/// что и `ffi_service_main`.
unsafe extern "system" fn ffi_control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
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
/// Единственное место в крейте, которое реально исполняет `netsh` и реально
/// пингует шлюз — см. `run_netsh`/`gateway_reachable` ниже. Всё остальное в
/// этой функции — уже проверенные библиотечные вызовы.
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
            .map(|n| proxypilot_core::mode::ConnectedNetwork {
                id: n.id,
                name: n.name,
            })
            .collect::<Vec<_>>(),
    );

    let applied = state::load_from(&state::path());

    // Адаптер берётся из NLM, только пока мы ещё в офисе — как только сеть
    // покинута, NLM больше не отдаёт то подключение вовсе (докблок
    // `state::AppliedState::iface`). Вне офиса единственный источник — то,
    // что мы сами записали в прошлый раз.
    let iface = if place.in_office {
        let adapters = gather_from_nlm()?;
        place
            .network
            .as_deref()
            .and_then(|net_id| find_office_adapter(&adapters, net_id))
            .map(str::to_owned)
    } else {
        applied.iface.clone()
    };

    let Some(iface) = iface else {
        // Не в офисе, и раньше статику мы не ставили — управлять нечем.
        return Ok(());
    };

    let Some(current) = current_ipv4_config(&iface)? else {
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

    apply_action(&iface, &action);
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

/// Исполняет решение `decide_profile` и обновляет собственную память
/// службы (`state::AppliedState`) так, чтобы она отражала то, что реально
/// применено — не то, что должно было применяться.
fn apply_action(iface: &str, action: &ProfileAction) {
    match action {
        ProfileAction::LeaveAlone => {}
        ProfileAction::SetStatic {
            ip, mask, gateway, ..
        } => {
            for mut cmd in commands_for_action(iface, action) {
                if let Err(e) = run_netsh(&mut cmd) {
                    error!(error = %e, iface, "команда netsh не выполнилась — статика могла примениться частично");
                }
            }
            let new_state = AppliedState {
                ip: Some(*ip),
                mask: Some(*mask),
                iface: Some(iface.to_string()),
            };
            if let Err(e) = state::save_to(&state::path(), &new_state) {
                warn!(error = %e, "не удалось записать applied.toml — служба забудет, что статику поставила она");
            }

            let (outcome, rollback) = evaluate_gateway(
                iface,
                *gateway,
                |gw| gateway_reachable(gw, GATEWAY_PING_TIMEOUT),
                |msg| warn!("{msg}"),
            );
            if let SafetyNetOutcome::RolledBack = outcome {
                for mut cmd in rollback {
                    if let Err(e) = run_netsh(&mut cmd) {
                        error!(error = %e, iface, "команда отката в DHCP не выполнилась");
                    }
                }
                if let Err(e) = state::save_to(&state::path(), &AppliedState::default()) {
                    warn!(error = %e, "не удалось записать applied.toml после отката в DHCP");
                }
            }
        }
        ProfileAction::SetDhcp => {
            for mut cmd in commands_for_action(iface, action) {
                if let Err(e) = run_netsh(&mut cmd) {
                    error!(error = %e, iface, "команда возврата в DHCP не выполнилась");
                }
            }
            if let Err(e) = state::save_to(&state::path(), &AppliedState::default()) {
                warn!(error = %e, "не удалось записать applied.toml после возврата в DHCP");
            }
        }
    }
}

/// Единственная точка исполнения `netsh` во всём крейте. Контроллер сессии
/// прямо запрещает вызывать её на машине разработки (`CLAUDE.md`) — ни один
/// тест до неё не доходит: `netsh_cmd` проверяет только построение команд,
/// не их запуск (докблок `netsh_cmd.rs`).
fn run_netsh(cmd: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    info!(?cmd, "выполняю netsh");
    cmd.status()
}

/// ICMP-эхо шлюзу — единственная точка проверки достижимости во всём
/// крейте. `safety::evaluate_gateway` тестируется подменой этой функции
/// замыканием (докблок `safety.rs`); сама она не тестируется по той же
/// причине, что и `run_netsh` выше.
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
