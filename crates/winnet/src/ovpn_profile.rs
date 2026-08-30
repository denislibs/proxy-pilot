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
//!
//! ## Один классификатор, а не три мнения о строке
//!
//! Раунды 1 и 2 чинили конкретные проявления одной и той же болезни:
//! разные части файла независимо решали, что такое «строка внутри
//! `<ca>`» или «наш маркер», и эти решения расходились. Каждая новая
//! правка добавляла ещё одно мнение вместо того, чтобы поправить решение
//! в одном месте. Здесь — единственная точка, [`classify_lines`], которая
//! проходит по источнику один раз и относит каждую строку ровно к одному
//! из трёх видов ([`LineKind`]): вершина файла, содержимое inline-блока
//! (`<ca>`, `<cert>`, `<key>`, ...) или наш собственный маркер. Всё
//! остальное — вычистка диапазона, проверка «фильтр уже стоит»,
//! вычистка `block-outside-dns` — потребляет готовую классификацию и
//! никогда не решает заново, какая перед ним строка.
//!
//! Незакрытый inline-блок — это уже нерабочий для OpenVPN профиль; здесь
//! он становится явной ошибкой [`ProfileError`], а не поводом гадать, где
//! блок должен был закончиться. Собрать правдоподобный профиль из
//! сломанного источника — значит отправить человека отлаживать OpenVPN
//! вместо своего же файла.

use proxypilot_core::net::{mask_of, Ipv4Net};
use std::net::Ipv4Addr;

/// Директива, которую наш клиент печатает как ошибку при каждом старте —
/// это параметр другой Windows-сборки OpenVPN, не той, что у нас (спека 8.1).
const BLOCK_OUTSIDE_DNS: &str = "setenv opt block-outside-dns";

const REDIRECT_GATEWAY_FILTER: &str = r#"pull-filter ignore "redirect-gateway""#;

const BEGIN_MARKER: &str =
    "# --- ProxyPilot: начало добавленного блока, не редактировать руками ---";
const END_MARKER: &str = "# --- ProxyPilot: конец добавленного блока ---";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// В источнике открыт inline-блок (`<tag>`), для которого до конца
    /// файла не нашлось парного `</tag>`. OpenVPN такой профиль сам не
    /// загрузит — отказ здесь честнее, чем правдоподобный, но неверный
    /// результат, который отправит человека искать проблему не там.
    #[error("незакрытый inline-блок «<{tag}>» (строка {line}) — в источнике нет соответствующего «</{tag}>»")]
    UnterminatedInlineBlock { tag: String, line: usize },
}

