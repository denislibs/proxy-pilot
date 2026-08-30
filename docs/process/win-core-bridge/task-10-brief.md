### Task 10: Бинарь и ручная проверка

**Files:**
- Create: `win/crates/bridge/src/main.rs`
- Modify: `win/crates/bridge/Cargo.toml`

**Interfaces:**
- Consumes: `Config` (3), `decide` (1), `Router` (4), `serve`/`Shared`/`Limits` (8).
- Produces: исполняемый `proxypilot-bridge` с аргументами `--port`, `--socks`, `--http`, `--mode`, `--no-proxy`.

- [ ] **Step 1: Написать падающий тест**

Добавь `win/crates/bridge/tests/cli.rs`:

```rust
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_proxypilot-bridge")
}

#[test]
fn rejects_an_invalid_upstream() {
    let out = Command::new(bin())
        .args(["--socks", "нет-порта"])
        .output()
        .expect("бинарь должен запускаться");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("host:port"));
}

#[test]
fn prints_usage_on_help() {
    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--port"));
    assert!(text.contains("--socks"));
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge --test cli`
Expected: FAIL — бинарной цели нет.

- [ ] **Step 3: Написать минимальную реализацию**

Добавь в `win/crates/bridge/Cargo.toml`:

```toml
[[bin]]
name = "proxypilot-bridge"
path = "src/main.rs"
```

`win/crates/bridge/src/main.rs`:

```rust
//! Тонкий запускатель моста — для ручной проверки и как основа будущего
//! приложения. Разбор аргументов свой: одна зависимость меньше, а флагов
//! здесь пять.

use std::sync::Arc;
use std::time::Duration;

use proxypilot_core::bypass::BypassList;
use proxypilot_core::config::{validate_upstream, Config};
use proxypilot_core::mode::{decide, Health, Mode, Place, Reachability};
use proxypilot_bridge::router::Router;
use proxypilot_bridge::serve::{serve, Limits, Shared};
use tokio::net::TcpListener;

const USAGE: &str = "\
proxypilot-bridge — локальный HTTP-CONNECT мост

  --port <N>          порт моста (по умолчанию 3129)
  --socks <host:port> апстрим SOCKS5
  --http <host:port>  апстрим HTTP-прокси
  --mode <режим>      socks | http | direct | auto (по умолчанию auto)
  --no-proxy <список> адреса мимо апстрима, через запятую
  --help              эта справка

Клиенты ходят на http://127.0.0.1:<порт>. Смена маршрута не разрывает
установленные соединения.
";

fn main() {
    if let Err(e) = run() {
        eprintln!("proxypilot-bridge: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut cfg = Config::default();
    // Сначала в вектор, потом по индексу: замыкание-хелпер над итератором
    // здесь спорит с заимствованием, а так всё прямолинейно.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;

    while i < args.len() {
        let flag = args[i].as_str();
        // значение следующего аргумента, с внятной ошибкой если его нет
        let mut next = || {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("у {flag} нет значения"))
        };
        match flag {
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--port" => {
                cfg.bridge_port = next()?
                    .parse()
                    .map_err(|_| "порт: число 1..65535".to_string())?;
                i += 1;
            }
            "--socks" => {
                cfg.socks_upstream = Some(next()?);
                i += 1;
            }
            "--http" => {
                cfg.http_upstream = Some(next()?);
                i += 1;
            }
            "--no-proxy" => {
                cfg.no_proxy = next()?;
                i += 1;
            }
            "--mode" => {
                cfg.mode = match next()?.as_str() {
                    "socks" => Mode::Socks,
                    "http" => Mode::Http,
                    "direct" => Mode::Direct,
                    "auto" => Mode::Auto,
                    other => return Err(format!("неизвестный режим: {other}")),
                };
                i += 1;
            }
            other => return Err(format!("неизвестный аргумент: {other}")),
        }
        i += 1;
    }

    for (name, value) in [("--socks", &cfg.socks_upstream), ("--http", &cfg.http_upstream)] {
        if let Some(v) = value {
            if !validate_upstream(v) {
                return Err(format!("{name}: нужен формат host:port, получено «{v}»"));
            }
        }
    }

    // В этом плане мы ещё не умеем определять сеть — считаем, что мы в офисе,
    // и что заданные апстримы живы. Опознание сети и проба живости придут
    // в плане 2 вместе с модулем winnet.
    let health = Health {
        socks: if cfg.socks_upstream.is_some() { Reachability::Up } else { Reachability::Down },
        http: if cfg.http_upstream.is_some() { Reachability::Up } else { Reachability::Down },
    };
    let decision = decide(cfg.mode, &cfg.upstreams(), Place { in_office: true }, health);

    let shared = Arc::new(Shared {
        router: Arc::new(Router::new(decision.route.clone())),
        bypass: Arc::new(BypassList::parse(&cfg.no_proxy)),
        limits: Limits {
            dial: Duration::from_millis(cfg.dial_timeout_ms),
            head: Duration::from_millis(cfg.head_timeout_ms),
            max_connections: cfg.max_connections,
        },
    });

    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        // Строго loopback: на 0.0.0.0 это был бы открытый прокси для всей
        // локальной сети.
        let addr = format!("127.0.0.1:{}", cfg.bridge_port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("не занять {addr}: {e}"))?;
        println!("мост слушает http://{addr}, маршрут: {:?}", decision.route);
        serve(listener, shared).await.map_err(|e| e.to_string())
    })
}
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 37 тестов.

- [ ] **Step 5: Проверить руками**

В одном окне:

```bash
cd win && cargo run -p proxypilot-bridge -- --mode direct
```

В другом:

```bash
curl -x http://127.0.0.1:3129 -sS -o /dev/null -w '%{http_code}\n' https://api.anthropic.com/v1/messages
```

Expected: `401` или `405` — то есть до сервиса дошли (без ключа он так и отвечает). Код `000` означает, что мост не отработал.

Проверь и обычный HTTP:

```bash
curl -x http://127.0.0.1:3129 -sS -o /dev/null -w '%{http_code}\n' http://example.com/
```

Expected: `200`.

- [ ] **Step 6: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): бинарь моста с разбором аргументов"
```

---

