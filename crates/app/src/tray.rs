//! Иконка в трее и её меню.
//!
//! Живёт строго на главном потоке: `tray-icon` создаёт скрытое окно, и все
//! обращения к нему обязаны идти с того же потока, что крутит цикл сообщений.
//! Поэтому здесь нет ни `Send`, ни блокировок — состояние приходит снимком
//! из `AppState`, а не читается из моста.
//!
//! То же скрытое окно — единственное окно главного потока и, значит,
//! единственный адресат `WM_QUERYENDSESSION`/`WM_ENDSESSION`: Windows
//! рассылает их каждому окну верхнего уровня в системе, видимому или нет
//! (`tray-icon` создаёт своё с нулевым родителем, `CreateWindowExW(..., null,
//! ...)`, а не `HWND_MESSAGE` — иначе оно вообще не участвовало бы в этой
//! рассылке). Заводить под это отдельное окно было бы лишним: пришлось бы
//! крутить для него свой цикл сообщений, а `GetMessageW(None, ...)` в
//! `main.rs` и так вычерпывает сообщения всех окон главного потока —
//! система доставляет присланные (`SendMessageW`, а не `PostMessageW`)
//! сообщения оконной процедуре напрямую, пока поток стоит в `GetMessageW`,
//! без явной диспетчеризации из цикла. `install_session_end_guard` подменяет
//! эту процедуру, а не трогает `GWL_USERDATA` — та ячейка занята самим
//! `tray-icon` (там указатель на его `TrayUserData`), и её перезапись
//! сломала бы обработку всех остальных сообщений трея.

use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, Ordering};

use proxypilot_bridge::supervisor::AppState;
use proxypilot_core::mode::{Mode, Reachability, Route};
use tracing::{info, warn};
use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::core::Error as WinError;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GetWindowLongPtrW, SetWindowLongPtrW, GWLP_WNDPROC,
    WM_ENDSESSION, WM_QUERYENDSESSION, WNDPROC,
};

use crate::icons::{icon_for, rgba, IconKind, ICON_SIDE};
use crate::settings_page::TunnelSnapshot;

/// Оконная процедура, которая стояла на окне трея ДО нашей подмены.
/// Глобальная — потому что новой процедуре (`session_end_wndproc`, это
/// `extern "system" fn`, а не замыкание) больше неоткуда её взять: у
/// Windows нет способа передать в `WNDPROC` собственное состояние.
/// `install_session_end_guard` вызывается ровно один раз за жизнь процесса
/// (из `main`, сразу после `Tray::new`), поэтому гонки за этой ячейкой нет.
///
/// На самом деле это почти наверняка НЕ `tray_proc` из `tray-icon` напрямую,
/// а диспетчер подклассов comctl32: `Tray::new` вызывает
/// `TrayIconBuilder::with_menu(...).build()`, а внутри `build()` `tray-icon`
/// сам подключает меню через `attach_menu_subclass_for_hwnd`, что для `muda`
/// означает `SetWindowSubclass` (`windows_sys::Win32::UI::Shell`), — то есть
/// к моменту, когда `install_session_end_guard` читает текущий `GWLP_WNDPROC`
/// (уже ПОСЛЕ `Tray::new`), там стоит не `tray_proc`, а обёртка comctl32,
/// которая сама вызывает `menu_subclass_proc` муды и в конце — `tray_proc`
/// через `DefSubclassProc`. Для пересылки необработанных сообщений это не
/// проблема: `CallWindowProcW(prev, ...)` работает с любым `WNDPROC`,
/// диспетчер comctl32 включая, и корректно доводит их до конца цепочки —
/// собственно, работающее меню и переключение режима в ручной проверке это
/// и подтверждают.
///
/// Но есть последствие, важное не сегодня, а при будущих изменениях:
/// `RemoveWindowSubclass` (её вызывает `TrayIcon::drop` через
/// `detach_menu_subclass_from_hwnd`, когда снимается последний подкласс)
/// восстанавливает `GWLP_WNDPROC` в то значение, которое comctl32 запомнил
/// как исходное на момент первого `SetWindowSubclass` — то есть в `tray_proc`,
/// — не спрашивая, что там стоит на самом деле. Наша подмена при этом
/// молча слетает. Сегодня это не страшно ровно потому, что происходит
/// только при `Drop`: настоящий логофф убивает процесс раньше, чем успеет
/// отработать хоть один `Drop`, а обычный путь «Выход» восстанавливает
/// системный прокси совсем другим механизмом (`proxy::RestoreOnDrop`,
/// а не этим оконным стражем) — то есть именно в момент, когда страж уже
/// не нужен. ВАЖНО: если когда-нибудь появится вызов `TrayIcon::set_menu`
/// во время работы (смена меню на лету), он тем же путём — detach/attach
/// подкласса — молча снимет эту подмену задолго до `Drop`, и тогда
/// перехват `WM_ENDSESSION` придётся переустанавливать заново после
/// каждой смены меню.
static PREV_WNDPROC: AtomicIsize = AtomicIsize::new(0);

