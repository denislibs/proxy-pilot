//! Страница настроек: транспорт.
//!
//! Приложение отдаёт свою страницу настроек само, на отдельном слушателе, а
//! показывает её браузер пользователя. Причина не в экономии: главный поток
//! уже крутит цикл сообщений, которым владеют `tray-icon` и `muda` (последняя
//! ставит на то же окно свой `SetWindowSubclass`), и второй GUI-фреймворк
//! пришёл бы со своими предположениями об этом цикле и об этом подклассе —
//! ровно тот стык, где ошибки не воспроизводятся. Асинхронный стек уже есть,
//! поэтому страница стоит десятков строк и ни одной новой зависимости в
//! дереве, которое придётся подписывать.
//!
//! Здесь — только транспорт: привязка, токен, маршрутизация, проверка
//! источника запроса и таймаут бездействия. Содержимое страницы (HTML, форма,
//! валидация, применение изменений) живёт в [`crate::settings_page`] и об
//! этом файле ничего не знает; всё, что здесь, от него тоже не зависит —
//! кроме двух вызовов в `serve_one`.
//!
//! # От чего это защищает, а от чего нет
//!
//! Слушатель на loopback доступен ЛЮБОМУ процессу, работающему под тем же
//! пользователем. Токен этого не меняет и не может изменить: такой процесс
//! и так читает `config.toml`, и так может подменить сам исполняемый файл, а
//! при желании — прочитать наш адрес из таблицы соединений и подсмотреть
//! токен в нашей же памяти. Считать токен защитой от локального
//! злоумышленника с правами пользователя значит обещать то, чего здесь нет.
//!
//! Токен защищает от другого, и это «другое» — реальные случаи:
//!
//! - от браузера или другой программы, зашедшей на угаданный адрес: портов
//!   на loopback немного, и перебрать их дешевле, чем кажется;
//! - от страницы, открытой у пользователя в браузере: она не видит наших
//!   ответов из-за политики одного источника, но послать запрос — например,
//!   форму — может. Токена она не знает, а без него получает `404`;
//! - от перепривязки DNS, когда чужое имя резолвится в `127.0.0.1` и
//!   политика одного источника перестаёт мешать: тогда работает ещё и
//!   проверка `Host` ниже.
//!
//! Поэтому на запрос без верного токена отвечаем `404`, а не `403`: `403`
//! означает «здесь что-то есть, но тебе нельзя», то есть подтверждает
//! существование сервера тому, кто его только нащупывает.
//!
//! Цена того, что токен лежит в ПУТИ адреса, а не в заголовке: адрес целиком
//! попадает в историю браузера, а оттуда — возможно, и в синхронизацию
//! профиля вместе с подсказками адресной строки. Смягчает это ровно одно, и
//! сказать об этом честнее, чем промолчать: токен умирает вместе с сеансом
//! окна. Запись в истории остаётся, но ключ, на который она указывает, уже
//! ни от чего не подходит — это проверено тестом
//! `a_token_from_a_previous_session_is_not_found`.
//!
//! «Одноразовый» токен здесь означает «один на сеанс окна», а не «сгорает на
//! первом запросе»: одна страница — это уже несколько запросов (сама
//! страница, отправка формы, перезагрузка), и токен, сгорающий на первом,
//! сломал бы ровно то, ради чего заведён. Новый сервер — новый токен, а
//! старый после остановки не подходит никуда.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use proxypilot_bridge::http::{read_head, Head, HeadError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use windows::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};

use crate::settings_page::{self, SettingsState};

/// Сколько сервер живёт без обращений. Постоянно открытая дверь в настройки
/// ни к чему: окно настроек — это разовый визит, а не фоновая служба.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Длина токена. 32 байта из системного ГСЧ — заведомо больше, чем нужно,
/// чтобы перебор по сети (пусть и по петле) не имел смысла, и достаточно
/// коротко, чтобы адрес влезал в строку браузера.
const TOKEN_BYTES: usize = 32;

/// Потолок заголовка запроса. То же значение, что у моста: браузерный GET с
/// куками в него укладывается с запасом.
const MAX_HEAD: usize = 8192;

/// Потолок тела формы. Настройки — это несколько строк; всё, что больше,
/// прислано не нашей страницей.
const MAX_BODY: usize = 64 * 1024;

/// Сколько ждём одно соединение целиком. Браузеры открывают сокеты заранее и
/// молчат в них; без потолка такой сокет висел бы до конца сеанса.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Сколько соединений обслуживаем одновременно.
///
/// Размер заголовка, размер тела и время жизни соединения ограничены каждый
/// по отдельности — а их количество без этой константы не ограничивалось
/// ничем, и это была единственная ось, по которой ресурсы кончались бы молча.
/// Мост ограничивает себя ровно по этой же причине (`serve::Limits`).
/// Браузер открывает на одну страницу несколько сокетов, но не десятки: 32 —
/// это с запасом для честной работы и потолок для всего остального.
const MAX_CONNECTIONS: usize = 32;

/// Сколько подряд отказов `accept` готовы пережить, прежде чем счесть
/// слушатель сломанным. Мост в такой ситуации обязан терпеть до последнего —
/// на него направлен системный прокси; здесь же цена ошибки другая:
/// не открылась страница настроек, и лучше честно закрыться, чем крутить
/// пустой цикл на ядре.
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 64;

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("не занять loopback-порт для страницы настроек: {0}")]
    Bind(std::io::Error),
    #[error("системный источник случайности отказал: NTSTATUS {0:#010x}")]
    Random(i32),
}

/// Адрес, который надо открыть в браузере. Токен — часть пути, поэтому это
/// не «адрес сервера», а одноразовый ключ от него; в лог такая строка не
/// пишется (лог живёт на диске дольше сеанса).
#[derive(Debug, Clone)]
pub struct SettingsUrl {
    pub url: String,
}

/// Общее для всех соединений одного сеанса.
struct Inner {
    /// Ключ от этого сеанса. Сравнивается только через [`constant_time_eq`].
    token: String,
    /// Наш собственный порт — он же то, чем «свой» источник отличается от
    /// чужого в заголовках `Origin`/`Referer`/`Host`.
    port: u16,
    state: Arc<SettingsState>,
    /// Момент последнего обращения С ВЕРНЫМ ТОКЕНОМ. Именно с верным:
    /// иначе любой процесс, не знающий токена, держал бы дверь в настройки
    /// открытой сколько угодно, просто стуча в неё.
    last_seen: Mutex<Instant>,
}

