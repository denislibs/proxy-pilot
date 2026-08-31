//! Сопоставление GUID офисной сети (NLM) с псевдонимом адаптера, которого
//! ожидает `netsh interface ipv4 ... name=`.
//!
//! Адаптер для применения статики берётся **из NLM-подключения к офисной
//! сети, а не по имени службы** (`docs/design.md` §6.4/7.2) — док-станции
//! и вторая сетевая карта перестают быть проблемой, в отличие от
//! macOS-версии, где адаптер задавался строкой (`NET_SERVICE="Wi-Fi"`).
//!
//! Два источника данных сводятся вместе:
//! - `INetworkListManager::GetNetworkConnections` (NLM) — у каждого
//!   подключения есть сеть (`GetNetwork` → `GetNetworkId`, тот самый GUID,
//!   что хранится в `ServiceProfile::office_networks`) и адаптер
//!   (`GetAdapterId`, GUID адаптера);
//! - `GetAdaptersAddresses` (IP Helper) — по GUID адаптера отдаёт его
//!   «дружественное имя» (`FriendlyName`), то самое, что видно в панели
//!   управления и что понимает `netsh ... name=`.
//!
//! `find_office_adapter` — чистая функция сопоставления, полностью
//! покрытая тестами на фикстурах. Сбор данных с живой машины
//! (`gather_from_nlm`) — тонкая обёртка над COM/IP Helper без собственной
//! логики выбора; она читает сеть, ничего не меняя, поэтому проверяется
//! только смоук-тестом на этой машине, тем же приёмом, что и
//! `winnet::networks::list_connected`.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use proxypilot_core::net::mask_of;
use proxypilot_winnet::networks::format_guid;
use windows::core::Error as WinError;
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GET_ADAPTERS_ADDRESSES_FLAGS, IP_ADAPTER_ADDRESSES_LH,
    IP_ADAPTER_DHCP_ENABLED,
};
use windows::Win32::Networking::NetworkListManager::{
    INetworkConnection, INetworkListManager, NetworkListManager,
};
use windows::Win32::Networking::WinSock::{SOCKADDR_IN, SOCKET_ADDRESS};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("ошибка Windows: {0}")]
    Windows(#[from] WinError),
    #[error("GetAdaptersAddresses отказала с кодом {0}")]
    GetAdaptersAddresses(u32),
}

/// Один адаптер, каким его видит связка NLM + IP Helper: сеть, к которой
/// он подключён, и «дружественное» имя подключения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NlmAdapter {
    /// GUID сети NLM в канонической форме — сравнивается с
    /// `ServiceProfile::office_networks[i].id`.
    pub network_id: String,
    /// То, что ожидает `netsh interface ipv4 ... name=`.
    pub friendly_name: String,
}

/// Находит псевдоним адаптера, подключённого к сети с данным GUID.
/// Сравнение регистронезависимое — GUID из NLM и из `profile.toml` могут
/// отличаться регистром (тот же довод, что и `Config::place_for` в core, и
/// `tunnel_state::same_alias` в `winnet`).
pub fn find_office_adapter<'a>(
    adapters: &'a [NlmAdapter],
    office_network_id: &str,
) -> Option<&'a str> {
    adapters
        .iter()
        .find(|a| a.network_id.eq_ignore_ascii_case(office_network_id))
        .map(|a| a.friendly_name.as_str())
}

/// IPv4-состояние одного адаптера, как его сейчас видит `GetAdaptersAddresses`
/// — сырые данные для `proxypilot_core::netprofile::AdapterConfig`. Не сама
/// эта структура: `set_by_us` в неё не входит, потому что источник этого
/// признака не адаптер, а собственная память службы (`state::set_by_us`) —
/// см. докблок `state.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CurrentIpv4Config {
    pub dhcp: bool,
    pub addr: Option<Ipv4Addr>,
    pub mask: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
}

