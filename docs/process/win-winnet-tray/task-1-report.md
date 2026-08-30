# Task 1 report: Логи и диагностика

## What I implemented

1. **Dependencies.** Added `tracing`, `tracing-subscriber` (features `env-filter`, `fmt`), `tracing-appender` to `win/Cargo.toml`'s `[workspace.dependencies]` and to `win/crates/bridge/Cargo.toml`'s `[dependencies]` as workspace references. `win/Cargo.lock` was staged and committed alongside so the build is reproducible (the brief's `git add` line didn't mention it, but leaving a dirty lockfile after the commit seemed wrong given new deps were pulled in).

2. **`win/crates/bridge/src/log.rs`** (new file, registered as `pub mod log;` in `lib.rs`), exactly per the brief:
   - `LOG_FILE_PREFIX: &str = "proxypilot"`, `ENV_VAR: &str = "PROXYPILOT_LOG"`.
   - `filter_directive(env: Option<&str>) -> String` — env var wins if set and non-empty, else `"proxypilot=info"`.
   - `init(dir: Option<&Path>) -> Option<WorkerGuard>` — `None` → stderr subscriber (CLI/tests); `Some(dir)` → daily-rotating file appender via `tracing_appender::rolling::daily`, non-blocking writer, ANSI off (file output), returns the `WorkerGuard` the caller must keep alive.
   - Two unit tests as specified in the brief.

3. **Instrumentation** in `serve.rs` and `connector.rs` (details below). No control flow was changed anywhere — every existing `let _ = respond(...)` and every early `return` is untouched; log calls were inserted immediately before or after the existing branching, and in two spots an `Err(_)` was renamed to `Err(e)` purely to bind the value already being discarded, so it could be logged.

## Every log call site added, level, and why

### `serve.rs`

| Location | Level | Call | Why this level |
|---|---|---|---|
| Start of `serve()` | `info!(%addr, "мост слушает")` (guarded by `if let Ok(addr) = listener.local_addr()`) | `info` | One-time lifecycle event, not per-connection — exactly the kind of line `info` is for. |
| Accept-error arm, transient (`ConnectionAborted`/`ConnectionReset`/`Interrupted`) | `debug!(error = %e, consecutive_transient_errors, "приём: временная ошибка")` | `debug` | **Revised after review** (see Fix report below) — this is the routine, expected per-connection case; `warn` here would drown the arm below under a resource-exhaustion burst. |
| Accept-error arm, general (counts toward `MAX_CONSECUTIVE_ACCEPT_ERRORS`) | `warn!(error = %e, consecutive_errors, "приём: ошибка, {consecutive_errors} подряд")` | `warn` | This is the arm that can eventually kill the listener — the one an operator needs to see. |
| Before 503 (connection-limit exceeded) | `warn!(limit = shared.limits.max_connections, "предел соединений исчерпан")` | `warn` | Per brief, verbatim. Operator-actionable (raise the limit), not a per-request event in the sense that matters — it only fires once the pool is saturated. |
| 408 arm (head-read timeout) | `debug!(error = %e, "некорректный запрос клиента")` | `debug` | Per brief. Renamed the previously-unused `Err(_)` to `Err(e)` (the `Elapsed` value) purely to log it; behavior unchanged. |
| 400 arm (`Ok(Err(e))`, malformed request) | `debug!(error = %e, "некорректный запрос клиента")` | `debug` | Per brief. `e` was already bound and previously discarded via `let _ = respond(...)`. |
| `handle_connect`, `connect_via` success | `debug!(%host, port, ?route, "апстрим соединён")` | `debug` | **Judgment rule**: this is the success path for every CONNECT a browser makes — an `info` line here would turn the log into noise nobody reads. Kept at `debug` and placed symmetrically with the failure branch below so a session can be reconstructed by raising the level. |
| `handle_connect`, `connect_via` failure (before 502) | `warn!(%host, port, error = %e, "апстрим недоступен")` | `warn` | Per brief. `e` was already bound and previously discarded. |
| `handle_plain`, `Route::Http` branch, `dial_upstream_plain` success | `debug!(%host, port, ?route, "апстрим соединён")` | `debug` | Same successful-path rule as above; not explicitly itemized in the brief's bullet list but symmetric with the two other upstream-connect sites, and covered by the brief's general statement that successful connections must be `debug`, not silent-and-unlogged-at-any-level. |
| `handle_plain`, `Route::Http` branch, `dial_upstream_plain` failure (before 502) | `warn!(%host, port, error = %e, "апстрим недоступен")` | `warn` | Per brief. |
| `handle_plain`, fallback branch, `connect_via` success | `debug!(%host, port, ?route, "апстрим соединён")` | `debug` | Same as above. |
| `handle_plain`, fallback branch, `connect_via` failure (before 502) | `warn!(%host, port, error = %e, "апстрим недоступен")` | `warn` | Per brief. |

### `connector.rs`