/// Что пользователь выбрал в меню.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SetMode(Mode),
    CopyAddress,
    /// Открыть страницу настроек. Сервер под неё поднимается по этому
    /// нажатию и гаснет сам (`websrv`) — постоянно открытой двери в
    /// настройки быть не должно.
    OpenSettings,
    /// Открыть ту же страницу настроек, но сразу на разделе замера
    /// (`#bench`). Второго сервера и второго токена это не заводит: раздел —
    /// это фрагмент URL, который сервер не видит и не участвует в маршрутизации.
    OpenBench,
    /// То же самое для раздела диагностики (`#doctor`).
    OpenDoctor,
    /// То же самое для раздела туннеля (`#tunnel`, задача 7) — управление
    /// (поднять/опустить, собрать профиль, установить службу) живёт там же,
    /// не в самом меню: там есть место для предупреждений (про UAC, про
    /// DNS, про чужой туннель), а пункт меню — это одна строка без него.
    OpenTunnel,
    Quit,
}

/// Порядок режимов в меню — он же порядок в макетах macOS-версии.
const MODES: [Mode; 4] = [Mode::Auto, Mode::Socks, Mode::Http, Mode::Direct];

/// Адрес, который клиенты вписывают себе в настройки.
pub fn bridge_address(port: u16) -> String {
    format!("127.0.0.1:{port}")
}

/// Заголовок меню: адрес моста и то, что с трафиком происходит на самом деле.
///
/// Понижение показывается, а не скрывается. Молчаливый обход выглядит как
/// «галочка стоит на SOCKS5, а трафик идёт мимо» — ровно то, что спека 4.2
/// запрещает: сохранённое предпочтение не меняется и вернётся само, но знать
/// об этом обязан пользователь, а не только лог.
pub fn header_text(state: &AppState) -> String {
    format!(
        "Мост {} · {}",
        bridge_address(state.port),
        situation_text(state)
    )
}

fn situation_text(state: &AppState) -> String {
    if state.demoted {
        return format!("{} недоступен → работаем напрямую", mode_name(state.mode));
    }
    match &state.route {
        Route::Socks(addr) => format!("SOCKS5 → {addr}"),
        Route::Http(addr) => format!("HTTP → {addr}"),
        Route::Direct => "напрямую".to_string(),
    }
}

/// Строка про сеть, по которой принято решение.
///
/// Отдельным пунктом, а не в заголовке: заголовок дублируется во всплывающую
/// подсказку иконки, а у неё длина ограничена — адрес моста, маршрут и имя
/// сети туда вместе не помещаются и обрежутся ровно посередине.
///
/// Имя, а не GUID: GUID человеку ничего не говорит, а сверять он будет с тем,
/// что показывает Windows. Пустое имя (сеть без профиля) — случай реальный,
/// и тогда GUID честнее пустого места после двоеточия.
pub fn network_text(state: &AppState) -> String {
    let name = state
        .place
        .network_name
        .as_deref()
        .filter(|n| !n.is_empty())
        .or(state.place.network.as_deref());
    match name {
        Some(name) if state.place.in_office => format!("Сеть: {name} · офис"),
        Some(name) => format!("Сеть: {name}"),
        None => "Сеть: не определена".to_string(),
    }
}

