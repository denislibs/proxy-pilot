### Task 3: Крейт winnet и опознание сетей через NLM

**Files:**
- Create: `win/crates/winnet/Cargo.toml`
- Create: `win/crates/winnet/src/lib.rs`
- Create: `win/crates/winnet/src/com.rs`
- Create: `win/crates/winnet/src/networks.rs`
- Modify: `win/Cargo.toml`

**Interfaces:**
- Consumes: ничего.
- Produces: `NetworkSnapshot { id: String, name: String, connected: bool, category: NetworkCategory, internet: bool }`, `NetworkCategory { Public, Private, Domain, Unknown }`, `list_connected() -> Result<Vec<NetworkSnapshot>, WinNetError>`, `ComGuard`.

**Почему это ядро всего плана.** macOS-версия не имеет понятия «сеть как объект» и вынуждена строить эвристику: перебрать интерфейсы, взять адрес и шлюз, сверить строковые префиксы, убедиться, что шлюз отвечает за физическим интерфейсом, пингануть, откатиться на ARP. Полторы сотни строк, и вся конструкция существует ради одного — чтобы поднятый VPN не сошёл за офис. Windows держит реестр сетей: у каждой есть GUID, имя, категория и состояние. Сравнение по GUID не подделывается туннелем, не ломается при смене подсети и не требует ни ICMP, ни ARP. Вся эвристическая половина не портируется.

- [ ] **Step 1: Создать крейт**

`win/crates/winnet/Cargo.toml`:

```toml
[package]
name = "proxypilot-winnet"
edition.workspace = true
rust-version.workspace = true
version.workspace = true

[target.'cfg(windows)'.dependencies]
windows = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_System_Com",
    "Win32_System_Variant",
    "Win32_Networking_NetworkListManager",
] }

[dependencies]
thiserror = { workspace = true }
tracing = { workspace = true }
```

В `win/Cargo.toml`: добавить `"crates/winnet"` в `members` и `windows = "0.58"` в `[workspace.dependencies]`.

- [ ] **Step 2: Написать падающий тест**

`win/crates/winnet/src/networks.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_maps_every_documented_value() {
        assert_eq!(NetworkCategory::from_raw(0), NetworkCategory::Public);
        assert_eq!(NetworkCategory::from_raw(1), NetworkCategory::Private);
        assert_eq!(NetworkCategory::from_raw(2), NetworkCategory::Domain);
        // Неизвестное значение не должно паниковать: Windows может завести
        // новую категорию, и падать из-за этого мы не обязаны.
        assert_eq!(NetworkCategory::from_raw(99), NetworkCategory::Unknown);
    }

    #[test]
    fn guid_is_formatted_in_the_canonical_braced_form() {
        // Этот идентификатор пользователь увидит в конфиге и, возможно,
        // сверит с `Get-NetConnectionProfile`. Форма обязана совпадать.
        let g = windows::core::GUID::from_u128(0x1234_5678_90ab_cdef_1234_5678_90ab_cdef);
        let s = format_guid(&g);
        assert!(s.starts_with('{') && s.ends_with('}'), "получили: {s}");
        assert_eq!(s.len(), 38);
        assert_eq!(s, s.to_uppercase(), "канонично — верхний регистр");
    }

    #[cfg(windows)]
    #[test]
    fn listing_connected_networks_does_not_fail_on_a_real_machine() {
        // Смоук: на живой машине вызов обязан отработать. Список может быть
        // пустым (машина без сети) — это не ошибка.
        let _guard = crate::com::ComGuard::new().expect("COM должен подняться");
        let nets = list_connected().expect("перечисление сетей не должно падать");
        for n in &nets {
            assert!(!n.id.is_empty(), "у сети обязан быть идентификатор");
            assert!(n.connected, "list_connected отдаёт только подключённые");
        }
    }
}
```

- [ ] **Step 3: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-winnet`
Expected: FAIL — крейта и его типов нет.

- [ ] **Step 4: Написать реализацию**

`win/crates/winnet/src/com.rs`:

```rust
//! Инициализация COM.
//!
//! NLM — COM-объект, и до первого обращения поток обязан войти в апартамент.
//! Держим это стражем, а не свободной функцией: `CoUninitialize` обязан
//! вызваться на том же потоке и ровно столько же раз, сколько `CoInitialize`.

use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};

use crate::WinNetError;

