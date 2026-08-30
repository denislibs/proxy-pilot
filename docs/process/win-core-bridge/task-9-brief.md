### Task 9: Обычный HTTP

**Files:**
- Modify: `win/crates/bridge/src/serve.rs`

**Interfaces:**
- Consumes: всё из задачи 8.
- Produces: `handle_plain(client: TcpStream, head: Head, shared: Arc<Shared>)`, вызывается из `handle` вместо ответа 501.

- [ ] **Step 1: Написать падающий тест**

Добавь в блок `mod tests` в `serve.rs`:

```rust
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

        let mut buf = vec![0u8; 256];
        let n = c.read(&mut buf).await.unwrap();
        let reply = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "получили: {reply}");
        assert!(reply.ends_with("hi"));

        let request = seen.await.unwrap();
        // origin-form, а не absolute-form
        assert!(request.starts_with("GET /path?q=1 HTTP/1.1\r\n"), "origin увидел: {request}");
        // hop-by-hop заголовок не должен просочиться
        assert!(!request.to_ascii_lowercase().contains("proxy-connection"));
        // v1 работает без keep-alive
        assert!(request.contains("Connection: close"));
    }

    #[tokio::test]
    async fn plain_http_with_dead_upstream_yields_502() {
        let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "").await;
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 256];
        let n = c.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 502"));
    }

    #[tokio::test]
    async fn non_absolute_target_yields_400() {
        let (bridge, _) = bridge_with(Route::Direct, "").await;
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(b"GET /just/a/path HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 256];
        let n = c.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 400"));
    }
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge serve::tests::plain`
Expected: FAIL — сейчас отдаётся 501.

- [ ] **Step 3: Написать минимальную реализацию**

В `handle` замени ветку `else` на вызов `handle_plain`:

```rust
    if head.is_connect() {
        handle_connect(client, head, shared).await;
    } else {
        handle_plain(client, head, shared).await;
    }
```

Добавь в `serve.rs`:

```rust
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

    let route = pick_route(&host, &shared);

    let mut upstream = match connect_via(&route, &host, port, shared.limits.dial).await {
        Ok(s) => s,
        Err(e) => {
            let _ = respond(&mut client, 502, &format!("upstream: {e}")).await;
            return;
        }
    };

    let mut request = format!("{} {} {}\r\n", head.method, path, head.version);
    for (name, value) in &head.headers {
        if is_hop_by_hop(name) {
            continue;
        }
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");

    if upstream.write_all(request.as_bytes()).await.is_err() {
        let _ = respond(&mut client, 502, "upstream write failed").await;
        return;
    }
    if !head.leftover.is_empty() && upstream.write_all(&head.leftover).await.is_err() {
        return;
    }

    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
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
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 35 тестов.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): обычный HTTP — origin-form, без hop-by-hop, без keep-alive"
```

---

