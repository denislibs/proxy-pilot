# Task 4 Report: Место в конфиге и `Place::network`

## What I implemented

- `win/crates/core/src/mode.rs`: `Place` gained a `network: Option<String>` field
  (doc comment in Russian explaining it's needed for the UI's "сейчас: Офис" and
  for logs to explain a routing decision). `Place` lost `Copy` (kept `Debug, Clone,
  PartialEq, Eq`, dropping `Copy`) since `Option<String>` isn't `Copy`.
- `win/crates/core/src/config.rs`:
  - New `OfficeNetwork { id: String, name: String }` struct (`Serialize`/`Deserialize`,
    `name` defaults on missing field).
  - New `Config::office_networks: Vec<OfficeNetwork>` field, `#[serde(default)]`,
    initialized to `Vec::new()` in `Default`.
  - New `Config::place_for(&self, connected_ids: &[String]) -> Place`: finds the
    first connected id that matches (case-insensitively, via `eq_ignore_ascii_case`)
    any configured office network. If found, `Place { in_office: true, network:
    Some(id) }`. Otherwise `Place { in_office: false, network: connected_ids.first().cloned() }`
    — an empty `office_networks` list can never match, so it always resolves to
    "not office", never the reverse.
  - Appended one new block to `Config::validate` (existing blocks untouched):
    an `OfficeNetwork` with an empty `id` is now a config error, because it could
    never match any network and would otherwise fail silently and undiagnosably.
- `win/crates/bridge/src/main.rs`: updated the one existing `Place { in_office: true }`
  construction to include `network: None` (this stage still doesn't know the real
  network; that arrives with the supervisor task).

## Every site the loss of `Copy` forced me to touch

Searched the whole repo for `Place` usage — only two files reference it:
`win/crates/core/src/mode.rs` and `win/crates/bridge/src/main.rs`. There is no
site anywhere that stores a `Place` value in a variable and then uses it twice
(which is the pattern that would actually break from losing `Copy` — implicit
re-copy on second use). All existing constructions are one-shot struct literals
passed straight into `decide(...)`, so nothing broke from losing `Copy` itself.
What *did* need touching, because the struct gained a mandatory new field
(orthogonal to the `Copy` question — Rust requires every field in a struct
literal regardless of `Copy`), was every `Place { in_office: ... }` literal:

- `win/crates/core/src/mode.rs`: 10 test-only literals across the `decide()`
  test module — each got `network: None` appended. No `.clone()` was needed
  anywhere; these are all inline literals consumed once.
- `win/crates/bridge/src/main.rs`: the single non-test `Place` construction in
  `run()` — same treatment, `network: None` (network detection isn't wired up
  yet in this task).

No call site needed restructuring or cloning to route around the lost `Copy`;
the fix was purely "add the new field to each literal."

## Config::validate

Appended a new loop after the existing `socks_upstream`/`http_upstream` format
check, leaving every earlier block untouched:

```rust
for o in &self.office_networks {
    // Пустой id никогда ни с чем не совпадёт — запись мертва, но
    // молча: пользователь не поймёт, почему офисная сеть не признаётся.
    if o.id.is_empty() {
        return Err(ConfigError::Invalid(format!(
            "office_networks: у сети «{}» пустой id",
            o.name
        )));
    }
}
```

## Deviation from the brief (and why)

The brief's `office_cfg()` test helper used:

```rust
let mut c = Config::default();
c.office_networks = vec![ ... ];
c
```

This trips `clippy::field_reassign_with_default` under `-D warnings`
(`field assignment outside of initializer for an instance created with
Default::default()`). Per the task's explicit instruction to fix clippy
findings properly and never `#[allow]`, I rewrote it using clippy's own
suggested pattern:

```rust
fn office_cfg() -> Config {
    Config {
        office_networks: vec![
            OfficeNetwork { id: "...".into(), name: "Офис".into() },
            OfficeNetwork { id: "...".into(), name: "Офис-2".into() },
        ],
        ..Default::default()
    }
}
```

Behavior and test assertions are identical to the brief; only the
construction idiom changed.

## What I tested and the results

Full workspace test suite: 99 tests pass (46 in `proxypilot-bridge` lib +
2 in its `cli` integration test + 44 in `proxypilot-core` + 7 in
`proxypilot-winnet`), 0 failures. All three CI commands pass (see verbatim
output below).

## TDD Evidence

### RED — before implementation existed

Command: `cd win && cargo test -p proxypilot-core place`