| Location | Level | Call | Why this level |
|---|---|---|---|
| `connect_via`, on any `Err` (timeout or `connect_inner` failure) | `debug!(route = ?route, %host, port, error = %e, "не удалось соединиться")` | `debug` | Per brief and the anti-duplication rule: every caller of `connect_via` in `serve.rs` already logs the same failure at `warn` with more context (the 502 response reason). Logging it again at `warn` here would be the same failure logged twice at the same level. `debug` gives a way to see the raw connector-level detail (including the route) without duplicating the operator-facing warning. |

I deliberately did **not** touch the `let _ = respond(&mut client, 502, "upstream write failed")` site in `handle_plain` (mid-request write failure to an already-established upstream) — it has no captured error value (only `.is_err()`), so there's no "lost information" to surface, and the brief's `502` bullet's `error = %e` pattern requires an actual `e`. Adding a log there without an `e` would be inventing a location the brief didn't specify.

## What I tested and the results

- `cargo test -p proxypilot-bridge log` — RED then GREEN (see TDD Evidence).
- `cargo test --all` — 77 tests pass (was 75 + the 2 new `log` tests), 0 failed, output pristine (no stray log lines — no test calls `log::init`, so tracing macros are no-ops without a subscriber).
- `cargo clippy --all-targets -- -D warnings` — clean, no warnings.
- `cargo fmt --all --check` — clean.

## TDD Evidence

### RED

Command: `cd win && cargo test -p proxypilot-bridge log`

Verbatim output (compilation failure — module didn't exist yet):

```
error[E0425]: cannot find value `LOG_FILE_PREFIX` in this scope
  --> crates\bridge\src\log.rs:20:20
   |
20 |         assert_eq!(LOG_FILE_PREFIX, "proxypilot");
   |                    ^^^^^^^^^^^^^^^ not found in this scope

warning: unused import: `super::*`
 --> crates\bridge\src\log.rs:3:9
  |
3 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0425]: cannot find function `filter_directive` in this scope
 --> crates\bridge\src\log.rs:8:20
  |
8 |         assert_eq!(filter_directive(None), "proxypilot=info");
  |                    ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `filter_directive` in this scope
  --> crates\bridge\src\log.rs:11:20
   |
11 |         assert_eq!(filter_directive(Some("proxypilot=debug")), "proxypilot=debug");
   |                    ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `filter_directive` in this scope
  --> crates\bridge\src\log.rs:13:20
   |
13 |         assert_eq!(filter_directive(Some("")), "proxypilot=info");
   |                    ^^^^^^^^^^^^^^^^ not found in this scope

For more information about this error, try `rustc --explain E0425`.
warning: `proxypilot-bridge` (lib test) generated 1 warning
error: could not compile `proxypilot-bridge` (lib test) due to 4 previous errors; 1 warning emitted
```

(Exit code 101. This is the real compiler output from the actual failing state — the test file existed with only the `#[cfg(test)] mod tests` block, before `filter_directive`/`LOG_FILE_PREFIX`/`init` were written.)

### GREEN

Command: `cd win && cargo test -p proxypilot-bridge log`

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.47s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-95fdda0dcb22b514.exe)

running 2 tests
test log::tests::log_file_name_is_stable ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 44 filtered out; finished in 0.00s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-d0e24345ab75dac4.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-b1e0f180995ec65f.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

### Final full-suite run

`cargo test --all`: 46 tests in `proxypilot-bridge` lib (including the 2 new `log` tests), 2 in `cli.rs`, 29 in `proxypilot-core` — all pass, 0 failed. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all --check` both clean.

## Files changed

- `C:\Users\User\Desktop\proxypilot\repo\win\Cargo.toml` — added `tracing`/`tracing-subscriber`/`tracing-appender` to workspace deps.
- `C:\Users\User\Desktop\proxypilot\repo\win\Cargo.lock` — updated (new transitive deps).
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\Cargo.toml` — added the three deps as workspace references.
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\log.rs` — new file: `filter_directive`, `init`, `LOG_FILE_PREFIX`, `ENV_VAR`, two tests.
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\lib.rs` — added `pub mod log;`.
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\serve.rs` — instrumented per table above.
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\connector.rs` — instrumented `connect_via`.

Commit: `04cf076 feat(win): логи с ротацией и инструментирование горячего пути`.

## Self-review findings

- Confirmed no successful-path log is at `info` — all three "апстрим соединён" sites and the connector's own success path (which isn't logged at all, matching the brief) are `debug` or silent. Only the one-time listener-start line is `info`.
- Confirmed no failure is logged twice at the same level: `connect_via` logs its own failure at `debug`; every caller in `serve.rs` logs the same failure at `warn` — different levels, so this satisfies the anti-duplication rule rather than violating it.
- Confirmed no control flow changed: diffed every touched function against the original; the only non-logging change is `Err(_) => Err(e) =>` in the 408 timeout arm, which only binds a value that was already being discarded, changing nothing observable.
- Confirmed test output is pristine (see run above) — no tracing subscriber is installed anywhere in the test suite, so all `tracing::*!` macro invocations are no-ops during `cargo test`.
- One judgment call worth flagging explicitly (see table): I put the `warn!` accept-error log in *both* match arms (the three-error-kind "transient" arm and the general arm), since the brief's phrasing ("в ветке ошибки accept") doesn't disambiguate between them and both are genuine accept failures. If the intent was to log only the arm that counts toward `MAX_CONSECUTIVE_ACCEPT_ERRORS` (to avoid any chance of noise from `ConnectionAborted`/`ConnectionReset` bursts, which the surrounding comment describes as expected/routine), that's a one-line removal from the transient arm — I lean towards keeping both because `warn` under a burst is still far quieter than an `info` per successful request, and visibility into *why* the listener eventually gives up seemed valuable given this task's whole premise (a currently silent hot path).

