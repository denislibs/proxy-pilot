### Task 8: Обслуживание соединения — CONNECT

**Files:**
- Create: `win/crates/bridge/src/serve.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Router` (4), `read_head`/`split_host_port` (5), `connect_via` (7), `BypassList` (2).
- Produces: `Limits { dial: Duration, head: Duration, max_connections: usize }`, `Shared { router: Arc<Router>, bypass: Arc<BypassList>, limits: Limits }`, `serve(listener: TcpListener, shared: Arc<Shared>) -> std::io::Result<()>`, `handle(stream: TcpStream, shared: Arc<Shared>)`.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/serve.rs`:

```rust
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
                let Ok((mut s, _)) = l.accept().await else { return };
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

        // а новое соединение — уже по новому маршруту, то есть 502
        let (_c2, reply2) = connect_through(&bridge, "example.com:443").await;
        assert!(reply2.starts_with("HTTP/1.1 502"), "получили: {reply2}");
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge serve`
Expected: FAIL — `serve`, `Shared`, `Limits` не определены.

- [ ] **Step 3: Написать минимальную реализацию**

Вставь в начало `win/crates/bridge/src/serve.rs`:

```rust
//! Приём и обслуживание клиентских соединений.
//!
//! Маршрут читается ОДИН РАЗ на соединение, в момент приёма. Дальше
//! соединение живёт с этим значением, что бы ни переключили потом —
//! именно поэтому смена маршрута не рвёт живой трафик.

use std::sync::Arc;
use std::time::Duration;

use proxypilot_core::bypass::BypassList;
use proxypilot_core::mode::Route;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use crate::connector::connect_via;
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
    let permits = Arc::new(Semaphore::new(shared.limits.max_connections));
    loop {
        let (stream, _) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        let permits = Arc::clone(&permits);

        tokio::spawn(async move {
            // Превышение лимита — честный отказ, а не молчаливое исчерпание
            // ресурсов: клиент должен узнать, что произошло.
            let Ok(permit) = permits.clone().try_acquire_owned() else {
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
    let head = match tokio::time::timeout(
        shared.limits.head,
        read_head(&mut client, 16 * 1024),
    )
    .await
    {
        Err(_) => {
            let _ = respond(&mut client, 408, "request head timed out").await;
            return;
        }
        Ok(Err(e)) => {
            let _ = respond(&mut client, 400, &format!("bad request: {e}")).await;
            return;
        }
        Ok(Ok(h)) => h,
    };

    if head.is_connect() {
        handle_connect(client, head, shared).await;
    } else {
        let _ = respond(&mut client, 501, "plain HTTP not implemented yet").await;
    }
}

async fn handle_connect(mut client: TcpStream, head: Head, shared: Arc<Shared>) {
    let Some((host, port)) = split_host_port(&head.target, 443) else {
        let _ = respond(&mut client, 400, "bad CONNECT target").await;
        return;
    };

    // Снимок маршрута на всё время жизни соединения.
    let route = pick_route(&host, &shared);

    let mut upstream = match connect_via(&route, &host, port, shared.limits.dial).await {
        Ok(s) => s,
        Err(e) => {
            // Тихого перехода на direct здесь нет и быть не должно: это была
            // бы утечка трафика мимо выбранного маршрута. Клиент получает
            // внятную ошибку, решение о смене маршрута принимает core.
            let _ = respond(&mut client, 502, &format!("upstream: {e}")).await;
            return;
        }
    };

    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }

    // Байты, захваченные вместе с заголовком (обычно TLS ClientHello),
    // обязаны уйти первыми — иначе рукопожатие сломается.
    if !head.leftover.is_empty() && upstream.write_all(&head.leftover).await.is_err() {
        return;
    }

    // Таймаута простоя нет сознательно: long-poll, websocket и ssh молчат
    // законно. Полагаемся на TCP.
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
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
    s.flush().await
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
```

`win/crates/bridge/src/lib.rs`:

```rust
pub mod connector;
pub mod http;
pub mod router;
pub mod serve;
pub mod socks5;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 32 теста. Особенно проверь, что проходит `changing_route_does_not_disturb_an_open_tunnel` — это главный тест плана.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): обслуживание CONNECT с bypass, лимитом и 502 вместо тихого обхода"
```

---

