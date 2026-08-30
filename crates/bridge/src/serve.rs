//! Приём и обслуживание клиентских соединений.
//!
//! Маршрут читается ОДИН РАЗ на соединение — снимок берётся до набора
//! апстрима и дальше не пересматривается: на пути данных к роутеру никто
//! больше не обращается, что бы ни переключили потом. Именно поэтому смена
//! маршрута не рвёт живой трафик.

use std::sync::Arc;
use std::time::Duration;

use proxypilot_core::bypass::BypassList;
use proxypilot_core::mode::Route;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::connector::{connect_via, dial_upstream_plain};
use crate::http::{read_head, split_host_port, Head};
use crate::router::Router;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub dial: Duration,
    pub head: Duration,
    pub max_connections: usize,
}

#[derive(Debug)]
pub struct Shared {
    pub router: Arc<Router>,
    pub bypass: Arc<BypassList>,
    pub limits: Limits,
}

pub async fn serve(listener: TcpListener, shared: Arc<Shared>) -> std::io::Result<()> {
    // Адрес не обязан быть доступен (экзотика платформы) — потеря этой
    // строки не повод отказывать в обслуживании.
    if let Ok(addr) = listener.local_addr() {
        info!(%addr, "мост слушает");
    }

    let permits = Arc::new(Semaphore::new(shared.limits.max_connections));

    // Ошибка accept почти всегда относится к одному соединению: клиент исчез
    // между SYN и accept, кончились дескрипторы. Уронить на ней весь цикл
    // нельзя — системный прокси продолжает указывать сюда, и для пользователя
    // это будет не деградация, а полное отсутствие сети. Сдаёмся, только если
    // слушатель сломан устойчиво.
    //
    // 640 ошибок по 50 мс — примерно полминуты непрерывного отказа. Прежние
    // 64 давали три секунды, а исчерпание дескрипторов под нагрузкой длится
    // дольше: мост выходил ровно там, где обязан был переждать.
    const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 640;
    // ConnectionAborted/ConnectionReset/Interrupted никогда не должны ронять
    // цикл и не идут в общий бюджет — по отдельности каждая относится к
    // одному соединению, а не к слушателю. Но если слушатель вдруг начнёт
    // сыпать именно ими устойчиво (баг ОС, экзотика на границе accept), цикл
    // без своего счётчика и паузы — чистый спин на ядре: без счётчика, без
    // лога, без выхода. Поэтому у них отдельный порог, не для выхода, а
    // чтобы после долгой серии перестать жечь ядро вхолостую.
    const TRANSIENT_ERROR_SLEEP_THRESHOLD: u32 = 64;
    let mut consecutive_errors: u32 = 0;
    let mut consecutive_transient_errors: u32 = 0;

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(pair) => {
                consecutive_errors = 0;
                consecutive_transient_errors = 0;
                pair
            }
            // Эти три относятся к ОДНОМУ соединению, а не к слушателю: клиент
            // отвалился между SYN и accept, либо accept прервали сигналом.
            // Слушатель цел, ждать нечего и считать в общий бюджет нечего —
            // иначе шквал оборванных соединений закрыл бы мост. Уйти отсюда
            // `return` нельзя ни при каком счётчике.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                consecutive_transient_errors += 1;
                // debug, не warn: это ровно тот рутинный, ожидаемый случай,
                // который описан в комментарии выше — клиент отвалился между
                // SYN и accept. Под всплеском таких ошибок (та самая нехватка
                // дескрипторов, ради которой существует этот код) warn здесь
                // захлестнул бы лог и заглушил бы соседнюю ветку ниже, которая
                // и есть то, что оператору нужно увидеть.
                debug!(
                    error = %e,
                    consecutive_transient_errors,
                    "приём: временная ошибка"
                );
                if consecutive_transient_errors >= TRANSIENT_ERROR_SLEEP_THRESHOLD {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                continue;
            }
            Err(e) => {
                consecutive_errors += 1;
                warn!(
                    error = %e,
                    consecutive_errors,
                    "приём: ошибка, {consecutive_errors} подряд"
                );
                if consecutive_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                    return Err(e);
                }
                // Исчерпание ресурсов лечится ожиданием, а не спином.
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let shared = Arc::clone(&shared);
        let permits = Arc::clone(&permits);

        tokio::spawn(async move {
            // Превышение лимита — честный отказ, а не молчаливое исчерпание
            // ресурсов: клиент должен узнать, что произошло.
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                warn!(
                    limit = shared.limits.max_connections,
                    "предел соединений исчерпан"
                );
                let mut s = stream;
                let _ = respond(&mut s, 503, "too many connections").await;
                return;
            };
            handle(stream, shared).await;
            drop(permit);
        });
    }
}

