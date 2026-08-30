# Task 1 report — Замер путей

## Post-review fix (commit `b56bece`)

Coordinator review confirmed the control-flow contract (timeout wraps dial+write+read as one budget, no silently omitted route, bounded read, `Router` untouched, no new dependency, RED capture genuine) and raised one Important finding plus two Minor ones.

**Finding 1 (Important) — `bytes` counted raw wire bytes, including the HTTP response's status line and headers, not just the body.** The brief said "read the body up to `limit` bytes"; the original `run()` wrote the `GET` and then accumulated every byte read off the socket straight into `total`, so headers rode along as if they were payload. Fixed by parsing the response head with the existing `crate::http::read_head` (the same function `connector.rs` already uses for the CONNECT reply): `upstream.pending` (bytes glued to a CONNECT reply, almost always empty for a plain GET but handled correctly if not) is joined with the live socket via `tokio::io::AsyncReadExt::chain` inside a scoped block so the mutable borrow of `stream` is released afterward, `read_head` finds the `\r\n\r\n` terminator across that joined stream, and `total` is seeded from `head.leftover.len()` — only bytes past the header terminator count from then on. Added constant `HEAD_CAP: usize = 8192`, matching the cap already used for the CONNECT-reply parse in `connector.rs`.

**Finding 2 (Minor, folded in) — no test exercised the happy path against a real listener**, which is exactly the test that would have caught Finding 1. Added `reported_bytes_are_the_body_not_the_headers`: a mock TCP listener (same `TcpListener::bind("127.0.0.1:0")` pattern as `connector.rs`'s `echo_server`) sends a response with deliberately padded headers (`Content-Length` plus a 200-byte `X-Padding` header) around a 10-byte body, and asserts `rs[0].bytes == body.len() as u64` with `rs[0].error == None`. This assertion fails against the pre-fix code (would have reported body length + ~250 bytes of headers) and passes now.

**Finding 3 (Minor, folded in) — the `Host` header omitted the port.** Added `host_header(host, port)`: returns the bare host when `port == 80`, otherwise `host:port` (bracketing IPv6 hosts, mirroring `connector.rs`'s `format_target`), per RFC 7230 §5.4 and to correctly address virtual-hosted servers keyed on `Host`.

**Not folded in, documented instead** — `fastest`'s tie-breaking: added a doc comment on `fastest` stating explicitly that ties resolve to the *last* maximum by `results` order, because that's `Iterator::max_by_key`'s documented behavior and not a deliberate choice, so nobody downstream assumes "first wins."

No other code changed; `bench_all`, `bench_one`, `speed_bps`, `fastest`'s logic, `BenchResult`'s fields, and `parse_url` are unchanged from the first pass except for the doc-comment addition above.

### CI re-run — verbatim

`cargo test -p proxypilot-bridge bench`:

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.61s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-31ec35a3b789e712.exe)

running 9 tests
test bench::tests::a_failed_measurement_has_no_speed ... ok
test bench::tests::a_zero_duration_does_not_divide_by_zero ... ok
test bench::tests::fastest_of_nothing_is_nothing ... ok
test bench::tests::fastest_ignores_failures ... ok
test bench::tests::speed_is_bytes_over_seconds ... ok
test bench::tests::reported_bytes_are_the_body_not_the_headers ... ok
test bench::tests::an_unconfigured_upstream_is_not_measured ... ok
test bench::tests::every_configured_route_is_measured_and_labelled ... ok
test bench::tests::a_dead_upstream_yields_an_error_not_a_hang ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 60 filtered out; finished in 1.03s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-129f707e33809f40.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-ba39af5c2ba4b27d.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s
```

`cargo test --all` (result lines per crate):

```
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   (proxypilot-app)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s   (proxypilot-bridge lib — was 68, +1 new test)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (proxypilot-bridge main.rs unittests)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s    (cli.rs)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s   (proxypilot-core)
test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s   (proxypilot-winnet)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (doc-tests x3)
```

Total: 165 passed, 1 ignored (pre-existing `watch_a_real_network_change`), 0 failed. Delta from the first pass: +1 (the new happy-path test).

`cargo clippy --all-targets -- -D warnings`:

```
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.26s
```

`cargo fmt --all --check`: exit 0, no output (clean on first try this round — no reformatting needed).

Commit: `b56bece` — `fix(win): замер путей — считать только тело ответа, не заголовки`

## Files

- Created: `win/crates/bridge/src/bench.rs`
- Modified: `win/crates/bridge/src/lib.rs` (added `pub mod bench;` alphabetically before `connector`)

Commit: `7890884` — `feat(win): замер путей — сравнение маршрутов между собой`

## What was built

`bench.rs` adds:

- `BenchResult { label: String, route: Route, bytes: u64, elapsed: Duration, error: Option<String> }`
- `BenchResult::speed_bps(&self) -> Option<u64>` — bytes/elapsed, `None` on error or zero elapsed time
- `fastest(&[BenchResult]) -> Option<&BenchResult>` — max by `speed_bps()`, skipping rows where it's `None`
- `bench_all(&Upstreams, url: &str, limit: u64, timeout: Duration) -> Vec<BenchResult>` — always measures `Direct`, plus `Socks`/`Http` when configured in `Upstreams`

Implementation of a single measurement (`run`, wrapped by `bench_one`):

1. Minimal manual parse of `http://host[:port]/path` (function `parse_url`), reusing the existing `crate::http::split_host_port` helper — no `url` crate, no HTTP library.
2. `connect_via(route, host, port, dial)` to dial the route.
3. Write a bare `GET {path} HTTP/1.1\r\nHost: ...\r\nConnection: close\r\n...` by hand over the resulting stream.
4. Read into an 8KiB buffer until `limit` bytes are reached or the connection reaches EOF, counting `upstream.pending` bytes (glued to the HTTP-CONNECT reply) as already-received.
5. The whole `run(...)` future — dial + write + read — is wrapped in one `tokio::time::timeout(timeout, ...)` in `bench_one`, so `timeout` bounds the measurement as a whole, not per phase. `elapsed` is measured from before that call to right after it resolves (success, error, or timeout).
6. Any failure (parse error, connect error, write error, read error, or the outer timeout firing) is turned into `BenchResult { bytes: 0, error: Some(...), .. }` — never a panic, never a skipped row.

`bench_all` never consults `Router` — it takes `Upstreams` directly, per the constraint that `Router::get()` keeps exactly one non-test call site (`serve.rs::pick_route`) and one non-test writer (`supervisor.rs`). Verified via grep after the change: those are still the only non-test `router.get()`/`.get()` call sites.

Labels chosen: `"Напрямую"`, `"SOCKS5"`, `"HTTP-прокси"` (arbitrary — the brief left the exact label text unspecified; task 5, per the plan ledger, will consume `bench_all` for the tray menu and can rename freely).

No new dependency was added — `Cargo.toml` for `proxypilot-bridge` is untouched.

## TDD evidence

### Step 1/2 — RED capture (verbatim)

`bench.rs` was first created containing only the `#[cfg(test)] mod tests { ... }` block verbatim from the brief, and `pub mod bench;` was added to `lib.rs`. Command run: `cargo test -p proxypilot-bridge bench`

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
error[E0425]: cannot find type `BenchResult` in this scope
 --> crates\bridge\src\bench.rs:6:68
  |
6 |     fn res(label: &str, bytes: u64, ms: u64, err: Option<&str>) -> BenchResult {
  |                                                                    ^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `BenchResult` in this scope
 --> crates\bridge\src\bench.rs:7:9
  |
7 |         BenchResult {
  |         ^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `Upstreams` in this scope
  --> crates\bridge\src\bench.rs:53:18
   |
53 |         let up = Upstreams { socks: Some("127.0.0.1:1".into()), http: None };
   |                  ^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `Upstreams` in this scope
  --> crates\bridge\src\bench.rs:62:18
   |
62 |         let up = Upstreams { socks: Some("127.0.0.1:1".into()), http: Some("127.0.0.1:2".into()) };
   |                  ^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `Upstreams` in this scope
  --> crates\bridge\src\bench.rs:73:18
   |
73 |         let up = Upstreams { socks: None, http: None };
   |                  ^^^^^^^^^ not found in this scope

warning: unused import: `super::*`
 --> crates\bridge\src\bench.rs:3:9
  |
3 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0433]: cannot find type `Route` in this scope
 --> crates\bridge\src\bench.rs:9:20
  |
9 |             route: Route::Direct,
  |                    ^^^^^ use of undeclared type `Route`

error[E0425]: cannot find function `fastest` in this scope
  --> crates\bridge\src\bench.rs:42:20
   |
42 |         assert_eq!(fastest(&rs).map(|r| r.label.as_str()), Some("быстрый"));
   |                    ^^^^^^^ not found in this scope

error[E0425]: cannot find function `fastest` in this scope
  --> crates\bridge\src\bench.rs:47:17
   |
47 |         assert!(fastest(&[]).is_none());
   |                 ^^^^^^^ not found in this scope

error[E0425]: cannot find function `fastest` in this scope
  --> crates\bridge\src\bench.rs:48:17
   |
48 |         assert!(fastest(&[res("x", 0, 0, Some("отказ"))]).is_none());
   |                 ^^^^^^^ not found in this scope

error[E0425]: cannot find function `bench_all` in this scope
  --> crates\bridge\src\bench.rs:55:18
   |
55 |         let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(500)).await;
   |                  ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `bench_all` in this scope
  --> crates\bridge\src\bench.rs:63:18
   |
63 |         let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(300)).await;
   |                  ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `bench_all` in this scope
  --> crates\bridge\src\bench.rs:74:18
   |
