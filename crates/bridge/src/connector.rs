//! Набор соединения до назначения по выбранному маршруту.
//!
//! Таймаут набора — наш и настраиваемый. В macOS-версии он был зашит
//! в gost и равен 15 секундам, из-за чего каждый запрос к недоступному
//! апстриму вешал браузер на эти 15 секунд; половина логики режимов там
//! существовала только чтобы в такой апстрим не смотреть.

use std::time::Duration;

use proxypilot_core::mode::Route;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tracing::debug;

use crate::http::{read_head, HeadError};
use crate::socks5::{socks5_handshake, Socks5Error};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("не уложились в таймаут набора")]
    Timeout,
    #[error("апстрим {addr} недоступен: {source}")]
    Upstream {
        addr: String,
        source: std::io::Error,
    },
    #[error("не удалось соединиться с {host}:{port}: {source}")]
    Origin {
        host: String,
        port: u16,
        source: std::io::Error,
    },
    #[error("апстрим-прокси ответил статусом {0}")]
    UpstreamStatus(u16),
    #[error("апстрим-прокси ответил неразборчиво: {0}")]
    UpstreamReply(#[from] HeadError),
    #[error(transparent)]
    Socks(#[from] Socks5Error),
}

/// Соединение до назначения плюс байты, которые апстрим успел прислать
/// вместе с ответом на наш CONNECT.
///
/// Читая ответ вышестоящего прокси, мы почти всегда захватываем часть того,
/// что идёт следом: у протоколов, где первым говорит сервер (ssh, smtp,
/// imap, mysql), приветствие origin приходит тем же сегментом, что и «200».
/// Выбросить эти байты — значит потерять начало диалога, не заметив этого.
/// Это ровно та же ошибка, что описана в http.rs про клиентскую сторону,
/// только на стороне апстрима.
#[derive(Debug)]
pub struct Upstream {
    pub stream: TcpStream,
    /// пусто для direct и socks5 — там мы читаем ответ фиксированной длины
    pub pending: Vec<u8>,
}

pub async fn connect_via(
    route: &Route,
    host: &str,
    port: u16,
    dial: Duration,
) -> Result<Upstream, ConnectError> {
    let result = match tokio::time::timeout(dial, connect_inner(route, host, port)).await {
        Err(_) => Err(ConnectError::Timeout),
        Ok(r) => r,
    };
    // debug, а не warn: вызывающий уже логирует эту же ошибку с бОльшим
    // контекстом (маршрут известен ему), дублировать на том же уровне незачем.
    if let Err(e) = &result {
        debug!(route = ?route, %host, port, error = %e, "не удалось соединиться");
    }
    result
}

async fn connect_inner(route: &Route, host: &str, port: u16) -> Result<Upstream, ConnectError> {
    match route {
        Route::Direct => {
            let stream =
                TcpStream::connect((host, port))
                    .await
                    .map_err(|source| ConnectError::Origin {
                        host: host.to_string(),
                        port,
                        source,
                    })?;
            Ok(Upstream {
                stream,
                pending: Vec::new(),
            })
        }

        Route::Socks(addr) => {
            let mut s = dial_upstream(addr).await?;
            socks5_handshake(&mut s, host, port).await?;
            Ok(Upstream {
                stream: s,
                pending: Vec::new(),
            })
        }

        Route::Http(addr) => {
            let mut s = dial_upstream(addr).await?;
            let target = format_target(host, port);
            let request = format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n\r\n"
            );
            s.write_all(request.as_bytes())
                .await
                .map_err(|source| ConnectError::Upstream {
                    addr: addr.clone(),
                    source,
                })?;

            let head = read_head(&mut s, 8192).await?;
            let status: u16 = head
                .target // в ответе на месте target стоит код статуса
                .parse()
                .map_err(|_| ConnectError::UpstreamStatus(0))?;
            if status != 200 {
                return Err(ConnectError::UpstreamStatus(status));
            }
            // Хвост, склеенный с ответом прокси, отдаём вызывающему: это уже
            // данные origin, и их место — в клиентском направлении.
            Ok(Upstream {
                stream: s,
                pending: head.leftover,
            })
        }
    }
}

async fn dial_upstream(addr: &str) -> Result<TcpStream, ConnectError> {
    TcpStream::connect(addr)
        .await
        .map_err(|source| ConnectError::Upstream {
            addr: addr.to_string(),
            source,
        })
}