```
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
error[E0422]: cannot find struct, variant or union type `OfficeNetwork` in this scope
   --> crates\core\src\config.rs:346:13
    |
346 |             OfficeNetwork {
    |             ^^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `OfficeNetwork` in this scope
   --> crates\core\src\config.rs:350:13
    |
350 |             OfficeNetwork {
    |             ^^^^^^^^^^^^^ not found in this scope

error[E0609]: no field `office_networks` on type `config::Config`
   --> crates\core\src\config.rs:345:11
    |
345 |         c.office_networks = vec![
    |           ^^^^^^^^^^^^^^^ unknown field
    |
    = note: available fields are: `bridge_port`, `mode`, `socks_upstream`, `http_upstream`, `no_proxy` ... and 3 others

error[E0599]: no method named `place_for` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:360:30
    |
 18 | pub struct Config {
    | ----------------- method `place_for` not found for this struct
...
360 |         let p = office_cfg().place_for(&["{AAAA0000-0000-0000-0000-000000000002}".to_string()]);
    |                              ^^^^^^^^^ method not found in `config::Config`

error[E0599]: no method named `place_for` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:370:30
    |
 18 | pub struct Config {
    | ----------------- method `place_for` not found for this struct
...
370 |         let p = office_cfg().place_for(&["{BBBB0000-0000-0000-0000-000000000000}".to_string()]);
    |                              ^^^^^^^^^ method not found in `config::Config`

error[E0599]: no method named `place_for` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:382:30
    |
 18 | pub struct Config {
    | ----------------- method `place_for` not found for this struct
...
382 |         let p = office_cfg().place_for(&["{aaaa0000-0000-0000-0000-000000000001}".to_string()]);
    |                              ^^^^^^^^^ method not found in `config::Config`

error[E0599]: no method named `place_for` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:388:30
    |
 18 | pub struct Config {
    | ----------------- method `place_for` not found for this struct
...
388 |         let p = office_cfg().place_for(&[]);
    |                              ^^^^^^^^^ method not found in `config::Config`

error[E0599]: no method named `place_for` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:397:35
    |
 18 | pub struct Config {
    | ----------------- method `place_for` not found for this struct
...
397 |         let p = Config::default().place_for(&["{AAAA0000-0000-0000-0000-000000000001}".to_string()]);
    |                                   ^^^^^^^^^ method not found in `config::Config`

error[E0599]: no method named `place_for` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:405:30
    |
 18 | pub struct Config {
    | ----------------- method `place_for` not found for this struct
...
405 |         let p = office_cfg().place_for(&[
    |                 -------------^^^^^^^^^ method not found in `config::Config`

Some errors have detailed explanations: E0422, E0599, E0609.
For more information about an error, try `rustc --explain E0422`.
error: could not compile `proxypilot-core` (lib test) due to 9 previous errors
```

Exit code: 101 (compile failure — the module existed, no need for the
empty-module workaround; the failure reaches real type errors directly).

### GREEN — after implementation

Command: `cd win && cargo test -p proxypilot-core place`

```
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.68s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-c0412214ae432639.exe)

running 3 tests
test mode::tests::pinned_mode_ignores_place ... ok
test config::tests::place_is_not_office_for_an_unknown_network ... ok
test config::tests::place_is_office_when_a_connected_network_matches ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 41 filtered out; finished in 0.00s
```

(Note: the substring filter `place` only matches 3 of the 6 new tests by name;
the full workspace run below shows all 6 new tests plus every pre-existing
test passing.)

## Verbatim output of the three CI commands

### `cargo fmt --all --check`

```
(no output — exit code 0)
```

### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.68s
```

(First attempt, before rewriting `office_cfg()`, failed with the
`field_reassign_with_default` finding shown above in "Deviation from the
brief". This is the output after the fix, confirming zero warnings.)

### `cargo test --all`

```
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.42s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-f9bfedea04baa417.exe)

running 46 tests
test http::tests::rejects_bad_port ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::parses_connect ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::truncated_input_is_an_error ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok
test log::tests::log_file_name_is_stable ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::is_shareable_across_threads ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... ok
test socks5::tests::surfaces_refusal_code ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-837393c89186d591.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-eb7488564f5ac25b.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-ce820a0b07ec9f56.exe)

running 44 tests
test bypass::tests::empty_list_matches_nothing ... ok
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test config::tests::defaults_match_the_spec ... ok
test config::tests::load_from_a_missing_file_yields_defaults ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test config::tests::several_connected_networks_office_wins ... ok
test config::tests::place_is_not_office_for_an_unknown_network ... ok
test config::tests::place_is_office_when_a_connected_network_matches ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::matching_is_case_insensitive ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::validate_rejects_a_port_below_the_privileged_range ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::no_network_at_all_is_not_office ... ok
test config::tests::validate_accepts_the_defaults ... ok
test config::tests::validate_rejects_a_malformed_upstream ... ok
test config::tests::validate_rejects_a_zero_connection_limit ... ok
test config::tests::validate_rejects_an_absurd_connection_limit ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test config::tests::without_configured_offices_nothing_is_office ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::direct_mode_is_direct ... ok
test config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test config::tests::save_then_load_roundtrips_through_a_real_file ... ok
test config::tests::config_path_matches_what_the_spec_promises ... ok

test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-1325b8c59218fb85.exe)

running 7 tests
test networks::tests::category_maps_every_documented_value ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

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

## Files changed

- `win/crates/core/src/mode.rs` — `Place` gains `network: Option<String>`, loses
  `Copy`; 10 test literals updated.
