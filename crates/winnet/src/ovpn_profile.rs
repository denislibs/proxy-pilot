//! Сборка split-tunnel профиля из пользовательского `.ovpn` (спека 8.1,
//! 8.2). Чистая функция над строками, без файлового ввода-вывода: куда
//! класть результат (`proxypilot-office.ovpn` рядом с исходником в
//! каталоге конфигураций OpenVPN) решает вызывающий код, не этот модуль.
//!
//! Исходные строки не теряются: профиль несёт сертификаты и параметры,
//! которых этот код не разбирает, и пересборка с нуля — тот самый способ,
//! которым рабочий профиль превращается в нерабочий. Правка — это ровно
//! две вещи: вычистить одну известную директиву и дописать блок,
//! отмеченный собственными маркерами, а не «дописать в конец, что бог на
//! душу положит».
//!
//! Блок с маркерами, а не проверка «такая строка уже есть» построчно: при
//! повторной сборке (после смены списка офисных подсетей — задача 5) старый
//! блок вычищается целиком и пишется заново, поэтому лишний `route` за
//! подсеть, которую убрали из конфига, не остаётся сиротой навсегда.

use proxypilot_core::net::Ipv4Net;

/// Директива, которую наш клиент печатает как ошибку при каждом старте —
/// это параметр другой Windows-сборки OpenVPN, не той, что у нас (спека 8.1).
const BLOCK_OUTSIDE_DNS: &str = "setenv opt block-outside-dns";

const BEGIN_MARKER: &str =
    "# --- ProxyPilot: начало добавленного блока, не редактировать руками ---";
const END_MARKER: &str = "# --- ProxyPilot: конец добавленного блока ---";

/// Собирает split-tunnel профиль поверх исходного `.ovpn`.
///
/// `source` — текст исходного профиля пользователя (сертификаты и параметры
/// сервера, задача 6 читает его с диска). `routes` — офисные подсети, в
/// которые нужны явные маршруты (задача 5 читает их из конфига; эта функция
/// конфиг не читает и `OfficeNetwork`, хранящий GUID сети NLM, не видит —
/// маршрут из GUID не выводится, см. брифинг задачи).
///
/// Идемпотентна: повторный вызов над уже собранным профилем (в том числе
/// с другим набором `routes`) не копит директивы — старый добавленный блок
/// целиком заменяется новым.
pub fn build_profile(source: &str, routes: &[Ipv4Net]) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut in_generated_block = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed == BEGIN_MARKER {
            in_generated_block = true;
            continue;
        }
        if trimmed == END_MARKER {
            in_generated_block = false;
            continue;
        }
        if in_generated_block {
            continue;
        }
        // Чужой (не наш) артефакт другого Windows-клиента — не то же самое,
        // что наш блок, поэтому вычищается отдельной проверкой и в любом
        // месте исходника, а не только внутри маркеров.
        if trimmed == BLOCK_OUTSIDE_DNS {
            continue;
        }
        lines.push(line);
    }
    // Убираем хвостовые пустые строки, оставшиеся после вычистки — иначе
    // каждая пересборка добавляла бы ещё одну пустую строку перед блоком.
    while matches!(lines.last(), Some(l) if l.trim().is_empty()) {
        lines.pop();
    }

    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(BEGIN_MARKER);
    out.push('\n');
    out.push_str(
        "# Сервер обычно пушит маршрут по умолчанию и не пушит маршруты\n\
         # в офисные подсети — без строки ниже весь трафик, включая видео,\n\
         # уходит в туннель кругом через офис (спека 8.1).\n",
    );
    out.push_str(r#"pull-filter ignore "redirect-gateway""#);
    out.push('\n');
    out.push_str(
        "# Явные маршруты в офисные подсети. Подсеть, где машина стоит\n\
         # физически, не страдает: её собственная запись в таблице\n\
         # маршрутов точнее любой из этих.\n",
    );
    for route in routes {
        out.push_str(&format!(
            "route {} {}\n",
            route.addr,
            proxypilot_core::net::mask_of(route.prefix)
        ));
    }
    out.push_str(
        "# Пушенный DNS осознанно НЕ фильтруется (расхождение с macOS-версией,\n\
         # спека 8.2): туннель нужен ради внутренних имён (git, dev-серверы),\n\
         # а без офисного DNS они не резолвятся. Плата — пока туннель поднят,\n\
         # все DNS-запросы идут в офис; это показывается в UI (задача 7).\n",
    );
    out.push_str(END_MARKER);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::net::Ipv4Net;
    use std::str::FromStr;

    // RFC 5737 — документационные диапазоны, никаких реальных офисных
    // подсетей в публичном репозитории (CLAUDE.md).
    const SOURCE: &str = "\
client
dev tun
proto udp
remote vpn.example.internal 1194
setenv opt block-outside-dns
<ca>
-----BEGIN CERTIFICATE-----
СЕРТИФИКАТ-ЗАГЛУШКА
-----END CERTIFICATE-----
</ca>
";

    fn routes() -> Vec<Ipv4Net> {
        vec![
            Ipv4Net::from_str("203.0.113.0/24").unwrap(),
            Ipv4Net::from_str("198.51.100.0/24").unwrap(),
        ]
    }

    #[test]
    fn redirect_gateway_is_filtered() {
        let out = build_profile(SOURCE, &routes());
        assert!(out.contains(r#"pull-filter ignore "redirect-gateway""#));
    }

    #[test]
    fn every_route_is_present() {
        let out = build_profile(SOURCE, &routes());
        assert!(out.contains("route 203.0.113.0 255.255.255.0"));
        assert!(out.contains("route 198.51.100.0 255.255.255.0"));
    }

    #[test]
    fn source_lines_survive() {
        let out = build_profile(SOURCE, &routes());
        assert!(out.contains("client"));
        assert!(out.contains("dev tun"));
        assert!(out.contains("proto udp"));
        assert!(out.contains("remote vpn.example.internal 1194"));
        assert!(out.contains("-----BEGIN CERTIFICATE-----"));
        assert!(out.contains("СЕРТИФИКАТ-ЗАГЛУШКА"));
        assert!(out.contains("-----END CERTIFICATE-----"));
    }

    #[test]
    fn block_outside_dns_is_stripped() {
        let out = build_profile(SOURCE, &routes());
        assert!(!out.contains("block-outside-dns"));
    }

    #[test]
    fn pushed_dns_is_not_filtered() {
        // Осознанное расхождение с macOS-версией (спека 8.2): пушенный
        // DNS принимаем, иначе внутренние имена не резолвятся.
        let out = build_profile(SOURCE, &routes());
        assert!(!out.contains(r#"dhcp-option DNS"#));
    }

    #[test]
    fn building_twice_does_not_duplicate_directives() {
        let once = build_profile(SOURCE, &routes());
        let twice = build_profile(&once, &routes());
        assert_eq!(
            twice
                .matches(r#"pull-filter ignore "redirect-gateway""#)
                .count(),
            1
        );
        assert_eq!(twice.matches("route 203.0.113.0 255.255.255.0").count(), 1);
        assert_eq!(twice.matches("route 198.51.100.0 255.255.255.0").count(), 1);
    }

    #[test]
    fn empty_routes_still_adds_the_filter() {
        let out = build_profile(SOURCE, &[]);
        assert!(out.contains(r#"pull-filter ignore "redirect-gateway""#));
    }
}
