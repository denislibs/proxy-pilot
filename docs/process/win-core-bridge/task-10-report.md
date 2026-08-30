# Task 10 Report: Бинарь и ручная проверка

## What was implemented

- `win/crates/bridge/tests/cli.rs` (new): two black-box tests that spawn the
  `proxypilot-bridge` binary via `CARGO_BIN_EXE_proxypilot-bridge` — one
  checks that a malformed `--socks` value is rejected with a message
  containing `host:port`, the other checks that `--help` succeeds and lists
  `--port` and `--socks`.
- `win/crates/bridge/Cargo.toml` (modified): added a `[[bin]]` section
  (`name = "proxypilot-bridge"`, `path = "src/main.rs"`).
- `win/crates/bridge/src/main.rs` (new): the CLI entry point exactly as
  specified in the brief — hand-rolled argument parsing for `--port`,
  `--socks`, `--http`, `--mode`, `--no-proxy`, `--help`/`-h`; validates
  upstream addresses via `validate_upstream`; builds `Health`/`Place`/`Decision`
  via `decide` (treating any configured upstream as reachable, with the
  brief's comment explaining that real network detection arrives in a later
  plan); assembles `Shared { router, bypass, limits }`; binds strictly to
  `127.0.0.1:<port>` (never `0.0.0.0`); and calls `serve`.

