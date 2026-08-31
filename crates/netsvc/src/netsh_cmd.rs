//! Построение команд `netsh interface ipv4 ...`, без исполнения.
//!
//! Ровно тот же приём, что и `winnet::openvpn::build_gui_command` (задача
//! 4): конструирование `std::process::Command` — чистая функция без
//! побочных эффектов, проверяемая тестами напрямую через
//! `get_program()`/`get_args()`. Само исполнение (`Command::status()`) —
//! отдельная, непроверяемая тестами точка в `service.rs`, и её не касается
//! ни один тест этого модуля: контроллер сессии прямо запрещает выполнять
//! `netsh interface ipv4 set address`/`set dnsservers` на этой машине
//! (`CLAUDE.md`, «Живые проверки, которые не делает агент»).

use std::net::Ipv4Addr;
use std::process::Command;

use proxypilot_core::netprofile::ProfileAction;

const NETSH: &str = "netsh";

/// `netsh interface ipv4 set address name=<iface> source=static
/// address=<ip> mask=<mask> [gateway=<gateway>]` — форма `key=value`, а не
/// позиционная (`static <ip> <mask> <gateway>`), потому что она
/// одинаково понятна что в контексте `ipv4`, что при чтении командной
/// строки человеком в логе, и не завязана на фиксированный порядок
/// необязательных параметров.
pub fn set_static_address_command(
    iface: &str,
    ip: Ipv4Addr,
    mask: Ipv4Addr,
    gateway: Option<Ipv4Addr>,
) -> Command {
    let mut cmd = Command::new(NETSH);
    cmd.args(["interface", "ipv4", "set", "address"]);
    cmd.arg(format!("name={iface}"));
    cmd.arg("source=static");
    cmd.arg(format!("address={ip}"));
    cmd.arg(format!("mask={mask}"));
    if let Some(gw) = gateway {
        cmd.arg(format!("gateway={gw}"));
    }
    cmd
}

/// `netsh interface ipv4 set address name=<iface> source=dhcp` — тот же
/// адаптер возвращается на автоматическое получение адреса.
pub fn dhcp_address_command(iface: &str) -> Command {
    let mut cmd = Command::new(NETSH);
    cmd.args(["interface", "ipv4", "set", "address"]);
    cmd.arg(format!("name={iface}"));
    cmd.arg("source=dhcp");
    cmd
}

/// `netsh interface ipv4 set dnsservers name=<iface> source=dhcp` — DNS
/// адаптера возвращается на то, что выдаёт DHCP.
pub fn dhcp_dns_command(iface: &str) -> Command {
    let mut cmd = Command::new(NETSH);
    cmd.args(["interface", "ipv4", "set", "dnsservers"]);
    cmd.arg(format!("name={iface}"));
    cmd.arg("source=dhcp");
    cmd
}

/// Полный откат адаптера на DHCP — адрес и DNS. Оба нужны: `SetDhcp` из
/// `decide_profile` описывает состояние адаптера целиком, а не только
/// адрес, и страховка (`safety::evaluate_gateway`) откатывает туда же.
pub fn dhcp_restore_commands(iface: &str) -> Vec<Command> {
    vec![dhcp_address_command(iface), dhcp_dns_command(iface)]
}