pub async fn handle(mut client: TcpStream, shared: Arc<Shared>) {
    let head =
        match tokio::time::timeout(shared.limits.head, read_head(&mut client, 16 * 1024)).await {
            Err(e) => {
                debug!(error = %e, "некорректный запрос клиента");
                let _ = respond(&mut client, 408, "request head timed out").await;
                return;
            }
            Ok(Err(e)) => {
                debug!(error = %e, "некорректный запрос клиента");
                let _ = respond(&mut client, 400, &format!("bad request: {e}")).await;
                return;
            }
            Ok(Ok(h)) => h,
        };

    // parse двусторонний намеренно — коннектор читает им ответ вышестоящего
    // прокси, и у ответа версия оказывается в method. Но от КЛИЕНТА строка
    // ответа приходить не должна: она проваливалась в handle_plain и уезжала
    // к origin бессмысленной строкой запроса.
    if head.method.starts_with("HTTP/") {
        let _ = respond(
            &mut client,
            400,
            "expected a request, got a response status line",
        )
        .await;
        return;
    }

    if head.is_connect() {
        handle_connect(client, head, shared).await;
    } else {
        handle_plain(client, head, shared).await;
    }
}

async fn handle_connect(mut client: TcpStream, head: Head, shared: Arc<Shared>) {
    let Some((host, port)) = split_host_port(&head.target, 443) else {
        let _ = respond(&mut client, 400, "bad CONNECT target").await;
        return;
    };

    // Снимок маршрута на всё время жизни соединения.
    let route = pick_route(&host, &shared);

    let upstream = match connect_via(&route, &host, port, shared.limits.dial).await {
        Ok(s) => {
            // Успех — путь каждого запроса браузера: info здесь превратил бы
            // лог в шум. debug достаточно для разбора конкретной сессии.
            debug!(%host, port, ?route, "апстрим соединён");
            s
        }
        Err(e) => {
            // Тихого перехода на direct здесь нет и быть не должно: это была
            // бы утечка трафика мимо выбранного маршрута. Клиент получает
            // внятную ошибку, решение о смене маршрута принимает core.
            warn!(%host, port, error = %e, "апстрим недоступен");
            let _ = respond(&mut client, 502, &format!("upstream: {e}")).await;
            return;
        }
    };
    let mut upstream_stream = upstream.stream;

    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }

    // Два независимых направления, оба обязаны уйти до старта перекачки.
    //
    // head.leftover — байты клиента, захваченные вместе с его заголовком
    // (обычно TLS ClientHello): клиент→апстрим.
    if !head.leftover.is_empty() && upstream_stream.write_all(&head.leftover).await.is_err() {
        return;
    }
    // upstream.pending — байты апстрима, склеенные с ответом на наш CONNECT
    // (приветствие ssh/smtp/imap): апстрим→клиент.
    if !upstream.pending.is_empty() && client.write_all(&upstream.pending).await.is_err() {
        return;
    }

    // Nagle добавляет до 40 мс каждой мелкой записи в туннель — нажатия в
    // ssh, короткие TLS-записи, болтливый запрос-ответ. Склеивать за нас
    // здесь нечего, задержка чистый вред. Ошибку игнорируем: сокет от неё
    // рабочим быть не перестаёт.
    let _ = client.set_nodelay(true);
    let _ = upstream_stream.set_nodelay(true);

    // Таймаута простоя нет сознательно: long-poll, websocket и ssh молчат
    // законно. Полагаемся на TCP.
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream_stream).await;
}