impl Inner {
    fn touch(&self) {
        *self.last_seen() = Instant::now();
    }

    fn last_seen(&self) -> std::sync::MutexGuard<'_, Instant> {
        // Отравить этот мьютекс может только паника обработчика, случившаяся
        // ровно между двумя строками присваивания момента времени. Момент
        // времени не бывает «испорченным наполовину», поэтому забирать его
        // из отравленного мьютекса безопасно, а паниковать здесь значило бы
        // уронить сервер настроек из-за чужой паники.
        self.last_seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Сервер настроек. Живёт, пока жив этот объект — и не дольше.
///
/// Остановка происходит по любому из трёх поводов, и все три ведут в одну
/// точку: цикл приёма выходит и слушатель разрушается вместе с ним.
///
/// - [`Server::stop`] — окно закрыли явно;
/// - `drop` — владелец исчез, в том числе при выходе из приложения;
/// - таймаут бездействия.
///
/// Уже принятое соединение при этом доигрывается до конца: новых слушатель
/// не примет, а рвать ответ на полуслове ради миллисекунды закрытия незачем.
pub struct Server {
    url: SettingsUrl,
    /// Он же признак жизни: цикл приёма держит приёмник этого канала, и
    /// когда цикл вышел — по любой из трёх причин, — приёмников не остаётся.
    stop: watch::Sender<bool>,
}

impl Server {
    /// Поднимает сервер на свободном порту loopback и выдаёт адрес с токеном.
    pub async fn start(state: Arc<SettingsState>) -> Result<Self, SettingsError> {
        Self::start_with_idle(state, IDLE_TIMEOUT).await
    }

    /// То же, но с явным таймаутом бездействия — так его можно проверить
    /// тестом, не ожидая четверти часа.
    pub async fn start_with_idle(
        state: Arc<SettingsState>,
        idle: Duration,
    ) -> Result<Self, SettingsError> {
        let token = random_token()?;

        // Строго loopback и порт 0. Loopback — потому что это сервер, который
        // ПРАВИТ настройки, и на `0.0.0.0` он был бы такой дверью для всей
        // локальной сети. Порт 0 — потому что фиксированный занимал бы чужой,
        // отказывал при повторном открытии окна и был бы предсказуем; систему
        // просят выдать свободный.
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let listener = TcpListener::bind(addr).await.map_err(SettingsError::Bind)?;
        let local = listener.local_addr().map_err(SettingsError::Bind)?;
        let port = local.port();

        // Адрес — в лог, токен — нет: лог лежит на диске дольше, чем живёт
        // сеанс, и ключ от настроек в нём остался бы навсегда.
        info!(port, "сервер настроек слушает на 127.0.0.1");

        let url = SettingsUrl {
            url: format!("http://127.0.0.1:{port}/{token}"),
        };
        let inner = Arc::new(Inner {
            token,
            port,
            state,
            last_seen: Mutex::new(Instant::now()),
        });
        let (stop, stop_rx) = watch::channel(false);
        tokio::spawn(accept_loop(listener, inner, idle, stop_rx));

        Ok(Self { url, stop })
    }

    pub fn url(&self) -> &SettingsUrl {
        &self.url
    }

    /// Жив ли ещё цикл приёма. Сервер мог погаснуть сам — по таймауту
    /// бездействия, — и тогда открывать браузер по сохранённому адресу
    /// значило бы отправить человека в закрытую дверь.
    pub fn is_running(&self) -> bool {
        self.stop.receiver_count() > 0
    }

