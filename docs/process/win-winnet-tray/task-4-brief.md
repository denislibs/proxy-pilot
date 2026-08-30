### Task 4: Место в конфиге и `Place::network`

**Files:**
- Modify: `win/crates/core/src/mode.rs`
- Modify: `win/crates/core/src/config.rs`

**Interfaces:**
- Consumes: `Place`, `Config` из плана 1; `NetworkSnapshot` концептуально (без зависимости — `core` остаётся платформонезависимым, идентификатор передаётся строкой).
- Produces: `Place { in_office: bool, network: Option<String> }`, `Config::office_networks: Vec<OfficeNetwork>`, `OfficeNetwork { id: String, name: String }`, `Config::place_for(&self, connected_ids: &[String]) -> Place`.

**Почему сейчас.** Финальное ревью плана 1: у `Place` сейчас один не-тестовый конструктор, и добавить поле дёшево; после появления супервизора и трея это станет ломающей правкой в десятке мест.

- [ ] **Step 1: Написать падающий тест**

Добавь в `mod tests` в `config.rs`:

```rust
    fn office_cfg() -> Config {
        let mut c = Config::default();
        c.office_networks = vec![
            OfficeNetwork { id: "{AAAA0000-0000-0000-0000-000000000001}".into(), name: "Офис".into() },
            OfficeNetwork { id: "{AAAA0000-0000-0000-0000-000000000002}".into(), name: "Офис-2".into() },
        ];
        c
    }

    #[test]
    fn place_is_office_when_a_connected_network_matches() {
        let p = office_cfg().place_for(&["{AAAA0000-0000-0000-0000-000000000002}".to_string()]);
        assert!(p.in_office);
        assert_eq!(p.network.as_deref(), Some("{AAAA0000-0000-0000-0000-000000000002}"));
    }

    #[test]
    fn place_is_not_office_for_an_unknown_network() {
        let p = office_cfg().place_for(&["{BBBB0000-0000-0000-0000-000000000000}".to_string()]);
        assert!(!p.in_office);
        assert_eq!(p.network.as_deref(), Some("{BBBB0000-0000-0000-0000-000000000000}"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        // GUID из реестра и из конфига могут отличаться регистром — это
        // один и тот же идентификатор, и различать их было бы ловушкой.
        let p = office_cfg().place_for(&["{aaaa0000-0000-0000-0000-000000000001}".to_string()]);
        assert!(p.in_office);
    }

    #[test]
    fn no_network_at_all_is_not_office() {
        let p = office_cfg().place_for(&[]);
        assert!(!p.in_office);
        assert!(p.network.is_none());
    }

    #[test]
    fn without_configured_offices_nothing_is_office() {
        // Пустой список — «мы не знаем, где находимся». Считать это офисом
        // означало бы гнать весь трафик через прокси в любой сети.
        let p = Config::default().place_for(&["{AAAA0000-0000-0000-0000-000000000001}".to_string()]);
        assert!(!p.in_office);
    }

    #[test]
    fn several_connected_networks_office_wins() {
        // Ноутбук может быть одновременно в Wi-Fi и в доке по кабелю.
        // Если хоть одна из них офисная — мы в офисе.
        let p = office_cfg().place_for(&[
            "{CCCC0000-0000-0000-0000-000000000000}".to_string(),
            "{AAAA0000-0000-0000-0000-000000000001}".to_string(),
        ]);
        assert!(p.in_office);
        assert_eq!(p.network.as_deref(), Some("{AAAA0000-0000-0000-0000-000000000001}"));
    }
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-core place`
Expected: FAIL — `OfficeNetwork` и `place_for` не определены.

- [ ] **Step 3: Написать реализацию**

В `mode.rs` заменить `Place`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub in_office: bool,
    /// Идентификатор сети, по которой принято решение. Нужен, чтобы UI мог
    /// показать «сейчас: Офис», а лог — объяснить, почему выбран маршрут.
    pub network: Option<String>,
}
```

`Place` перестаёт быть `Copy` — поправь места, где он копировался.

В `config.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeNetwork {
    /// GUID сети в канонической форме, как его отдаёт NLM.
    pub id: String,
    /// Человекочитаемое имя — только для UI, в сравнении не участвует:
    /// пользователь может переименовать сеть, а идентификатор останется.
    #[serde(default)]
    pub name: String,
}

impl Config {
    /// Где мы, судя по списку подключённых сетей.
    ///
    /// Пустой список офисов означает «не знаем» и трактуется как «не офис»:
    /// считать иначе значило бы гнать весь трафик через прокси в любой сети.
    pub fn place_for(&self, connected_ids: &[String]) -> Place {
        let office = connected_ids.iter().find(|id| {
            self.office_networks
                .iter()
                .any(|o| o.id.eq_ignore_ascii_case(id))
        });
        match office {
            Some(id) => Place { in_office: true, network: Some(id.clone()) },
            None => Place { in_office: false, network: connected_ids.first().cloned() },
        }
    }
}
```

И поле в `Config` (плюс `office_networks: Vec::new()` в `Default`):

```rust
    #[serde(default)]
    pub office_networks: Vec<OfficeNetwork>,
```

- [ ] **Step 4: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/core
git commit -m "feat(win): офисные сети в конфиге и Place::network"
```

---