74 |         let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(300)).await;
   |                  ^^^^^^^^^ not found in this scope

error[E0433]: cannot find type `Route` in this scope
  --> crates\bridge\src\bench.rs:68:53
   |
68 |         assert!(rs.iter().any(|r| matches!(r.route, Route::Http(_))));
   |                                                     ^^^^^ use of undeclared type `Route`

error[E0433]: cannot find type `Route` in this scope
  --> crates\bridge\src\bench.rs:66:53
   |
66 |         assert!(rs.iter().any(|r| matches!(r.route, Route::Direct)));
   |                                                     ^^^^^ use of undeclared type `Route`

error[E0433]: cannot find type `Route` in this scope
  --> crates\bridge\src\bench.rs:67:53
   |
67 |         assert!(rs.iter().any(|r| matches!(r.route, Route::Socks(_))));
   |                                                     ^^^^^ use of undeclared type `Route`

Some errors have detailed explanations: E0422, E0425, E0433.
For more information about an error, try `rustc --explain E0422`.
warning: `proxypilot-bridge` (lib test) generated 1 warning
error: could not compile `proxypilot-bridge` (lib test) due to 15 previous errors; 1 warning emitted
warning: build failed, waiting for other jobs to finish...
```

Type errors reached, as required (module was declared so it wasn't "file not found for module").

### Step 3/4 — GREEN

After writing the implementation, `cargo test -p proxypilot-bridge bench`:

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.14s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-31ec35a3b789e712.exe)

running 8 tests
test bench::tests::a_failed_measurement_has_no_speed ... ok
test bench::tests::speed_is_bytes_over_seconds ... ok
test bench::tests::a_zero_duration_does_not_divide_by_zero ... ok
test bench::tests::fastest_of_nothing_is_nothing ... ok
test bench::tests::fastest_ignores_failures ... ok
test bench::tests::an_unconfigured_upstream_is_not_measured ... ok
test bench::tests::every_configured_route_is_measured_and_labelled ... ok
test bench::tests::a_dead_upstream_yields_an_error_not_a_hang ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 60 filtered out; finished in 1.03s
     ... (main.rs unittests, cli.rs — unaffected, 0/2 tests as before)
```