- `win/crates/core/src/config.rs` — `OfficeNetwork`, `Config::office_networks`,
  `Config::place_for`, new `validate` block, 6 new tests + `office_cfg()` helper.
- `win/crates/bridge/src/main.rs` — one `Place` literal updated with `network: None`.

## Self-review findings

- Confirmed via repo-wide grep that `Place` is referenced only in `mode.rs` and
  `bridge/src/main.rs` — no other ripple sites exist (no supervisor/tray code
  yet in this branch).
- Confirmed all seven other `Config { ... }` struct literals in `config.rs`
  tests use `..Default::default()`, so none needed touching for the new
  `office_networks` field.
- Confirmed `proxypilot-core` still has zero platform dependencies — the network
  identifier crosses into `core` only as `String`/`Vec<String>`, never touching
  `proxypilot-winnet` types.
- Confirmed `Router::get()` has no new non-test call site (untouched by this task).
- One deviation from the brief's literal code was required (clippy
  `field_reassign_with_default` on the `office_cfg()` helper) — documented above,
  fixed via clippy's own suggested idiom rather than `#[allow]`.

## Issues or concerns

None. All three CI commands pass cleanly; the diff is scoped exactly to what
the brief and the ripple instructions called for.

---

## Follow-up: review findings fix

Two findings from the coordinator's review of the first pass, addressed in
commit `a02ede4`.

### FINDING 1 — missing test for the empty-`id` validation branch

Added `validate_rejects_an_office_network_with_empty_id` to `config.rs`,
following the existing `validate_rejects_a_malformed_upstream` pattern.

**RED** — captured by temporarily replacing the validation block (the `for
(i, o) in self.office_networks...` loop in `Config::validate`) with a no-op
comment, then running the new test alone:

Command: `cd win && cargo test -p proxypilot-core validate_rejects_an_office_network_with_empty_id`

```
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.82s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-c0412214ae432639.exe)

running 1 test
test config::tests::validate_rejects_an_office_network_with_empty_id ... FAILED

failures:

---- config::tests::validate_rejects_an_office_network_with_empty_id stdout ----

thread 'config::tests::validate_rejects_an_office_network_with_empty_id' (17288) panicked at crates\core\src\config.rs:351:9:
assertion failed: matches!(c.validate(), Err(ConfigError::Invalid(_)))
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    config::tests::validate_rejects_an_office_network_with_empty_id

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 44 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p proxypilot-core --lib`
```

The validation block was then restored verbatim from a pre-edit backup copy
(diffed to confirm byte-for-byte identity with the pre-disable version).

**GREEN** — same command, block restored (note: the first attempt showed a
stale pass/fail due to a Windows `mv`-preserved-mtime issue tricking cargo's
build-freshness check into skipping recompilation; `touch`ing the file forced
a real rebuild, shown below by the `Compiling` line reappearing):

```
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.95s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-c0412214ae432639.exe)

running 1 test
test config::tests::validate_rejects_an_office_network_with_empty_id ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 44 filtered out; finished in 0.00s
```

### FINDING 2 — error message named the entry by `name`, which can itself be empty

Changed the error format in `Config::validate` from:

```rust
"office_networks: у сети «{}» пустой id", o.name
```

to always report the index, and append the name only when non-empty:

```rust
let name_suffix = if o.name.is_empty() {
    String::new()
} else {
    format!(" «{}»", o.name)
};
ConfigError::Invalid(format!("office_networks[{i}]{name_suffix}: пустой id"))
```

So `office_networks[2] «Офис»: пустой id` when a name is present, and
`office_networks[2]: пустой id` when it is not — never the useless
`office_networks: у сети «» пустой id`.

### CI trio after the fix (verbatim)

`cargo fmt --all --check`:
```
(no output — exit code 0)
```

`cargo clippy --all-targets -- -D warnings`:
```
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.19s
```

`cargo test --all` (summary lines; full per-test listing identical to the
first pass plus the one new test):
```
running 46 tests ... test result: ok. 46 passed; 0 failed  (proxypilot-bridge lib)
running 0 tests  ... test result: ok. 0 passed; 0 failed   (proxypilot-bridge bin)
running 2 tests  ... test result: ok. 2 passed; 0 failed   (cli.rs)
running 45 tests ... test result: ok. 45 passed; 0 failed  (proxypilot-core lib, includes
                     config::tests::validate_rejects_an_office_network_with_empty_id ... ok)
running 7 tests  ... test result: ok. 7 passed; 0 failed   (proxypilot-winnet lib)
```
Total: 100 passed, 0 failed across the workspace.

### Files changed (this follow-up)

- `win/crates/core/src/config.rs` — new test, reworded validation error
  message (index + conditional name). No other file touched, per the
  coordinator's instruction not to touch `place_for`, the `Copy` fixes, or
  the other validation blocks.

### Commit

`a02ede4` — "fix(win): тест и точный текст ошибки для office_networks[i]"
