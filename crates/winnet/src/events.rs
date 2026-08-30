//! События смены сети.
//!
//! NLM отдаёт их через точку подключения: создать NetworkListManager,
//! запросить IConnectionPointContainer, найти точку по IID
//! INetworkListManagerEvents, вызвать Advise с приёмником.
//!
//! Каналов два, и оба поднимаются всегда: NLM видит агрегат связности
//! машины, IP Helper — появление и пропажу интерфейсов. Ни один из них не
//! покрывает другого, а пачку дублей на одну физическую смену схлопывает
//! `debounce`. Подробности и обоснование — в `watcher_thread`.
//!
//! Поток, сделавший Advise, обязан крутить цикл сообщений: COM доставляет
//! события апартаментного объекта оконными сообщениями, и без цикла приёмник
//! просто не вызовется. Отсюда выделенный поток и канал наружу.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{channel, Receiver, Sender, WeakSender};
use tracing::{debug, info, warn};
use windows::core::{implement, Error as WinError, Interface, HRESULT};
use windows::Win32::Foundation::{BOOLEAN, E_FAIL, HANDLE, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, MibAddInstance, MibDeleteInstance, NotifyIpInterfaceChange,
    MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE,
};
use windows::Win32::Networking::NetworkListManager::{
    INetworkListManagerEvents, INetworkListManagerEvents_Impl, NetworkListManager, NLM_CONNECTIVITY,
};
use windows::Win32::Networking::WinSock::AF_UNSPEC;
use windows::Win32::System::Com::{
    CoCreateInstance, IConnectionPoint, IConnectionPointContainer, CLSCTX_ALL,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP,
    WM_USER,
};

use crate::com::ComGuard;
use crate::WinNetError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkChange {
    Connectivity,
    NetworkAdded,
    NetworkPropertyChanged,
}

/// Ёмкость канала событий. Больше и не нужно: потребитель пересчитывает
/// решение целиком, и очередь из восьми «пересчитай» — уже перебор.
const EVENT_CAPACITY: usize = 8;

/// Просьба потоку подписки выйти из цикла сообщений. `WM_APP` — диапазон,
/// который система обещает не занимать своими сообщениями.
const WM_STOP_WATCHER: u32 = WM_APP + 1;

/// Схлопывает пачку событий в передний и задний фронт.
///
/// Первое событие проходит сразу — реагировать надо быстро. Всё, что пришло
/// в течение окна после него, схлопывается в одно, и это одно уходит наружу
/// по закрытии окна.
///
/// Задний фронт обязателен. Потребитель пересчитывает решение целиком, но
/// читает он сеть в момент пересчёта, а не в момент конца пачки: одно
/// переключение Wi-Fi даёт события и в середине ассоциации, и в её конце, и
/// без заднего фронта решение осталось бы посчитанным по недоустоявшейся
/// сети до самой следующей физической смены. Худший случай — две строки на
/// пачку вместо одной; это не тот шум, ради которого схлопывание затевалось.
pub fn debounce(mut rx: Receiver<NetworkChange>, window: Duration) -> Receiver<NetworkChange> {
    let (tx, out) = channel(EVENT_CAPACITY);
    tokio::spawn(async move {
        loop {
            // Ждём событие, но не забываем следить за собственным выходом:
            // без этого выброшенный потребителем приёмник заметился бы
            // только на следующем событии, а до тех пор `rx` оставался бы
            // жив и держал подписку (см. `watch_network_changes`).
            let first = tokio::select! {
                ev = rx.recv() => match ev {
                    Some(ev) => ev,
                    None => return,
                },
                _ = tx.closed() => return,
            };
            if tx.send(first).await.is_err() {
                return;
            }

            // Дожёвываем хвост пачки, запоминая последнее событие.
            let deadline = tokio::time::Instant::now() + window;
            let mut trailing = None;
            let mut source_closed = false;
            loop {
                let received = tokio::select! {
                    r = tokio::time::timeout_at(deadline, rx.recv()) => r,
                    _ = tx.closed() => return,
                };
                match received {
                    Ok(Some(ev)) => trailing = Some(ev),
                    Ok(None) => {
                        source_closed = true;
                        break;
                    }
                    Err(_) => break,
                }
            }

            if let Some(ev) = trailing {
                if tx.send(ev).await.is_err() {
                    return;
                }
            }
            if source_closed {
                return;
            }
        }
    });
    out
}

