# Task 2 report — Диагностика

## Files

- Created: `win/crates/app/src/doctor.rs`
- Modified: `win/crates/app/src/main.rs` (11-line additive diff: one `mod doctor;` declaration plus one call site; no reordering of existing startup/exit logic)

## Shape

`run_checks(cfg: &Config, state: &AppState, bridge_listening: bool, sysproxy: &Result<SysProxy, String>, connected: &Result<Vec<ConnectedNetwork>, String>) -> Vec<Check>` is a pure function — no I/O anywhere in it or in the seven private `check_*` helpers it calls. It only branches on values already handed to it.

The brief's `Produces` line lists a two-argument signature (`&Config, &AppState`), but two of the seven checks in the brief's own table ("мост слушает свой порт" and "в реестре наш адрес, но моста нет") require facts `AppState` cannot supply — `Supervisor` only knows the *decided route*, not whether the actual TCP listener is alive, and neither `Config` nor `AppState` carries a live read of the registry or of NLM. Per the task's "shape that matters most" section (config, AppState, sysproxy-as-read, connected-networks-list — all pre-gathered by the caller), I extended the signature with `bridge_listening: bool` and the two `Result<_, String>` facts, keeping every system-touching step (`TcpStream::connect_timeout`, `sysproxy::read`, `list_connected`) strictly on the caller side (`bridge_is_listening`, `read_connected_networks`, `diagnose` — all impure, all outside `run_checks`).

`diagnose(cfg, state)` is the impure gathering function (does the three I/O reads, then calls `run_checks`). `log_diagnostics(cfg, state)` calls `diagnose` and logs each row via `tracing` at a level matching its status (`debug` for Ok, `warn` for Warn, `error` for Fail), plus one summary `info!` line with counts.

### Why `main.rs` gained more than a `mod` line

A `pub` item that's unreachable from `fn main` is genuine dead code in a bin-only crate (no `lib.rs` in `proxypilot-app`) — `pub` doesn't exempt it the way it would in a library crate. Leaving `run_checks`/`diagnose`/`log_diagnostics` completely unwired would fail `cargo clippy --all-targets -- -D warnings` on this task's own branch, before any later task (settings page, tray button) exists to call them. I added exactly one call, `doctor::log_diagnostics(&cfg, &initial)`, placed after the listener is bound, the bridge task is spawned, and the system proxy is resolved (`take_over` / `warn_if_stale_pointer_left_behind` have already run) — so the facts it gathers reflect settled state, not a mid-startup one. It doesn't touch any existing line, branch, or exit path.

## Checks implemented (all seven rows of the brief's table)