/// Вызывающий обязан держать живым `ComGuard` (`proxypilot_winnet::com`) —
/// тот же договор, что и у `winnet::networks::list_connected`, чей приёмник
/// COM-объектов используется тем же образом здесь.
///
/// SAFETY-обоснование живёт у каждого вызова ниже отдельно: это функция с
/// несколькими независимыми небезопасными операциями (COM, сырой Win32 API
/// с буфером переменного размера), и общий блок SAFETY на всю функцию
/// затемнил бы, какое именно свойство гарантирует каждый конкретный вызов.
pub fn gather_from_nlm() -> Result<Vec<NlmAdapter>, AdapterError> {
    let friendly_names = adapter_friendly_names_by_guid()?;

    // SAFETY: COM инициализирован вызывающим (см. докблок функции);
    // `NetworkListManager` — предопределённый CLSID, соответствующий
    // интерфейсу `INetworkListManager`.
    let manager: INetworkListManager =
        unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)? };
    // SAFETY: `manager` — валидный, только что созданный указатель.
    let enumerator = unsafe { manager.GetNetworkConnections()? };

    let mut out = Vec::new();
    loop {
        let mut item = [None::<INetworkConnection>; 1];
        let mut fetched = 0u32;
        // SAFETY: `item`/`fetched` — живые локальные переменные нужного
        // размера; тот же классический OLE-энумератор, что и в
        // `winnet::networks::list_connected`.
        unsafe { enumerator.Next(&mut item, Some(&mut fetched))? };
        if fetched == 0 {
            break;
        }
        let Some(conn) = item[0].take() else { break };

        // SAFETY: `conn` — живой указатель, только что полученный от
        // энумератора.
        let network = unsafe { conn.GetNetwork()? };
        // SAFETY: `network` — живой указатель, полученный строкой выше.
        let network_id = format_guid(&unsafe { network.GetNetworkId()? });
        // SAFETY: `conn` — тот же живой указатель.
        let adapter_guid = format_guid(&unsafe { conn.GetAdapterId()? });

        if let Some(friendly_name) = friendly_names.get(&adapter_guid.to_uppercase()) {
            out.push(NlmAdapter {
                network_id,
                friendly_name: friendly_name.clone(),
            });
        }
    }
    Ok(out)
}

/// Текущее IPv4-состояние адаптера по его дружественному имени
/// (`FriendlyName`, то же, что несёт `NlmAdapter::friendly_name` и что
/// ожидает `netsh ... name=`). `Ok(None)` — адаптера с таким именем сейчас
/// нет в `GetAdaptersAddresses`: не ошибка, а законный исход (адаптер
/// отключили физически между опознанием сети и применением статики).
pub fn current_ipv4_config(friendly_name: &str) -> Result<Option<CurrentIpv4Config>, AdapterError> {
    for_each_adapter(|adapter| {
        // SAFETY: `adapter.FriendlyName` — живой PWSTR внутри буфера,
        // которым владеет вызывающий (`for_each_adapter`); `to_string()`
        // копирует данные в собственную `String`, ничего не удерживая после
        // возврата.
        let name = unsafe { adapter.FriendlyName.to_string() }.unwrap_or_default();
        if name != friendly_name {
            return None;
        }
        // SAFETY: `Anonymous2` — union из одного `u32`-поля (`Flags`) во
        // всех вариантах; читать его как `Flags` всегда корректно
        // независимо от того, каким вариантом его в последний раз писал
        // API — представление битов одно и то же.
        let flags = unsafe { adapter.Anonymous2.Flags };
        let dhcp = flags & IP_ADAPTER_DHCP_ENABLED != 0;

        let mut addr = None;
        let mut mask = None;
        // SAFETY: `FirstUnicastAddress` — либо `null` (пустой список), либо
        // указывает в тот же буфер `GetAdaptersAddresses`, что и `adapter`
        // сам, живой на всё время замыкания `for_each_adapter`.
        let mut cur = adapter.FirstUnicastAddress;
        while !cur.is_null() {
            let unicast = unsafe { &*cur };
            if let Some(ip) = sockaddr_to_ipv4(&unicast.Address) {
                addr = Some(ip);
                mask = Some(mask_of(unicast.OnLinkPrefixLength));
                break;
            }
            cur = unicast.Next;
        }

        let mut dns = Vec::new();
        // SAFETY: тот же приём, что и для `FirstUnicastAddress` выше.
        let mut cur = adapter.FirstDnsServerAddress;
        while !cur.is_null() {
            let entry = unsafe { &*cur };
            if let Some(ip) = sockaddr_to_ipv4(&entry.Address) {
                dns.push(ip);
            }
            cur = entry.Next;
        }

        Some(CurrentIpv4Config {
            dhcp,
            addr,
            mask,
            dns,
        })
    })
}