    /// Закрыть дверь. Повторный вызов безвреден, как и вызов после того, как
    /// сервер уже погас сам.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for Server {
    /// Дверь закрывается вместе с владельцем: окно настроек не должно
    /// пережить того, кто его открыл. Уничтожение канала остановки дало бы
    /// тот же результат и само по себе, но это зависимость от устройства
    /// канала — явный вызов говорит то же самое вслух и переживёт смену
    /// примитива.
    fn drop(&mut self) {
        self.stop();
    }
}

async fn accept_loop(
    listener: TcpListener,
    inner: Arc<Inner>,
    idle: Duration,
    mut stop_rx: watch::Receiver<bool>,
) {
    let mut consecutive_errors: u32 = 0;
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let deadline = *inner.last_seen() + idle;
        tokio::select! {
            // `changed` отказывает и когда отправитель уничтожен, то есть
            // когда `Server` уронили, — оба повода закрыться сходятся здесь.
            _ = stop_rx.changed() => {
                info!("сервер настроек остановлен");
                return;
            }
            _ = tokio::time::sleep_until(deadline) => {
                // Обращение могло прийти, пока мы спали: срок считался от
                // старого значения, а обработчик успел его подвинуть.
                if inner.last_seen().elapsed() < idle {
                    continue;
                }
                info!(?idle, "сервер настроек закрылся по бездействию");
                return;
            }
            accepted = listener.accept() => {
                let (sock, _) = match accepted {
                    Ok(pair) => {
                        consecutive_errors = 0;
                        pair
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        warn!(error = %e, consecutive_errors, "сервер настроек: ошибка приёма");
                        if consecutive_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                // Сверх предела — молча закрытый сокет, а не ответ `503`, как
                // у моста: ответ подтвердил бы существование сервера тому,
                // кто токена не предъявлял, а именно этого мы и не делаем.
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    debug!(
                        limit = MAX_CONNECTIONS,
                        "предел одновременных соединений страницы настроек исчерпан"
                    );
                    drop(sock);
                    continue;
                };
                // Отдельной задачей, а не тут же: браузер открывает сокеты
                // заранее и молчит в них, и обслуживание по очереди означало
                // бы, что такой сокет задерживает саму страницу.
                let inner = Arc::clone(&inner);
                tokio::spawn(async move {
                    if tokio::time::timeout(REQUEST_TIMEOUT, serve_one(sock, inner))
                        .await
                        .is_err()
                    {
                        debug!("запрос к странице настроек не уложился в срок");
                    }
                    drop(permit);
                });
            }
        }
    }
}

async fn serve_one(mut sock: TcpStream, inner: Arc<Inner>) {
    let head = match read_head(&mut sock, MAX_HEAD).await {
        Ok(head) => head,
        Err(HeadError::Truncated) => return, // сокет открыт «на всякий случай» и закрыт
        Err(e) => {
            debug!(error = %e, "запрос к странице настроек не разобран");
            reply(&mut sock, 400, TEXT, "Bad Request\n").await;
            return;
        }
    };

    // Проверка `Host` — против перепривязки DNS: чужая страница резолвит своё
    // имя в 127.0.0.1, политика одного источника перестаёт её сдерживать, а
    // `Host` при этом остаётся чужим. Наш собственный адрес мы знаем точно —
    // сами его и выдали.
    if !host_is_ours(&head, inner.port) {
        not_found(&mut sock).await;
        return;
    }

    let Some(path) = request_path(&head.target) else {
        not_found(&mut sock).await;
        return;
    };
    let (given, tail) = split_first_segment(path);
    if !constant_time_eq(inner.token.as_bytes(), given.as_bytes()) {
        not_found(&mut sock).await;
        return;
    }
    // Разделов под токеном пока нет; неизвестный путь ничем не лучше
    // неизвестного токена.
    if !tail.is_empty() {
        not_found(&mut sock).await;
        return;
    }

    inner.touch();

    if head.method.eq_ignore_ascii_case("GET") {
        reply(
            &mut sock,
            200,
            HTML,
            &settings_page::render(&inner.state, None),
        )
        .await;
        return;
    }
    if !head.method.eq_ignore_ascii_case("POST") {
        reply(&mut sock, 405, TEXT, "Method Not Allowed\n").await;
        return;
    }

    // Дальше — то, что меняет состояние. Токен здесь уже предъявлен, значит
    // о нашем существовании отправитель знает, и прятаться за `404` больше
    // не от кого: честный `403` полезнее для отладки.
    if !origin_is_ours(&head, inner.port) {
        // `Origin` — это схема и авторитет, без пути: токена в нём нет и быть
        // не может, поэтому его можно писать целиком. А вот `Referer` несёт
        // ПУТЬ, то есть у нашей же страницы — токен. Лог живёт на диске
        // дольше сеанса (см. модульный комментарий), так что о `Referer`
        // сообщается только сам факт его наличия.
        warn!(
            origin = ?head.header("Origin"),
            has_referer = head.header("Referer").is_some(),
            "запрос со стороны к странице настроек отвергнут"
        );
        reply(&mut sock, 403, TEXT, "Forbidden\n").await;
        return;
    }

    let body = match read_body(&mut sock, &head).await {
        Ok(body) => body,
        Err(e) => {
            debug!(error = %e, "тело формы не прочитано");
            reply(&mut sock, 400, TEXT, "Bad Request\n").await;
            return;
        }
    };
    let page = settings_page::handle_post(&inner.state, &body).await;
    reply(&mut sock, 200, HTML, &page).await;
}

/// Ответ на всё, что не предъявило верный токен.
///
/// Ни слова о том, что здесь за сервер: тело — ровно то, что отдал бы любой
/// чужой веб-сервер. См. модульный комментарий, почему `404`, а не `403`.
async fn not_found(sock: &mut TcpStream) {
    reply(sock, 404, TEXT, "Not Found\n").await;
}

const TEXT: &str = "text/plain; charset=utf-8";
const HTML: &str = "text/html; charset=utf-8";

async fn reply(sock: &mut TcpStream, code: u16, content_type: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: same-origin\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'self'; style-src 'self' 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'\r\n\
         \r\n",
        reason = status_text(code),
        len = body.len()
    );
    // `Referrer-Policy` и `form-action` — не украшение: токен лежит в адресе,
    // и без них первая же внешняя ссылка или форма на странице унесла бы его
    // в заголовке `Referer` на чужой сервер.
    //
    // Именно `same-origin`, а НЕ `no-referrer`, и это не вкусовщина. По Fetch
    // (шаг «append a request Origin header») запрос не-GET/HEAD при политике
    // `no-referrer` уходит с `Origin: null` — и `Referer` при ней не уходит
    // вовсе. То есть отправка нашей же формы (`<form method="post">`) пришла
    // бы к нам вообще без признаков своего происхождения, и `origin_is_ours`
    // честно отдал бы `403` на нажатие собственной кнопки «Сохранить».
    // Лечить это, начав принимать `Origin: null`, нельзя: `null` шлют и
    // непрозрачные источники — песочница в iframe, страница из `data:`, — то
    // есть ровно те, от кого проверка и заведена. При `same-origin` браузер
    // шлёт настоящий `Origin` и настоящий `Referer` своему же источнику, а
    // при переходе на чужой не шлёт `Referer` вовсе — токен по-прежнему не
    // покидает 127.0.0.1.
    //
    // `frame-ancestors 'none'` — единственное, что политика оставляла
    // открытым: встроить нашу страницу в свой фрейм чужой сайт не сможет.
    if let Err(e) = write_all(sock, head.as_bytes(), body.as_bytes()).await {
        debug!(error = %e, "ответ странице настроек не доставлен");
    }
}

async fn write_all(sock: &mut TcpStream, head: &[u8], body: &[u8]) -> std::io::Result<()> {
    sock.write_all(head).await?;
    sock.write_all(body).await?;
    sock.flush().await?;

    // Ровно та же тонкость, что у моста (`serve::respond`): закрытие сокета
    // с непрочитанными входящими байтами заставляет Windows послать RST
    // вместо FIN, и уже записанный ответ до клиента не доходит. Поэтому
    // сначала закрываем свою половину на запись, потом коротко вычитываем
    // остаток — с ограниченным бюджетом, висеть на болтливом клиенте мы не
    // обязаны.
    let _ = sock.shutdown().await;
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut junk = [0u8; 1024];
    while let Ok(Ok(n)) = tokio::time::timeout_at(deadline, sock.read(&mut junk)).await {
        if n == 0 {
            break;
        }
    }
    Ok(())
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    }
}

/// Путь без ведущего `/`, без строки запроса и без якоря.
///
/// Строка запроса отрезается, а не сравнивается: `?section=bench` — это
/// адрес той же страницы, и токен в нём тот же самый.
fn request_path(target: &str) -> Option<&str> {
    let path = target.split(['?', '#']).next().unwrap_or("");
    path.strip_prefix('/')
}

