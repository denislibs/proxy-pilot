# Task 5 Report: Пробы живости апстримов (Fixes Applied)

## Findings Addressed

### FINDING 1 & 2: Critical Cache and Invalidation Defects

**Original bugs:**
1. Cache keyed only on elapsed time, not on probed addresses — could return results for wrong address
2. Invalidation could be silently undone by in-flight probe overwrites

**Fix:** Restructured with two key changes:

#### Cache Key Now Includes Address
```rust
struct Cached {
    at: Instant,
    probed: Upstreams,  // NEW: Remember what was probed
    socks: Reachability,
    http: Reachability,
}
```

Cache hits now require: `c.probed == *up && c.at.elapsed() < self.ttl`

#### Generation Counter Prevents Invalidation Races
```rust
struct State {
    cache: Option<Cached>,
    generation: u64,  // Incremented on each invalidate
}
```

Flow:
1. `health()` reads generation before probing
2. Probes complete (up to timeout seconds) without holding lock
3. Takes lock again, writes cache **only if generation unchanged**
4. `invalidate()` clears cache and increments generation

If invalidate runs during probe, generation changes and the in-flight result doesn't overwrite.

### FINDING 3: Sequential Probes

Changed from sequential to concurrent:
```rust
// Before:
let socks = self.probe(up.socks.as_deref()).await;
let http = self.probe(up.http.as_deref()).await;

// After:
let (socks, http) = tokio::join!(
    self.probe(up.socks.as_deref()),
    self.probe(up.http.as_deref())
);
```

Worst case is now one timeout, not two.

### FINDING 4: Added Test + RED Evidence

Captures test scenario where cache returns result for wrong address:

```rust
#[tokio::test]
async fn a_changed_address_is_not_answered_from_the_old_cache() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let live = l.local_addr().unwrap().to_string();
    let p = Prober::new(Duration::from_secs(30), Duration::from_millis(300));

    let up_live = Upstreams { socks: Some(live), http: None };
    assert_eq!(p.health(&up_live).await.socks, Reachability::Up);

    let up_dead = Upstreams { socks: Some("127.0.0.1:1".into()), http: None };
    assert_eq!(p.health(&up_dead).await.socks, Reachability::Down);
}
```

## Test Evidence: RED → GREEN

### RED State (Before Fix)
```
running 1 test
test probe::tests::a_changed_address_is_not_answered_from_the_old_cache ... FAILED

failures:

---- probe::tests::a_changed_address_is_not_answered_from_the_old_cache stdout ----

thread 'probe::tests::a_changed_address_is_not_answered_from_the_old_cache' (19596) panicked at crates\bridge\src\probe.rs:168:9:
assertion `left == right` failed
  left: Up       <-- BUG: Returns old result (live address was Up)
 right: Down     <-- Expected: new address (127.0.0.1:1) should be Down

failures:
    probe::tests::a_changed_address_is_not_answered_from_the_old_cache

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 50 filtered out; finished in 0.00s
```

### GREEN State (After Fix)
All probe tests pass:
```
running 5 tests
test probe::tests::an_unconfigured_upstream_is_unknown_not_down ... ok
test probe::tests::a_silent_address_is_down_within_the_timeout ... ok
test probe::tests::a_changed_address_is_not_answered_from_the_old_cache ... ok
test probe::tests::the_result_is_cached_within_the_ttl ... ok
test probe::tests::a_live_listener_is_up_and_a_closed_port_is_down ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 46 filtered out; finished in 1.02s
```

## CI Output: Final

### `cargo test --all`
```
running 51 tests [bridge lib]
test result: ok. 51 passed; 0 failed; finished in 2.04s

running 2 tests [bridge cli]
test result: ok. 2 passed; 0 failed; finished in 0.02s

running 45 tests [core lib]
test result: ok. 45 passed; 0 failed; finished in 0.01s

running 7 tests [winnet lib]
test result: ok. 7 passed; 0 failed; finished in 0.02s

Total: 105 tests (104 original + 1 new a_changed_address test)
```

### `cargo clippy --all-targets -- -D warnings`
```
Checking proxypilot-core v0.1.0
Checking proxypilot-bridge v0.1.0
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.31s
```
No warnings.

### `cargo fmt --all --check`
```
PASS
```

## Generation Counter Analysis

**Why no deterministic race test:** The generation counter's correctness is verified by construction:
- Each `invalidate()` increments generation
- `health()` reads generation, then releases lock before awaiting probes
- After await, checks if generation unchanged before writing
- If invalidate ran during await, generation will differ and write is skipped

The race is deterministic in effect (no write happens) but non-deterministic in timing. A test would either:
1. Pass trivially if the race doesn't happen to occur
2. Require adding timing seams (artificial delays) which defeats testing real concurrency

The structure itself makes the race impossible: once generation increments, any in-flight probe cannot write its result.

## Files Changed

- `C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge\src\probe.rs` — Refactored with `probed` key and `generation` counter
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\core\src\mode.rs` — Added `PartialEq, Eq` derives to `Upstreams`

## Commits

- `d10b65d` — feat(win): пробы живости апстримов с кэшем (initial implementation)
- `3f0c1a6` — fix(win): кэш проб помнит адреса, генерация защищает от гонок (fixes applied)

## Key Properties Preserved

- **`Unknown` ≠ `Down`**: Both paths maintain distinction (cache hits and misses)
- **Timeout respected**: Concurrent probes complete within max timeout
- **No damping**: No artificial delays or state machine; recomputed freely
- **No mutex across await**: Lock held only around state reads/writes, released before probing

## Summary

Both critical defects fixed with a single architectural change: keying cache on addresses and protecting against invalidation races with a generation counter. Concurrent probing reduces worst-case latency. All 105 tests pass; all CI checks pass; no clippy warnings; properly formatted.
