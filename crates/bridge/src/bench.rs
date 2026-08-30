//! Сравнение настроенных маршрутов между собой по скорости.
//!
//! Один поток и короткий файл — намеренно: цифры сравнивают маршруты друг
//! с другом, а не измеряют пропускную способность линии. Для последнего
//! нужны много параллельных потоков и разгон, здесь этого сознательно нет.
//! Если подать эти цифры как измерение канала, пользователь справедливо
//! в них не поверит.
//!
//! Мост в сравнении не участвует как путь: он проверка, а не маршрут.
//! Измерить его значило бы измерить один из тех же трёх путей плюс
//! накладные расходы на лишний хоп — то есть не то же самое сравнение.
//!
//! HTTP-библиотека тут не нужна: запрос — это один статический `GET`,
//! а тащить зависимость ради него в дерево, которое рано или поздно
//! придётся подписывать, не стоит того.

use std::time::{Duration, Instant};

use proxypilot_core::mode::{Route, Upstreams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::connector::connect_via;
use crate::http::{read_head, split_host_port};

/// Сколько байт заголовка ответа готовы прочитать, прежде чем сдаться.
/// То же значение, что и у чтения ответа апстрима на CONNECT (`connector.rs`)
/// — заголовок собственного простого GET заведомо в него укладывается.
const HEAD_CAP: usize = 8192;

/// Результат одного замера одного маршрута.
#[derive(Debug, Clone)]
pub struct BenchResult {
    /// Человекочитаемая подпись маршрута — то, что покажет UI.
    pub label: String,
    pub route: Route,
    /// Сколько байт ТЕЛА успели прочитать (0 при ошибке). Заголовки ответа
    /// в это число не входят: файл короткий намеренно (см. заголовок
    /// модуля), и заголовки были бы заметной долей `limit`, исказив цифру,
    /// которую видит пользователь как скорость передачи.
    pub bytes: u64,
    /// От начала набора до последнего прочитанного байта, а при ошибке —
    /// до момента, когда она случилась.
    pub elapsed: Duration,
    pub error: Option<String>,
}

impl BenchResult {
    /// `None` при ошибке или при нулевом времени — иначе «0 байт за 0
    /// секунд» превратилось бы в деление на ноль или в бесконечность, и
    /// провалившийся путь выглядел бы бесконечно быстрым.
    pub fn speed_bps(&self) -> Option<u64> {
        if self.error.is_some() {
            return None;
        }
        let secs = self.elapsed.as_secs_f64();
        if secs <= 0.0 {
            return None;
        }
        Some((self.bytes as f64 / secs) as u64)
    }
}

/// Самый быстрый среди отработавших замеров. Провалившиеся строки и строки
/// без скорости (нулевое время) в сравнение не входят.
///
/// При равенстве скоростей побеждает последний по порядку `results` —
/// таково поведение `Iterator::max_by_key`, которым это и реализовано.
/// Здесь это не выбор в чью-то пользу, а честная фиксация факта: порядок
/// маршрутов не гарантирует ничего для равных скоростей, и рассчитывать
/// на «первый» не стоит.
pub fn fastest(results: &[BenchResult]) -> Option<&BenchResult> {
    results
        .iter()
        .filter_map(|r| r.speed_bps().map(|speed| (speed, r)))
        .max_by_key(|(speed, _)| *speed)
        .map(|(_, r)| r)
}

/// Измерить каждый настроенный маршрут: всегда `Direct`, плюс `Socks`/`Http`,
/// если заданы в `up`. Ни одна ошибка не превращается в панику и не даёт
/// молча пропущенную строку — путь, который не отработал, обязан быть
/// показан как не отработавший, иначе это выглядело бы как «не настроен».
pub async fn bench_all(
    up: &Upstreams,
    url: &str,
    limit: u64,
    timeout: Duration,
) -> Vec<BenchResult> {
    let mut routes = vec![("Напрямую".to_string(), Route::Direct)];
    if let Some(addr) = &up.socks {
        routes.push(("SOCKS5".to_string(), Route::Socks(addr.clone())));
    }
    if let Some(addr) = &up.http {
        routes.push(("HTTP-прокси".to_string(), Route::Http(addr.clone())));
    }

    let mut results = Vec::with_capacity(routes.len());
    for (label, route) in routes {
        results.push(bench_one(label, route, url, limit, timeout).await);
    }
    results
}

async fn bench_one(
    label: String,
    route: Route,
    url: &str,
    limit: u64,
    timeout: Duration,
) -> BenchResult {
    let started = Instant::now();
    // Таймаут оборачивает набор, запрос и чтение целиком одним вызовом —
    // «целиком, а не по-фазно»: если раздать этот же таймаут отдельно на
    // каждую фазу, суммарно замер мог бы растянуться на кратное ему время.
    let outcome = tokio::time::timeout(timeout, run(&route, url, limit, timeout)).await;
    let elapsed = started.elapsed();
    match outcome {
        Ok(Ok(bytes)) => BenchResult {
            label,
            route,
            bytes,
            elapsed,
            error: None,
        },
        Ok(Err(e)) => BenchResult {
            label,
            route,
            bytes: 0,
            elapsed,
            error: Some(e),
        },
        Err(_) => BenchResult {
            label,
            route,
            bytes: 0,
            elapsed,
            error: Some("не уложились в таймаут".to_string()),
        },
    }
}

/// Набрать маршрут, отправить простой `GET` и прочитать ТЕЛО ответа до
/// `limit` байт или до конца соединения. Заголовки ответа разбираются
/// через `read_head` (тот же разбор, что и для ответа апстрима на CONNECT)
/// и в счёт байт не идут — считается только то, что пришло после них.
async fn run(route: &Route, url: &str, limit: u64, dial: Duration) -> Result<u64, String> {
    let (host, port, path) = parse_url(url)?;

    let upstream = connect_via(route, &host, port, dial)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = upstream.stream;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: proxypilot-bench\r\n\r\n",
        host_header(&host, port)
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;

    // `pending` — байты origin, которые апстрим успел приклеить к своему
    // ответу на CONNECT (см. connector.rs). Для собственного GET такого
    // почти не бывает, но если случилось — это уже реальные данные ответа,
    // и `read_head` обязан видеть их вместе с тем, что придёт следом по
    // сокету: `chain` склеивает их в один поток байт для разбора заголовка,
    // а после этого блока заимствование `stream` освобождается и чтение
    // тела продолжается напрямую из сокета.
    let head = {
        let mut reader = std::io::Cursor::new(upstream.pending).chain(&mut stream);
        read_head(&mut reader, HEAD_CAP)
            .await
            .map_err(|e| e.to_string())?
    };

    let mut total = head.leftover.len() as u64;
    let mut buf = [0u8; 8192];
    while total < limit {
        let n = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
    }
    Ok(total)
}

/// Значение заголовка `Host`: с портом, если он не 80 (RFC 7230 §5.4).
/// Виртуальный хостинг на некоторых серверах ключуется именно по этому
/// заголовку, и без порта на нестандартном порту это был бы не тот адрес.
fn host_header(host: &str, port: u16) -> String {
    if port == 80 {
        host.to_string()
    } else if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Минимальный разбор `http://host[:port]/path` — ровно то, что нужно
/// собственному GET. Полноценный разбор URL — за пределами задачи и
/// лишняя зависимость, которой здесь нет ни одной причины появляться.
fn parse_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("замер поддерживает только http://: {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = split_host_port(authority, 80)
        .ok_or_else(|| format!("не удалось разобрать адрес: {authority}"))?;
    Ok((host, port, path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn res(label: &str, bytes: u64, ms: u64, err: Option<&str>) -> BenchResult {
        BenchResult {
            label: label.to_string(),
            route: Route::Direct,
            bytes,
            elapsed: Duration::from_millis(ms),
            error: err.map(|e| e.to_string()),
        }
    }

    #[test]
    fn speed_is_bytes_over_seconds() {
        assert_eq!(res("x", 1_000_000, 1000, None).speed_bps(), Some(1_000_000));
        assert_eq!(res("x", 500_000, 500, None).speed_bps(), Some(1_000_000));
    }

    #[test]
    fn a_failed_measurement_has_no_speed() {
        // Иначе «0 байт за 0 секунд» превратилось бы в деление на ноль
        // или в бесконечность, и путь выглядел бы бесконечно быстрым.
        assert_eq!(res("x", 0, 0, Some("отказ")).speed_bps(), None);
        assert_eq!(res("x", 0, 100, Some("отказ")).speed_bps(), None);
    }

    #[test]
    fn a_zero_duration_does_not_divide_by_zero() {
        assert_eq!(res("x", 1000, 0, None).speed_bps(), None);
    }

    #[test]
    fn fastest_ignores_failures() {
        let rs = vec![
            res("мёртвый", 0, 0, Some("отказ")),
            res("медленный", 100_000, 1000, None),
            res("быстрый", 900_000, 1000, None),
        ];
        assert_eq!(fastest(&rs).map(|r| r.label.as_str()), Some("быстрый"));
    }

    #[test]
    fn fastest_of_nothing_is_nothing() {
        assert!(fastest(&[]).is_none());
        assert!(fastest(&[res("x", 0, 0, Some("отказ"))]).is_none());
    }

    #[tokio::test]
    async fn a_dead_upstream_yields_an_error_not_a_hang() {
        let up = Upstreams {
            socks: Some("127.0.0.1:1".into()),
            http: None,
        };
        let started = std::time::Instant::now();
        let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(500)).await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "замер обязан укладываться в таймаут"
        );
        assert!(
            rs.iter().any(|r| r.error.is_some()),
            "мёртвый путь обязан быть помечен ошибкой"
        );
    }

    #[tokio::test]
    async fn every_configured_route_is_measured_and_labelled() {
        let up = Upstreams {
            socks: Some("127.0.0.1:1".into()),
            http: Some("127.0.0.1:2".into()),
        };
        let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(300)).await;
        // напрямую + socks + http
        assert_eq!(
            rs.len(),
            3,
            "получили: {:?}",
            rs.iter().map(|r| &r.label).collect::<Vec<_>>()
        );
        assert!(rs.iter().any(|r| matches!(r.route, Route::Direct)));
        assert!(rs.iter().any(|r| matches!(r.route, Route::Socks(_))));
        assert!(rs.iter().any(|r| matches!(r.route, Route::Http(_))));
    }

    #[tokio::test]
    async fn an_unconfigured_upstream_is_not_measured() {
        let up = Upstreams {
            socks: None,
            http: None,
        };
        let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(300)).await;
        assert_eq!(rs.len(), 1, "только «напрямую»");
    }

    #[tokio::test]
    async fn reported_bytes_are_the_body_not_the_headers() {
        // Заголовки специально сделаны заметно длиннее тела: если бы они
        // попадали в счёт байт, число разошлось бы с длиной тела на порядок,
        // а не на пару байт погрешности.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let body = "hello-body";
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 512];
            let _ = s.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-Padding: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                "x".repeat(200),
                body
            );
            let _ = s.write_all(response.as_bytes()).await;
        });

        let up = Upstreams {
            socks: None,
            http: None,
        };
        let url = format!("http://{addr}/");
        let rs = bench_all(&up, &url, 10_000, Duration::from_secs(2)).await;
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].error, None, "получили: {:?}", rs[0].error);
        assert_eq!(rs[0].bytes, body.len() as u64);
    }
}
