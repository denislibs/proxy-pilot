//! Занята ли уже офисная подсеть каким-нибудь туннельным адаптером.
//!
//! Критерий не «есть ли туннельный адаптер», а «лежит ли уже маршрут в
//! наши офисные сети через туннель». У пользователей Tailscale или
//! WireGuard туннельный адаптер поднят постоянно; наивная проверка «есть
//! tun — значит занято» не дала бы поднять офисный туннель никогда и
//! убила бы функцию у всех, кто пользуется хоть каким-то VPN. Логика
//! перенесена из macOS-версии дословно — там она такая по той же причине.
//!
//! # Alias здесь больше нет — история почему (round 1 и round 2, задача 7)
//!
//! До round 1 задачи 7 этот модуль ещё и различал «наш» туннель от
//! «чужого» по псевдониму сетевого адаптера (`interface_alias`,
//! параметр `our_alias`). Round 1 прочёл реальные адаптеры живой машины
//! (`Get-NetAdapter`, только чтение) и нашёл: OpenVPN называет адаптер по
//! ДРАЙВЕРУ («OpenVPN Wintun», «TAP-Windows Adapter V9»), никогда — по
//! имени профиля/соединения. Alias как признак «наш или чужой» был не
//! просто ненадёжным (это и так было задокументировано ниже и в задаче
//! 3) — он был структурно неверным допущением, гарантированно
//! проваливавшимся: свой же поднятый туннель читался как чужой, и
//! правило «не трогать чужой» запирало разом и подъём, и опускание.
//!
//! Round 1 подключил надёжный источник для «наш ли туннель поднят» —
//! `winnet::tunnel_log::liveness`, лог `openvpn-gui.exe`, ключуемый именем
//! профиля (тем, чем вызывающий код действительно владеет), — но оставил
//! alias работать в этом модуле для «чужой» половины вопроса. Round 2
//! заметил: раз имя адаптера доказанно ничего не отличает, оставлять его
//! хоть где-то в этом модуле — держать наполовину то же основание, что
//! только что признано неверным. Убран целиком: `any_tunnel_carries`
//! ниже отвечает только на вопрос «несёт ли ХОТЬ ОДИН туннельный адаптер
//! наши подсети», без единой попытки решить, чей это адаптер. Различение
//! «наш/чужой/неизвестно» полностью переехало к вызывающему
//! (`settings_page::Tunnel` в `crates/app`), который комбинирует этот
//! факт с `tunnel_log::liveness` — двумя независимыми источниками вместо
//! одного ненадёжного.
//!
//! Это и закрывает дыру, которую `tunnel_log` в одиночку закрыть не
//! может: если `openvpn.exe` убит без штатного выхода (`taskkill /F`,
//! обрыв питания), лог продолжает утверждать «поднято», но маршруты
//! уходят вместе с процессом — `any_tunnel_carries` в этот момент честно
//! становится `false`, и вызывающий перестаёт заявлять «поднято». Лог
//! один этого не знал бы; маршруты одни не отличили бы наш адаптер от
//! чужого. Вместе — отличают.
//!
//! Функции чистые: таблицу маршрутов и список адаптеров собирает
//! вызывающий (winnet не делает здесь никакого ввода-вывода), что и
//! позволяет проверить редкие сочетания (широкий маршрут поверх узкой
//! офисной сети и наоборот) на фикстурах, не имея их под рукой на этой
//! машине.
//!
//! `AdapterRoute.interface_alias` (форма задана брифом задачи 3) поле
//! сохранено — `winnet::routes` всё ещё его собирает, дёшево, тем же
//! проходом, что и `is_tunnel` — но этот модуль его больше не читает.
//! Причина, по которой оно НЕ годится для решений, остаётся в силе и
//! задокументирована выше: псевдоним интерфейса Windows не устойчивый
//! идентификатор (переименовываем пользователем, разные средства —
//! `route print`, `Get-NetRoute`, `netsh` — показывают его по-разному).

use proxypilot_core::net::{mask_of, Ipv4Net};

