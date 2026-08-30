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

use proxypilot_core::net::{mask_of, Ipv4Net};
use std::net::Ipv4Addr;

/// Директива, которую наш клиент печатает как ошибку при каждом старте —
/// это параметр другой Windows-сборки OpenVPN, не той, что у нас (спека 8.1).
const BLOCK_OUTSIDE_DNS: &str = "setenv opt block-outside-dns";

const REDIRECT_GATEWAY_FILTER: &str = r#"pull-filter ignore "redirect-gateway""#;

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
/// целиком заменяется новым. Это проверено не только на первом вызове —
/// задача 5 пересобирает профиль при каждой смене списка офисных подсетей,
/// то есть второй (и третий, и десятый) вызов — обычный ход дел, а не
/// патология, и раунд 2 ревью нашёл ровно баг, ломавшийся именно на втором
/// проходе (см. `lines_to_drop`).
///
/// Окончания строк источника (`\n` или `\r\n`) сохраняются: результат не
/// переписывает файл в стиль, который сам не выбирал.
pub fn build_profile(source: &str, routes: &[Ipv4Net]) -> String {
    let newline = detect_line_ending(source);
    let raw_lines: Vec<&str> = source.lines().collect();
    let drop = lines_to_drop(&raw_lines);

    let mut lines: Vec<&str> = raw_lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !drop[*i])
        .map(|(_, l)| *l)
        .collect();
    // Убираем хвостовые пустые строки, оставшиеся после вычистки — иначе
    // каждая пересборка добавляла бы ещё одну пустую строку перед блоком.
    while matches!(lines.last(), Some(l) if l.trim().is_empty()) {
        lines.pop();
    }

    // Если такой фильтр уже стоит вне нашего блока (профиль подготовили
    // руками), вторая копия в новом блоке ничему не вредит для OpenVPN, но
    // это ровно та видимая избыточность, от которой должен спасать блок с
    // маркерами — поэтому не добавляем. Сравнение через `normalize_directive`
    // — та же нормализация, что и у `block-outside-dns`: раунд 1 научил ей
    // только одну из двух похожих проверок, раунд 2 научил вторую.
    let redirect_gateway_present = lines
        .iter()
        .any(|l| normalize_directive(l) == REDIRECT_GATEWAY_FILTER);

    let mut out_lines: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
    // Пустая строка-разделитель нужна только если в источнике вообще что-то
    // осталось — иначе пустой (или полностью вычищенный) источник получает
    // добавленный блок с лишней пустой строкой перед ним.
    if !out_lines.is_empty() {
        out_lines.push(String::new());
    }

    out_lines.push(BEGIN_MARKER.to_string());
    if !redirect_gateway_present {
        out_lines
            .push("# Сервер обычно пушит маршрут по умолчанию и не пушит маршруты".to_string());
        out_lines
            .push("# в офисные подсети — без строки ниже весь трафик, включая видео,".to_string());
        out_lines.push("# уходит в туннель кругом через офис (спека 8.1).".to_string());
        out_lines.push(REDIRECT_GATEWAY_FILTER.to_string());
    }
    out_lines.push("# Явные маршруты в офисные подсети. Подсеть, где машина стоит".to_string());
    out_lines.push("# физически, не страдает: её собственная запись в таблице".to_string());
    out_lines.push("# маршрутов точнее любой из этих.".to_string());
    for route in routes {
        out_lines.push(format!(
            "route {} {}",
            masked_addr(route),
            mask_of(route.prefix)
        ));
    }
    out_lines
        .push("# Пушенный DNS осознанно НЕ фильтруется (расхождение с macOS-версией,".to_string());
    out_lines
        .push("# спека 8.2): туннель нужен ради внутренних имён (git, dev-серверы),".to_string());
    out_lines
        .push("# а без офисного DNS они не резолвятся. Плата — пока туннель поднят,".to_string());
    out_lines.push("# все DNS-запросы идут в офис; это показывается в UI (задача 7).".to_string());
    out_lines.push(END_MARKER.to_string());

    let mut out = out_lines.join(newline);
    out.push_str(newline);
    out
}