/// Подписывается на смену сети и отдаёт канал событий.
///
/// Подписка живёт на выделенном потоке (см. модульный комментарий) ровно до
/// тех пор, пока жив возвращённый приёмник: когда потребитель его выбросит,
/// поток снимет `Advise` и выйдет из цикла сообщений. `debounce` этот
/// договор не рвёт — его задача тоже следит за своим выходом и отпускает
/// приёмник сразу, не дожидаясь очередного события.
///
/// Обратное тоже верно: если поток подписки завершится сам, канал
/// закроется и `recv()` вернёт `None`, а не будет молчать вечно.
///
/// Функция синхронная и на короткое время блокируется, дожидаясь от потока
/// ответа, поднялась ли подписка: `CoCreateInstance` + `Advise` — это
/// миллисекунды, а иначе про отказ пришлось бы узнавать по молчанию канала.
pub fn watch_network_changes() -> Result<Receiver<NetworkChange>, WinNetError> {
    let (tx, rx) = channel(EVENT_CAPACITY);
    let runtime = tokio::runtime::Handle::try_current().ok();

    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<Arc<Pump>, WinNetError>>(1);
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    // Сторож держит сильный отправитель: только на нём можно дождаться
    // `closed()`. Поэтому он обязан умереть вместе с потоком — иначе канал
    // остался бы открытым, и потребитель никогда не узнал бы, что событий
    // больше не будет.
    let probe = tx.clone();

    let joiner = std::thread::Builder::new()
        .name("proxypilot-netwatch".to_owned())
        .spawn(move || watcher_thread(tx, ready_tx, done_tx))
        .map_err(|e| fail(&format!("не удалось создать поток подписки: {e}")))?;

    let pump = ready_rx
        .recv()
        .map_err(|_| fail("поток подписки завершился, не сообщив о результате"))??;

    match runtime {
        Some(handle) => {
            handle.spawn(async move {
                // Пока дескриптор потока открыт, ядро не переиспользует его
                // идентификатор — а `post_stop` адресуется именно по
                // идентификатору. Поэтому отпускаем дескриптор последним.
                let _joiner = joiner;
                tokio::select! {
                    _ = probe.closed() => post_stop(&pump),
                    _ = done_rx => {}
                }
                drop(probe);
            });
        }
        None => {
            // Не отказ: подписка работает и без сторожа, просто разбор
            // случится по первому событию на закрытом канале.
            debug!("рантайм tokio недоступен, сторож закрытия канала не заведён");
        }
    }

    Ok(rx)
}

/// Какие каналы событий в итоге подняты — одной строкой для лога.
///
/// Вынесено в чистую функцию ради теста: разобрать по логу, работает ли
/// приложение как задумано или на одном канале из двух, — единственный
/// способ, доступный оператору, и подпись обязана различать все случаи.
fn source_label(nlm: bool, iphelper: bool) -> &'static str {
    match (nlm, iphelper) {
        (true, true) => "nlm+iphelper",
        (true, false) => "nlm",
        (false, true) => "iphelper",
        // До сюда не доходит: случай «оба отказали» разобран возвратом
        // выше. Отдельная подпись вместо `unreachable!` — потому что
        // паниковать на потоке подписки нельзя ни при каких обстоятельствах.
        (false, false) => "нет",
    }
}

/// Ошибка без кода Windows: `WinNetError` умеет только их, а заводить ради
/// трёх случаев отдельный вариант — плодить сущности. `E_FAIL` с внятным
/// текстом даёт ровно ту же диагностику в логе.
fn fail(message: &str) -> WinNetError {
    WinNetError::Windows(WinError::new(E_FAIL, message))
}

/// Состояние потока подписки, общее для приёмника, запасного канала и
/// сторожа.
struct Pump {
    thread_id: u32,
    /// `true`, пока поток крутит цикл сообщений. Windows переиспользует
    /// идентификаторы потоков, и сообщение, посланное по идентификатору
    /// умершего потока, может достаться чужому — в этом же процессе скоро
    /// заведётся поток трея со своим циклом сообщений.
    alive: AtomicBool,
}

