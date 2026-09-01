//! Сопоставление GUID офисной сети (NLM) с адаптером, а также чтение
//! текущего IPv4-состояния адаптера и его «дружественного» имени — того,
//! что ожидает `netsh interface ipv4 ... name=`.
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
//! - `GetAdaptersAddresses` (IP Helper) — по GUID адаптера (`AdapterName`)
//!   отдаёт его текущее IPv4-состояние и «дружественное имя»
//!   (`FriendlyName`), то самое, что видно в панели управления и что
//!   понимает `netsh ... name=`.
//!
//! **Идентичность адаптера — GUID, не `FriendlyName`.** Ревью round 2
//! (задача 6) нашло дыру: раньше `NlmAdapter`/`AppliedState` несли именно
//! дружественное имя, а оно, как задача 3 уже установила для алиасов
//! интерфейсов, переименовываемо человеком в любой момент — в том числе в
//! окне между применением статики и попыткой её откатить. GUID адаптера
//! Windows не меняет никогда; `find_office_adapter` и
//! `state::AppliedState::iface_guid` несут именно его, а `FriendlyName`
//! резолвится заново, непосредственно перед тем, как понадобится для
//! самой команды `netsh` (`friendly_name_for_guid`).
//!
//! `find_office_adapter` — чистая функция сопоставления, полностью
//! покрытая тестами на фикстурах. Сбор данных с живой машины
//! (`gather_from_nlm`, `current_ipv4_config`, `friendly_name_for_guid`) —
//! тонкие обёртки над COM/IP Helper без собственной логики выбора; они
//! читают сеть, ничего не меняя, поэтому проверяются только смоук-тестами
//! на этой машине, тем же приёмом, что и
//! `winnet::networks::list_connected`.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use proxypilot_core::net::mask_of;
use proxypilot_winnet::networks::format_guid;
use windows::core::Error as WinError;
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GET_ADAPTERS_ADDRESSES_FLAGS, IP_ADAPTER_ADDRESSES_LH,
    IP_ADAPTER_DHCP_ENABLED,
};
use windows::Win32::Networking::NetworkListManager::{
    INetworkConnection, INetworkListManager, NetworkListManager,
};
use windows::Win32::Networking::WinSock::{SOCKADDR_IN, SOCKET_ADDRESS};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

/// Сколько раз повторить чтение `GetAdaptersAddresses` при
/// `ERROR_BUFFER_OVERFLOW`, прежде чем считать это отказом. Документация
/// Microsoft предупреждает, что теоретически возможна гонка (адаптер
/// появляется между попытками, и запрошенного размера снова не хватает) и
/// рекомендует несколько попыток, а не бесконечный цикл — без предела
/// патологический случай на живой машине превратил бы чтение сети в
/// зависание потока, крутящего единственный цикл службы.
const MAX_GET_ADAPTERS_ATTEMPTS: u32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("ошибка Windows: {0}")]
    Windows(#[from] WinError),
    #[error("GetAdaptersAddresses отказала с кодом {0}")]
    GetAdaptersAddresses(u32),
}

/// Один адаптер, каким его видит связка NLM + IP Helper: сеть, к которой
/// он подключён, GUID самого адаптера и его текущее дружественное имя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NlmAdapter {
    /// GUID сети NLM в канонической форме — сравнивается с
    /// `ServiceProfile::office_networks[i].id`.
    pub network_id: String,
    /// GUID адаптера (`AdapterName` в терминах IP Helper) — устойчивый
    /// идентификатор, не меняющийся при переименовании подключения.
    pub adapter_guid: String,
    /// То, что ожидает `netsh interface ipv4 ... name=` ПРЯМО СЕЙЧАС — не
    /// хранится нигде дольше одного цикла (докблок модуля).
    pub friendly_name: String,
}

/// Находит GUID адаптера, подключённого к сети с данным GUID. Сравнение
/// регистронезависимое — GUID из NLM и из `profile.toml` могут отличаться
/// регистром (тот же довод, что и `Config::place_for` в core, и
/// `tunnel_state::same_alias` в `winnet`).
pub fn find_office_adapter<'a>(
    adapters: &'a [NlmAdapter],
    office_network_id: &str,
) -> Option<&'a str> {
    adapters
        .iter()
        .find(|a| a.network_id.eq_ignore_ascii_case(office_network_id))
        .map(|a| a.adapter_guid.as_str())
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
                adapter_guid,
                friendly_name: friendly_name.clone(),
            });
        }
    }
    Ok(out)
}