/// Для каждой строки источника решает, войдёт ли она в сохранённую часть.
///
/// Два источника опасности, оба — уроки раунда 2 ревью:
///
/// 1. **Непарный маркер переживает сборку.** Раунд 1 оставлял одинокий
///    `BEGIN` без `END` нетронутым как обычную строку — но тогда наш же
///    свежедобавленный `END` в конце файла становился ему парой на
///    *следующей* сборке, и всё между ними (сертификаты включительно)
///    считалось «нашим блоком» и стиралось. Тихо, потому что первая
///    сборка выглядела правильной. Здесь одинокий маркер (будь то `BEGIN`
///    без `END`, `END` без предшествующего `BEGIN`, или более ранний
///    `BEGIN`, вытесненный более поздним до того, как у него нашёлся свой
///    `END`) вычищается только сам — одна строка, а не диапазон.
/// 2. **Маркер внутри inline-блока (`<ca>`, `<cert>`, `<key>`, ...) — не
///    наш.** Между открывающим и закрывающим тегом лежат непрозрачные
///    PEM-данные; текст, случайно совпавший там с нашим маркером,
///    трогать нельзя вообще — ни как одиночную строку, ни тем более как
///    пару. Наш собственный блок туда никогда не попадает: он всегда
///    дописывается на верхнем уровне, в конец файла.
fn lines_to_drop(raw_lines: &[&str]) -> Vec<bool> {
    let mut drop = vec![false; raw_lines.len()];
    let mut pending_begin: Option<usize> = None;
    let mut in_inline_block = false;

    for (i, line) in raw_lines.iter().enumerate() {
        let trimmed = line.trim();
        if is_inline_block_open(trimmed) {
            in_inline_block = true;
        }

        if !in_inline_block {
            if trimmed == BEGIN_MARKER {
                // Более ранний непарный BEGIN (если был) сиротой не
                // остаётся молча: вычищаем его как одиночную строку —
                // ровно это и не делалось в раунде 1.
                if let Some(prev) = pending_begin {
                    drop[prev] = true;
                }
                pending_begin = Some(i);
            } else if trimmed == END_MARKER {
                match pending_begin {
                    Some(begin) => {
                        for slot in drop.iter_mut().take(i + 1).skip(begin) {
                            *slot = true;
                        }
                        pending_begin = None;
                    }
                    // END без открытого BEGIN — сам по себе не образует
                    // диапазон, но как маркер он тоже не содержимое
                    // профиля, поэтому вычищается один.
                    None => drop[i] = true,
                }
            } else if is_block_outside_dns_directive(line) {
                drop[i] = true;
            }
        }

        if is_inline_block_close(trimmed) {
            in_inline_block = false;
        }
    }
    // BEGIN, для которого END не нашёлся вовсе до конца файла, — тоже
    // сирота: вычищаем только его. Это и есть Critical раунда 2: раньше
    // здесь ничего не помечалось, и одинокий BEGIN оставался в тексте,
    // готовый спариться с чужим END через сборку.
    if let Some(begin) = pending_begin {
        drop[begin] = true;
    }
    drop
}

/// Строка вида `<tag>` — начало inline-блока OpenVPN (`<ca>`, `<cert>`,
/// `<key>`, `<tls-auth>`, ...). Имя тега не проверяется: профилю всё равно
/// известны только эти несколько тегов, а любой `<...>` в начале строки на
/// верхнем уровне `.ovpn`-файла — это открывающий тег inline-блока, не
/// обычный параметр.
fn is_inline_block_open(trimmed: &str) -> bool {
    trimmed.starts_with('<') && !trimmed.starts_with("</") && trimmed.ends_with('>')
}

/// Строка вида `</tag>` — конец inline-блока.
fn is_inline_block_close(trimmed: &str) -> bool {
    trimmed.starts_with("</") && trimmed.ends_with('>')
}