One deliberate deviation from the brief's verbatim code: the brief's
`let mut next = || { ... };` triggers rustc's `unused_mut` lint (the closure
only reads captured variables), which `cargo clippy --all-targets -- -D
warnings` promotes to a hard error. Per the task instructions ("Fix clippy
findings properly; do not silence them with `#[allow]`"), I changed it to
`let next = || { ... };`. No other line was altered from the brief's code.

## What was tested and the results

- `cargo test -p proxypilot-bridge --test cli` — RED before the binary
  target existed (see TDD Evidence below), then GREEN afterward.
- `cargo test -p proxypilot-bridge` — 37 unit tests + 2 CLI tests, all pass.
- `cargo test` (whole workspace) — bridge: 37 + 2 = 39 pass; core: 29 pass.
  All 68 tests green.
- `cargo fmt --check` — clean (exit 0) after running `cargo fmt` once (it
  only reformatted the `decide(...)` call onto multiple lines and added the
  new `[[bin]]` block's blank-line spacing already matched).
- `cargo clippy --all-targets -- -D warnings` — clean, no warnings, no
  `#[allow]` added anywhere.
- Manual verification (Step 5 of the brief) — see below.
- Manually re-verified (beyond the automated tests) that `--socks bad-value`
  exits 1 with a `host:port`-naming message, and that `--help` exits 0 and
  prints every flag.

## TDD Evidence

### RED

Command:
```
cd win && cargo test -p proxypilot-bridge --test cli
```

Verbatim, complete output (before `src/main.rs` / the `[[bin]]` section
existed — at this point only `tests/cli.rs` had been written):

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
error: environment variable `CARGO_BIN_EXE_proxypilot-bridge` not defined at compile time
 --> crates\bridge\tests\cli.rs:4:5
  |
4 |     env!("CARGO_BIN_EXE_proxypilot-bridge")
  |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = help: `CARGO_BIN_EXE_proxypilot-bridge` may not be available for the current Cargo target
  = help: Cargo sets build script variables at run time. Use `std::env::var("CARGO_BIN_EXE_proxypilot-bridge")` instead

error: could not compile `proxypilot-bridge` (test "cli") due to 1 previous error
```

Why this failure is expected: `CARGO_BIN_EXE_proxypilot-bridge` is only
defined at compile time when the package actually declares a binary target
named `proxypilot-bridge`. Before Step 3 (adding the `[[bin]]` section and
`src/main.rs`), no such target existed, so the `env!()` macro used by the
test helper `bin()` fails to compile — this is the brief's expected "FAIL —
бинарной цели нет" (no binary target).

### GREEN

Command:
```
cd win && cargo test -p proxypilot-bridge
```

Verbatim output:

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.01s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-9c2fac5c83a9c1ce.exe)

running 37 tests
test http::tests::rejects_bad_port ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::parses_connect ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::truncated_input_is_an_error ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::is_shareable_across_threads ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test serve::tests::malformed_request_yields_400 ... ok
test connector::tests::direct_connects_to_origin ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test socks5::tests::surfaces_refusal_code ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.07s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-c3a64ea26c1c605f.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-ab4a9a8014464208.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

   Doc-tests proxypilot_bridge

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Full workspace run (`cargo test`) additionally confirms core crate's 29
tests: `test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0
filtered out`.

## Manual verification (Step 5)

Bridge started in one shell:
```
cd win && cargo run -p proxypilot-bridge -- --mode direct
```
(built and ran the release-less debug binary directly:
`./target/debug/proxypilot-bridge.exe --mode direct`)

Output:
```
мост слушает http://127.0.0.1:3129, маршрут: Direct
```

From another shell, HTTPS via CONNECT:
```
curl -x http://127.0.0.1:3129 -sS -o /dev/null -w '%{http_code}\n' https://api.anthropic.com/v1/messages
```
Output:
```
405
```
This matches the brief's expectation (`401` or `405` — the request reached
the real service; `000` would mean the bridge did not work).

Plain HTTP:
```
curl -x http://127.0.0.1:3129 -sS -o /dev/null -w '%{http_code}\n' http://example.com/
```
Output:
```
200
```
Matches the brief's expectation exactly.

Additional manual checks (not required by Step 5 but covered by the
self-review checklist):
```
./target/debug/proxypilot-bridge.exe --socks bad-value
```
Output (exit code 1):
```
proxypilot-bridge: --socks: нужен формат host:port, получено «bad-value»
```

```
./target/debug/proxypilot-bridge.exe --help
```
Output (exit code 0), lists all five flags (`--port`, `--socks`, `--http`,
`--mode`, `--no-proxy`) plus `--help` itself.

The bridge process was terminated (`taskkill /F /IM proxypilot-bridge.exe`)
after verification.

This environment does have outbound internet access, so the above `curl`
output is real, live traffic through the bridge to `api.anthropic.com` and
`example.com` — not fabricated.

## Files changed

- `win/crates/bridge/Cargo.toml` — added `[[bin]]` section.
- `win/crates/bridge/src/main.rs` — new, CLI entry point (per brief, with the
  one `unused_mut` fix described above).
- `win/crates/bridge/tests/cli.rs` — new, black-box CLI tests (verbatim from
  brief).

## Self-review findings

- Compared `main.rs` line-by-line against the brief's code block: identical
  except for the removed `mut` on `next`, which was necessary to pass
  `cargo clippy --all-targets -- -D warnings` (the closure only reads `i`,
  `args`, and `flag`; nothing inside it is mutated). This is a mechanical,
  behavior-preserving fix, not a redesign.
- `cargo fmt` reformatted the `decide(cfg.mode, &cfg.upstreams(), Place { in_office: true }, health)`
  call across four lines (line-length driven); no semantic change.
- Confirmed the listener binds `127.0.0.1` (never `0.0.0.0`) — verified both
  by reading the code and by observing the printed `слушает
  http://127.0.0.1:3129` at runtime.
- Confirmed `--help` exits 0 and enumerates every flag the binary accepts
  (`--port`, `--socks`, `--http`, `--mode`, `--no-proxy`, `--help`).
- Confirmed a bad `--socks` value exits non-zero with a message naming the
  `host:port` format.
- No extra flags, types, or tests were added beyond what the brief
  specifies (YAGNI check passed).
- Final test count matches the plan exactly: bridge 37 unit + 2 CLI = 39;
  core 29.

## Issues or concerns

None. The only departure from the brief's literal text (`mut` removal) was
required to keep clippy's `-D warnings` gate green and does not change
behavior; everything else is a faithful transcription.
