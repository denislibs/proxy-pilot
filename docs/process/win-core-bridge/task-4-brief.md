### Task 4: Держатель маршрута (атомарная смена)

**Files:**
- Create: `win/crates/bridge/Cargo.toml`
- Create: `win/crates/bridge/src/lib.rs`
- Create: `win/crates/bridge/src/router.rs`
- Modify: `win/Cargo.toml` (вернуть `crates/bridge` в `members`)

**Interfaces:**
- Consumes: `Route` из задачи 1.
- Produces: `Router::new(Route) -> Router`, `Router::get(&self) -> Arc<Route>`, `Router::set(&self, Route)`.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/router.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn returns_the_route_it_was_built_with() {
        let r = Router::new(Route::Direct);
        assert_eq!(*r.get(), Route::Direct);
    }

    #[test]
    fn set_replaces_the_route_for_later_readers() {
        let r = Router::new(Route::Direct);
        r.set(Route::Socks("10.0.0.2:9999".into()));
        assert_eq!(*r.get(), Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn a_handle_taken_before_set_keeps_the_old_route() {
        // Это и есть свойство, ради которого писался свой мост: соединение
        // взяло маршрут в момент установки и доживает по нему, что бы ни
        // переключили потом.
        let r = Router::new(Route::Socks("old:1080".into()));
        let held = r.get();
        r.set(Route::Direct);
        assert_eq!(*held, Route::Socks("old:1080".into()));
        assert_eq!(*r.get(), Route::Direct);
    }

    #[test]
    fn is_shareable_across_threads() {
        let r = Arc::new(Router::new(Route::Direct));
        let r2 = Arc::clone(&r);
        let t = std::thread::spawn(move || {
            r2.set(Route::Http("p:3128".into()));
        });
        t.join().unwrap();
        assert_eq!(*r.get(), Route::Http("p:3128".into()));
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: FAIL — крейта `proxypilot-bridge` ещё нет.

- [ ] **Step 3: Написать минимальную реализацию**

`win/crates/bridge/Cargo.toml`:

```toml
[package]
name = "proxypilot-bridge"
edition.workspace = true
rust-version.workspace = true
version.workspace = true

[dependencies]
proxypilot-core = { path = "../core" }
tokio = { workspace = true }
arc-swap = { workspace = true }
thiserror = { workspace = true }
```

Верни `crates/bridge` в `members` в `win/Cargo.toml`.

Вставь в начало `win/crates/bridge/src/router.rs`:

```rust
//! Текущий маршрут моста.
//!
//! Смена маршрута обязана быть атомарной и НЕ трогать установленные
//! соединения: каждое соединение читает маршрут один раз, в момент приёма,
//! и дальше живёт с этим значением. Именно поэтому в macOS-версии был нужен
//! трёхуровневый антифлаппинг — там смена режима перезапускала gost и рвала
//! всё живое. Здесь рвать нечего.

use std::sync::Arc;

use arc_swap::ArcSwap;
use proxypilot_core::mode::Route;

#[derive(Debug)]
pub struct Router {
    current: ArcSwap<Route>,
}

impl Router {
    pub fn new(route: Route) -> Self {
        Self { current: ArcSwap::from_pointee(route) }
    }

    /// Снимок маршрута. Держатель снимка не заметит последующих `set`.
    pub fn get(&self) -> Arc<Route> {
        self.current.load_full()
    }

    pub fn set(&self, route: Route) {
        self.current.store(Arc::new(route));
    }
}
```

`win/crates/bridge/src/lib.rs`:

```rust
pub mod router;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-bridge`
Expected: PASS, 4 теста.

- [ ] **Step 5: Коммит**

```bash
git add win/Cargo.toml win/crates/bridge
git commit -m "feat(win): держатель маршрута с атомарной сменой"
```

---