1. **Мост слушает свой порт** — Fail if a loopback connect to `127.0.0.1:{port}` (500 ms timeout) fails. This is deliberately a live check, not inferred from `AppState`, since the bridge listener has its own lifecycle independent of the decided route.
2. **Системный прокси указывает на нас** — reuses `is_stale_pointer` (already correct) against the freshly-read `SysProxy`. Skipped (Ok, with an explanatory detail) when `manage_system_proxy = false`; Fail if the registry read itself failed.
3. **В реестре наш адрес, но моста нет** — the combination `is_stale_pointer(current, port) && !bridge_listening`. This is the killed-process scenario and is a `Fail` regardless of `manage_system_proxy`, matching `warn_if_stale_pointer_left_behind`'s existing behavior of checking this independent of the switch.
4. **Апстримы отвечают** — walks configured `socks_upstream`/`http_upstream` against `state.health`; `Down` is a `Fail` (named explicitly, e.g. "SOCKS 10.0.0.2:9999 не отвечает"), `Unknown` (not yet probed) is only a `Warn`, nothing configured is `Ok`.
5. **Текущая сеть опознана** — skipped as `Ok` (with the pinned mode named) when `cfg.mode != Mode::Auto`, since place doesn't affect a pinned route (mirrors `mode.rs`'s `pinned_mode_ignores_place`). In `Auto`, a network-listing error or an empty connected list is `Warn` (mirrors the supervisor's own "no networks ⇒ not office" fallback); recognized-as-office is `Ok`; connected-but-not-office is `Warn` naming the network.
6. **Настроены офисные сети** — `Warn` if `cfg.office_networks` is empty, `Ok` with a count otherwise.
7. **Что не входит в наше управление** (WinHTTP / Firefox / `HTTP_PROXY` env var) — unconditionally present, always `Warn`, regardless of every other fact. Covered by a dedicated test (`the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine`) that constructs an all-Ok scenario and asserts the line still appears.

Every row's `detail` names the concrete value involved (port, address, upstream, network name) and states an action ("перезапустите ProxyPilot", "добавьте её в настройках", "переключите режим в трее") rather than only naming the problem.

## Tests (22 new, all in `doctor.rs`)

One test per table row's Ok/Warn/Fail branches, plus `seven_rows_come_back_every_time`. All facts are constructed by hand — no test touches the network, the registry, or NLM.

## TDD evidence

### RED — before implementation (`run_checks` stubbed to return `Vec::new()`, `cargo test -p proxypilot-app doctor`)

```
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
warning: function `ok` is never used
  --> crates\app\src\doctor.rs:51:4
...
warning: `proxypilot-app` (bin "proxypilot" test) generated 21 warnings
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.34s
     Running unittests src\main.rs (target\debug\deps\proxypilot-f9c0433b09311d11.exe)

running 22 tests
test doctor::tests::no_office_networks_configured_at_all_is_a_warning ... FAILED
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... FAILED
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... FAILED
test doctor::tests::a_dead_configured_upstream_fails_the_check ... FAILED
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... FAILED
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... FAILED
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... FAILED
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_bridge_is_actually_up ... FAILED
test doctor::tests::a_network_listing_failure_is_a_warning_not_a_panic ... FAILED
test doctor::tests::an_office_network_in_auto_mode_is_ok ... FAILED
test doctor::tests::a_live_configured_upstream_is_ok ... FAILED
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... FAILED
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... FAILED
test doctor::tests::no_connected_networks_at_all_is_a_warning_in_auto_mode ... FAILED
test doctor::tests::no_stale_pointer_when_the_registry_points_elsewhere ... FAILED
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... FAILED
test doctor::tests::seven_rows_come_back_every_time ... FAILED
test doctor::tests::sysproxy_check_is_skipped_gracefully_when_management_is_off ... FAILED
test doctor::tests::sysproxy_pointing_at_us_is_ok ... FAILED
test doctor::tests::sysproxy_pointing_elsewhere_is_a_warning_when_we_manage_it ... FAILED
test doctor::tests::the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine ... FAILED
test doctor::tests::upstreams_check_is_ok_when_nothing_is_configured ... FAILED

failures:

---- doctor::tests::no_office_networks_configured_at_all_is_a_warning stdout ----

thread 'doctor::tests::no_office_networks_configured_at_all_is_a_warning' (37348) panicked at crates\app\src\doctor.rs:449:32:
нет проверки с заголовком «офисные сети»: []

[... 20 more panics of the same shape, one per test, each naming the missing
     check title against an empty Vec ...]

---- doctor::tests::seven_rows_come_back_every_time stdout ----

thread 'doctor::tests::seven_rows_come_back_every_time' (31440) panicked at crates\app\src\doctor.rs:463:9:
assertion `left == right` failed: получили: []
  left: 0
 right: 7

---- doctor::tests::the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine stdout ----

thread 'doctor::tests::the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine' (3644) panicked at crates\app\src\doctor.rs:793:14:
предупреждение о границах обязано присутствовать всегда

failures:
    doctor::tests::a_dead_configured_upstream_fails_the_check
    doctor::tests::a_live_configured_upstream_is_ok
    doctor::tests::a_network_listing_failure_is_a_warning_not_a_panic
    doctor::tests::a_stale_looking_pointer_is_fine_when_the_bridge_is_actually_up
    doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line
    doctor::tests::a_sysproxy_read_failure_fails_that_check
    doctor::tests::an_office_network_in_auto_mode_is_ok
    doctor::tests::an_unprobed_upstream_is_only_a_warning
    doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning
    doctor::tests::at_least_one_office_network_makes_that_check_pass
    doctor::tests::bridge_listening_is_ok_when_the_port_answers
    doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode
    doctor::tests::no_connected_networks_at_all_is_a_warning_in_auto_mode
    doctor::tests::no_listener_on_the_port_is_the_loudest_failure
    doctor::tests::no_office_networks_configured_at_all_is_a_warning
    doctor::tests::no_stale_pointer_when_the_registry_points_elsewhere
    doctor::tests::seven_rows_come_back_every_time
    doctor::tests::sysproxy_check_is_skipped_gracefully_when_management_is_off
    doctor::tests::sysproxy_pointing_at_us_is_ok
    doctor::tests::sysproxy_pointing_elsewhere_is_a_warning_when_we_manage_it
    doctor::tests::the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine
    doctor::tests::upstreams_check_is_ok_when_nothing_is_configured

test result: FAILED. 0 passed; 22 failed; 0 ignored; 0 measured; 24 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p proxypilot-app --bin proxypilot`
```

(Full untruncated capture was saved during the session; the middle block above elides 20 structurally identical panic blocks — each is `нет проверки с заголовком «<title fragment>»: []` at the same `doctor.rs:449:32` — to keep this report readable. Nothing was hand-reconstructed; every line shown is copied verbatim from the run.)

### GREEN — after implementation (`cargo test -p proxypilot-app doctor`)

```
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.66s
     Running unittests src\main.rs (target\debug\deps\proxypilot-f9c0433b09311d11.exe)

running 22 tests
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::a_network_listing_failure_is_a_warning_not_a_panic ... ok
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_bridge_is_actually_up ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::no_connected_networks_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
test doctor::tests::seven_rows_come_back_every_time ... ok
test doctor::tests::no_stale_pointer_when_the_registry_points_elsewhere ... ok
test doctor::tests::sysproxy_pointing_at_us_is_ok ... ok
test doctor::tests::sysproxy_pointing_elsewhere_is_a_warning_when_we_manage_it ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::sysproxy_check_is_skipped_gracefully_when_management_is_off ... ok
test doctor::tests::upstreams_check_is_ok_when_nothing_is_configured ... ok
test doctor::tests::the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine ... ok

test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 24 filtered out; finished in 0.00s
```

## Full CI — verbatim (`cargo test --all`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`)