/// Набор до вышестоящего HTTP-прокси БЕЗ рукопожатия CONNECT.
///
/// Нужен обычному (не CONNECT) HTTP по спеке 5.3: запрос к апстриму `http`
/// уходит в absolute-form как есть, напрямую в TCP-соединение с прокси —
/// это соединение просто набирается, а не туннелируется. `pending` тут
/// всегда пуст: в отличие от CONNECT, где апстрим сначала отвечает своей
/// строкой статуса и мог приклеить к ней байты origin, здесь первым
/// говорим мы, и до записи запроса апстрим прислать ничего не может.
pub async fn dial_upstream_plain(addr: &str, dial: Duration) -> Result<Upstream, ConnectError> {
    match tokio::time::timeout(dial, dial_upstream(addr)).await {
        Err(_) => Err(ConnectError::Timeout),
        Ok(Ok(stream)) => Ok(Upstream {
            stream,
            pending: Vec::new(),
        }),
        Ok(Err(e)) => Err(e),
    }
}

/// IPv6 в строке запроса обязан быть в скобках.
fn format_target(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn echo_server() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut b = [0u8; 4];
            let _ = s.read_exact(&mut b).await;
            let _ = s.write_all(b"pong").await;
        });
        addr
    }

    #[tokio::test]
    async fn direct_connects_to_origin() {
        let addr = echo_server().await;
        let (host, port) = crate::http::split_host_port(&addr, 80).unwrap();
        let up = connect_via(&Route::Direct, &host, port, Duration::from_secs(3))
            .await
            .unwrap();
        assert!(up.pending.is_empty());
        let mut s = up.stream;
        s.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        s.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"pong");
    }

    #[tokio::test]
    async fn dial_timeout_is_honoured() {
        // Слушатель принимает соединение и молчит: рукопожатие SOCKS5 никогда
        // не завершится, и сработать обязан наш таймаут. Ровно эта проблема
        // была с зашитыми в gost 15 секундами.
        //
        // Недостижимый адрес вроде 10.255.255.1 для проверки не годится:
        // поведение зависит от ОС, Windows может мгновенно ответить «сеть
        // недоступна», и тест поймал бы Origin вместо Timeout.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let _silent = tokio::spawn(async move {
            let _accepted = l.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let started = std::time::Instant::now();
        let r = connect_via(
            &Route::Socks(addr),
            "example.com",
            443,
            Duration::from_millis(300),
        )
        .await;
        let elapsed = started.elapsed();
        assert!(matches!(&r, Err(ConnectError::Timeout)), "получили: {r:?}");
        assert!(elapsed < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn refused_upstream_reports_error() {
        // порт 1 на loopback закрыт
        let r = connect_inner(&Route::Socks("127.0.0.1:1".into()), "example.com", 443).await;
        assert!(matches!(r, Err(ConnectError::Upstream { .. })));
    }

    #[tokio::test]
    async fn http_upstream_sends_connect_and_accepts_200() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let seen = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 128];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });

        let up = connect_via(
            &Route::Http(addr),
            "example.com",
            443,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert!(up.pending.is_empty());
        let request = seen.await.unwrap();
        assert!(request.starts_with("CONNECT example.com:443 HTTP/1.1\r\n"));
        assert!(request.contains("Host: example.com:443\r\n"));
    }

    #[tokio::test]
    async fn http_upstream_keeps_bytes_glued_to_the_reply() {
        // Прокси отдаёт «200» и приветствие origin одним сегментом — так
        // выглядит ssh/smtp/imap через корпоративный прокси. Эти байты
        // обязаны дойти до вызывающего, а не быть выброшены вместе с Head.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 128];
            let _ = s.read(&mut buf).await;
            let _ = s
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0-OpenSSH_9.6\r\n")
                .await;
            std::future::pending::<()>().await;
        });

        let up = connect_via(
            &Route::Http(addr),
            "ssh.example.com",
            22,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(up.pending, b"SSH-2.0-OpenSSH_9.6\r\n");
    }

    #[tokio::test]
    async fn http_upstream_non_200_is_an_error() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 128];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
        });

        let r = connect_via(
            &Route::Http(addr),
            "example.com",
            443,
            Duration::from_secs(2),
        )
        .await;
        assert!(matches!(r, Err(ConnectError::UpstreamStatus(403))));
    }
}
