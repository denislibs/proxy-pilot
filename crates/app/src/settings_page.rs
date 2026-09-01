//! Страница настроек: содержимое.
//!
//! Транспорт (привязка, токен, проверка источника, таймаут бездействия)
//! живёт в [`crate::websrv`] и об этом файле ничего не знает. Здесь — HTML,
//! разбор формы и применение изменений.
//!
//! # Три правила, вокруг которых всё построено
//!
//! **1. Смена порта моста не применяется на лету.** Поле сохраняется в
//! конфиг, но живой конфиг, который получает супервизор, продолжает нести
//! порт, на котором слушатель УЖЕ привязан ([`live_config`]). Тихая
//! перепривязка убила бы то единственное свойство, ради которого продукт
//! переписан, — установленные соединения переживают смену маршрута, — и
//! убила бы его в том самом месте, куда пользователь дотягивается руками.
//! Инвариант записан в докблоке `supervisor.rs`; здесь он выполняется.
//!
//! **2. Единственный путь в супервизор — канал [`crate::Cmd`].** Тот же, что
//! у трея и у подписки на смену сети. Второй писатель в `Router` в обход
//! канала был бы молча затёрт следующим пересчётом, а разошедшиеся пути
//! пересчёта — это два места, где решение может разъехаться.
//!
//! **3. Правила проверки — только `Config::validate`.** Не вторая копия
//! правил в JavaScript: две копии расходятся, и расходится обычно та, что в
//! браузере. Страница показывает ту ошибку, которую вернул сервер, дословно.
//!
//! # Почему на странице нет ни строчки скрипта
//!
//! Кнопка «эта сеть — офис» напрашивалась на `onclick`, который подставил бы
//! GUID в поле. Она сделана обычной кнопкой отправки формы: сервер и так
//! знает текущую сеть из `AppState.place`, а страница без скрипта не требует
//! ослаблять `Content-Security-Policy` до `script-src 'unsafe-inline'` —
//! то есть ровно до той дырки, через которую любое пропущенное экранирование
//! превратилось бы в выполнение чужого кода на странице, которая правит
//! настройки прокси.
//!
//! Поэтому же всё, что попадает в разметку, проходит через [`escape_html`]:
//! имя сети приходит из системы (её показывает Windows, а задал её тот, кто
//! поднял точку доступа), адрес апстрима и bypass-список — из файла, который
//! правят руками. Ни одно из этих значений не наше.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use proxypilot_bridge::bench::{bench_all, fastest, BenchResult};
use proxypilot_bridge::supervisor::AppState;
use proxypilot_core::config::{Config, OfficeNetwork};
use proxypilot_core::mode::{Mode, Reachability, Route};
use proxypilot_core::net::Ipv4Net;
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

use crate::doctor::{self, Check, CheckStatus};
use crate::Cmd;

/// Что качаем при замере и сколько.
///
/// Короткий файл и один поток — намеренно (см. заголовок `bench.rs`): цифры
/// сравнивают маршруты между собой, а не измеряют канал.
///
/// Арифметика таймаута не случайна: маршрутов максимум три (напрямую, SOCKS5,
/// HTTP), они меряются по очереди, и `3 с × 3 = 9 с` обязаны уместиться в
/// `websrv::REQUEST_TIMEOUT` (15 с) вместе с отрисовкой страницы. Увеличивать
/// эту константу без оглядки на ту нельзя: замер молча оборвался бы на
/// полпути, и последний маршрут выглядел бы мёртвым.
const BENCH_URL: &str = "http://cachefly.cachefly.net/1mb.test";
const BENCH_LIMIT: u64 = 1024 * 1024;
const BENCH_TIMEOUT: Duration = Duration::from_secs(3);

/// Сколько ждём, пока супервизор применит изменение и отчитается.
///
/// Пересчёт включает пробы обоих апстримов, а `Prober` даёт каждой свой
/// таймаут набора; ждать вечно нельзя — на том конце браузер, а перед ним
/// человек, нажавший «Сохранить».
const APPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Сколько ждём ответа от собственного порта при живой диагностике.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Имя нашего профиля в OpenVPN — то же значение, что уходит и в имя файла
/// (`<name>.ovpn`, `winnet::openvpn::install_profile`), и в
/// `--command connect|disconnect <name>` (`docs/design.md` §8.3), и в
/// `winnet::tunnel_log::liveness(name)` — имя файла лога `<name>.log`,
/// который сам `openvpn-gui.exe` ведёт для этого подключения.
///
/// **Fix round 1, задача 7.** Первая версия этой константы подставлялась
/// ещё и как `our_alias` в `tunnel_state::{our_tunnel_up,
/// foreign_tunnel_up}` — псевдоним сетевого АДАПТЕРА. Это было неверно по
/// факту, не только по осторожности: реальные адаптеры OpenVPN на Windows
/// называются по драйверу («OpenVPN Wintun», «TAP-Windows Adapter V9»), а
/// не по имени соединения, и ни один из них никогда не совпал бы со
/// строкой ниже — подтверждено чтением `Get-NetAdapter` на живой машине,
/// не по памяти. Из-за этого `our_tunnel_up` не срабатывала никогда, а
/// `foreign_tunnel_up` классифицировала НАШ ЖЕ поднятый туннель как
/// чужой — правило «не трогать чужой туннель» запрещало и подъём, и
/// опускание разом, то есть ровно тот дедлок, ради устранения которого
/// задача 3 и писала `same_alias`. Живость теперь определяет
/// `winnet::tunnel_log` по логу, ключуемому этим же именем — тем, чем мы
/// на самом деле владеем, — а не по имени адаптера, которым не владеем
/// (см. докблок `tunnel_log`).
///
/// **Fix round 2, задача 7.** Round 1 оставил имя адаптера работать
/// наполовину — в `foreign_tunnel_up`, пока `our_tunnel_up == false`.
/// Ревью заметило: раз оно доказанно ничего не отличает, держать его
/// хоть где-то — держать то же неверное основание. Убрано отовсюду:
/// `tunnel_state::any_tunnel_carries` больше не видит и не принимает
/// никакого алиаса, а `our_tunnel_up`/`foreign_tunnel_up` строятся у
/// вызывающего (`WinTunnel::snapshot`, `main.rs`) из связки «что говорит
/// лог» + «несёт ли хоть один туннельный адаптер наши подсети», без
/// участия имени адаптера вовсе.
pub const TUNNEL_PROFILE_NAME: &str = "proxypilot-office";

/// Доступ к OpenVPN-туннелю и к установке службы статического IP с точки
/// зрения страницы настроек.
///
/// Реальная реализация (`WinTunnel` в `main.rs`) вызывает
/// `proxypilot_winnet::{openvpn, routes, tunnel_log, tunnel_state}` и
/// `ShellExecuteW` с verb `runas` для установки службы. Тестовая
/// (`FakeTunnel`, ниже) отдаёт заранее заданный снимок и ничего не
/// трогает — ни диск, ни реестр, ни `openvpn-gui.exe`. Абстракция
/// обязательна, не по вкусу: если бы тесты этой страницы звали настоящую
/// реализацию, `cargo test` на машине, где OpenVPN установлен (а он
/// установлен на машине, где велась эта сессия — см. отчёт задачи 1),
/// реально запускал бы `openvpn-gui.exe --command connect`, писал файлы в
/// каталог конфигураций OpenVPN и мог бы попытаться повысить права —
/// ровно то, что `CLAUDE.md` запрещает агенту делать на этой машине.
pub trait Tunnel: Send + Sync {
    /// Снимок состояния — только чтение (реестр, файловая система, лог
    /// `openvpn-gui.exe`, живая таблица маршрутов), безопасно вызывать
    /// когда угодно и как угодно часто.
    fn snapshot(&self, office_subnets: &[Ipv4Net], profile_name: &str) -> TunnelSnapshot;
    /// Собирает split-tunnel профиль (`ovpn_profile::build_profile`,
    /// ошибка которого не проглатывается — доходит до пользователя как
    /// есть) и кладёт его в каталог конфигураций OpenVPN
    /// (`openvpn::build_and_install_profile` →
    /// `openvpn::Installation::user_config_dir`). Прав администратора не
    /// требует — но НЕ потому, что каталог конфигураций OpenVPN в целом
    /// доступен на запись обычному пользователю: это неверно для
    /// СИСТЕМНОГО каталога (`Installation::config_dir`, на обычной
    /// установке — под `Program Files`, запись туда отказывает access
    /// denied без прав администратора, проверено на живой машине). Запись
    /// идёт в ДРУГОЙ каталог — `Installation::user_config_dir`
    /// (`%USERPROFILE%\OpenVPN\config`) — именно туда сам OpenVPN GUI и
    /// сохраняет профили без UAC; он же показывает профили из обоих
    /// каталогов разом, поэтому наш профиль в пользовательском каталоге
    /// виден в GUI наравне с системными.
    fn build_profile(&self, profile_name: &str, office_subnets: &[Ipv4Net]) -> Result<(), String>;
    /// `Ok` значит только «команда передана `openvpn-gui.exe`», не «туннель
    /// поднят» — тот же контракт, что у `openvpn::connect`.
    fn raise(&self, profile_name: &str) -> Result<(), String>;
    fn lower(&self, profile_name: &str) -> Result<(), String>;
    /// Запускает `<этот же .exe> install-service` с запросом повышения прав
    /// (`ShellExecuteW`, verb `runas`) — единственный путь к UAC во всём
    /// приложении (`CLAUDE.md`, «Права администратора»). `Ok` значит только
    /// «запрос ушёл в Windows», не «служба установлена»: принять/отклонить
    /// диалог UAC и саму регистрацию видит уже не эта функция.
    fn install_service(&self) -> Result<(), String>;
}

/// Что странице нужно знать об OpenVPN-туннеле для отрисовки. Не то же
/// самое, что `openvpn::ProfileStatus` — тот отвечает «есть ли файл
/// профиля на диске», а не «поднят ли туннель» (докблок `ProfileStatus`);
/// здесь оба факта разложены по разным полям намеренно, чтобы не повторить
/// ту же путаницу, ради которой `ProfileStatus` был переименован из
/// `TunnelStatus`.
///
/// Два независимых источника, две независимые ошибки. `our_tunnel_up`
/// (лог `openvpn-gui.exe`, ключ — имя профиля, которым мы владеем) и
/// `foreign_tunnel_up` (таблица маршрутов + псевдоним адаптера, которым
/// мы НЕ владеем — задача 3 и fix round 1 задачи 7) отвечают на разные
/// вопросы и могут отказать порознь. `foreign_tunnel_up` вдобавок
/// осмысленна ТОЛЬКО пока `our_tunnel_up == false` — устройство приоритета
/// у [`tunnel_section`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunnelSnapshot {
    /// OpenVPN не найден на машине вовсе — прочие поля смысла не имеют,
    /// раздел неактивен целиком.
    pub installed: bool,
    /// Файл `<profile_name>.ovpn` есть в каталоге конфигураций OpenVPN.
    pub profile_installed: bool,
    /// **Round 2 задачи 7.** Требует подтверждения ОБОИХ независимых
    /// источников разом: `winnet::tunnel_log::liveness(profile_name) ==
    /// Up` (лог `openvpn-gui.exe`, ключ — имя профиля) И
    /// `tunnel_state::any_tunnel_carries` (хоть один туннельный адаптер
    /// несёт наши офисные подсети — без всякой привязки к тому, чей это
    /// адаптер: round 1 доказал, что имя адаптера никогда не совпадает с
    /// именем профиля, и round 2 убрал эту привязку отовсюду). Один лог
    /// был недостаточен: `openvpn.exe`, убитый без штатного выхода
    /// (`taskkill /F`), продолжал бы врать «поднято» сколько угодно —
    /// маршруты в этот момент уже исчезли и гасят ложный `true` (докблок
    /// `winnet::tunnel_log`, «Честные пределы»).
    pub our_tunnel_up: bool,
    /// Лог подтвердил успешное подключение, но ни один адаптер ещё не
    /// несёт наши подсети — короткое окно сразу после «Поднять туннель»,
    /// пока `route ...` из профиля не встали. Не «опущен» (это выглядело
    /// бы как приглашение нажать «Поднять» ещё раз) — отдельное честное
    /// состояние.
    pub rising: bool,
    /// Не удалось прочитать лог, чтобы решить `our_tunnel_up` честно, ЛИБО
    /// лог сказал «поднято», а таблицу маршрутов прочитать не удалось,
    /// чтобы это подтвердить (round 2: логу в одиночку теперь не верим —
    /// значит и в этом случае ответ «не знаю», не «поднято на слово
    /// лога»). `Some` не гасит кнопки, а замещает их предупреждением:
    /// раздел, признающий «не знаю», лучше раздела, который молча
    /// блокирует обе кнопки (дедлок, найденный в round 1).
    pub liveness_error: Option<String>,
    /// Живая таблица маршрутов несёт наши подсети, а лог НЕ подтверждает,
    /// что это мы (round 2: раньше это называлось «чужой туннель» и
    /// решалось по алиасу адаптера — round 1 показал, что алиас никогда
    /// не совпадает, а round 2 убрал его из решения вовсе). Смысл у поля
    /// только пока `our_tunnel_up == false` и `liveness_error == None`:
    /// предупредить ДО подъёма, что подсети уже кем-то заняты — своим ли
    /// забытым «хвостом» или правда чужим VPN, различить нельзя, поэтому
    /// текст говорит «занято», не утверждая, чьё.
    pub foreign_tunnel_up: bool,
    /// Не удалось прочитать живую таблицу маршрутов, а лог при этом
    /// СПОКОЙНО говорит «не поднято» — про `foreign_tunnel_up` сказать
    /// нечего честно, но про `our_tunnel_up` вопросов нет и без маршрутов
    /// (уточнение round 2: лог, уверенно сказавший «нет», не нуждается в
    /// подтверждении маршрутами — участие маршрутов там только
    /// подтверждающее). Гасит только кнопку подъёма.
    pub routes_error: Option<String>,
}