/// Обычный HTTP (не CONNECT), запрос в absolute-form.
///
/// Keep-alive сознательно не поддерживается: подавляющая часть трафика —
/// HTTPS через CONNECT, и усложнять первую версию ради http:// не стоит.
/// Каждый запрос обслуживается в отдельном соединении к origin.
async fn handle_plain(mut client: TcpStream, head: Head, shared: Arc<Shared>) {
    let Some((host, port, path)) = split_absolute(&head.target) else {
        let _ = respond(&mut client, 400, "expected an absolute-form URL").await;
        return;
    };

    // Снимок маршрута на всё время жизни соединения — тот же принцип, что
    // и в handle_connect.
    let route = pick_route(&host, &shared);

    // Апстрим http — особый случай (спец 5.3). CONNECT на 80-й порт у
    // корпоративных прокси сплошь и рядом запрещён политикой: Squid из
    // коробки во всей линейке 2.x-6.x несёт `acl SSL_ports port 443` и
    // `http_access deny CONNECT !SSL_ports`, коммерческие шлюзы (Blue
    // Coat, Zscaler, Forcepoint) по умолчанию делают то же самое. Если
    // гонять обычный http:// через CONNECT-туннель, прокси отвечает 403,
    // и любой http://-адрес — CRL, OCSP, captive portal, внутренние
    // сайты без TLS — ломается именно там, где включён http-апстрим.
    // Поэтому для него запрос идёт как есть, в absolute-form, прямо в
    // TCP-соединение с прокси, без CONNECT и без переписывания в
    // origin-form. Только socks5 и direct переписываются на origin.
    let (upstream, request_line) = match &route {
        Route::Http(addr) => {
            let upstream = match dial_upstream_plain(addr, shared.limits.dial).await {
                Ok(s) => {
                    debug!(%host, port, ?route, "апстрим соединён");
                    s
                }
                Err(e) => {
                    warn!(%host, port, error = %e, "апстрим недоступен");
                    let _ = respond(&mut client, 502, &format!("upstream: {e}")).await;
                    return;
                }
            };
            (
                upstream,
                format!("{} {} {}\r\n", head.method, head.target, head.version),
            )
        }
        _ => {
            let upstream = match connect_via(&route, &host, port, shared.limits.dial).await {
                Ok(s) => {
                    debug!(%host, port, ?route, "апстрим соединён");
                    s
                }
                Err(e) => {
                    warn!(%host, port, error = %e, "апстрим недоступен");
                    let _ = respond(&mut client, 502, &format!("upstream: {e}")).await;
                    return;
                }
            };
            (
                upstream,
                format!("{} {} {}\r\n", head.method, path, head.version),
            )
        }
    };
    let mut upstream_stream = upstream.stream;

    let mut request = request_line;
    for (name, value) in &head.headers {
        if is_hop_by_hop(name) {
            continue;
        }
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");

    if upstream_stream.write_all(request.as_bytes()).await.is_err() {
        let _ = respond(&mut client, 502, "upstream write failed").await;
        return;
    }
    if !head.leftover.is_empty() && upstream_stream.write_all(&head.leftover).await.is_err() {
        return;
    }
    // Байты апстрима, склеенные с ответом на наш CONNECT, идут в обратную
    // сторону — клиенту. Для absolute-form через http-апстрим это начало
    // ответа origin, и потерять его так же нельзя. На новой ветке (http
    // без CONNECT) pending всегда пуст, так что запись остаётся корректной
    // без изменений.
    if !upstream.pending.is_empty() && client.write_all(&upstream.pending).await.is_err() {
        return;
    }

    // См. handle_connect: Nagle добавляет задержку мелким записям, а
    // склеивать здесь нечего.
    let _ = client.set_nodelay(true);
    let _ = upstream_stream.set_nodelay(true);

    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream_stream).await;
}