/// Текущее дружественное имя адаптера по его GUID — то, что нужно ПРЯМО
/// СЕЙЧАС для команды `netsh ... name=` (докблок модуля: имя не хранится
/// между циклами, только резолвится заново). `Ok(None)` — адаптера с таким
/// GUID сейчас нет: не ошибка (адаптер мог физически исчезнуть), а
/// законный исход, который обязан обработать вызывающий.
pub fn friendly_name_for_guid(guid: &str) -> Result<Option<String>, AdapterError> {
    let guid_upper = guid.to_uppercase();
    for_each_adapter(|adapter| {
        // SAFETY: `AdapterName` — живой PSTR (ANSI) внутри буфера,
        // которым владеет `for_each_adapter`, действителен на всё время
        // этого замыкания.
        let name = unsafe { adapter.AdapterName.to_string() }.unwrap_or_default();
        if name.to_uppercase() != guid_upper {
            return None;
        }
        // SAFETY: `FriendlyName` — тот же буфер, тот же довод.
        Some(unsafe { adapter.FriendlyName.to_string() }.unwrap_or_default())
    })
}

/// Текущее IPv4-состояние адаптера по его GUID (`AdapterName` в терминах
/// IP Helper) — устойчивому идентификатору, не дружественному имени
/// (докблок модуля). `Ok(None)` — адаптера с таким GUID сейчас нет в
/// `GetAdaptersAddresses`: не ошибка, а законный исход (адаптер отключили
/// физически между опознанием сети и применением статики).
pub fn current_ipv4_config(guid: &str) -> Result<Option<CurrentIpv4Config>, AdapterError> {
    let guid_upper = guid.to_uppercase();
    for_each_adapter(|adapter| {
        // SAFETY: `adapter.AdapterName` — живой PSTR внутри буфера,
        // которым владеет вызывающий (`for_each_adapter`); `to_string()`
        // копирует данные в собственную `String`, ничего не удерживая после
        // возврата.
        let name = unsafe { adapter.AdapterName.to_string() }.unwrap_or_default();
        if name.to_uppercase() != guid_upper {
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
/// связного списка адаптеров. Функции выше (`current_ipv4_config`,
/// `friendly_name_for_guid`, `adapter_friendly_names_by_guid`) — просто
/// разные тела для одного и того же каркаса, чтобы сам каркас (и его
/// SAFETY) не расходился в трёх копиях.
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
    // машины, если адаптеров окажется больше (в пределах
    // `MAX_GET_ADAPTERS_ATTEMPTS`).
    let mut byte_capacity: u32 = 15 * 1024;
    // `Vec<u64>`, а не `Vec<u8>` — ревью round 2 нашло здесь неопределённое
    // поведение по букве стандарта: `IP_ADAPTER_ADDRESSES_LH` несёt
    // указательные и `u64`-поля и требует выравнивания 8 байт, а `Vec<u8>`
    // гарантирует только выравнивание 1. Раньше это «работало» только
    // потому, что аллокатор Windows на практике отдаёт блоки такого
    // размера уже выровненными — везение реализации, не гарантия языка.
    // `Vec<u64>` даёт выравнивание 8 честно, самим типом элемента.
    let mut buf: Vec<u64>;
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let words = (byte_capacity as usize).div_ceil(8);
        buf = vec![0u64; words];
        let mut size_arg = (words * 8) as u32;
        // SAFETY: `buf` — живой, только что выделенный буфер размера
        // `size_arg` байт, выровненный на 8 (тип элемента `u64`);
        // `family = 0` (`AF_UNSPEC`) просит и IPv4, и IPv6 (нужен только
        // IPv4, но фильтрация — дело вызывающего замыкания, не самого
        // чтения); `size_arg` передаётся и как вход (размер буфера), и как
        // выход (сколько байт нужно, если буфер мал).
        let rc = unsafe {
            GetAdaptersAddresses(
                0,
                GET_ADAPTERS_ADDRESSES_FLAGS(0),
                None,
                Some(buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &mut size_arg,
            )
        };
        if rc == ERROR_SUCCESS.0 {
            break;
        }
        if rc == ERROR_NO_DATA.0 {
            // Ни одного адаптера в системе вообще — законный исход (машина
            // без единой сетевой карты), не отказ. Обходить нечего.
            return Ok(());
        }
        if rc == ERROR_BUFFER_OVERFLOW.0 && attempts < MAX_GET_ADAPTERS_ATTEMPTS {
            // `size_arg` уже переписан настоящим нужным значением —
            // следующая итерация выделяет буфер точного размера.
            byte_capacity = size_arg;
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

    fn adapter(network_id: &str, adapter_guid: &str, friendly_name: &str) -> NlmAdapter {
        NlmAdapter {
            network_id: network_id.to_string(),
            adapter_guid: adapter_guid.to_string(),
            friendly_name: friendly_name.to_string(),
        }
    }

    #[test]
    fn finds_the_adapter_connected_to_the_office_network() {
        let adapters = vec![
            adapter(
                "{BBBB0000-0000-0000-0000-000000000000}",
                "{EEEE0000-0000-0000-0000-000000000000}",
                "Домашний Wi-Fi",
            ),
            adapter(
                "{AAAA0000-0000-0000-0000-000000000001}",
                "{FFFF0000-0000-0000-0000-000000000001}",
                "Ethernet 2",
            ),
        ];
        let got = find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}");
        assert_eq!(got, Some("{FFFF0000-0000-0000-0000-000000000001}"));
    }

    #[test]
    fn no_matching_network_is_none() {
        let adapters = vec![adapter(
            "{BBBB0000-0000-0000-0000-000000000000}",
            "{EEEE0000-0000-0000-0000-000000000000}",
            "Wi-Fi",
        )];
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
            "{FFFF0000-0000-0000-0000-000000000001}",
            "Ethernet 2",
        )];
        assert_eq!(
            find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}"),
            Some("{FFFF0000-0000-0000-0000-000000000001}")
        );
    }

    #[test]
    fn a_docking_station_second_nic_does_not_confuse_the_match() {
        // Ровно тот сценарий, ради которого адаптер берётся из NLM, а не по
        // имени: несколько адаптеров подключены одновременно (докблок
        // модуля), верно выбирается тот, что несёт офисную сеть.
        let adapters = vec![
            adapter(
                "{CCCC0000-0000-0000-0000-000000000000}",
                "{1111-guest}",
                "Гостевая",
            ),
            adapter(
                "{AAAA0000-0000-0000-0000-000000000001}",
                "{FFFF0000-0000-0000-0000-000000000001}",
                "USB-C Ethernet",
            ),
            adapter(
                "{DDDD0000-0000-0000-0000-000000000000}",
                "{2222-wifi}",
                "Wi-Fi",
            ),
        ];
        assert_eq!(
            find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}"),
            Some("{FFFF0000-0000-0000-0000-000000000001}")
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
        let got = current_ipv4_config("{00000000-0000-0000-0000-000000000000}")
            .expect("чтение не должно падать даже без совпадения");
        assert_eq!(got, None);
    }

    #[test]
    fn resolving_friendly_name_of_a_nonexistent_adapter_is_none_not_an_error() {
        let got = friendly_name_for_guid("{00000000-0000-0000-0000-000000000000}")
            .expect("чтение не должно падать даже без совпадения");
        assert_eq!(got, None);
    }

    /// Находит GUID любого реального адаптера этой машины напрямую через IP
    /// Helper (без NLM) — общий помощник для двух смоук-тестов ниже.
    fn any_real_adapter_guid() -> Option<String> {
        let mut found = None;
        for_each_adapter_ref(|a| {
            if found.is_none() {
                // SAFETY: `AdapterName` — живой PSTR внутри буфера,
                // действителен на всё время этого замыкания.
                if let Ok(name) = unsafe { a.AdapterName.to_string() } {
                    if !name.is_empty() {
                        found = Some(name);
                    }
                }
            }
        })
        .expect("перечисление адаптеров не должно падать");
        found
    }

    #[test]
    fn reading_current_ipv4_config_does_not_fail_for_a_real_connected_adapter() {
        // Если сеть вообще подключена, у машины обязан быть хотя бы один
        // адаптер; если нет вовсе — тест пропускает себя, а не падает.
        let Some(guid) = any_real_adapter_guid() else {
            eprintln!("на машине не нашлось ни одного адаптера — тест пропущен");
            return;
        };
        let got = current_ipv4_config(&guid).expect("чтение не должно падать");
        assert!(got.is_some(), "адаптер с найденным GUID обязан прочитаться");
    }

    #[test]
    fn resolving_friendly_name_does_not_fail_for_a_real_connected_adapter() {
        let Some(guid) = any_real_adapter_guid() else {
            eprintln!("на машине не нашлось ни одного адаптера — тест пропущен");
            return;
        };
        let got = friendly_name_for_guid(&guid).expect("чтение не должно падать");
        assert!(
            got.is_some(),
            "адаптер с найденным GUID обязан резолвиться в имя"
        );
    }
}
