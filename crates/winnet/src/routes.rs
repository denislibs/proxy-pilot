//! Живой сбор IPv4-таблицы маршрутов и списка адаптеров —
//! единственный вызывающий `tunnel_state::{our_tunnel_up, foreign_tunnel_up}`
//! в проде, поставляющий им реальные данные с этой машины.
//!
//! `tunnel_state` сознательно не делает ввода-вывода вовсе (см. её докблок:
//! «таблицу маршрутов и список адаптеров собирает вызывающий»). Этот модуль
//! и есть тот вызывающий — задача 7, единственная, кому в проде понадобилась
//! живая таблица маршрутов.
//!
//! Два источника сводятся вместе, тем же приёмом, что и в
//! `crates/netsvc/src/adapter.rs` (растущий буфер `GetAdaptersAddresses` +
//! join по индексу интерфейса):
//! - `GetIpForwardTable2` (IP Helper) — сама таблица маршрутов IPv4: для
//!   каждой записи — подсеть назначения и индекс интерфейса, через который
//!   она идёт;
//! - `GetAdaptersAddresses` — по индексу интерфейса отдаёт его
//!   «дружественное имя» (`interface_alias` в терминах `tunnel_state`) и
//!   `IfType`, по которому решается [`is_tunnel_if_type`].
//!
//! Только чтение — ни один вызов здесь не меняет ни таблицу маршрутов, ни
//! настройки адаптера. Это тот же класс операции, что и `route print -4`,
//! которым задача 3 сверяла `tunnel_state` на живой машине (см. её отчёт):
//! CLAUDE.md запрещает агенту `netsh ... set ...`, подключение/отключение
//! OpenVPN и установку службы, но не запрещает чтение.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use proxypilot_core::net::Ipv4Net;
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetAdaptersAddresses, GetIpForwardTable2, GET_ADAPTERS_ADDRESSES_FLAGS,
    IF_TYPE_PPP, IF_TYPE_PROP_VIRTUAL, IF_TYPE_TUNNEL, IP_ADAPTER_ADDRESSES_LH,
    MIB_IPFORWARD_TABLE2,
};
use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_INET};

use crate::tunnel_state::AdapterRoute;

#[derive(Debug, thiserror::Error)]
pub enum RoutesError {
    #[error("GetIpForwardTable2 отказала с кодом {0}")]
    ForwardTable(u32),
    #[error("GetAdaptersAddresses отказала с кодом {0}")]
    Adapters(u32),
}

/// Один из трёх типов IANA `ifType` (стандартные, платформенно-нейтральные
/// номера — RFC 2863), под которыми на практике ходят туннельные адаптеры
/// Windows: `PPP` (23, коммерческие/L2TP VPN), `PROP_VIRTUAL` (53, под ним
/// регистрируются и TAP-Windows, и Wintun, и WireGuard-подобные драйверы),
/// `TUNNEL` (131, явные IP-in-IP туннели). Та же классификация, что
/// `docs/design.md` и докблок `tunnel_state` описывают словами «tun/utun» —
/// перенесена с macOS-версии на понятия, которые отдаёт Windows.
///
/// Обычные адаптеры (Ethernet = 6, Wi-Fi = 71, loopback = 24, ...) под эти
/// номера не попадают — чистая функция ради теста, отдельно от чтения
/// реального `IfType` из `GetAdaptersAddresses`.
fn is_tunnel_if_type(if_type: u32) -> bool {
    matches!(if_type, IF_TYPE_PPP | IF_TYPE_PROP_VIRTUAL | IF_TYPE_TUNNEL)
}

/// Дружественное имя и признак «туннельный» для каждого индекса интерфейса
/// на машине сейчас.
fn adapter_info_by_index() -> Result<HashMap<u32, (String, bool)>, RoutesError> {
    let mut map = HashMap::new();
    for_each_adapter(|adapter| {
        // SAFETY: `Anonymous1` — union, чей единственный содержательный
        // вариант (`Anonymous`, поле `IfIndex`) документирован Microsoft как
        // всегда заполненный `GetAdaptersAddresses`; тот же приём, что и
        // чтение `Anonymous2.Flags` в `netsvc::adapter::current_ipv4_config`.
        let if_index = unsafe { adapter.Anonymous1.Anonymous.IfIndex };
        // SAFETY: `FriendlyName` — живой PWSTR внутри буфера, которым
        // владеет `for_each_adapter` на всё время замыкания.
        let friendly = unsafe { adapter.FriendlyName.to_string() }.unwrap_or_default();
        map.insert(if_index, (friendly, is_tunnel_if_type(adapter.IfType)));
    })?;
    Ok(map)
}

