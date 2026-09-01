// В релизе — без консольного окна: запуск из проводника не должен пугать
// пользователя чёрным окном терминала. В отладочной сборке консоль остаётся:
// это единственный простой способ увидеть `tracing`-вывод и `eprintln!` из
// `main` живьём, да и `SetConsoleCtrlHandler` ниже без неё бессмыслен.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! ProxyPilot — приложение в трее.
//!
//! Устройство процесса продиктовано `tray-icon`: иконка требует цикла
//! сообщений на том потоке, где создана, а на Windows это практически
//! главный поток. Мост живёт в отдельном tokio-рантайме.
//!
//! ```text
//! главный поток              tokio runtime
//! ─────────────              ─────────────
//! цикл сообщений      ←──→   serve()      (мост)
//! трей + меню                supervisor   (пересчёт маршрута)
//!       │                          │
//!       └───── Router (ArcSwap) ───┘
//! ```
//!
//! Мост не потребовал ни одной правки: маршрут уже живёт в атомарной ячейке,
//! поэтому трей меняет его из главного потока, мост видит новое значение на
//! следующем соединении, а живые туннели не замечают ничего.
//!
//! ПОРЯДОК СТАРТА — не косметика. Мост НЕ ДОЛЖЕН обслуживать трафик на том
//! маршруте, с которым `Router` был сконструирован: `Route::Direct` в поле
//! конструктора — это не решение, а заглушка, и отправить по ней хоть одно
//! соединение значит соврать про режим. Поэтому первый `reevaluate`
//! выполняется ДО того, как слушатель создан (`TcpListener::bind` идёт
//! строкой ниже), — окна, в котором можно было бы принять соединение на
//! незаполненном маршруте, не существует вовсе. Второй порядок здесь же:
//! системный прокси направляется на нас только ПОСЛЕ того, как слушатель
//! принят системой, иначе трафик пошёл бы туда, где ещё никто не слушает.
//!
//! ЗАВЕРШЕНИЕ СЕАНСА в релизе (без консоли) ловится не `SetConsoleCtrlHandler`
//! — эта функция без консоли бессмысленна, — а `WM_QUERYENDSESSION`/
//! `WM_ENDSESSION`, которые Windows шлёт оконными сообщениями. Единственное
//! окно главного потока — то, что создаёт сам `tray-icon` для своей иконки;
//! `tray::install_session_end_guard` подменяет его оконную процедуру, чтобы
//! перехватить эти два сообщения и не более того (см. модульный комментарий
//! `tray.rs`). В отладочной сборке остаётся и `SetConsoleCtrlHandler` — там
//! есть консоль, а Ctrl+C с неё для разработчика привычнее, чем закрытие
//! окна через диспетчер задач.

mod doctor;
mod icons;
mod proxy;
mod settings_page;
mod tray;
mod ui;
mod update;
mod websrv;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use proxypilot_bridge::log;
use proxypilot_bridge::probe::Prober;
use proxypilot_bridge::router::Router;
use proxypilot_bridge::serve::{serve, Limits, Shared};
use proxypilot_bridge::supervisor::{AppState, NetworkSource, Supervisor, SupervisorError};
use proxypilot_core::bypass::BypassList;
use proxypilot_core::config::Config;
use proxypilot_core::mode::{ConnectedNetwork, Mode, Route};
use proxypilot_core::net::Ipv4Net;
use proxypilot_winnet::autostart;
use proxypilot_winnet::com::ComGuard;
use proxypilot_winnet::events::{debounce, watch_network_changes};
use proxypilot_winnet::networks::list_connected;
use proxypilot_winnet::openvpn::{self, ProfileStatus};
use proxypilot_winnet::{routes as ip_routes, tunnel_log, tunnel_state};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};
use tray_icon::menu::MenuEvent;
#[cfg(debug_assertions)]
use windows::Win32::Foundation::BOOL;
use windows::Win32::Foundation::{LPARAM, WPARAM};
#[cfg(debug_assertions)]
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_APP,
};

use crate::tray::{Action, Tray};

/// «Состояние пересчитано, перерисуй меню». `WM_APP` — диапазон, который
/// система обещает не занимать своими сообщениями; `+1` уже занят потоком
/// подписки на смену сети (`winnet::events`), и хотя это другой поток,
/// одинаковые номера на соседних циклах сообщений — приглашение к ошибке.
const WM_STATE_CHANGED: u32 = WM_APP + 2;

/// «Мост больше не принимает соединения». Сообщение служит ровно одному —
/// разбудить цикл сообщений; решение принимается по флагу `BRIDGE_STOPPED`,
/// см. его комментарий.
const WM_BRIDGE_STOPPED: u32 = WM_APP + 3;

/// Мост перестал принимать соединения.
///
/// Флаг, а не только сообщение, и именно он авторитетен. `tray-icon`
/// показывает меню, вызывая `TrackPopupMenu` прямо из оконной процедуры, то
/// есть ВНУТРИ нашего `DispatchMessageW`: пока меню открыто, крутится
/// вложенный цикл сообщений. Потоковые сообщения (те, у которых `hwnd == 0`,
/// а `PostThreadMessageW` шлёт именно такие) этот вложенный цикл извлекает и
/// выбрасывает — диспетчеризовать их некуда. Смерть моста ровно в тот
/// момент, когда пользователь держит меню открытым, потеряла бы уведомление
/// навсегда, и приложение осталось бы работать с закрытым портом, реестром,
/// указывающим на него, и бодрой иконкой — тем самым состоянием, ради
/// устранения которого всё это и заведено.
///
/// Поэтому: задача-сторож выставляет флаг и посылает сообщение; цикл
/// перечитывает флаг на каждом витке — в том числе сразу после возврата из
/// `DispatchMessageW`, то есть после закрытия всплывающего меню.
static BRIDGE_STOPPED: AtomicBool = AtomicBool::new(false);

/// Окно схлопывания событий смены сети. Одно переключение Wi-Fi даёт их
/// пачку; пересчитывать маршрут на каждое — жечь пробы впустую.
const NETWORK_DEBOUNCE: Duration = Duration::from_millis(1500);

/// Сколько живёт результат пробы живости апстримов.
const PROBE_TTL: Duration = Duration::from_secs(30);

/// Как часто пересчитывать решение просто по времени.
///
/// НАМЕРЕННО больше `PROBE_TTL`: на более частом тике `Prober` отдавал бы
/// кэш, то есть половина пересчётов не проверяла бы ничего и лишь жгла
/// вызовы NLM. При 60 с каждый тик — свежая проба обоих апстримов.
const REEVALUATE_PERIOD: Duration = Duration::from_secs(60);

/// Как часто фоновая проверка спрашивает GitHub про новый релиз. Не на
/// каждом старте и не каждую минуту: анонимные запросы к API ограничены по
/// числу в час, а релизы происходят не каждый день — раз в сутки более чем
/// достаточно для «неблокирующего фонового уведомления», которым эта
/// проверка и является (задача 3).
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Задержка первой проверки после старта. Не сразу: старт и так занят более
/// срочными вещами (первый пересчёт маршрута, привязка слушателя,
/// самопроверка), а сеть на проверку обновлений может быть недоступна ровно
/// в момент старта — приёмка задачи 3 требует, чтобы это не задерживало и
/// не портило впечатление о старте вовсе.
const UPDATE_CHECK_INITIAL_DELAY: Duration = Duration::from_secs(5 * 60);

/// Что делать супервизору. Смена сети, смена режима и правка настроек
/// сходятся в один канал: все три означают «пересчитай», и обрабатывать их
/// порознь значило бы завести несколько мест, где пересчёт может разойтись.
///
/// Страница настроек ходит сюда же и никуда больше: второй писатель в
/// `Router` в обход этого канала был бы молча затёрт следующим пересчётом.
pub enum Cmd {
    Reevaluate,
    SetMode(Mode),
    /// Новый конфиг целиком, как его собрала и проверила страница настроек.
    ///
    /// `Box`, потому что `Config` заметно крупнее остальных вариантов, а
    /// размер варианта определяет размер каждого сообщения в очереди.
    ///
    /// `done` — не украшение: страница обязана показать человеку, что
    /// именно случилось (в том числе отказ записи на диск), а не бодрое
    /// «сохранено» вслепую. Отправитель ждёт этот ответ со своим сроком.
    ApplyConfig {
        config: Box<Config>,
        done: oneshot::Sender<Result<(), String>>,
    },
}

/// Почему закончился цикл сообщений. Разные причины — разный код возврата, но
/// путь выхода один и тот же: возврат из `run` через страж восстановления.
enum Exit {
    /// «Выход» в меню.
    User,
    /// `WM_QUIT` — например, завершение сеанса Windows.
    SessionEnd,
    /// Мост перестал принимать соединения.
    BridgeStopped,
    /// Сама `GetMessageW` отказала.
    MessageLoopFailed(String),
}