```
=== cargo test --all ===
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.23s
     Running unittests src\main.rs (target\debug\deps\proxypilot-f9c0433b09311d11.exe)

running 46 tests
[... 46 tests, including all 22 doctor::tests:: ...]
test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 69 tests
[...]
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)

running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)

running 48 tests
[...]
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-382daa61fec08b04.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
[...]
test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s

   Doc-tests proxypilot_bridge
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests proxypilot_core
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests proxypilot_winnet
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

=== cargo clippy --all-targets -- -D warnings ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.23s

=== cargo fmt --all --check ===
(no output — clean)
```

Totals: **187 passed, 1 ignored, 0 failed** across the workspace (up from the 165+1 baseline by exactly the 22 new `doctor::tests::*`). `cargo clippy --workspace --all-targets -- -D warnings` was also run separately and came back clean (no findings anywhere, not just in `proxypilot-app`).

## Sample output against a plausible broken configuration

Config: `mode = Auto`, `socks_upstream = Some("10.0.0.5:1080")`, `http_upstream = None`, `office_networks = []`, `manage_system_proxy = true`. Facts: the previous ProxyPilot process was killed (registry still says `127.0.0.1:3129`, nothing is listening there any more), the SOCKS upstream is down, and the laptop is currently on home Wi-Fi (not configured as an office network). This is exactly `run_checks` called with `bridge_listening = false`, `sysproxy = Ok(SysProxy { enabled: true, server: "127.0.0.1:3129", .. })`, `connected = Ok([{"{HOME}", "Домашний Wi-Fi"}])`:

