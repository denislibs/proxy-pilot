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

    // Форма настроек. Отдельная от кнопок замера и диагностики: те не
    // должны тащить с собой поля, которые человек ещё правит.
    b.push_str("<form method=\"post\" action=\"\">\n");

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
        &health_text(cfg.socks_upstream.as_deref(), app.health.socks),
    ));
    b.push_str(&field(
        "http_upstream",
        "HTTP-прокси",
        cfg.http_upstream.as_deref().unwrap_or(""),
        &health_text(cfg.http_upstream.as_deref(), app.health.http),
    ));

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

    b.push_str("<h2 id=\"networks\">Офисные сети</h2>\n");
    b.push_str(
        "<p class=\"hint\">Применяется сразу. Решение принимается по GUID: имя \
         сети человек может переименовать в любой момент. Чтобы убрать сеть — \
         очистите её GUID и сохраните.</p>\n",
    );
    b.push_str("<table>\n<tr><th>GUID</th><th>Имя</th></tr>\n");
    for net in cfg.office_networks.iter() {
        b.push_str(&office_row(&net.id, &net.name));
    }
    // Пустая строка для добавления руками. Пустой id отбрасывается при
    // разборе, поэтому нетронутая строка ничего не портит.
    b.push_str(&office_row("", ""));
    b.push_str("</table>\n");

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

    b.push_str("<p class=\"buttons\">");
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
    b.push_str("</p>\n</form>\n");

    // Замер и диагностика — отдельными формами: их кнопки не должны
    // отправлять поля настроек, которые человек ещё правит.
    b.push_str("<h2 id=\"bench\">Замер скорости</h2>\n");
    b.push_str(&format!(
        "<p class=\"hint\">Один поток и короткий файл: цифры сравнивают \
         маршруты между собой, а не измеряют канал. Качается {}.</p>\n",
        escape_html(BENCH_URL)
    ));
    b.push_str(
        "<form method=\"post\" action=\"\"><button type=\"submit\" name=\"action\" \
         value=\"bench\">Замерить</button></form>\n",
    );
    if let Some(results) = outcome.and_then(|o| o.bench.as_ref()) {
        b.push_str(&bench_table(results));
    }

    b.push_str("<h2 id=\"doctor\">Диагностика</h2>\n");
    b.push_str(
        "<form method=\"post\" action=\"\"><button type=\"submit\" name=\"action\" \
         value=\"doctor\">Проверить</button></form>\n",
    );
    if let Some(checks) = outcome.and_then(|o| o.doctor.as_ref()) {
        b.push_str(&doctor_table(checks));
    }

    b.push_str("</body></html>\n");
    b
}

const STYLE: &str = "\
body{font:14px/1.5 'Segoe UI',system-ui,sans-serif;margin:2rem auto;max-width:46rem;padding:0 1rem}\
h1{font-size:1.4rem}h2{font-size:1.05rem;margin-top:1.8rem;border-bottom:1px solid #ccc;padding-bottom:.2rem}\
label{display:block;margin-top:.8rem}\
input[type=text],textarea{width:100%;box-sizing:border-box;font-family:inherit}\
table{border-collapse:collapse;width:100%}td,th{text-align:left;padding:.2rem .4rem 0 0}\
.now{background:#f3f3f3;padding:.6rem;border-radius:4px}\
.hint,.warn,.note{margin:.4rem 0}.hint{color:#555}.warn{color:#7a4a00}\
.note{padding:.5rem;border-radius:4px}.note.good{background:#e6f4e6}.note.bad{background:#fbe6e6}\
.buttons{margin-top:1.4rem}button{margin-right:.6rem;padding:.4rem .9rem}\
.ok{color:#1a6b1a}.fail{color:#a11}.pre{white-space:pre-wrap;font-family:Consolas,monospace}";

fn field(name: &str, label: &str, value: &str, note: &str) -> String {
    let note = if note.is_empty() {
        String::new()
    } else {
        format!(" <span class=\"hint\">{}</span>", escape_html(note))
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

fn bench_table(results: &[BenchResult]) -> String {
    let best = fastest(results).map(|r| r.label.clone());
    let mut out = String::from("<table>\n<tr><th>Маршрут</th><th>Скорость</th><th></th></tr>\n");
    for r in results {
        let speed = match r.speed_bps() {
            Some(bps) => format!("{:.2} МБ/с", bps as f64 / 1_048_576.0),
            None => "—".to_string(),
        };
        // Путь, который не отработал, показывается как не отработавший, а не
        // пропускается: пропущенная строка выглядела бы как «не настроен».
        let note = match &r.error {
            Some(e) => format!("<span class=\"fail\">{}</span>", escape_html(e)),
            None if best.as_deref() == Some(r.label.as_str()) => {
                "<span class=\"ok\">быстрее прочих</span>".to_string()
            }
            None => format!("{} байт", r.bytes),
        };
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            escape_html(&r.label),
            escape_html(&speed),
            note
        ));
    }
    out.push_str("</table>\n");
    out
}

fn doctor_table(checks: &[Check]) -> String {
    let mut out = String::from("<table>\n");
    for c in checks {
        let (cls, mark) = match c.status {
            CheckStatus::Ok => ("ok", "ок"),
            CheckStatus::Warn => ("warn", "внимание"),
            CheckStatus::Fail => ("fail", "отказ"),
        };
        out.push_str(&format!(
            "<tr><td class=\"{cls}\">{mark}</td><td><b>{title}</b><div class=\"pre\">\
             {detail}</div></td></tr>\n",
            title = escape_html(&c.title),
            detail = escape_html(&c.detail),
        ));
    }
    out.push_str("</table>\n");
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
}