fn main() {
    // `install-service`/`uninstall-service` — единственный запрос прав
    // администратора во всём продукте (`CLAUDE.md`, «Права
    // администратора»): регистрация службы `ProxyPilotNetProfile`,
    // которая одна умеет менять IPv4-адрес. Явные, однократные,
    // необязательные — кто их не вызвал, не увидит ни одного UAC.
    // Разбираются раньше `run()`: это не запуск трея, а обычная короткая
    // команда, которая обязана вернуть управление сразу же.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("install-service") => std::process::exit(run_install_service()),
        Some("uninstall-service") => std::process::exit(run_uninstall_service()),
        _ => {}
    }

    if let Err(e) = run() {
        report_failure(&e);
        std::process::exit(1);
    }
}

/// Путь к бинарнику самой службы — отдельный исполняемый файл
/// (`proxypilot-netsvc.exe`), а не этот процесс: ядро (этот `.exe`)
/// обязано остаться без единого запроса UAC, поэтому статика живёт в
/// отдельной программе (`crates/netsvc`). Ставится рядом, в том же
/// каталоге, куда установлен `proxypilot.exe` — оба поставляются вместе
/// одним инсталлятором (`docs/design.md` §12).
fn netsvc_exe_path() -> Result<std::path::PathBuf, String> {
    let self_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = self_exe
        .parent()
        .ok_or("у пути к proxypilot.exe нет каталога")?;
    Ok(dir.join("proxypilot-netsvc.exe"))
}

/// Возвращает код возврата процесса — 0 при успехе, 1 при отказе, с текстом
/// отказа в stderr. Не паникует и не показывает `ui::error_box`: это
/// команда для консоли (человек, поставивший её вручную, с правами
/// администратора), а не для двойного щелчка по иконке.
fn run_install_service() -> i32 {
    match netsvc_exe_path()
        .and_then(|p| proxypilot_netsvc::install::install(&p).map_err(|e| e.to_string()))
    {
        Ok(()) => {
            println!(
                "Служба {} зарегистрирована (автозапуск). Она ещё не запущена — \
                 первый пуск сделает Windows при следующей перезагрузке, или \
                 запустите её вручную: services.msc / Start-Service {}.",
                proxypilot_netsvc::SERVICE_NAME,
                proxypilot_netsvc::SERVICE_NAME
            );
            0
        }
        Err(e) => {
            eprintln!(
                "Не удалось установить службу {}: {e}",
                proxypilot_netsvc::SERVICE_NAME
            );
            1
        }
    }
}

fn run_uninstall_service() -> i32 {
    match proxypilot_netsvc::install::uninstall() {
        Ok(()) => {
            println!(
                "Служба {} снята с регистрации.",
                proxypilot_netsvc::SERVICE_NAME
            );
            0
        }
        Err(e) => {
            eprintln!(
                "Не удалось удалить службу {}: {e}",
                proxypilot_netsvc::SERVICE_NAME
            );
            1
        }
    }
}

/// Единственный способ, которым человек узнает об отказе.
///
/// В релизе консоли нет (`windows_subsystem = "windows"`), и `eprintln!`
/// читать некому. Самый вероятный отказ — тот, который получает всякий, кто
/// дважды щёлкнул по иконке уже запущенного приложения: `bind` на занятый
/// порт. Без окна это выглядит как «ничего не произошло» — ни иконки, ни
/// сообщения. Руками испорченный `config.toml` ведёт себя так же.
///
/// В лог отказ уже записан — внутри `run`, где ещё жив страж лога (см. там
/// же, почему не здесь). Эта функция отвечает только за человека.
///
/// Порядок важен: `run` уже вернулась, то есть страж восстановления
/// системного прокси отработал, — окно не задерживает приведение реестра в
/// порядок, а показывается после него.
fn report_failure(e: &str) {
    // `cfg!`, а не `#[cfg]`: обе ветки обязаны компилироваться в обеих
    // сборках. При `#[cfg]` в отладочной сборке `ui::error_box` осталась бы
    // никем не вызванной, то есть мёртвым кодом, и глушить предупреждение
    // атрибутом пришлось бы ради ничего.
    if cfg!(debug_assertions) {
        eprintln!("proxypilot: {e}");
    } else {
        ui::error_box(
            "ProxyPilot",
            &format!(
                "Не удалось запустить.

{e}"
            ),
        );
    }
}

/// Поднимает лог и передаёт работу дальше, а на отказе — записывает его,
/// пока лог ещё жив.
///
/// Разделение на две функции существует ровно ради этой записи. Страж
/// `tracing-appender` останавливает пишущий поток в своём `Drop`, то есть
/// на выходе отсюда; строка, напечатанная после возврата (как это делал
/// `main`), уходит в остановленный писатель и пропадает молча — проверено,
/// в файле её не было. А без неё в релизной сборке отказ старта не
/// оставлял бы вообще никакого следа: ни консоли, ни лога.
fn run() -> Result<(), String> {
    let config_path = Config::path().ok_or("не нашёл каталог настроек пользователя")?;
    let dir = config_path
        .parent()
        .ok_or("у пути конфига нет каталога")?
        .to_path_buf();
    let log_dir = dir.join("logs");
    std::fs::create_dir_all(&log_dir)
        .map_err(|e| format!("не создать {}: {e}", log_dir.display()))?;

    // Ровно один вызов на процесс и как можно раньше: всё, что случится до
    // него, в файл не попадёт. Страж обязан дожить до конца `run`.
    let _log_guard = log::init(Some(&log_dir));

    // Обновление, отложенное фоновой проверкой ПРОШЛОГО запуска
    // (`update::check`), ставится на место здесь — ДО `Config::load`, ДО
    // COM, ДО привязки слушателя моста. Это и есть «замена при следующем
    // запуске, а не под работающим процессом» (`CLAUDE.md`,
    // `docs/process/win-delivery/task-3-brief.md`): этот процесс ещё
    // ничего не взял на себя — ни порт, ни системный прокси, — и если
    // своп произошёл, он тут же перезапускает себя же и завершается, так
    // и не тронув реестр. Подробности — докблок `update::install`.
    if let Ok(exe) = std::env::current_exe() {
        let update_dir = dir.join("update");
        match update::install::apply_pending_update(
            &update_dir,
            &exe,
            update::install::real_verifier,
        ) {
            Ok(true) => {
                info!("отложенное обновление применено — перезапускаемся");
                match update::install::relaunch(&exe) {
                    Ok(()) => return Ok(()),
                    Err(e) => error!(
                        error = %e,
                        "не удалось перезапуститься после обновления — продолжаем со старой копией в памяти"
                    ),
                }
            }
            Ok(false) => {}
            Err(e) => warn!(error = %e, "отложенное обновление не применилось"),
        }
    } else {
        warn!("не узнать свой путь — проверка отложенного обновления пропущена");
    }

    let outcome = run_logged(&config_path);
    if let Err(e) = &outcome {
        error!(error = %e, "запуск не удался");
    }
    outcome
}