/// Тумблер автозапуска.
///
/// Трейт, а не прямой вызов `winnet::autostart`: настоящая реализация
/// (`WinAutostart` в `main.rs`) знает про `std::env::current_exe`, страница
/// же обязана уметь показывать тумблер, ничего не зная о реестре и о том,
/// как определяется путь к себе. Тесты страницы используют вторую,
/// заглушечную реализацию ниже — ей незачем трогать реальный реестр.
pub trait Autostart: Send + Sync {
    fn is_enabled(&self) -> Result<bool, String>;
    fn set(&self, on: bool) -> Result<(), String>;
}

/// Заглушка для тестов страницы: не молчит и не врёт — по этой ошибке
/// страница покажет тумблер выключенным и подписанным как есть. Только для
/// тестов: в рабочей сборке автозапуск подключён (`WinAutostart` в
/// `main.rs`), а публичность нужна лишь для сборки самих тестов из
/// `#[cfg(test)] mod tests` этого же файла.
#[cfg(test)]
pub struct AutostartPending;

#[cfg(test)]
impl Autostart for AutostartPending {
    fn is_enabled(&self) -> Result<bool, String> {
        Err("автозапуск ещё не подключён в этой сборке".to_string())
    }

    fn set(&self, _on: bool) -> Result<(), String> {
        Err("автозапуск ещё не подключён в этой сборке".to_string())
    }
}

/// Заглушка `Tunnel` для тестов, которым сам туннель безразличен (например,
/// тесты `websrv.rs`, проверяющие транспорт, а не раздел «Туннель»): всегда
/// «OpenVPN не найден», действия отказывают явным текстом, а не молчат.
/// Только для тестов — публична ради сборки тестов из других файлов крейта
/// (`websrv.rs`), тем же приёмом, что и `AutostartPending`.
#[cfg(test)]
pub struct TunnelPending;

#[cfg(test)]
impl Tunnel for TunnelPending {
    fn snapshot(&self, _office_subnets: &[Ipv4Net], _profile_name: &str) -> TunnelSnapshot {
        TunnelSnapshot::default()
    }
    fn build_profile(
        &self,
        _profile_name: &str,
        _office_subnets: &[Ipv4Net],
    ) -> Result<(), String> {
        Err("туннель ещё не подключён в этой сборке".to_string())
    }
    fn raise(&self, _profile_name: &str) -> Result<(), String> {
        Err("туннель ещё не подключён в этой сборке".to_string())
    }
    fn lower(&self, _profile_name: &str) -> Result<(), String> {
        Err("туннель ещё не подключён в этой сборке".to_string())
    }
    fn install_service(&self) -> Result<(), String> {
        Err("туннель ещё не подключён в этой сборке".to_string())
    }
}

/// Вторая тестовая реализация — с управляемым, а не всегда отказывающим
/// результатом. `AutostartPending` покрывает только путь «реализации нет»;
/// без этой реализации ветки `apply_autostart`, где `Autostart` реально
/// работает (обе `Ok`-ветки: «менять нечего» и «изменили и сообщили»), и
/// рендер включённого, не задизейбленного тумблера оставались бы без единого
/// теста — реализация была бы подключена в `main.rs`, но не проверена здесь
/// ни разу.
#[cfg(test)]
struct FakeAutostart {
    enabled: std::sync::atomic::AtomicBool,
    fail_set: bool,
}

#[cfg(test)]
impl FakeAutostart {
    fn new(enabled: bool) -> Self {
        Self {
            enabled: std::sync::atomic::AtomicBool::new(enabled),
            fail_set: false,
        }
    }

    fn failing_to_set(enabled: bool) -> Self {
        Self {
            enabled: std::sync::atomic::AtomicBool::new(enabled),
            fail_set: true,
        }
    }
}

