### Task 2: Bypass-матчер

**Files:**
- Create: `win/crates/core/src/bypass.rs`
- Modify: `win/crates/core/src/lib.rs`

**Interfaces:**
- Consumes: ничего из предыдущих задач.
- Produces: `BypassList::parse(list: &str) -> BypassList`, `BypassList::matches(&self, host: &str) -> bool`. Мост вызывает `matches` для каждого соединения.

- [ ] **Step 1: Написать падающий тест**

`win/crates/core/src/bypass.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "localhost,127.0.0.1,::1,.local,192.168.0.0/16,10.0.0.0/8,git.company.kz";

    fn list() -> BypassList {
        BypassList::parse(LIST)
    }

    #[test]
    fn exact_hostname_matches() {
        assert!(list().matches("localhost"));
        assert!(list().matches("git.company.kz"));
    }

    #[test]
    fn exact_hostname_is_case_insensitive() {
        assert!(list().matches("LocalHost"));
        assert!(list().matches("GIT.Company.KZ"));
    }

    #[test]
    fn dot_suffix_matches_subdomains_only() {
        assert!(list().matches("printer.local"));
        assert!(list().matches("a.b.local"));
        // сам суффикс без метки слева — не совпадение
        assert!(!list().matches("local"));
    }

    #[test]
    fn ip_literal_matches() {
        assert!(list().matches("127.0.0.1"));
        assert!(list().matches("::1"));
    }

    #[test]
    fn cidr_matches_addresses_inside() {
        assert!(list().matches("203.0.113.246"));
        assert!(list().matches("10.20.30.40"));
    }

    #[test]
    fn cidr_does_not_match_outside() {
        assert!(!list().matches("172.16.0.1"));
        assert!(!list().matches("8.8.8.8"));
    }

    #[test]
    fn cidr_never_matches_a_hostname() {
        // Имя не адрес: «192.168.0.0/16» не должно ловить «example.com».
        assert!(!list().matches("example.com"));
        assert!(!list().matches("api.anthropic.com"));
    }

    #[test]
    fn empty_and_blank_entries_are_ignored() {
        let l = BypassList::parse("localhost, ,,  ,127.0.0.1");
        assert!(l.matches("localhost"));
        assert!(!l.matches("anything.else"));
    }

    #[test]
    fn empty_list_matches_nothing() {
        let l = BypassList::parse("");
        assert!(!l.matches("localhost"));
    }

    #[test]
    fn bracketed_ipv6_host_is_unwrapped() {
        // В CONNECT адрес приходит как [::1]:443
        assert!(list().matches("[::1]"));
    }

    #[test]
    fn zero_prefix_cidr_matches_every_ipv4() {
        // /0 не должен паниковать на сдвиге на 32
        let l = BypassList::parse("0.0.0.0/0");
        assert!(l.matches("8.8.8.8"));
        assert!(!l.matches("example.com"));
    }

    #[test]
    fn full_prefix_cidr_matches_single_address() {
        let l = BypassList::parse("203.0.113.246/32");
        assert!(l.matches("203.0.113.246"));
        assert!(!l.matches("203.0.113.247"));
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-core bypass`
Expected: FAIL — `BypassList` не определён.

- [ ] **Step 3: Написать минимальную реализацию**

Вставь в начало `win/crates/core/src/bypass.rs`:

```rust
//! Какие адреса идут мимо апстрима.
//!
//! Правило живёт здесь, в мосте, а не в клиентах — и это осознанно.
//! Node/Bun и python-requests не понимают CIDR (только точное имя или
//! суффикс с точкой), а часть приложений вообще перетирает NO_PROXY своим
//! списком. Мост — единственное место, где список соблюдается гарантированно.

use std::net::{IpAddr, Ipv4Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    /// точное имя хоста, в нижнем регистре
    Exact(String),
    /// суффикс с ведущей точкой, в нижнем регистре: ".local"
    Suffix(String),
    /// IPv4-подсеть: адрес сети и длина префикса
    Cidr4 { net: u32, bits: u32 },
    /// конкретный адрес
    Ip(IpAddr),
}

#[derive(Debug, Clone, Default)]
pub struct BypassList {
    entries: Vec<Entry>,
}

impl BypassList {
    /// Разбирает список через запятую. Нераспознанные элементы трактуются
    /// как имена хостов — молча игнорировать их было бы хуже.
    pub fn parse(list: &str) -> Self {
        let mut entries = Vec::new();
        for raw in list.split(',') {
            let e = raw.trim();
            if e.is_empty() {
                continue;
            }
            if let Some((net, bits)) = e.split_once('/') {
                if let (Ok(ip), Ok(b)) = (net.parse::<Ipv4Addr>(), bits.parse::<u32>()) {
                    if b <= 32 {
                        entries.push(Entry::Cidr4 { net: u32::from(ip), bits: b });
                        continue;
                    }
                }
            }
            if let Ok(ip) = e.parse::<IpAddr>() {
                entries.push(Entry::Ip(ip));
                continue;
            }
            if let Some(sfx) = e.strip_prefix('.') {
                entries.push(Entry::Suffix(format!(".{}", sfx.to_ascii_lowercase())));
                continue;
            }
            entries.push(Entry::Exact(e.to_ascii_lowercase()));
        }
        Self { entries }
    }

    /// `host` — имя или адрес без порта. Скобки вокруг IPv6 снимаются.
    pub fn matches(&self, host: &str) -> bool {
        let h = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let ip = h.parse::<IpAddr>().ok();

        self.entries.iter().any(|e| match e {
            Entry::Exact(s) => h == *s,
            Entry::Suffix(s) => h.ends_with(s.as_str()),
            Entry::Ip(a) => ip == Some(*a),
            Entry::Cidr4 { net, bits } => match ip {
                Some(IpAddr::V4(v4)) => {
                    // сдвиг на 32 — паника в debug, поэтому /0 отдельно
                    let mask = if *bits == 0 { 0 } else { u32::MAX << (32 - bits) };
                    (u32::from(v4) & mask) == (*net & mask)
                }
                _ => false,
            },
        })
    }
}
```

`win/crates/core/src/lib.rs`:

```rust
pub mod bypass;
pub mod mode;
```

- [ ] **Step 4: Проверить, что тесты проходят**

Run: `cd win && cargo test -p proxypilot-core`
Expected: PASS, 22 теста.

- [ ] **Step 5: Коммит**

```bash
git add win/crates/core
git commit -m "feat(win): bypass-матчер — имя, суффикс, CIDR, IP"
```

---

