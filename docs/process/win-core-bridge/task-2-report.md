# Task 2: Bypass-матчер — Implementation Report

## What Was Implemented

Implemented the `BypassList` data structure with full support for matching hostnames and IP addresses against a bypass list. The implementation supports:

1. **Exact hostname matching** — case-insensitive exact match
2. **Dot-suffix matching** — matches subdomains with dot prefix (e.g., `.local` matches `printer.local` but not `local`)
3. **IPv4 addresses** — exact IP address matching
4. **IPv6 addresses** — exact IP address matching with bracket stripping (for `[::1]` format from CONNECT)
5. **CIDR notation** — IPv4 CIDR blocks (e.g., `192.168.0.0/16`) with proper masking, including edge cases (`/0` and `/32`)
6. **Empty/blank entry handling** — gracefully ignores empty strings in the comma-separated list

## Files Created/Modified

- **Created:** `win/crates/core/src/bypass.rs`
  - 177 lines total
  - Includes module documentation explaining why the bypass matcher lives in the bridge
  - Private `Entry` enum with 4 variants for different match types
  - Public `BypassList` struct with `parse()` and `matches()` methods
  - 12 comprehensive tests

- **Modified:** `win/crates/core/src/lib.rs`
  - Added `pub mod bypass;` declaration alongside existing `pub mod mode;`

## TDD Evidence

### RED Phase: Failing Tests

**Command run:**
```bash
cd win && cargo test -p proxypilot-core bypass
```

**Initial state (test only, no implementation):**
```
error[E0425]: cannot find type `BypassList` in this scope
error[E0433]: cannot find type `BypassList` in this scope
...
warning: unused import: `super::*`
...
error: could not compile `proxypilot-core` (lib test) due to 6 previous errors; 1 warning emitted
```

The test module couldn't compile because `BypassList` was not defined anywhere. This was the expected failure state before implementation.

### GREEN Phase: All Tests Passing

**Command run after implementation:**
```bash
cd win && cargo test -p proxypilot-core
```

**Output:**
```
running 22 tests
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test bypass::tests::ip_literal_matches ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok

test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

All 22 tests pass: 12 bypass tests + 10 existing mode tests.

## Quality Checks Passed

### Formatting
```bash
cargo fmt --check
```
Result: **PASS** (code formatted per Rust conventions)

### Linting
```bash
cargo clippy --all-targets -- -D warnings
```
Result: **PASS** (no warnings, no clippy violations)

### Full Test Suite
```bash
cargo test -p proxypilot-core
```
Result: **PASS** (22/22 tests passing, output pristine)

## Self-Review Findings

### Correctness
- ✅ All 12 test cases pass successfully
- ✅ Edge cases handled correctly:
  - `/0` CIDR prefix doesn't panic on 32-bit shift (uses special-case check)
  - `/32` CIDR prefix matches only the exact address
  - IPv6 addresses with brackets are properly unwrapped
  - Dot-suffix matching requires at least one character before the dot (`.local` doesn't match `local`)
  - CIDR entries never match hostnames that aren't IP addresses
- ✅ Empty/blank entries are silently ignored (no panics or errors)

### Code Quality
- ✅ Follows brief specifications exactly (no additions, no omissions)
- ✅ Documentation comments explain the purpose of each type and method
- ✅ Russian comments preserved from brief as-is
- ✅ `Entry` enum is private (implementation detail)
- ✅ Only public API is `BypassList::parse()` and `BypassList::matches()`
- ✅ Code formatting passes `cargo fmt`
- ✅ No clippy warnings or violations

### Test Coverage
- ✅ Exact hostname matching (with case-insensitivity)
- ✅ Dot-suffix matching (subdomain case)
- ✅ IP literal matching (IPv4 and IPv6)
- ✅ CIDR matching (inside range, outside range, edge cases)
- ✅ CIDR never matches non-IP hostnames
- ✅ Empty/blank entry handling
- ✅ IPv6 bracket unwrapping
- ✅ Zero-prefix and full-prefix CIDR edge cases

## Implementation Details

### Matching Algorithm
1. Normalize the host string: remove IPv6 brackets, convert to lowercase
2. Attempt to parse as IpAddr (succeeds for IP literals, fails for hostnames)
3. Iterate through all entries, short-circuit on first match:
   - **Exact**: lowercase host == lowercase entry
   - **Suffix**: host ends with `.suffix` (prevents false matches on just the suffix)
   - **Ip**: parsed IP equals entry IP
   - **Cidr4**: apply bitmask to both the entry network and the host IP, compare

### CIDR Masking
The critical detail for `/0` handling:
```rust
let mask = if *bits == 0 { 0 } else { u32::MAX << (32 - bits) };
```

This avoids undefined behavior from shifting left by 32 bits, which would panic in debug mode and be undefined in release mode.

## Git Commit

```
commit 095369d62b3f819c93575a0aed9a2fb02a506fe9
Author: d.maramygin <denismakste@gmail.com>
Date:   Sun Aug 30 01:16:37 2026 +0500

    feat(win): bypass-матчер — имя, суффикс, CIDR, IP

 win/crates/core/src/bypass.rs | 177 ++++++++++++++++++++++++++++++++++++++++++
 win/crates/core/src/lib.rs    |   1 +
 2 files changed, 178 insertions(+)
```

## Issues or Concerns

None. The implementation is complete, all tests pass, code quality checks pass, and all requirements from the brief have been met.
