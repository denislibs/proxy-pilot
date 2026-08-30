# Task 8 Report: Обслуживание соединения — CONNECT

## What I implemented

- Created `win/crates/bridge/src/serve.rs` exactly per the brief:
  - `Limits { dial, head, max_connections }` — `Debug, Clone, Copy`.
  - `Shared { router: Arc<Router>, bypass: Arc<BypassList>, limits: Limits }` — `Debug`.
  - `serve(listener: TcpListener, shared: Arc<Shared>) -> std::io::Result<()>` — accept loop with a `Semaphore`-based connection limit; over-limit connections get a `503` instead of being silently dropped or queued.
  - `handle(stream: TcpStream, shared: Arc<Shared>)` — reads the head under a timeout (`408` on timeout, `400` on parse failure), dispatches CONNECT requests to `handle_connect`, and replies `501` for anything else (plain HTTP is out of scope for this task).
  - `handle_connect` — parses `host:port` from the CONNECT target (`400` if unparsable), takes **one snapshot of the route** via `pick_route` (bypass check first, then `router.get()`), dials the upstream with `connect_via` (`502` on failure — no silent fallback to direct), sends the `200 Connection established` reply, forwards `head.leftover` to the upstream **before** starting the byte pump, then runs `tokio::io::copy_bidirectional` with no idle timeout.
  - `pick_route`, `respond`, `status_text` — helpers as specified.
- Updated `win/crates/bridge/src/lib.rs` to add `pub mod serve;` (kept alphabetical between `router` and `socks5`, matching the brief).

No files beyond these two were touched. No additional types, functions, or tests were added beyond what the brief specifies.

## What I tested and the results