/// Как именно закончился цикл сообщений.
enum PumpExit {
    /// Нас попросили остановиться — штатный путь.
    Stopped,
    /// В очередь попал `WM_QUIT`. Мы его не шлём, значит прислал кто-то ещё.
    Quit,
    /// `GetMessageW` отказала.
    Failed(WinError),
}

/// Тело выделенного потока: поднять подписку, отчитаться и крутить цикл
/// сообщений до просьбы остановиться.
fn watcher_thread(
    tx: Sender<NetworkChange>,
    ready: std::sync::mpsc::SyncSender<Result<Arc<Pump>, WinNetError>>,
    done: tokio::sync::oneshot::Sender<()>,
) {
    // Свой поток — свой апартамент: `ComGuard` привязан к потоку вызова, и
    // страж вызывающей стороны сюда не годится. Держим его живым до самого
    // конца функции: точка подключения обязана умереть раньше апартамента.
    let com = ComGuard::new();
    let pump = Arc::new(Pump {
        thread_id: current_thread_id(),
        alive: AtomicBool::new(true),
    });

    // Точку подключения и куку держим здесь, чтобы `Unadvise` случился в
    // конце этой же функции, на том же потоке, где был `Advise`.
    let mut nlm: Option<(IConnectionPoint, u32)> = None;
    let mut iphelper: Option<IpHelperSubscription> = None;

    // ОБА канала поднимаются вместе, а не «IP Helper только если NLM не
    // завёлся».
    //
    // Почему: у `INetworkListManagerEvents` ровно одно событие —
    // `ConnectivityChanged`, и это машинный АГРЕГАТ связности. Смены, не
    // двигающие агрегат, обратного вызова не дают вовсе: док-станция при
    // живом Wi-Fi с интернетом, смена категории сети Public↔Domain,
    // появление второй сети. А это ровно те переходы, которые обязана
    // увидеть `Config::place_for`: офисная сеть появляется и пропадает, не
    // меняя уровня связности машины.
    //
    // `NotifyIpInterfaceChange` срабатывает на добавление и удаление
    // интерфейса и закрывает эти случаи. Дубли, которые два канала дадут на
    // одну физическую смену, схлопывает `debounce` — он для того и написан.
    let mut nlm_err = None;
    match &com {
        Ok(_) => match subscribe_nlm(&tx, &pump) {
            Ok(sub) => nlm = Some(sub),
            Err(e) => nlm_err = Some(e),
        },
        Err(e) => nlm_err = Some(fail(&format!("апартамент COM не поднялся: {e}"))),
    }

    let mut ip_err = None;
    match subscribe_ip_helper(&tx, &pump) {
        Ok(sub) => iphelper = Some(sub),
        Err(e) => ip_err = Some(e),
    }

    // Молча деградировать нельзя: без этих строк человек не узнает, почему
    // смена сети замечается грубее, чем должна. Формулировки разные
    // нарочно — по логу обязано быть видно, какой именно канал выпал.
    match (&nlm_err, &ip_err) {
        (None, None) => {}
        (Some(e), None) => warn!(
            error = %e,
            "подписка на события NLM не поднялась: смена сети замечается только по NotifyIpInterfaceChange"
        ),
        (None, Some(e)) => warn!(
            error = %e,
            "NotifyIpInterfaceChange не поднялся: смена сети замечается только по агрегату связности NLM"
        ),
        (Some(n), Some(i)) => {
            warn!(nlm = %n, iphelper = %i, "ни один канал событий смены сети не поднялся");
            pump.alive.store(false, Ordering::Release);
            let _ = ready.send(Err(fail(&format!(
                "подписка на смену сети не поднялась: NLM: {n}; IP Helper: {i}"
            ))));
            let _ = done.send(());
            return;
        }
    }

    // Очередь сообщений у потока появляется лениво — при первом обращении к
    // user32. Создаём её до того, как отдадим наружу свой идентификатор,
    // иначе `PostThreadMessageW` может прилететь в поток без очереди и
    // пропасть вместе с просьбой остановиться.
    force_message_queue();

    let source = source_label(nlm.is_some(), iphelper.is_some());

    if ready.send(Ok(pump.clone())).is_err() {
        // Вызывающая сторона исчезла, не дождавшись ответа — разбирать
        // подписку незачем откладывать.
        teardown(nlm, iphelper);
        pump.alive.store(false, Ordering::Release);
        let _ = done.send(());
        return;
    }
    // `info`, а не `debug`: уровень лога по умолчанию — info (см.
    // `bridge::log`), а строка пишется один раз за жизнь процесса. Без неё
    // «оба канала подняты, как задумано» и «остался один» выглядят в логе
    // одинаково — молчанием.
    info!(
        thread = pump.thread_id,
        source, "подписка на события сети поднята"
    );

    let exit = pump_messages();

    teardown(nlm, iphelper);
    pump.alive.store(false, Ordering::Release);
    drop(com);

    match exit {
        PumpExit::Stopped => debug!(thread = pump.thread_id, "подписка на события сети снята"),
        // Оба следующих случая означают, что смены сети больше не
        // замечаются. Уронить это в debug значило бы деградировать молча —
        // ровно то, что запрещено на этапе подъёма подписки.
        PumpExit::Quit => warn!(
            thread = pump.thread_id,
            "цикл сообщений получил WM_QUIT со стороны: подписка на смену сети прекращена"
        ),
        PumpExit::Failed(e) => warn!(
            thread = pump.thread_id,
            error = %e,
            "GetMessageW отказала, подписка на смену сети прекращена"
        ),
    }

    // Последними отпускаем отправитель и будим сторожа: с этого момента
    // потребитель увидит конец канала, а не бесконечную тишину.
    drop(tx);
    let _ = done.send(());
}

