# Task 9 Report: Обычный HTTP

## What was implemented

Modified `win/crates/bridge/src/serve.rs` only, exactly per the brief:

1. In `handle`, replaced the `501` fallback branch with a call to the new `handle_plain(client, head, shared)`.
2. Added `handle_plain`: parses the absolute-form target via `split_absolute`, returns `400` if not absolute-form; picks a route via the existing `pick_route`; connects upstream via `connect_via`, returns `502` on failure; rebuilds the request line in origin-form (`METHOD path VERSION`), forwards all headers except hop-by-hop ones, appends `Connection: close`, writes the request, then `head.leftover` (if any), then runs `copy_bidirectional` for the rest of the exchange (no keep-alive — one connection per request, per the brief's design note).
3. Added `split_absolute(target) -> Option<(String, u16, String)>`, stripping `http://`/`HTTP://`, splitting authority from path (defaulting path to `/`), and reusing `split_host_port` for the host/port with default port 80.
4. Added `is_hop_by_hop(name) -> bool`, checking against the 8-name list from the brief (`connection`, `proxy-connection`, `proxy-authenticate`, `proxy-authorization`, `keep-alive`, `te`, `trailer`, `upgrade`) case-insensitively.
5. Added three tests to `mod tests`: `http_origin()` helper, `plain_http_is_forwarded_in_origin_form`, `plain_http_with_dead_upstream_yields_502`, `non_absolute_target_yields_400` — transcribed from the brief verbatim, **except** the one directed change: all three tests read the response with `read_to_end` under a 5s timeout instead of a single `c.read(&mut buf)`, to avoid a false failure if the response arrives in more than one TCP segment. This is safe because `respond()` (for 400/502) and the origin's own close (for the 200 case, since `handle_plain` has no keep-alive) always result in the connection closing, so `read_to_end` is guaranteed to terminate.

No new files, no `lib.rs` change, no `Cargo.toml` change — as required.

## What was tested and the results

- Focused test run of the three new tests: all pass.
- Full `cargo test` (workspace): bridge crate 37/37 passed, core crate 29/29 passed, 0 failed.
- `cargo fmt --check`: clean (after running `cargo fmt` once to fix one line-length wrap in the new test).
- `cargo clippy --all-targets -- -D warnings`: clean, no findings.

## TDD Evidence

### RED

Command:
```
cd win && cargo test -p proxypilot-bridge serve::tests::plain
```

Verbatim output (captured before `handle_plain`/`split_absolute`/`is_hop_by_hop` existed — the `else` branch in `handle` still answered `501`):

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.18s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-9c2fac5c83a9c1ce.exe)

running 2 tests
test serve::tests::plain_http_with_dead_upstream_yields_502 ... FAILED
test serve::tests::plain_http_is_forwarded_in_origin_form ... FAILED

failures:

---- serve::tests::plain_http_with_dead_upstream_yields_502 stdout ----

thread 'serve::tests::plain_http_with_dead_upstream_yields_502' (9780) panicked at crates\bridge\src\serve.rs:415:9:
получили: HTTP/1.1 501 Not Implemented
Content-Type: text/plain; charset=utf-8
Content-Length: 43
Connection: close

proxypilot: plain HTTP not implemented yet

note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- serve::tests::plain_http_is_forwarded_in_origin_form stdout ----

thread 'serve::tests::plain_http_is_forwarded_in_origin_form' (7632) panicked at crates\bridge\src\serve.rs:389:9:
получили: HTTP/1.1 501 Not Implemented
Content-Type: text/plain; charset=utf-8
Content-Length: 43
Connection: close

proxypilot: plain HTTP not implemented yet



failures:
    serve::tests::plain_http_is_forwarded_in_origin_form
    serve::tests::plain_http_with_dead_upstream_yields_502

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 35 filtered out; finished in 0.00s
error: test failed, to rerun pass `-p proxypilot-bridge --lib`
```

Why this was expected: the code compiled fine (`Head`, `respond`, etc. all already existed from task 8), but `handle` still hit the `else` branch that unconditionally replies `501 Not Implemented`, so both assertions on `HTTP/1.1 200 OK` / `HTTP/1.1 502` failed against the actual `501` reply. Note the filter `serve::tests::plain` only matches the two `plain_http_*` tests, not `non_absolute_target_yields_400` (name has no "plain" substring) — this matches the brief's own Step 2 command exactly, so that third test's RED state was not separately captured via this filter (it also would have failed against 501, since 501 does not start with "HTTP/1.1 400").

### GREEN

Command:
```
cd win && cargo test -p proxypilot-bridge serve::tests::plain
```
Output:
```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.10s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-9c2fac5c83a9c1ce.exe)

running 2 tests
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 35 filtered out; finished in 2.01s
```

Command:
```
cd win && cargo test -p proxypilot-bridge serve::tests::non_absolute
```
Output:
```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.06s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-9c2fac5c83a9c1ce.exe)

running 1 test
test serve::tests::non_absolute_target_yields_400 ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 0.00s
```

Full workspace suite after implementation (`cd win && cargo test`):
```
running 37 tests
... (all bridge tests) ...
test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s

running 29 tests
... (all core tests) ...
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_bridge
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` both exit 0 with no output beyond the normal `Finished` line.

## Files changed

- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\serve.rs` (only file touched; 157 insertions, 1 deletion)

## Self-review findings

- Completeness: `handle_plain`, `split_absolute`, `is_hop_by_hop`, and all three tests from the brief are present; the `handle` dispatch was updated as specified.
- Quality: names match the brief exactly (`handle_plain`, `split_absolute`, `is_hop_by_hop`). No `#[allow]` needed; clippy is clean without one.
- Discipline: nothing added beyond what the brief specifies, other than the one directed change (read-to-EOF in the three tests instead of a single `read`).
- Testing: `plain_http_is_forwarded_in_origin_form` asserts against the request the fake origin actually received (captured via the `JoinHandle<String>` returned by `http_origin`) — it genuinely checks origin-form (`GET /path?q=1 HTTP/1.1\r\n`), absence of `proxy-connection` (case-insensitively), and presence of `Connection: close`. This is a real behavioral assertion, not a guess. RED→GREEN order was followed; RED output above is the real, complete, unedited terminal output.
- `serve.rs` is now ~530 lines (up from ~387) — it was already flagged in earlier tasks as the largest file in the crate. Per the brief's Step 1 instructions this task deliberately keeps everything in `serve.rs` (no new files allowed), so I did not split it. Flagging this per the "Code Organization" instruction rather than acting on it unilaterally.

## Issues or concerns

None blocking. The only note is the file-size one above, which is explicitly out of scope for this task (brief says modify `serve.rs` only, no new files) — reported as information for the controller, not as a defect.