fn run_logged(config_path: &std::path::Path) -> Result<(), String> {
    let mut cfg = Config::load().map_err(|e| e.to_string())?;
    info!(
        port = cfg.bridge_port,
        mode = ?cfg.mode,
        manage_system_proxy = cfg.manage_system_proxy,
        config = %config_path.display(),
        "proxypilot запускается"
    );

    // Апартамент главного потока. `tray-icon` COM сам не поднимает, но
    // область уведомлений — часть оболочки, и любой вызов оболочки с этого
    // потока требует апартамента. Страж знает про `RPC_E_CHANGED_MODE` и не
    // снимет чужой апартамент, если поток уже в нём.
    let _com = ComGuard::new().map_err(|e| format!("COM на главном потоке: {e}"))?;

    let runtime = tokio::runtime::Runtime::new().map_err(|e| format!("tokio: {e}"))?;

    let port = cfg.bridge_port;
    let router = Arc::new(Router::new(Route::Direct));
    let mut supervisor = new_supervisor(&router, &cfg);

    // ПЕРВЫЙ пересчёт — до создания слушателя. См. модульный комментарий:
    // маршрут из конструктора `Router` обслуживать нельзя.
    let initial = runtime.block_on(supervisor.reevaluate());

    let shared = Arc::new(Shared {
        router: Arc::clone(&router),
        bypass: Arc::new(BypassList::parse(&cfg.no_proxy)),
        limits: Limits {
            dial: Duration::from_millis(cfg.dial_timeout_ms),
            head: Duration::from_millis(cfg.head_timeout_ms),
            max_connections: cfg.max_connections,
        },
    });

    // Строго loopback: на 0.0.0.0 это был бы открытый прокси для всей
    // локальной сети. Слушатель создаётся один раз за жизнь процесса и не
    // перепривязывается — смена порта требует перезапуска (инвариант
    // `supervisor.rs`).
    let addr = tray::bridge_address(port);
    let listener = runtime
        .block_on(TcpListener::bind(&addr))
        .map_err(|e| format!("не занять {addr}: {e}; возможно, proxypilot уже запущен"))?;
    let bridge = runtime.spawn(serve(listener, shared));

    // Трей создаётся здесь, а не рядом с фоновыми задачами ниже, и это
    // раньше `take_over`, а не после: `install_session_end_guard` подменяет
    // оконную процедуру окна трея, чтобы поймать `WM_QUERYENDSESSION`/
    // `WM_ENDSESSION` (см. модульный комментарий `tray.rs`), а сделать это
    // можно только когда окно уже существует — раньше просто нечего
    // подменять. Тот же аргумент, что и у стража с обработчиком консоли
    // ниже: страж завершения сеанса обязан встать ДО того, как реестр
    // укажет на нас, иначе останется зазор, в котором конец сеанса некому
    // будет перехватить.
    // Единственный экземпляр на весь процесс — тот же самый нужен и трею
    // (снимок в заголовке меню), и каждому открытию страницы настроек
    // (`SettingsState.tunnel`): без состояния, поэтому клонировать `Arc`,
    // а не заводить второй, безопасно и дёшево.
    let tunnel: Arc<dyn settings_page::Tunnel> = Arc::new(WinTunnel);
    let initial_tunnel_snapshot =
        tunnel.snapshot(&cfg.office_subnets, settings_page::TUNNEL_PROFILE_NAME);
    let tray = Tray::new(&initial, &initial_tunnel_snapshot).map_err(|e| format!("трей: {e}"))?;
    tray.install_session_end_guard();

    // Страж и обработчик консоли ставятся ДО `take_over`, а не после.
    //
    // `sysproxy::apply` пишет три значения и лишь потом уведомляет систему;
    // отказ уведомления — как и отказ политики на последней записи — приходит
    // наружу как `Err`, когда реестр УЖЕ изменён (это прямо оговорено в
    // `sysproxy`, и ради этого случая `take_over` заполняет `ORIGINAL` до
    // записи). Если бы страж создавался после `take_over`, то `?` уносил бы
    // нас из `run` мимо него — и машина оставалась бы с указателем на нас,
    // а восстанавливать было бы нечем. Ранний страж безвреден: пока
    // `ORIGINAL` пуст, `restore()` ничего не делает.
    let _restore = cfg.manage_system_proxy.then(|| {
        install_console_handler();
        proxy::RestoreOnDrop
    });

    // Системный прокси — только теперь, когда слушатель уже принят системой.
    //
    // Обе ветки возвращают то, что реестр говорил ДО этого вызова — тот же
    // факт, который диагностика ниже проверяет на «мёртвый указатель». Второе
    // чтение ПОСЛЕ этого блока показало бы уже наш собственный адрес (когда
    // управление включено, `take_over` его только что записал) и замаскировало
    // бы ровно то, что диагностика обязана заметить, — поэтому берём именно
    // то значение, что эти функции уже прочитали, а не читаем реестр заново.
    let sysproxy_before: Result<proxypilot_winnet::sysproxy::SysProxy, String> =
        if cfg.manage_system_proxy {
            Ok(proxy::take_over(&mut cfg, port).map_err(|e| e.to_string())?)
        } else {
            info!("manage_system_proxy = false: системные настройки не трогаем");
            // Не писать — правильно, но молчать нельзя. Выключатель могли
            // передвинуть уже ПОСЛЕ того, как нас однажды убили с включённым
            // управлением: тогда в реестре остался наш мёртвый адрес, трей
            // показывает исправный мост, а всё, что ходит через WinINET, лежит.
            // Обе проверки без побочных эффектов, так что это ничего не стоит.
            warn_if_stale_pointer_left_behind(port)
        };

    // Самопроверка при каждом старте — срез момента запуска, и только его.
    // Кнопка «Диагностика» на странице настроек читает систему заново по
    // нажатию и видит по-настоящему ЖИВОЕ состояние; здешние строки ложатся
    // в лог, а именно его в первую очередь попросит прислать поддержка — и
    // человек, у которого «не работает», до страницы может и не дойти.
    //
    // ДВА РАЗНЫХ факта ниже — не один и тот же под двумя именами. Раньше
    // здесь стоял один общий булев параметр, который путался между «мост
    // слушает СЕЙЧАС» (для этой строки правда — `true`: `bind` и `serve`
    // уже отработали, сокет действительно принимает соединения) и «был ли
    // порт свободен ДО нашего `bind`» (тоже `true`, но по другой причине —
    // сам факт, что мы досюда дошли, доказывает это: живой мост на этом
    // порту означал бы отказ `bind` кодом «адрес занят» и ранний выход
    // через `?` выше). Слияние этих двух фактов в один заставляло «мост
    // слушает свой порт» — самую частую жалобу таблицы брифа — кричать
    // `Fail` на КАЖДОМ здоровом старте. `sysproxy_before` — третий факт,
    // тоже «как было до починки»: см. комментарий выше, где он собирается.
    doctor::log_diagnostics(&doctor::run_checks(
        &cfg,
        &initial,
        /* bridge_listening_now */ true,
        /* port_was_free_before_bind */ true,
        &sysproxy_before,
    ));

    let state = Arc::new(ArcSwap::from_pointee(initial));

    // Конфиг, каким его задал человек, — для чтения страницей настроек.
    // Писать сюда имеет право только задача, обслуживающая канал `Cmd`
    // (ниже): второй писатель означал бы, что страница может показать
    // значение, до супервизора не доехавшее.
    //
    // Именно «как задал человек», а не «как лежит на диске»: при отказе
    // записи ячейка всё равно обновляется — правка применена и живёт до
    // перезапуска, — а про сам отказ страница говорит отдельной строкой.
    let saved_config = Arc::new(ArcSwap::from_pointee(cfg.clone()));

    // SAFETY: `GetCurrentThreadId` не принимает аргументов, не может отказать
    // и не трогает память. Идентификатор главного потока живёт столько же,
    // сколько процесс, поэтому переиспользования номера здесь не бывает.
    let main_thread = unsafe { GetCurrentThreadId() };

    // Слежение за мостом ставится после трея: именно трей создаёт очередь
    // сообщений главного потока, а без неё просьба выйти пропала бы молча.
    spawn_bridge_watch(&runtime, bridge, main_thread);

    let (commands, mut inbox) = mpsc::channel::<Cmd>(16);
    spawn_network_watch(&runtime, commands.clone());
    spawn_periodic_reevaluate(&runtime, commands.clone());

    // Итог последней фоновой проверки обновлений — читает страница настроек
    // (`update_status_note`), пишет только задача, запущенная ниже.
    // `None` — «ещё не проверялось», отдельно от «проверили и отказало»
    // (см. докблок `settings_page::SettingsState::update_status`).
    let update_status: Arc<ArcSwap<Option<update::check::CheckOutcome>>> =
        Arc::new(ArcSwap::from_pointee(None));
    if let Some(update_dir) = config_path.parent().map(|d| d.join("update")) {
        spawn_update_check(
            &runtime,
            Arc::clone(&saved_config),
            update_dir,
            Arc::clone(&update_status),
        );
    } else {
        warn!("нет каталога конфигурации — фоновая проверка обновлений не запущена");
    }

    {
        let state = Arc::clone(&state);
        let saved_config = Arc::clone(&saved_config);
        let router = Arc::clone(&router);
        // ДВА конфига, а не один, и разница между ними — это и есть правило
        // «смена порта не применяется на лету». `saved` уходит на диск таким,
        // каким его задал человек; `live` получает супервизор, и в нём стоит
        // порт, на котором слушатель УЖЕ привязан. Совместить их в одну
        // переменную значило бы либо перепривязывать слушатель (чего продукт
        // не делает — см. инвариант `supervisor.rs`), либо записывать на диск
        // не то, что человек ввёл.
        //
        // Клон обязан сниматься ПОСЛЕ `take_over`: именно там в конфиг
        // попадает `saved_sysproxy`. Снятый раньше клон не содержал бы его,
        // и первая же запись (`saved.save()` ниже) стёрла бы с диска
        // единственный след исходных настроек пользователя — вместе с
        // возможностью восстановиться после того, как нас убьют.
        let mut saved = cfg.clone();
        runtime.spawn(async move {
            while let Some(cmd) = inbox.recv().await {
                // Ответ странице отправляется в самом конце витка, а не
                // сразу: страница по нему перерисовывается и обязана увидеть
                // уже пересчитанное состояние, а не то, что было до правки.
                let mut reply: Option<oneshot::Sender<Result<(), String>>> = None;
                let mut outcome: Result<(), String> = Ok(());
                let change = match cmd {
                    Cmd::Reevaluate => None,
                    Cmd::SetMode(mode) => Some(Change::Mode(mode)),
                    Cmd::ApplyConfig { config, done } => {
                        reply = Some(done);
                        Some(Change::Whole(config))
                    }
                };
                if let Some(change) = change {
                    // Единственное место, где решается, ЧТО получит
                    // супервизор, — и единственное, где действует правило
                    // порта. Почему отдельной функцией — см. `apply_change`.
                    let live = apply_change(&mut saved, change, port);
                    // Предпочтение пользователя переживает перезапуск — это
                    // всё, ради чего конфиг здесь пишется на диск.
                    outcome = saved.save().map_err(|e| e.to_string());
                    if let Err(e) = &outcome {
                        warn!(error = %e, mode = ?saved.mode, "настройки не сохранены в конфиг");
                    }
                    // Супервизор владеет конфигом целиком; пересобрать его
                    // дешевле, чем заводить в мосте изменяемое состояние.
                    // Заодно сбрасывается кэш проб — после ручной правки
                    // человек ждёт свежий ответ, а не тридцатисекундной
                    // давности.
                    supervisor = new_supervisor(&router, &live);
                    saved_config.store(Arc::new(saved.clone()));
                }
                state.store(Arc::new(supervisor.reevaluate().await));
                post_to_main(main_thread, WM_STATE_CHANGED);
                if let Some(done) = reply {
                    // Приёмника может уже не быть: браузер закрыл соединение,
                    // не дождавшись. Это не повод шуметь.
                    let _ = done.send(outcome);
                }
            }
        });
    }

    // Собран здесь же, а не внутри `message_loop`: восьмым параметром
    // (`update_status`, задача 3) собственный список аргументов функции
    // перевалил бы за предел `clippy::too_many_arguments` (7) второй раз —
    // тем же приёмом, каким уже собран `SettingsDeps` для `open_settings`.
    let settings_deps = SettingsDeps {
        runtime: &runtime,
        state: &state,
        saved_config: &saved_config,
        commands: &commands,
        bound_port: port,
        tunnel: &tunnel,
        update_status: &update_status,
    };
    match message_loop(&tray, &settings_deps) {
        Exit::User => {
            info!("выход по команде пользователя");
            Ok(())
        }
        Exit::SessionEnd => {
            info!("получен WM_QUIT, завершаемся");
            Ok(())
        }
        // Порт закрыт — держать системный прокси направленным на него нельзя.
        // Возврат ошибки уводит нас из `run` через `_restore`, так что реестр
        // приводится в порядок раньше, чем процесс уйдёт с кодом 1.
        Exit::BridgeStopped => Err(concat!(
            "мост перестал принимать соединения; выходим, ",
            "чтобы не оставлять системный прокси направленным в пустоту"
        )
        .to_string()),
        Exit::MessageLoopFailed(e) => Err(e),
    }
}

