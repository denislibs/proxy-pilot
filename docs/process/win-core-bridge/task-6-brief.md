### Task 6: Клиент SOCKS5

**Files:**
- Create: `win/crates/bridge/src/socks5.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: ничего.
- Produces: `socks5_handshake<S: AsyncRead + AsyncWrite + Unpin>(&mut S, host: &str, port: u16) -> Result<(), Socks5Error>`, `Socks5Error`.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/socks5.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Фальшивый SOCKS5-сервер: отвечает заранее заданными байтами и
    /// возвращает всё, что ему прислал клиент.
    async fn fake_server(reply_greeting: Vec<u8>, reply_connect: Vec<u8>) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut b = [0u8; 3];
            s.read_exact(&mut b).await.unwrap();
            seen.extend_from_slice(&b);
            s.write_all(&reply_greeting).await.unwrap();
            if reply_greeting.get(1) == Some(&0x00) {
                let mut hdr = [0u8; 5];
                s.read_exact(&mut hdr).await.unwrap();
                seen.extend_from_slice(&hdr);
                let mut rest = vec![0u8; hdr[4] as usize + 2];
                s.read_exact(&mut rest).await.unwrap();
                seen.extend_from_slice(&rest);
                s.write_all(&reply_connect).await.unwrap();
            }
            seen
        });
        (addr, h)
    }

    #[tokio::test]
    async fn sends_hostname_not_resolved_address() {
        // socks5h: резолвить должен апстрим — иначе внутренние имена
        // офиса не разрешатся на нашей стороне.
        let (addr, h) = fake_server(vec![0x05, 0x00], vec![0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        socks5_handshake(&mut s, "git.company.kz", 443).await.unwrap();

        let seen = h.await.unwrap();
        assert_eq!(&seen[0..3], &[0x05, 0x01, 0x00]);
        assert_eq!(&seen[3..8], &[0x05, 0x01, 0x00, 0x03, 14]);
        assert_eq!(&seen[8..22], b"git.company.kz");
        assert_eq!(&seen[22..24], &443u16.to_be_bytes());
    }

    #[tokio::test]
    async fn accepts_ipv4_bound_address_in_reply() {
        let (addr, _h) = fake_server(vec![0x05, 0x00], vec![0x05, 0x00, 0x00, 0x01, 1, 2, 3, 4, 0, 80]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(socks5_handshake(&mut s, "h", 80).await.is_ok());
    }

    #[tokio::test]
    async fn accepts_domain_bound_address_in_reply() {
        let mut reply = vec![0x05, 0x00, 0x00, 0x03, 3];
        reply.extend_from_slice(b"abc");
        reply.extend_from_slice(&[0, 80]);
        let (addr, _h) = fake_server(vec![0x05, 0x00], reply).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(socks5_handshake(&mut s, "h", 80).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_server_demanding_auth() {
        // Мы не храним секретов, поэтому апстрим с логином не поддерживаем —
        // но обязаны сказать это внятно, а не зависнуть.
        let (addr, _h) = fake_server(vec![0x05, 0x02], vec![]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(matches!(
            socks5_handshake(&mut s, "h", 80).await,
            Err(Socks5Error::AuthRequired(0x02))
        ));
    }

    #[tokio::test]
    async fn surfaces_refusal_code() {
        let (addr, _h) = fake_server(vec![0x05, 0x00], vec![0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(matches!(
            socks5_handshake(&mut s, "h", 80).await,
            Err(Socks5Error::Refused(0x05))
        ));
    }

    #[tokio::test]
    async fn rejects_non_socks5_greeting() {
        let (addr, _h) = fake_server(vec![0x04, 0x00], vec![]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        assert!(matches!(
            socks5_handshake(&mut s, "h", 80).await,
            Err(Socks5Error::BadVersion(0x04))
        ));
    }

    #[tokio::test]
    async fn rejects_overlong_hostname() {
        let (addr, _h) = fake_server(vec![0x05, 0x00], vec![]).await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        let long = "a".repeat(256);
        assert!(matches!(
            socks5_handshake(&mut s, &long, 80).await,
            Err(Socks5Error::HostTooLong)
        ));
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge socks5`
Expected: FAIL — `socks5_handshake` не определён.

- [ ] **Step 3: Написать минимальную реализацию**

Вставь в начало `win/crates/bridge/src/socks5.rs`:

```rust
//! Клиентская сторона SOCKS5 — ровно столько, сколько нужно мосту.
//!
//! Адрес назначения передаётся ИМЕНЕМ (ATYP=0x03), а не разрешённым
//! адресом: резолвить должен апстрим, иначе внутренние имена офиса не
//! разрешатся на нашей стороне. Это семантика `socks5h`.
//!
//! Авторизация не поддерживается сознательно: продукт не хранит секретов.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, thiserror::Error)]
pub enum Socks5Error {
    #[error("апстрим ответил версией {0:#04x}, а не SOCKS5")]
    BadVersion(u8),
    #[error("апстрим требует авторизацию (метод {0:#04x}), а мы её не поддерживаем")]
    AuthRequired(u8),
    #[error("апстрим отказал, код {0:#04x}")]
    Refused(u8),
    #[error("апстрим вернул неизвестный тип адреса {0:#04x}")]
    BadAtyp(u8),
    #[error("имя хоста длиннее 255 байт")]
    HostTooLong,
    #[error("ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn socks5_handshake<S>(s: &mut S, host: &str, port: u16) -> Result<(), Socks5Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let host_bytes = host.as_bytes();
    if host_bytes.len() > 255 {
        return Err(Socks5Error::HostTooLong);
    }

    // приветствие: версия 5, один метод, «без авторизации»
    s.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut greeting = [0u8; 2];
    s.read_exact(&mut greeting).await?;
    if greeting[0] != 0x05 {
        return Err(Socks5Error::BadVersion(greeting[0]));
    }
    if greeting[1] != 0x00 {
        return Err(Socks5Error::AuthRequired(greeting[1]));
    }

    // запрос CONNECT с именем хоста
    let mut req = Vec::with_capacity(7 + host_bytes.len());
    req.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8]);
    req.extend_from_slice(host_bytes);
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await?;

    let mut reply = [0u8; 4];
    s.read_exact(&mut reply).await?;
    if reply[0] != 0x05 {
        return Err(Socks5Error::BadVersion(reply[0]));
    }
    if reply[1] != 0x00 {
        return Err(Socks5Error::Refused(reply[1]));
    }

    // адрес привязки нам не нужен, но вычитать его обязаны — иначе он
    // окажется в потоке данных и испортит первое же чтение
    match reply[3] {
        0x01 => {
            let mut skip = [0u8; 6];
            s.read_exact(&mut skip).await?;
        }
        0x04 => {
            let mut skip = [0u8; 18];
            s.read_exact(&mut skip).await?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut skip = vec![0u8; len[0] as usize + 2];
            s.read_exact(&mut skip).await?;
        }
        other => return Err(Socks5Error::BadAtyp(other)),
    }

    Ok(())
}
```

`win/crates/bridge/src/lib.rs`:

```rust
pub mod http;
pub mod router;
pub mod socks5;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 22 теста.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): клиент SOCKS5 с передачей имени хоста"
```

---

