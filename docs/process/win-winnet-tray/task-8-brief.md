### Task 8: Супервизор

**Files:**
- Create: `win/crates/bridge/src/supervisor.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Router` (план 1), `Prober` (задача 5), `Config`/`place_for` (задачи 2, 4), `core::decide`.
- Produces: `NetworkSource` (трейт), `Supervisor::new(router, prober, config, source)`, `Supervisor::reevaluate(&self) -> AppState`, `Supervisor::run(self, events)`, `AppState { mode, route, demoted, place, health, port }`.

**Что делает.** На старте и на каждое схлопнутое событие смены сети: список подключённых сетей → `config.place_for` → `prober.health` → `core::decide` → **если решение изменилось, `router.set()`**. Мост при этом не трогается вообще.

**Трейт ради тестируемости.** `NetworkSource` с одним синхронным методом `connected_ids() -> Result<Vec<String>, ...>` — NLM синхронен, и async здесь ничего не даёт. В бою — реализация поверх `winnet::list_connected`, в тестах — подставная. Это единственное место, где нужен трейт: остальное чистые функции.

- [ ] **Step 1: Написать падающий тест**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct FakeNet(std::sync::Mutex<Vec<String>>);
    impl NetworkSource for FakeNet {
        fn connected_ids(&self) -> Result<Vec<String>, SupervisorError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn office_config(socks: &str) -> Config {
        let mut c = Config::default();
        c.socks_upstream = Some(socks.to_string());
        c.mode = Mode::Auto;
        c.office_networks = vec![OfficeNetwork { id: "{OFFICE}".into(), name: "Офис".into() }];
        c
    }

    #[tokio::test]
    async fn in_the_office_with_a_live_socks_the_route_becomes_socks() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();

        let router = Arc::new(Router::new(Route::Direct));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config(&addr),
            Box::new(FakeNet(Mutex::new(vec!["{OFFICE}".into()]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Socks(addr.clone()));
        assert_eq!(*router.get(), Route::Socks(addr));
        assert!(state.place.in_office);
    }

    #[tokio::test]
    async fn outside_the_office_the_route_is_direct_even_with_a_live_upstream() {
        // Правило спеки 4.2 дословно: снаружи офисный прокси тоже отвечает
        // (через туннель), но маршрут через него был бы кругом через офис.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();

        let router = Arc::new(Router::new(Route::Socks(addr.clone())));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config(&addr),
            Box::new(FakeNet(Mutex::new(vec!["{HOME}".into()]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Direct);
        assert_eq!(*router.get(), Route::Direct);
        assert!(!state.place.in_office);
    }

    #[tokio::test]
    async fn an_unchanged_decision_does_not_touch_the_router() {
        // Лишний set безвреден для соединений, но маскирует ошибки в логике
        // и засоряет лог. Решение не изменилось — значит и трогать нечего.
        let router = Arc::new(Router::new(Route::Direct));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config("127.0.0.1:1"),
            Box::new(FakeNet(Mutex::new(vec!["{HOME}".into()]))),
        );

        let before = router.get();
        sup.reevaluate().await;
        let after = router.get();
        assert!(Arc::ptr_eq(&before, &after), "router.set вызывать не следовало");
    }

    #[tokio::test]
    async fn a_dead_pinned_upstream_is_reported_as_demoted() {
        let router = Arc::new(Router::new(Route::Direct));
        let mut cfg = office_config("127.0.0.1:1");
        cfg.mode = Mode::Socks;
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_millis(200)),
            cfg,
            Box::new(FakeNet(Mutex::new(vec!["{OFFICE}".into()]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Direct);
        assert!(state.demoted, "понижение обязано быть видно в состоянии");
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge supervisor`
Expected: FAIL — `Supervisor` не определён.

- [ ] **Step 3: Написать реализацию**

Ключевой комментарий, который обязан быть в файле:

```rust
//! Супервизор: пересчёт маршрута при смене обстановки.
//!
//! ИНВАРИАНТ. Слушатель привязывается один раз за жизнь процесса и не
//! перепривязывается. Супервизор меняет ТОЛЬКО маршрут — через router.set(),
//! который не касается установленных соединений. Смена порта требует
//! перезапуска моста и обязана быть явным действием пользователя: тихая
//! перепривязка убьёт то самое свойство, ради которого продукт переписан.
```

`reevaluate` возвращает `AppState` — его же читает трей, чтобы нарисовать иконку и меню, не дублируя логику.

- [ ] **Step 4: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`

- [ ] **Step 5: Коммит**

```bash
git add win/crates/bridge
git commit -m "feat(win): супервизор — пересчёт маршрута на смену сети"
```

---

### Task 8: Супервизор

**Files:**
- Create: `win/crates/bridge/src/supervisor.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Router`, `Prober`, `Config`, `decide`, `list_connected`, `watch_network_changes`.
- Produces: `Supervisor::new(...)`, `Supervisor::run(self)`, `Supervisor::state() -> AppState`, `AppState { mode, route, demoted, place, health, port }`.

**Что делает.** На старте и на каждое событие смены сети: спросить NLM список подключённых сетей → `config.place_for` → `prober.health` → `core::decide` → **если решение изменилось, `router.set()`**. Мост при этом не трогается.

**Инвариант, который здесь надо записать и в код, и в комментарий:**

```rust
// Слушатель привязывается один раз за жизнь процесса и не перепривязывается.
// Супервизор меняет ТОЛЬКО маршрут — через router.set(), который не касается
// установленных соединений. Смена порта требует перезапуска моста и обязана
// быть явным действием пользователя: тихая перепривязка убьёт то самое
// свойство, ради которого продукт переписан.
```

- [ ] **Step 1: Тесты на чистую часть** — таблица «место + живость → ожидаемый вызов `router.set` или его отсутствие», с подставными источником сетей и пробером.
- [ ] **Step 2: Тест «решение не изменилось — `set` не вызывается»** — лишний `set` безвреден, но он маскирует ошибки в логике и мешает логу.
- [ ] **Step 3: Реализация.**
- [ ] **Step 4: Коммит** — `feat(win): супервизор — пересчёт маршрута на смену сети`.

---