/// Строка про туннель — короткая версия того же снимка, что подробно
/// разбирает раздел «Туннель» на странице настроек: пункт меню это одна
/// строка, места на предупреждения (про DNS, про UAC, про чужой туннель) в
/// ней нет — за подробностями пункт `Action::OpenTunnel` ведёт туда же.
/// Тот же приоритет, что и `settings_page::tunnel_section` (fix round 1,
/// задача 7): `our_tunnel_up` (лог OpenVPN GUI, ключ — имя профиля)
/// проверяется раньше `foreign_tunnel_up` (таблица маршрутов + алиас
/// адаптера) — иначе свой же поднятый туннель, ошибочно прочитанный по
/// алиасу как чужой, показывал бы в меню «обнаружен чужой» вместо
/// «поднят», и разошёлся бы со страницей настроек, которая уже покажет
/// кнопку «опустить».
pub fn tunnel_text(snap: &TunnelSnapshot) -> String {
    if !snap.installed {
        return "Туннель: OpenVPN не установлен".to_string();
    }
    if snap.liveness_error.is_some() {
        return "Туннель: состояние неизвестно".to_string();
    }
    if snap.our_tunnel_up {
        return "Туннель: поднят".to_string();
    }
    if snap.rising {
        // Round 2: лог уже подтвердил успех, маршруты профиля ещё не
        // встали — короткое окно сразу после «Поднять туннель».
        return "Туннель: поднимается…".to_string();
    }
    if snap.routes_error.is_some() {
        return "Туннель: опущен · маршруты не проверены".to_string();
    }
    if snap.foreign_tunnel_up {
        return "Туннель: опущен · маршруты заняты другим адаптером".to_string();
    }
    if snap.profile_installed {
        "Туннель: опущен".to_string()
    } else {
        "Туннель: опущен · профиль не собран".to_string()
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Auto => "Авто",
        Mode::Socks => "SOCKS5",
        Mode::Http => "HTTP",
        Mode::Direct => "Напрямую",
    }
}

/// Подпись пункта режима с индикатором доступности.
///
/// «Не задан» отделено от «недоступен» намеренно: первое чинится настройкой,
/// второе — сетью, и путать их значит посылать человека чинить не то.
fn mode_label(mode: Mode, state: &AppState) -> String {
    let health = match mode {
        Mode::Socks => Some(state.health.socks),
        Mode::Http => Some(state.health.http),
        Mode::Auto | Mode::Direct => None,
    };
    match health {
        None => mode_name(mode).to_string(),
        Some(Reachability::Up) => format!("{} · доступен", mode_name(mode)),
        Some(Reachability::Down) => format!("{} · недоступен", mode_name(mode)),
        Some(Reachability::Unknown) => format!("{} · не задан", mode_name(mode)),
    }
}

pub struct Tray {
    icon: TrayIcon,
    header: MenuItem,
    /// Сеть, по которой принято решение (спека 11.1, секция сети). Пункт
    /// неактивный: это надпись, а кнопка «эта сеть — офис» (спека 6.1) живёт
    /// на странице настроек — там же, где конфиг, в который она пишет.
    network: MenuItem,
    /// Короткий снимок состояния туннеля (см. `tunnel_text`). Тоже
    /// неактивная надпись — управление (кнопки, предупреждения про DNS и
    /// UAC) живёт на странице настроек, куда ведёт `open_tunnel` ниже.
    tunnel: MenuItem,
    modes: Vec<(Mode, CheckMenuItem)>,
    settings: MenuItem,
    bench: MenuItem,
    doctor: MenuItem,
    open_tunnel: MenuItem,
    copy: MenuItem,
    quit: MenuItem,
    /// Какая иконка сейчас нарисована. `Shell_NotifyIcon` на каждое
    /// обновление — лишняя работа и заметное мигание в трее, а состояние
    /// пересчитывается чаще, чем меняется.
    shown: Cell<Option<IconKind>>,
    /// `Cell`, а не поле: `Tray` живёт за `&`, а порт меняется вместе с
    /// конфигом (перепривязка требует перезапуска, но подпись обязана
    /// соответствовать тому, что показал супервизор).
    port: Cell<u16>,
}