/// Собирает split-tunnel профиль поверх исходного `.ovpn`.
///
/// `source` — текст исходного профиля пользователя (сертификаты и параметры
/// сервера, задача 6 читает его с диска). `routes` — офисные подсети, в
/// которые нужны явные маршруты (задача 5 читает их из конфига; эта функция
/// конфиг не читает и `OfficeNetwork`, хранящий GUID сети NLM, не видит —
/// маршрут из GUID не выводится, см. брифинг задачи).
///
/// `Err` — только при структурно сломанном источнике (см. [`ProfileError`]);
/// во всех остальных случаях, включая пустой источник, возвращает `Ok`.
///
/// Идемпотентна: повторный вызов над уже собранным профилем (в том числе
/// с другим набором `routes`) не копит директивы — старый добавленный блок
/// целиком заменяется новым. Это проверено не только на первом вызове —
/// задача 5 пересобирает профиль при каждой смене списка офисных подсетей,
/// то есть второй (и третий) вызов — обычный ход дел, а не патология.
///
/// Окончания строк источника (`\n` или `\r\n`) сохраняются: результат не
/// переписывает файл в стиль, который сам не выбирал.
pub fn build_profile(source: &str, routes: &[Ipv4Net]) -> Result<String, ProfileError> {
    let newline = detect_line_ending(source);
    let raw_lines: Vec<&str> = source.lines().collect();
    let classified = classify_lines(&raw_lines)?;
    let drop = drop_mask(&classified);

    let mut lines: Vec<&str> = Vec::with_capacity(raw_lines.len());
    // Единственное место, читающее классификацию: «уже стоит ли
    // top-level redirect-gateway-фильтр» и «что уцелело» решаются по
    // одним и тем же данным, а не заново угадываются по тексту.
    let mut redirect_gateway_present = false;
    for (i, kind) in classified.iter().enumerate() {
        if drop[i] {
            continue;
        }
        match kind {
            LineKind::TopLevel(line) => {
                if normalize_directive(line) == REDIRECT_GATEWAY_FILTER {
                    redirect_gateway_present = true;
                }
                lines.push(*line);
            }
            LineKind::Inline(line) => lines.push(*line),
            // По построению drop_mask каждый Begin/End либо входит в
            // вычищенный диапазон, либо вычищен как сирота — сюда дойти
            // невозможно. Не паникуем на случай будущей правки drop_mask:
            // просто ничего не сохраняем, а не ломаем сборку.
            LineKind::Begin | LineKind::End => {}
        }
    }
    // Убираем хвостовые пустые строки, оставшиеся после вычистки — иначе
    // каждая пересборка добавляла бы ещё одну пустую строку перед блоком.
    while matches!(lines.last(), Some(l) if l.trim().is_empty()) {
        lines.pop();
    }

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
    Ok(out)
}

/// Ровно то, чем является строка источника — единственная классификация,
/// которую потребляет всё остальное в этом модуле.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind<'a> {
    /// Строка верхнего уровня — параметр профиля, комментарий, что угодно
    /// вне inline-блока и не наш маркер.
    TopLevel(&'a str),
    /// Строка внутри `<tag>...</tag>` — включая сами открывающую и
    /// закрывающую строки. Непрозрачна для всех проверок ниже: то, что
    /// там лежит, может быть чем угодно, вплоть до текста, случайно
    /// совпавшего с нашим маркером или с чужим `pull-filter`.
    Inline(&'a str),
    /// Наш собственный маркер начала блока.
    Begin,
    /// Наш собственный маркер конца блока.
    End,
}

/// Единственный проход, решающий для каждой строки источника, что она
/// такое. Дальше по этому файлу никто не спрашивает заново «а не внутри
/// ли я `<ca>`» или «а не наш ли это маркер» — ответ уже дан здесь и ровно
/// один раз.
///
/// `Err`, если по источнику остался открытый inline-блок: OpenVPN такой
/// профиль не загрузит, и предположение здесь о том, где он должен был
/// закончиться, было бы недобросовестной догадкой поверх уже сломанных
/// данных.
fn classify_lines<'a>(raw_lines: &[&'a str]) -> Result<Vec<LineKind<'a>>, ProfileError> {
    let mut result = Vec::with_capacity(raw_lines.len());
    // Тег текущего открытого inline-блока и номер строки, где он открылся
    // (для сообщения об ошибке, если блок так и не закроется).
    let mut open: Option<(&str, usize)> = None;

    for (i, line) in raw_lines.iter().enumerate() {
        let trimmed = line.trim();

        if let Some((tag, _)) = open {
            // Внутри уже открытого блока распознаётся только парная
            // закрывающая строка с тем же именем тега — вложенности у
            // inline-блоков не бывает, и текст, похожий на другой тег или
            // на наш маркер, здесь — не более чем PEM-данные.
            if inline_tag_close(trimmed) == Some(tag) {
                open = None;
            }
            result.push(LineKind::Inline(line));
            continue;
        }

        if trimmed == BEGIN_MARKER {
            result.push(LineKind::Begin);
            continue;
        }
        if trimmed == END_MARKER {
            result.push(LineKind::End);
            continue;
        }
        if let Some(tag) = inline_tag_open(trimmed) {
            open = Some((tag, i));
            result.push(LineKind::Inline(line));
            continue;
        }
        result.push(LineKind::TopLevel(line));
    }

    if let Some((tag, line)) = open {
        return Err(ProfileError::UnterminatedInlineBlock {
            tag: tag.to_string(),
            line: line + 1, // строки нумеруются с 1 для человека
        });
    }
    Ok(result)
}