/// Проверяет, не остался ли в реестре наш адрес от убитого запуска.
/// Ничего не пишет в реестр — только читает и, если нашла, кричит.
///
/// Возвращает то же прочитанное значение вызывающему: это единственное
/// чтение реестра в этой ветке старта (`manage_system_proxy = false`), и
/// диагностике ниже нужно ровно оно, а не второе, независимое чтение.
fn warn_if_stale_pointer_left_behind(
    port: u16,
) -> Result<proxypilot_winnet::sysproxy::SysProxy, String> {
    let current = proxypilot_winnet::sysproxy::read().map_err(|e| {
        warn!(error = %e, "не прочитать системные настройки прокси");
        e.to_string()
    })?;
    if proxy::is_stale_pointer(&current, port) {
        error!(
            server = %current.server,
            concat!(
                "в системных настройках остался наш адрес от прошлого запуска, ",
                "но управление системным прокси выключено — уберите значение ",
                "ProxyServer вручную, иначе приложения, читающие настройки ",
                "Windows, останутся без сети"
            )
        );
    }
    Ok(current)
}

/// Следит за мостом и просит главный поток выйти, если тот перестал
/// принимать соединения.
///
/// `serve` сдаётся сам после длинной серии отказов `accept` — и тогда
/// слушатель разрушается, порт закрывается, а системный прокси продолжает
/// указывать на него. Приложение, которое в этом состоянии продолжает
/// показывать исправную иконку, врёт хуже, чем отсутствие иконки. Паника в
/// цикле приёма даёт ровно то же состояние, поэтому ждём именно `JoinHandle`,
/// а не результат `serve`: он ловит и её.
fn spawn_bridge_watch(
    runtime: &tokio::runtime::Runtime,
    bridge: tokio::task::JoinHandle<std::io::Result<()>>,
    main_thread: u32,
) {
    runtime.spawn(async move {
        match bridge.await {
            // `serve` крутится вечно; сюда он попасть не должен вовсе.
            Ok(Ok(())) => error!("мост завершился сам, хотя не должен был"),
            Ok(Err(e)) => error!(error = %e, "мост остановился"),
            Err(e) => error!(error = %e, "мост упал"),
        }
        // Сначала флаг, потом побудка: цикл, разбуженный сообщением, обязан
        // увидеть уже выставленный флаг, а не гонку с ним.
        BRIDGE_STOPPED.store(true, Ordering::Release);
        post_to_main(main_thread, WM_BRIDGE_STOPPED);
    });
}

/// Что именно команда меняет в настройках.
enum Change {
    /// Переключение режима из трея — одно поле.
    Mode(Mode),
    /// Форма страницы настроек прислала конфиг целиком.
    ///
    /// `Box`, тем же приёмом, что и `Cmd::ApplyConfig` рядом: задача 5
    /// добавила в `Config` `office_subnets`, `net_profile` и тумблер
    /// автоматики туннеля, и вариант без коробки стал заметно крупнее
    /// `Mode` — тот самый `large_enum_variant`, который здесь уже когда-то
    /// решили боксом, а не `#[allow(...)]` (запрещён CLAUDE.md).
    Whole(Box<Config>),
}

/// Применяет изменение к сохранённому конфигу и отдаёт ЖИВОЙ — тот, с
/// которым пересобирается супервизор.
///
/// Отдельной функцией, а не двумя строками внутри цикла, ровно затем, чтобы
/// правило «порт моста не применяется на лету» проверялось ТАМ, ГДЕ ОНО
/// ДЕЙСТВУЕТ. Тест на одну лишь `settings_page::live_config` живёт слоем
/// ниже и этого не ловит: сотри её вызов отсюда — и такой тест останется
/// зелёным, а `AppState.port` начнёт называть порт, на котором никто не
/// слушает. Вместе с ним соврали бы заголовок меню, «скопировать адрес» и
/// проба порта в диагностике.
///
/// `#[must_use]` — не вежливость, а вторая половина той же защиты, и он тут
/// обязателен. Обойти функцию можно двумя способами, и без атрибута
/// компилятор ловит только первый. Заменить `&live` на `&saved` в вызове
/// супервизора — да, `live` становится неиспользуемой переменной, и
/// `-D warnings` это валит. А вот «упростить» её до вызова ради побочного
/// эффекта — `apply_change(&mut saved, change, port);` строкой, и дальше
/// `new_supervisor(&router, &saved)` — компилируется без единого замечания:
/// `Config` не помечен `#[must_use]`, и выброшенный результат никого не
/// смущает. Это и есть более естественный способ «убрать лишнюю
/// абстракцию», и без атрибута дыра открывалась бы молча.
#[must_use = "это ЖИВОЙ конфиг; выбросив его и отдав супервизору сохранённый, переедешь мост на новый порт на лету"]
fn apply_change(saved: &mut Config, change: Change, bound_port: u16) -> Config {
    match change {
        Change::Mode(mode) => saved.mode = mode,
        Change::Whole(next) => *saved = *next,
    }
    settings_page::live_config(saved, bound_port)
}

fn new_supervisor(router: &Arc<Router>, cfg: &Config) -> Supervisor {
    Supervisor::new(
        Arc::clone(router),
        Prober::new(PROBE_TTL, Duration::from_millis(cfg.dial_timeout_ms)),
        cfg.clone(),
        Box::new(NlmSource),
    )
}

/// Подписка на смену сети. Отказ не смертелен: маршрут продолжит
/// пересчитываться на переключение режима, просто перестанет реагировать на
/// переезд между сетями сам. Молчать об этом нельзя — иначе «авто перестало
/// работать» будет выглядеть как загадка.
fn spawn_network_watch(runtime: &tokio::runtime::Runtime, commands: mpsc::Sender<Cmd>) {
    // `watch_network_changes` и `debounce` заводят задачи в текущем рантайме.
    let _enter = runtime.enter();
    let raw = match watch_network_changes() {
        Ok(rx) => rx,
        Err(e) => {
            warn!(error = %e, "не подписаться на смену сети: маршрут не будет пересчитываться сам");
            return;
        }
    };
    let mut events = debounce(raw, NETWORK_DEBOUNCE);
    runtime.spawn(async move {
        while events.recv().await.is_some() {
            if commands.send(Cmd::Reevaluate).await.is_err() {
                return;
            }
        }
        warn!("подписка на смену сети закончилась");
    });
}

/// Пересчёт по таймеру.
///
/// События смены сети покрывают переезд между сетями, но не смерть апстрима
/// на неизменной сети: офисный SOCKS5 упал в 11:00, сеть та же, события нет
/// — и `health.socks` остаётся `Up` навсегда. Трей продолжает показывать
/// апстрим доступным, каждое новое соединение получает `502`, а починить это
/// можно только переключив режим руками. Спека 4.2 обещает обратное:
/// понижение до `Direct` — продуктовое поведение, «пользователь не остаётся
/// без сети», — и без таймера обещание не выполняется. Обратный переход,
/// когда апстрим оживёт, тем же тиком и случается.
///
/// Тот же самый канал `Cmd`, что у смены сети и у меню: второй путь в
/// супервизор был бы вторым местом, где пересчёт может разойтись.
fn spawn_periodic_reevaluate(runtime: &tokio::runtime::Runtime, commands: mpsc::Sender<Cmd>) {
    runtime.spawn(async move {
        // Отсчёт от «сейчас плюс период», а не от «сейчас»: `interval`
        // отдаёт первый тик немедленно, а пересчёт на старте уже сделан —
        // до создания слушателя (см. модульный комментарий).
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + REEVALUATE_PERIOD,
            REEVALUATE_PERIOD,
        );
        // Ноутбук, вернувшийся из сна, обязан получить один пересчёт, а не
        // столько тиков, сколько он проспал: поведение по умолчанию
        // (`Burst`) выдало бы их пачкой, и очередь команд на 16 мест
        // переполнилась бы на ровном месте.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            // Приёмник выброшен — приложение уже завершается.
            if commands.send(Cmd::Reevaluate).await.is_err() {
                return;
            }
        }
    });
}