impl Tray {
    pub fn new(state: &AppState, tunnel: &TunnelSnapshot) -> Result<Self, String> {
        let header = MenuItem::new(header_text(state), false, None);
        let network = MenuItem::new(network_text(state), false, None);
        let tunnel_item = MenuItem::new(tunnel_text(tunnel), false, None);
        let modes: Vec<(Mode, CheckMenuItem)> = MODES
            .iter()
            .map(|&m| {
                (
                    m,
                    CheckMenuItem::new(mode_label(m, state), true, state.mode == m, None),
                )
            })
            .collect();
        let settings = MenuItem::new("Настройки…", true, None);
        // «Замерить скорость…», «Диагностика…» и «Туннель…» — та же
        // страница настроек, открытая сразу на нужном разделе (спека 11.2
        // отдаёт им якорь `#bench`/`#doctor`/`#tunnel`, а не отдельный
        // сервер): второго входа в приложение заводить незачем, а держать
        // его — значит держать второй токен и второй слушатель loopback.
        let bench = MenuItem::new("Замерить скорость…", true, None);
        let doctor = MenuItem::new("Диагностика…", true, None);
        let open_tunnel = MenuItem::new("Туннель…", true, None);
        let copy = MenuItem::new("Копировать адрес моста", true, None);
        let quit = MenuItem::new("Выход", true, None);

        let menu = Menu::new();
        menu.append(&header).map_err(|e| e.to_string())?;
        menu.append(&network).map_err(|e| e.to_string())?;
        menu.append(&tunnel_item).map_err(|e| e.to_string())?;
        // Каждому разделителю — свой экземпляр: у `muda` пункт меню несёт
        // собственный идентификатор и состояние, и один и тот же объект,
        // добавленный в меню трижды, — не три разделителя, а одна запись,
        // вставленная три раза.
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| e.to_string())?;
        for (_, item) in &modes {
            menu.append(item).map_err(|e| e.to_string())?;
        }
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| e.to_string())?;
        menu.append(&settings).map_err(|e| e.to_string())?;
        menu.append(&bench).map_err(|e| e.to_string())?;
        menu.append(&doctor).map_err(|e| e.to_string())?;
        menu.append(&open_tunnel).map_err(|e| e.to_string())?;
        menu.append(&copy).map_err(|e| e.to_string())?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| e.to_string())?;
        menu.append(&quit).map_err(|e| e.to_string())?;

        let kind = icon_for(state);
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(header_text(state))
            .with_icon(make_icon(kind)?)
            .build()
            .map_err(|e| format!("не создать иконку в трее: {e}"))?;

        Ok(Self {
            icon,
            header,
            network,
            tunnel: tunnel_item,
            modes,
            settings,
            bench,
            doctor,
            open_tunnel,
            copy,
            quit,
            shown: Cell::new(Some(kind)),
            port: Cell::new(state.port),
        })
    }

    /// Приводит меню и иконку в соответствие свежему состоянию.
    pub fn refresh(&self, state: &AppState, tunnel: &TunnelSnapshot) {
        self.port.set(state.port);
        let text = header_text(state);
        self.header.set_text(&text);
        self.network.set_text(network_text(state));
        self.tunnel.set_text(tunnel_text(tunnel));
        if let Err(e) = self.icon.set_tooltip(Some(&text)) {
            warn!(error = %e, "не обновить подсказку иконки");
        }
        for (mode, item) in &self.modes {
            item.set_text(mode_label(*mode, state));
            item.set_checked(state.mode == *mode);
        }

        let kind = icon_for(state);
        if self.shown.get() == Some(kind) {
            return;
        }
        match make_icon(kind).and_then(|i| {
            self.icon
                .set_icon(Some(i))
                .map_err(|e| format!("не сменить иконку: {e}"))
        }) {
            Ok(()) => {
                // На вопрос «что было видно у пользователя в трее» иначе
                // ответить нечем: картинку в лог не положишь.
                info!(?kind, route = ?state.route, "иконка трея сменилась");
                self.shown.set(Some(kind));
            }
            // Не смертельно: меню и подсказка уже говорят правду. Но
            // расхождение картинки с состоянием — именно то, на что потом
            // жалуются, поэтому в лог обязательно.
            Err(e) => {
                self.shown.set(None);
                warn!(error = %e, ?kind, "иконка в трее не отражает состояние");
            }
        }
    }

    /// Какому пункту меню принадлежит событие.
    pub fn action_for(&self, id: &MenuId) -> Option<Action> {
        if id == self.quit.id() {
            return Some(Action::Quit);
        }
        if id == self.copy.id() {
            return Some(Action::CopyAddress);
        }
        if id == self.settings.id() {
            return Some(Action::OpenSettings);
        }
        if id == self.bench.id() {
            return Some(Action::OpenBench);
        }
        if id == self.doctor.id() {
            return Some(Action::OpenDoctor);
        }
        if id == self.open_tunnel.id() {
            return Some(Action::OpenTunnel);
        }
        self.modes
            .iter()
            .find(|(_, item)| item.id() == id)
            .map(|(mode, _)| Action::SetMode(*mode))
    }

    /// Кладёт адрес моста в буфер обмена — в том виде, в каком его вписывают
    /// в настройки клиента.
    pub fn copy_address(&self) {
        let text = format!("http://{}", bridge_address(self.port.get()));
        if let Err(e) = copy_to_clipboard(&text) {
            warn!(error = %e, "не скопировать адрес в буфер обмена");
        }
    }

    /// Ставит перехват `WM_QUERYENDSESSION`/`WM_ENDSESSION` на окно трея.
    /// См. модульный комментарий — почему именно это окно и почему подменой
    /// процедуры, а не отдельным окном.
    ///
    /// Вызывать ровно один раз, сразу после `Tray::new`, и до того, как
    /// что-либо ещё в процессе получит право менять системный прокси
    /// (`take_over`): иначе останется зазор, в котором конец сеанса некому
    /// перехватить.
    pub fn install_session_end_guard(&self) {
        // SAFETY: `hwnd` — живое окно только что созданного `TrayIcon`,
        // владеет им этот же (главный) поток. `session_end_wndproc` живёт
        // статически. Возврат `SetWindowLongPtrW` — прежняя процедура
        // (на практике это диспетчер подклассов comctl32, а не `tray_proc`
        // напрямую, см. комментарий у `PREV_WNDPROC`), сохраняем её, чтобы
        // передавать необработанные сообщения дальше.
        let hwnd = self.hwnd();
        // Через указатель, а не напрямую в целое: прямое приведение функции
        // к числу — это уже само по себе предупреждение (может незаметно
        // обрезать адрес там, где указатель шире целого), а окольный путь
        // через `*const ()` его снимает, ничего не меняя по факту.
        let addr = session_end_wndproc as *const () as isize;

        // Сначала запомнить, потом подменить — а не наоборот.
        //
        // Между `SetWindowLongPtrW` и записью в `PREV_WNDPROC` новая
        // процедура УЖЕ стоит на окне, и пришедшее в этот зазор сообщение
        // увидело бы нулевой `PREV_WNDPROC`: оно ушло бы в `DefWindowProcW`
        // мимо всей цепочки `tray-icon`, то есть щелчок по иконке или
        // выбор пункта меню просто пропал бы. На практике недостижимо —
        // между двумя инструкциями главного потока сообщений не
        // обрабатывается, — но правильный порядок ничего не стоит.
        //
        // SAFETY: `hwnd` — живое окно только что созданного `TrayIcon`,
        // владеет им этот же (главный) поток; чтение `GWLP_WNDPROC` память
        // не трогает.
        let prev = unsafe { GetWindowLongPtrW(hwnd, GWLP_WNDPROC) };
        PREV_WNDPROC.store(prev, Ordering::Release);

        // SAFETY: то же живое окно того же потока; `session_end_wndproc`
        // живёт статически и имеет нужную сигнатуру.
        let replaced = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, addr) };
        if replaced != prev {
            // Кто-то встал в цепочку между чтением и подменой. Авторитетно
            // то, что вернула сама подмена: сохранив прочитанное раньше, мы
            // выкинули бы его звено и сломали бы то, что оно обслуживало.
            warn!("оконная процедура сменилась между чтением и подменой, берём возврат подмены");
            PREV_WNDPROC.store(replaced, Ordering::Release);
        }
    }

    fn hwnd(&self) -> HWND {
        // `tray-icon` отдаёт хэндл как `windows_sys::HWND` (голый `isize` в
        // используемой версии) — свой `windows`, отдельный от нашего.
        // Конвертация — это она и есть, без какого-либо смысла помимо смены
        // обёртки: число остаётся тем же адресом окна.
        HWND(self.icon.window_handle() as *mut _)
    }
}