/// `SOCKADDR_INET` → `Ipv4Addr`, если это вообще IPv4 (`AF_INET`) — таблица
/// запрошена с `family = AF_INET`, поэтому иное здесь не ожидается, но
/// проверка дешёвая и делает предположение явным, а не молчаливым.
fn sockaddr_inet_to_ipv4(addr: &SOCKADDR_INET) -> Option<Ipv4Addr> {
    // SAFETY: `si_family` — общее по смещению поле для всех вариантов
    // union'а (тот же приём, что и в `netsvc::adapter::sockaddr_to_ipv4`
    // для родственной структуры `SOCKET_ADDRESS`).
    if unsafe { addr.si_family } != AF_INET {
        return None;
    }
    // SAFETY: `si_family == AF_INET` подтверждает, что активный вариант —
    // `Ipv4` (документированный контракт `SOCKADDR_INET`).
    let sin_addr = unsafe { addr.Ipv4.sin_addr };
    // SAFETY: `S_un` — union из вариантов одного размера (4 байта); чтение
    // байтового варианта корректно независимо от того, каким вариантом его
    // заполнил API.
    let bytes = unsafe { sin_addr.S_un.S_un_b };
    Some(Ipv4Addr::new(
        bytes.s_b1, bytes.s_b2, bytes.s_b3, bytes.s_b4,
    ))
}

/// Живая таблица маршрутов IPv4 вместе с адаптерами, которым каждый
/// маршрут принадлежит — прямой вход для `tunnel_state::our_tunnel_up` /
/// `foreign_tunnel_up`.
///
/// Маршруты не-IPv4 семейств здесь появиться не могут: `GetIpForwardTable2`
/// вызывается с `family = AF_INET`, и Windows отдаёт только их. Маршрут, чей
/// индекс интерфейса не нашёлся среди адаптеров (адаптер исчез между двумя
/// вызовами — окно гонки física возможно, но узкое), получает пустой
/// псевдоним и `is_tunnel = false`: он не совпадёт ни с одним `our_alias` и
/// не попадёт под `is_tunnel` в `foreign_tunnel_up` (окно гонки — адаптер
/// исчез между двумя вызовами — физически возможно, но узкое) — отказ в
/// консервативную сторону, тот же принцип, что `same_alias` в `tunnel_state`.
pub fn gather_ipv4_routes() -> Result<Vec<AdapterRoute>, RoutesError> {
    let adapters = adapter_info_by_index()?;

    let mut table_ptr: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    // SAFETY: `table_ptr` — живая локальная переменная; при успехе API
    // выделяет память сама и отдаёт указатель на неё, освобождается
    // парным `FreeMibTable` ниже (страж `TableGuard`).
    let rc = unsafe { GetIpForwardTable2(AF_INET, &mut table_ptr) };
    if rc != ERROR_SUCCESS {
        return Err(RoutesError::ForwardTable(rc.0));
    }

    struct TableGuard(*mut MIB_IPFORWARD_TABLE2);
    impl Drop for TableGuard {
        fn drop(&mut self) {
            // SAFETY: `self.0` — указатель, который `GetIpForwardTable2`
            // сама выделила и вернула через `Ok`-путь; освобождается ровно
            // один раз, здесь же.
            unsafe { FreeMibTable(self.0.cast()) };
        }
    }
    let _guard = TableGuard(table_ptr);

    // SAFETY: `table_ptr` ненулевой (проверено через `rc == ERROR_SUCCESS`)
    // и живёт до конца функции (страж `_guard` освобождает его после).
    let table = unsafe { &*table_ptr };
    let num_entries = table.NumEntries as usize;
    // SAFETY: `Table` объявлен как массив из одного элемента (гибкий
    // массив в стиле C) — реальных элементов `num_entries`, и они лежат
    // подряд сразу после `NumEntries` в том же выделении; тот же приём,
    // которым `GetIpForwardTable2` документирован Microsoft.
    let rows = unsafe { std::slice::from_raw_parts(table.Table.as_ptr(), num_entries) };

    let mut out = Vec::with_capacity(num_entries);
    for row in rows {
        let Some(addr) = sockaddr_inet_to_ipv4(&row.DestinationPrefix.Prefix) else {
            continue;
        };
        let dest = Ipv4Net {
            addr,
            prefix: row.DestinationPrefix.PrefixLength,
        };
        let (interface_alias, is_tunnel) = adapters
            .get(&row.InterfaceIndex)
            .cloned()
            .unwrap_or_default();
        out.push(AdapterRoute {
            dest,
            interface_alias,
            is_tunnel,
        });
    }
    Ok(out)
}