/// Фоновая проверка обновлений (план 5, задача 3).
///
/// **Не блокирует старт**: заводится через `runtime.spawn`, а не
/// `block_on`, и сама функция вызывается не отсюда напрямую, а из тела
/// заведённой задачи — то есть первая проверка происходит не раньше, чем
/// истечёт `UPDATE_CHECK_INITIAL_DELAY` уже ПОСЛЕ того, как привязан
/// слушатель, поднят трей и запущена самопроверка. `update::check::run`
/// сама оборачивает сетевую работу таймаутом, так что зависшая сеть роняет
/// одну проверку, а не поток целиком.
///
/// Тумблер (`cfg.check_for_updates`) читается заново на каждом тике из
/// `saved_config`, а не один раз при запуске задачи: человек мог выключить
/// проверку уже после старта, и следующий же тик обязан это увидеть, а не
/// продолжать спрашивать сеть по инерции.
fn spawn_update_check(
    runtime: &tokio::runtime::Runtime,
    saved_config: Arc<ArcSwap<Config>>,
    update_dir: std::path::PathBuf,
    status: Arc<ArcSwap<Option<update::check::CheckOutcome>>>,
) {
    let source: Arc<dyn update::source::UpdateSource> =
        Arc::new(update::source::GithubSource::default());
    runtime.spawn(async move {
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + UPDATE_CHECK_INITIAL_DELAY,
            UPDATE_CHECK_INTERVAL,
        );
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let enabled = saved_config.load().check_for_updates;
            let outcome = update::check::run(
                env!("CARGO_PKG_VERSION").to_string(),
                enabled,
                Arc::clone(&source),
                update_dir.clone(),
                update::install::real_verifier,
                update::check::DEFAULT_TIMEOUT,
            )
            .await;
            info!(?outcome, "проверка обновлений завершена");
            status.store(Arc::new(Some(outcome)));
        }
    });
}

/// Список подключённых сетей через NLM.
///
/// Реализация трейта живёт здесь, а не в мосте: мост обязан оставаться
/// переносимым (см. модульный комментарий `supervisor.rs`), а `winnet` не
/// знает про мост и не должен от него зависеть. Приложение — единственное
/// место, которое видит оба.
struct NlmSource;

impl NetworkSource for NlmSource {
    fn connected(&self) -> Result<Vec<ConnectedNetwork>, SupervisorError> {
        // Апартамент нужен на том потоке, что зовёт NLM, а зовёт его рабочий
        // поток tokio — не тот, где живёт трей, и не обязательно один и тот
        // же от вызова к вызову. Поэтому страж создаётся здесь, на каждый
        // вызов, и снимается до возврата: COM-объекты `list_connected`
        // разрушаются раньше него.
        let _com = ComGuard::new().map_err(|e| SupervisorError::Network(e.to_string()))?;
        let networks = list_connected().map_err(|e| SupervisorError::Network(e.to_string()))?;
        // Здесь же и проходит граница переносимости: `NetworkSnapshot` —
        // тип крейта, который знает про Windows, а дальше едут простые
        // данные, ровно как это уже было с идентификатором. Категория и
        // признак интернета остаются здесь: решение принимается по GUID.
        Ok(networks
            .into_iter()
            .map(|n| ConnectedNetwork {
                id: n.id,
                name: n.name,
            })
            .collect())
    }
}

/// Автозапуск через `HKCU\...\Run`.
///
/// Реализация трейта живёт здесь же, а не в `winnet`, по той же причине, что
/// и у `NlmSource` выше: `winnet::autostart` не знает про страницу настроек
/// и не должен от неё зависеть, а свой путь исполняемого файла — это факт
/// уровня процесса, а не факт уровня реестра.
struct WinAutostart;

impl settings_page::Autostart for WinAutostart {
    fn is_enabled(&self) -> Result<bool, String> {
        autostart::is_enabled().map_err(|e| e.to_string())
    }

    fn set(&self, on: bool) -> Result<(), String> {
        if on {
            // Тот же путь, что видит сам процесс: если exe перенесли уже
            // после запуска, `current_exe()` всё равно вернёт путь, по
            // которому его запустили сейчас, — ровно то, что должно
            // оказаться в реестре, чтобы следующий запуск нашёл его там же.
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            autostart::enable(&exe).map_err(|e| e.to_string())
        } else {
            autostart::disable().map_err(|e| e.to_string())
        }
    }
}

/// Имя файла, откуда «Собрать профиль» берёт исходный `.ovpn` (сертификаты,
/// адрес сервера — то, что выдаёт офисный администратор), под именем,
/// которое не совпадёт ни с одним профилем пользователя и ни с нашим
/// собственным выходом (`TUNNEL_PROFILE_NAME`, `settings_page.rs`).
///
/// Ищется через `openvpn::find_config_file` в ОБОИХ каталогах конфигураций
/// — `Installation::user_config_dir` (куда обычный пользователь способен
/// положить файл сам, без UAC — именно туда стоит класть его по умолчанию)
/// и `Installation::config_dir` (системный, куда мог заранее положить файл
/// администратор при установке). Кладётся сам файл пользователем вручную,
/// не этим кодом — сюда он только читается.
///
/// Больше нигде в проекте это имя не специфицировано: ни один из планов
/// 1-6 не назвал источник `ovpn_profile::build_profile`, а первым реальным
/// вызывающим стала именно эта задача — см. отчёт задачи 7.
const TUNNEL_SOURCE_FILE: &str = "proxypilot-source.ovpn";

/// Реализация [`settings_page::Tunnel`] поверх `proxypilot_winnet`.
///
/// Без состояния: каждый вызов заново читает систему (`find_installation`,
/// `profile_status`, живую таблицу маршрутов) — тот же принцип, что и у
/// `openvpn.rs` (`ensure_still_installed` на каждом вызове, а не однажды
/// найденный `Installation`, который могли снести между поиском и
/// использованием).
struct WinTunnel;

impl WinTunnel {
    /// Живой список адаптеров-маршрутов для `tunnel_state`, с честным
    /// признаком отказа чтения — `snapshot` обязана СКАЗАТЬ, что не смогла
    /// проверить чужой туннель, а не тихо посчитать, что его нет.
    fn adapters() -> Result<Vec<tunnel_state::AdapterRoute>, String> {
        ip_routes::gather_ipv4_routes().map_err(|e| e.to_string())
    }
}

impl settings_page::Tunnel for WinTunnel {
    fn snapshot(
        &self,
        office_subnets: &[Ipv4Net],
        profile_name: &str,
    ) -> settings_page::TunnelSnapshot {
        let Ok(Some(inst)) = openvpn::find_installation() else {
            return settings_page::TunnelSnapshot::default();
        };
        let profile_installed = matches!(
            openvpn::profile_status(&inst, profile_name),
            Ok(ProfileStatus::Installed)
        );

        // Живость — по логу openvpn-gui.exe для нашего профиля
        // (`tunnel_log::liveness`), НЕ по имени адаптера: fix round 1
        // задачи 7 нашёл, что реальные адаптеры называются по драйверу
        // («OpenVPN Wintun», «TAP-Windows Adapter V9»), никогда — по имени
        // соединения.
        let log_up = tunnel_log::liveness(profile_name)
            .map(|live| live == tunnel_log::TunnelLiveness::Up)
            .map_err(|e| e.to_string());

        // Несёт ли ХОТЬ ОДИН туннельный адаптер наши подсети — round 2
        // задачи 7 убрал alias отсюда целиком (`tunnel_state::any_tunnel_carries`
        // больше не пытается решить, чей это адаптер).
        let carries = Self::adapters()
            .map(|adapters| tunnel_state::any_tunnel_carries(office_subnets, &adapters));

        let (our_tunnel_up, rising, foreign_tunnel_up, liveness_error, routes_error) =
            combine_tunnel_facts(log_up, carries);

        settings_page::TunnelSnapshot {
            installed: true,
            profile_installed,
            our_tunnel_up,
            rising,
            liveness_error,
            foreign_tunnel_up,
            routes_error,
        }
    }