/// Действительно ли сеанс завершается, а не было отменено чужим вето на
/// `WM_QUERYENDSESSION`. `WM_ENDSESSION` шлётся в обоих случаях; отличает их
/// только `wParam`. Вынесено в чистую функцию ради теста — гонять настоящий
/// `WPARAM` можно только через реальное оконное сообщение.
fn session_is_ending(wparam: WPARAM) -> bool {
    wparam.0 != 0
}

/// Оконная процедура, подменяющая ту, что поставил `tray-icon`.
///
/// Обрабатывает только два сообщения и оба коротко:
/// - `WM_QUERYENDSESSION` — отвечает TRUE немедленно: ветировать выход из
///   системы не наше дело, а `DefWindowProcW` по умолчанию отвечает тем же,
///   так что достающая до оригинальной процедуры дорога здесь не нужна;
/// - `WM_ENDSESSION` — восстанавливает системный прокси, если сеанс
///   действительно завершается (`session_is_ending`). Windows даёт на эту
///   процедуру ограниченный бюджет времени и убивает процесс по истечении
///   без дальнейших вопросов, поэтому `proxy::restore` обязана быть
///   короткой и не блокировать — она и так делает не больше, чем на обычном
///   выходе (пара синхронных обращений к реестру), и `ORIGINAL.take()`
///   внутри нее гарантирует, что повторный вызов при обычном `Drop` после
///   этого — не более чем no-op, а не двойная запись.
///
/// Всё остальное уходит в сохранённую прежнюю процедуру `tray-icon`
/// (`CallWindowProcW`) — как обычные сообщения самой иконки и меню, так и
/// системные вроде `WM_CREATE`/`WM_NCCREATE`, без которых окно не будет
/// готово принять новую процедуру вовремя.
unsafe extern "system" fn session_end_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION if session_is_ending(wparam) => crate::proxy::restore(),
        _ => {}
    }

    let prev = PREV_WNDPROC.load(Ordering::Acquire);
    if prev != 0 {
        // SAFETY: `prev` — значение, которое сама система вернула из
        // `SetWindowLongPtrW(GWLP_WNDPROC, ...)` как действующую на тот
        // момент процедуру окна; по контракту `GWLP_WNDPROC` это указатель
        // на функцию нужной сигнатуры (`WNDPROC`).
        let prev_proc: WNDPROC = std::mem::transmute(prev);
        return CallWindowProcW(prev_proc, hwnd, msg, wparam, lparam);
    }
    // SAFETY: обычные аргументы оконной процедуры, переданные без изменений.
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn make_icon(kind: IconKind) -> Result<Icon, String> {
    Icon::from_rgba(rgba(kind), ICON_SIDE, ICON_SIDE)
        .map_err(|e| format!("иконка {kind:?} не собралась: {e}"))
}