/// DNS-серверы профиля. Пустой список — не «ничего не делаем»: он означает
/// «своих DNS для статики нет», и адаптер возвращается на DHCP-DNS, а не
/// залипает на прежнем значении молча.
///
/// Первый сервер идёт через `set dnsservers ... source=static
/// address=<первый> register=primary` — эта форма ЗАМЕНЯЕТ весь список
/// адаптера, поэтому она обязана быть первой; остальные добавляются по
/// одному через `add dnsservers ... address=<следующий> index=<N>`,
/// начиная с индекса 2 (1 — уже занят primary).
pub fn set_dns_commands(iface: &str, dns: &[Ipv4Addr]) -> Vec<Command> {
    let Some((first, rest)) = dns.split_first() else {
        return vec![dhcp_dns_command(iface)];
    };

    let mut primary = Command::new(NETSH);
    primary.args(["interface", "ipv4", "set", "dnsservers"]);
    primary.arg(format!("name={iface}"));
    primary.arg("source=static");
    primary.arg(format!("address={first}"));
    primary.arg("register=primary");

    let mut out = vec![primary];
    for (i, addr) in rest.iter().enumerate() {
        let mut cmd = Command::new(NETSH);
        cmd.args(["interface", "ipv4", "add", "dnsservers"]);
        cmd.arg(format!("name={iface}"));
        cmd.arg(format!("address={addr}"));
        // Индекс 1 занят primary-командой выше; первый элемент `rest` —
        // второй DNS-сервер по счёту, отсюда `+ 2`.
        cmd.arg(format!("index={}", i + 2));
        out.push(cmd);
    }
    out
}