    fn build_profile(&self, profile_name: &str, office_subnets: &[Ipv4Net]) -> Result<(), String> {
        let inst = openvpn::find_installation()
            .map_err(|e| e.to_string())?
            .ok_or("OpenVPN не найден")?;
        // Источник ищется в обоих каталогах конфигураций (докблок
        // TUNNEL_SOURCE_FILE), предпочитая пользовательский; если файла нет
        // ни там, ни там — путь в сообщении об ошибке указывает именно на
        // пользовательский каталог, потому что это единственное место, куда
        // обычный пользователь способен его положить сам.
        let source_path = openvpn::find_config_file(&inst, TUNNEL_SOURCE_FILE)
            .unwrap_or_else(|| inst.user_config_dir.join(TUNNEL_SOURCE_FILE));
        let source = std::fs::read_to_string(&source_path).map_err(|e| {
            format!(
                "не найден файл источника {}: {e}. Положите свой .ovpn \
                 (сертификаты, адрес сервера — то, что выдал администратор) \
                 туда под этим именем и нажмите «Собрать профиль» ещё раз.",
                source_path.display()
            )
        })?;
        openvpn::build_and_install_profile(&inst, profile_name, &source, office_subnets)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn raise(&self, profile_name: &str) -> Result<(), String> {
        let inst = openvpn::find_installation()
            .map_err(|e| e.to_string())?
            .ok_or("OpenVPN не найден")?;
        openvpn::connect(&inst, profile_name).map_err(|e| e.to_string())
    }

    fn lower(&self, profile_name: &str) -> Result<(), String> {
        let inst = openvpn::find_installation()
            .map_err(|e| e.to_string())?
            .ok_or("OpenVPN не найден")?;
        openvpn::disconnect(&inst, profile_name).map_err(|e| e.to_string())
    }

    fn install_service(&self) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        ui::request_elevation(&exe, "install-service")
    }
}

/// Сводит два независимых источника («лог считает поднятым?», «несёт ли
/// хоть один туннельный адаптер наши подсети?») в пять полей
/// `TunnelSnapshot` — чистая функция ради теста, отдельно от чтения лога
/// и таблицы маршрутов (`WinTunnel::snapshot` выше).
///
/// Round 2 задачи 7: `our_tunnel_up` требует ПОДТВЕРЖДЕНИЯ ОБОИХ
/// источников разом, а не одного лога — иначе `openvpn.exe`, убитый без
/// штатного выхода, продолжал бы читаться как «поднят» сколько угодно
/// долго (докблок `winnet::tunnel_log`, «Честные пределы»).
/// `foreign_tunnel_up` — что маршруты заняты, а лог НЕ подтверждает, что
/// это мы. Возврат: `(our_tunnel_up, rising, foreign_tunnel_up,
/// liveness_error, routes_error)`.
///
/// Два источника ошибок остаются независимыми (см. докблок
/// `settings_page::TunnelSnapshot`): отказ прочитать маршруты не должен
/// стирать то, что честно сказал лог, и наоборот. Особый случай — лог
/// говорит «поднято», а маршруты прочитать не удалось: подтвердить
/// подъём НЕЧЕМ (весь смысл требовать оба источника — не верить логу в
/// одиночку), поэтому это тоже «не знаю», а не «поднято на слово лога».
fn combine_tunnel_facts(
    log_up: Result<bool, String>,
    carries: Result<bool, String>,
) -> (bool, bool, bool, Option<String>, Option<String>) {
    match (log_up, carries) {
        (Ok(true), Ok(true)) => (true, false, false, None, None),
        // «Поднимается»: лог уже увидел успешное подключение, но
        // маршруты профиля (`route ...` из собранного `.ovpn`) ещё не
        // встали — короткое окно сразу после «Поднять туннель».
        (Ok(true), Ok(false)) => (false, true, false, None, None),
        (Ok(false), Ok(true)) => (false, false, true, None, None),
        (Ok(false), Ok(false)) => (false, false, false, None, None),
        // Лог точно говорит «не поднято» — про `our_tunnel_up` сказано
        // всё, что нужно, без участия маршрутов. Отказ читать маршруты
        // означает только «не проверить, свободны ли они для подъёма».
        (Ok(false), Err(e)) => (false, false, false, None, Some(e)),
        // Лог говорит «поднято», но подтвердить нечем — не верим логу в
        // одиночку (в этом и была цель объединения), и не притворяемся,
        // что знаем больше маршрутов.
        (Ok(true), Err(e)) => (
            false,
            false,
            false,
            Some(format!(
                "лог утверждает, что туннель поднят, но подтвердить это \
                 таблицей маршрутов не удалось: {e}"
            )),
            None,
        ),
        // Лог недоступен целиком — по этой же причине не подтвердить
        // `our_tunnel_up`, и по той же причине не утверждать
        // `foreign_tunnel_up`: маршруты МОГУТ быть заняты нашим же
        // туннелем, который мы просто не можем подтвердить логом
        // (регрессия round 2 — см. тест
        // `an_unreadable_log_does_not_call_our_own_tunnel_someone_elses`).
        (Err(e), Ok(_)) => (false, false, false, Some(e), None),
        (Err(e1), Err(e2)) => (
            false,
            false,
            false,
            Some(format!("{e1}; и таблица маршрутов тоже не читается: {e2}")),
            None,
        ),
    }
}

/// Общая часть, которую `open_settings` собирает в `SettingsState` заново
/// при каждом открытии страницы — все шесть значений всегда идут вместе, и
/// ни один вызывающий (`message_loop`) не передаёт их по отдельности.
struct SettingsDeps<'a> {
    runtime: &'a tokio::runtime::Runtime,
    state: &'a Arc<ArcSwap<AppState>>,
    saved_config: &'a Arc<ArcSwap<Config>>,
    commands: &'a mpsc::Sender<Cmd>,
    bound_port: u16,
    tunnel: &'a Arc<dyn settings_page::Tunnel>,
    /// Итог последней фоновой проверки обновлений (задача 3) — та же
    /// ячейка, что пишет `spawn_update_check`, а не копия: странице и
    /// заведённой задаче незачем расходиться в том, что показывать.
    update_status: &'a Arc<ArcSwap<Option<update::check::CheckOutcome>>>,
}

/// Открывает страницу настроек: поднимает сервер, если его нет, и зовёт
/// браузер.
///
/// Сервер не поднимается на старте и не живёт постоянно: он гаснет сам по
/// таймауту бездействия (см. `websrv`), поэтому перед каждым открытием
/// проверяется, что сохранённая дверь ещё та же самая. Отправить браузер по
/// адресу уже погасшего сервера значило бы показать человеку «страница
/// недоступна» вместо настроек.
///
/// `section` — якорь раздела страницы (`bench`, `doctor` и т. п.), на который
/// сразу должен прокрутиться браузер. Это фрагмент URL (`#bench`): сервер его
/// не видит и в маршрутизации не участвует, поэтому пункты меню «Замерить
/// скорость…» и «Диагностика…» — не второй вход в приложение, а то же самое
/// окно настроек с тем же сервером и тем же токеном, просто открытое сразу на
/// нужном месте.
///
/// Отказ не смертелен — приложение продолжает работать, — но и молчать о
/// нём нельзя: человек нажал пункт меню и ждёт окна.
///
/// Параметры собраны в [`SettingsDeps`], а не переданы по отдельности:
/// добавление `tunnel` (задача 7) перевалило их число за предел
/// `clippy::too_many_arguments` (7) — не обход находки, а обычная
/// группировка: все шесть всегда идут вместе, ни один вызывающий не
/// передаёт их по отдельности.
fn open_settings(deps: &SettingsDeps, server: &mut Option<websrv::Server>, section: Option<&str>) {
    if !server.as_ref().is_some_and(|s| s.is_running()) {
        let shared = Arc::new(settings_page::SettingsState {
            // Та же ячейка, что читает трей, а не копия: разойдись они —
            // и меню со страницей показывали бы разное состояние.
            app: Arc::clone(deps.state),
            config: Arc::clone(deps.saved_config),
            // Единственный путь применить изменение — тот же канал, которым
            // ходят трей и подписка на смену сети.
            commands: deps.commands.clone(),
            // Порт, на котором слушатель уже привязан. Страница обязана
            // знать именно его, а не то, что записано в конфиге: разойтись
            // они могут ровно на одну правку — ту, что требует перезапуска.
            bound_port: deps.bound_port,
            autostart: Arc::new(WinAutostart),
            tunnel: Arc::clone(deps.tunnel),
            update_status: Arc::clone(deps.update_status),
        });
        // `block_on` с главного потока безопасен: он не внутри рантайма, а
        // сама привязка слушателя занимает микросекунды — цикл сообщений
        // этого не заметит.
        match deps.runtime.block_on(websrv::Server::start(shared)) {
            Ok(s) => {
                server.replace(s);
            }
            Err(e) => {
                error!(error = %e, "сервер настроек не поднялся");
                ui::error_box(
                    "ProxyPilot",
                    &format!(
                        "Не удалось открыть настройки.

{e}"
                    ),
                );
                return;
            }
        }
    }
    let Some(mut url) = server.as_ref().map(|s| s.url().url.clone()) else {
        return;
    };
    if let Some(section) = section {
        // Фрагмент — не часть пути: он не уйдёт на сервер ни в одном
        // запросе, значит не тронет ни проверку токена, ни `Origin`.
        url = format!("{url}#{section}");
    }
    if let Err(e) = ui::open_in_browser(&url) {
        error!(error = %e, "не открыть страницу настроек в браузере");
        ui::error_box(
            "ProxyPilot",
            &format!(
                "Не удалось открыть браузер со страницей настроек.

{e}"
            ),
        );
    }
}