/// Строка вида `<tag>`, возможно с хвостовым комментарием (`<ca> # заметка`)
/// — так же, как `block-outside-dns` и `redirect-gateway` терпят хвостовой
/// комментарий (раунды 1 и 2), открывающий тег обязан распознаваться и с
/// ним: иначе `<ca> # заметка` не считается началом блока, а закрывающий
/// `</ca>` его всё равно закрывает — и то, что лежит между ними, вычищается
/// как «наш» диапазон, если туда попали текст наших маркеров.
fn inline_tag_open(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix('<')?;
    if rest.starts_with('/') {
        return None;
    }
    let end = rest.find('>')?;
    let name = &rest[..end];
    is_tag_name(name).then_some(name)
}

/// Строка вида `</tag>`, тем же допуском на хвостовой комментарий, что и
/// у открывающей — симметрично `inline_tag_open`: раунд 2 сломался именно
/// на том, что открывающая и закрывающая проверки требовали разного.
fn inline_tag_close(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("</")?;
    let end = rest.find('>')?;
    let name = &rest[..end];
    is_tag_name(name).then_some(name)
}

/// Имя inline-тега OpenVPN: непустая последовательность ASCII-букв, цифр,
/// `-` и `_` (`ca`, `tls-auth`, `http_proxy_user_pass`, ...).
fn is_tag_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Для каждой позиции решает, войдёт ли строка в сохранённую часть.
/// Работает только с уже готовой классификацией [`LineKind`] — сама не
/// разбирает, что такое `<ca>` или наш маркер, это сделал `classify_lines`.
///
/// Непарный `Begin`/`End` (одинокий `BEGIN` без `END`, одинокий `END` без
/// предшествующего `BEGIN`, более ранний `BEGIN`, вытесненный более
/// поздним до появления `END`) вычищается сам, одной строкой — не как
/// начало диапазона до первого попавшегося чужого маркера. Раунд 1 держал
/// такую сироту нетронутой как обычную строку, и она спаривалась с нашим
/// же добавленным `END` на следующей сборке, стирая всё между ними.
fn drop_mask(classified: &[LineKind]) -> Vec<bool> {
    let mut drop = vec![false; classified.len()];
    let mut pending_begin: Option<usize> = None;

    for (i, kind) in classified.iter().enumerate() {
        match kind {
            LineKind::Begin => {
                if let Some(prev) = pending_begin {
                    drop[prev] = true;
                }
                pending_begin = Some(i);
            }
            LineKind::End => match pending_begin {
                Some(begin) => {
                    for slot in drop.iter_mut().take(i + 1).skip(begin) {
                        *slot = true;
                    }
                    pending_begin = None;
                }
                None => drop[i] = true,
            },
            LineKind::TopLevel(line) => {
                if is_block_outside_dns_directive(line) {
                    drop[i] = true;
                }
            }
            LineKind::Inline(_) => {}
        }
    }
    if let Some(begin) = pending_begin {
        drop[begin] = true;
    }
    drop
}