All 8 tests from the brief pass, unmodified in substance (only reformatted by `cargo fmt`, see below).

## CI commands — verbatim

### `cargo test --all`

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src\main.rs (target\debug\deps\proxypilot-f9c0433b09311d11.exe)

running 24 tests
... (proxypilot-app: 24 passed, 0 failed)
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 68 tests
test bench::tests::a_failed_measurement_has_no_speed ... ok
test bench::tests::speed_is_bytes_over_seconds ... ok
test bench::tests::fastest_ignores_failures ... ok
test bench::tests::a_zero_duration_does_not_divide_by_zero ... ok
test bench::tests::fastest_of_nothing_is_nothing ... ok
... (existing http/log/probe/router/connector/serve/socks5/supervisor tests) ...
test bench::tests::an_unconfigured_upstream_is_not_measured ... ok
test bench::tests::every_configured_route_is_measured_and_labelled ... ok
test bench::tests::a_dead_upstream_yields_an_error_not_a_hang ... ok
... 
test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)

running 48 tests
... (bypass/config/mode tests, unchanged)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-382daa61fec08b04.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
... (networks/events/sysproxy/com tests, unchanged)
test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s

   Doc-tests proxypilot_bridge
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_winnet
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Total: 24 + 68 + 0 + 2 + 48 + 22 = 164 passed, 1 ignored (`watch_a_real_network_change`, pre-existing manual test), 0 failed. Before this task: 156 passed + 1 ignored. Delta: +8, exactly the new `bench` tests.

### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.70s
```

Clean, no warnings, no `#[allow]` added.

### `cargo fmt --all --check`

First run (before running `cargo fmt --all`) failed — the test module, pasted verbatim from the brief, was not `rustfmt`-clean (single-line struct literals and `assert!`/`assert_eq!` calls that exceed the line-width limit). Diff (abridged, showing the three reformatted call sites):

```
Diff in ...bench.rs:227:
     async fn a_dead_upstream_yields_an_error_not_a_hang() {
-        let up = Upstreams { socks: Some("127.0.0.1:1".into()), http: None };
+        let up = Upstreams {
+            socks: Some("127.0.0.1:1".into()),
+            http: None,
+        };
         let started = std::time::Instant::now();
         let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(500)).await;
-        assert!(started.elapsed() < Duration::from_secs(5), "замер обязан укладываться в таймаут");
-        assert!(rs.iter().any(|r| r.error.is_some()), "мёртвый путь обязан быть помечен ошибкой");
+        assert!(
+            started.elapsed() < Duration::from_secs(5),
+            "замер обязан укладываться в таймаут"
+        );
+        assert!(
+            rs.iter().any(|r| r.error.is_some()),
+            "мёртвый путь обязан быть помечен ошибкой"
+        );
     }
... (two more similar hunks: every_configured_route_is_measured_and_labelled, an_unconfigured_upstream_is_not_measured)
```

`cargo fmt --all` was run to apply this reformatting (whitespace/line-wrapping only — no logic, assertions, or test names changed). After that:

```
$ cargo fmt --all --check
FMT OK   (exit 0, no output)
```

## Self-review checklist

- Every configured route produces a row, including failed ones: yes — `bench_all` always builds the `routes` vec first (Direct + configured Socks/Http), then unconditionally pushes a `BenchResult` per entry via `bench_one`, which itself always returns a `BenchResult` (success, error, or timeout branch) and never propagates an error out of the function. Covered by `every_configured_route_is_measured_and_labelled` and `a_dead_upstream_yields_an_error_not_a_hang`.
- The whole measurement (dial + request + read) is bounded by `timeout` via one `tokio::time::timeout(timeout, run(...))` in `bench_one`, not per-phase. `connect_via`'s own `dial` parameter is also fed `timeout`, but that's redundant/harmless since it's nested inside the same outer timeout.
- `speed_bps` cannot divide by zero or report a failed route as infinitely fast: it returns `None` for any `error.is_some()` and for `elapsed.as_secs_f64() <= 0.0`, checked before the division.
- No dependency added — reused `connect_via`, `split_host_port`, and hand-wrote the `GET` line and read loop with `tokio::io`.
- Test output is pristine: `cargo test --all` shows only `ok` lines plus the one pre-existing `ignored` line; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all --check` are both clean.

## Concerns

None blocking. Two minor judgment calls worth flagging for reviewers:

1. Route labels (`"Напрямую"`, `"SOCKS5"`, `"HTTP-прокси"`) are my choice — the brief's interface only specifies `label: String`, and per the plan's task-pairing table (T1 ↔ T5), the tray-menu task consuming `bench_all` can rename them freely without touching this file's logic.
2. `run()` passes the same `timeout` value both as the outer `tokio::time::timeout` bound and as `connect_via`'s internal `dial` duration. This is intentional (the outer timeout is authoritative and bounds everything regardless), but a reviewer might expect `dial` to be a fraction of `timeout` instead — I judged that unnecessary complexity since the requirement only asks for one overall bound, not a dial/read split.