/// `SOCKET_ADDRESS` → `Ipv4Addr`, если это вообще IPv4 (`AF_INET`).
/// `GetAdaptersAddresses` кладёт в `FirstUnicastAddress`/
/// `FirstDnsServerAddress` вперемешку IPv4- и IPv6-адреса; здесь нас
/// интересует только IPv4 (задача 5/6 целиком про IPv4, IPv6 вне области).
fn sockaddr_to_ipv4(addr: &SOCKET_ADDRESS) -> Option<Ipv4Addr> {
    if addr.lpSockaddr.is_null() {
        return None;
    }
    // SAFETY: `lpSockaddr` указывает в буфер `GetAdaptersAddresses`, живой
    // на весь вызов; читаем только `sa_family`, общее поле обоих вариантов
    // (`SOCKADDR`/`SOCKADDR_IN`) с одинаковым смещением.
    let family = unsafe { (*addr.lpSockaddr).sa_family };
    if family != windows::Win32::Networking::WinSock::AF_INET {
        return None;
    }
    // SAFETY: `sa_family == AF_INET` подтверждает по документации Winsock,
    // что за указателем лежит именно `SOCKADDR_IN`, а не общий `SOCKADDR`.
    let sin = unsafe { &*(addr.lpSockaddr as *const SOCKADDR_IN) };
    // SAFETY: `S_un` — union из вариантов одного размера (4 байта); чтение
    // байтового варианта корректно независимо от того, каким вариантом его
    // заполнил API — представление одно и то же.
    let bytes = unsafe { sin.sin_addr.S_un.S_un_b };
    Some(Ipv4Addr::new(
        bytes.s_b1, bytes.s_b2, bytes.s_b3, bytes.s_b4,
    ))
}

/// Собирает `AdapterName` (GUID адаптера, ASCII, как отдаёт IP Helper) →
/// `FriendlyName` для всех адаптеров машины — join-таблица для
/// `gather_from_nlm`. Ключ приводится к верхнему регистру: `format_guid`
/// печатает GUID именно в верхнем, а регистр ASCII-строки `AdapterName` не
/// документирован и не обязан совпадать.
fn adapter_friendly_names_by_guid() -> Result<HashMap<String, String>, AdapterError> {
    let mut map = HashMap::new();
    for_each_adapter_ref(|adapter| {
        // SAFETY: `AdapterName` — живой PSTR (ANSI) внутри буфера
        // `GetAdaptersAddresses`, действителен до конца замыкания.
        let name = unsafe { adapter.AdapterName.to_string() }.unwrap_or_default();
        // SAFETY: см. докблок функции про `FriendlyName`.
        let friendly = unsafe { adapter.FriendlyName.to_string() }.unwrap_or_default();
        map.insert(name.to_uppercase(), friendly);
    })?;
    Ok(map)
}

/// Общий каркас чтения `GetAdaptersAddresses`: растущий буфер (Win32
/// `ERROR_BUFFER_OVERFLOW` — сигнал «дай буфер больше», а не отказ) и обход
/// связного списка адаптеров. Обе функции выше (`current_ipv4_config`,
/// `adapter_friendly_names_by_guid`) — просто разные тела для одного и того
/// же каркаса, чтобы сам каркас (и его SAFETY) не расходился в двух копиях.
fn for_each_adapter<T>(
    mut visit: impl FnMut(&IP_ADAPTER_ADDRESSES_LH) -> Option<T>,
) -> Result<Option<T>, AdapterError> {
    let mut result = None;
    for_each_adapter_ref(|adapter| {
        if result.is_none() {
            result = visit(adapter);
        }
    })?;
    Ok(result)
}