/// Общий каркас чтения `GetAdaptersAddresses` — растущий буфер
/// (`ERROR_BUFFER_OVERFLOW` значит «дай буфер больше», не отказ) и обход
/// связного списка адаптеров. Тот же приём и то же обоснование, что и
/// `netsvc::adapter::for_each_adapter_ref` — независимая копия здесь, а не
/// общая функция, потому что это разные крейты (`winnet` не зависит от
/// `netsvc`, а зависимость в обратную сторону сделала бы `winnet`
/// потребителем бинарной цели службы).
fn for_each_adapter(mut visit: impl FnMut(&IP_ADAPTER_ADDRESSES_LH)) -> Result<(), RoutesError> {
    let mut size: u32 = 15 * 1024;
    let mut buf: Vec<u8>;
    loop {
        buf = vec![0u8; size as usize];
        // SAFETY: `buf` — живой, только что выделенный буфер размера
        // `size`; `family = AF_INET` — нужны только IPv4-адаптеры (задача
        // 7 целиком про IPv4-таблицу маршрутов); `size` — вход и выход.
        let rc = unsafe {
            GetAdaptersAddresses(
                AF_INET.0 as u32,
                GET_ADAPTERS_ADDRESSES_FLAGS(0),
                None,
                Some(buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &mut size,
            )
        };
        if rc == ERROR_SUCCESS.0 {
            break;
        }
        if rc == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        return Err(RoutesError::Adapters(rc));
    }

    // SAFETY: буфер только что успешно заполнен `GetAdaptersAddresses`
    // (`rc == ERROR_SUCCESS`) и живёт до конца этой функции; связный список
    // `Next` гарантированно завершается `null` — так документирован сам API.
    let mut cur = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !cur.is_null() {
        let adapter = unsafe { &*cur };
        visit(adapter);
        cur = adapter.Next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppp_is_a_tunnel_if_type() {
        assert!(is_tunnel_if_type(IF_TYPE_PPP));
    }

    #[test]
    fn prop_virtual_is_a_tunnel_if_type() {
        // TAP-Windows, Wintun и WireGuard-подобные драйверы регистрируются
        // под этим номером.
        assert!(is_tunnel_if_type(IF_TYPE_PROP_VIRTUAL));
    }

    #[test]
    fn tunnel_is_a_tunnel_if_type() {
        assert!(is_tunnel_if_type(IF_TYPE_TUNNEL));
    }

    #[test]
    fn ethernet_is_not_a_tunnel_if_type() {
        // IANA ifType 6 — ethernetCsmacd, обычная проводная карта.
        assert!(!is_tunnel_if_type(6));
    }

    #[test]
    fn wifi_is_not_a_tunnel_if_type() {
        // IANA ifType 71 — ieee80211.
        assert!(!is_tunnel_if_type(71));
    }

    #[test]
    fn software_loopback_is_not_a_tunnel_if_type() {
        // IANA ifType 24 — softwareLoopback: тоже виртуальный адаптер, но
        // не туннель, и не должен путаться с PROP_VIRTUAL (53).
        assert!(!is_tunnel_if_type(24));
    }

    // ---- Смоук на живой машине: только чтение, ничего не меняет ----
    // Тот же класс проверки, что и `winnet::networks::
    // listing_connected_networks_does_not_fail_on_a_real_machine` и
    // `netsvc::adapter::gathering_from_nlm_does_not_fail_on_a_real_machine`
    // — таблица маршрутов может быть любой, отказ (Err) — нет.

    #[test]
    fn gathering_ipv4_routes_does_not_fail_on_a_real_machine() {
        let routes = gather_ipv4_routes().expect("сбор таблицы маршрутов не должен падать");
        // Не проверяем конкретное содержимое (оно у каждой машины своё и
        // CLAUDE.md запрещает называть в репозитории настоящие подсети
        // этой инфраструктуры) — только то, что вызов вообще отработал и
        // вернул структурно осмысленные данные.
        for r in &routes {
            let _ = r.dest.to_string();
        }
    }
}