/// Цикл сообщений главного потока. Без него не работает ни иконка, ни меню:
/// оболочка общается с ними оконными сообщениями.
fn message_loop(tray: &Tray, deps: &SettingsDeps) -> Exit {
    let mut msg = MSG::default();
    // Сервер настроек живёт ровно столько, сколько живёт эта переменная:
    // выход из цикла — любой из четырёх — уничтожает её вместе с дверью.
    let mut settings: Option<websrv::Server> = None;
    loop {
        // До блокирующего ожидания: мост мог умереть ещё до того, как мы
        // сюда дошли, и тогда ждать сообщений незачем.
        if BRIDGE_STOPPED.load(Ordering::Acquire) {
            return Exit::BridgeStopped;
        }

        // SAFETY: `msg` — живая структура нужного типа; `None` в качестве
        // окна означает «все сообщения потока», включая присланные
        // `PostThreadMessageW`.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 == -1 {
            // Код ошибки снимаем немедленно, пока его не затёр следующий
            // вызов Windows.
            let e = windows::core::Error::from_win32();
            error!(error = %e, "GetMessageW отказала");
            return Exit::MessageLoopFailed(format!("цикл сообщений отказал: {e}"));
        }
        if got.0 == 0 {
            // WM_QUIT. Мы его не шлём, значит попросил кто-то ещё — например,
            // завершение сеанса.
            return Exit::SessionEnd;
        }

        // Проверка по флагу, а не по `msg.message`: сообщение могло не
        // дойти вовсе (см. `BRIDGE_STOPPED`), а флаг доходит всегда.
        if BRIDGE_STOPPED.load(Ordering::Acquire) {
            return Exit::BridgeStopped;
        }
        if msg.message == WM_STATE_CHANGED {
            let snapshot = deps.state.load();
            // Живой снимок туннеля — те же чтения (реестр, файловая
            // система, таблица маршрутов), что и на странице настроек, тут
            // же на главном потоке: все они быстрые локальные вызовы без
            // сети, тем же приёмом, что и `icon_for`/`network_text` рядом.
            let tunnel_snapshot = deps.tunnel.snapshot(
                &deps.saved_config.load().office_subnets,
                settings_page::TUNNEL_PROFILE_NAME,
            );
            tray.refresh(&snapshot, &tunnel_snapshot);
        }

        // SAFETY: `msg` заполнена успешным `GetMessageW` и не изменялась.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Именно здесь заканчивается вложенный цикл всплывающего меню — и
        // именно здесь ловится смерть моста, случившаяся, пока меню было
        // открыто и наше сообщение было съедено этим вложенным циклом.
        if BRIDGE_STOPPED.load(Ordering::Acquire) {
            return Exit::BridgeStopped;
        }

        // Событие меню кладётся в канал изнутри обработки сообщения выше,
        // поэтому вычерпываем канал сразу после диспетчеризации.
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match tray.action_for(event.id()) {
                Some(Action::Quit) => return Exit::User,
                Some(Action::CopyAddress) => tray.copy_address(),
                Some(Action::OpenSettings) => open_settings(deps, &mut settings, None),
                Some(Action::OpenBench) => open_settings(deps, &mut settings, Some("bench")),
                Some(Action::OpenDoctor) => open_settings(deps, &mut settings, Some("doctor")),
                Some(Action::OpenTunnel) => open_settings(deps, &mut settings, Some("tunnel")),
                Some(Action::SetMode(mode)) => {
                    // `try_send`, а не `blocking_send`: главный поток обязан
                    // вернуться в цикл сообщений — застрявший цикл выглядит
                    // как зависшая система, а очередь на 16 команд от кликов
                    // мышью переполниться не может.
                    if let Err(e) = deps.commands.try_send(Cmd::SetMode(mode)) {
                        warn!(error = %e, ?mode, "команда смены режима не доставлена");
                    }
                }
                None => {}
            }
        }
    }
}

fn post_to_main(thread_id: u32, message: u32) {
    // SAFETY: `PostThreadMessageW` сама проверяет идентификатор потока и при
    // несуществующем возвращает ошибку, а не портит память. Отказ означает,
    // что главный поток уже вышел, — сообщать больше некому и не о чем.
    let _ = unsafe { PostThreadMessageW(thread_id, message, WPARAM(0), LPARAM(0)) };
}

/// Обработчик закрытия консоли и Ctrl+C. Только отладочная сборка: в релизе
/// (`windows_subsystem = "windows"`) у процесса нет консоли, а
/// `SetConsoleCtrlHandler` без неё ничего не ловит — событий этого рода
/// попросту не бывает. Функция существует ровно ради одного: `Drop` стража
/// при закрытии окна консоли не вызывается, а системный прокси всё равно
/// обязан вернуться.
///
/// Аналог для релиза — не эта функция, а `WM_ENDSESSION` в оконной
/// процедуре трея (`tray::install_session_end_guard`): у релизной сборки
/// нет консоли, но есть окно, и завершение сеанса Windows приходит именно
/// туда.
#[cfg(debug_assertions)]
unsafe extern "system" fn on_console_ctrl(_ctrl_type: u32) -> BOOL {
    proxy::restore();
    // FALSE — «не обработали»: пусть система завершит процесс, как и
    // собиралась. Наше дело здесь только прибраться.
    BOOL(0)
}

#[cfg(debug_assertions)]
fn install_console_handler() {
    // SAFETY: функция-обработчик статическая и живёт столько же, сколько
    // процесс. Отказ означает лишь отсутствие консоли у процесса — тогда и
    // событий этих не будет.
    if let Err(e) = unsafe { SetConsoleCtrlHandler(Some(on_console_ctrl), true) } {
        info!(error = %e, "обработчик закрытия консоли не установлен");
    }
}

/// В релизе консоли нет, а значит и ставить обработчик её закрытия незачем:
/// событий `SetConsoleCtrlHandler` без консоли не бывает вовсе. Тело пустое
/// нарочно — вызывающий код (`run`) не должен знать, отладочная это сборка
/// или релизная, поэтому функция существует в обоих вариантах.
#[cfg(not(debug_assertions))]
fn install_console_handler() {}

