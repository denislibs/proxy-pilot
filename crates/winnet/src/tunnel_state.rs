//! Распознавание чужого туннеля по таблице маршрутов.
//!
//! Критерий не «есть ли туннельный адаптер», а «лежит ли уже маршрут в
//! наши офисные сети через чужой туннель». У пользователей Tailscale или
//! WireGuard туннельный адаптер поднят постоянно; наивная проверка «есть
//! tun — значит занято» не дала бы поднять офисный туннель никогда и
//! убила бы функцию у всех, кто пользуется хоть каким-то VPN. Логика
//! перенесена из macOS-версии дословно — там она такая по той же причине.
//!
//! Функции чистые: таблицу маршрутов и список адаптеров собирает
//! вызывающий (winnet не делает здесь никакого ввода-вывода), что и
//! позволяет проверить редкие сочетания (широкий маршрут поверх узкой
//! офисной сети и наоборот) на фикстурах, не имея их под рукой на этой
//! машине.
//!
//! Унаследованное ограничение (форма `AdapterRoute` задана брифом этой
//! задачи, менять её — не решение этого модуля): `interface_alias` —
//! это псевдоним сетевого интерфейса Windows, свободная строка,
//! которую пользователь может переименовать когда угодно, а разные
//! средства (`route print`, `Get-NetRoute`, `netsh`) ещё и показывают
//! её по-разному. Это не устойчивый идентификатор — переименование
//! адаптера, пока наш туннель поднят, снаружи выглядит так же, как
//! появление чужого туннеля с тем же маршрутом. Устойчивым был бы LUID
//! или индекс интерфейса. Задачи 4 и 7 наследуют это ограничение вместе
//! с типом и должны о нём знать, а не удивляться ему на живой машине.

use proxypilot_core::net::{mask_of, Ipv4Net};

/// Один маршрут из таблицы Windows вместе с адаптером, через который он
/// идёт. `is_tunnel` — платформенный факт (TAP/TUN/WireGuard-подобный
/// адаптер), который вычисляет вызывающий; здесь он уже дан как данное.
#[derive(Debug)]
pub struct AdapterRoute {
    pub dest: Ipv4Net,
    pub interface_alias: String,
    pub is_tunnel: bool,
}

