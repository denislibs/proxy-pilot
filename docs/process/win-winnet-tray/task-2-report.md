# Task 2 Report: Конфиг на диске

## Summary

Implemented disk-based configuration loading and saving with comprehensive validation of untrusted values from files. Added 8 new tests bringing the total from 77 to 85 tests.

## Implementation Details

### Dependencies Added
- Added `directories = "5"` to win/Cargo.toml [workspace.dependencies]
- Added `directories = { workspace = true }` to win/crates/core/Cargo.toml

### Code Changes

#### ConfigError Enum Extended
Added three new error variants:
- `Io(std::io::Error)` - for file I/O errors
- `Invalid(String)` - for validation failures with descriptive messages
- `NoConfigDir` - when user's config directory cannot be determined

#### Config Implementation
Implemented the following public methods:

1. **`path() -> Option<PathBuf>`** - Returns `%APPDATA%\ProxyPilot\config.toml` using directories crate
2. **`load() -> Result<Self, ConfigError>`** - Loads from default path
3. **`load_from(&Path) -> Result<Self, ConfigError>`** - Loads from specific path; returns defaults if file missing (first-run scenario)
4. **`save(&self) -> Result<(), ConfigError>`** - Saves to default path
5. **`save_to(&self, &Path) -> Result<(), ConfigError>`** - Saves to specific path, creating parent directories as needed
6. **`validate(&self) -> Result<(), ConfigError>`** - Validates configuration with separate checks for future extensibility

#### Validation Rules
The validate() method checks:
1. **bridge_port** - Must be >= 1024 (rejects privileged range)
2. **max_connections** - Must be 1..=65536 (prevents Semaphore::new() panic and guards against absurd values)
3. **upstream addresses** - Both socks_upstream and http_upstream must be valid host:port format when present

The validate() method is structured as discrete blocks so a new check can be appended without rewriting the method (preparation for Task 4's office_networks validation).

### Tests Added (8 total)

1. **validate_rejects_a_port_below_the_privileged_range** - Tests that bridge_port=80 fails validation
2. **validate_rejects_an_absurd_connection_limit** - Tests that max_connections=usize::MAX fails validation (Semaphore safety)
3. **validate_rejects_a_zero_connection_limit** - Tests that max_connections=0 fails validation
4. **validate_rejects_a_malformed_upstream** - Tests that invalid upstream format fails validation
5. **validate_accepts_the_defaults** - Tests that default config validates successfully
6. **load_from_a_missing_file_yields_defaults** - Tests first-run scenario (file missing returns defaults, not error)
7. **save_then_load_roundtrips_through_a_real_file** - Tests save/load with modified config values
8. **load_from_an_invalid_file_is_an_error_not_a_panic** - Tests that invalid TOML is caught as error, not panic

Each test uses distinct temp directory paths to prevent collisions in parallel test runs:
- proxypilot-test-missing
- proxypilot-test-roundtrip
- proxypilot-test-invalid

## TDD Evidence

### RED State (Before Implementation)

13 compilation errors shown:
```
error[E0599]: no method named `validate` found for struct `config::Config`
error[E0599]: no variant named `Invalid` found for enum `config::ConfigError`
error[E0599]: no associated function named `load_from` found for struct `config::Config`
error[E0599]: no method named `save_to` found for struct `config::Config`
```

Full test command output:
```
$ cd win && cargo test -p proxypilot-core config

error[E0599]: no method named `validate` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:170:28

error[E0599]: no variant, associated function, or constant named `Invalid` found for enum `config::ConfigError` in the current scope
   --> crates\core\src\config.rs:170:57

error[E0599]: no method named `validate` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:180:28

error[E0599]: no variant, associated function, or constant named `Invalid` found for enum `config::ConfigError` in the current scope
   --> crates\core\src\config.rs:180:57

error[E0599]: no method named `validate` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:187:28

error[E0599]: no variant, associated function, or constant named `Invalid` found for enum `config::ConfigError` in the current scope
   --> crates\core\src\config.rs:187:57

error[E0599]: no method named `validate` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:194:28

error[E0599]: no variant, associated function, or constant named `Invalid` found for enum `config::ConfigError` in the current scope
   --> crates\core\src\config.rs:194:57

error[E0599]: no method named `validate` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:199:35

error[E0599]: no associated function or constant named `load_from` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:207:25

error[E0599]: no method named `save_to` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:219:11

error[E0599]: no associated function or constant named `load_from` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:221:28

error[E0599]: no associated function or constant named `load_from` found for struct `config::Config` in the current scope
   --> crates\core\src\config.rs:232:25

For more information about the error, try `rustc --explain E0599`.
error: could not compile `proxypilot-core` (lib test) due to 13 previous errors
```

### GREEN State (After Implementation)

Test results after implementation:
```
test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Total: 85 tests (77 before + 8 new)

New tests verified passing:
- config::tests::validate_rejects_a_port_below_the_privileged_range ✓
- config::tests::validate_rejects_an_absurd_connection_limit ✓
- config::tests::validate_rejects_a_zero_connection_limit ✓
- config::tests::validate_rejects_a_malformed_upstream ✓
- config::tests::validate_accepts_the_defaults ✓
- config::tests::load_from_a_missing_file_yields_defaults ✓
- config::tests::save_then_load_roundtrips_through_a_real_file ✓
- config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ✓

Clippy check: ✓ PASS
Fmt check: ✓ PASS

## Self-Review Findings

### Requirements Verification
- ✅ Added directories dependency as specified in brief
- ✅ All method signatures match the brief exactly
- ✅ ConfigError variants match the brief exactly
- ✅ All 8 tests present with correct names from brief
- ✅ validate() structured for future extensibility (Task 4)
- ✅ No silent fallback behavior
- ✅ Config path respects %APPDATA% (no UAC prompts, no 0.0.0.0)

### Code Quality
- ✅ Fixed clippy warnings by using struct initializer syntax
- ✅ Proper error handling with map_err for I/O operations
- ✅ Comprehensive validation with descriptive error messages
- ✅ MAX_CONNECTIONS_CEILING constant documents safety rationale
- ✅ Russian comments explain the why of each check

### Test Isolation
- ✅ Each file I/O test uses unique temp directory names
- ✅ Tests clean up after themselves
- ✅ No collisions possible in parallel test runs

### Design for Extensibility
- ✅ validate() uses sequential if/for blocks, not fused expressions
- ✅ New validation checks can be appended as their own block
- ✅ Task 4's office_networks validation can be added without rewriting validate()

## Files Modified

- win/Cargo.toml
- win/Cargo.lock (updated with directories dependency)
- win/crates/core/Cargo.toml
- win/crates/core/src/config.rs

## Commit Created

```
e47f7cf feat(win): конфиг на диске с валидацией недоверенных значений
```

## Test Summary

- Before: 77 tests (46 bridge + 2 cli + 29 core)
- After: 85 tests (46 bridge + 2 cli + 37 core)
- New tests: 8 (all passing)

All tests pass. Clippy clean. Fmt compliant.