/// Закрывает буфер обмена на любом выходе, включая ошибочные пути и панику.
/// Открытый чужим процессом буфер обмена — глобальная поломка: остальные
/// приложения перестают копировать вообще.
struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: страж создаётся только после успешного `OpenClipboard` на
        // этом же потоке и разрушается ровно один раз.
        let _ = unsafe { CloseClipboard() };
    }
}

fn copy_to_clipboard(text: &str) -> Result<(), WinError> {
    // UTF-16 с завершающим нулём — формат CF_UNICODETEXT.
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = std::mem::size_of_val(&utf16[..]);

    // SAFETY: окно-владельца не передаём (буфер обмена берёт текущий поток);
    // при отказе страж не создаётся и закрывать нечего.
    unsafe { OpenClipboard(HWND::default()) }?;
    let _guard = ClipboardGuard;

    // SAFETY: буфер обмена открыт нами и ещё не закрыт (см. страж).
    unsafe { EmptyClipboard() }?;

    // SAFETY: GMEM_MOVEABLE обязателен — SetClipboardData принимает только
    // перемещаемые блоки; размер ненулевой (строка всегда содержит хотя бы
    // завершающий ноль).
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }?;

    // SAFETY: `handle` только что выделен и принадлежит нам.
    let ptr = unsafe { GlobalLock(handle) };
    if ptr.is_null() {
        return Err(WinError::from_win32());
    }
    // SAFETY: блок выделен ровно на `bytes` байт, источник — живой срез той
    // же длины, области не пересекаются (память только что выделена).
    unsafe { std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr as *mut u16, utf16.len()) };
    // SAFETY: парный вызов к GlobalLock выше. Отказ здесь означает лишь
    // «счётчик блокировок дошёл до нуля» — это и есть нужный исход.
    let _ = unsafe { GlobalUnlock(handle) };

    // SAFETY: формат соответствует содержимому (UTF-16 с нулём), а владение
    // блоком с этого момента переходит к системе — освобождать его нам
    // больше нельзя, и мы этого не делаем.
    unsafe { SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(handle.0)) }?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::mode::{Health, Mode, Place, Reachability, Route};

    fn state(route: Route, demoted: bool) -> AppState {
        AppState {
            mode: Mode::Auto,
            route,
            demoted,
            place: Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health: Health {
                socks: Reachability::Up,
                http: Reachability::Up,
            },
            port: 3129,
        }
    }

    fn place(in_office: bool, id: Option<&str>, name: Option<&str>) -> Place {
        Place {
            in_office,
            network: id.map(str::to_string),
            network_name: name.map(str::to_string),
        }
    }

    /// «OpenVPN не установлен» — тот же снимок, что и `TunnelSnapshot::default()`
    /// (все поля `false`/`None`), но по имени, а не по умолчанию: тесты,
    /// которым сам туннель безразличен, не должны зависеть от того, что
    /// значит пустое значение каждого поля структуры.
    fn no_tunnel() -> TunnelSnapshot {
        TunnelSnapshot::default()
    }

    #[test]
    fn the_network_line_shows_the_name_and_marks_the_office() {
        // Голый GUID в меню бесполезен: сверять человек будет с тем именем,
        // которое показывает Windows.
        let mut s = state(Route::Direct, false);
        s.place = place(true, Some("{OFFICE}"), Some("OFFICE-WIFI"));
        let t = network_text(&s);
        assert!(t.contains("OFFICE-WIFI"), "получили: {t}");
        assert!(t.contains("офис"), "получили: {t}");
        assert!(!t.contains("{OFFICE}"), "GUID тут не нужен: {t}");
    }

    #[test]
    fn a_network_outside_the_office_is_not_marked_as_one() {
        let mut s = state(Route::Direct, false);
        s.place = place(false, Some("{HOME}"), Some("Домашний Wi-Fi"));
        let t = network_text(&s);
        assert!(t.contains("Домашний Wi-Fi"), "получили: {t}");
        assert!(!t.contains("офис"), "получили: {t}");
    }

    #[test]
    fn a_nameless_network_falls_back_to_its_guid() {
        // Сеть без профиля отдаёт пустое имя. Пустое место после двоеточия
        // выглядит как поломка меню; GUID хотя бы можно сверить.
        let mut s = state(Route::Direct, false);
        s.place = place(false, Some("{NONAME}"), Some(""));
        assert!(
            network_text(&s).contains("{NONAME}"),
            "получили: {}",
            network_text(&s)
        );
    }

    #[test]
    fn without_any_network_the_line_says_so() {
        let mut s = state(Route::Direct, false);
        s.place = place(false, None, None);
        let t = network_text(&s);
        assert!(t.contains("не определена"), "получили: {t}");
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
    fn header_names_the_upstream_it_actually_uses() {
        let h = header_text(&state(Route::Socks("203.0.113.10:9999".into()), false));
        assert!(h.contains("203.0.113.10:9999"), "получили: {h}");
    }

    #[test]
    fn a_mode_that_is_merely_unconfigured_says_so() {
        // «Не задан» чинится настройкой, «недоступен» — сетью. Одинаковая
        // подпись отправила бы человека чинить не то.
        let mut s = state(Route::Direct, false);
        s.health = Health {
            socks: Reachability::Unknown,
            http: Reachability::Down,
        };
        assert_eq!(mode_label(Mode::Socks, &s), "SOCKS5 · не задан");
        assert_eq!(mode_label(Mode::Http, &s), "HTTP · недоступен");
        assert_eq!(mode_label(Mode::Auto, &s), "Авто");
    }

    #[test]
    fn the_bridge_address_is_always_loopback() {
        // Мост принципиально не слушает 0.0.0.0; подпись обязана обещать
        // ровно то, что есть.
        assert_eq!(bridge_address(3129), "127.0.0.1:3129");
    }

    #[test]
    fn wm_endsession_only_means_the_session_is_ending_when_wparam_is_true() {
        // WM_ENDSESSION приходит и тогда, когда другое приложение
        // ветировало WM_QUERYENDSESSION: сеанс на самом деле продолжается,
        // и в этом случае восстанавливать прокси нельзя — программа
        // работает дальше.
        assert!(session_is_ending(WPARAM(1)));
        assert!(!session_is_ending(WPARAM(0)));
    }

    // Ниже — единственное место в этом файле, где меню строится по-настоящему
    // (`Tray::new`), а не через чистые функции. Всплывающее меню отрисовать
    // здесь нельзя (нет живого клика пользователя), но сама постройка меню и
    // id-based маршрутизация `action_for` — самый обычный Win32-объект
    // (`CreateMenu`/`AppendMenuW`), не требующий видимого окна, поэтому это
    // конструирование безопасно гонять в тесте. Поля `Tray` приватные, но
    // видимы отсюда — этот `mod tests` вложен в тот же модуль.

    #[test]
    fn opening_the_speed_test_and_diagnostics_are_distinct_actions() {
        let t = Tray::new(&state(Route::Direct, false), &no_tunnel())
            .expect("трей строится в этом окружении");
        assert_eq!(
            t.action_for(t.bench.id()),
            Some(Action::OpenBench),
            "пункт «Замерить скорость…» обязан вести на замер"
        );
        assert_eq!(
            t.action_for(t.doctor.id()),
            Some(Action::OpenDoctor),
            "пункт «Диагностика…» обязан вести на диагностику"
        );
        assert_eq!(
            t.action_for(t.open_tunnel.id()),
            Some(Action::OpenTunnel),
            "пункт «Туннель…» обязан вести на раздел туннеля"
        );
        // Разные пункты — разные действия: если бы оба пункта случайно
        // получили один и тот же `MenuId::new()` по ошибке копипаста,
        // это сравнение бы это поймало.
        assert_ne!(t.bench.id(), t.doctor.id());
        assert_ne!(t.bench.id(), t.open_tunnel.id());
        assert_ne!(t.doctor.id(), t.open_tunnel.id());
    }

    #[test]
    fn the_new_items_do_not_disturb_the_rest_of_the_menu() {
        // Меню строилось по двум предыдущим планам: режимы с индикаторами
        // доступности, «Настройки…», копирование адреса и «Выход» обязаны
        // остаться на месте и вести на прежние действия после того, как
        // задача 5, а следом и задача 7, вставили в это же меню новые
        // пункты.
        let t = Tray::new(&state(Route::Direct, false), &no_tunnel())
            .expect("трей строится в этом окружении");
        assert_eq!(t.action_for(t.quit.id()), Some(Action::Quit));
        assert_eq!(t.action_for(t.copy.id()), Some(Action::CopyAddress));
        assert_eq!(t.action_for(t.settings.id()), Some(Action::OpenSettings));
        for (mode, item) in &t.modes {
            assert_eq!(t.action_for(item.id()), Some(Action::SetMode(*mode)));
        }
        // Неизвестный id — не пункт этого меню вовсе, а не случайное совпадение.
        assert_eq!(t.action_for(&MenuId::new("несуществующий-пункт")), None);
    }

    // ---- Задача 7: строка про туннель ----

    #[test]
    fn tunnel_text_explains_a_missing_openvpn() {
        assert!(tunnel_text(&no_tunnel()).contains("не установлен"));
    }

    #[test]
    fn tunnel_text_names_a_down_tunnel() {
        let snap = TunnelSnapshot {
            installed: true,
            profile_installed: true,
            ..Default::default()
        };
        let t = tunnel_text(&snap);
        assert!(t.contains("опущен"), "получили: {t}");
    }

    #[test]
    fn tunnel_text_names_our_tunnel_up() {
        let snap = TunnelSnapshot {
            installed: true,
            profile_installed: true,
            our_tunnel_up: true,
            ..Default::default()
        };
        assert_eq!(tunnel_text(&snap), "Туннель: поднят");
    }

    #[test]
    fn tunnel_text_warns_about_routes_being_occupied() {
        let snap = TunnelSnapshot {
            installed: true,
            foreign_tunnel_up: true,
            ..Default::default()
        };
        let t = tunnel_text(&snap);
        assert!(t.contains("заняты"), "получили: {t}");
    }

    #[test]
    fn tunnel_text_reports_an_unknown_liveness_honestly() {
        // Не выдаёт «поднят»/«опущен» увереннее, чем знает: не сумели
        // прочитать лог OpenVPN GUI — состояние неизвестно, а не «опущен»
        // по умолчанию (тот самый дедлок, который эта же честность и
        // устраняет — молчаливое «опущен» скрыло бы кнопку «опустить»,
        // если туннель на самом деле поднят).
        let snap = TunnelSnapshot {
            installed: true,
            liveness_error: Some("тестовый отказ".to_string()),
            ..Default::default()
        };
        let t = tunnel_text(&snap);
        assert!(t.contains("неизвестно"), "получили: {t}");
    }

    #[test]
    fn tunnel_text_does_not_hide_an_unreadable_route_table_as_plain_down() {
        let snap = TunnelSnapshot {
            installed: true,
            profile_installed: true,
            routes_error: Some("тестовый отказ".to_string()),
            ..Default::default()
        };
        let t = tunnel_text(&snap);
        assert!(t.contains("не проверены"), "получили: {t}");
    }

    #[test]
    fn tunnel_text_prefers_confirmed_liveness_over_a_misclassified_foreign_reading() {
        // Регрессия на fix round 1: `our_tunnel_up` (лог, ключ — имя
        // профиля) обязана перевешивать `foreign_tunnel_up` (таблица
        // маршрутов + ненадёжный алиас адаптера) — иначе меню трея и
        // страница настроек разошлись бы в самый важный момент: страница
        // уже покажет кнопку «опустить», а трей — «обнаружен чужой».
        let snap = TunnelSnapshot {
            installed: true,
            profile_installed: true,
            our_tunnel_up: true,
            foreign_tunnel_up: true,
            ..Default::default()
        };
        assert_eq!(tunnel_text(&snap), "Туннель: поднят");
    }

    #[test]
    fn tunnel_text_names_the_rising_window_distinctly_from_down() {
        // Round 2: лог уже подтвердил успех, маршруты профиля ещё не
        // встали — не должно читаться как «опущен» (приглашение нажать
        // «Поднять» ещё раз).
        let snap = TunnelSnapshot {
            installed: true,
            profile_installed: true,
            rising: true,
            ..Default::default()
        };
        assert_eq!(tunnel_text(&snap), "Туннель: поднимается…");
    }

    #[test]
    fn tunnel_text_prefers_rising_over_a_misclassified_occupied_reading() {
        let snap = TunnelSnapshot {
            installed: true,
            profile_installed: true,
            rising: true,
            foreign_tunnel_up: true,
            ..Default::default()
        };
        assert_eq!(tunnel_text(&snap), "Туннель: поднимается…");
    }
}
