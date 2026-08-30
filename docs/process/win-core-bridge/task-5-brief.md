### Task 5: Разбор заголовка запроса

**Files:**
- Create: `win/crates/bridge/src/http.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: ничего.
- Produces: `Head { method: String, target: String, version: String, headers: Vec<(String, String)>, leftover: Vec<u8> }`, `read_head<R: AsyncRead + Unpin>(&mut R, max: usize) -> Result<Head, HeadError>`, `split_host_port(&str, u16) -> Option<(String, u16)>`, `Head::is_connect(&self) -> bool`.

**Критично:** читая заголовок, мы почти всегда захватываем лишние байты — клиенты присылают TLS ClientHello сразу за `CONNECT`, не дожидаясь ответа. Эти байты обязаны сохраниться в `leftover` и уйти в апстрим первыми, иначе рукопожатие TLS сломается. Это самая частая ошибка в самописных прокси.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/http.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    async fn head_of(input: &[u8]) -> Result<Head, HeadError> {
        let mut cursor = std::io::Cursor::new(input.to_vec());
        read_head(&mut cursor, 8192).await
    }

    #[tokio::test]
    async fn parses_connect() {
        let h = head_of(b"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: api.anthropic.com:443\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(h.method, "CONNECT");
        assert_eq!(h.target, "api.anthropic.com:443");
        assert_eq!(h.version, "HTTP/1.1");
        assert!(h.is_connect());
        assert_eq!(h.headers.len(), 1);
        assert!(h.leftover.is_empty());
    }

    #[tokio::test]
    async fn keeps_bytes_that_follow_the_head() {
        // Клиент шлёт TLS ClientHello, не дожидаясь нашего 200. Потерять эти
        // байты — сломать рукопожатие.
        let h = head_of(b"CONNECT h:443 HTTP/1.1\r\n\r\n\x16\x03\x01ABC")
            .await
            .unwrap();
        assert_eq!(h.leftover, b"\x16\x03\x01ABC");
    }

    #[tokio::test]
    async fn parses_absolute_form_request() {
        let h = head_of(b"GET http://example.com/a?b=1 HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(h.method, "GET");
        assert_eq!(h.target, "http://example.com/a?b=1");
        assert!(!h.is_connect());
    }

    #[tokio::test]
    async fn header_names_keep_value_spacing_trimmed() {
        let h = head_of(b"CONNECT h:443 HTTP/1.1\r\nProxy-Connection:   keep-alive  \r\n\r\n")
            .await
            .unwrap();
        assert_eq!(h.headers[0].0, "Proxy-Connection");
        assert_eq!(h.headers[0].1, "keep-alive");
    }

    #[tokio::test]
    async fn oversized_head_is_rejected() {
        let mut big = b"CONNECT h:443 HTTP/1.1\r\n".to_vec();
        big.extend(std::iter::repeat(b'x').take(9000));
        let mut cursor = std::io::Cursor::new(big);
        assert!(matches!(read_head(&mut cursor, 4096).await, Err(HeadError::TooLarge)));
    }

    #[tokio::test]
    async fn truncated_input_is_an_error() {
        assert!(matches!(
            head_of(b"CONNECT h:443 HTTP/1.1\r\n").await,
            Err(HeadError::Truncated)
        ));
    }

    #[tokio::test]
    async fn garbage_request_line_is_an_error() {
        assert!(matches!(head_of(b"nonsense\r\n\r\n").await, Err(HeadError::Malformed)));
    }

    #[tokio::test]
    async fn parses_a_response_status_line_too() {
        // Тот же разбор используется для ответа вышестоящего HTTP-прокси на
        // наш CONNECT (задача 7). Разбор позиционный, поэтому у ответа
        // «HTTP/1.1 200 OK» версия попадает в method, а код — в target.
        let h = head_of(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
        assert_eq!(h.method, "HTTP/1.1");
        assert_eq!(h.target, "200");
    }

    #[test]
    fn splits_host_and_port() {
        assert_eq!(split_host_port("h:443", 80), Some(("h".into(), 443)));
        assert_eq!(split_host_port("h", 80), Some(("h".into(), 80)));
    }

    #[test]
    fn splits_bracketed_ipv6() {
        assert_eq!(split_host_port("[::1]:443", 80), Some(("::1".into(), 443)));
        assert_eq!(split_host_port("[::1]", 80), Some(("::1".into(), 80)));
    }

    #[test]
    fn rejects_bad_port() {
        assert_eq!(split_host_port("h:0", 80), None);
        assert_eq!(split_host_port("h:99999", 80), None);
        assert_eq!(split_host_port("h:abc", 80), None);
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge http`
Expected: FAIL — `read_head` не определён.