/// Один маршрут из таблицы Windows вместе с адаптером, через который он
/// идёт. `is_tunnel` — платформенный факт (TAP/TUN/WireGuard-подобный
/// адаптер), который вычисляет вызывающий; здесь он уже дан как данное.
#[derive(Debug)]
pub struct AdapterRoute {
    pub dest: Ipv4Net,
    /// Собирается вызывающим (`winnet::routes`) и сохранён в структуре
    /// на случай будущей диагностики/лога, но `any_tunnel_carries` ниже
    /// его не читает — см. докблок модуля про то, почему alias здесь
    /// больше ничего не решает.
    pub interface_alias: String,
    pub is_tunnel: bool,
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

/// Несёт ли хоть один туннельный адаптер маршрут, пересекающийся с
/// `routes` — без какой-либо попытки решить, ЧЕЙ это адаптер (round 2
/// задачи 7, см. докблок модуля). Вызывающий комбинирует этот факт с
/// `tunnel_log::liveness`, чтобы получить «наш поднят» / «занято кем-то
/// ещё» / «не знаю» — этот модуль сам этих трёх слов не произносит.
pub fn any_tunnel_carries(routes: &[Ipv4Net], adapters: &[AdapterRoute]) -> bool {
    adapters
        .iter()
        .any(|a| a.is_tunnel && routes.iter().any(|r| overlaps(r, &a.dest)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::str::FromStr;

    fn net(s: &str) -> Ipv4Net {
        Ipv4Net::from_str(s).expect("валидная подсеть в тесте")
    }

    fn adapter(dest: &str, is_tunnel: bool) -> AdapterRoute {
        AdapterRoute {
            dest: net(dest),
            // Значение — только для читаемости отладочного вывода теста;
            // `any_tunnel_carries` его не читает (докблок модуля).
            interface_alias: "SomeAdapter".to_string(),
            is_tunnel,
        }
    }

    #[test]
    fn a_permanently_up_tailscale_does_not_carry_a_disjoint_office_subnet() {
        // Tailscale-подобный адаптер поднят постоянно и несёт СВОЮ
        // подсеть (100.64.0.0/10) — она не пересекается с офисной 10/8,
        // поэтому не в счёт, независимо от того, чей это адаптер.
        let adapters = [adapter("100.64.0.0/10", true)];
        let office = [net("10.0.0.0/8")];
        assert!(!any_tunnel_carries(&office, &adapters));
    }

    #[test]
    fn a_tunnel_carrying_the_exact_office_route_counts() {
        let adapters = [adapter("10.5.0.0/16", true)];
        let office = [net("10.5.0.0/16")];
        assert!(any_tunnel_carries(&office, &adapters));
    }

    #[test]
    fn office_route_through_a_non_tunnel_adapter_does_not_count() {
        let adapters = [adapter("10.5.0.0/16", false)];
        let office = [net("10.5.0.0/16")];
        assert!(!any_tunnel_carries(&office, &adapters));
    }

    #[test]
    fn an_empty_routing_table_carries_nothing() {
        assert!(!any_tunnel_carries(&[], &[]));
    }

    #[test]
    fn a_broader_tunnel_route_covering_a_narrower_office_subnet_counts() {
        let adapters = [adapter("10.0.0.0/8", true)];
        let office = [net("10.5.0.0/16")];
        assert!(any_tunnel_carries(&office, &adapters));
    }

    #[test]
    fn a_narrower_tunnel_route_inside_a_broader_office_subnet_counts() {
        let adapters = [adapter("10.5.1.0/24", true)];
        let office = [net("10.5.0.0/16")];
        assert!(any_tunnel_carries(&office, &adapters));
    }

    #[test]
    fn a_disjoint_tunnel_route_does_not_count() {
        let adapters = [adapter("192.168.1.0/24", true)];
        let office = [net("10.5.0.0/16")];
        assert!(!any_tunnel_carries(&office, &adapters));
    }

    #[test]
    fn a_full_tunnel_commercial_vpn_counts() {
        // 0.0.0.0/0 — типичный маршрут коммерческого full-tunnel VPN
        // (NordVPN и подобные): он несёт вообще всё, включая наши
        // офисные сети, а не только их.
        let adapters = [adapter("0.0.0.0/0", true)];
        let office = [net("10.5.0.0/16")];
        assert!(any_tunnel_carries(&office, &adapters));
    }

    #[test]
    fn a_single_host_route_inside_the_office_subnet_counts() {
        // /32 — маршрут на один хост, целиком внутри офисной /16.
        let adapters = [adapter("10.5.0.7/32", true)];
        let office = [net("10.5.0.0/16")];
        assert!(any_tunnel_carries(&office, &adapters));
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
            interface_alias: "SomeAdapter".to_string(),
            is_tunnel: true,
        }];
        let office = [net("10.5.0.0/16")];
        assert!(any_tunnel_carries(&office, &adapters));
    }

    #[test]
    fn several_adapters_only_one_of_which_carries_still_counts() {
        // Не первый попавшийся адаптер решает: если хоть один несёт
        // пересекающийся маршрут, функция обязана вернуть true, даже
        // если остальные — нет.
        let adapters = [
            adapter("192.168.1.0/24", true),
            adapter("10.5.0.0/16", true),
        ];
        let office = [net("10.5.0.0/16")];
        assert!(any_tunnel_carries(&office, &adapters));
    }
}