/// `http://host:port/path?q` → (host, port, "/path?q")
fn split_absolute(target: &str) -> Option<(String, u16, String)> {
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("HTTP://"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = split_host_port(authority, 80)?;
    Some((host, port, path.to_string()))
}

/// Заголовки, относящиеся к соединению с прокси, а не к запросу.
fn is_hop_by_hop(name: &str) -> bool {
    const NAMES: [&str; 8] = [
        "connection",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "keep-alive",
        "te",
        "trailer",
        "upgrade",
    ];
    let lower = name.to_ascii_lowercase();
    NAMES.contains(&lower.as_str())
}

fn pick_route(host: &str, shared: &Shared) -> Route {
    if shared.bypass.matches(host) {
        Route::Direct
    } else {
        (*shared.router.get()).clone()
    }
}

async fn respond(s: &mut TcpStream, code: u16, reason: &str) -> std::io::Result<()> {
    let body = format!("proxypilot: {reason}\n");
    let head = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status_text(code),
        body.len()
    );
    s.write_all(head.as_bytes()).await?;
    s.write_all(body.as_bytes()).await?;
    s.flush().await?;

    // Закрытие сокета с непрочитанными входящими байтами заставляет ОС
    // послать абортивный RST вместо FIN, и уже записанный ответ до клиента
    // не доходит — на Windows это воспроизводится стабильно. Поэтому сначала
    // закрываем свою половину на запись (клиент видит, что ответ закончен),
    // затем коротко вычитываем то, что он успел прислать. Бюджет общий и
    // ограниченный: висеть на болтливом клиенте мы не обязаны.
    let _ = s.shutdown().await;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
    let mut junk = [0u8; 1024];
    while let Ok(Ok(n)) = tokio::time::timeout_at(deadline, s.read(&mut junk)).await {
        if n == 0 {
            break;
        }
    }
    Ok(())
}