fn split_first_segment(path: &str) -> (&str, &str) {
    match path.split_once('/') {
        Some((first, rest)) => (first, rest),
        None => (path, ""),
    }
}

/// Сравнение токенов за время, не зависящее от того, где именно они
/// разошлись.
///
/// Обычного `==` здесь мало. Сравнение срезов сводится к `memcmp`, а тот
/// возвращается на ПЕРВОМ различающемся байте: время ответа тем больше, чем
/// длиннее совпавшая приставка. Это превращает перебор из «16^64 вариантов»
/// в «64 позиции по 16 вариантов» — по байту за раз, замеряя ответы. Петля
/// даёт сигнал слабый, но измеримый, а стоит его отсутствие дюжины строк.
///
/// Длина сравнивается отдельно и с ранним выходом сознательно: длина токена
/// одна и та же всегда и ни от чего не зависит, секрета в ней нет. Секрет —
/// содержимое, и оно проходится целиком, без единого выхода посередине.
/// `black_box` не даёт оптимизатору свернуть цикл обратно в тот же `memcmp`
/// с ранним возвратом — без него весь этот код мог бы ничего не значить.
fn constant_time_eq(expected: &[u8], given: &[u8]) -> bool {
    if expected.len() != given.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(given.iter()) {
        diff |= a ^ b;
    }
    std::hint::black_box(diff) == 0
}

/// Свой ли источник у запроса, меняющего состояние.
///
/// `Origin` — основной признак; `Referer` — запасной, потому что часть
/// браузеров не шлёт `Origin` на отправку формы в тот же источник. Ни того,
/// ни другого — отказ: наша собственная страница шлёт хотя бы один из них, а
/// запрос, пришедший вообще без них, послан не ею.
fn origin_is_ours(head: &Head, port: u16) -> bool {
    let ours = format!("http://127.0.0.1:{port}");
    if let Some(origin) = head.header("Origin") {
        return origin == ours;
    }
    if let Some(referer) = head.header("Referer") {
        return referer == ours || referer.starts_with(&format!("{ours}/"));
    }
    false
}

fn host_is_ours(head: &Head, port: u16) -> bool {
    head.header("Host")
        .is_some_and(|host| host == format!("127.0.0.1:{port}"))
}