/// Сравнение псевдонимов интерфейса без учёта регистра и обрамляющих
/// пробелов. Windows не гарантирует, каким регистром вернёт alias тот
/// или иной инструмент (`route print` и `Get-NetRoute` расходятся), и
/// байт-в-байт сравнение превращает это расхождение в ложный «чужой
/// туннель» на нашем же адаптере — то есть в дедлок: подъём и останов
/// заблокированы, потому что наш туннель считается чужим. Через
/// `to_lowercase()`, а не `eq_ignore_ascii_case`, — сам псевдоним не
/// обязан быть ASCII.
///
/// Пустой `our_alias` (`""`) намеренно не считается совпадением ни с
/// каким реальным именем адаптера: непустой алиас никогда не сравнится
/// равным пустой строке, поэтому вызывающий с ещё не настроенным алиасом
/// получит «наш туннель не поднят» и «есть чужой» — отказ в
/// консервативную сторону (не занизит риск), а не наоборот.
fn same_alias(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Границы подсети как пара адресов `[start, end]`. Использует `mask_of`
/// из `core`, а не сдвиг напрямую, — там уже решён случай `/0` (сдвиг на
/// 32 паникует в debug-сборке). Маскирует ещё раз при вычислении `start`,
/// хотя `Ipv4Net::from_str` уже маскирует хостовые биты: поля `Ipv4Net`
/// публичны, и адрес мог быть собран напрямую (`Ipv4Net { addr, prefix }`)
/// в обход разбора — тот же приём двойной защиты, что и при выводе
/// `route` в `ovpn_profile.rs`.
fn range(net: &Ipv4Net) -> (u32, u32) {
    let mask = u32::from(mask_of(net.prefix));
    let start = u32::from(net.addr) & mask;
    let end = start | !mask;
    (start, end)
}

/// «Несёт» ли один маршрут другой — пересекаются ли их диапазоны адресов
/// в любую сторону: `a` шире `b` (Tailscale-подобный `100.64.0.0/10` над
/// нашей `/24`), `a` уже `b` и целиком лежит внутри, или оба совпадают
/// точно. Для двух настоящих (выровненных) CIDR-блоков третьего варианта
/// — частичного, не-вложенного пересечения — не существует: блоки либо
/// вложены один в другой, либо не пересекаются. Формула по границам
/// корректна и для этого случая, и для не выровненных значений,
/// собранных мимо `FromStr` — специальный случай не нужен.
fn overlaps(a: &Ipv4Net, b: &Ipv4Net) -> bool {
    let (a_start, a_end) = range(a);
    let (b_start, b_end) = range(b);
    a_start <= b_end && b_start <= a_end
}

/// Наш собственный туннель уже поднят: среди адаптеров есть туннельный с
/// нашим псевдонимом интерфейса. Маршруты роли не играют — здесь важен
/// сам факт «адаптер существует и это туннель», а не то, что через него
/// идёт (этим занимается `foreign_tunnel_up`).
pub fn our_tunnel_up(adapters: &[AdapterRoute], our_alias: &str) -> bool {
    adapters
        .iter()
        .any(|a| a.is_tunnel && same_alias(&a.interface_alias, our_alias))
}

/// Чужой туннель уже несёт наши офисные сети: среди адаптеров есть
/// туннельный (`is_tunnel`), чей псевдоним НЕ совпадает с нашим, и чей
/// маршрут (`dest`) пересекается хотя бы с одной из подсетей `routes`.
///
/// Отклонение от сигнатуры плана: добавлен параметр `our_alias`. Без
/// него функция не смогла бы отличить наш собственный (уже поднятый)
/// туннель от чужого, а «наш туннель не считается чужим» — прямое
/// требование приёмки; переносить это решение на вызывающего значило бы
/// не проверять его тестами этого модуля. Сигнатура остаётся чистой
/// функцией: вызывающий (задачи 4 и 7) и так знает `our_alias` — он же
/// передаётся в `our_tunnel_up`.
pub fn foreign_tunnel_up(routes: &[Ipv4Net], adapters: &[AdapterRoute], our_alias: &str) -> bool {
    adapters.iter().any(|a| {
        a.is_tunnel
            && !same_alias(&a.interface_alias, our_alias)
            && routes.iter().any(|r| overlaps(r, &a.dest))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::str::FromStr;

    fn net(s: &str) -> Ipv4Net {
        Ipv4Net::from_str(s).expect("валидная подсеть в тесте")
    }

    #[test]
    fn permanently_up_tailscale_is_not_foreign_for_office_10_x() {
        let adapters = [AdapterRoute {
            dest: net("100.64.0.0/10"),
            interface_alias: "Tailscale".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.0.0.0/8")];
        assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn tunnel_carrying_office_route_is_foreign() {
        let adapters = [AdapterRoute {
            dest: net("10.5.0.0/16"),
            interface_alias: "SomeoneElsesVPN".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn our_own_tunnel_is_not_foreign() {
        let adapters = [AdapterRoute {
            dest: net("10.5.0.0/16"),
            interface_alias: "OfficeVPN".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn office_route_through_non_tunnel_adapter_is_not_foreign() {
        let adapters = [AdapterRoute {
            dest: net("10.5.0.0/16"),
            interface_alias: "Ethernet".to_string(),
            is_tunnel: false,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn empty_routing_table_is_not_foreign() {
        assert!(!foreign_tunnel_up(&[], &[], "OfficeVPN"));
    }

    #[test]
    fn broader_foreign_route_covering_a_narrower_office_subnet_is_foreign() {
        let adapters = [AdapterRoute {
            dest: net("10.0.0.0/8"),
            interface_alias: "SomeoneElsesVPN".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn narrower_foreign_route_inside_a_broader_office_subnet_is_foreign() {
        let adapters = [AdapterRoute {
            dest: net("10.5.1.0/24"),
            interface_alias: "SomeoneElsesVPN".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn disjoint_foreign_tunnel_route_is_not_foreign() {
        let adapters = [AdapterRoute {
            dest: net("192.168.1.0/24"),
            interface_alias: "SomeoneElsesVPN".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn our_tunnel_up_true_when_our_alias_is_a_tunnel() {
        let adapters = [AdapterRoute {
            dest: net("10.5.0.0/16"),
            interface_alias: "OfficeVPN".to_string(),
            is_tunnel: true,
        }];
        assert!(our_tunnel_up(&adapters, "OfficeVPN"));
    }

    #[test]
    fn our_tunnel_up_false_when_alias_matches_but_not_a_tunnel() {
        let adapters = [AdapterRoute {
            dest: net("10.5.0.0/16"),
            interface_alias: "OfficeVPN".to_string(),
            is_tunnel: false,
        }];
        assert!(!our_tunnel_up(&adapters, "OfficeVPN"));
    }

    #[test]
    fn our_tunnel_up_false_on_empty_adapters() {
        assert!(!our_tunnel_up(&[], "OfficeVPN"));
    }

    #[test]
    fn a_full_tunnel_commercial_vpn_is_foreign() {
        // 0.0.0.0/0 — типичный маршрут коммерческого full-tunnel VPN
        // (NordVPN и подобные): он несёт вообще всё, включая наши
        // офисные сети, а не только их.
        let adapters = [AdapterRoute {
            dest: net("0.0.0.0/0"),
            interface_alias: "CommercialVPN".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn a_single_host_route_inside_the_office_subnet_is_foreign() {
        // /32 — маршрут на один хост, целиком внутри офисной /16.
        let adapters = [AdapterRoute {
            dest: net("10.5.0.7/32"),
            interface_alias: "SomeoneElsesVPN".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn a_host_bits_set_destination_built_past_from_str_still_matches() {
        // Поля Ipv4Net публичны — конструктор в обход FromStr не
        // маскирует хостовые биты. range() перемаскирует ещё раз при
        // вычислении границ, поэтому такой адрес всё равно matches.
        let unmasked_dest = Ipv4Net {
            addr: Ipv4Addr::new(10, 5, 0, 200),
            prefix: 16,
        };
        let adapters = [AdapterRoute {
            dest: unmasked_dest,
            interface_alias: "SomeoneElsesVPN".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }

    #[test]
    fn alias_comparison_is_case_insensitive_and_trims_whitespace() {
        // Находка ревью: байт-в-байт сравнение алиаса дало бы дедлок —
        // наш же туннель считался бы чужим только из-за регистра или
        // пробела, добавленного каким-то инструментом Windows.
        let adapters = [AdapterRoute {
            dest: net("10.5.0.0/16"),
            interface_alias: " officevpn ".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(our_tunnel_up(&adapters, "OfficeVPN"));
        assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
    }
}