fn teardown(nlm: Option<(IConnectionPoint, u32)>, iphelper: Option<IpHelperSubscription>) {
    if let Some((point, cookie)) = nlm {
        // SAFETY: `cookie` получена от `Advise` на этой же точке подключения
        // и на этом же потоке, и используется ровно один раз. Ошибку только
        // логируем: разбирать подписку всё равно больше нечем.
        if let Err(e) = unsafe { point.Unadvise(cookie) } {
            warn!(error = %e, "Unadvise не удался");
        }
    }
    drop(iphelper);
}

fn current_thread_id() -> u32 {
    // SAFETY: `GetCurrentThreadId` не принимает аргументов, не может отказать
    // и не трогает память.
    unsafe { GetCurrentThreadId() }
}

/// Просит поток подписки выйти из цикла сообщений.
///
/// Проверка `alive` — не оптимизация: идентификаторы потоков Windows
/// переиспользует, и после смерти потока тот же номер может достаться
/// чужому потоку этого же процесса. Пока сторож держит `JoinHandle`,
/// идентификатор к тому же зарезервирован ядром.
fn post_stop(pump: &Pump) {
    if !pump.alive.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: `PostThreadMessageW` проверяет идентификатор потока сама и при
    // несуществующем потоке возвращает ошибку, а не портит память. Ошибка
    // здесь означает «поток уже вышел» — ровно то, чего мы и добивались.
    let _ = unsafe { PostThreadMessageW(pump.thread_id, WM_STOP_WATCHER, WPARAM(0), LPARAM(0)) };
}

fn force_message_queue() {
    let mut msg = MSG::default();
    // SAFETY: `msg` — живая инициализированная структура нужного типа;
    // `PM_NOREMOVE` ничего из очереди не забирает, вызов нужен только ради
    // побочного эффекта — создания очереди сообщений у потока.
    let _ = unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) };
}