#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::config::OfficeNetwork;

    // ---- Задача 7, fix round 2: combine_tunnel_facts ----

    #[test]
    fn both_sources_confirm_up() {
        assert_eq!(
            combine_tunnel_facts(Ok(true), Ok(true)),
            (true, false, false, None, None)
        );
    }

    #[test]
    fn log_up_but_routes_not_installed_yet_is_rising_not_alarming() {
        // Round 2: короткое окно сразу после «Поднять туннель» —
        // лог уже видел успех, маршруты профиля ещё не встали. Не «не
        // поднят» (это выглядело бы как «нажмите снова») и не «поднят»
        // (маршруты ещё не доказали это) — отдельное честное состояние.
        let (our_tunnel_up, rising, foreign_tunnel_up, liveness_error, routes_error) =
            combine_tunnel_facts(Ok(true), Ok(false));
        assert!(!our_tunnel_up);
        assert!(rising);
        assert!(!foreign_tunnel_up);
        assert!(liveness_error.is_none());
        assert!(routes_error.is_none());
    }

    #[test]
    fn routes_carried_but_log_says_not_up_is_foreign() {
        assert_eq!(
            combine_tunnel_facts(Ok(false), Ok(true)),
            (false, false, true, None, None)
        );
    }

    #[test]
    fn neither_source_reports_anything_is_plain_down() {
        assert_eq!(
            combine_tunnel_facts(Ok(false), Ok(false)),
            (false, false, false, None, None)
        );
    }

    #[test]
    fn a_hard_killed_process_stops_claiming_up_once_routes_disappear() {
        // Ровно сценарий, ради которого round 2 затеян: openvpn.exe убит
        // без штатного выхода — лог (в одиночку) продолжал бы врать
        // «поднято», но маршруты уходят с процессом почти сразу.
        // combine_tunnel_facts обязана погасить our_tunnel_up, а не
        // поверить логу на слово.
        let (our_tunnel_up, ..) = combine_tunnel_facts(Ok(true), Ok(false));
        assert!(!our_tunnel_up, "лог в одиночку не должен решать «поднято»");
    }

    #[test]
    fn routes_unreadable_while_log_says_down_blocks_only_the_raise_reason() {
        // Лог уверенно говорит «не поднято» — про our_tunnel_up вопросов
        // нет без всякого участия маршрутов. Отказ читать маршруты сюда
        // не просачивается как liveness_error.
        let (our_tunnel_up, rising, foreign_tunnel_up, liveness_error, routes_error) =
            combine_tunnel_facts(Ok(false), Err("тестовый отказ".to_string()));
        assert!(!our_tunnel_up);
        assert!(!rising);
        assert!(!foreign_tunnel_up);
        assert!(liveness_error.is_none());
        assert_eq!(routes_error.as_deref(), Some("тестовый отказ"));
    }

    #[test]
    fn log_says_up_but_routes_cannot_confirm_it_is_reported_as_unknown() {
        // «Поднято» на одно лишь слово лога, без маршрутов, подтвердить
        // нечем — ровно то доверие логу в одиночку, ради устранения
        // которого и введено объединение. Не «поднято», а «не знаю».
        let (our_tunnel_up, rising, foreign_tunnel_up, liveness_error, routes_error) =
            combine_tunnel_facts(Ok(true), Err("тестовый отказ".to_string()));
        assert!(!our_tunnel_up);
        assert!(!rising);
        assert!(!foreign_tunnel_up);
        assert!(liveness_error.is_some());
        assert!(routes_error.is_none());
    }

    #[test]
    fn an_unreadable_log_does_not_call_our_own_tunnel_someone_elses() {
        // Регрессия, которая мотивировала round 2: лог нечитаем, а
        // какой-то туннельный адаптер несёт наши подсети (возможно, это
        // как раз наш собственный, только что поднятый, туннель — узнать
        // логом сейчас нельзя). Страница обязана сказать «не знаю», а не
        // «занято чужим» — до этого исправления `foreign_tunnel_up`
        // строилась бы по alias'у адаптера и ошибочно утверждала бы
        // именно это.
        let (our_tunnel_up, rising, foreign_tunnel_up, liveness_error, routes_error) =
            combine_tunnel_facts(Err("тестовый отказ чтения лога".to_string()), Ok(true));
        assert!(!our_tunnel_up);
        assert!(!rising);
        assert!(
            !foreign_tunnel_up,
            "неизвестность лога не обязана превращаться в «занято чужим»"
        );
        assert_eq!(
            liveness_error.as_deref(),
            Some("тестовый отказ чтения лога")
        );
        assert!(routes_error.is_none());
    }

    #[test]
    fn an_unreadable_log_stays_unknown_even_when_routes_are_free() {
        let (our_tunnel_up, rising, foreign_tunnel_up, liveness_error, _) =
            combine_tunnel_facts(Err("тестовый отказ".to_string()), Ok(false));
        assert!(!our_tunnel_up);
        assert!(!rising);
        assert!(!foreign_tunnel_up);
        assert!(liveness_error.is_some());
    }

    #[test]
    fn both_sources_failing_mentions_both_errors() {
        let (.., liveness_error, routes_error) = combine_tunnel_facts(
            Err("отказ лога".to_string()),
            Err("отказ маршрутов".to_string()),
        );
        let msg = liveness_error.expect("оба отказа обязаны быть сообщены");
        assert!(msg.contains("отказ лога"), "получили: {msg}");
        assert!(msg.contains("отказ маршрутов"), "получили: {msg}");
        assert!(routes_error.is_none());
    }

    /// Порт, на котором слушатель привязан в этих тестах, и порт, который
    /// «вводит человек». Разные намеренно: совпади они — тесты не различали
    /// бы соблюдение правила и его отсутствие.
    const BOUND: u16 = 3129;
    const REQUESTED: u16 = 3999;

    #[test]
    fn a_port_change_does_not_reach_the_config_the_supervisor_gets() {
        // Правило, ради которого написана вся задача, проверяется в той
        // самой функции, что его применяет. Убери из `apply_change` вызов
        // `live_config` — этот тест покраснеет; тест на саму `live_config`
        // остался бы зелёным, потому что живёт слоем ниже.
        let mut saved = Config {
            bridge_port: BOUND,
            ..Config::default()
        };
        let live = apply_change(
            &mut saved,
            Change::Whole(Box::new(Config {
                bridge_port: REQUESTED,
                ..Config::default()
            })),
            BOUND,
        );
        assert_eq!(
            saved.bridge_port, REQUESTED,
            "на диск обязано лечь введённое значение"
        );
        assert_eq!(
            live.bridge_port, BOUND,
            "супервизор обязан остаться на привязанном порту"
        );
    }

    #[test]
    fn everything_except_the_port_does_reach_the_supervisor() {
        // Обратная половина того же правила: задерживается ТОЛЬКО порт.
        // Без неё «правило» выполнила бы и функция, не пропускающая ничего.
        let mut saved = Config {
            bridge_port: BOUND,
            ..Config::default()
        };
        let live = apply_change(
            &mut saved,
            Change::Whole(Box::new(Config {
                bridge_port: REQUESTED,
                socks_upstream: Some("203.0.113.10:9999".into()),
                office_networks: vec![OfficeNetwork {
                    id: "{A}".into(),
                    name: "Офис".into(),
                }],
                ..Config::default()
            })),
            BOUND,
        );
        assert_eq!(live.socks_upstream.as_deref(), Some("203.0.113.10:9999"));
        assert_eq!(live.office_networks.len(), 1);
        assert_eq!(live.bridge_port, BOUND);
    }

    #[test]
    fn switching_the_mode_does_not_smuggle_a_pending_port_change_through() {
        // Человек сменил порт и ещё не перезапустился: на диске уже новый.
        // Переключение режима в трее не должно протащить его в супервизор
        // «заодно» — иначе правило обходилось бы одним кликом по меню.
        let mut saved = Config {
            bridge_port: REQUESTED,
            mode: Mode::Auto,
            ..Config::default()
        };
        let live = apply_change(&mut saved, Change::Mode(Mode::Socks), BOUND);
        assert_eq!(live.mode, Mode::Socks, "смена режима обязана доехать");
        assert_eq!(live.bridge_port, BOUND, "а порт — нет");
        assert_eq!(
            saved.bridge_port, REQUESTED,
            "и на диске он остаётся тем, что ввёл человек"
        );
    }

    #[test]
    fn the_periodic_reevaluation_is_slower_than_the_probe_cache() {
        // Тик чаще TTL проб — холостой: `Prober` вернул бы кэш, и смерть
        // апстрима осталась бы незамеченной до следующего тика, зато вызовы
        // NLM тратились бы вдвое чаще. Инвариант держится этим тестом:
        // трогать одну из констант, не взглянув на вторую, не выйдет.
        assert!(
            REEVALUATE_PERIOD > PROBE_TTL,
            "период пересчёта ({REEVALUATE_PERIOD:?}) обязан быть больше TTL проб ({PROBE_TTL:?})"
        );
    }

    #[test]
    fn the_window_messages_do_not_collide() {
        // Оба сообщения ходят по одной очереди главного потока, и совпадение
        // номеров означало бы перепутанные события: «перерисуй меню» вместо
        // «мост умер» — то самое состояние, ради устранения которого заведён
        // `BRIDGE_STOPPED`.
        assert_ne!(WM_STATE_CHANGED, WM_BRIDGE_STOPPED);
    }

    #[test]
    fn win_autostart_is_enabled_does_not_fail() {
        // Смоук без мутаций реестра (по образцу
        // `sysproxy::reading_current_settings_does_not_fail`): доказывает,
        // что этот адаптер реально доходит до `winnet::autostart::is_enabled`
        // и не падает, а не просто компилируется рядом с ним. Мутирующий
        // путь (`set`) проверен отдельным ручным тестом ниже.
        use crate::settings_page::Autostart as _;
        let _ = WinAutostart
            .is_enabled()
            .expect("is_enabled обязан читаться");
    }

    #[test]
    #[ignore = "трогает настоящий Run этой машины: гонять только руками"]
    fn win_autostart_set_round_trips_through_the_real_registry() {
        // Тот же живой реестр, что и в `winnet::autostart`'s собственном
        // ignored-тесте, но через реальный адаптер приложения — доказывает,
        // что `WinAutostart::set` действительно берёт `current_exe()` и зовёт
        // `enable`/`disable`, а не просто собирается рядом с ними.
        //
        // Раньше восстановление здесь было прямолинейным кодом ПОСЛЕ
        // проверок (`if previous == Ok(true) { win.set(true) }`), а не
        // страж-Drop'ом: упавший `assert!` пропускал восстановление
        // насквозь и оставлял `ProxyPilot` указывающим на тестовый
        // бинарник в РЕАЛЬНОМ `Run` того, кто это запустил, — то есть fix
        // round 2 нашёл в этом тесте ровно ту опасность, ради устранения
        // которой был написан finding №3 предыдущего раунда. Хуже того,
        // `previous == Ok(false)` не отличало «было пусто» от «стояла
        // чужая запись, указывающая куда-то ещё» — вторую тест удалял и
        // никогда не возвращал. Теперь — тот же страж, что и в
        // `winnet::autostart`, через сырую строку
        // (`raw_value_for_tests`/`restore_raw_value_for_tests`), а не
        // булеву сводку: адаптер `settings_page::Autostart` даёт только
        // `bool`, поэтому для восстановления берём `winnet::autostart`
        // напрямую, а не через `WinAutostart`.
        struct RestorePrevious(String, u32);
        impl Drop for RestorePrevious {
            fn drop(&mut self) {
                // Не паникуем даже в Drop — по той же причине, что и в
                // `winnet::autostart`'s страже: он может отрабатывать во
                // время уже идущей паники этого же теста.
                if let Err(e) = autostart::restore_raw_value_for_tests(&self.0, self.1) {
                    eprintln!("не удалось восстановить прежнее значение автозапуска: {e}");
                }
            }
        }

        use crate::settings_page::Autostart as _;
        let (previous, previous_type) =
            autostart::raw_value_for_tests().expect("Run обязан читаться перед тестом");
        let _restore = RestorePrevious(previous, previous_type);

        let win = WinAutostart;
        win.set(true)
            .expect("включение обязано пройти без прав администратора");
        assert!(
            win.is_enabled().expect("is_enabled обязан читаться"),
            "после set(true) тумблер обязан показывать «включено»"
        );

        win.set(false).expect("выключение обязано пройти");
        assert!(
            !win.is_enabled().expect("is_enabled обязан читаться"),
            "после set(false) тумблер обязан показывать «выключено»"
        );
    }
}