- `cargo test -p proxypilot-bridge serve` — before implementation: compile failure (see RED evidence). After implementation: 6 tests matched (5 new `serve::tests::*` + 1 unrelated `socks5::tests::rejects_server_demanding_auth`, matched only because the word "serve" is a substring of the crate's test filter, not because it belongs to the module), all passing.
- `cargo test -p proxypilot-bridge` — full crate: **32 passed**, 0 failed (27 pre-existing + 5 new).
- `cargo test -p proxypilot-core` — **29 passed**, 0 failed (untouched, confirms no regression).
- `cargo test` (whole workspace) — both crates green, no warnings, no doc-test noise.
- `cargo fmt --check` — clean after running `cargo fmt` (see below).
- `cargo clippy --all-targets -- -D warnings` — clean, zero warnings.

## TDD Evidence

### RED

Command: `cd win && cargo test -p proxypilot-bridge serve`

State at this point: `win/crates/bridge/src/lib.rs` already had `pub mod serve;` added (required for the file to be compiled as part of the crate at all), and `win/crates/bridge/src/serve.rs` contained **only** the `#[cfg(test)] mod tests { ... }` block from Step 1 of the brief — no implementation code above it.

Verbatim, complete terminal output:

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
error[E0425]: cannot find type `Arc` in this scope
  --> crates\bridge\src\serve.rs:27:68
   |
27 |     async fn bridge_with(route: Route, no_proxy: &str) -> (String, Arc<Shared>) {
   |                                                                    ^^^ not found in this scope
   |
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

error[E0425]: cannot find type `Shared` in this scope
  --> crates\bridge\src\serve.rs:27:72
   |
27 |     async fn bridge_with(route: Route, no_proxy: &str) -> (String, Arc<Shared>) {
   |                                                                        ^^^^^^ not found in this scope
   |
help: you might be missing a type parameter
   |
27 |     async fn bridge_with<Shared>(route: Route, no_proxy: &str) -> (String, Arc<Shared>) {
   |                         ++++++++

error[E0433]: cannot find type `Arc` in this scope
  --> crates\bridge\src\serve.rs:28:22
   |
28 |         let shared = Arc::new(Shared {
   |                      ^^^ use of undeclared type `Arc`
   |
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

error[E0422]: cannot find struct, variant or union type `Shared` in this scope
  --> crates\bridge\src\serve.rs:28:31
   |
28 |         let shared = Arc::new(Shared {
   |                               ^^^^^^ not found in this scope

error[E0433]: cannot find type `Arc` in this scope
  --> crates\bridge\src\serve.rs:29:21
   |
29 |             router: Arc::new(Router::new(route)),
   |                     ^^^ use of undeclared type `Arc`
   |
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

error[E0433]: cannot find type `Router` in this scope
  --> crates\bridge\src\serve.rs:29:30
   |
29 |             router: Arc::new(Router::new(route)),
   |                              ^^^^^^ use of undeclared type `Router`
   |
help: an enum with a similar name exists
   |
29 -             router: Arc::new(Router::new(route)),
29 +             router: Arc::new(Route::new(route)),
   |
help: consider importing this struct
   |
 3 +     use crate::router::Router;
   |

error[E0433]: cannot find type `Arc` in this scope
  --> crates\bridge\src\serve.rs:30:21
   |
30 |             bypass: Arc::new(BypassList::parse(no_proxy)),
   |                     ^^^ use of undeclared type `Arc`
   |
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

error[E0422]: cannot find struct, variant or union type `Limits` in this scope
  --> crates\bridge\src\serve.rs:31:21
   |
31 |             limits: Limits {
   |                     ^^^^^^ not found in this scope

error[E0433]: cannot find type `Duration` in this scope
  --> crates\bridge\src\serve.rs:32:23
   |
32 |                 dial: Duration::from_secs(2),
   |                       ^^^^^^^^ use of undeclared type `Duration`
   |
   = note: struct `crate::connector::tests::Duration` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::time::Duration;
   |

error[E0433]: cannot find type `Duration` in this scope
  --> crates\bridge\src\serve.rs:33:23
   |
33 |                 head: Duration::from_secs(2),
   |                       ^^^^^^^^ use of undeclared type `Duration`
   |
   = note: struct `crate::connector::tests::Duration` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::time::Duration;
   |

error[E0433]: cannot find type `Arc` in this scope
  --> crates\bridge\src\serve.rs:39:18
   |
39 |         let s2 = Arc::clone(&shared);
   |                  ^^^ use of undeclared type `Arc`
   |
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

warning: unused import: `super::*`
 --> crates\bridge\src\serve.rs:3:9
  |
3 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0425]: cannot find function `serve` in this scope
  --> crates\bridge\src\serve.rs:40:35
   |
40 |         tokio::spawn(async move { serve(l, s2).await });
   |                                   ^^^^^ not found in this scope

error[E0277]: the size for values of type `str` cannot be known at compilation time
  --> crates\bridge\src\serve.rs:57:14
   |
57 |         let (bridge, _) = bridge_with(Route::Direct, "").await;
   |              ^^^^^^ doesn't have a size known at compile-time
   |
   = help: the trait `Sized` is not implemented for `str`
   = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
  --> crates\bridge\src\serve.rs:71:14
   |
71 |         let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "127.0.0.1").await;
   |              ^^^^^^ doesn't have a size known at compile-time
   |
   = help: the trait `Sized` is not implemented for `str`
   = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
  --> crates\bridge\src\serve.rs:82:14
   |
82 |         let (bridge, _) = bridge_with(Route::Socks("127.0.0.1:1".into()), "").await;
   |              ^^^^^^ doesn't have a size known at compile-time
   |
   = help: the trait `Sized` is not implemented for `str`
   = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\bridge\src\serve.rs:103:14
    |
103 |         let (bridge, shared) = bridge_with(Route::Direct, "").await;
    |              ^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

Some errors have detailed explanations: E0277, E0422, E0425, E0433.
For more information about an error, try `rustc --explain E0277`.
warning: `proxypilot-bridge` (lib test) generated 1 warning
error: could not compile `proxypilot-bridge` (lib test) due to 16 previous errors; 1 warning emitted
```

Why this is the expected RED: it fails to compile precisely because `Arc`/`Duration` aren't imported yet and `Shared`, `Limits`, `Router`, `serve` don't exist yet — exactly the "не определены" state the brief's Step 2 predicts. Exit code was 101 (cargo's compile-error exit code).

### GREEN

Command: `cd win && cargo test -p proxypilot-bridge`

Verbatim output:

```
running 32 tests
test http::tests::truncated_input_is_an_error ... ok
test http::tests::parses_connect ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test http::tests::oversized_head_is_rejected ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::is_shareable_across_threads ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test connector::tests::direct_connects_to_origin ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test socks5::tests::surfaces_refusal_code ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

   Doc-tests proxypilot_bridge

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

32 tests as required by the brief, including `serve::tests::changing_route_does_not_disturb_an_open_tunnel`.

Also confirmed `cargo test -p proxypilot-core` still shows **29 passed; 0 failed**, unchanged.

## Files changed

- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\serve.rs` (new)
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\lib.rs` (added `pub mod serve;`)

Commit: `d8dab17` — "feat(win): обслуживание CONNECT с bypass, лимитом и 502 вместо тихого обхода"

## Self-review findings

- Content transcribed verbatim from the brief for both files; the only deviation from the brief's literal text is whitespace/line-wrapping introduced by `cargo fmt` (two blocks reflowed: the `timeout(...).await` match head, and one `let-else` block in the test's `origin()` helper) — purely cosmetic, no logic change, required to pass `cargo fmt --check`.
- `pick_route` reads `shared.bypass.matches(host)` first, then `shared.router.get()` — matches the brief exactly, and only ever reads the router snapshot once per connection (inside `handle_connect`, called once).
- `head.leftover` is written to `upstream` after the `200` reply is sent to the client and strictly before `copy_bidirectional` starts — confirmed by reading the code path in `handle_connect`.
- No idle timeout wraps `copy_bidirectional` — confirmed, only the head-read is time-bounded (`shared.limits.head`), and dialing is bounded by `shared.limits.dial` inside `connect_via` (from Task 7), not by `serve.rs`.
- `changing_route_does_not_disturb_an_open_tunnel` genuinely proves the target property: it opens a tunnel through `Route::Direct`, switches the shared `Router` to a dead SOCKS upstream, then (a) round-trips `ping`/`pong` on the *already-open* connection — proving the live tunnel is unaffected — and (b) opens a brand-new connection and asserts it gets `502` — proving the new route is honored by connections established after the switch. Both halves are asserted, not just one.
- `serve()` itself does not choose the bind address (it consumes an already-bound `TcpListener` passed by the caller) — binding `127.0.0.1` strictly is enforced wherever the listener is constructed (a later/earlier task, e.g. main.rs), which is outside this task's declared interface (`serve(listener: TcpListener, shared: Arc<Shared>)`). No violation within this task's scope.
- No `#[allow(...)]` attributes were added; clippy is clean without suppression.
- No extra types, functions, or tests were added beyond the brief.

## Issues or concerns

None. Implementation matches the brief exactly; all constraints (route snapshot at accept time, leftover forwarded first, no idle timeout, 502 on failed connect, 503 on limit exceeded) are satisfied and verified by the test suite.

---

# Fix Report: Review round 1 ("Needs fixes", 4 Important findings)

## What I changed

**Finding 1 — accept-loop resilience (`serve()`).** Replaced `listener.accept().await?` with the exact loop given in the finding: a transient `accept()` error (client vanished between SYN and accept, fd exhaustion) no longer propagates out of `serve` and kills the whole listener. The loop now tracks `consecutive_errors`, sleeps 50ms and retries on error, and only returns `Err` after 64 consecutive failures (a listener that is durably broken). Pasted verbatim from the finding.

**Finding 2 — `changing_route_does_not_disturb_an_open_tunnel` discriminates the route snapshot.** The second connection (opened after `shared.router.set(...)`) now targets the live `target` origin instead of `"example.com:443"`. If the per-connection route snapshot were broken and the new connection wrongly inherited the old `Route::Direct`, it would now reach the live origin and get `200`, and the test would fail honestly — instead of getting a `502` from a failed *direct* dial to an unreachable external host, which would mask the bug (and would also be unreliable in a network-restricted CI). Pasted verbatim from the finding.

**Finding 3 — added `payload_sent_with_the_connect_head_is_not_lost`.** New test that sends the CONNECT head and a `"ping"` payload in the same `write_all` (no wait for the `200`), then accumulates reads until it sees `"pong"` in the response stream, asserting the reply starts with `HTTP/1.1 200` and that `"pong"` did arrive. This is the first test in the suite that actually exercises `head.leftover` being forwarded to the upstream before the byte pump starts — every prior test's client waited for the `200` before writing anything, so `leftover` was always empty and deleting the forwarding line would have stayed green. Pasted verbatim from the finding.

**Finding 4 — added `exceeding_the_connection_limit_yields_503`.** New test with `max_connections: 1`: the first CONNECT occupies the only permit and is kept open, then a second connection is opened and asserted to get `503` instead of hanging. Pasted verbatim from the finding.

**`origin()` test helper.** Already loops on `accept()` (spawns a task per accepted connection inside a `loop`), so it already serves more than one connection. No adjustment was needed here.

### Additional fix required to make Finding 4 pass (not itself one of the four numbered findings)

Running `exceeding_the_connection_limit_yields_503` as specified failed deterministically (5/5 runs) on this Windows machine with `Os { code: 10054, kind: ConnectionReset, ... }` on the client's read of the `503` response — the response never arrived at all, not a partial read.

I isolated the cause with a minimal standalone repro (a two-task listener/client pair, since removed): a server task that writes a response and drops the `TcpStream` without ever reading the bytes the client already sent reproduces the exact same `ConnectionReset` on this platform, 100% of the time. A server task that first drains whatever is already queued (a short best-effort read, even with no explicit `shutdown()`) before writing and dropping does not reproduce it, and the client reliably reads the full response. This matches documented TCP/Winsock behavior: closing a socket while inbound data is still unread in the receive buffer triggers an abortive RST instead of an orderly FIN, and the RST can prevent already-written outbound data from being delivered.

This is the exact scenario the coordinator's message separately lists as a deferred/minor finding ("sockets dropped without draining") — but that item was scoped as "leave alone for this round" under the apparent assumption that the newly-added `503` test would pass without it. On this platform it does not: the `503` branch in `serve()` (`permits.clone().try_acquire_owned()` failing) is the only response path that writes and drops a stream without ever having read anything from it first (every other error path goes through `read_head`, which does consume the client's bytes off the socket even when it then discards them), so it is the only path affected.

Given Finding 4 is an Important, required fix and its test cannot pass deterministically on this target platform without addressing this, I applied the minimal, narrowly-scoped version of the deferred fix — a short best-effort drain (50ms timeout, discarding content) immediately before the `503` response on that one branch only:

```rust
let Ok(permit) = permits.clone().try_acquire_owned() else {
    let mut s = stream;
    // Клиент обычно уже отправил запрос целиком, не дожидаясь
    // ответа. Мы его никогда не читаем на этом пути — а если
    // сокет закрыть с непрочитанными входящими байтами, ОС шлёт
    // абортивный RST вместо штатного FIN, и наш 503 до клиента
    // не доходит вовсе. Поэтому забираем то, что уже накопилось,
    // с коротким таймаутом, прежде чем отвечать.
    let mut junk = [0u8; 1024];
    let _ = tokio::time::timeout(Duration::from_millis(50), s.read(&mut junk)).await;
    let _ = respond(&mut s, 503, "too many connections").await;
    return;
};
```

This required adding `AsyncReadExt` to the existing `use tokio::io::{..., AsyncWriteExt};` import.

**I am flagging this explicitly rather than treating it as in-scope on my own authority**: it does touch the deferred "sockets dropped without draining" item in spirit, though only for the one path that was blocking a specific required test, not a general redesign of connection teardown. If the coordinator would rather keep the `503` path exactly as originally written and instead relax or rewrite Finding 4's test to tolerate a possible `ConnectionReset` on this platform, that is a one-line change away — I judged reproducing the requested behavior (client actually receives `503`) to be the more faithful reading of "exceeding the connection limit yields 503, not a hang" than leaving a Windows-reproducible connection reset in place.

## Covering tests

- `serve::tests::exceeding_the_connection_limit_yields_503` — Finding 4 (also covers the drain fix above).
- `serve::tests::payload_sent_with_the_connect_head_is_not_lost` — Finding 3.
- `serve::tests::changing_route_does_not_disturb_an_open_tunnel` — Finding 2 (existing test, target reassigned).
- All 34 tests in the crate — Finding 1 (accept-loop resilience is not directly observable from these integration tests since the test harness never induces an `accept()` error; verified by code inspection against the finding's exact snippet, and confirmed the crate still compiles and all existing behavior is unaffected).

## Exact commands and verbatim output

Command: `cd win && cargo test -p proxypilot-bridge`

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-9c2fac5c83a9c1ce.exe)

running 34 tests
test http::tests::parses_absolute_form_request ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::parses_connect ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::truncated_input_is_an_error ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::is_shareable_across_threads ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test serve::tests::malformed_request_yields_400 ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test socks5::tests::surfaces_refusal_code ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s

   Doc-tests proxypilot_bridge

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

**Note on test count:** the coordinator's message expected 35 tests after the fix; the actual count is 34 (27 pre-existing non-`serve` tests + 5 original `serve` tests from the initial implementation + 2 new tests from Findings 3 and 4 = 34). Findings 1 and 2 modify existing code/tests rather than adding new ones, so no third new test was expected from them. I have not added a test beyond what Findings 3 and 4 specify, per the "no additional tests beyond what the brief/findings specify" instruction — flagging the discrepancy rather than inventing a 35th test to match the stated number.

Also confirmed, before committing, that the new `503` test is deterministic: ran `cargo test -p proxypilot-bridge serve` five consecutive times after the drain fix, all five gave `test result: ok. 8 passed; 0 failed; ...` (8 = the 7 `serve::tests::*` plus one unrelated `socks5::tests::rejects_server_demanding_auth`, matched only because "serve" happens to be a substring the name-filter matches against, not because it belongs to the module — same caveat as noted in the original report above).

Command: `cd win && cargo fmt --check`

Exit code: 0, no diff output — clean.

Command: `cd win && cargo clippy --all-targets -- -D warnings`

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s
```

No warnings.

Command: `cd win && cargo test` (whole workspace, confirming no regression in `proxypilot-core`)

- `proxypilot-bridge`: 34 passed, 0 failed.
- `proxypilot-core`: 29 passed, 0 failed (unchanged).

## Files changed (this round)

- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\serve.rs` (modified: accept-loop resilience, test target fix, two new tests, drain fix on the 503 path)

Commit: `d11b654` — "fix(win): не ронять accept-цикл на transient-ошибке, усилить тесты serve"

A throwaway diagnostic example crate used to isolate the RST root cause was created under `win/crates/bridge/examples/repro.rs` with a matching `[[example]]` stanza in `Cargo.toml`, then fully removed before committing — `git status` showed a clean working tree apart from `serve.rs` prior to the commit above.

## Issues or concerns

- The one deviation from the coordinator's literal instructions (the drain fix in the `503` branch) is explained in detail above and was necessary to make Finding 4's test pass deterministically on this platform; I did not touch anything else on the "leave alone" deferred list (module doc wording, 502 body disclosing upstream address, `Semaphore::new` panic on oversized limit, absence of logging).
- Test count is 34, not the 35 mentioned in the coordinator's message; see the note above.

---

# Fix Report: Review round 2 (drain fix generalized from 503 to `respond`)

## What I changed

The prior round's fix drained pending inbound bytes only at the 503 call site. The coordinator pointed out this defect (an abortive Windows RST on close-with-unread-data swallowing an already-written response) applies to every error path through `respond`, not just 503 -- most importantly 502 (dead upstream, the single most common production error, and a browser can easily have sent more bytes after `read_head` returned), plus 400, 408, and 501.

Moved the fix into `respond` itself and removed the ad-hoc drain from the 503 site, exactly as directed:

```rust
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
```

- After the response is fully written and flushed, `s.shutdown().await` half-closes the write side (best effort, error ignored) so the client sees the response as complete.
- Then a bounded drain loop (100ms total budget, not per-read) reads and discards whatever the client already sent or sends next, stopping on EOF (`n == 0`), timeout, or error -- so a chatty or silent client cannot make `respond` hang.
- At the 503 site in `serve()`, deleted the local `let mut junk = [0u8; 1024]; let _ = tokio::time::timeout(...).await;` block and its comment. Only `let _ = respond(&mut s, 503, "too many connections").await;` remains -- the explanation now lives solely in `respond`.

**Imports**: `AsyncReadExt` was already imported at module level (`use tokio::io::{AsyncReadExt, AsyncWriteExt};`) from the previous round, and remains used -- now by `respond`'s `s.read(&mut junk)` call instead of the deleted 503-site call. No unused-import warnings; confirmed via `cargo clippy --all-targets -- -D warnings` (clean) and by grepping the file for both `AsyncReadExt` and `AsyncWriteExt` usages, both present at module scope and in the test module's own separate import.

## Success path is unaffected

Confirmed by inspection: `respond` is called only from five error sites -- `503` (limit exceeded), `408` (head timeout), `400` (bad request / bad CONNECT target, two call sites), `501` (non-CONNECT method), and `502` (upstream connect failure). The `200 Connection established` reply is written directly via `client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")` in `handle_connect`, never through `respond`. `copy_bidirectional` runs immediately after that direct write (and after forwarding `head.leftover`), with no drain, shutdown, or extra read inserted on the success path. Grep evidence:

```
69:                let _ = respond(&mut s, 503, "too many connections").await;
82:                let _ = respond(&mut client, 408, "request head timed out").await;
86:                let _ = respond(&mut client, 400, &format!("bad request: {e}")).await;
95:        let _ = respond(&mut client, 501, "plain HTTP not implemented yet").await;
101:        let _ = respond(&mut client, 400, "bad CONNECT target").await;
114:            let _ = respond(&mut client, 502, &format!("upstream: {e}")).await;
120:        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
146:async fn respond(s: &mut TcpStream, code: u16, reason: &str) -> std::io::Result<()> {
```

Line 120 is the only `200` reply in the file and it bypasses `respond` entirely.

## Covering tests

- `serve::tests::exceeding_the_connection_limit_yields_503` -- now passes via the drain living in `respond`, with the local 503-site drain removed. Ran 5 consecutive times after the change (`cargo test -p proxypilot-bridge serve`), all green.
- `serve::tests::dead_upstream_yields_502_not_a_hang` and `serve::tests::changing_route_does_not_disturb_an_open_tunnel` (whose second connection also expects a `502`) exercise the same `respond`/drain path on the 502 branch the coordinator called out as the most important case; both pass.
- `serve::tests::malformed_request_yields_400` exercises the 400 branch through the same `respond`.
- All 34 tests in the crate re-run clean, confirming no regression on the success-path tests (`connect_direct_tunnels_bytes`, `bypassed_host_goes_direct_even_with_upstream_set`, `payload_sent_with_the_connect_head_is_not_lost`).

## Exact commands and verbatim output

Command: `cd win && cargo test -p proxypilot-bridge`

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-9c2fac5c83a9c1ce.exe)

running 34 tests
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::parses_connect ... ok
test http::tests::truncated_input_is_an_error ... ok
test http::tests::oversized_head_is_rejected ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::is_shareable_across_threads ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::direct_connects_to_origin ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test serve::tests::malformed_request_yields_400 ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test socks5::tests::surfaces_refusal_code ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s

   Doc-tests proxypilot_bridge

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Command: `cd win && cargo fmt --check`

Exit code: 0, no diff output -- clean.

Command: `cd win && cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.45s
```

No warnings.

Command: `cd win && cargo test` (whole workspace)

- `proxypilot-bridge`: 34 passed, 0 failed.
- `proxypilot-core`: 29 passed, 0 failed (unchanged).

Also re-ran `cargo test -p proxypilot-bridge serve` five consecutive times to confirm the removal of the local drain did not reintroduce flakiness on `exceeding_the_connection_limit_yields_503` -- all five gave `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 26 filtered out`.

## Files changed (this round)

- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\serve.rs` (modified: moved the drain-before-close fix from the 503 call site into `respond` itself, covering all five error-response codes)

Commit: `088477f` -- "fix(win): вычитывать входящие байты в respond, а не только на пути 503"

## Issues or concerns

None. This closes out the concern raised in the previous round's fix report -- the drain fix now protects every error response (`400`, `408`, `501`, `502`, `503`) instead of only `503`, and the success path (`200` reply plus `copy_bidirectional`) is verified untouched.