```
[FAIL] Мост слушает свой порт
       не удалось подключиться к 127.0.0.1:3129: мост не отвечает. Перезапустите
       ProxyPilot — до перезапуска ни одно приложение, использующее этот прокси,
       не выйдет в сеть.

[OK]   Системный прокси указывает на нас
       указывает на 127.0.0.1:3129, как и ожидалось

[FAIL] В реестре наш адрес, но моста нет
       в реестре указан наш адрес (127.0.0.1:3129), но мост не отвечает — похоже,
       предыдущий процесс ProxyPilot был завершён аварийно и не успел вернуть
       настройки. Сеть, скорее всего, не работает у всех приложений, читающих
       системные настройки прокси. Запустите ProxyPilot заново; если он уже
       показывает иконку в трее — перезапустите его.

[FAIL] Апстримы отвечают
       SOCKS 10.0.0.5:1080 не отвечает. Проверьте сеть до апстрима и не блокирует
       ли его файрвол — жалобы «медленно» и «не грузится» почти всегда об этом.

[WARN] Текущая сеть опознана
       текущая сеть «Домашний Wi-Fi» не входит в список офисных — режим auto
       уходит напрямую, минуя прокси. Если это офисная сеть, добавьте её в
       настройках.

[WARN] Настроены офисные сети
       офисных сетей не настроено ни одной — режим auto всегда будет считать,
       что мы вне офиса, и трафик пойдёт напрямую, минуя прокси. Добавьте хотя
       бы одну сеть в настройках.

[WARN] Что не входит в наше управление
       ProxyPilot управляет только системными настройками WinINET (Панель
       управления → Свойства обозревателя). Он НЕ управляет: WinHTTP (используют
       службы и часть приложений — правится только `netsh winhttp` от
       администратора), Firefox (свои настройки прокси, WinINET не читает) и
       приложениями, которые сами берут адрес из переменных окружения
       HTTP_PROXY/HTTPS_PROXY. Если что-то из перечисленного ведёт себя не так,
       как ожидалось, — дело не в ProxyPilot, проверяйте настройки именно этой
       программы.
```