fn status_text(code: u16) -> &'static str {
    match code {
        400 => "Bad Request",
        408 => "Request Timeout",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::bypass::BypassList;
    use proxypilot_core::mode::Route;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Origin, отвечающий "pong" на любые 4 байта.
    async fn origin() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut b = [0u8; 4];
                    if s.read_exact(&mut b).await.is_ok() {
                        let _ = s.write_all(b"pong").await;
                    }
                });
            }
        });
        addr
    }

    async fn bridge_with(route: Route, no_proxy: &str) -> (String, Arc<Shared>) {
        let shared = Arc::new(Shared {
            router: Arc::new(Router::new(route)),
            bypass: Arc::new(BypassList::parse(no_proxy)),
            limits: Limits {
                dial: Duration::from_secs(2),
                head: Duration::from_secs(2),
                max_connections: 512,
            },
        });
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let s2 = Arc::clone(&shared);
        tokio::spawn(async move { serve(l, s2).await });
        (addr, shared)
    }

    async fn connect_through(bridge: &str, target: &str) -> (TcpStream, String) {
        let mut c = TcpStream::connect(bridge).await.unwrap();
        c.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = vec![0u8; 128];
        let n = c.read(&mut buf).await.unwrap();
        (c, String::from_utf8_lossy(&buf[..n]).to_string())
    }

    #[tokio::test]
    async fn connect_direct_tunnels_bytes() {
        let target = origin().await;
        let (bridge, _) = bridge_with(Route::Direct, "").await;
        let (mut c, reply) = connect_through(&bridge, &target).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");

        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn bypassed_host_goes_direct_even_with_upstream_set() {
        // Апстрим заведомо мёртвый: если bypass не сработает, будет 502.
        let target = origin().await;
        let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "127.0.0.1").await;
        let (mut c, reply) = connect_through(&bridge, &target).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");
        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn dead_upstream_yields_502_not_a_hang() {
        let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "").await;
        let (_c, reply) = connect_through(&bridge, "example.com:443").await;
        assert!(reply.starts_with("HTTP/1.1 502"), "получили: {reply}");
    }

    #[tokio::test]
    async fn malformed_request_yields_400() {
        let (bridge, _) = bridge_with(Route::Direct, "").await;
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(b"nonsense\r\n\r\n").await.unwrap();
        let mut buf = vec![0u8; 128];
        let n = c.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 400"));
    }

    #[tokio::test]
    async fn changing_route_does_not_disturb_an_open_tunnel() {
        // ЦЕНТРАЛЬНОЕ СВОЙСТВО. Ради него писался свой мост: в macOS-версии
        // смена режима перезапускала gost и рвала всё живое, из-за чего там
        // понадобился трёхуровневый антифлаппинг.
        let target = origin().await;
        let (bridge, shared) = bridge_with(Route::Direct, "").await;
        let (mut c, reply) = connect_through(&bridge, &target).await;
        assert!(reply.starts_with("HTTP/1.1 200"));

        // переключаем маршрут на заведомо мёртвый апстрим
        shared.router.set(Route::Socks("127.0.0.1:1".into()));

        // уже открытый туннель обязан продолжать работать
        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        c.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");

        // Новое соединение обязано идти уже по новому маршруту, то есть в
        // мёртвый апстрим → 502. Целимся в ЖИВОЙ origin, а не во внешний
        // адрес: если снимок маршрута сломан и соединение унаследовало старый
        // Direct, оно дойдёт до origin и вернёт 200, и тест честно упадёт.
        // С внешним адресом он зеленел бы и при поломке — в CI без сети
        // прямой набор тоже не удался бы.
        let (_c2, reply2) = connect_through(&bridge, &target).await;
        assert!(reply2.starts_with("HTTP/1.1 502"), "получили: {reply2}");
    }

    #[tokio::test]
    async fn payload_sent_with_the_connect_head_is_not_lost() {
        // Клиенты шлют TLS ClientHello сразу за CONNECT, не дожидаясь 200.
        // Эти байты захватываются вместе с заголовком и обязаны уйти в
        // апстрим первыми — иначе ломается любое рукопожатие TLS.
        let target = origin().await;
        let (bridge, _) = bridge_with(Route::Direct, "").await;

        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\nping").as_bytes())
            .await
            .unwrap();

        // Ответ моста и ответ origin могут прийти как одним чтением, так и
        // разными — копим, пока не увидим «pong».
        let mut acc: Vec<u8> = Vec::new();
        let mut buf = [0u8; 64];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let n = tokio::time::timeout_at(deadline, c.read(&mut buf))
                .await
                .expect("origin не ответил на данные, отправленные вместе с заголовком")
                .unwrap();
            if n == 0 {
                break;
            }
            acc.extend_from_slice(&buf[..n]);
            if acc.windows(4).any(|w| w == b"pong") {
                break;
            }
        }

        let text = String::from_utf8_lossy(&acc).to_string();
        assert!(text.starts_with("HTTP/1.1 200"), "получили: {text}");
        assert!(
            acc.windows(4).any(|w| w == b"pong"),
            "хвост запроса потерян: {text}"
        );
    }

    /// Фальшивый SOCKS5-сервер, который не только жмёт руку, но и РЕЛЕИТ.
    ///
    /// Фальшивка из socks5.rs проверяет только рукопожатие; здесь нужен
    /// апстрим, через который реально текут байты — иначе поломка пересылки
    /// после успешного рукопожатия остаётся незамеченной.
    async fn fake_socks5_relay() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    // приветствие: версия 5, список методов
                    let mut hello = [0u8; 2];
                    if c.read_exact(&mut hello).await.is_err() {
                        return;
                    }
                    let mut methods = vec![0u8; hello[1] as usize];
                    if c.read_exact(&mut methods).await.is_err() {
                        return;
                    }
                    if c.write_all(&[0x05, 0x00]).await.is_err() {
                        return;
                    }

                    // запрос CONNECT: мост обязан прислать ИМЯ (ATYP=0x03)
                    let mut req = [0u8; 5];
                    if c.read_exact(&mut req).await.is_err() {
                        return;
                    }
                    assert_eq!(req[1], 0x01, "ожидали команду CONNECT");
                    assert_eq!(req[3], 0x03, "ожидали ATYP=имя хоста (socks5h)");
                    let mut rest = vec![0u8; req[4] as usize + 2];
                    if c.read_exact(&mut rest).await.is_err() {
                        return;
                    }
                    let host = String::from_utf8_lossy(&rest[..req[4] as usize]).to_string();
                    let port = u16::from_be_bytes([rest[rest.len() - 2], rest[rest.len() - 1]]);

                    let Ok(mut origin) = TcpStream::connect((host.as_str(), port)).await else {
                        let _ = c
                            .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                            .await;
                        return;
                    };
                    if c.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let _ = tokio::io::copy_bidirectional(&mut c, &mut origin).await;
                });
            }
        });
        addr
    }

    /// Фальшивый вышестоящий HTTP-прокси: принимает CONNECT и релеит.
    async fn fake_http_relay() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let Ok(head) = read_head(&mut c, 8192).await else {
                        return;
                    };
                    assert!(
                        head.is_connect(),
                        "ожидали CONNECT, получили {}",
                        head.method
                    );
                    let Some((host, port)) = split_host_port(&head.target, 443) else {
                        return;
                    };
                    let Ok(mut origin) = TcpStream::connect((host.as_str(), port)).await else {
                        let _ = c.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                        return;
                    };
                    if c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if !head.leftover.is_empty() && origin.write_all(&head.leftover).await.is_err()
                    {
                        return;
                    }
                    let _ = tokio::io::copy_bidirectional(&mut c, &mut origin).await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn connect_through_socks5_upstream_tunnels_bytes() {
        // Спец 9.2: сквозная передача байтов через КАЖДЫЙ коннектор. Без
        // этого теста поломка пересылки после удачного рукопожатия SOCKS5
        // прошла бы мимо всего набора: остальные serve-тесты целятся в
        // заведомо мёртвый апстрим и проверяют только 502.
        let target = origin().await;
        let socks = fake_socks5_relay().await;
        let (bridge, _) = bridge_with(Route::Socks(socks), "").await;

        let (mut c, reply) = connect_through(&bridge, &target).await;
        // Ровно ответ моста и ничего сверх него: остаток рукопожатия с
        // апстримом, не вычитанный коннектором, вылез бы здесь хвостом.
        assert_eq!(
            reply, "HTTP/1.1 200 Connection established\r\n\r\n",
            "в ответ клиенту просочились байты апстрима"
        );
        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(5), c.read_exact(&mut b))
            .await
            .expect("ответ origin не дошёл через SOCKS5-апстрим")
            .unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn connect_through_http_upstream_tunnels_bytes() {
        let target = origin().await;
        let proxy = fake_http_relay().await;
        let (bridge, _) = bridge_with(Route::Http(proxy), "").await;

        let (mut c, reply) = connect_through(&bridge, &target).await;
        // Ровно ответ моста и ничего сверх него: остаток рукопожатия с
        // апстримом, не вычитанный коннектором, вылез бы здесь хвостом.
        assert_eq!(
            reply, "HTTP/1.1 200 Connection established\r\n\r\n",
            "в ответ клиенту просочились байты апстрима"
        );
        c.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(5), c.read_exact(&mut b))
            .await
            .expect("ответ origin не дошёл через HTTP-апстрим")
            .unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn banner_arriving_with_the_upstream_reply_is_not_lost() {
        // Апстрим-прокси кладёт «200» и приветствие origin в ОДИН сегмент —
        // так ведут себя протоколы, где первым говорит сервер (ssh, smtp,
        // imap, mysql). Читая ответ на наш CONNECT, мы захватываем эти байты
        // вместе с заголовком, и выбросить их — значит потерять начало
        // диалога: клиент молча ждёт приветствия, которого уже нет.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = l.local_addr().unwrap().to_string();
        let _fake = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 512];
            let _ = s.read(&mut buf).await;
            // именно одним write_all — иначе тест не воспроизводит склейку
            s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\nBANNER")
                .await
                .unwrap();
            // держим соединение открытым: туннель не должен закрыться раньше,
            // чем клиент успеет прочитать
            std::future::pending::<()>().await;
        });

        let (bridge, _) = bridge_with(Route::Http(proxy), "").await;
        let (mut c, reply) = connect_through(&bridge, "ssh.example.com:22").await;
        assert!(reply.starts_with("HTTP/1.1 200"), "получили: {reply}");

        let mut acc: Vec<u8> = reply.into_bytes();
        let mut buf = [0u8; 64];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !acc.windows(6).any(|w| w == b"BANNER") {
            let n = tokio::time::timeout_at(deadline, c.read(&mut buf))
                .await
                .expect("приветствие апстрима не дошло до клиента")
                .unwrap();
            if n == 0 {
                break;
            }
            acc.extend_from_slice(&buf[..n]);
        }
        assert!(
            acc.windows(6).any(|w| w == b"BANNER"),
            "приветствие потеряно: {}",
            String::from_utf8_lossy(&acc)
        );
    }

    /// Origin, отвечающий фиксированным HTTP-ответом и запоминающий запрос.
    async fn http_origin() -> (String, tokio::task::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 512];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });
        (addr, h)
    }

    #[tokio::test]
    async fn plain_http_is_forwarded_in_origin_form() {
        let (origin_addr, seen) = http_origin().await;
        let (bridge, _) = bridge_with(Route::Direct, "").await;

        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(
            format!("GET http://{origin_addr}/path?q=1 HTTP/1.1\r\nHost: {origin_addr}\r\nProxy-Connection: keep-alive\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();

        // Ответ может прийти несколькими сегментами — читаем до конца,
        // иначе тест изредка падал бы на разрыве по границе чтения.
        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut raw))
            .await
            .expect("ответ не пришёл целиком")
            .unwrap();
        let reply = String::from_utf8_lossy(&raw).to_string();
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "получили: {reply}");
        assert!(reply.ends_with("hi"));

        let request = seen.await.unwrap();
        // origin-form, а не absolute-form
        assert!(
            request.starts_with("GET /path?q=1 HTTP/1.1\r\n"),
            "origin увидел: {request}"
        );
        // hop-by-hop заголовок не должен просочиться
        assert!(!request.to_ascii_lowercase().contains("proxy-connection"));
        // v1 работает без keep-alive
        assert!(request.contains("Connection: close"));
    }

    /// Фальшивый вышестоящий HTTP-прокси для ОБЫЧНОГО HTTP (не CONNECT).
    ///
    /// В отличие от `fake_http_relay` (который жмёт руку через CONNECT), эта
    /// фальшивка вообще не умеет CONNECT: она читает запрос как обычный
    /// HTTP-запрос и требует, чтобы первая строка была absolute-form GET.
    /// Если мост всё-таки пошлёт CONNECT, `assert_eq!` ниже упадёт с внятным
    /// сообщением вместо того, чтобы тихо зависнуть или дать неверный relay.
    async fn fake_http_proxy_plain(origin_addr: String) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let Ok((mut c, _)) = l.accept().await else {
                return;
            };
            let Ok(head) = read_head(&mut c, 8192).await else {
                return;
            };
            let first_line = format!("{} {} {}", head.method, head.target, head.version);
            assert_eq!(
                first_line,
                format!("GET http://{origin_addr}/ HTTP/1.1"),
                "апстрим-http должен получать absolute-form запрос без CONNECT, получил: {first_line}"
            );
            let Ok(mut origin) = TcpStream::connect(&origin_addr).await else {
                return;
            };
            if !head.leftover.is_empty() && origin.write_all(&head.leftover).await.is_err() {
                return;
            }
            let _ = tokio::io::copy_bidirectional(&mut c, &mut origin).await;
        });
        addr
    }

    #[tokio::test]
    async fn plain_http_through_http_upstream_uses_absolute_form_not_connect() {
        // Спец 5.3: апстрим http получает запрос как есть, в absolute-form,
        // без CONNECT. У Squid из коробки (2.x-6.x) стоит
        // `acl SSL_ports port 443` + `http_access deny CONNECT !SSL_ports`,
        // коммерческие шлюзы по умолчанию тоже режут CONNECT на 80-й порт —
        // значит http:// через такой апстрим не должен зависеть от того,
        // разрешает ли прокси CONNECT вообще.
        let target = origin().await;
        let proxy = fake_http_proxy_plain(target.clone()).await;
        let (bridge, _) = bridge_with(Route::Http(proxy), "").await;

        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(
            format!("GET http://{target}/ HTTP/1.1\r\nHost: {target}\r\n\r\nping").as_bytes(),
        )
        .await
        .unwrap();

        let mut b = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(5), c.read_exact(&mut b))
            .await
            .expect("ответ от origin через http-апстрим не дошёл")
            .unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn plain_http_with_dead_upstream_yields_502() {
        let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "").await;
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();

        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut raw))
            .await
            .expect("ответ не пришёл целиком")
            .unwrap();
        let reply = String::from_utf8_lossy(&raw).to_string();
        assert!(reply.starts_with("HTTP/1.1 502"), "получили: {reply}");
    }

    #[tokio::test]
    async fn non_absolute_target_yields_400() {
        let (bridge, _) = bridge_with(Route::Direct, "").await;
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(b"GET /just/a/path HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();

        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut raw))
            .await
            .expect("ответ не пришёл целиком")
            .unwrap();
        let reply = String::from_utf8_lossy(&raw).to_string();
        assert!(reply.starts_with("HTTP/1.1 400"), "получили: {reply}");
    }

    #[tokio::test]
    async fn a_response_status_line_from_a_client_yields_400() {
        // parse двусторонний намеренно: им же коннектор читает ответ
        // апстрима. Но от КЛИЕНТА строка ответа — бессмыслица. Целимся во
        // второе поле живым absolute-form адресом: без проверки запрос
        // проваливался в handle_plain и уезжал к origin строкой
        // «HTTP/1.1 / OK», а клиент получал бодрый 200 от origin.
        let (origin_addr, _seen) = http_origin().await;
        let (bridge, _) = bridge_with(Route::Direct, "").await;

        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(
            format!("HTTP/1.1 http://{origin_addr}/ OK\r\nHost: {origin_addr}\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();

        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut raw))
            .await
            .expect("ответ не пришёл целиком")
            .unwrap();
        let reply = String::from_utf8_lossy(&raw).to_string();
        assert!(reply.starts_with("HTTP/1.1 400"), "получили: {reply}");
    }

    #[tokio::test]
    async fn exceeding_the_connection_limit_yields_503() {
        let target = origin().await;
        let shared = Arc::new(Shared {
            router: Arc::new(Router::new(Route::Direct)),
            bypass: Arc::new(BypassList::parse("")),
            limits: Limits {
                dial: Duration::from_secs(2),
                head: Duration::from_secs(2),
                max_connections: 1,
            },
        });
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bridge = l.local_addr().unwrap().to_string();
        let s2 = Arc::clone(&shared);
        tokio::spawn(async move { serve(l, s2).await });

        // Первое соединение занимает единственный слот и держит его открытым.
        let (_held, reply1) = connect_through(&bridge, &target).await;
        assert!(reply1.starts_with("HTTP/1.1 200"), "получили: {reply1}");

        // Второе обязано получить честный отказ, а не зависнуть.
        let mut c2 = TcpStream::connect(&bridge).await.unwrap();
        c2.write_all(format!("CONNECT {target} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = vec![0u8; 128];
        let n = c2.read(&mut buf).await.unwrap();
        assert!(
            String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 503"),
            "получили: {}",
            String::from_utf8_lossy(&buf[..n])
        );
    }
}