- [ ] **Step 3: Написать минимальную реализацию**

Добавь в `win/crates/bridge/Cargo.toml` в `[dev-dependencies]`:

```toml
[dev-dependencies]
tokio = { workspace = true, features = ["rt", "macros", "io-util"] }
```

Вставь в начало `win/crates/bridge/src/http.rs`:

```rust
//! Разбор заголовка запроса от клиента.
//!
//! Нам нужны только строка запроса и заголовки — тело мы не интерпретируем,
//! а переливаем. Ключевая тонкость: читая заголовок, мы почти всегда
//! захватываем часть следующих байтов (клиенты шлют TLS ClientHello сразу
//! за CONNECT, не дожидаясь ответа). Они сохраняются в `leftover` и уходят
//! в апстрим первыми.

use tokio::io::{AsyncRead, AsyncReadExt};

const TERMINATOR: &[u8] = b"\r\n\r\n";

#[derive(Debug, thiserror::Error)]
pub enum HeadError {
    #[error("заголовок больше допустимого")]
    TooLarge,
    #[error("соединение закрылось до конца заголовка")]
    Truncated,
    #[error("нечитаемая строка запроса")]
    Malformed,
    #[error("ошибка чтения: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct Head {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
    /// байты, прочитанные сверх заголовка
    pub leftover: Vec<u8>,
}

impl Head {
    pub fn is_connect(&self) -> bool {
        self.method.eq_ignore_ascii_case("CONNECT")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub async fn read_head<R: AsyncRead + Unpin>(r: &mut R, max: usize) -> Result<Head, HeadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    loop {
        if let Some(pos) = find_terminator(&buf) {
            let body_at = pos + TERMINATOR.len();
            let leftover = buf[body_at..].to_vec();
            let mut head = parse(&buf[..pos])?;
            head.leftover = leftover;
            return Ok(head);
        }
        if buf.len() >= max {
            return Err(HeadError::TooLarge);
        }
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Err(HeadError::Truncated);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn find_terminator(buf: &[u8]) -> Option<usize> {
    buf.windows(TERMINATOR.len()).position(|w| w == TERMINATOR)
}

/// Разбор позиционный и намеренно годится и для строки ЗАПРОСА
/// (`CONNECT host:443 HTTP/1.1`), и для строки ОТВЕТА
/// (`HTTP/1.1 200 OK`) — последняя нужна задаче 7, чтобы прочитать ответ
/// вышестоящего HTTP-прокси на наш CONNECT. У ответа версия оказывается
/// в `method`, а код статуса — в `target`.
fn parse(head: &[u8]) -> Result<Head, HeadError> {
    let text = std::str::from_utf8(head).map_err(|_| HeadError::Malformed)?;
    let mut lines = text.split("\r\n");

    let first_line = lines.next().ok_or(HeadError::Malformed)?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().ok_or(HeadError::Malformed)?.to_string();
    let target = parts.next().ok_or(HeadError::Malformed)?.to_string();
    let version = parts.next().unwrap_or("").to_string();
    // «HTTP/x» обязан присутствовать: в запросе третьим полем, в ответе первым
    if !method.starts_with("HTTP/") && !version.starts_with("HTTP/") {
        return Err(HeadError::Malformed);
    }

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':').ok_or(HeadError::Malformed)?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }

    Ok(Head { method, target, version, headers, leftover: Vec::new() })
}

/// Разбирает `host:port`. IPv6 приходит в скобках: `[::1]:443`.
/// Порт 0 и вне диапазона отвергаются.
pub fn split_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => parse_port(p)?,
            None if tail.is_empty() => default_port,
            None => return None,
        };
        return Some((host.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Some((host.to_string(), parse_port(port)?)),
        Some(_) => None,
        None => Some((s.to_string(), default_port)),
    }
}

fn parse_port(s: &str) -> Option<u16> {
    match s.parse::<u16>() {
        Ok(p) if p > 0 => Some(p),
        _ => None,
    }
}
```

`win/crates/bridge/src/lib.rs`:

```rust
pub mod http;
pub mod router;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 15 тестов.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): разбор заголовка запроса с сохранением хвоста"
```

---