#[cfg(test)]
impl Autostart for FakeAutostart {
    fn is_enabled(&self) -> Result<bool, String> {
        Ok(self.enabled.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn set(&self, on: bool) -> Result<(), String> {
        if self.fail_set {
            return Err("тестовый отказ записи в реестр".to_string());
        }
        self.enabled.store(on, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// Всё, что странице нужно знать о приложении.
pub struct SettingsState {
    /// Та же ячейка `ArcSwap`, что читает трей, а не копия: страница обязана
    /// показывать то же состояние, что и меню, иначе они разойдутся в первый
    /// же пересчёт маршрута.
    pub app: Arc<ArcSwap<AppState>>,
    /// Конфиг, каким его ЗАДАЛ ЧЕЛОВЕК. Только для чтения: писать в него
    /// имеет право лишь задача, обслуживающая канал `Cmd` (см. правило 2 в
    /// заголовке модуля), — она же кладёт сюда результат. Не «как на диске»:
    /// при отказе записи значение здесь всё равно обновится, а про отказ
    /// страница скажет отдельной строкой.
    ///
    /// Именно сохранённый, а не живой: в поле «порт моста» человек обязан
    /// увидеть то, что он туда вписал, а не то, на чём мы слушаем сейчас.
    /// Разницу между ними страница показывает отдельной строкой.
    pub config: Arc<ArcSwap<Config>>,
    /// Единственный путь применить изменение.
    pub commands: mpsc::Sender<Cmd>,
    /// Порт, на котором мост слушает СЕЙЧАС и до конца жизни процесса.
    pub bound_port: u16,
    pub autostart: Arc<dyn Autostart>,
    /// OpenVPN-туннель и установка службы статического IP (задача 7).
    pub tunnel: Arc<dyn Tunnel>,
}

/// Что показать над формой после действия.
#[derive(Debug, Default)]
pub struct Outcome {
    pub notes: Vec<Note>,
    pub bench: Option<Vec<BenchResult>>,
    pub doctor: Option<Vec<Check>>,
}

#[derive(Debug)]
pub struct Note {
    pub bad: bool,
    pub text: String,
}

impl Outcome {
    /// Одна строка отказа и ничего больше. Успех такой пары не имеет: он
    /// приходит вместе с остальными строками из [`apply`], где их может быть
    /// несколько (конфиг и автозапуск применяются по отдельности).
    fn bad(text: impl Into<String>) -> Self {
        Self {
            notes: vec![Note {
                bad: true,
                text: text.into(),
            }],
            ..Default::default()
        }
    }
}

/// Конфиг, который получает супервизор, из конфига, который лёг на диск.
///
/// Отличается ровно одним полем — портом моста, — и это и есть правило 1
/// заголовка модуля, выраженное кодом. Слушатель привязан один раз за жизнь
/// процесса; конфиг, в котором стоит другой порт, заставил бы `AppState.port`
/// (а через него — заголовок меню, «скопировать адрес» и диагностику)
/// говорить про порт, на котором никто не слушает.
pub fn live_config(saved: &Config, bound_port: u16) -> Config {
    Config {
        bridge_port: bound_port,
        ..saved.clone()
    }
}

/// Экранирование для вставки и в текст, и в значение атрибута.
///
/// Одна функция на оба случая сознательно: две — это два места, где можно
/// взять не ту. Кавычки экранируются обе, поэтому значение годится и внутри
/// `"…"`, и внутри `'…'`.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            // Амперсанд обязан идти первым: иначе он съел бы подстановки
            // соседей и `<` превратился бы в `&amp;lt;`.
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Разобранное тело формы: пары в том порядке, в каком их прислал браузер.
///
/// Именно последовательность, а не словарь: офисные сети приходят парой
/// повторяющихся полей (`office_id`, `office_name`), и связывает их между
/// собой только порядок — браузер отправляет поля в порядке разметки.
#[derive(Debug, Default)]
pub struct Form(Vec<(String, String)>);

impl Form {
    pub fn parse(body: &[u8]) -> Self {
        let mut pairs = Vec::new();
        for chunk in body.split(|&b| b == b'&') {
            if chunk.is_empty() {
                continue;
            }
            let (name, value) = match chunk.iter().position(|&b| b == b'=') {
                Some(i) => (&chunk[..i], &chunk[i + 1..]),
                // Поле без `=` — законная форма; браузеры так не шлют, но
                // отбросить его значило бы потерять флажок.
                None => (chunk, &b""[..]),
            };
            pairs.push((decode(name), decode(value)));
        }
        Self(pairs)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Флажок отправляется, только когда он отмечен, — снятый не приходит
    /// вовсе. Поэтому «отмечен» здесь означает «поле присутствует».
    pub fn checked(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }
}

/// Процентное декодирование `application/x-www-form-urlencoded`.
///
/// По байтам, а не по символам, и только в конце — сборка в строку: имя сети
/// приходит по-русски, то есть многобайтовым UTF-8, и декодировать `%D0%9E`
/// посимвольно значило бы получить два «символа» вместо одной буквы.
/// Недостроенная последовательность (`%D` в конце) остаётся собой, а не
/// роняет разбор: тело формы — вход недоверенный.
fn decode(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < raw.len() => match (hex(raw[i + 1]), hex(raw[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h << 4) | l);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Конфиг из формы поверх текущего.
///
/// Поверх, а не с нуля: форма владеет не всеми полями конфига. `mode` меняет
/// трей, тайминги и предел соединений правятся файлом, а `saved_sysproxy` —
/// единственный след системных настроек пользователя до нас, и потерять его
/// при сохранении формы значило бы потерять возможность вернуть машине сеть.
///
/// Правила проверки сюда НЕ дублируются: здесь только разбор текста в типы,
/// а осмысленность значений устанавливает `Config::validate` (правило 3).
pub fn config_from_form(base: &Config, form: &Form) -> Result<Config, String> {
    let raw_port = form.get("bridge_port").unwrap_or_default().trim();
    // Единственная проверка, которой в `Config::validate` быть не может:
    // там поле уже имеет тип `u16`, а «abc» до этого типа не доживает.
    // Осмысленность самого числа устанавливает именно `validate`.
    let bridge_port: u16 = raw_port
        .parse()
        .map_err(|_| format!("порт моста «{raw_port}»: нужно целое число от 1 до 65535"))?;

    let ids = form.all("office_id");
    let names = form.all("office_name");
    let office_networks = ids
        .iter()
        .enumerate()
        // На странице всегда есть пустая строка «добавить»; отправленная
        // нетронутой, она не должна превращаться в запись с пустым id —
        // такая запись не совпала бы ни с чем и была бы отвергнута
        // `Config::validate` на ровном месте.
        .filter(|(_, id)| !id.trim().is_empty())
        .map(|(i, id)| OfficeNetwork {
            id: id.trim().to_string(),
            name: names
                .get(i)
                .map(|n| n.trim())
                .unwrap_or_default()
                .to_string(),
        })
        .collect();

    Ok(Config {
        bridge_port,
        socks_upstream: optional(form.get("socks_upstream")),
        http_upstream: optional(form.get("http_upstream")),
        no_proxy: normalise_bypass(form.get("no_proxy").unwrap_or_default()),
        manage_system_proxy: form.checked("manage_system_proxy"),
        office_networks,
        // Спека 8.5: по умолчанию выключено — форма не присылает поле
        // вовсе, пока человек его не отметил (`Form::checked`), и тогда
        // `false` — то же самое значение, что и `Config::default()`.
        automate_tunnel: form.checked("automate_tunnel"),
        // Всё остальное — из текущего конфига, а не из формы, и это не
        // экономия. `mode` переключает трей; тайминги и предел соединений
        // правятся файлом; `saved_sysproxy` — единственный след системных
        // настроек пользователя ДО нас, и стереть его при сохранении формы
        // значило бы потерять возможность вернуть машине сеть после
        // аварийного завершения.
        ..base.clone()
    })
}

/// Пустое поле означает «не задан», а не «задан пустой строкой»:
/// `Config::validate` отвергла бы пустую строку как адрес без порта, и
/// человек не смог бы очистить поле вообще никак.
fn optional(v: Option<&str>) -> Option<String> {
    let v = v.unwrap_or_default().trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// `BypassList::parse` разбирает список через запятую, а в текстовое поле
/// человек естественно пишет по элементу на строку. Приводим к тому виду,
/// который умеет читать разбор, вместо того чтобы учить разбор второму
/// формату: список хранится в конфиге и правится ещё и файлом.
fn normalise_bypass(raw: &str) -> String {
    raw.split([',', '\n', '\r', ' ', '\t'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// Вся страница целиком.
///
/// Страница в бинаре, ни строки с диска: файл рядом с исполняемым означал бы,
/// что содержимое окна настроек может подменить кто угодно, кто умеет писать
/// в этот каталог.
pub fn render(state: &SettingsState, outcome: Option<&Outcome>) -> String {
    let app = state.app.load();
    let cfg = state.config.load();

    let mut b = String::with_capacity(16 * 1024);
    b.push_str("<!doctype html>\n<html lang=\"ru\"><head><meta charset=\"utf-8\">\n");
    b.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    b.push_str("<title>ProxyPilot — настройки</title>\n<style>");
    b.push_str(STYLE);
    b.push_str("</style></head>\n<body>\n");

    b.push_str("<h1>ProxyPilot</h1>\n");
    b.push_str("<section class=\"card\">\n");
    b.push_str(&format!(
        "<p class=\"now\">Мост слушает <b>127.0.0.1:{port}</b> · {route}<br>{network}</p>\n",
        port = app.port,
        route = escape_html(&route_text(&app)),
        network = escape_html(&network_text(&app)),
    ));

    if let Some(o) = outcome {
        for note in &o.notes {
            b.push_str(&format!(
                "<p class=\"note {cls}\">{text}</p>\n",
                cls = if note.bad { "bad" } else { "good" },
                text = escape_html(&note.text)
            ));
        }
    }
    b.push_str("</section>\n");

    // Форма настроек. Отдельная от кнопок замера и диагностики: те не
    // должны тащить с собой поля, которые человек ещё правит. Каждый раздел
    // — своя карточка (`section.card`): границы разделов держит вёрстка, а
    // не подчёркивание под заголовком.
    b.push_str("<form method=\"post\" action=\"\">\n");

    b.push_str("<section class=\"card\" aria-labelledby=\"upstreams\">\n");
    b.push_str("<h2 id=\"upstreams\">Апстримы</h2>\n");
    // Что применяется сразу, а что при запуске, сказано у каждого раздела
    // своими словами: одна общая оговорка внизу страницы читается ровно теми,
    // кто и так знает ответ.
    b.push_str(
        "<p class=\"hint\">Применяется сразу: маршрут пересчитывается тем же \
         путём, что и при переключении режима в трее, а установленные \
         соединения при этом не рвутся.</p>\n",
    );
    b.push_str(&field(
        "socks_upstream",
        "SOCKS5",
        cfg.socks_upstream.as_deref().unwrap_or(""),
        &health_chip(cfg.socks_upstream.as_deref(), app.health.socks),
    ));
    b.push_str(&field(
        "http_upstream",
        "HTTP-прокси",
        cfg.http_upstream.as_deref().unwrap_or(""),
        &health_chip(cfg.http_upstream.as_deref(), app.health.http),
    ));
    b.push_str("</section>\n");

    b.push_str("<section class=\"card\" aria-labelledby=\"port\">\n");
    b.push_str("<h2 id=\"port\">Порт моста</h2>\n");
    b.push_str(&field(
        "bridge_port",
        "Порт",
        &cfg.bridge_port.to_string(),
        "",
    ));
    // Не мелким шрифтом внизу, а прямо под полем: это единственное поле на
    // странице, которое не применяется сразу, и молчаливое расхождение
    // между «сохранено» и «работает» — худшее, что здесь можно сделать.
    if cfg.bridge_port == state.bound_port {
        b.push_str(
            "<p class=\"warn\">Порт применяется только при запуске: после \
             изменения перезапустите ProxyPilot. На лету он не меняется \
             намеренно — перепривязка оборвала бы все установленные \
             соединения.</p>\n",
        );
    } else {
        b.push_str(&format!(
            "<p class=\"warn\">Сохранено {saved}, но мост слушает {bound} — \
             перезапустите ProxyPilot, чтобы новый порт вступил в силу. \
             Перепривязать слушатель на лету значило бы оборвать все \
             установленные соединения.</p>\n",
            saved = cfg.bridge_port,
            bound = state.bound_port
        ));
    }
    b.push_str("</section>\n");

    b.push_str("<section class=\"card\" aria-labelledby=\"networks\">\n");
    b.push_str("<h2 id=\"networks\">Офисные сети</h2>\n");
    b.push_str(
        "<p class=\"hint\">Применяется сразу. Решение принимается по GUID: имя \
         сети человек может переименовать в любой момент. Чтобы убрать сеть — \
         очистите её GUID и сохраните.</p>\n",
    );
    b.push_str("<div class=\"table-wrap\"><table>\n<tr><th>GUID</th><th>Имя</th></tr>\n");
    for net in cfg.office_networks.iter() {
        b.push_str(&office_row(&net.id, &net.name));
    }
    // Пустая строка для добавления руками. Пустой id отбрасывается при
    // разборе, поэтому нетронутая строка ничего не портит.
    b.push_str(&office_row("", ""));
    b.push_str("</table></div>\n");
    b.push_str("</section>\n");

    b.push_str("<section class=\"card\" aria-labelledby=\"tunnel-automation\">\n");
    b.push_str("<h2 id=\"tunnel-automation\">Автоматика туннеля</h2>\n");
    b.push_str(&checkbox(
        "automate_tunnel",
        "Поднимать туннель автоматически вне офиса",
        cfg.automate_tunnel,
        false,
        "Выключено по умолчанию (спека 8.5) — туннель поднимается руками, пока \
         вы не включите это сами. Пока только сохраняет намерение: сам подъём и \
         опускание по смене сети этот тумблер ещё не выполняет — управляйте \
         туннелем кнопками ниже.",
    ));
    b.push_str("</section>\n");

    b.push_str("<section class=\"card\" aria-labelledby=\"bypass\">\n");
    b.push_str("<h2 id=\"bypass\">Мимо прокси</h2>\n");
    b.push_str(&format!(
        "<label for=\"no_proxy\">Адреса и подсети, по одному в строке или через \
         запятую</label>\n<textarea id=\"no_proxy\" name=\"no_proxy\" rows=\"4\">\
         {}</textarea>\n",
        escape_html(&cfg.no_proxy)
    ));
    b.push_str(
        "<p class=\"warn\">Список применяется при запуске: после изменения \
         перезапустите ProxyPilot.</p>\n",
    );
    b.push_str("</section>\n");

    b.push_str("<section class=\"card\" aria-labelledby=\"system\">\n");
    b.push_str("<h2 id=\"system\">Система</h2>\n");
    b.push_str(&checkbox(
        "manage_system_proxy",
        "Управлять системными настройками прокси Windows",
        cfg.manage_system_proxy,
        false,
        "Применяется при запуске: после изменения перезапустите ProxyPilot. \
         Выключите, если прокси задаёт групповая политика или вы ходите через \
         мост только явным -x.",
    ));

    let (autostart_on, autostart_note) = match state.autostart.is_enabled() {
        Ok(on) => (on, String::new()),
        Err(e) => (false, e),
    };
    b.push_str(&checkbox(
        "autostart",
        "Запускать вместе с Windows",
        autostart_on,
        !autostart_note.is_empty(),
        &autostart_note,
    ));
    b.push_str("</section>\n");

    b.push_str("<div class=\"actions\">");
    b.push_str("<button type=\"submit\" name=\"action\" value=\"save\">Сохранить</button>");
    // Кнопка «эта сеть — офис» — обычная кнопка отправки, а не скрипт: GUID
    // текущей сети сервер и так знает из `AppState.place`, а страница без
    // скрипта не требует ослаблять CSP до `script-src 'unsafe-inline'`.
    if let Some(id) = app.place.network.as_deref() {
        let name = app
            .place
            .network_name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(id);
        b.push_str(&format!(
            "<button type=\"submit\" name=\"action\" value=\"office\">\
             Эта сеть — офис: {}</button>",
            escape_html(name)
        ));
    }
    b.push_str("</div>\n</form>\n");

    // Раздел «Туннель» — отдельными формами по той же причине, что и замер
    // и диагностика ниже: каждая кнопка не должна тащить с собой поля
    // настроек, которые человек ещё правит.
    let tunnel_snapshot = state
        .tunnel
        .snapshot(&cfg.office_subnets, TUNNEL_PROFILE_NAME);
    b.push_str("<section class=\"card\" aria-labelledby=\"tunnel\">\n");
    b.push_str(&tunnel_section(&cfg.office_subnets, &tunnel_snapshot));
    b.push_str("</section>\n");

    // Замер и диагностика — отдельными формами: их кнопки не должны
    // отправлять поля настроек, которые человек ещё правит.
    b.push_str("<section class=\"card\" aria-labelledby=\"bench\">\n");
    b.push_str("<h2 id=\"bench\">Замер скорости</h2>\n");
    b.push_str(&format!(
        "<p class=\"hint\">Один поток и короткий файл: цифры сравнивают \
         маршруты между собой, а не измеряют канал. Качается {}.</p>\n",
        escape_html(BENCH_URL)
    ));
    b.push_str(
        "<form method=\"post\" action=\"\" class=\"action\"><button type=\"submit\" \
         name=\"action\" value=\"bench\">Замерить</button></form>\n",
    );
    if let Some(results) = outcome.and_then(|o| o.bench.as_ref()) {
        b.push_str(&bench_table(results));
    }
    b.push_str("</section>\n");

    b.push_str("<section class=\"card\" aria-labelledby=\"doctor\">\n");
    b.push_str("<h2 id=\"doctor\">Диагностика</h2>\n");
    b.push_str(
        "<form method=\"post\" action=\"\" class=\"action\"><button type=\"submit\" \
         name=\"action\" value=\"doctor\">Проверить</button></form>\n",
    );
    if let Some(checks) = outcome.and_then(|o| o.doctor.as_ref()) {
        b.push_str(&doctor_table(checks));
    }
    b.push_str("</section>\n");

    b.push_str("</body></html>\n");
    b
}

/// Вся вёрстка страницы — один инлайн `<style>`, потому что CSP разрешает
/// именно это (`style-src 'self' 'unsafe-inline'`) и запрещает вообще любой
/// скрипт: значит состояния, переключатели и раскладка обязаны решаться
/// чистым CSS (`:has()`, `:focus-visible`, `accent-color`-подобные трюки на
/// нативном `<input type=checkbox>`, медиа-запросы), а не JS.
///
/// Три принципа, которые тут закодированы:
///
/// 1. **Тёмная тема — через `prefers-color-scheme`, без переключателя.**
///    Трей живёт на рабочем столе, где тёмная тема — обычное дело; второй
///    системы цветов, кроме системной, здесь нет и не должно быть.
/// 2. **Состояние — не только цветом.** «доступен» / «недоступен» / «не
///    задан» и «ок» / «внимание» / «отказ» получают ещё и разную форму
///    значка (кружок с «✓», «✕», «!», «…», «—»), потому что часть людей не
///    отличает красный от зелёного — тот же довод, по которому иконки в
///    трее различаются формой, а не только цветом.
/// 3. **Ни один шрифт не подгружается извне.** Сокет — локальный,
///    `default-src 'self'` не пропустит ни один внешний хост, поэтому оба
///    стека — только системные гарнитуры Windows.
const STYLE: &str = r#"
:root{
  --bg:#f3f4f6;--bg-elevated:#ffffff;--fg:#1a1d21;--fg-muted:#5b6169;
  --border:#d9dce1;--accent:#2f5fd6;--focus:#2f5fd6;--focus-ring:rgba(47,95,214,.28);
  --good-fg:#146c37;--good-bg:#e3f5e9;--bad-fg:#a3241c;--bad-bg:#fbe8e6;
  --warn-fg:#8a5a00;--warn-bg:#fbf0d9;
  --sans:'Segoe UI Variable Text','Segoe UI',system-ui,-apple-system,sans-serif;
  --mono:'Cascadia Mono',Consolas,'Courier New',monospace;--radius:10px;
}
/* Тёмная тема следует системной настройке; собственного переключателя нет
   намеренно — на странице без скрипта переключать было бы нечем. */
@media (prefers-color-scheme:dark){:root{
  --bg:#15171b;--bg-elevated:#1e2126;--fg:#e8eaed;--fg-muted:#9aa0a8;
  --border:#33373e;--accent:#7aa5ff;--focus:#7aa5ff;--focus-ring:rgba(122,165,255,.35);
  --good-fg:#5fd88b;--good-bg:#173225;--bad-fg:#ff9086;--bad-bg:#3a201f;
  --warn-fg:#f0c14b;--warn-bg:#3a2f12;
}}
*{box-sizing:border-box}
body{font:15px/1.55 var(--sans);margin:0;padding:2.25rem 1rem 4rem;max-width:44rem;
  margin-inline:auto;background:var(--bg);color:var(--fg)}
h1{font-size:1.55rem;font-weight:650;letter-spacing:-.01em;margin:.1rem 0 1.4rem}
h2{font-size:1.02rem;font-weight:650;margin:0 0 .8rem;color:var(--fg)}
section.card{background:var(--bg-elevated);border:1px solid var(--border);
  border-radius:var(--radius);padding:1.15rem 1.3rem;margin-bottom:1rem;
  box-shadow:0 1px 2px rgba(0,0,0,.05)}
label{display:block;margin-top:.9rem;font-weight:500}
input[type=text],textarea{width:100%;font:inherit;color:var(--fg);background:var(--bg);
  border:1px solid var(--border);border-radius:8px;padding:.5rem .65rem;margin-top:.35rem;
  transition:border-color .15s,box-shadow .15s}
textarea{min-height:5rem;resize:vertical}
input[type=text]:focus-visible,textarea:focus-visible{
  outline:none;border-color:var(--focus);box-shadow:0 0 0 3px var(--focus-ring)}
table{border-collapse:collapse;width:100%;font-size:.92rem}
th,td{text-align:left;padding:.4rem .5rem;border-bottom:1px solid var(--border)}
th{font-weight:600;color:var(--fg-muted);font-size:.78rem;text-transform:uppercase;
  letter-spacing:.03em}
table input[type=text]{margin-top:0}
.table-wrap{overflow-x:auto;margin-top:.5rem}
/* Нативный чекбокс перерисован в переключатель чистым CSS — `appearance:none`
   плюс `::before` для ползунка; никакого скрипта для этого не нужно. */
label:has(>input[type=checkbox]){display:flex;align-items:center;gap:.65rem;
  padding:.45rem 0;cursor:pointer;font-weight:500}
input[type=checkbox]{appearance:none;-webkit-appearance:none;width:2.15rem;height:1.2rem;
  border-radius:999px;background:var(--border);position:relative;flex:none;margin:0;
  cursor:pointer;transition:background-color .15s}
input[type=checkbox]::before{content:"";position:absolute;top:2px;left:2px;width:1rem;
  height:1rem;border-radius:50%;background:var(--bg-elevated);
  box-shadow:0 1px 2px rgba(0,0,0,.3);transition:transform .15s}
input[type=checkbox]:checked{background:var(--accent)}
input[type=checkbox]:checked::before{transform:translateX(.95rem)}
input[type=checkbox]:disabled{opacity:.5;cursor:not-allowed}
input[type=checkbox]:focus-visible{outline:2px solid var(--focus);outline-offset:2px}
button{font:inherit;font-weight:600;border:1px solid var(--accent);border-radius:8px;
  padding:.55rem 1.1rem;min-height:2.5rem;cursor:pointer;background:var(--accent);
  color:#fff;transition:filter .15s,transform .05s}
button:hover{filter:brightness(1.08)}
button:active{transform:translateY(1px)}
button:focus-visible{outline:2px solid var(--focus);outline-offset:3px}
/* Кнопка «Сохранить» — единственное основное действие формы, поэтому
   единственная сплошная; сеть-в-офис и кнопки разделов ниже — второстепенные
   действия, поэтому контурные (тот же язык, что у переключателя). */
button[value=office]{background:transparent;color:var(--accent)}
button[value=office]:hover{background:var(--accent);color:#fff}
form.action button{background:transparent;color:var(--accent)}
form.action button:hover{background:var(--accent);color:#fff}
/* Установка службы — единственный путь к UAC во всём приложении (см.
   CLAUDE.md, «Права администратора») — помечена отдельным цветом, чтобы
   не потеряться среди обычных кнопок. */
form.action button[value=install_service]{color:var(--warn-fg);border-color:var(--warn-fg)}
form.action button[value=install_service]:hover{background:var(--warn-fg);color:#fff}
form.action{display:inline-block;margin:0 .5rem .5rem 0}
.actions{display:flex;flex-wrap:wrap;gap:.6rem;margin-top:1.2rem}
.now{background:var(--bg);border:1px dashed var(--border);border-radius:8px;
  padding:.75rem .9rem;font-size:.95rem;line-height:1.6}
.now b{font-variant-numeric:tabular-nums}
.hint{color:var(--fg-muted);font-size:.88rem;margin:.4rem 0}
.warn{color:var(--warn-fg);font-size:.88rem;margin:.5rem 0;padding:.5rem .7rem;
  background:var(--warn-bg);border-radius:8px}
.note{margin:.5rem 0;padding:.6rem .8rem;border-radius:8px;font-size:.92rem;
  display:flex;align-items:flex-start;gap:.55rem}
.note::before{flex:none;font-weight:700}
.note.good{background:var(--good-bg);color:var(--good-fg)}
.note.good::before{content:"✓"}
.note.bad{background:var(--bad-bg);color:var(--bad-fg)}
.note.bad::before{content:"✕"}
/* Значок статуса апстрима — форма плюс цвет, не только цвет (см. докблок
   константы). */
.chip{display:inline-flex;align-items:center;gap:.3rem;font-size:.82rem;font-weight:600;
  padding:.15rem .55rem;border-radius:999px}
.chip::before{font-weight:700}
.chip.status-up{background:var(--good-bg);color:var(--good-fg)}
.chip.status-up::before{content:"✓"}
.chip.status-down{background:var(--bad-bg);color:var(--bad-fg)}
.chip.status-down::before{content:"✕"}
.chip.status-unknown{background:var(--warn-bg);color:var(--warn-fg)}
.chip.status-unknown::before{content:"…"}
.chip.status-unset{background:var(--border);color:var(--fg-muted)}
.chip.status-unset::before{content:"—"}
.checks{list-style:none;margin:.75rem 0 0;padding:0}
.check{display:flex;gap:.65rem;align-items:flex-start;padding:.65rem .8rem;
  border:1px solid var(--border);border-radius:8px;margin-bottom:.5rem;background:var(--bg)}
.check::before{flex:none;width:1.3rem;height:1.3rem;border-radius:50%;display:flex;
  align-items:center;justify-content:center;font-size:.78rem;font-weight:700;color:#fff}
.check.ok::before{content:"✓";background:var(--good-fg)}
.check.warn::before{content:"!";background:var(--warn-fg)}
.check.fail::before{content:"✕";background:var(--bad-fg)}
.check-body{flex:1;min-width:0}
.check-head{display:flex;justify-content:space-between;gap:.6rem;flex-wrap:wrap}
.check-title{font-weight:600}
.check-status{font-size:.72rem;font-weight:700;text-transform:uppercase;
  letter-spacing:.03em;color:var(--fg-muted)}
.check-detail{margin-top:.3rem;font-family:var(--mono);font-size:.82rem;
  color:var(--fg-muted);white-space:pre-wrap;word-break:break-word}
.bench-results{list-style:none;margin:.75rem 0 0;padding:0}
.bench-row{display:grid;grid-template-columns:1fr auto auto;gap:.4rem 1rem;
  align-items:center;padding:.55rem .8rem;border:1px solid var(--border);border-radius:8px;
  margin-bottom:.4rem;background:var(--bg)}
.bench-row.winner{border-color:var(--good-fg)}
.bench-row.failed{border-color:var(--bad-fg)}
.bench-label{font-weight:600}
.bench-speed{font-variant-numeric:tabular-nums;color:var(--fg-muted)}
.ok{color:var(--good-fg);font-weight:600}
.fail{color:var(--bad-fg);font-weight:600}
/* Страница открывается в том окне браузера, какое случится — узкое не
   исключение. */
@media (max-width:480px){
  body{padding:1.4rem .75rem 3rem}
  section.card{padding:.9rem 1rem}
  .bench-row{grid-template-columns:1fr}
  .check-head{flex-direction:column;gap:.15rem}
}
"#;

/// `note_html` — уже готовая разметка (значок статуса или пусто), не
/// сырой текст: единственный вызывающий, которому есть что показать
/// (апстримы), собирает её через [`health_chip`], которая экранирует
/// текст сама. Собственного экранирования здесь поэтому нет — второе
/// было бы дублем первого, а не защитой.
fn field(name: &str, label: &str, value: &str, note_html: &str) -> String {
    let note = if note_html.is_empty() {
        String::new()
    } else {
        format!(" {note_html}")
    };
    format!(
        "<label for=\"{name}\">{label}{note}</label>\n\
         <input type=\"text\" id=\"{name}\" name=\"{name}\" value=\"{value}\">\n",
        name = escape_html(name),
        label = escape_html(label),
        value = escape_html(value),
    )
}

fn checkbox(name: &str, label: &str, on: bool, disabled: bool, note: &str) -> String {
    let note = if note.is_empty() {
        String::new()
    } else {
        format!("<p class=\"hint\">{}</p>\n", escape_html(note))
    };
    format!(
        "<label><input type=\"checkbox\" name=\"{name}\"{on}{dis}> {label}</label>\n{note}",
        name = escape_html(name),
        on = if on { " checked" } else { "" },
        dis = if disabled { " disabled" } else { "" },
        label = escape_html(label),
    )
}

fn office_row(id: &str, name: &str) -> String {
    format!(
        "<tr><td><input type=\"text\" name=\"office_id\" value=\"{id}\"></td>\
         <td><input type=\"text\" name=\"office_name\" value=\"{name}\"></td></tr>\n",
        id = escape_html(id),
        name = escape_html(name),
    )
}

/// Раздел «Туннель»: где мы стоим (OpenVPN найден? профиль собран?
/// поднято?), явное предупреждение про DNS до подъёма (спека 8.2), и
/// кнопки — свои, отдельными формами (см. комментарий в `render`).
///
/// Если поднят чужой туннель в наши сети — кнопка подъёма не рисуется
/// вовсе (приёмка задачи 7): показывать кнопку, которая приведёт к двум
/// туннелям, спорящим за маршруты, значит предлагать человеку то, что мы
/// уже знаем — плохая идея.
fn tunnel_section(office_subnets: &[Ipv4Net], snap: &TunnelSnapshot) -> String {
    let mut b = String::new();
    b.push_str("<h2 id=\"tunnel\">Туннель (OpenVPN)</h2>\n");

    if !snap.installed {
        // Приёмка: раздел неактивен с внятным объяснением, а не просто
        // серый — дальше в разделе рисовать нечего, все прочие поля снимка
        // не имеют смысла без установленного OpenVPN.
        b.push_str(
            "<p class=\"hint\">OpenVPN не найден на этой машине — раздел \
             недоступен. Установите OpenVPN (openvpn.net/community-downloads) \
             и откройте эту страницу снова.</p>\n",
        );
        return b;
    }

    if office_subnets.is_empty() {
        b.push_str(
            "<p class=\"hint\">Офисные подсети не заданы — маршруты в профиль \
             не добавятся (раздел «Апстримы» их не хранит; правится файлом \
             конфига).</p>\n",
        );
    } else {
        let list = office_subnets
            .iter()
            .map(Ipv4Net::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        b.push_str(&format!(
            "<p class=\"hint\">Через туннель маршрутизируются подсети: {}.</p>\n",
            escape_html(&list)
        ));
    }

    let profile_word = if snap.profile_installed {
        "установлен"
    } else {
        "не собран"
    };
    b.push_str(&format!(
        "<p>Профиль в OpenVPN: <b>{profile_word}</b> (имя «{}»; поднят ли \
         туннель — определяется по логу OpenVPN GUI для этого же имени, а \
         не по имени адаптера).</p>\n",
        escape_html(TUNNEL_PROFILE_NAME)
    ));

    // Приёмка: пользователь узнаёт про DNS ДО подъёма, а не догадывается —
    // показывается всегда, а не только когда туннель уже поднят.
    b.push_str(
        "<p class=\"warn\">Пока туннель поднят, все DNS-запросы уходят в офис \
         — так резолвятся внутренние имена (git, dev-серверы), и это \
         осознанная плата (спека 8.2). Вне туннеля резолвинг снова \
         обычный.</p>\n",
    );

    // Приоритет проверок — не произвольный порядок if/else, а прямое
    // следствие fix round 1 задачи 7: `our_tunnel_up` (лог, ключ — имя
    // профиля, которым мы владеем) проверяется РАНЬШЕ `foreign_tunnel_up`
    // (таблица маршрутов + псевдоним адаптера, которым мы не владеем).
    // Обратный порядок — ровно то, что сломалось до этого исправления:
    // наш же поднятый туннель, классифицированный как «чужой» по
    // ненадёжному алиасу, прятал кнопку «опустить» и запирал раздел
    // навсегда. Теперь подтверждённая логом поднятость перевешивает любую
    // догадку по маршрутам.
    if let Some(err) = &snap.liveness_error {
        // Честное «не знаю» — не запертые кнопки. Обе кнопки показаны:
        // повторный connect/disconnect для уже подключённого/отключённого
        // профиля openvpn-gui.exe принимает как обычную команду (докблок
        // `openvpn::connect`/`disconnect` — «Ok» значит «доставлено», не
        // «изменило состояние»), а раздел, который вместо этого молча
        // прячет обе кнопки, — тот самый дедлок, который здесь как раз
        // устраняется.
        b.push_str(&format!(
            "<p class=\"note bad\">Не удалось определить, поднят ли туннель \
             (лог OpenVPN GUI): {} — состояние неизвестно. Обе кнопки ниже \
             показаны: лишний повторный подъём или опускание для OpenVPN \
             GUI обычно безвреден, но после нажатия стоит свериться с \
             иконкой ProxyPilot в трее.</p>\n",
            escape_html(err)
        ));
        b.push_str(&tunnel_action_form("raise_tunnel", "Поднять туннель"));
        b.push_str(&tunnel_action_form("lower_tunnel", "Опустить туннель"));
    } else if snap.our_tunnel_up {
        b.push_str("<p class=\"note good\">Туннель поднят.</p>\n");
        b.push_str(&tunnel_action_form("lower_tunnel", "Опустить туннель"));
    } else if snap.rising {
        // Round 2: лог уже подтвердил успешное подключение, но маршруты
        // профиля ещё не встали — короткое окно сразу после «Поднять
        // туннель». Не «опущен» (выглядело бы как приглашение нажать
        // «Поднять» ещё раз, породив второй connect) и не «поднят» (это
        // ещё не доказано маршрутами) — отдельная, спокойная формулировка.
        // Кнопка «опустить» предложена как отмена: команда disconnect
        // тому же профилю прерывает и ещё не завершившееся подключение.
        b.push_str("<p class=\"note good\">Туннель поднимается…</p>\n");
        b.push_str(&tunnel_action_form(
            "lower_tunnel",
            "Отменить / опустить туннель",
        ));
    } else if let Some(err) = &snap.routes_error {
        b.push_str(&format!(
            "<p class=\"note bad\">Туннель опущен. Не удалось прочитать \
             таблицу маршрутов, чтобы проверить, не занят ли он уже другим \
             VPN: {} — подъём временно недоступен, пока это не \
             исправится.</p>\n",
            escape_html(err)
        ));
    } else if snap.foreign_tunnel_up {
        // Не «ЧУЖОЙ туннель» безусловно: адаптер мог остаться и от нашей
        // же незакрытой предыдущей сессии — алиас это не различает (см.
        // докблок TUNNEL_PROFILE_NAME). Безопасное действие одно и то же
        // в обоих случаях — не поднимать поверх, — поэтому текст говорит
        // «занято», а не приписывает адаптер конкретно чужому VPN.
        b.push_str(
            "<p class=\"note bad\">Туннель опущен, но какой-то туннельный \
             адаптер уже несёт офисные подсети (другой VPN, Tailscale, \
             WireGuard... или наша же не до конца закрытая прошлая сессия — \
             отличить по имени адаптера нельзя). Поднимать свой нельзя — \
             два туннеля будут спорить за маршруты. Закройте тот, что уже \
             поднят, и вернитесь сюда.</p>\n",
        );
    } else {
        b.push_str("<p>Туннель опущен.</p>\n");
        if snap.profile_installed {
            b.push_str(&tunnel_action_form("raise_tunnel", "Поднять туннель"));
        } else {
            b.push_str("<p class=\"hint\">Сначала соберите профиль — кнопка ниже.</p>\n");
        }
    }

    // Приёмка: явное предупреждение про UAC у обеих кнопок, ДО нажатия —
    // одним абзацем перед обеими, а не после клика.
    b.push_str(
        "<p class=\"hint\">Кнопки ниже выходят за рамки обычной работы \
         приложения. «Собрать профиль» пишет .ovpn-файл в каталог \
         конфигураций OpenVPN — прав администратора не нужно, окна UAC не \
         будет. «Установить службу статического IP» — да, нужны права \
         администратора: Windows покажет запрос UAC, единственный во всём \
         приложении.</p>\n",
    );
    b.push_str(&tunnel_action_form(
        "build_tunnel_profile",
        "Собрать профиль",
    ));
    b.push_str(&tunnel_action_form(
        "install_service",
        "Установить службу статического IP…",
    ));

    b
}

fn tunnel_action_form(action: &str, label: &str) -> String {
    format!(
        "<form method=\"post\" action=\"\" class=\"action\"><button type=\"submit\" \
         name=\"action\" value=\"{action}\">{label}</button></form>\n",
        action = escape_html(action),
        label = escape_html(label),
    )
}

/// Строка про апстрим: адрес плюс то, отвечал ли он на последней пробе.
/// «Не задан» отделено от «недоступен» тем же словарём, что и в трее:
/// первое чинится настройкой, второе — сетью.
fn health_text(addr: Option<&str>, health: Reachability) -> String {
    if addr.is_none_or(str::is_empty) {
        return "не задан".to_string();
    }
    match health {
        Reachability::Up => "доступен".to_string(),
        Reachability::Down => "недоступен".to_string(),
        Reachability::Unknown => "ещё не проверен".to_string(),
    }
}

/// Класс значка для того же состояния, что и [`health_text`] — разнесены,
/// а не слиты в одну строку «class:text»: два места, разбирающих такую
/// строку обратно, были бы более хрупким кодом, чем два маленьких `match`.
fn health_class(addr: Option<&str>, health: Reachability) -> &'static str {
    if addr.is_none_or(str::is_empty) {
        return "status-unset";
    }
    match health {
        Reachability::Up => "status-up",
        Reachability::Down => "status-down",
        Reachability::Unknown => "status-unknown",
    }
}

/// Готовый значок статуса апстрима для вставки в разметку — форма (через
/// класс, см. докблок [`STYLE`]) плюс текст, который уже нельзя спутать по
/// смыслу (те же три слова, что и в трее).
fn health_chip(addr: Option<&str>, health: Reachability) -> String {
    format!(
        "<span class=\"chip {}\">{}</span>",
        health_class(addr, health),
        escape_html(&health_text(addr, health))
    )
}

/// То же, что показывает заголовок меню: что с трафиком происходит НА САМОМ
/// ДЕЛЕ, а не что выбрано. Понижение показывается, а не скрывается.
fn route_text(app: &AppState) -> String {
    let mode = match app.mode {
        Mode::Auto => "Авто",
        Mode::Socks => "SOCKS5",
        Mode::Http => "HTTP",
        Mode::Direct => "Напрямую",
    };
    if app.demoted {
        return format!("режим {mode}: апстрим недоступен → работаем напрямую");
    }
    match &app.route {
        Route::Socks(addr) => format!("режим {mode}: SOCKS5 → {addr}"),
        Route::Http(addr) => format!("режим {mode}: HTTP → {addr}"),
        Route::Direct => format!("режим {mode}: напрямую"),
    }
}

fn network_text(app: &AppState) -> String {
    let name = app
        .place
        .network_name
        .as_deref()
        .filter(|n| !n.is_empty())
        .or(app.place.network.as_deref());
    match name {
        Some(n) if app.place.in_office => format!("Сеть: {n} — офисная"),
        Some(n) => format!("Сеть: {n} — не в списке офисных"),
        None => "Сеть: не определена".to_string(),
    }
}

/// Результаты замера — список карточек, а не таблица: у каждого маршрута
/// свой блок с рамкой, подсвеченной по исходу (победитель / отказ), а не
/// строка среди прочих строк. Тот же довод, что у [`doctor_table`].
fn bench_table(results: &[BenchResult]) -> String {
    let best = fastest(results).map(|r| r.label.clone());
    let mut out = String::from("<ul class=\"bench-results\">\n");
    for r in results {
        let is_winner = best.as_deref() == Some(r.label.as_str());
        // Путь, который не отработал, показывается как не отработавший, а не
        // пропускается: пропущенная строка выглядела бы как «не настроен».
        let row_class = if r.error.is_some() {
            "failed"
        } else if is_winner {
            "winner"
        } else {
            ""
        };
        let speed = match r.speed_bps() {
            Some(bps) => format!("{:.2} МБ/с", bps as f64 / 1_048_576.0),
            None => "—".to_string(),
        };
        let note = match &r.error {
            Some(e) => format!("<span class=\"fail\">{}</span>", escape_html(e)),
            None if is_winner => "<span class=\"ok\">быстрее прочих</span>".to_string(),
            None => format!("{} байт", r.bytes),
        };
        out.push_str(&format!(
            "<li class=\"bench-row {row_class}\"><span class=\"bench-label\">{}</span>\
             <span class=\"bench-speed\">{}</span><span class=\"bench-note\">{}</span></li>\n",
            escape_html(&r.label),
            escape_html(&speed),
            note
        ));
    }
    out.push_str("</ul>\n");
    out
}

/// Диагностика — список результатов, каждый со своим значком (форма, не
/// только цвет — см. докблок [`STYLE`]), а не таблица из строк
/// преформатированного текста: результат должен читаться как результат, а
/// не как вывод консоли.
fn doctor_table(checks: &[Check]) -> String {
    let mut out = String::from("<ul class=\"checks\">\n");
    for c in checks {
        let (cls, mark) = match c.status {
            CheckStatus::Ok => ("ok", "ок"),
            CheckStatus::Warn => ("warn", "внимание"),
            CheckStatus::Fail => ("fail", "отказ"),
        };
        out.push_str(&format!(
            "<li class=\"check {cls}\"><div class=\"check-body\">\
             <div class=\"check-head\"><span class=\"check-title\">{title}</span>\
             <span class=\"check-status\">{mark}</span></div>\
             <div class=\"check-detail\">{detail}</div></div></li>\n",
            title = escape_html(&c.title),
            detail = escape_html(&c.detail),
        ));
    }
    out.push_str("</ul>\n");
    out
}

/// Обработка отправленной формы. Возвращает уже готовую страницу.
pub async fn handle_post(state: &SettingsState, body: &[u8]) -> String {
    let form = Form::parse(body);
    let outcome = match form.get("action").unwrap_or("save") {
        "bench" => Outcome {
            bench: Some(
                bench_all(
                    &state.config.load().upstreams(),
                    BENCH_URL,
                    BENCH_LIMIT,
                    BENCH_TIMEOUT,
                )
                .await,
            ),
            ..Default::default()
        },
        "doctor" => Outcome {
            doctor: Some(live_checks(state).await),
            ..Default::default()
        },
        action @ ("save" | "office") => apply(state, &form, action == "office").await,
        "build_tunnel_profile" => {
            let cfg = state.config.load();
            tunnel_outcome(
                state
                    .tunnel
                    .build_profile(TUNNEL_PROFILE_NAME, &cfg.office_subnets),
                "Профиль собран и записан в каталог конфигураций OpenVPN.",
            )
        }
        "raise_tunnel" => raise_tunnel(state).await,
        "lower_tunnel" => tunnel_outcome(
            state.tunnel.lower(TUNNEL_PROFILE_NAME),
            "Команда на опускание туннеля отправлена OpenVPN GUI.",
        ),
        "install_service" => tunnel_outcome(
            state.tunnel.install_service(),
            "Запрос на установку службы отправлен — подтвердите запрос UAC, \
             если Windows его покажет.",
        ),
        other => Outcome::bad(format!("неизвестное действие «{other}»")),
    };
    render(state, Some(&outcome))
}

/// Собрать конфиг из формы, проверить его `Config::validate` и отправить
/// единственным путём — каналом `Cmd` в супервизор.
async fn apply(state: &SettingsState, form: &Form, add_current_network: bool) -> Outcome {
    let mut next = match config_from_form(&state.config.load(), form) {
        Ok(c) => c,
        Err(e) => return Outcome::bad(e),
    };

    if add_current_network {
        let app = state.app.load();
        let Some(id) = app.place.network.clone() else {
            return Outcome::bad("текущая сеть не определена — добавлять нечего");
        };
        if next
            .office_networks
            .iter()
            .any(|o| o.id.eq_ignore_ascii_case(&id))
        {
            return Outcome::bad("эта сеть уже в списке офисных");
        }
        next.office_networks.push(OfficeNetwork {
            id,
            name: app.place.network_name.clone().unwrap_or_default(),
        });
    }

    // Правила — только здесь и только одни. Пересказывать их скриптом на
    // странице значило бы завести вторую копию, которая разойдётся с этой.
    if let Err(e) = next.validate() {
        return Outcome::bad(e.to_string());
    }

    let mut notes = Vec::new();
    // Автозапуск живёт не в конфиге, а в реестре, поэтому применяется
    // отдельно — но его отказ не отменяет остальных изменений.
    if let Some(note) = apply_autostart(state, form) {
        notes.push(note);
    }

    let port_changed = next.bridge_port != state.bound_port;
    let (done, wait) = oneshot::channel();
    if state
        .commands
        .send(Cmd::ApplyConfig {
            config: Box::new(next),
            done,
        })
        .await
        .is_err()
    {
        notes.push(Note {
            bad: true,
            text: "приложение завершается — изменения не применены".to_string(),
        });
        return Outcome {
            notes,
            ..Default::default()
        };
    }

    match tokio::time::timeout(APPLY_TIMEOUT, wait).await {
        Ok(Ok(Ok(()))) => notes.push(Note {
            bad: false,
            text: if port_changed {
                "Сохранено и применено. Порт моста вступит в силу после \
                 перезапуска ProxyPilot — на лету он не меняется."
                    .to_string()
            } else {
                "Сохранено и применено.".to_string()
            },
        }),
        // Применить успели, а записать на диск — нет: правка живёт до
        // перезапуска. Умолчать об этом хуже, чем сказать.
        Ok(Ok(Err(e))) => notes.push(Note {
            bad: true,
            text: format!("Применено, но не сохранено в конфиг: {e}"),
        }),
        Ok(Err(_)) => notes.push(Note {
            bad: true,
            text: "приложение завершается — изменения не применены".to_string(),
        }),
        Err(_) => notes.push(Note {
            bad: true,
            text: "супервизор не ответил вовремя; проверьте состояние в трее".to_string(),
        }),
    }
    Outcome {
        notes,
        ..Default::default()
    }
}

/// Единственный результат одной кнопки раздела «Туннель» — успех или отказ,
/// дословно из [`Tunnel`] (приёмка задачи 7: ошибка `build_profile` не
/// проглатывается, а доходит до человека как есть).
fn tunnel_outcome(result: Result<(), String>, ok_text: &str) -> Outcome {
    match result {
        Ok(()) => Outcome {
            notes: vec![Note {
                bad: false,
                text: ok_text.to_string(),
            }],
            ..Default::default()
        },
        Err(e) => Outcome::bad(e),
    }
}

/// Обработчик «Поднять туннель» — со свежей проверкой чужого туннеля НЕ
/// только на уровне разметки (кнопка отсутствует, если он поднят), но и
/// здесь: прямой POST в обход кнопки (например, повторно отправленная
/// старая форма) не должен обойти правило «не предлагать подъём», пока
/// чужой туннель несёт наши подсети (задача 3, приёмка задачи 7).
async fn raise_tunnel(state: &SettingsState) -> Outcome {
    let cfg = state.config.load();
    let snap = state
        .tunnel
        .snapshot(&cfg.office_subnets, TUNNEL_PROFILE_NAME);
    // Тот же приоритет, что и в `tunnel_section`: неизвестность
    // (`liveness_error`) НЕ блокирует — раздел в этом случае намеренно
    // показывает кнопку (честное «не знаю» лучше запертых кнопок), а
    // подтверждённая логом поднятость и «поднимается» (round 2 — лог уже
    // подтвердил успех, маршруты вот-вот встанут) делают повторный подъём
    // избыточным, но не опасным. Блокируют только те два случая, где
    // раздел вовсе не рисует кнопку подъёма: неизвестность таблицы
    // маршрутов и обнаруженный на ней туннель, несущий наши подсети, —
    // оба имеют смысл только пока `our_tunnel_up == false` и
    // `rising == false`.
    if snap.liveness_error.is_none() && !snap.our_tunnel_up && !snap.rising {
        if let Some(err) = &snap.routes_error {
            return Outcome::bad(format!(
                "не удалось прочитать таблицу маршрутов: {err} — подъём отменён"
            ));
        }
        if snap.foreign_tunnel_up {
            return Outcome::bad(
                "туннель уже занят другим туннельным адаптером, несущим офисные подсети — подъём отменён",
            );
        }
    }
    tunnel_outcome(
        state.tunnel.raise(TUNNEL_PROFILE_NAME),
        "Команда на подъём туннеля отправлена OpenVPN GUI.",
    )
}

/// Тумблер автозапуска. Возвращает строку для показа, только если есть о чём
/// сказать: молчаливый отказ здесь означал бы галочку, которая ничего не
/// делает и об этом не сообщает.
fn apply_autostart(state: &SettingsState, form: &Form) -> Option<Note> {
    let wanted = form.checked("autostart");
    match state.autostart.is_enabled() {
        // Ничего не меняли — и трогать реестр незачем.
        //
        // Решение, записанное явно (fix round 2, finding F): запись в `Run`,
        // указывающая на СТАРОЕ место exe (перенесли/переустановили), тоже
        // читается как `current == false` — `points_at` в
        // `winnet::autostart` сверяет именно с ЭТИМ исполняемым файлом.
        // Если человек хочет автозапуск ВЫКЛЮЧИТЬ и видит невзведённую
        // галочку (`wanted == false`), он тоже получает `current == wanted`
        // здесь — и мёртвая запись остаётся лежать в `Run` до следующего
        // раза, когда галочку взведут (тогда `Ok(_)` ниже перезапишет её
        // новой). Снять её через этот тумблер нельзя. Мы с этим миримся:
        // мёртвая запись безвредна — она указывает туда, откуда ничего не
        // запустится, — а объявить «включено» и удалить её значило бы
        // соврать о состоянии, которого нет, ровно тем же способом, каким
        // тумблер лгал до критической находки №1, только в обратную
        // сторону. Не путать с недосмотром: это осознанный выбор в пользу
        // честного «не трогаем непонятное» вместо удобного, но лживого
        // «прибрали».
        Ok(current) if current == wanted => None,
        Ok(_) => match state.autostart.set(wanted) {
            Ok(()) => Some(Note {
                bad: false,
                text: if wanted {
                    "Автозапуск включён.".to_string()
                } else {
                    "Автозапуск выключен.".to_string()
                },
            }),
            Err(e) => Some(Note {
                bad: true,
                text: format!("Автозапуск не изменён: {e}"),
            }),
        },
        // Состояние неизвестно. Пытаться что-то записать вслепую нельзя, но
        // и молчать — тоже: человек передвинул галочку и ждёт результата.
        Err(e) => wanted.then(|| Note {
            bad: true,
            text: format!("Автозапуск не изменён: {e}"),
        }),
    }
}

/// Диагностика по нажатию кнопки — единственный путь, где проверки видят
/// по-настоящему ТЕКУЩЕЕ состояние, а не срез момента запуска: порт
/// опрашивается живым подключением, реестр читается заново.
async fn live_checks(state: &SettingsState) -> Vec<Check> {
    let app = state.app.load();
    let cfg = state.config.load();

    // `app.port` — порт, на котором слушатель привязан (см. `live_config`),
    // а не тот, что человек мог вписать в форму минуту назад.
    let listening = matches!(
        tokio::time::timeout(
            PROBE_TIMEOUT,
            tokio::net::TcpStream::connect(("127.0.0.1", app.port)),
        )
        .await,
        Ok(Ok(_))
    );
    let sysproxy = proxypilot_winnet::sysproxy::read().map_err(|e| {
        warn!(error = %e, "диагностика не прочитала системные настройки прокси");
        e.to_string()
    });

    // Второй параметр `run_checks` спрашивает «был ли порт свободен ДО
    // нашего bind» — то есть «не отвечал ли там никто». В живом пути тот же
    // вопрос звучит как «не отвечает ли там никто сейчас», и честный ответ
    // на него — отрицание первого параметра. Подставить сюда `listening`
    // означало бы, что проверка «в реестре наш адрес, но моста нет» кричала
    // бы отказ ровно тогда, когда мост как раз жив.
    //
    // Живой конфиг, а не сохранённый: проверки судят о том, что работает.
    doctor::run_checks(
        &live_config(&cfg, app.port),
        &app,
        listening,
        !listening,
        &sysproxy,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::config::SavedSysProxy;
    use proxypilot_core::mode::{Health, Place};

    fn base() -> Config {
        Config {
            bridge_port: 3129,
            mode: Mode::Socks,
            socks_upstream: Some("203.0.113.10:9999".into()),
            max_connections: 77,
            saved_sysproxy: Some(SavedSysProxy {
                enabled: true,
                server: "было.example:3128".into(),
                bypass: "<local>".into(),
            }),
            ..Default::default()
        }
    }

    fn form(body: &str) -> Form {
        Form::parse(body.as_bytes())
    }

    /// Тело, которое прислал бы браузер при нажатии «Сохранить» без правок.
    fn unchanged_body(port: u16) -> String {
        format!(
            "action=save&bridge_port={port}&socks_upstream=203.0.113.10%3A9999\
             &http_upstream=&no_proxy=localhost&manage_system_proxy=on"
        )
    }

    #[test]
    fn html_metacharacters_are_escaped() {
        // Незакрытая дырка здесь — выполнение чужого кода на странице,
        // которая правит настройки прокси. Кавычки обе: значение попадает и
        // в текст, и в атрибут.
        assert_eq!(
            escape_html("<script>alert(\"1\")</script>"),
            "&lt;script&gt;alert(&quot;1&quot;)&lt;/script&gt;"
        );
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("it's"), "it&#39;s");
        // Амперсанд экранируется первым, иначе он съел бы собственные
        // подстановки соседей: `<` превратился бы в `&amp;lt;`.
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
    }

    #[test]
    fn a_form_is_parsed_with_percent_and_plus_decoding() {
        let f = form("a=1&b=%D0%9E%D1%84%D0%B8%D1%81&c=x+y&empty=&flag");
        assert_eq!(f.get("a"), Some("1"));
        // Имя сети приходит по-русски — процентная кодировка обязана
        // собираться обратно в UTF-8, а не в мусор по байту.
        assert_eq!(f.get("b"), Some("Офис"));
        assert_eq!(f.get("c"), Some("x y"));
        assert_eq!(f.get("empty"), Some(""));
        // Флажок браузер шлёт как `имя=on`, но поле без `=` тоже законно.
        assert!(f.checked("flag"));
        assert!(!f.checked("missing"));
    }

    #[test]
    fn repeated_fields_keep_their_order() {
        // Офисную сеть связывает с её именем только порядок полей.
        let f = form("office_id=A&office_name=Первая&office_id=B&office_name=Вторая");
        assert_eq!(f.all("office_id"), vec!["A", "B"]);
        assert_eq!(f.all("office_name"), vec!["Первая", "Вторая"]);
    }

    #[test]
    fn the_form_does_not_touch_the_fields_it_does_not_own() {
        // `saved_sysproxy` — единственный след настроек пользователя до нас.
        // Стереть его при сохранении формы значит потерять возможность
        // вернуть машине сеть после аварийного завершения.
        let b = base();
        let next = config_from_form(&b, &form(&unchanged_body(3129))).unwrap();
        assert_eq!(next.saved_sysproxy, b.saved_sysproxy);
        // Режим переключает трей, а не эта форма.
        assert_eq!(next.mode, Mode::Socks);
        // Тайминги и предел соединений правятся файлом.
        assert_eq!(next.max_connections, 77);
    }

    #[test]
    fn an_invalid_upstream_is_rejected_by_config_validate() {
        // Правило одно и живёт в `Config::validate`; страница обязана
        // показать ИМЕННО его текст, а не свой пересказ.
        let next = config_from_form(
            &base(),
            &form("action=save&bridge_port=3129&socks_upstream=%D0%B1%D0%B5%D0%B7-%D0%BF%D0%BE%D1%80%D1%82%D0%B0&http_upstream=&no_proxy="),
        )
        .unwrap();
        let err = next
            .validate()
            .expect_err("апстрим без порта обязан быть отвергнут");
        assert!(err.to_string().contains("host:port"), "получили: {err}");
    }

    #[test]
    fn a_privileged_port_is_rejected_by_config_validate() {
        let next = config_from_form(
            &base(),
            &form("action=save&bridge_port=80&socks_upstream=&http_upstream=&no_proxy="),
        )
        .unwrap();
        let err = next
            .validate()
            .expect_err("порт ниже 1024 обязан быть отвергнут");
        assert!(err.to_string().contains("1024"), "получили: {err}");
    }

    #[test]
    fn a_port_that_is_not_a_number_is_reported_not_swallowed() {
        let e = config_from_form(
            &base(),
            &form("action=save&bridge_port=abc&socks_upstream=&http_upstream=&no_proxy="),
        )
        .expect_err("«abc» — не порт");
        assert!(e.contains("abc"), "получили: {e}");
    }

    #[test]
    fn the_live_config_keeps_the_port_the_bridge_is_bound_to() {
        // ЭТО и есть правило, ради которого написана вся задача. Слушатель
        // привязан один раз за жизнь процесса; конфиг, ушедший в супервизор
        // с другим портом, заставил бы `AppState.port` — а через него меню,
        // «скопировать адрес» и диагностику — говорить про порт, на котором
        // никто не слушает.
        let saved = Config {
            bridge_port: 3999,
            ..base()
        };
        let live = live_config(&saved, 3129);
        assert_eq!(live.bridge_port, 3129, "порт нельзя менять на лету");
        // И ничего кроме порта: живой конфиг обязан отличаться от
        // сохранённого ровно одним полем.
        assert_eq!(
            live,
            Config {
                bridge_port: 3129,
                ..saved
            }
        );
    }

    #[test]
    fn empty_office_rows_are_dropped() {
        // На странице всегда есть пустая строка «добавить»; отправленная
        // нетронутой, она не должна превращаться в запись с пустым id —
        // такая запись не совпадёт ни с чем и была бы отвергнута
        // `Config::validate` на ровном месте.
        let next = config_from_form(
            &base(),
            &form(
                "action=save&bridge_port=3129&socks_upstream=&http_upstream=&no_proxy=\
                 &office_id=%7BA%7D&office_name=%D0%9E%D1%84%D0%B8%D1%81\
                 &office_id=&office_name=",
            ),
        )
        .unwrap();
        assert_eq!(next.office_networks.len(), 1);
        assert_eq!(next.office_networks[0].id, "{A}");
        assert_eq!(next.office_networks[0].name, "Офис");
        assert!(next.validate().is_ok(), "пустая строка не должна мешать");
    }

    fn state_with(app: AppState, cfg: Config) -> (SettingsState, mpsc::Receiver<Cmd>) {
        state_with_autostart(app, cfg, Arc::new(AutostartPending))
    }

    /// Тот же `state_with`, но с выбираемой реализацией `Autostart` — для
    /// тестов, которым нужно увидеть страницу и `apply_autostart` в
    /// состоянии, где автозапуск реально работает, а не только в состоянии
    /// «реализации ещё нет» (см. `FakeAutostart`).
    fn state_with_autostart(
        app: AppState,
        cfg: Config,
        autostart: Arc<dyn Autostart>,
    ) -> (SettingsState, mpsc::Receiver<Cmd>) {
        let (tx, rx) = mpsc::channel(4);
        (
            SettingsState {
                app: Arc::new(ArcSwap::from_pointee(app)),
                config: Arc::new(ArcSwap::from_pointee(cfg)),
                commands: tx,
                bound_port: 3129,
                autostart,
                tunnel: Arc::new(FakeTunnel::new(TunnelSnapshot::default())),
            },
            rx,
        )
    }

    fn app_state(port: u16, network_name: Option<&str>) -> AppState {
        AppState {
            mode: Mode::Auto,
            route: Route::Direct,
            demoted: false,
            place: Place {
                in_office: false,
                network: Some("{NET-1}".into()),
                network_name: network_name.map(|s| s.to_string()),
            },
            health: Health {
                socks: Reachability::Up,
                http: Reachability::Down,
            },
            port,
        }
    }

    #[test]
    fn the_page_shows_both_upstreams_with_their_availability() {
        let cfg = Config {
            socks_upstream: Some("203.0.113.10:9999".into()),
            http_upstream: Some("203.0.113.10:3128".into()),
            ..Default::default()
        };
        let (state, _rx) = state_with(app_state(3129, Some("Офис")), cfg);
        let html = render(&state, None);
        assert!(html.contains("203.0.113.10:9999"), "нет адреса SOCKS5");
        assert!(html.contains("203.0.113.10:3128"), "нет адреса HTTP");
        assert!(html.contains("доступен"), "нет индикатора доступности");
        assert!(html.contains("недоступен"), "нет индикатора недоступности");
    }

    #[test]
    fn everything_rendered_into_the_page_is_escaped() {
        // Имя сети приходит из системы — его задал тот, кто поднял точку
        // доступа. Bypass-список приходит из файла, который правят руками.
        // Ни то, ни другое не наше.
        let cfg = Config {
            no_proxy: "\"><script>alert(1)</script>".into(),
            socks_upstream: Some("<b>x</b>:1".into()),
            office_networks: vec![OfficeNetwork {
                id: "{<img src=x>}".into(),
                name: "Офис & Ко".into(),
            }],
            ..Default::default()
        };
        let (state, _rx) = state_with(app_state(3129, Some("<script>alert('сеть')</script>")), cfg);
        let html = render(&state, None);
        assert!(
            !html.contains("<script>alert"),
            "неэкранированный скрипт в разметке"
        );
        assert!(!html.contains("<img src=x>"), "неэкранированный тег");
        assert!(html.contains("&lt;script&gt;"), "экранированного вида нет");
        assert!(html.contains("Офис &amp; Ко"), "амперсанд не экранирован");
    }

    #[test]
    fn the_page_says_the_port_needs_a_restart() {
        let (state, _rx) = state_with(app_state(3129, None), Config::default());
        let html = render(&state, None);
        assert!(
            html.contains("перезапуст"),
            "страница молчит о том, что порт применится только после перезапуска"
        );
    }

    #[test]
    fn the_page_offers_the_office_button_only_when_a_network_is_known() {
        let (with, _a) = state_with(app_state(3129, Some("Офис")), Config::default());
        assert!(render(&with, None).contains("value=\"office\""));

        let mut unknown = app_state(3129, None);
        unknown.place.network = None;
        let (without, _b) = state_with(unknown, Config::default());
        assert!(
            !render(&without, None).contains("value=\"office\""),
            "нечего добавлять — кнопки быть не должно"
        );
    }

    #[test]
    fn a_failed_route_is_shown_as_failed_not_omitted() {
        // Пропущенная строка выглядела бы как «не настроен», а это другое:
        // первое чинится сетью, второе — настройкой.
        let outcome = Outcome {
            bench: Some(vec![
                BenchResult {
                    label: "Напрямую".into(),
                    route: Route::Direct,
                    bytes: 1_048_576,
                    elapsed: Duration::from_millis(1000),
                    error: None,
                },
                BenchResult {
                    label: "SOCKS5".into(),
                    route: Route::Socks("10.0.0.2:9999".into()),
                    bytes: 0,
                    elapsed: Duration::from_millis(300),
                    error: Some("<отказ набора>".into()),
                },
            ]),
            ..Default::default()
        };
        let (state, _rx) = state_with(app_state(3129, None), Config::default());
        let html = render(&state, Some(&outcome));
        assert!(html.contains("Напрямую"), "быстрый маршрут не показан");
        assert!(html.contains("SOCKS5"), "мёртвый маршрут пропущен");
        assert!(html.contains("быстрее прочих"), "победитель не отмечен");
        // Текст ошибки приходит из сети (адрес апстрима, ответ сервера) —
        // экранируется наравне со всем остальным.
        assert!(
            html.contains("&lt;отказ набора&gt;"),
            "ошибка не экранирована"
        );
        assert!(html.contains("1.00 МБ/с"), "скорость не показана: {html}");
    }

    #[test]
    fn diagnostics_output_is_shown_in_place_and_escaped() {
        let outcome = Outcome {
            doctor: Some(vec![Check {
                title: "Мост слушает свой порт".into(),
                status: CheckStatus::Fail,
                detail: "не отвечает <b>вовсе</b>".into(),
            }]),
            ..Default::default()
        };
        let (state, _rx) = state_with(app_state(3129, None), Config::default());
        let html = render(&state, Some(&outcome));
        assert!(html.contains("Мост слушает свой порт"));
        assert!(html.contains("не отвечает &lt;b&gt;вовсе&lt;/b&gt;"));
    }

    #[test]
    fn the_autostart_toggle_says_it_is_not_wired_yet_instead_of_pretending() {
        // Регрессия на честность рендера: если `Autostart::is_enabled`
        // вернул ошибку (в проде так было до задачи 6; здесь эту роль
        // играет тестовая заглушка `AutostartPending`), тумблер обязан
        // показать текст ошибки и быть заблокирован, а не притворяться
        // рабочим. Галочка, которая ничего не делает и молчит об этом,
        // хуже её отсутствия.
        let (state, _rx) = state_with(app_state(3129, None), Config::default());
        let html = render(&state, None);
        assert!(html.contains("Запускать вместе с Windows"));
        assert!(html.contains("автозапуск ещё не подключён"), "тумблер врёт");
        assert!(html.contains("disabled"), "тумблер не заблокирован");
    }

    #[test]
    fn the_autostart_toggle_reflects_a_working_enabled_state_without_being_disabled() {
        // Обратная сторона предыдущего теста: до `FakeAutostart` тесты
        // страницы видели только реализацию, которая всегда отказывает, и
        // рендер РАБОТАЮЩЕГО, включённого тумблера не проверялся ни разу.
        let (state, _rx) = state_with_autostart(
            app_state(3129, None),
            Config::default(),
            Arc::new(FakeAutostart::new(true)),
        );
        let html = render(&state, None);
        assert!(
            html.contains("<input type=\"checkbox\" name=\"autostart\" checked>"),
            "тумблер обязан быть отмечен и не задизейблен: {html}"
        );
        assert!(
            !html.contains("автозапуск ещё не подключён"),
            "рабочая реализация не должна показывать текст заглушки"
        );
    }

    #[test]
    fn apply_autostart_does_nothing_when_the_checkbox_already_matches_reality() {
        // Ветка `Ok(current) if current == wanted => None` — без рабочей
        // реализации `Autostart` в тестах до неё нельзя было даже дойти:
        // `is_enabled()` всегда возвращал `Err`.
        let (state, _rx) = state_with_autostart(
            app_state(3129, None),
            Config::default(),
            Arc::new(FakeAutostart::new(true)),
        );
        let form = Form::parse(b"autostart=on");
        let note = apply_autostart(&state, &form);
        assert!(note.is_none(), "менять нечего — заметки быть не должно");
        assert!(
            state.autostart.is_enabled().unwrap(),
            "«менять нечего» не должно было ничего изменить"
        );
    }

    #[test]
    fn apply_autostart_turns_it_on_and_reports_success() {
        let (state, _rx) = state_with_autostart(
            app_state(3129, None),
            Config::default(),
            Arc::new(FakeAutostart::new(false)),
        );
        let form = Form::parse(b"autostart=on");
        let note = apply_autostart(&state, &form).expect("изменение обязано дать заметку");
        assert!(!note.bad, "успешное изменение — не ошибка: {}", note.text);
        assert!(note.text.contains("включён"), "получили: {}", note.text);
        assert!(
            state.autostart.is_enabled().unwrap(),
            "set(true) обязан был реально сработать, а не только вернуть Ok"
        );
    }

    #[test]
    fn apply_autostart_turns_it_off_and_reports_success() {
        let (state, _rx) = state_with_autostart(
            app_state(3129, None),
            Config::default(),
            Arc::new(FakeAutostart::new(true)),
        );
        let form = Form::parse(b""); // галочка снята
        let note = apply_autostart(&state, &form).expect("изменение обязано дать заметку");
        assert!(!note.bad, "успешное изменение — не ошибка: {}", note.text);
        assert!(note.text.contains("выключен"), "получили: {}", note.text);
        assert!(
            !state.autostart.is_enabled().unwrap(),
            "set(false) обязан был реально сработать, а не только вернуть Ok"
        );
    }

    #[test]
    fn apply_autostart_reports_the_underlying_error_when_set_fails() {
        let (state, _rx) = state_with_autostart(
            app_state(3129, None),
            Config::default(),
            Arc::new(FakeAutostart::failing_to_set(false)),
        );
        let form = Form::parse(b"autostart=on");
        let note = apply_autostart(&state, &form).expect("отказ тоже обязан дать заметку");
        assert!(note.bad);
        assert!(
            note.text.contains("тестовый отказ записи"),
            "получили: {}",
            note.text
        );
    }

    #[test]
    fn apply_autostart_says_nothing_when_state_is_unknown_and_the_box_stays_unchecked() {
        // Ветка `Err(e) => wanted.then(...)`: состояние неизвестно, но
        // человек и не просил его менять — тревожить его нечем.
        let (state, _rx) = state_with(app_state(3129, None), Config::default());
        let form = Form::parse(b"");
        assert!(apply_autostart(&state, &form).is_none());
    }

    #[test]
    fn apply_autostart_reports_when_state_is_unknown_and_the_box_is_checked() {
        let (state, _rx) = state_with(app_state(3129, None), Config::default());
        let form = Form::parse(b"autostart=on");
        let note = apply_autostart(&state, &form).expect("человек ждёт результата");
        assert!(note.bad);
    }

    // ---- Задача 7: раздел «Туннель» ----

    struct FakeTunnel {
        snapshot: TunnelSnapshot,
        build_result: Result<(), String>,
        raise_result: Result<(), String>,
        lower_result: Result<(), String>,
        install_result: Result<(), String>,
    }

    impl FakeTunnel {
        fn new(snapshot: TunnelSnapshot) -> Self {
            Self {
                snapshot,
                build_result: Ok(()),
                raise_result: Ok(()),
                lower_result: Ok(()),
                install_result: Ok(()),
            }
        }

        fn failing_to_build(snapshot: TunnelSnapshot, err: &str) -> Self {
            Self {
                build_result: Err(err.to_string()),
                ..Self::new(snapshot)
            }
        }
    }

    impl Tunnel for FakeTunnel {
        fn snapshot(&self, _office_subnets: &[Ipv4Net], _profile_name: &str) -> TunnelSnapshot {
            self.snapshot.clone()
        }
        fn build_profile(
            &self,
            _profile_name: &str,
            _office_subnets: &[Ipv4Net],
        ) -> Result<(), String> {
            self.build_result.clone()
        }
        fn raise(&self, _profile_name: &str) -> Result<(), String> {
            self.raise_result.clone()
        }
        fn lower(&self, _profile_name: &str) -> Result<(), String> {
            self.lower_result.clone()
        }
        fn install_service(&self) -> Result<(), String> {
            self.install_result.clone()
        }
    }

    fn down_installed_snapshot() -> TunnelSnapshot {
        TunnelSnapshot {
            installed: true,
            profile_installed: true,
            our_tunnel_up: false,
            rising: false,
            liveness_error: None,
            foreign_tunnel_up: false,
            routes_error: None,
        }
    }

    fn state_with_tunnel(
        app: AppState,
        cfg: Config,
        tunnel: FakeTunnel,
    ) -> (SettingsState, mpsc::Receiver<Cmd>) {
        let (tx, rx) = mpsc::channel(4);
        (
            SettingsState {
                app: Arc::new(ArcSwap::from_pointee(app)),
                config: Arc::new(ArcSwap::from_pointee(cfg)),
                commands: tx,
                bound_port: 3129,
                autostart: Arc::new(AutostartPending),
                tunnel: Arc::new(tunnel),
            },
            rx,
        )
    }

    #[test]
    fn tunnel_section_explains_when_openvpn_is_not_installed() {
        // Приёмка: раздел неактивен с внятным объяснением, а не просто серый.
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(TunnelSnapshot::default()),
        );
        let html = render(&state, None);
        assert!(html.contains("OpenVPN не найден"), "получили: {html}");
        assert!(!html.contains("value=\"raise_tunnel\""));
        assert!(!html.contains("value=\"build_tunnel_profile\""));
        assert!(!html.contains("value=\"install_service\""));
    }

    #[test]
    fn tunnel_section_shows_a_down_tunnel_with_a_raise_button() {
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(down_installed_snapshot()),
        );
        let html = render(&state, None);
        assert!(html.contains("Туннель опущен"), "получили: {html}");
        assert!(html.contains("value=\"raise_tunnel\""));
        assert!(!html.contains("value=\"lower_tunnel\""));
    }

    #[test]
    fn tunnel_section_hides_the_raise_button_until_a_profile_is_built() {
        let snap = TunnelSnapshot {
            profile_installed: false,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = render(&state, None);
        assert!(!html.contains("value=\"raise_tunnel\""));
        assert!(
            html.contains("Сначала соберите профиль"),
            "получили: {html}"
        );
    }

    #[test]
    fn tunnel_section_shows_our_tunnel_up_with_a_lower_button() {
        let snap = TunnelSnapshot {
            our_tunnel_up: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = render(&state, None);
        assert!(html.contains("Туннель поднят"), "получили: {html}");
        assert!(html.contains("value=\"lower_tunnel\""));
        assert!(!html.contains("value=\"raise_tunnel\""));
    }

    #[test]
    fn tunnel_section_refuses_to_offer_raising_over_a_foreign_tunnel() {
        // Приёмка: если поднят чужой туннель в наши сети — показать это и
        // не предлагать подъём (задача 3, задача 7).
        let snap = TunnelSnapshot {
            foreign_tunnel_up: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = render(&state, None);
        assert!(
            html.contains("уже несёт офисные подсети") || html.contains("занят"),
            "получили: {html}"
        );
        assert!(!html.contains("value=\"raise_tunnel\""));
        assert!(!html.contains("value=\"lower_tunnel\""));
    }

    #[test]
    fn our_confirmed_up_tunnel_wins_over_a_misclassified_foreign_reading() {
        // Регрессия на fix round 1: до исправления `our_tunnel_up`
        // определялась по имени адаптера, которое НИКОГДА не совпадает
        // (round 1 отчёта задачи 7) — из-за этого свой же поднятый туннель
        // читался ещё и как `foreign_tunnel_up == true` (тот же адаптер,
        // тот же маршрут, алиас не совпал), и раздел показывал «занято»
        // вместо кнопки «опустить» — то есть НЕЛЬЗЯ было опустить туннель,
        // который сам же и подняли. Теперь `our_tunnel_up` (лог, ключ —
        // имя профиля) проверяется раньше `foreign_tunnel_up` (таблица
        // маршрутов + алиас адаптера) — оба поля здесь одновременно true,
        // намеренно, как и было бы в реальности до исправления.
        let snap = TunnelSnapshot {
            our_tunnel_up: true,
            foreign_tunnel_up: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = render(&state, None);
        assert!(html.contains("Туннель поднят"), "получили: {html}");
        assert!(
            html.contains("value=\"lower_tunnel\""),
            "кнопка опустить обязана быть доступна для своего же поднятого туннеля: {html}"
        );
        assert!(!html.contains("value=\"raise_tunnel\""));
    }

    #[test]
    fn tunnel_section_names_the_rising_window_distinctly_from_down_or_up() {
        // Round 2: лог уже подтвердил успех, а маршруты профиля ещё не
        // встали — короткое окно сразу после «Поднять туннель». Не
        // «опущен» (выглядело бы как приглашение нажать «Поднять» ещё
        // раз, породив второй connect).
        let snap = TunnelSnapshot {
            rising: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = render(&state, None);
        assert!(html.contains("поднимается"), "получили: {html}");
        assert!(!html.contains("Туннель опущен"), "получили: {html}");
        assert!(
            !html.contains("<p class=\"note good\">Туннель поднят.</p>"),
            "получили: {html}"
        );
        assert!(!html.contains("value=\"raise_tunnel\""));
        // «Опустить» доступна как отмена ещё не завершившегося подъёма.
        assert!(html.contains("value=\"lower_tunnel\""));
    }

    #[test]
    fn rising_wins_over_a_misclassified_occupied_reading() {
        let snap = TunnelSnapshot {
            rising: true,
            foreign_tunnel_up: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = render(&state, None);
        assert!(html.contains("поднимается"), "получили: {html}");
        assert!(
            !html.contains("Туннель опущен, но какой-то"),
            "получили: {html}"
        );
    }

    #[tokio::test]
    async fn raising_the_tunnel_is_not_refused_server_side_while_rising() {
        // «Поднимается» уже подтверждено логом — раздел не рисует кнопку
        // «Поднять» в этом состоянии вовсе (см. рендер-тесты выше), но
        // прямой POST в обход разметки не обязан отвечать «занято чужим»:
        // `foreign_tunnel_up` здесь заведомо ложное срабатывание (тот же
        // наш адаптер, маршруты которого ещё не осели) — то самое
        // избыточное, но безвредное действие, которое round 1 уже признал
        // нормальным для подтверждённых состояний.
        let snap = TunnelSnapshot {
            rising: true,
            foreign_tunnel_up: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = handle_post(&state, b"action=raise_tunnel").await;
        assert!(
            !html.contains("подъём отменён"),
            "«поднимается» не обязано отменять подъём: {html}"
        );
    }

    #[test]
    fn an_unknown_liveness_shows_both_buttons_instead_of_locking_the_section() {
        // Приёмка коррекции: раздел, признающий «не знаю», лучше раздела,
        // который молча запирает обе кнопки.
        let snap = TunnelSnapshot {
            liveness_error: Some("тестовый отказ чтения лога".to_string()),
            // routes/foreign — заведомо в состоянии, которое БЫ заблокировало
            // подъём в обычном приоритете; проверяем, что liveness_error
            // перевешивает и это тоже.
            foreign_tunnel_up: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = render(&state, None);
        assert!(html.contains("состояние неизвестно"), "получили: {html}");
        assert!(html.contains("value=\"raise_tunnel\""), "получили: {html}");
        assert!(html.contains("value=\"lower_tunnel\""), "получили: {html}");
    }

    #[test]
    fn the_dns_caveat_is_shown_before_the_tunnel_is_ever_raised() {
        // Приёмка: пользователь узнаёт об этом до подъёма, а не догадывается.
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(down_installed_snapshot()),
        );
        let html = render(&state, None);
        let dns_pos = html
            .find("DNS-запрос")
            .expect("предупреждение про DNS обязано быть на странице");
        let raise_pos = html
            .find("value=\"raise_tunnel\"")
            .expect("кнопка подъёма обязана быть на странице");
        assert!(
            dns_pos < raise_pos,
            "предупреждение про DNS обязано стоять до кнопки подъёма"
        );
    }

    #[test]
    fn a_uac_warning_appears_before_both_privileged_buttons() {
        // Приёмка: явное предупреждение про UAC у обеих кнопок, ДО нажатия.
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(down_installed_snapshot()),
        );
        let html = render(&state, None);
        let build_pos = html
            .find("value=\"build_tunnel_profile\"")
            .expect("кнопка сборки обязана быть на странице");
        let install_pos = html
            .find("value=\"install_service\"")
            .expect("кнопка установки службы обязана быть на странице");
        let uac_pos = html
            .find("UAC")
            .expect("предупреждение про UAC обязано быть на странице");
        assert!(
            uac_pos < build_pos && uac_pos < install_pos,
            "предупреждение обязано стоять до обеих кнопок"
        );
    }

    #[test]
    fn the_routes_error_disables_both_tunnel_buttons_and_is_escaped() {
        let snap = TunnelSnapshot {
            routes_error: Some("<script>bad</script>".to_string()),
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = render(&state, None);
        assert!(
            !html.contains("<script>bad"),
            "неэкранированный скрипт в разметке"
        );
        assert!(html.contains("&lt;script&gt;"), "экранированного вида нет");
        assert!(!html.contains("value=\"raise_tunnel\""));
        assert!(!html.contains("value=\"lower_tunnel\""));
    }

    #[test]
    fn office_subnets_are_listed_and_escaped() {
        let cfg = Config {
            office_subnets: vec!["203.0.113.0/24".parse().unwrap()],
            ..Default::default()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            cfg,
            FakeTunnel::new(down_installed_snapshot()),
        );
        let html = render(&state, None);
        assert!(html.contains("203.0.113.0/24"), "получили: {html}");
    }

    #[test]
    fn the_automate_tunnel_toggle_is_off_by_default() {
        // Приёмка: тумблер автоматики по умолчанию выключен (спека 8.5).
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(down_installed_snapshot()),
        );
        let html = render(&state, None);
        assert!(
            html.contains("name=\"automate_tunnel\""),
            "получили: {html}"
        );
        assert!(!html.contains("name=\"automate_tunnel\" checked"));
    }

    #[test]
    fn the_automate_tunnel_toggle_can_be_turned_on_and_saved() {
        let cfg = Config {
            automate_tunnel: true,
            ..Default::default()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            cfg,
            FakeTunnel::new(down_installed_snapshot()),
        );
        let html = render(&state, None);
        assert!(
            html.contains("name=\"automate_tunnel\" checked"),
            "получили: {html}"
        );
    }

    #[test]
    fn automate_tunnel_survives_config_from_form() {
        let next = config_from_form(
            &base(),
            &form(&format!("{}&automate_tunnel=on", unchanged_body(3129))),
        )
        .unwrap();
        assert!(next.automate_tunnel);
    }

    #[tokio::test]
    async fn a_failed_profile_build_is_shown_not_swallowed() {
        // Приёмка: ошибка build_profile доходит до пользователя, не
        // проглатывается.
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::failing_to_build(
                down_installed_snapshot(),
                "незакрытый inline-блок «<ca>» (строка 2)",
            ),
        );
        let html = handle_post(&state, b"action=build_tunnel_profile").await;
        assert!(
            html.contains("незакрытый inline-блок"),
            "ошибка build_profile обязана быть показана дословно: {html}"
        );
    }

    #[tokio::test]
    async fn raising_the_tunnel_is_refused_server_side_when_a_foreign_tunnel_is_up() {
        // Защита не только на уровне разметки (кнопки нет), но и на уровне
        // обработчика: прямой POST в обход кнопки тоже обязан быть отвергнут.
        let snap = TunnelSnapshot {
            foreign_tunnel_up: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = handle_post(&state, b"action=raise_tunnel").await;
        assert!(
            html.contains("занят другим туннельным адаптером"),
            "получили: {html}"
        );
    }

    #[tokio::test]
    async fn raising_the_tunnel_is_not_refused_when_liveness_is_merely_unknown() {
        // Обратная сторона предыдущего теста: неизвестность НЕ должна
        // блокировать так же, как обнаруженный чужой туннель, — иначе
        // «честное “не знаю”» на странице было бы ложью, если сервер за
        // кулисами всё равно отказывает.
        let snap = TunnelSnapshot {
            liveness_error: Some("тестовый отказ чтения лога".to_string()),
            foreign_tunnel_up: true,
            ..down_installed_snapshot()
        };
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(snap),
        );
        let html = handle_post(&state, b"action=raise_tunnel").await;
        assert!(
            !html.contains("подъём отменён"),
            "неизвестность не обязана отменять подъём: {html}"
        );
    }

    #[tokio::test]
    async fn raising_the_tunnel_succeeds_and_reports_so() {
        let (state, _rx) = state_with_tunnel(
            app_state(3129, None),
            Config::default(),
            FakeTunnel::new(down_installed_snapshot()),
        );
        let html = handle_post(&state, b"action=raise_tunnel").await;
        assert!(!html.contains("class=\"note bad\""), "получили: {html}");
    }
}