/// Цикл сообщений. Без него COM не доставит ни одного апартаментного
/// вызова, и приёмник не будет вызван ни разу.
fn pump_messages() -> PumpExit {
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` — живая структура нужного типа; `None` в качестве
        // окна означает «все сообщения потока», включая присланные
        // `PostThreadMessageW`.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 == -1 {
            // Код ошибки снимаем немедленно, пока его не затёр следующий
            // вызов Windows.
            return PumpExit::Failed(WinError::from_win32());
        }
        if got.0 == 0 {
            return PumpExit::Quit;
        }
        if msg.message == WM_STOP_WATCHER {
            return PumpExit::Stopped;
        }
        // `TranslateMessage` не зовём сознательно: у потока нет окон с
        // вводом, а COM нужна только доставка в оконную процедуру.
        // SAFETY: `msg` заполнена успешным `GetMessageW` выше и жива до
        // конца итерации.
        unsafe { DispatchMessageW(&msg) };
    }
}

/// Кладёт событие в канал, ничего не блокируя.
///
/// Отправитель слабый: сильный клон в приёмнике или в утёкшем контексте
/// запасного канала держал бы канал открытым и после смерти потока, и
/// потребитель никогда бы не узнал, что события кончились.
fn offer(tx: &WeakSender<NetworkChange>, pump: &Pump, change: NetworkChange) {
    let Some(tx) = tx.upgrade() else {
        // Канала больше нет — подписка никому не нужна.
        post_stop(pump);
        return;
    };
    match tx.try_send(change) {
        Ok(()) => {}
        // Канал полон: потребитель ещё не разгрёб прошлое событие.
        // Терять новое безопасно — решение всё равно пересчитывается
        // целиком, и один пересчёт покроет обе смены.
        Err(TrySendError::Full(_)) => {}
        // Приёмник выброшен: подписка больше никому не нужна.
        Err(TrySendError::Closed(_)) => post_stop(pump),
    }
}

/// Приёмник событий NLM.
///
/// Всё, что он делает, — кладёт событие в канал неблокирующе. Блокировать
/// COM-обратный вызов нельзя: он исполняется внутри диспетчеризации
/// апартамента, и остановка здесь останавливает доставку всему потоку.
#[implement(INetworkListManagerEvents)]
struct NlmSink {
    tx: WeakSender<NetworkChange>,
    pump: Arc<Pump>,
}

impl INetworkListManagerEvents_Impl for NlmSink_Impl {
    fn ConnectivityChanged(&self, _newconnectivity: NLM_CONNECTIVITY) -> windows::core::Result<()> {
        offer(&self.tx, &self.pump, NetworkChange::Connectivity);
        Ok(())
    }
}

/// Классический танец точек подключения. Возвращает саму точку и куку —
/// снимать подписку положено ими обеими.
fn subscribe_nlm(
    tx: &Sender<NetworkChange>,
    pump: &Arc<Pump>,
) -> Result<(IConnectionPoint, u32), WinNetError> {
    // SAFETY: поток уже в апартаменте (`ComGuard` создан вызывающей
    // функцией и жив до конца её тела), CLSID и тип интерфейса
    // соответствуют друг другу.
    let manager: IConnectionPointContainer =
        unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)? };

    // SAFETY: `manager` — живой COM-указатель, IID указывает на статическую
    // константу, живущую всю программу.
    let point = unsafe { manager.FindConnectionPoint(&INetworkListManagerEvents::IID)? };

    let sink: INetworkListManagerEvents = NlmSink {
        tx: tx.downgrade(),
        pump: pump.clone(),
    }
    .into();

    // SAFETY: `point` и `sink` — живые COM-указатели; `Advise` сам берёт
    // ссылку на приёмник, поэтому наш `sink` может умереть сразу после.
    let cookie = unsafe { point.Advise(&sink)? };

    Ok((point, cookie))
}

/// Контекст, который IP Helper отдаёт обратно в обратный вызов.
struct IpHelperContext {
    tx: WeakSender<NetworkChange>,
    pump: Arc<Pump>,
}

/// Живая подписка IP Helper.
struct IpHelperSubscription {
    handle: HANDLE,
}

impl Drop for IpHelperSubscription {
    fn drop(&mut self) {
        // SAFETY: `handle` получен от `NotifyIpInterfaceChange` и до сих пор
        // не отменялся — `Drop` случается ровно один раз.
        let rc = unsafe { CancelMibChangeNotify2(self.handle) };
        if rc.is_err() {
            warn!(code = rc.0, "CancelMibChangeNotify2 не удался");
        }
        // Контекст не освобождаем — см. `subscribe_ip_helper`.
    }
}

/// Второй канал: уведомления IP Helper об изменении IP-интерфейсов.
///
/// Не запасной, а равноправный — поднимается всегда, вместе с NLM (см.
/// `watcher_thread`). Он видит интерфейсы, а не профиль сети, и именно
/// поэтому дополняет NLM, а не дублирует: агрегат связности не двигается,
/// когда рядом с живым Wi-Fi появляется Ethernet док-станции, а интерфейс
/// при этом добавляется. Опроса по таймеру здесь по-прежнему нет.
///
/// Контекст обратного вызова утекает сознательно. Он живёт на куче, а
/// обратный вызов приходит на поток системного пула, и на вопрос «может ли
/// вызов быть в полёте, когда `CancelMibChangeNotify2` уже вернулась»,
/// документация внятного ответа не даёт. Одна маленькая утечка на подписку
/// — не на событие и не на попытку — дешевле, чем гонка с чтением
/// освобождённой памяти. Ничего живого контекст при этом не удерживает:
/// отправитель в нём слабый, а `Pump` — это два поля.
fn subscribe_ip_helper(
    tx: &Sender<NetworkChange>,
    pump: &Arc<Pump>,
) -> Result<IpHelperSubscription, WinNetError> {
    let context = Box::into_raw(Box::new(IpHelperContext {
        tx: tx.downgrade(),
        pump: pump.clone(),
    }));
    let mut handle = HANDLE::default();

    // SAFETY: `context` — валидный указатель на кучу, который никогда не
    // освобождается; `handle` — живая переменная, которую функция
    // заполняет; `BOOLEAN(0)` просит не присылать стартовое уведомление,
    // которое означало бы смену сети на ровном месте.
    let rc = unsafe {
        NotifyIpInterfaceChange(
            AF_UNSPEC,
            Some(on_ip_interface_change),
            Some(context as *const c_void),
            BOOLEAN(0),
            &mut handle,
        )
    };

    if rc.is_err() {
        // SAFETY: подписка не завелась, обратный вызов не придёт никогда,
        // и владельцем указателя остались только мы — здесь освободить
        // контекст можно без всяких гонок.
        drop(unsafe { Box::from_raw(context) });
        return Err(WinNetError::Windows(WinError::from_hresult(
            HRESULT::from_win32(rc.0),
        )));
    }

    Ok(IpHelperSubscription { handle })
}

/// Обратный вызов IP Helper. Приходит на поток системного пула — здесь
/// нельзя ни блокироваться, ни паниковать.
unsafe extern "system" fn on_ip_interface_change(
    callercontext: *const c_void,
    _row: *const MIB_IPINTERFACE_ROW,
    notificationtype: MIB_NOTIFICATION_TYPE,
) {
    if callercontext.is_null() {
        return;
    }
    // SAFETY: указатель — тот самый `context`, отданный в
    // `NotifyIpInterfaceChange`, и он живёт до конца процесса. Именно ради
    // этого разыменования `Drop` у `IpHelperSubscription` его сознательно
    // не освобождает: вызов может оказаться в полёте, когда
    // `CancelMibChangeNotify2` уже вернулась. Ссылка не переживает вызов.
    let ctx = unsafe { &*(callercontext as *const IpHelperContext) };

    // Сравнением, а не `match`: константы windows-rs названы в стиле C, и в
    // образцах на них ругается `non_upper_case_globals`.
    let change = if notificationtype == MibAddInstance {
        NetworkChange::NetworkAdded
    } else if notificationtype == MibDeleteInstance {
        // Пропажа интерфейса — это про связность, а не про свойства сети.
        NetworkChange::Connectivity
    } else {
        NetworkChange::NetworkPropertyChanged
    };

    offer(&ctx.tx, &ctx.pump, change);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_log_line_names_every_combination_of_armed_channels() {
        // Единственный способ, которым оператор отличает «работаем как
        // задумано, оба канала» от «остался один», — эта подпись в логе.
        // Если два случая совпадут, разбор «почему смена сети не заметилась»
        // упрётся в неотличимые строки.
        assert_eq!(source_label(true, true), "nlm+iphelper");
        assert_eq!(source_label(true, false), "nlm");
        assert_eq!(source_label(false, true), "iphelper");
        assert_ne!(source_label(true, true), source_label(false, true));
        assert_ne!(source_label(true, true), source_label(true, false));
    }

    #[tokio::test]
    async fn a_burst_collapses_to_its_first_and_last_event() {
        // Одно переключение Wi-Fi даёт пачку событий. Наружу обязаны уйти
        // передний фронт (реагировать надо быстро) и задний (иначе решение
        // останется посчитанным по недоустоявшейся сети), а середина —
        // схлопнуться.
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut out = debounce(rx, Duration::from_millis(100));

        for _ in 0..5 {
            tx.send(NetworkChange::Connectivity).await.unwrap();
        }
        drop(tx);

        assert!(out.recv().await.is_some(), "первое событие обязано пройти");
        assert!(
            out.recv().await.is_some(),
            "последнее событие пачки обязано пройти"
        );
        assert!(out.recv().await.is_none(), "середина — схлопнуться");
    }

    #[tokio::test]
    async fn events_further_apart_than_the_window_both_pass() {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut out = debounce(rx, Duration::from_millis(50));

        tx.send(NetworkChange::Connectivity).await.unwrap();
        assert!(out.recv().await.is_some());

        tokio::time::sleep(Duration::from_millis(120)).await;
        tx.send(NetworkChange::NetworkPropertyChanged)
            .await
            .unwrap();
        drop(tx);
        assert!(
            out.recv().await.is_some(),
            "после окна событие обязано пройти"
        );
    }

    #[tokio::test]
    async fn the_trailing_event_is_the_last_one_of_the_burst() {
        // Задний фронт обязан описывать ту сеть, на которой машина в итоге
        // оказалась, а не середину переключения.
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut out = debounce(rx, Duration::from_millis(100));

        tx.send(NetworkChange::Connectivity).await.unwrap();
        assert_eq!(out.recv().await, Some(NetworkChange::Connectivity));

        tx.send(NetworkChange::NetworkAdded).await.unwrap();
        tx.send(NetworkChange::NetworkPropertyChanged)
            .await
            .unwrap();
        drop(tx);

        assert_eq!(
            out.recv().await,
            Some(NetworkChange::NetworkPropertyChanged),
            "наружу должно уйти последнее событие пачки"
        );
    }

    #[tokio::test]
    async fn closing_the_source_closes_the_output() {
        let (tx, rx) = tokio::sync::mpsc::channel::<NetworkChange>(1);
        let mut out = debounce(rx, Duration::from_millis(10));
        drop(tx);
        assert!(out.recv().await.is_none());
    }

    #[tokio::test]
    async fn dropping_the_debounced_receiver_releases_the_source() {
        // Договор `watch_network_changes`: подписка живёт ровно столько,
        // сколько живёт приёмник. `debounce` не имеет права его удлинять —
        // иначе выброшенный приёмник заметится только на следующем
        // событии, а его может не быть никогда.
        let (tx, rx) = tokio::sync::mpsc::channel::<NetworkChange>(1);
        let out = debounce(rx, Duration::from_millis(10));
        drop(out);

        // `closed()` разрешится, только когда задача `debounce` отпустит
        // источник. Если бы она ждала события — не разрешился бы никогда.
        tokio::time::timeout(Duration::from_secs(5), tx.closed())
            .await
            .expect("источник обязан освободиться без единого события");
    }

    /// Ручная проверка подписки: только она доказывает, что цикл сообщений
    /// написан верно — приёмник, который компилируется, но не вызывается,
    /// выглядит точно так же, как рабочий.
    ///
    /// В CI не гоняется: нужен живой адаптер и физическое переключение сети.
    /// Запуск:
    /// `cargo test -p proxypilot-winnet -- --ignored --nocapture watch_a_real`
    ///
    /// Каналов теперь два, поэтому в сырой пачке ожидаются события обоих
    /// (`Connectivity` от NLM вперемешку с `NetworkAdded`/
    /// `NetworkPropertyChanged` от IP Helper) — именно это и надо увидеть.
    /// Наружу же после схлопывания обязана уходить по-прежнему пара
    /// «передний фронт + задний», а не по паре на канал.
    #[tokio::test]
    #[ignore = "нужна живая сеть: переключить Wi-Fi руками"]
    async fn watch_a_real_network_change() {
        use tracing::info;

        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();

        let mut raw = watch_network_changes().expect("подписка обязана подняться");

        // Разветвляем поток событий: в лог уходит и сырая пачка, и то, что
        // от неё осталось после схлопывания. Иначе не видно, что именно
        // схлопнулось.
        let (tee_tx, tee_rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            while let Some(ev) = raw.recv().await {
                info!(?ev, "сырое событие NLM");
                if tee_tx.send(ev).await.is_err() {
                    break;
                }
            }
        });
        let mut out = debounce(tee_rx, Duration::from_secs(2));

        info!("ждём смены сети 45 секунд: выключите и включите Wi-Fi");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let mut seen = 0usize;
        while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, out.recv()).await {
            seen += 1;
            info!(?ev, номер = seen, "смена сети после схлопывания");
        }
        info!(всего = seen, "окно наблюдения закрыто");
        assert!(seen > 0, "за 45 секунд не пришло ни одного события");
    }
}