## Issues or concerns

None blocking. The one item above is a documented judgment call, not a defect — happy to remove the transient-arm log if review disagrees with the call.

---

## Fix report (post-review)

Review verdict: "Needs fixes" — 1 Important + 2 folded-in items. All three addressed.

### FINDING 1 (Important) — transient accept-error arm logged at `warn`

**Problem:** `serve.rs`'s transient accept-error arm (`ConnectionAborted`/`ConnectionReset`/`Interrupted` — a client that vanished between SYN and accept, per the existing comment) was logged at `warn!`, same level as the counted arm that can end the listener. Under exactly the resource-exhaustion burst this code exists to tolerate, the transient arm can fire hundreds of times and drown the one line an operator actually needs. This is the same noise problem the success-path rule exists to prevent, and confirms my earlier judgment call (documented above) resolved the wrong way.

**Fix:** Dropped the transient arm to `debug!`. Kept `warn!` only on the general counted arm (the one that counts toward `MAX_CONSECUTIVE_ACCEPT_ERRORS` and can `return Err(e)`, ending the listener). Added a comment explaining why the level differs, directly referencing the noise mechanism the reviewer described.

### FINDING 2 (Minor, folded in) — shared message text and field name between the two arms

**Problem:** Both arms logged the literal message "ошибка приёма соединения" with a field named `consecutive`, for two different counters — impossible to tell apart in the log without cross-referencing thresholds.

**Fix:** Distinct messages and distinct field names:
- Transient arm: `debug!(error = %e, consecutive_transient_errors, "приём: временная ошибка")` — field name matches the variable (`consecutive_transient_errors`), a bare shorthand tying the name unambiguously to that counter.
- Counted arm: `warn!(error = %e, consecutive_errors, "приём: ошибка, {consecutive_errors} подряд")` — field name `consecutive_errors`, and the count is also interpolated straight into the human-readable message text via Rust's captured-identifier format-string syntax, so the number is visible without inspecting structured fields.

### FINDING 3 (reviewer's own, same file) — `log::init` panics on a second subscriber install

**Problem:** `tracing_subscriber::fmt()....init()` panics if a global subscriber is already set. Nothing calls `log::init` yet, but Task 9 (application wiring) and Task 10 (CLI) will both call it, and a double call would crash the process at startup instead of degrading gracefully.

**Fix:** Switched both branches (`None` / `Some(dir)`) from `.init()` to `.try_init()`, discarding the `Result` with `let _ =`. Added:
- A doc comment on `init` explaining the non-panicking guarantee.
- An inline comment at the `None` branch explaining why the error is deliberately dropped: if a subscriber is already installed, logging still works through it, and there's no channel to report the failure to (that channel would have been the very subscriber that failed to install).

The `Some(dir)` branch still returns `Some(guard)` even if `try_init` silently loses the race — the returned `WorkerGuard` remains valid to hold (it only governs the non-blocking writer's background flush thread), and if our own file-writing subscriber didn't win the race, whatever subscriber did win is still logging somewhere, matching the "degrade, don't panic" intent.

### What I did NOT change

Per instruction, left untouched: the three `debug!` calls on the upstream-connect success sites (`handle_connect` and both branches of `handle_plain`), and the `debug!` in `connector.rs`'s `connect_via` — reviewer confirmed both are correct as-is.

### Verification

Command: `cd win && cargo test --all`

Verbatim output (full run):

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.27s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-95fdda0dcb22b514.exe)

running 46 tests
test http::tests::rejects_bad_port ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::parses_connect ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::truncated_input_is_an_error ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok
test log::tests::log_file_name_is_stable ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::is_shareable_across_threads ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::direct_connects_to_origin ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test socks5::tests::surfaces_refusal_code ... ok
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-d0e24345ab75dac4.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-b1e0f180995ec65f.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-6d8e89ae7fb487cd.exe)

running 29 tests
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test config::tests::defaults_match_the_spec ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test bypass::tests::exact_hostname_matches ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::upstream_format_is_validated ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok

test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_bridge

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

77/77 tests pass (unchanged count — this was a level/message/panic-safety fix, not new behavior needing new tests).

Command: `cd win && cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.76s
```

Clean — no warnings.

Command: `cd win && cargo fmt --all --check`

No output — clean.

### Files changed (this fix pass)

- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\serve.rs` — transient accept-error arm to `debug!` with its own message/field; counted arm's message/field made distinct.
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\log.rs` — `init` now uses `try_init()` on both branches, doc comment updated, inline comment added explaining the dropped error.

Commit: `181bb54 fix(win): понизить уровень рутинной accept-ошибки, различить сообщения, try_init для подписчика`
