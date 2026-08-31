//! Страховка (спека 7.3): после применения статического адреса проверяем
//! достижимость офисного шлюза, и при неудаче откатываемся в DHCP с
//! записью в лог.
//!
//! На Windows риск ниже, чем на macOS-версии (сеть опознана по GUID NLM, а
//! не по совпадению префикса подсети), но откат стоит дёшево и остаётся.
//!
//! Проверка достижимости и запись в лог приходят СНАРУЖИ замыканиями, а не
//! вызываются отсюда напрямую (`IcmpSendEcho`/`tracing::warn!`) — ровно
//! затем, чтобы эту функцию можно было проверить тестом, не трогая
//! настоящую сеть и не заводя подписчика `tracing` в тесте. Реальные
//! замыкания (пинг шлюза, `tracing::warn!`) подставляет `service.rs`,
//! непроверяемый автотестами по той же причине, что и исполнение самих
//! команд `netsh` (см. докблок `netsh_cmd`).

use std::net::Ipv4Addr;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyNetOutcome {
    /// Шлюз в профиле не задан — проверять нечего, статика остаётся как есть.
    NoGatewayToCheck,
    /// Шлюз ответил — статика остаётся применённой.
    GatewayReachable,
    /// Шлюз не ответил — построены команды отката в DHCP (см. возвращаемый
    /// вектор). Сами команды здесь не выполняются.
    RolledBack,
}

/// Решает, нужен ли откат в DHCP после применения статики, и строит его
/// команды — но не выполняет ни их, ни саму проверку достижимости.
///
/// `is_reachable` и `log` приходят замыканиями намеренно (докблок модуля):
/// это единственный способ проверить всю логику — «без шлюза не проверяем»,
/// «шлюз ответил — не трогаем», «шлюз не ответил — откат и одна запись в
/// лог» — тестом, не отправляя ни одного настоящего пакета и не заводя
/// подписчика `tracing`. `is_reachable` — `FnOnce`: проверка одна на вызов,
/// а не потенциально многократная, и это видно из типа, а не только из
/// реализации.
pub fn evaluate_gateway(
    iface: &str,
    gateway: Option<Ipv4Addr>,
    is_reachable: impl FnOnce(Ipv4Addr) -> bool,
    mut log: impl FnMut(&str),
) -> (SafetyNetOutcome, Vec<Command>) {
    let Some(gw) = gateway else {
        return (SafetyNetOutcome::NoGatewayToCheck, Vec::new());
    };
    if is_reachable(gw) {
        return (SafetyNetOutcome::GatewayReachable, Vec::new());
    }
    log(&format!(
        "шлюз {gw} недостижим после применения статики на «{iface}» — откатываемся в DHCP"
    ));
    (
        SafetyNetOutcome::RolledBack,
        crate::netsh_cmd::dhcp_restore_commands(iface),
    )
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
    fn no_gateway_configured_skips_the_check_entirely() {
        let mut probed = false;
        let (outcome, cmds) = evaluate_gateway(
            "OfficeAdapter",
            None,
            |_| {
                probed = true;
                true
            },
            |_| panic!("без шлюза логировать нечего"),
        );
        assert_eq!(outcome, SafetyNetOutcome::NoGatewayToCheck);
        assert!(cmds.is_empty());
        assert!(
            !probed,
            "без шлюза в профиле проверка достижимости не должна вызываться вовсе"
        );
    }

    #[test]
    fn reachable_gateway_needs_no_rollback_and_no_log_entry() {
        let mut logged = Vec::new();
        let (outcome, cmds) = evaluate_gateway(
            "OfficeAdapter",
            Some(Ipv4Addr::new(203, 0, 113, 1)),
            |_| true,
            |msg| logged.push(msg.to_string()),
        );
        assert_eq!(outcome, SafetyNetOutcome::GatewayReachable);
        assert!(cmds.is_empty());
        assert!(logged.is_empty());
    }

    #[test]
    fn unreachable_gateway_produces_the_dhcp_restore_commands_and_a_log_entry() {
        let mut logged = Vec::new();
        let (outcome, cmds) = evaluate_gateway(
            "OfficeAdapter",
            Some(Ipv4Addr::new(203, 0, 113, 1)),
            |_| false,
            |msg| logged.push(msg.to_string()),
        );
        assert_eq!(outcome, SafetyNetOutcome::RolledBack);

        // Ровно те же команды, что и `netsh_cmd::dhcp_restore_commands` —
        // адрес и DNS обратно на DHCP.
        assert_eq!(cmds.len(), 2);
        assert!(args_of(&cmds[0]).contains(&"source=dhcp".to_string()));
        assert!(args_of(&cmds[0]).contains(&"name=OfficeAdapter".to_string()));
        assert!(args_of(&cmds[1]).contains(&"source=dhcp".to_string()));

        assert_eq!(logged.len(), 1, "ровно одна запись в лог на откат");
        assert!(
            logged[0].contains("203.0.113.1"),
            "лог обязан называть недостижимый шлюз: {}",
            logged[0]
        );
        assert!(
            logged[0].contains("DHCP"),
            "лог обязан говорить, что происходит откат: {}",
            logged[0]
        );
    }

    #[test]
    fn the_closure_is_called_with_the_configured_gateway() {
        let mut seen = None;
        evaluate_gateway(
            "OfficeAdapter",
            Some(Ipv4Addr::new(198, 51, 100, 1)),
            |gw| {
                seen = Some(gw);
                true
            },
            |_| {},
        );
        assert_eq!(seen, Some(Ipv4Addr::new(198, 51, 100, 1)));
    }
}
