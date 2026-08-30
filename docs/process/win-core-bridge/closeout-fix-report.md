# Closeout fix report — bridge, 2026-08-30

Branch `feat/windows-rust`, base `756d11b`. Three fixes to
`win/crates/bridge`.

## FIX 1 — plain HTTP through an HTTP upstream must not use CONNECT

### Failing state captured first (TDD)

Added the test `plain_http_through_http_upstream_uses_absolute_form_not_connect`
to `win/crates/bridge/src/serve.rs` (with its `fake_http_proxy_plain` helper)
*before* touching `handle_plain`, then ran it against the unmodified code:

```
cargo test --all -p proxypilot-bridge plain_http_through_http_upstream_uses_absolute_form_not_connect -- --nocapture
```

Verbatim failure:

```
thread 'serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect' (21576) panicked at crates\bridge\src\serve.rs:739:13:
assertion `left == right` failed: апстрим-http должен получать absolute-form запрос без CONNECT, получил: CONNECT 127.0.0.1:3174 HTTP/1.1
  left: "CONNECT 127.0.0.1:3174 HTTP/1.1"
 right: "GET http://127.0.0.1:3174/ HTTP/1.1"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

thread 'serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect' (21576) panicked at crates\bridge\src\serve.rs:777:9:
assertion `left == right` failed
  left: [72, 84, 84, 80]
 right: [112, 111, 110, 103]
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... FAILED
```

This shows the two symptoms described in the ticket in one run: the fake
upstream saw a `CONNECT` line instead of an absolute-form `GET`, and the
client ended up reading the start of a `502` body (`HTTP`) instead of
`pong` — the request never reached `origin`.

### Fix

- `win/crates/bridge/src/connector.rs`: added
  `pub async fn dial_upstream_plain(addr: &str, dial: Duration) -> Result<Upstream, ConnectError>`.
  It reuses the existing private `dial_upstream` helper for the connect +
  error mapping, wraps it in the same `tokio::time::timeout`, and returns
  `Upstream { stream, pending: Vec::new() }` — no CONNECT handshake, no read
  of a proxy response, because with an `http` upstream on the *plain* path
  we speak first and the proxy can't have sent anything yet.
- `win/crates/bridge/src/serve.rs`, `handle_plain`: the route snapshot is
  still taken once before dialling (`pick_route`), but now branches on it
  before connecting:
  - `Route::Http(addr)` → `dial_upstream_plain(addr, shared.limits.dial)`,
    request line built as `"{method} {target} {version}"` from
    `head.target` unchanged (the client's original absolute-form URL).
  - every other route (`Direct`, `Socks`) → unchanged: `connect_via` to the
    origin, request line rewritten to origin-form (`path`).
  Both branches keep the identical hop-by-hop header filter, the
  `Connection: close` tail, the `head.leftover` write to the upstream, and
  the `upstream.pending` write to the client (always empty on the new
  branch, so unchanged).
- Comment added above the branch explaining *why*, in Russian, matching the
  file's style: names the CONNECT-policy problem (Squid's stock
  `acl SSL_ports port 443` / `http_access deny CONNECT !SSL_ports` across
  its whole 2.x-6.x line, plus commercial gateways defaulting the same way)
  rather than just describing the mechanic.
- FIX 3 (`set_nodelay`) was folded into this same edit of `handle_plain`
  since it touches the same lines immediately before `copy_bidirectional`
  (see FIX 3 below).

### Test-harness pitfall found and corrected

The first version of the test awaited the fake proxy's `JoinHandle` after
reading `pong`, expecting the panic (pre-fix) or clean return (post-fix) to
surface directly. Post-fix, this **deadlocked**: `tokio::io::copy_bidirectional`
only returns once *both* directions have reached EOF, and the test's own
client socket is never closed while the test awaits the proxy task — so the
"client → proxy → origin" direction stays open forever even though `origin`
already closed the other direction after sending `pong`. This was visible
as `cargo test` printing `... has been running for over 60 seconds` and the
whole binary run stretching from ~2s to ~120s (confirmed by comparing two
full `cargo test --all` runs, before and after removing the `.await` on the
join handle). Fixed by making `fake_http_proxy_plain` fire-and-forget, the
same style already used by the sibling helpers `fake_http_relay` and
`fake_socks5_relay` in the same test module — the outer assertion on `pong`
is sufficient to catch a regression, since a wrong first line makes the fake
proxy panic before it ever dials `origin`.

## FIX 2 — accept loop could spin on a core forever

`win/crates/bridge/src/serve.rs`, `serve()`: the
`ConnectionAborted | ConnectionReset | Interrupted` arm now tracks its own
`consecutive_transient_errors` counter, separate from the real
`consecutive_errors` budget used to eventually return an error. Once that
run passes `TRANSIENT_ERROR_SLEEP_THRESHOLD = 64`, the arm sleeps the same
50 ms as the other error arm before `continue`-ing, so a persistent run of
these kinds stops being a hot spin. It still never counts toward
`MAX_CONSECUTIVE_ACCEPT_ERRORS` and never returns — per-connection errors
must not end the listener. Both counters reset to 0 on any successful
`accept()`.

## FIX 3 — missing `set_nodelay` on the `handle_plain` pump

`win/crates/bridge/src/serve.rs`, `handle_plain`: added the same two lines
`handle_connect` already has, immediately before its
`copy_bidirectional`, errors ignored:

```rust
let _ = client.set_nodelay(true);
let _ = upstream_stream.set_nodelay(true);
```

## Verification

### `cargo test --all` (win/)

```
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-9c2fac5c83a9c1ce.exe)

running 44 tests
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::parses_connect ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::oversized_head_is_rejected ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test http::tests::truncated_input_is_an_error ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::is_shareable_across_threads ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::direct_connects_to_origin ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test socks5::tests::surfaces_refusal_code ... ok
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-c3a64ea26c1c605f.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-ab4a9a8014464208.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-6d8e89ae7fb487cd.exe)

running 29 tests
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test config::tests::defaults_match_the_spec ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::direct_mode_is_direct ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_mode_ignores_place ... ok
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

Total: 75 tests passing (44 + 2 + 29), up from 74 (one new test added).

### `cargo clippy --all-targets -- -D warnings` (win/)

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s
```

Clean, no warnings, no `#[allow]` added.

### `cargo fmt --all --check` (win/)

```
(no output, exit code 0)
```

## Design-property checks

- `Router::get()` still has exactly one non-test call site
  (`serve.rs::pick_route`), confirmed with
  `grep -rn "router.get()" crates/bridge/src/`.
- The route snapshot in `handle_plain` is still taken once, before dialling
  either branch, and nothing on the data path re-consults the router
  afterwards.
- No silent fallback to `direct`: both the `Route::Http` and the
  `Direct`/`Socks` branches in `handle_plain` return `502` on a failed
  upstream connect.

## Files touched

- `win/crates/bridge/src/connector.rs`
- `win/crates/bridge/src/serve.rs`