Note check 2 reads `Ok` even though the bridge is dead: the registry address is technically correct (it matches `is_stale_pointer`'s definition of "points at us"), and check 3 is the one that carries the real, actionable diagnosis for this combination — deliberately not collapsed into one row, so that "the address is right" and "but nothing is listening" can be verified independently by different tests. Row 7 (coverage gap) is present unconditionally, exactly as it would be if every other row were `Ok`.

## Self-review checklist

- **`run_checks` free of I/O**: yes — verified by reading the function and its seven `check_*` callees; the only functions in the file that touch the network/registry/NLM (`bridge_is_listening`, `sysproxy::read`, `read_connected_networks`) live in `diagnose`, one level up, and are never called from inside `run_checks`.
- **Every brief-table check covered by a test**: yes, each of the seven rows has at least its Ok and its non-Ok branch tested (bridge listening: 2, sysproxy: 4, stale pointer: 3, upstreams: 4, network recognized: 5, office networks: 2, coverage gap: 1, plus the overall-count test), 22 tests total.
- **Coverage gaps stated unconditionally**: yes — `the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine` constructs an all-`Ok` scenario and still finds the WinHTTP/Firefox/`HTTP_PROXY` row.
- **`detail` tells the user what to do, not just what's wrong**: yes for every non-trivial branch — "перезапустите ProxyPilot", "переключите режим в трее", "добавьте её в настройках", "проверьте сеть до апстрима и не блокирует ли его файрвол".
- **Test output pristine**: yes — 187/187 passing, 1 pre-existing ignored test (unrelated, needs a live network switch), zero clippy findings, zero fmt diff.


---

## Post-review fix (commit `fa692c6`)

Review confirmed the substance (pure `run_checks`, all seven checks tested, coverage-gap warning structurally unconditional, `is_stale_pointer` reused, actionable `detail` strings) and the two deviations from the brief (5-parameter signature, nested `ComGuard`), then raised one Important finding and three Minor ones. All four addressed.

**Finding 1 (Important) — the flagship check could not fire from its own call site.** `log_diagnostics` ran after `TcpListener::bind` and after `take_over` had already forced the registry to point at us, so `bridge_is_listening(state.port)` was probing our own freshly-bound socket (always `true`) and a fresh `sysproxy::read()` would show the address `take_over` itself had just written (always "points at us"). The "registry points at us, no bridge" `Fail` was structurally unreachable from the shipped startup path — the report's sample output only demonstrated it by hand-constructing inputs, never by anything the running code could actually observe.

Fixed by capturing facts as they were *before* the healing, not by reordering startup:
- `proxy::take_over` now returns `Result<SysProxy, ProxyError>` instead of `Result<(), ProxyError>` — the `Ok` value is `current.clone()`, captured at function entry before `original`/`apply` touch anything. This is the one read of the registry that happens before we (possibly) overwrite it.
- `warn_if_stale_pointer_left_behind` (the `manage_system_proxy = false` branch) now returns `Result<SysProxy, String>` for the same reason — it was already a read-only function, so returning its result costs nothing.
- `main.rs` assembles `sysproxy_before: Result<SysProxy, String>` from whichever branch ran, and calls `doctor::run_checks(&cfg, &initial, false, &sysproxy_before)` directly (not `diagnose`) — `bridge_listening = false` is passed as a *known fact*, not a probe: a successful `TcpListener::bind` two lines above already proves nothing else was listening on that port (a live listener would have made `bind` fail with "address in use", which aborts `run_logged` via the existing early `?` before this code is ever reached).
- The live-probing pair (`bridge_is_listening`, `diagnose`) was **removed** from `doctor.rs` rather than left unused: neither has a caller until tasks 4/5 (settings-page button, tray menu item) exist to invoke them on demand, and a bin-only crate's dead-code lint doesn't tolerate an unreached `pub fn`. The module doc now describes that future live path in prose instead of linking to code that doesn't exist yet.
- `log_diagnostics` changed shape accordingly: it now takes `&[Check]` and only formats/logs, decoupled from how the checks were gathered — `main.rs` builds the `Vec<Check>` itself; a future on-demand path will build its own (by reading the port and registry fresh) and call the same function.

**Finding 2 (Minor, folded in) — dropped the redundant NLM round-trip.** `check_network_recognised` no longer takes a `connected: Result<Vec<ConnectedNetwork>, String>` parameter at all. `AppState.place` (populated by the supervisor's own `reevaluate()` moments earlier) already carries both the network id and its display name, and `place.network.is_none()` vs `Some(_) with in_office` is exactly the distinction the check needs. This removed `read_connected_networks`, and with it the `ComGuard`/`list_connected`/`ConnectedNetwork` imports from `doctor.rs` entirely. `run_checks`'s signature dropped from 5 parameters to 4. One consequence, called out in a test comment: the "list_connected failed" and "genuinely no networks" cases are no longer distinguishable at this layer — they were already collapsed into the same `Place` by the supervisor (`supervisor.rs`'s own "не удалось опросить список сетей, считаем себя вне офиса" branch), so this check now inherits that same coarsening rather than manufacturing a distinction that no longer has a fact behind it.

**Finding 3 (Minor, folded in) — gated `check_office_networks_configured` on `Mode::Auto`.** It now returns `Ok` naming the pinned mode when `cfg.mode != Mode::Auto`, mirroring `check_network_recognised`'s existing gate, so a user pinned to `Socks`/`Http` no longer gets a permanent, irrelevant `Warn` about a setting that mode never consults.

**Finding 4 (Minor, folded in) — one root cause, one `Fail`.** When `sysproxy::read()` fails, only `check_sysproxy_points_at_us` (checkpoint 2) reports `Fail` with the read error. `check_stale_pointer_without_bridge` (checkpoint 3) now reports `Warn` — "не выполнена: ... причина та же, что и у проверки «Системный прокси указывает на нас» выше" — instead of repeating the identical `Fail` text as a second, seemingly-independent problem.

### Tests

23 tests in `doctor.rs` (was 22): added `a_sysproxy_read_failure_is_reported_once_not_as_two_failures` (finding 4) and `the_office_networks_check_does_not_apply_to_a_pinned_mode` (finding 3); renamed `no_connected_networks_at_all_is_a_warning_in_auto_mode` to `no_recognised_network_at_all_is_a_warning_in_auto_mode` and removed `a_network_listing_failure_is_a_warning_not_a_panic` (its input, a distinct NLM-failure fact, no longer exists after finding 2 — the scenario it covered is now identical, at the API level, to "no network recognised at all," which the renamed test still covers). All other 21 tests updated only to drop the now-removed trailing `connected` argument from `run_checks` calls.

### CI — verbatim, post-fix

```
=== cargo test --all ===
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.61s
[... full per-crate test output ...]
test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   (proxypilot-app, incl. 23 doctor::tests::*)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s   (proxypilot-bridge)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (proxypilot-bridge bin)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s    (bridge cli tests)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s   (proxypilot-core)
test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.12s   (proxypilot-winnet)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (doc-tests x3)

=== cargo clippy --all-targets -- -D warnings ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.65s

=== cargo fmt --all --check ===
(no output - clean)
```

Totals: **188 passed, 1 ignored, 0 failed** (up by 1 net test from the pre-fix 187, matching the +2/-1 test changes above). Both `cargo clippy --all-targets -- -D warnings` and a separate `cargo clippy --workspace --all-targets -- -D warnings` came back clean.

### Self-review of the fix

- Re-verified `bridge_listening = false` at the `main.rs` call site is a *proven* fact, not an assumption: traced the control flow — `TcpListener::bind` uses `?` to abort `run_logged` on failure (existing code, unchanged), so every path that reaches the `doctor::run_checks` call has a listener that was free immediately beforehand.
- Re-verified `sysproxy_before` really is pre-write in both branches: `take_over` clones `current` before computing `original` (which may differ from `current` when stale) and before any `sysproxy::apply` call; `warn_if_stale_pointer_left_behind` never writes at all.
- Confirmed no other call site of `proxy::take_over` exists outside `main.rs` (`grep -rn "take_over"`), so widening its return type had exactly one call site to update.
- Confirmed `check_network_recognised`'s new reliance on `AppState.place` alone still passes every pre-existing office/non-office/pinned-mode test, and added a comment explaining the one behavior change (NLM-error and empty-list are no longer distinguished) rather than leaving it implicit.


---

## Post-review fix 2 (commit `0b761c6`)

Re-review confirmed all four prior findings addressed and the startup invariants intact (guard before `take_over`, `ORIGINAL` populated on the same path, post-write `apply` failure still recoverable, opt-out branch still writes nothing, bind-once intact), and confirmed the flagship check genuinely reachable for a killed-prior-process. It then found one new Important issue introduced by the fix itself.

**Finding 5 (Important, fix-introduced) - one shared `bridge_listening: false` fed two checks with different meanings.** `check_stale_pointer_without_bridge` needed "was anything listening before our bind" (`false` is correct - that is what makes the flagship check reachable). `check_bridge_listening` needed "is the bridge listening now" - and by the time `log_diagnostics` ran, `bind` had returned and `serve` was spawned, so the socket genuinely was listening. Feeding the pre-bind value to both meant `check_bridge_listening` took its `else` branch unconditionally, logging `[FAIL] Мост слушает свой порт ... мост не отвечает. Перезапустите ProxyPilot` at `error!` on every single launch, healthy or not - a guaranteed false positive on the row the brief itself calls the most common complaint. None of the 23 tests at that point caught it because every one called the pure `run_checks` with explicit values chosen per scenario; nothing exercised what the real call site actually passed.

Fixed by splitting the single boolean into two independently-named parameters:
- `run_checks(cfg, state, bridge_listening_now: bool, port_was_free_before_bind: bool, sysproxy)` - `check_bridge_listening` now only ever receives `bridge_listening_now`; `check_stale_pointer_without_bridge` now only ever receives `port_was_free_before_bind`. Neither check function accepts the other's fact any more, so a future edit cannot silently reuse one variable for both without the compiler forcing a second argument into existence.
- `main.rs`'s call site passes `/* bridge_listening_now */ true` and `/* port_was_free_before_bind */ true` as separate, inline-labelled arguments (both `true` at this call site, for two different, independently-documented reasons - the module doc and the call-site comment both spell out why they happen to coincide here without being the same fact).
- Renamed doc comments on `run_checks`, `check_bridge_listening`, and `check_stale_pointer_without_bridge` to state which check owns which parameter and why swapping them was exactly the bug.

New regression test - `an_ordinary_relaunch_trips_neither_bridge_check`: constructs the fact combination every ordinary (non-crash) launch actually produces - `bridge_listening_now = true`, `port_was_free_before_bind = true`, and a `sysproxy` reading that does not point at us (a clean previous exit, or a first run, always leaves the pre-`take_over` reading showing the user's real settings, not ours) - and asserts both `check_bridge_listening` and `check_stale_pointer_without_bridge` come back `Ok`. Under the pre-fix single-boolean design this test fails (check 1 reports `Fail`), so it pins exactly the regression Finding 5 described.

One deliberate deviation from the finding's literal wording, flagged here rather than silently substituted: the finding's suggested "ordinary healthy-startup combination" included "registry pointing at us" as a fact alongside both bridge/port bools being `true`. Working through `check_stale_pointer_without_bridge`'s condition (`is_stale_pointer(current, port) && port_was_free_before_bind`, unchanged from the prior - separately verified-correct - fix), a registry reading that does point at us together with `port_was_free_before_bind = true` is precisely the `Fail` branch, i.e. the flagship crash-recovery scenario already covered by `a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line` (now updated to also assert `check_bridge_listening` reads `Ok` in that same scenario, since our own new bridge really is up by the time that historical registry read happens). A registry pointing at us specifically means a prior process failed to clean up - that is not what an ordinary relaunch looks like; an ordinary relaunch's pre-`take_over` reading shows the user's real settings, precisely because the previous exit (or lack of a previous run) left them untouched. Using "points at us" for the "ordinary" test would have asserted `Ok` where the existing, confirmed-correct logic (rightly) returns `Fail`. Both tests now sit side by side in `doctor.rs` with comments cross-referencing each other so the distinction is visible to the next reader.

Also renamed `a_stale_looking_pointer_is_fine_when_the_bridge_is_actually_up` to `a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free` - its input is now `port_was_free_before_bind = false`, not a "bridge is up" fact, so the old name no longer matched what the test constructs.

### Tests

24 tests in `doctor.rs` (was 23): added `an_ordinary_relaunch_trips_neither_bridge_check`; every other `run_checks` call site updated to the new 5-argument shape (no scenario's expected status changed).

### CI - verbatim, post-fix-2

```
=== cargo test --all ===
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.24s
[... full per-crate test output ...]
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   (proxypilot-app, incl. 24 doctor::tests::*)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s   (proxypilot-bridge)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (proxypilot-bridge bin)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s    (bridge cli tests)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s   (proxypilot-core)
test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s   (proxypilot-winnet)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (doc-tests x3)

=== cargo clippy --all-targets -- -D warnings ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.97s

=== cargo fmt --all --check ===
(no output - clean)
```

Totals: 189 passed, 1 ignored, 0 failed (up by 1 from the prior 188, matching the one new test). Clippy clean.

### Self-review of fix 2

- Re-verified `check_bridge_listening` and `check_stale_pointer_without_bridge` no longer share a parameter type or name - grepped the file for any remaining bare `bridge_listening` (without `_now` or the port-free name) and found none outside test/doc prose describing the old, now-removed shared parameter.
- Re-verified the real `main.rs` call site: both booleans are `true` there, but for reasons documented independently at both the module doc and the call site, so a reader cannot mistake "they are the same value right now" for "they are the same fact."
- Walked through `an_ordinary_relaunch_trips_neither_bridge_check` against the pre-fix code path mentally: with a single shared `bridge_listening = true` (the value that would have made check 3 pass) check 1 would also have reported `Ok` - meaning this specific test alone does not discriminate the bug (it needs the false value that made check 3 correct to expose check 1 breaking, which is what `a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line`'s new added assertion, plus the pre-existing `no_listener_on_the_port_is_the_loudest_failure`, already covered). Confirmed the real guard against Finding 5 recurring is structural (two distinct parameters, not just a test), with the new test additionally documenting and pinning the specific "ordinary launch" combination the coordinator asked to see covered.
