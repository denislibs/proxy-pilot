# Task 1 Report: Каркас воркспейса и выбор маршрута

## Summary

Successfully implemented the workspace framework and pure route-selection logic for the ProxyPilot Windows port. The task created the cargo workspace structure, defined all required types (Mode, Route, Reachability, Upstreams, Health, Place, Decision), and implemented the core `decide()` function with complete test coverage.

## Implementation

### Files Created

1. **win/Cargo.toml** - Workspace manifest with members `["crates/core"]` only (bridge crate deferred to task 4 per decision note)
2. **win/crates/core/Cargo.toml** - Core crate manifest inheriting edition 2021 and rust-version 1.75 from workspace
3. **win/crates/core/src/lib.rs** - Library root exporting the mode module
4. **win/crates/core/src/mode.rs** - Complete route selection logic with all types and the `decide()` function
5. **win/.gitignore** - Ignore cargo target directory

### Types Defined

- `Mode` (enum): User preference - Socks, Http, Direct, Auto
- `Route` (enum): Selected outbound route with addresses for Socks/Http or Direct
- `Reachability` (enum): Health check status - Up, Down, Unknown
- `Upstreams` (struct): Optional addresses for SOCKS5 and HTTP proxies
- `Health` (struct): Reachability status for both upstreams
- `Place` (struct): Location indicator - in_office boolean
- `Decision` (struct): Route decision with demoted flag for degraded service indication

### Core Logic: `decide()` Function

The function implements the routing policy:

- **Direct mode**: Always returns direct route, no demotion
- **Auto mode**: 
  - Outside office: Always direct (avoids roundtrip through office)
  - Inside office: Prefers SOCKS5 if available, falls back to HTTP, then direct
  - Only uses upstreams with Reachability::Up status
- **Pinned modes (Socks/Http)**:
  - Returns specified upstream if available and Up
  - Falls back to direct with demoted=true if unavailable
  - Ignores location (user preference overrides Place)

### Helper Function: `usable()`

Private function that returns the address only if both configured AND reachable (Up status). Rejects Unknown status as not yet verified.

## Test-Driven Development Evidence

### RED Phase (Step 3)
**Command:** `cd win && cargo test -p proxypilot-core`

**Output (excerpt):**
```
error[E0425]: cannot find type `Upstreams` in this scope
error[E0425]: cannot find function `decide` in this scope
error[E0433]: cannot find type `Route` in this scope
... (67 total errors)
```

**Expected:** Tests fail because types and `decide()` function are not yet defined. The module structure exists but is empty (only tests).

### GREEN Phase (Step 5)
**Command:** `cd win && cargo test -p proxypilot-core`

**Output:**
```
running 10 tests
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::pinned_mode_ignores_place ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured
```

**Verification:** All 10 tests pass. Implementation correctly handles all 10 test cases covering the complete routing policy.

## Quality Checks

### Formatting
**Command:** `cargo fmt`
Output: Successfully formatted mode.rs (struct literal formatting adjusted by rustfmt)

### Linting
**Command:** `cargo clippy --all-targets -- -D warnings`
Output: No warnings or errors. All clippy checks pass.

### Final Tests
**Command:** `cargo test -p proxypilot-core` (full suite)
Output: 10 tests passed, pristine output

## Commit

**SHA:** `497e2fb`
**Message:** `feat(win): каркас воркспейса и выбор маршрута`
**Files:** 5 files created, 290 insertions

## Self-Review Findings

### Completeness
- ✓ All files from brief created in correct locations
- ✓ All types defined exactly as specified
- ✓ `decide()` function signature matches brief exactly
- ✓ All 10 tests from brief transcribed correctly
- ✓ Test helpers (ups(), health()) included
- ✓ Implementation matches brief code exactly
- ✓ Documentation comments preserved in Russian

### Discipline
- ✓ No additional types or fields beyond brief specification
- ✓ No unnecessary derive attributes or features
- ✓ Workspace configuration exactly as specified (only crates/core in members)
- ✓ Rust edition 2021, rust-version 1.75 as required
- ✓ No #[allow] attributes used; clippy passes cleanly

### Code Quality
- ✓ Type naming matches brief (Mode, Route, Reachability, etc.)
- ✓ Public API surface correct (all types and decide function are pub)
- ✓ Helper function usable() is private as intended
- ✓ Test organization: nested module with test helpers
- ✓ Properly formatted by cargo fmt
- ✓ All dependency versions locked from workspace

### Testing
- ✓ Tests follow TDD red-green order as instructed
- ✓ Each test verifies one aspect of routing policy
- ✓ Edge cases covered: unconfigured upstreams, Unknown status, demotion
- ✓ Test output is pristine (no warnings, no noise)

## Issues and Concerns

None. The task is complete per specification.

### Notes

- The workspace member list correctly omits `crates/bridge` per the decision note. Task 4 will add it later.
- Windows line-ending warnings during commit are normal and not problematic; git will handle CRLF properly on Windows.
- All workspace dependency versions are pinned to allow future crates to inherit shared versions.
- The route selection logic is pure (no I/O, no state), suitable for atomic decision-making in the proxy bridge.