/// Схлопывает повторяющиеся пробелы и отрезает хвостовой комментарий
/// (`#`/`;`) перед сравнением с канонической записью директивы. Общая
/// процедура для `block-outside-dns` и для `redirect-gateway`-фильтра:
/// раунд 1 научил этому только первую проверку, раунд 2 — вторую. Одна и
/// та же нормализация для обеих не даёт им разойтись снова.
fn normalize_directive(line: &str) -> String {
    let without_comment = line.split(['#', ';']).next().unwrap_or("");
    without_comment
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `true`, если строка — директива `setenv opt block-outside-dns`, пусть
/// даже с необычными пробелами (`setenv  opt  block-outside-dns`) или
/// хвостовым комментарием (`setenv opt block-outside-dns # заметка`) —
/// обе формы синтаксически валидны для OpenVPN так же, как каноническая
/// запись, и обязаны вычищаться одинаково.
fn is_block_outside_dns_directive(line: &str) -> bool {
    normalize_directive(line) == BLOCK_OUTSIDE_DNS
}

/// Маска считается заново, а не берётся из уже промаскированного
/// `Ipv4Net::from_str` — потому что поля `Ipv4Net` публичны, и этот
/// конструктор (`Ipv4Net { addr, prefix }` напрямую) можно обойти. Задачи
/// 3, 5 и 7 будут собирать такие значения не только через `FromStr`. Это не
/// дублирование маскировки в `core::net::Ipv4Net::from_str` — это два
/// разных места, куда адрес с битами хоста может прийти, и каждое обязано
/// защищаться само, независимо от другого.
fn masked_addr(net: &Ipv4Net) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(net.addr) & u32::from(mask_of(net.prefix)))
}