/// Превращает решение `decide_profile` (задача 5) в готовые команды
/// `netsh`, ничего не выполняя. Единственное место в крейте, где решение
/// `ProfileAction` встречается командам — сама логика решения не
/// дублируется ни здесь, ни где-либо ещё.
pub fn commands_for_action(iface: &str, action: &ProfileAction) -> Vec<Command> {
    match action {
        ProfileAction::SetStatic {
            ip,
            mask,
            gateway,
            dns,
        } => {
            let mut cmds = vec![set_static_address_command(iface, *ip, *mask, *gateway)];
            cmds.extend(set_dns_commands(iface, dns));
            cmds
        }
        ProfileAction::SetDhcp => dhcp_restore_commands(iface),
        ProfileAction::LeaveAlone => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn static_address_command_carries_address_mask_and_gateway() {
        let cmd = set_static_address_command(
            "OfficeAdapter",
            Ipv4Addr::new(203, 0, 113, 10),
            Ipv4Addr::new(255, 255, 255, 0),
            Some(Ipv4Addr::new(203, 0, 113, 1)),
        );
        assert_eq!(cmd.get_program(), "netsh");
        assert_eq!(
            args_of(&cmd),
            vec![
                "interface",
                "ipv4",
                "set",
                "address",
                "name=OfficeAdapter",
                "source=static",
                "address=203.0.113.10",
                "mask=255.255.255.0",
                "gateway=203.0.113.1",
            ]
        );
    }

    #[test]
    fn static_address_command_omits_gateway_when_none() {
        let cmd = set_static_address_command(
            "OfficeAdapter",
            Ipv4Addr::new(203, 0, 113, 10),
            Ipv4Addr::new(255, 255, 255, 0),
            None,
        );
        let args = args_of(&cmd);
        assert!(!args.iter().any(|a| a.starts_with("gateway=")));
        assert!(args.contains(&"address=203.0.113.10".to_string()));
        assert!(args.contains(&"mask=255.255.255.0".to_string()));
    }

    #[test]
    fn interface_alias_with_spaces_stays_one_argument() {
        // Обычные имена подключений Windows несут пробелы («Ethernet 2»,
        // «Беспроводная сеть»). `Command::arg` кладёт каждый аргумент
        // отдельным значением, не строковой конкатенацией — тот же приём,
        // что и `build_gui_command_survives_a_program_path_with_spaces`
        // в `winnet::openvpn`.
        let cmd = set_static_address_command(
            "Local Area Connection 2",
            Ipv4Addr::new(203, 0, 113, 10),
            Ipv4Addr::new(255, 255, 255, 0),
            None,
        );
        assert!(args_of(&cmd).contains(&"name=Local Area Connection 2".to_string()));
    }

    #[test]
    fn single_dns_server_is_set_as_primary() {
        let cmds = set_dns_commands("OfficeAdapter", &[Ipv4Addr::new(203, 0, 113, 53)]);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].get_program(), "netsh");
        assert_eq!(
            args_of(&cmds[0]),
            vec![
                "interface",
                "ipv4",
                "set",
                "dnsservers",
                "name=OfficeAdapter",
                "source=static",
                "address=203.0.113.53",
                "register=primary",
            ]
        );
    }

    #[test]
    fn several_dns_servers_the_rest_are_added_with_increasing_index() {
        let dns = [
            Ipv4Addr::new(203, 0, 113, 53),
            Ipv4Addr::new(198, 51, 100, 53),
            Ipv4Addr::new(198, 51, 100, 54),
        ];
        let cmds = set_dns_commands("OfficeAdapter", &dns);
        assert_eq!(cmds.len(), 3, "primary + два add");

        assert_eq!(
            args_of(&cmds[0]),
            vec![
                "interface",
                "ipv4",
                "set",
                "dnsservers",
                "name=OfficeAdapter",
                "source=static",
                "address=203.0.113.53",
                "register=primary",
            ]
        );
        assert_eq!(
            args_of(&cmds[1]),
            vec![
                "interface",
                "ipv4",
                "add",
                "dnsservers",
                "name=OfficeAdapter",
                "address=198.51.100.53",
                "index=2",
            ]
        );
        assert_eq!(
            args_of(&cmds[2]),
            vec![
                "interface",
                "ipv4",
                "add",
                "dnsservers",
                "name=OfficeAdapter",
                "address=198.51.100.54",
                "index=3",
            ]
        );
    }

    #[test]
    fn empty_dns_list_falls_back_to_dhcp_source() {
        // Профиль может задавать статический адрес, но не задавать
        // собственных DNS — тогда резолвер остаётся тем, что выдаёт DHCP,
        // а не залипает на прошлом значении навсегда.
        let cmds = set_dns_commands("OfficeAdapter", &[]);
        assert_eq!(cmds.len(), 1);
        assert!(args_of(&cmds[0]).contains(&"source=dhcp".to_string()));
    }

    #[test]
    fn dhcp_restore_resets_both_address_and_dns() {
        let cmds = dhcp_restore_commands("OfficeAdapter");
        assert_eq!(cmds.len(), 2);
        assert!(args_of(&cmds[0]).contains(&"source=dhcp".to_string()));
        assert!(args_of(&cmds[0]).contains(&"name=OfficeAdapter".to_string()));
        assert_eq!(
            args_of(&cmds[1]),
            vec![
                "interface",
                "ipv4",
                "set",
                "dnsservers",
                "name=OfficeAdapter",
                "source=dhcp",
            ]
        );
    }

    #[test]
    fn commands_for_leave_alone_is_empty() {
        assert!(commands_for_action("OfficeAdapter", &ProfileAction::LeaveAlone).is_empty());
    }

    #[test]
    fn commands_for_set_dhcp_action_matches_dhcp_restore() {
        let via_action = commands_for_action("OfficeAdapter", &ProfileAction::SetDhcp);
        let direct = dhcp_restore_commands("OfficeAdapter");
        assert_eq!(via_action.len(), direct.len());
        for (a, b) in via_action.iter().zip(direct.iter()) {
            assert_eq!(args_of(a), args_of(b));
        }
    }

    #[test]
    fn commands_for_set_static_action_bundles_address_and_dns() {
        let action = ProfileAction::SetStatic {
            ip: Ipv4Addr::new(203, 0, 113, 10),
            mask: Ipv4Addr::new(255, 255, 255, 0),
            gateway: Some(Ipv4Addr::new(203, 0, 113, 1)),
            dns: vec![
                Ipv4Addr::new(203, 0, 113, 53),
                Ipv4Addr::new(198, 51, 100, 53),
            ],
        };
        let cmds = commands_for_action("OfficeAdapter", &action);
        // 1 команда на адрес + 2 команды на DNS (primary + один add).
        assert_eq!(cmds.len(), 3);
        assert!(args_of(&cmds[0]).contains(&"address=203.0.113.10".to_string()));
        assert!(args_of(&cmds[1]).contains(&"address=203.0.113.53".to_string()));
        assert!(args_of(&cmds[2]).contains(&"address=198.51.100.53".to_string()));
    }
}