fn for_each_adapter_ref(
    mut visit: impl FnMut(&IP_ADAPTER_ADDRESSES_LH),
) -> Result<(), AdapterError> {
    // 15 КиБ — типичный стартовый размер из документации Microsoft для
    // `GetAdaptersAddresses`: хватает почти всегда с первого раза, а цикл
    // ниже всё равно корректно досчитывает буфер под настоящий размер
    // машины, если адаптеров окажется больше.
    let mut size: u32 = 15 * 1024;
    let mut buf: Vec<u8>;
    loop {
        buf = vec![0u8; size as usize];
        // SAFETY: `buf` — живой, только что выделенный буфер размера
        // `size`; `family = AF_UNSPEC (0)` просит и IPv4, и IPv6 (нужен
        // только IPv4, но фильтрация — дело вызывающего замыкания, не
        // самого чтения); `size` передаётся и как вход (размер буфера), и
        // как выход (сколько байт нужно, если буфер мал).
        let rc = unsafe {
            GetAdaptersAddresses(
                0,
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
            // `size` уже переписан настоящим нужным значением — следующая
            // итерация выделяет буфер точного размера.
            continue;
        }
        return Err(AdapterError::GetAdaptersAddresses(rc));
    }

    // SAFETY: буфер `buf` только что успешно заполнен `GetAdaptersAddresses`
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

    fn adapter(network_id: &str, friendly_name: &str) -> NlmAdapter {
        NlmAdapter {
            network_id: network_id.to_string(),
            friendly_name: friendly_name.to_string(),
        }
    }

    #[test]
    fn finds_the_adapter_connected_to_the_office_network() {
        let adapters = vec![
            adapter("{BBBB0000-0000-0000-0000-000000000000}", "Домашний Wi-Fi"),
            adapter("{AAAA0000-0000-0000-0000-000000000001}", "Ethernet 2"),
        ];
        let got = find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}");
        assert_eq!(got, Some("Ethernet 2"));
    }

    #[test]
    fn no_matching_network_is_none() {
        let adapters = vec![adapter("{BBBB0000-0000-0000-0000-000000000000}", "Wi-Fi")];
        assert_eq!(
            find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}"),
            None
        );
    }

    #[test]
    fn empty_adapter_list_is_none() {
        assert_eq!(
            find_office_adapter(&[], "{AAAA0000-0000-0000-0000-000000000001}"),
            None
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        // GUID из NLM и из конфига могут отличаться регистром — тот же
        // довод, что и `Config::matching_is_case_insensitive` в core.
        let adapters = vec![adapter(
            "{aaaa0000-0000-0000-0000-000000000001}",
            "Ethernet 2",
        )];
        assert_eq!(
            find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}"),
            Some("Ethernet 2")
        );
    }

    #[test]
    fn a_docking_station_second_nic_does_not_confuse_the_match() {
        // Ровно тот сценарий, ради которого адаптер берётся из NLM, а не по
        // имени: несколько адаптеров подключены одновременно (докблок
        // модуля), верно выбирается тот, что несёт офисную сеть.
        let adapters = vec![
            adapter("{CCCC0000-0000-0000-0000-000000000000}", "Гостевая"),
            adapter("{AAAA0000-0000-0000-0000-000000000001}", "USB-C Ethernet"),
            adapter("{DDDD0000-0000-0000-0000-000000000000}", "Wi-Fi"),
        ];
        assert_eq!(
            find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}"),
            Some("USB-C Ethernet")
        );
    }

    // ---- Смоук-тесты на живой машине ----
    //
    // Читают сеть, ничего не меняя — тот же приём и то же обоснование, что
    // и `winnet::networks::listing_connected_networks_does_not_fail_on_a_real_machine`:
    // список может быть пустым (нет офисной сети среди подключённых прямо
    // сейчас — обычное дело вне офиса), это не отказ; отказ — `Err`.

    #[test]
    fn gathering_from_nlm_does_not_fail_on_a_real_machine() {
        let _guard = proxypilot_winnet::com::ComGuard::new().expect("COM должен подняться");
        let adapters = gather_from_nlm().expect("сбор не должен падать");
        for a in &adapters {
            assert!(
                a.network_id.starts_with('{'),
                "GUID сети обязан быть в фигурных скобках: {}",
                a.network_id
            );
            assert!(
                !a.friendly_name.is_empty(),
                "дружественное имя адаптера не должно быть пустым, если адаптер вообще нашёлся"
            );
        }
    }

    #[test]
    fn reading_current_ipv4_config_of_a_nonexistent_adapter_is_none_not_an_error() {
        let got = current_ipv4_config("совершенно точно нет такого адаптера на этой машине")
            .expect("чтение не должно падать даже без совпадения");
        assert_eq!(got, None);
    }

    #[test]
    fn reading_current_ipv4_config_does_not_fail_for_a_real_connected_adapter() {
        // Берём реальный адаптер этой машины через тот же путь, что и
        // `gather_from_nlm` (IP Helper напрямую, без NLM) — если сеть вообще
        // подключена, у машины обязан быть хотя бы один адаптер с DHCP-флагом
        // или статикой; если нет вовсе — тест пропускает себя, а не падает.
        let mut any_named = None;
        for_each_adapter_ref(|a| {
            if any_named.is_none() {
                // SAFETY: `FriendlyName` — живой PWSTR внутри буфера,
                // действителен на всё время этого замыкания.
                if let Ok(name) = unsafe { a.FriendlyName.to_string() } {
                    if !name.is_empty() {
                        any_named = Some(name);
                    }
                }
            }
        })
        .expect("перечисление адаптеров не должно падать");

        let Some(name) = any_named else {
            eprintln!("на машине не нашлось ни одного именованного адаптера — тест пропущен");
            return;
        };
        let got = current_ipv4_config(&name).expect("чтение не должно падать");
        assert!(
            got.is_some(),
            "адаптер с найденным именем обязан прочитаться"
        );
    }
}
