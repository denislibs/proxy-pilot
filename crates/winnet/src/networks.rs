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
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}

/// Подключённые сейчас сети. Вызывающий обязан держать живым `ComGuard`.
pub fn list_connected() -> Result<Vec<NetworkSnapshot>, WinNetError> {
    // SAFETY: COM инициализирован вызывающим (ComGuard) на этом же потоке;
    // CoCreateInstance создаёт объект в текущем апартаменте, а сами
    // COM-интерфейсы (INetworkListManager, IEnumNetworks, INetwork)
    // освобождаются автоматически по Drop, который генерирует windows-rs.
    unsafe {
        let manager: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
        let enumerator = manager.GetNetworks(NLM_ENUM_NETWORK_CONNECTED)?;

        let mut out = Vec::new();
        loop {
            // `IEnumNetworks::Next` — классический OLE-энумератор: просим
            // один элемент за раз, реальное число попавших в `item`
            // приходит через `fetched` (у windows-rs 0.58 это
            // `Option<*mut u32>`, а не `&mut u32`, как в более новых крейтах
            // для похожих энумераторов).
            let mut item = [None::<INetwork>; 1];
            let mut fetched = 0u32;
            enumerator.Next(&mut item, Some(&mut fetched))?;
            if fetched == 0 {
                break;
            }
            let Some(net) = item[0].take() else { break };

            let id = format_guid(&net.GetNetworkId()?);
            let name = net.GetName()?.to_string();
            let connected = net.IsConnected()?.as_bool();
            let internet = net.IsConnectedToInternet()?.as_bool();
            let category = NetworkCategory::from_raw(net.GetCategory()?.0);

            out.push(NetworkSnapshot {
                id,
                name,
                connected,
                category,
                internet,
            });
        }
        Ok(out)
    }
}

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

    #[test]
    fn guid_with_leading_zeros_keeps_fixed_field_widths() {
        // У 0x1234...cdef выше ни в одном поле нет нулевого полубайта, так
        // что этот тест прошёл бы, даже удали кто-то спецификаторы ширины
        // (`08`/`04`/`02`) из format!. Проверяем на значении, где нули есть,
        // и сверяем точную строку, а не только длину и регистр.
        let g = windows::core::GUID::from_u128(0x0000_000B_000C_00D0_0001_0000_0000_0A00);
        assert_eq!(format_guid(&g), "{0000000B-000C-00D0-0001-000000000A00}");
    }

    #[cfg(windows)]
    #[test]
    fn listing_connected_networks_does_not_fail_on_a_real_machine() {
        // Смоук: на живой машине вызов обязан отработать. Список может быть
        // пустым (машина без сети) — это не ошибка.
        let _guard = crate::com::ComGuard::new().expect("COM должен подняться");
        let nets = list_connected().expect("перечисление сетей не должно падать");
        for n in &nets {
            // Ровно та форма, что попадёт в конфиг и что человек сверит с
            // `Get-NetConnectionProfile` — не просто "непусто".
            assert!(
                n.id.starts_with('{'),
                "GUID обязан быть в фигурных скобках: {}",
                n.id
            );
            assert_eq!(n.id.len(), 38, "канонический GUID — 38 символов: {}", n.id);
            assert!(n.connected, "list_connected отдаёт только подключённые");
        }
    }
}