/// Пока жив — поток в апартаменте. Сбрасывать вручную не нужно.
pub struct ComGuard;

impl ComGuard {
    pub fn new() -> Result<Self, WinNetError> {
        // SAFETY: вызов на текущем потоке, парный CoUninitialize — в Drop.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok()?;
        }
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: парный вызов к CoInitializeEx на том же потоке.
        unsafe { CoUninitialize() };
    }
}
```

`win/crates/winnet/src/networks.rs` (перед блоком тестов):

```rust
//! Опознание сети через Network List Manager.
//!
//! Windows помнит каждую сеть, которую видела: у неё есть GUID, имя,
//! категория (Public/Private/Domain) и состояние. Сравнение по GUID —
//! то, чего у macOS-версии не было и ради чего там пришлось городить
//! эвристику из адреса, шлюза, ping и ARP: GUID не подделывается поднятым
//! туннелем и не меняется при смене подсети.

use windows::core::GUID;
use windows::Win32::Networking::NetworkListManager::{
    INetwork, INetworkListManager, NetworkListManager, NLM_ENUM_NETWORK_CONNECTED,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use crate::WinNetError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkCategory {
    Public,
    Private,
    Domain,
    Unknown,
}

impl NetworkCategory {
    pub fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Public,
            1 => Self::Private,
            2 => Self::Domain,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSnapshot {
    /// GUID в канонической форме `{XXXXXXXX-XXXX-...}` — то, что попадёт
    /// в конфиг и что человек может сверить с `Get-NetConnectionProfile`.
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub category: NetworkCategory,
    pub internet: bool,
}

pub fn format_guid(g: &GUID) -> String {
    format!("{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3,
        g.data4[0], g.data4[1], g.data4[2], g.data4[3],
        g.data4[4], g.data4[5], g.data4[6], g.data4[7])
}

/// Подключённые сейчас сети. Вызывающий обязан держать живым `ComGuard`.
pub fn list_connected() -> Result<Vec<NetworkSnapshot>, WinNetError> {
    // SAFETY: COM инициализирован вызывающим (ComGuard), интерфейсы
    // освобождаются самим windows-rs по Drop.
    unsafe {
        let manager: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
        let enumerator = manager.GetNetworks(NLM_ENUM_NETWORK_CONNECTED)?;

        let mut out = Vec::new();
        loop {
            let mut item = [None::<INetwork>; 1];
            let mut fetched = 0u32;
            enumerator.Next(&mut item, &mut fetched)?;
            if fetched == 0 {
                break;
            }
            let Some(net) = item[0].as_ref() else { break };

            let id = format_guid(&net.GetNetworkId()?);
            let name = net.GetName()?.to_string();
            let connected = net.IsConnected()?.as_bool();
            let internet = net.IsConnectedToInternet()?.as_bool();
            let category = NetworkCategory::from_raw(net.GetCategory()?.0);

            out.push(NetworkSnapshot { id, name, connected, category, internet });
        }
        Ok(out)
    }
}
```

`win/crates/winnet/src/lib.rs`:

```rust
//! Всё, что говорит с Windows: опознание сети, настройки прокси, события.
//!
//! Вынесено отдельным крейтом сознательно: `proxypilot-core` обязан
//! оставаться без платформенных зависимостей, а `proxypilot-bridge` —
//! переносимым (он говорит только на tokio).

pub mod com;
pub mod networks;

#[derive(Debug, thiserror::Error)]
pub enum WinNetError {
    #[error("ошибка Windows: {0}")]
    Windows(#[from] windows::core::Error),
}
```

- [ ] **Step 5: Прогнать тесты и линтеры**

Run: `cd win && cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS. Смоук-тест на живой машине обязан перечислить хотя бы одну сеть, если машина в сети.

- [ ] **Step 6: Ручная проверка — вывести реальные сети**

Добавь временный пример или воспользуйся тестом с `-- --nocapture`, чтобы увидеть свои настоящие сети и их GUID. Запиши GUID своей текущей сети в отчёт: он понадобится при настройке офиса.

Run: `cd win && cargo test -p proxypilot-winnet -- --nocapture listing_connected`

- [ ] **Step 7: Коммит**

```bash
git add win/Cargo.toml win/crates/winnet
git commit -m "feat(win): крейт winnet и опознание сетей через NLM"
```

---