/// `\r\n`, если в источнике `\r\n` не меньше, чем голых `\n` (типичный
/// случай для профиля, сохранённого на Windows — и ничья решается в его
/// пользу: не переписывать существующий CRLF в LF безопаснее, чем
/// наоборот), иначе `\n`. Источник вовсе без переносов строк (пустой или
/// однострочный без завершающего `\n`) — сигнала нет, и дефолт `\n`
/// безопаснее любого предположения.
fn detect_line_ending(source: &str) -> &'static str {
    let crlf = source.matches("\r\n").count();
    let all_newlines = source.matches('\n').count();
    let lf_only = all_newlines.saturating_sub(crlf);
    if crlf == 0 && lf_only == 0 {
        return "\n";
    }
    if crlf >= lf_only {
        "\r\n"
    } else {
        "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn block_outside_dns_with_extra_whitespace_is_stripped() {
        let source = SOURCE.replace(
            "setenv opt block-outside-dns",
            "setenv  opt   block-outside-dns",
        );
        let out = build_profile(&source, &routes());
        assert!(!out.contains("block-outside-dns"));
    }

    #[test]
    fn block_outside_dns_with_a_trailing_comment_is_stripped() {
        let source = SOURCE.replace(
            "setenv opt block-outside-dns",
            "setenv opt block-outside-dns # не наше, но клиент ругается",
        );
        let out = build_profile(&source, &routes());
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

    #[test]
    fn an_unbalanced_begin_marker_does_not_truncate_the_profile() {
        // Обрезанный (или вручную подправленный) прошлый результат: BEGIN
        // есть, END потерялся. Раунд 1 чинил только первую сборку — второй
        // вызов (обычный ход дел в задаче 5, пересобирающей профиль при
        // каждой смене офисных подсетей) снова терял сертификат, потому что
        // добавленный нами END на первом проходе спаривался с чужим,
        // непарным BEGIN на втором. Проверяем оба прохода.
        let source = format!("client\n{BEGIN_MARKER}\n<ca>\nCERT\n</ca>\n");

        let once = build_profile(&source, &routes());
        assert!(once.contains("CERT"), "первая сборка потеряла CERT");
        assert!(once.contains("<ca>"));
        assert!(once.contains("</ca>"));

        let twice = build_profile(&once, &routes());
        assert!(twice.contains("CERT"), "вторая сборка потеряла CERT");
        assert!(twice.contains("<ca>"));
        assert!(twice.contains("</ca>"));
    }

    #[test]
    fn a_source_that_is_only_a_begin_marker_is_a_fixpoint() {
        // Источник целиком — один одинокий BEGIN без END. Первая сборка
        // вычищает его (одну строку) и дописывает собственный блок; вторая
        // сборка над этим результатом обязана дать тот же самый текст —
        // иначе конвергенции нет и на каждом вызове что-то тихо меняется.
        let source = format!("{BEGIN_MARKER}\n");
        let once = build_profile(&source, &routes());
        let twice = build_profile(&once, &routes());
        assert_eq!(twice, once);
    }

    #[test]
    fn markers_inside_an_inline_block_do_not_delete_its_content() {
        // Маркеры, оказавшиеся внутри <ca>...</ca> (например, после ручной
        // правки источника или совпадения в самих PEM-данных), — не наш
        // блок: это непрозрачное содержимое сертификата, и пара между ними
        // может быть настоящим текстом ключа, а не сгенерированными нами
        // директивами.
        let source = format!("client\n<ca>\n{BEGIN_MARKER}\nCERT-D\n{END_MARKER}\n</ca>\n");
        let once = build_profile(&source, &routes());
        assert!(once.contains("CERT-D"), "первая сборка потеряла CERT-D");
        assert!(once.contains(BEGIN_MARKER));
        assert!(once.contains(END_MARKER));
        assert!(once.contains("<ca>"));
        assert!(once.contains("</ca>"));

        // И на втором проходе: наш настоящий блок (дописанный первой
        // сборкой снаружи <ca>) обязан правильно опознаться и замениться
        // собой же, не задев то, что лежит внутри <ca>.
        let twice = build_profile(&once, &routes());
        assert!(twice.contains("CERT-D"), "вторая сборка потеряла CERT-D");
        assert!(twice.contains("<ca>"));
        assert!(twice.contains("</ca>"));
    }

    #[test]
    fn an_existing_redirect_gateway_filter_outside_the_block_is_not_duplicated() {
        let source = format!("{SOURCE}pull-filter ignore \"redirect-gateway\"\n");
        let out = build_profile(&source, &routes());
        assert_eq!(
            out.matches(r#"pull-filter ignore "redirect-gateway""#)
                .count(),
            1
        );
    }

    #[test]
    fn an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated() {
        // Раунд 1 научил нормализации только вычистку block-outside-dns;
        // эта проверка сравнивала строки буквально и не замечала свой же
        // фильтр под двойным пробелом.
        let source = format!("{SOURCE}pull-filter  ignore \"redirect-gateway\"\n");
        let out = build_profile(&source, &routes());
        assert_eq!(out.matches("redirect-gateway").count(), 1);
    }

    #[test]
    fn route_host_bits_are_masked_even_when_ipv4net_is_built_directly() {
        // В обход FromStr (который уже маскирует сам) — напрямую через
        // публичные поля, как это потенциально сделают задачи 3/5/7.
        let route = Ipv4Net {
            addr: "203.0.113.5".parse().unwrap(),
            prefix: 24,
        };
        let out = build_profile(SOURCE, &[route]);
        assert!(out.contains("route 203.0.113.0 255.255.255.0"));
        assert!(!out.contains("203.0.113.5"));
    }

    #[test]
    fn crlf_source_stays_crlf() {
        let source = SOURCE.replace('\n', "\r\n");
        let out = build_profile(&source, &routes());
        assert!(out.contains("client\r\ndev tun"));
        // Каждый перенос строки в результате — именно CRLF, а не голый LF:
        // число "\n" совпадает с числом "\r\n".
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    }

    #[test]
    fn a_tie_between_crlf_and_lf_favours_crlf() {
        // Один CRLF и один голый LF — сигнал поровну; ничья в пользу CRLF,
        // а не LF: профиль обычно готовят на Windows.
        let source = "client\r\ndev tun\n";
        let out = build_profile(source, &routes());
        assert!(out.starts_with("client\r\ndev tun\r\n"));
    }

    #[test]
    fn empty_source_has_no_leading_blank_line() {
        let out = build_profile("", &routes());
        assert!(out.starts_with(BEGIN_MARKER));
    }
}