async fn read_body(sock: &mut TcpStream, head: &Head) -> std::io::Result<Vec<u8>> {
    // Без `Content-Length` тела нет: `Transfer-Encoding: chunked` наша форма
    // не шлёт, а разбирать его ради несуществующего отправителя незачем.
    let len: usize = match head.header("Content-Length") {
        Some(v) => v.trim().parse().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "нечитаемый Content-Length")
        })?,
        None => 0,
    };
    if len > MAX_BODY {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "тело формы больше допустимого",
        ));
    }
    // Часть тела почти всегда приезжает вместе с заголовком — `read_head`
    // отдаёт её в `leftover`, и потерять её значит недосчитаться байтов.
    let mut body = head.leftover.clone();
    body.truncate(len);
    while body.len() < len {
        let mut chunk = vec![0u8; len - body.len()];
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "тело формы оборвалось",
            ));
        }
        chunk.truncate(n);
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Токен из системного ГСЧ.
///
/// `BCryptGenRandom` с `BCRYPT_USE_SYSTEM_PREFERRED_RNG` — документированный
/// способ взять системный источник случайности Windows. Ни `SystemTime`, ни
/// счётчик, ни адрес объекта в памяти: всё это предсказуемо ровно настолько,
/// насколько предсказуем момент запуска приложения, и токен из них не был бы
/// токеном. Отказ ГСЧ — отказ запуска сервера: без токена дверь открыта, а
/// открытая дверь хуже отсутствующей.
fn random_token() -> Result<String, SettingsError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    // SAFETY: буфер живёт дольше вызова, а его длину функция берёт из самого
    // среза. Нулевой дескриптор алгоритма — не забывчивость, а требование
    // `BCRYPT_USE_SYSTEM_PREFERRED_RNG`: флаг означает «возьми системный
    // ГСЧ», и дескриптор при нём обязан быть пустым.
    let status = unsafe {
        BCryptGenRandom(
            BCRYPT_ALG_HANDLE::default(),
            &mut bytes,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status.0 != 0 {
        return Err(SettingsError::Random(status.0));
    }
    let mut token = String::with_capacity(TOKEN_BYTES * 2);
    for b in bytes {
        token.push(HEX[(b >> 4) as usize] as char);
        token.push(HEX[(b & 0x0f) as usize] as char);
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use arc_swap::ArcSwap;
    use proxypilot_bridge::supervisor::AppState;
    use proxypilot_core::config::{Config, OfficeNetwork};
    use proxypilot_core::mode::{Health, Mode, Place, Reachability, Route};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;

    use crate::settings_page::{AutostartPending, TunnelPending};
    use crate::Cmd;

    /// Порт «моста» по умолчанию. Не 8080 из вежливости, а чтобы страница
    /// доказала: она читает состояние, а не печатает константу.
    const BRIDGE_PORT: u16 = 41777;

    /// Что дошло до супервизора. Подставной приёмник канала команд делает
    /// ровно то же, что задача в `main.rs`: забирает конфиг и отвечает.
    #[derive(Clone, Default)]
    struct Applied(Arc<Mutex<Vec<Config>>>);

    impl Applied {
        fn all(&self) -> Vec<Config> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    /// Делает то же, что задача из `main.rs`: забирает конфиг, кладёт его в
    /// ячейку, которую читает страница, и только потом отвечает. Порядок
    /// именно такой — страница перерисовывается по ответу и обязана увидеть
    /// уже применённое, а не то, что было до правки.
    fn spawn_supervisor_stub(
        mut inbox: mpsc::Receiver<Cmd>,
        config: Arc<ArcSwap<Config>>,
    ) -> Applied {
        let applied = Applied::default();
        let sink = applied.clone();
        tokio::spawn(async move {
            while let Some(cmd) = inbox.recv().await {
                if let Cmd::ApplyConfig { config: next, done } = cmd {
                    config.store(Arc::new((*next).clone()));
                    sink.0
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(*next);
                    let _ = done.send(Ok(()));
                }
            }
        });
        applied
    }

    fn app_state(port: u16) -> AppState {
        AppState {
            mode: Mode::Auto,
            route: Route::Direct,
            demoted: false,
            place: Place {
                in_office: false,
                network: None,
                network_name: None,
            },
            health: Health {
                socks: Reachability::Unknown,
                http: Reachability::Unknown,
            },
            port,
        }
    }

    /// Состояние страницы вместе с подставным супервизором на другом конце
    /// канала команд.
    fn state_with(app: AppState, cfg: Config, bound_port: u16) -> (Arc<SettingsState>, Applied) {
        let config = Arc::new(ArcSwap::from_pointee(cfg));
        let (tx, rx) = mpsc::channel(8);
        let applied = spawn_supervisor_stub(rx, Arc::clone(&config));
        (
            Arc::new(SettingsState {
                app: Arc::new(ArcSwap::from_pointee(app)),
                config,
                commands: tx,
                bound_port,
                autostart: Arc::new(AutostartPending),
                tunnel: Arc::new(TunnelPending),
                update_status: Arc::new(ArcSwap::from_pointee(None)),
            }),
            applied,
        )
    }

    fn state() -> Arc<SettingsState> {
        state_with(app_state(BRIDGE_PORT), Config::default(), BRIDGE_PORT).0
    }

    /// `http://127.0.0.1:PORT/TOKEN` → (`127.0.0.1:PORT`, `TOKEN`).
    fn split_url(url: &str) -> (String, String) {
        let rest = url.strip_prefix("http://").expect("схема http://");
        let (authority, token) = rest.split_once('/').expect("путь с токеном");
        (authority.to_string(), token.to_string())
    }

    async fn open(idle: Duration) -> (Server, String, String) {
        open_with(state(), idle).await
    }

    async fn open_with(state: Arc<SettingsState>, idle: Duration) -> (Server, String, String) {
        let server = Server::start_with_idle(state, idle).await.unwrap();
        assert!(server.is_running(), "сервер не поднялся");
        let (authority, token) = split_url(&server.url().url);
        (server, authority, token)
    }

    /// Отправка формы так, как её шлёт наша же страница.
    async fn post_form(authority: &str, path: &str, body: &str) -> String {
        raw(
            authority,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: {authority}\r\nOrigin: http://{authority}\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\n\
                 Content-Length: {len}\r\nConnection: close\r\n\r\n{body}",
                len = body.len()
            ),
        )
        .await
    }

    async fn raw(authority: &str, request: &str) -> String {
        let mut c = TcpStream::connect(authority).await.unwrap();
        c.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        c.read_to_end(&mut out).await.unwrap();
        String::from_utf8_lossy(&out).to_string()
    }

    async fn get(authority: &str, path: &str) -> String {
        raw(
            authority,
            &format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"),
        )
        .await
    }

    async fn post(authority: &str, path: &str, origin: Option<&str>) -> String {
        let origin = match origin {
            Some(o) => format!("Origin: {o}\r\n"),
            None => String::new(),
        };
        raw(
            authority,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: {authority}\r\n{origin}Content-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        )
        .await
    }

    /// Запрос без токена, переживающий закрытую дверь: подключиться к уже
    /// погасшему серверу нельзя, и это не повод падать.
    ///
    /// Концы строк обязаны быть `\r\n`. С голым `\n` `read_head` не находит
    /// `\r\n\r\n`, ждёт до закрытия сокета и отдаёт `Truncated`, на который
    /// `serve_one` не отвечает вовсе, — а значит стук не доходит до проверки
    /// токена, цикл вызывающего успевает провернуться один раз, и тест
    /// перестаёт проверять то, ради чего написан.
    async fn knock(authority: &str) {
        let Ok(mut c) = TcpStream::connect(authority).await else {
            return;
        };
        let request = format!("GET / HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
        let _ = c.write_all(request.as_bytes()).await;
        // Одно чтение со сроком, а не `read_to_end`: ответ (404) приходит
        // целиком одним куском, а вот закрытия сокета сервер ждёт ещё до
        // 100 мс (он дочитывает наш запрос, чтобы Windows послала FIN, а не
        // RST, — см. `write_all`). Ждать этого на каждой итерации значило бы
        // стучать вдвое реже, чем задумано.
        let mut buf = [0u8; 512];
        let _ = tokio::time::timeout(Duration::from_secs(1), c.read(&mut buf)).await;
    }

    /// Ждёт, пока порт перестанет принимать соединения. Само по себе
    /// подключение сервер не будит: таймер бездействия сбрасывает только
    /// запрос с верным токеном, — иначе эта функция держала бы сервер живым
    /// ровно тем, что проверяет его смерть.
    async fn closed_within(authority: &str, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            if TcpStream::connect(authority).await.is_err() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    /// Ждёт, пока сервер сам объявит себя погасшим.
    ///
    /// Дешевле, чем стучаться в порт: подключение к уже закрытому порту
    /// Windows отвергает не мгновенно, а примерно через две секунды (SYN
    /// уходит в пустоту и переспрашивается), и в тестах, где проверяется
    /// ТАЙМЕР, а не сам факт закрытия сокета, эти секунды тратятся впустую.
    /// Что порт действительно закрывается, доказывают `closed_within` в
    /// `stopping_closes_the_door` и `dropping_the_handle_closes_the_door`.
    async fn stopped_within(server: &Server, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            if !server.is_running() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    const LONG_IDLE: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn a_request_without_the_token_is_not_found() {
        let (_server, authority, _token) = open(LONG_IDLE).await;
        let reply = get(&authority, "/").await;
        assert!(reply.starts_with("HTTP/1.1 404"), "получили: {reply}");
        // 404, а не 403: не подтверждаем существование сервера тому, кто не
        // знает токена. И в теле — ни слова про ProxyPilot.
        assert!(
            !reply.contains("ProxyPilot") && !reply.contains("proxypilot"),
            "404 выдал сам себя: {reply}"
        );
    }

    #[tokio::test]
    async fn a_wrong_token_is_not_found() {
        let (_server, authority, token) = open(LONG_IDLE).await;
        // Токен той же длины и того же алфавита, отличается одним символом —
        // угадывание по коду ответа не должно ничего давать.
        let mut wrong: Vec<char> = token.chars().collect();
        wrong[0] = if wrong[0] == 'a' { 'b' } else { 'a' };
        let wrong: String = wrong.into_iter().collect();
        let reply = get(&authority, &format!("/{wrong}")).await;
        assert!(reply.starts_with("HTTP/1.1 404"), "получили: {reply}");
    }

    #[tokio::test]
    async fn a_truncated_token_is_not_found() {
        let (_server, authority, token) = open(LONG_IDLE).await;
        let reply = get(&authority, &format!("/{}", &token[..token.len() - 1])).await;
        assert!(reply.starts_with("HTTP/1.1 404"), "получили: {reply}");
    }

    #[tokio::test]
    async fn the_right_token_serves_the_page() {
        let (_server, authority, token) = open(LONG_IDLE).await;
        let reply = get(&authority, &format!("/{token}")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
        // Заглушка читает состояние, а не печатает константу.
        assert!(reply.contains("41777"), "страница без порта моста: {reply}");
        // Токен в адресе утёк бы в Referer на любую внешнюю ссылку. Именно
        // `same-origin`, а не `no-referrer`: последняя заставила бы браузер
        // слать нашу же форму с `Origin: null` и без `Referer` — см. `reply`.
        assert!(
            reply.contains("Referrer-Policy: same-origin"),
            "страница без Referrer-Policy: {reply}"
        );
        // Единственное, что политика оставляла открытым, — встраивание во
        // фрейм чужой страницы.
        assert!(
            reply.contains("frame-ancestors 'none'"),
            "страница без frame-ancestors: {reply}"
        );
    }

    #[tokio::test]
    async fn the_number_of_simultaneous_connections_is_capped() {
        // Размер заголовка, размер тела и время жизни соединения ограничены
        // каждый; количество соединений было единственной осью без потолка.
        let (_server, authority, token) = open(LONG_IDLE).await;
        // Молчащие сокеты занимают места: обработчик ждёт заголовка.
        let mut hogs = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            hogs.push(TcpStream::connect(&authority).await.unwrap());
        }
        // Цикл приёма разбирает их не мгновенно — дадим ему добраться.
        tokio::time::sleep(Duration::from_millis(300)).await;
        // Сверх предела — закрытый сокет без ответа: `503` подтвердил бы
        // существование сервера тому, кто токена не предъявлял. Чтение
        // терпимое: закрытие сокета с непрочитанным запросом Windows
        // оформляет как RST, и это не ошибка теста, а тот самый отказ.
        let mut over = TcpStream::connect(&authority).await.unwrap();
        let request =
            format!("GET /{token} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
        let _ = over.write_all(request.as_bytes()).await;
        let mut reply = Vec::new();
        let _ = over.read_to_end(&mut reply).await;
        assert!(
            reply.is_empty(),
            "предел не сработал, получили: {}",
            String::from_utf8_lossy(&reply)
        );
        // А как только места освободились, страница снова отдаётся.
        drop(hogs);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let reply = get(&authority, &format!("/{token}")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
    }

    #[tokio::test]
    async fn the_query_string_does_not_hide_the_token() {
        let (_server, authority, token) = open(LONG_IDLE).await;
        let reply = get(&authority, &format!("/{token}?section=bench")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
    }

    #[tokio::test]
    async fn an_unknown_path_under_a_valid_token_is_not_found() {
        let (_server, authority, token) = open(LONG_IDLE).await;
        let reply = get(&authority, &format!("/{token}/nothing")).await;
        assert!(reply.starts_with("HTTP/1.1 404"), "получили: {reply}");
    }

    #[tokio::test]
    async fn a_state_changing_request_from_a_foreign_origin_is_rejected() {
        let (_server, authority, token) = open(LONG_IDLE).await;
        let reply = post(
            &authority,
            &format!("/{token}"),
            Some("http://evil.example"),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 403"), "получили: {reply}");
    }

    #[tokio::test]
    async fn a_state_changing_request_without_any_origin_is_rejected() {
        let (_server, authority, token) = open(LONG_IDLE).await;
        let reply = post(&authority, &format!("/{token}"), None).await;
        assert!(reply.starts_with("HTTP/1.1 403"), "получили: {reply}");
    }

    #[tokio::test]
    async fn our_own_page_may_post() {
        // Настоящий `Origin` здесь не для того, чтобы утверждение сошлось, а
        // потому что браузер его действительно пришлёт: политика ответа —
        // `same-origin`, и она это разрешает. При `no-referrer` (как было
        // сначала) отправка формы ушла бы с `Origin: null` и вовсе без
        // `Referer` — то есть наша же кнопка «Сохранить» получила бы `403`.
        // Ровно поэтому политика такая, а не `no-referrer`; см. `reply`.
        let (_server, authority, token) = open(LONG_IDLE).await;
        let ours = format!("http://{authority}");
        let reply = post(&authority, &format!("/{token}"), Some(&ours)).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
    }

    #[tokio::test]
    async fn an_opaque_origin_is_rejected() {
        // `Origin: null` шлют непрозрачные источники: песочница в iframe,
        // страница из `data:`. Принять его значило бы открыть дверь ровно
        // тем, от кого проверка и заведена, — поэтому это отдельный тест, а
        // не подразумеваемое следствие соседнего.
        let (_server, authority, token) = open(LONG_IDLE).await;
        let reply = post(&authority, &format!("/{token}"), Some("null")).await;
        assert!(reply.starts_with("HTTP/1.1 403"), "получили: {reply}");
    }

    #[tokio::test]
    async fn a_referer_from_our_own_page_is_accepted_when_origin_is_missing() {
        // Форма без Origin — реальный случай у части браузеров; Referer
        // тогда единственное, чем запрос отличается от чужого. Политика
        // `same-origin` его своему же источнику шлёт, так что этот заголовок
        // тест не выдумывает.
        let (_server, authority, token) = open(LONG_IDLE).await;
        let reply = raw(
            &authority,
            &format!(
                "POST /{token} HTTP/1.1\r\nHost: {authority}\r\nReferer: http://{authority}/{token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
    }

    #[tokio::test]
    async fn a_foreign_host_header_is_not_found() {
        // Перепривязка DNS: чужая страница резолвит своё имя в 127.0.0.1 и
        // ходит к нам как к себе. Заголовок `Host` при этом остаётся чужим.
        let (_server, authority, token) = open(LONG_IDLE).await;
        let port = authority.split(':').next_back().unwrap();
        let reply = raw(
            &authority,
            &format!(
                "GET /{token} HTTP/1.1\r\nHost: evil.example:{port}\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 404"), "получили: {reply}");
    }

    #[tokio::test]
    async fn the_listener_is_on_loopback() {
        let (server, authority, _token) = open(LONG_IDLE).await;
        assert!(
            server.url().url.starts_with("http://127.0.0.1:"),
            "адрес не loopback: {}",
            server.url().url
        );
        let addr: std::net::SocketAddr = authority.parse().unwrap();
        assert!(addr.ip().is_loopback(), "слушатель не на loopback: {addr}");
        // Порт 0 означает «дай свободный», а не «слушай на нулевом».
        assert_ne!(addr.port(), 0);
    }

    #[tokio::test]
    async fn every_session_gets_its_own_token() {
        let (_a, _aa, token_a) = open(LONG_IDLE).await;
        let (_b, _bb, token_b) = open(LONG_IDLE).await;
        assert_ne!(token_a, token_b);
        // 32 байта → 64 шестнадцатеричных знака.
        assert_eq!(token_a.len(), 64);
        assert!(token_a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn a_token_from_a_previous_session_is_not_found() {
        let (server, _authority, token) = open(LONG_IDLE).await;
        server.stop();
        assert!(stopped_within(&server, Duration::from_secs(5)).await);

        let (_next, next_authority, next_token) = open(LONG_IDLE).await;
        assert_ne!(token, next_token);
        let reply = get(&next_authority, &format!("/{token}")).await;
        assert!(reply.starts_with("HTTP/1.1 404"), "получили: {reply}");
    }

    #[tokio::test]
    async fn stopping_closes_the_door() {
        let (server, authority, _token) = open(LONG_IDLE).await;
        server.stop();
        assert!(
            closed_within(&authority, Duration::from_secs(5)).await,
            "порт остался открытым после stop()"
        );
        assert!(!server.is_running());
    }

    #[tokio::test]
    async fn dropping_the_handle_closes_the_door() {
        // Окно настроек закрылось вместе с владельцем сервера — дверь
        // обязана закрыться сама, без явного вызова.
        let (server, authority, _token) = open(LONG_IDLE).await;
        drop(server);
        assert!(
            closed_within(&authority, Duration::from_secs(5)).await,
            "порт остался открытым после drop"
        );
    }

    #[tokio::test]
    async fn the_server_stops_after_the_idle_timeout() {
        let (server, authority, _token) = open(Duration::from_millis(200)).await;
        // Владелец обязан увидеть, что сервера больше нет: иначе он откроет
        // браузер по мёртвому адресу.
        assert!(
            stopped_within(&server, Duration::from_secs(5)).await,
            "сервер пережил таймаут бездействия"
        );
        // И порт при этом действительно закрыт. Здесь это уже дёшево:
        // отказа ждать не приходится, он наступил.
        assert!(closed_within(&authority, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn activity_postpones_the_idle_timeout() {
        let idle = Duration::from_millis(400);
        let (server, authority, token) = open(idle).await;
        // Три обращения с шагом меньше таймаута — суммарно дольше него.
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let reply = get(&authority, &format!("/{token}")).await;
            assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
        }
        // А теперь замолкаем — и он всё-таки гаснет.
        assert!(
            stopped_within(&server, Duration::from_secs(5)).await,
            "сервер не погас после того, как обращения прекратились"
        );
    }

    #[tokio::test]
    async fn a_request_without_a_token_does_not_postpone_the_timeout() {
        // Иначе любой процесс без токена держал бы дверь в настройки
        // открытой сколько угодно, просто стуча в неё.
        let idle = Duration::from_millis(500);
        let (server, authority, _token) = open(idle).await;
        // Первый стук — по заведомо живому серверу, и он обязан вернуться
        // быстро. Это страж самого теста: пока `knock` слал голые `\n`,
        // `read_head` не находил `\r\n\r\n`, ждал закрытия сокета, и один
        // стук висел до `REQUEST_TIMEOUT`. Цикл ниже тогда проворачивался
        // ровно один раз, а тест тихо вырождался в дубликат соседнего.
        // Счётчик витков этого не поймал бы устойчиво: витки считаются по
        // настенным часам, а подключение к серверу, гаснущему ровно в этот
        // миг, у Windows занимает секунды.
        let one = tokio::time::Instant::now();
        knock(&authority).await;
        let one = one.elapsed();
        assert!(one < idle, "один стук по живому серверу занял {one:?}");

        // Дальше стучим чаще, чем срабатывает таймаут, и суммарно вдвое
        // дольше него: если бы стук его сбрасывал, сервер пережил бы цикл
        // целиком. Отказ подключения по ходу дела — не ошибка теста, а ровно
        // то, чего мы и ждём.
        let mut knocks = 1;
        let started = tokio::time::Instant::now();
        while server.is_running() && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            knock(&authority).await;
            knocks += 1;
        }
        // Стучать в уже закрытую дверь незачем и дорого: отказ подключения
        // Windows отдаёт примерно через две секунды, и четыре лишних витка
        // стоили бы восьми секунд на ровном месте.
        assert!(knocks >= 3, "не успели постучать: всего {knocks} раз");
        assert!(
            !server.is_running(),
            "стук без токена продлил жизнь сервера"
        );
    }

    #[tokio::test]
    async fn changing_only_the_port_does_not_rebind_the_listener() {
        // Единственное место во всём плане, где инвариант «слушатель
        // привязывается один раз за жизнь процесса» можно нарушить руками
        // пользователя. Тихая перепривязка оборвала бы все установленные
        // соединения — то самое свойство, ради которого продукт переписан.
        let bridge = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = bridge.local_addr().unwrap().port();

        // Порт, который человек впишет в форму: заведомо свободный — заняли
        // и сразу отпустили.
        let requested = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let p = l.local_addr().unwrap().port();
            drop(l);
            p
        };
        assert_ne!(bound, requested);

        let cfg = Config {
            bridge_port: bound,
            ..Default::default()
        };
        let (st, applied) = state_with(app_state(bound), cfg, bound);
        let (_server, authority, token) = open_with(Arc::clone(&st), LONG_IDLE).await;

        let reply = post_form(
            &authority,
            &format!("/{token}"),
            &format!(
                "action=save&bridge_port={requested}&socks_upstream=&http_upstream=&no_proxy="
            ),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");

        // 1. Слушатель, который был, по-прежнему принимает соединения.
        assert!(
            TcpStream::connect(("127.0.0.1", bound)).await.is_ok(),
            "живой слушатель исчез"
        );
        // 2. На запрошенном порту никто не слушает — и доказывает это то,
        //    что мы сами можем его занять: перепривязка отдала бы «адрес
        //    уже используется».
        assert!(
            TcpListener::bind(("127.0.0.1", requested)).await.is_ok(),
            "порт перепривязан на лету"
        );
        // 3. На диск ушло то, что человек ввёл...
        let sent = applied.all();
        assert_eq!(sent.len(), 1, "команда не дошла до супервизора");
        assert_eq!(
            sent[0].bridge_port, requested,
            "введённое значение обязано сохраниться"
        );
        // ...а живой конфиг продолжает нести порт, на котором слушатель уже
        // привязан. Иначе `AppState.port` — а через него меню, «скопировать
        // адрес» и диагностика — говорили бы про порт, где никто не слушает.
        assert_eq!(
            settings_page::live_config(&sent[0], bound).bridge_port,
            bound
        );
        // 4. И страница не делает вид, что уже переехала.
        assert!(
            reply.contains(&format!("127.0.0.1:{bound}")),
            "страница показывает не тот порт: {reply}"
        );
        assert!(
            reply.contains("перезапуст"),
            "страница молчит про перезапуск: {reply}"
        );
    }

    #[tokio::test]
    async fn a_valid_change_reaches_the_supervisor_through_the_command_channel() {
        let (st, applied) = state_with(app_state(BRIDGE_PORT), Config::default(), BRIDGE_PORT);
        let (_server, authority, token) = open_with(Arc::clone(&st), LONG_IDLE).await;
        let reply = post_form(
            &authority,
            &format!("/{token}"),
            "action=save&bridge_port=41777&socks_upstream=203.0.113.10%3A9999\
             &http_upstream=&no_proxy=localhost",
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");

        let sent = applied.all();
        assert_eq!(sent.len(), 1, "изменение не дошло до супервизора");
        assert_eq!(sent[0].socks_upstream.as_deref(), Some("203.0.113.10:9999"));
        // Снятый флажок браузер не шлёт вовсе — «нет поля» означает «снят».
        assert!(!sent[0].manage_system_proxy);
        // И страница показывает применённое значение, а не прежнее.
        assert!(reply.contains("203.0.113.10:9999"), "получили: {reply}");
    }

    #[tokio::test]
    async fn an_invalid_value_shows_the_message_config_validate_returned() {
        let (st, applied) = state_with(app_state(BRIDGE_PORT), Config::default(), BRIDGE_PORT);
        let (_server, authority, token) = open_with(Arc::clone(&st), LONG_IDLE).await;
        let reply = post_form(
            &authority,
            &format!("/{token}"),
            "action=save&bridge_port=41777&socks_upstream=nope&http_upstream=&no_proxy=",
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
        // Дословно то, что вернул `Config::validate`, а не пересказ: вторая
        // копия правил разошлась бы с первой, и разошлась бы молча.
        assert!(
            reply.contains("host:port"),
            "страница не показала причину отказа: {reply}"
        );
        assert!(
            applied.all().is_empty(),
            "негодный конфиг не должен доходить до супервизора"
        );
    }

    #[tokio::test]
    async fn values_with_html_metacharacters_are_escaped_in_the_page() {
        // Имя сети приходит из системы, а задал его тот, кто поднял точку
        // доступа; bypass-список и адреса апстримов — из файла, который
        // правят руками. Ни одно из этих значений не наше.
        let cfg = Config {
            no_proxy: "<script>alert(1)</script>".into(),
            office_networks: vec![OfficeNetwork {
                id: "{X}".into(),
                name: "\"><b>Офис</b>".into(),
            }],
            ..Default::default()
        };
        let mut app = app_state(BRIDGE_PORT);
        app.place.network = Some("{X}".into());
        app.place.network_name = Some("<img src=x onerror=alert(1)>".into());

        let (st, _applied) = state_with(app, cfg, BRIDGE_PORT);
        let (_server, authority, token) = open_with(st, LONG_IDLE).await;
        let reply = get(&authority, &format!("/{token}")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
        assert!(
            !reply.contains("<script>alert"),
            "неэкранированный скрипт в разметке: {reply}"
        );
        assert!(
            !reply.contains("<img src=x"),
            "неэкранированный тег в разметке: {reply}"
        );
        assert!(
            reply.contains("&lt;script&gt;"),
            "экранированного вида нет вовсе: {reply}"
        );
    }

    #[tokio::test]
    async fn the_office_button_prefills_the_current_network_guid() {
        // Человек не должен переписывать GUID руками — ровно за этим
        // `AppState.place` носит его вместе с именем.
        const ID: &str = "{AAAA0000-0000-0000-0000-000000000001}";
        let mut app = app_state(BRIDGE_PORT);
        app.place.network = Some(ID.into());
        app.place.network_name = Some("OFFICE-WIFI".into());

        let (st, applied) = state_with(app, Config::default(), BRIDGE_PORT);
        let (_server, authority, token) = open_with(Arc::clone(&st), LONG_IDLE).await;
        let reply = post_form(
            &authority,
            &format!("/{token}"),
            "action=office&bridge_port=41777&socks_upstream=&http_upstream=&no_proxy=",
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");

        let sent = applied.all();
        assert_eq!(sent.len(), 1, "кнопка не дошла до супервизора");
        assert_eq!(sent[0].office_networks.len(), 1);
        assert_eq!(sent[0].office_networks[0].id, ID);
        assert_eq!(sent[0].office_networks[0].name, "OFFICE-WIFI");
        assert!(reply.contains(ID), "GUID не попал на страницу: {reply}");
    }

    #[tokio::test]
    async fn the_diagnostics_button_shows_its_output_in_place() {
        let (st, _applied) = state_with(app_state(BRIDGE_PORT), Config::default(), BRIDGE_PORT);
        let (_server, authority, token) = open_with(st, LONG_IDLE).await;
        let reply = post_form(&authority, &format!("/{token}"), "action=doctor").await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
        assert!(
            reply.contains("Мост слушает свой порт"),
            "вывод диагностики не показан: {reply}"
        );
    }

    #[test]
    fn the_token_comparison_is_length_and_content_sensitive() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"abc", b""));
        assert!(constant_time_eq(b"", b""));
    }
}