/// Схлопывает повторяющиеся пробелы и отрезает хвостовой комментарий
/// (`#`/`;`) перед сравнением с канонической записью директивы. Общая
/// процедура для `block-outside-dns` и для `redirect-gateway`-фильтра —
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

    fn build(source: &str) -> String {
        build_profile(source, &routes()).expect("источник этого теста обязан собраться")
    }

    /// Строит профиль `n` раз подряд, каждый раз над результатом
    /// предыдущего — ровно то, что задача 5 будет делать при каждой смене
    /// списка офисных подсетей.
    fn build_chain(source: &str, n: usize) -> Vec<String> {
        let mut chain = Vec::with_capacity(n);
        let mut current = source.to_string();
        for _ in 0..n {
            let out = build(&current);
            chain.push(out.clone());
            current = out;
        }
        chain
    }

    #[test]
    fn redirect_gateway_is_filtered() {
        let out = build(SOURCE);
        assert!(out.contains(r#"pull-filter ignore "redirect-gateway""#));
    }

    #[test]
    fn every_route_is_present() {
        let out = build(SOURCE);
        assert!(out.contains("route 203.0.113.0 255.255.255.0"));
        assert!(out.contains("route 198.51.100.0 255.255.255.0"));
    }

    #[test]
    fn source_lines_survive() {
        let out = build(SOURCE);
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
        let out = build(SOURCE);
        assert!(!out.contains("block-outside-dns"));
    }

    #[test]
    fn block_outside_dns_with_extra_whitespace_is_stripped() {
        let source = SOURCE.replace(
            "setenv opt block-outside-dns",
            "setenv  opt   block-outside-dns",
        );
        let out = build(&source);
        assert!(!out.contains("block-outside-dns"));
    }

    #[test]
    fn block_outside_dns_with_a_trailing_comment_is_stripped() {
        let source = SOURCE.replace(
            "setenv opt block-outside-dns",
            "setenv opt block-outside-dns # не наше, но клиент ругается",
        );
        let out = build(&source);
        assert!(!out.contains("block-outside-dns"));
    }

    #[test]
    fn pushed_dns_is_not_filtered() {
        // Осознанное расхождение с macOS-версией (спека 8.2): пушенный
        // DNS принимаем, иначе внутренние имена не резолвятся.
        let out = build(SOURCE);
        assert!(!out.contains(r#"dhcp-option DNS"#));
    }

    #[test]
    fn building_twice_does_not_duplicate_directives() {
        let chain = build_chain(SOURCE, 2);
        assert_eq!(
            chain[1]
                .matches(r#"pull-filter ignore "redirect-gateway""#)
                .count(),
            1
        );
        assert_eq!(
            chain[1].matches("route 203.0.113.0 255.255.255.0").count(),
            1
        );
        assert_eq!(
            chain[1].matches("route 198.51.100.0 255.255.255.0").count(),
            1
        );
    }

    #[test]
    fn empty_routes_still_adds_the_filter() {
        let out = build_profile(SOURCE, &[]).unwrap();
        assert!(out.contains(r#"pull-filter ignore "redirect-gateway""#));
    }

    #[test]
    fn an_unbalanced_begin_marker_does_not_truncate_the_profile() {
        // Обрезанный (или вручную подправленный) прошлый результат: BEGIN
        // есть, END потерялся. Раунд 1 чинил только первую сборку — второй
        // вызов (обычный ход дел в задаче 5, пересобирающей профиль при
        // каждой смене офисных подсетей) снова терял сертификат, потому что
        // добавленный нами END на первом проходе спаривался с чужим,
        // непарным BEGIN на втором. Проверяем три прохода подряд, не только
        // первые два.
        let source = format!("client\n{BEGIN_MARKER}\n<ca>\nCERT\n</ca>\n");
        let chain = build_chain(&source, 3);
        for (n, out) in chain.iter().enumerate() {
            assert!(out.contains("CERT"), "сборка {} потеряла CERT", n + 1);
            assert!(out.contains("<ca>"));
            assert!(out.contains("</ca>"));
        }
        assert_eq!(chain[1], chain[2], "не устоялось ко второй сборке");
    }

    #[test]
    fn a_source_that_is_only_a_begin_marker_is_a_fixpoint() {
        // Источник целиком — один одинокий BEGIN без END.
        let source = format!("{BEGIN_MARKER}\n");
        let chain = build_chain(&source, 3);
        assert_eq!(chain[0], chain[1]);
        assert_eq!(chain[1], chain[2]);
    }

    #[test]
    fn markers_inside_an_inline_block_do_not_delete_its_content() {
        // Маркеры, оказавшиеся внутри <ca>...</ca> (например, после ручной
        // правки источника или совпадения в самих PEM-данных), — не наш
        // блок: это непрозрачное содержимое сертификата, и пара между ними
        // может быть настоящим текстом ключа, а не сгенерированными нами
        // директивами.
        let source = format!("client\n<ca>\n{BEGIN_MARKER}\nCERT-D\n{END_MARKER}\n</ca>\n");
        let chain = build_chain(&source, 3);
        for (n, out) in chain.iter().enumerate() {
            assert!(out.contains("CERT-D"), "сборка {} потеряла CERT-D", n + 1);
            assert!(out.contains("<ca>"));
            assert!(out.contains("</ca>"));
        }
        assert_eq!(chain[1], chain[2]);
    }

    #[test]
    fn an_existing_redirect_gateway_filter_outside_the_block_is_not_duplicated() {
        let source = format!("{SOURCE}pull-filter ignore \"redirect-gateway\"\n");
        let out = build(&source);
        assert_eq!(
            out.matches(r#"pull-filter ignore "redirect-gateway""#)
                .count(),
            1
        );
    }

    #[test]
    fn an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated() {
        let source = format!("{SOURCE}pull-filter  ignore \"redirect-gateway\"\n");
        let out = build(&source);
        assert_eq!(out.matches("redirect-gateway").count(), 1);
    }

    #[test]
    fn a_redirect_gateway_filter_inside_a_connection_block_does_not_suppress_the_top_level_one() {
        // Round 3, находка C: `pull-filter ignore "redirect-gateway"`
        // внутри <connection> — валидный OpenVPN, но не top-level. Если
        // проверка не знает про inline-блоки, она молча решает, что фильтр
        // уже стоит, и не добавляет наш — а это именно то, что спека 8.1
        // называет причиной существования этой строки: весь трафик уходит
        // в туннель кругом через офис, и состояние после этого — устойчивая
        // неподвижная точка, которая сама себя не починит.
        let source =
            "client\n<connection>\npull-filter ignore \"redirect-gateway\"\n</connection>\n";
        let chain = build_chain(source, 2);
        for out in &chain {
            assert_eq!(
                out.matches(r#"pull-filter ignore "redirect-gateway""#)
                    .count(),
                2,
                "top-level фильтр обязан быть добавлен рядом с тем, что внутри <connection>"
            );
            assert!(out.contains("<connection>"));
            assert!(out.contains("</connection>"));
        }
    }

    #[test]
    fn an_open_tag_with_a_trailing_comment_still_protects_its_contents() {
        // Round 3, находка B: `is_inline_block_open`/`_close` раунда 2
        // требовали от открывающей строки заканчиваться на `>`, а
        // закрывающая была не так строга — `<ca> # заметка` не считался
        // открытием блока, но `</ca>` его всё равно закрывал, и всё между
        // ними вычищалось как «наш» диапазон, если туда попадал текст,
        // похожий на маркер.
        let source =
            format!("client\n<ca> # сертификат\n{BEGIN_MARKER}\nCERT3\n{END_MARKER}\n</ca>\n");
        let chain = build_chain(&source, 3);
        for (n, out) in chain.iter().enumerate() {
            assert!(out.contains("CERT3"), "сборка {} потеряла CERT3", n + 1);
            assert!(out.contains("<ca> # сертификат"));
            assert!(out.contains("</ca>"));
        }
    }

    #[test]
    fn an_unterminated_inline_block_is_rejected_not_silently_mangled() {
        // Round 3, находка A: раньше незакрытый <ca> оставлял
        // `in_inline_block` взведённым до конца файла, наш собственный
        // хвостовой блок становился невидим для распознавания маркеров, и
        // при каждой следующей сборке дописывался ещё один — раздувая
        // список маршрутов без предела (в пробнике ревью — 6 сборок дали
        // route-count 6). Теперь это ошибка с первой же попытки.
        let source = "client\n<ca>\nCERT\n";
        let err = build_profile(source, &routes()).expect_err("незакрытый блок обязан отказать");
        match &err {
            ProfileError::UnterminatedInlineBlock { tag, line } => {
                assert_eq!(tag, "ca");
                assert_eq!(*line, 2);
            }
        }
        let message = err.to_string();
        assert!(message.contains("ca"));
        assert!(message.contains('2'));
    }

    #[test]
    fn repeatedly_building_from_an_unterminated_block_keeps_failing_the_same_way() {
        // Не растит route-count по кругу и не начинает вдруг собираться —
        // отказ детерминирован и стабилен.
        let source = "client\n<ca>\nCERT\n";
        let first = build_profile(source, &routes());
        let second = build_profile(source, &routes());
        assert_eq!(first, second);
        assert!(first.is_err());
    }

    #[test]
    fn route_host_bits_are_masked_even_when_ipv4net_is_built_directly() {
        // В обход FromStr (который уже маскирует сам) — напрямую через
        // публичные поля, как это потенциально сделают задачи 3/5/7.
        let route = Ipv4Net {
            addr: "203.0.113.5".parse().unwrap(),
            prefix: 24,
        };
        let out = build_profile(SOURCE, &[route]).unwrap();
        assert!(out.contains("route 203.0.113.0 255.255.255.0"));
        assert!(!out.contains("203.0.113.5"));
    }

    #[test]
    fn crlf_source_stays_crlf() {
        let source = SOURCE.replace('\n', "\r\n");
        let out = build(&source);
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
        let out = build(source);
        assert!(out.starts_with("client\r\ndev tun\r\n"));
    }

    #[test]
    fn an_inline_block_survives_a_crlf_source_across_three_builds() {
        let source = "client\r\n<ca>\r\nCERT\r\n</ca>\r\n";
        let chain = build_chain(source, 3);
        for out in &chain {
            assert!(out.contains("CERT"));
            assert!(out.contains("<ca>"));
            assert!(out.contains("</ca>"));
        }
    }

    #[test]
    fn empty_source_has_no_leading_blank_line() {
        let out = build_profile("", &routes()).unwrap();
        assert!(out.starts_with(BEGIN_MARKER));
    }

    /// Прямые пробы классификатора — единственной точки, которая решает,
    /// что такое строка. По просьбе ревью: весь тот же набор
    /// маркер-раскладок, который раньше проверялся косвенно через
    /// `build_profile`, здесь проверяется напрямую по `classify_lines`, не
    /// смешиваясь с форматированием вывода и списком маршрутов.
    mod classify {
        use super::*;

        fn classify(source: &str) -> Vec<LineKind<'_>> {
            let lines: Vec<&str> = source.lines().collect();
            classify_lines(&lines).expect("источник этого теста обязан разобраться")
        }

        #[test]
        fn an_empty_source_classifies_to_nothing() {
            assert!(classify("").is_empty());
        }

        #[test]
        fn a_lone_begin_is_begin() {
            assert_eq!(classify(BEGIN_MARKER), vec![LineKind::Begin]);
        }

        #[test]
        fn a_lone_end_is_end() {
            assert_eq!(classify(END_MARKER), vec![LineKind::End]);
        }

        #[test]
        fn a_paired_block_is_begin_then_end() {
            let source = format!("{BEGIN_MARKER}\n{END_MARKER}");
            assert_eq!(classify(&source), vec![LineKind::Begin, LineKind::End]);
        }

        #[test]
        fn two_begins_then_an_end_are_all_markers() {
            let source = format!("{BEGIN_MARKER}\n{BEGIN_MARKER}\n{END_MARKER}");
            assert_eq!(
                classify(&source),
                vec![LineKind::Begin, LineKind::Begin, LineKind::End]
            );
        }

        #[test]
        fn begin_end_begin_are_all_markers() {
            let source = format!("{BEGIN_MARKER}\n{END_MARKER}\n{BEGIN_MARKER}");
            assert_eq!(
                classify(&source),
                vec![LineKind::Begin, LineKind::End, LineKind::Begin]
            );
        }

        #[test]
        fn end_before_begin_are_both_markers() {
            let source = format!("{END_MARKER}\n{BEGIN_MARKER}");
            assert_eq!(classify(&source), vec![LineKind::End, LineKind::Begin]);
        }

        #[test]
        fn markers_tolerate_surrounding_whitespace() {
            let source = format!("   {BEGIN_MARKER}   \n\t{END_MARKER}\t");
            let classified = classify(&source);
            assert_eq!(classified, vec![LineKind::Begin, LineKind::End]);
        }

        #[test]
        fn a_marker_as_a_substring_is_not_recognised() {
            let source = format!("xx{BEGIN_MARKER}\n{END_MARKER}yy");
            let classified = classify(&source);
            assert_eq!(classified.len(), 2);
            assert!(matches!(classified[0], LineKind::TopLevel(_)));
            assert!(matches!(classified[1], LineKind::TopLevel(_)));
        }

        #[test]
        fn a_tag_lookalike_inside_an_open_block_does_not_start_a_nested_block() {
            let source = "<ca>\n<cert>\nPEM\n</ca>\n";
            let classified = classify(source);
            assert_eq!(classified.len(), 4);
            for kind in &classified {
                assert!(matches!(kind, LineKind::Inline(_)));
            }
        }

        #[test]
        fn an_open_tag_with_a_trailing_comment_is_recognised() {
            let source = "<ca> # сертификат\nPEM\n</ca>\n";
            let classified = classify(source);
            assert_eq!(classified.len(), 3);
            for kind in &classified {
                assert!(matches!(kind, LineKind::Inline(_)));
            }
        }

        #[test]
        fn a_close_tag_with_a_trailing_comment_is_recognised() {
            let source = "<ca>\nPEM\n</ca> # конец\ntop-level-again\n";
            let classified = classify(source);
            assert_eq!(classified.len(), 4);
            assert!(matches!(classified[0], LineKind::Inline(_)));
            assert!(matches!(classified[1], LineKind::Inline(_)));
            assert!(matches!(classified[2], LineKind::Inline(_)));
            assert!(matches!(classified[3], LineKind::TopLevel(_)));
        }

        #[test]
        fn a_mismatched_close_tag_does_not_close_the_block() {
            // </cert> внутри открытого <ca> — не пара; блок остаётся
            // открытым до настоящего </ca>.
            let source = "<ca>\n</cert>\nPEM\n</ca>\ntop-level\n";
            let classified = classify(source);
            assert_eq!(classified.len(), 5);
            assert!(matches!(classified[0], LineKind::Inline(_)));
            assert!(matches!(classified[1], LineKind::Inline(_)));
            assert!(matches!(classified[2], LineKind::Inline(_)));
            assert!(matches!(classified[3], LineKind::Inline(_)));
            assert!(matches!(classified[4], LineKind::TopLevel(_)));
        }

        #[test]
        fn an_unterminated_block_is_an_error_naming_the_tag_and_line() {
            let source = "client\n<ca>\nCERT\n";
            let lines: Vec<&str> = source.lines().collect();
            let err = classify_lines(&lines).unwrap_err();
            match err {
                ProfileError::UnterminatedInlineBlock { tag, line } => {
                    assert_eq!(tag, "ca");
                    assert_eq!(line, 2);
                }
            }
        }

        #[test]
        fn a_whole_previously_built_profile_re_classifies_cleanly() {
            // Целиком уже собранный профиль (наш блок плюс исходник),
            // поданный заново, — обычный случай пересборки задачи 5.
            let built = build(SOURCE);
            let lines: Vec<&str> = built.lines().collect();
            let classified = classify_lines(&lines).expect("собственный вывод обязан разбираться");
            assert!(classified.iter().any(|k| matches!(k, LineKind::Begin)));
            assert!(classified.iter().any(|k| matches!(k, LineKind::End)));
        }
    }
}
