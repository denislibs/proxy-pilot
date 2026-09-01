//! Минимальный разбор JSON — ровно то, что нужно [`super::source`] для
//! ответа GitHub Releases API, и ни строки больше.
//!
//! Отдельного крейта (`serde_json`) в дереве нет: в этой песочнице сборки
//! `cargo` не может обратиться к `crates.io` за НОВОЙ зависимостью (проверено
//! вживую — `cargo check --offline` при добавленном `serde_json` отказывает
//! «no matching package», а онлайн-попытка виснет на проверке отзыва
//! сертификата), а тащить в дерево, которое рано или поздно придётся
//! подписывать, зависимость ради разбора двух строковых полей — то самое
//! неоправданное усложнение, от которого `bench.rs` уже отказался ради
//! своего единственного статического `GET` (см. докблок `bridge::bench`).
//!
//! Разбор полный (объекты, массивы, строки со стандартными escape-
//! последовательностями, числа, `true`/`false`/`null`), а не «найти
//! подстроку `"tag_name":`» по двум причинам: порядок полей в ответе API не
//! гарантирован контрактом, а значения — не наши (сеть недоверенная целиком),
//! и строка типа `"name": "\"tag_name\": подделка"` внутри чужого поля не
//! должна путать поиск нужного.
//!
//! **Вход — байты из сети, а не доверенный текст.** `parse_value` ↔
//! `parse_object`/`parse_array` взаимно рекурсивны по глубине вложенности
//! JSON, и без предела эта глубина — это глубина СТЕКА ВЫЗОВОВ. Тело вида
//! `[[[[[…` с достаточным числом скобок переполнило бы стек; переполнение
//! стека в Rust — это `abort`, а не `panic`, оно НЕ разворачивает стек и не
//! запускает ни один `Drop` — в частности, `RestoreOnDrop` (`proxy.rs`),
//! чья единственная работа — вернуть системный прокси на выход из процесса.
//! Процесс, упавший так, оставляет реестр указывающим на `127.0.0.1:PORT`,
//! где никто больше не слушает, — то самое состояние, ради недопущения
//! которого весь этот сторож и заведён (`CLAUDE.md`, «Любой путь завершения
//! процесса восстанавливает системный прокси»). Поэтому [`MAX_DEPTH`] —
//! обязательный параметр, а не защита «на всякий случай»: без него ответ
//! от сети мог бы обрушить весь продукт способом, которого штатный сторож
//! не видит и не может увидеть.
//!
//! Вторая половина того же требования — **ни один вход не паникует**, а не
//! только не переполняет стек: усечённый на середине токена ответ, битый
//! `\uXXXX`, голый `-` без цифр, одинокий суррогат — всё это `Err`, не
//! `unwrap`/индексация мимо границ. Тесты `hostile` внизу гоняют это не
//! горсткой отобранных случаев, а генерируемыми и мутированными строками:
//! только «не паникует» здесь ценнее десятка удачно угаданных примеров,
//! потому что вход не наш и предугадать его форму нельзя в принципе.

use std::collections::BTreeMap;

/// Потолок вложенности объектов/массивов друг в друга. Ответ GitHub Releases
/// API вкладывает не больше 3-4 уровней (объект релиза → массив `assets` →
/// объект ассета → изредка вложенный `uploader`); 32 — генеральский запас
/// поверх этого, а не тесная подгонка под конкретный ответ, и всё ещё
/// исчезающе маленькая глубина рекурсии для любого стека потока (в том
/// числе для `spawn_blocking`, откуда этот разбор и вызывается).
const MAX_DEPTH: u32 = 32;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    // BTreeMap, а не Vec<(String, Json)>: доступ по ключу нужен многократно
    // (`get`), а порядок полей ответа API для наших нужд не важен —
    // сохранять его было бы работой без потребителя.
    Object(BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(m) => m.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }
}

/// Разбирает JSON целиком; лишний хвост после значения (например, вторая
/// строка ответа) — тоже ошибка: незачем молча принимать то, что не отдал бы
/// ни один настоящий парсер.
pub fn parse(text: &str) -> Result<Json, String> {
    let bytes = text.as_bytes();
    let mut pos = skip_ws(bytes, 0);
    let (value, next) = parse_value(bytes, pos, 0)?;
    pos = skip_ws(bytes, next);
    if pos != bytes.len() {
        return Err(format!("лишние данные после значения на байте {pos}"));
    }
    Ok(value)
}

fn skip_ws(b: &[u8], mut pos: usize) -> usize {
    while pos < b.len() && matches!(b[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

/// `depth` — сколько объектов/массивов уже открыто СНАРУЖИ этого значения
/// (0 на верхнем уровне). Растёт только здесь, в единственной точке, откуда
/// разбор уходит вглубь ([`parse_object`]/[`parse_array`]) — оба они лишь
/// передают то же самое `depth + 1` каждому вложенному значению, ни разу не
/// увеличивая его сами, поэтому предел проверяется ровно один раз на
/// уровень, а не может быть случайно обойдён вторым местом инкремента.
fn parse_value(b: &[u8], pos: usize, depth: u32) -> Result<(Json, usize), String> {
    let pos = skip_ws(b, pos);
    match b.get(pos) {
        Some(b'{') | Some(b'[') if depth >= MAX_DEPTH => Err(format!(
            "JSON вложен глубже {MAX_DEPTH} уровней на байте {pos} — отклонено"
        )),
        Some(b'{') => parse_object(b, pos, depth + 1),
        Some(b'[') => parse_array(b, pos, depth + 1),
        Some(b'"') => {
            let (s, next) = parse_string(b, pos)?;
            Ok((Json::String(s), next))
        }
        Some(b't') if b[pos..].starts_with(b"true") => Ok((Json::Bool(true), pos + 4)),
        Some(b'f') if b[pos..].starts_with(b"false") => Ok((Json::Bool(false), pos + 5)),
        Some(b'n') if b[pos..].starts_with(b"null") => Ok((Json::Null, pos + 4)),
        Some(c) if c.is_ascii_digit() || *c == b'-' => parse_number(b, pos),
        _ => Err(format!("неожиданный символ на байте {pos}")),
    }
}

fn parse_object(b: &[u8], mut pos: usize, depth: u32) -> Result<(Json, usize), String> {
    pos += 1; // '{'
    let mut map = BTreeMap::new();
    pos = skip_ws(b, pos);
    if b.get(pos) == Some(&b'}') {
        return Ok((Json::Object(map), pos + 1));
    }
    loop {
        pos = skip_ws(b, pos);
        if b.get(pos) != Some(&b'"') {
            return Err(format!("ожидался ключ-строка на байте {pos}"));
        }
        let (key, next) = parse_string(b, pos)?;
        pos = skip_ws(b, next);
        if b.get(pos) != Some(&b':') {
            return Err(format!("ожидалось «:» на байте {pos}"));
        }
        pos += 1;
        let (value, next) = parse_value(b, pos, depth)?;
        map.insert(key, value);
        pos = skip_ws(b, next);
        match b.get(pos) {
            Some(b',') => {
                pos += 1;
            }
            Some(b'}') => return Ok((Json::Object(map), pos + 1)),
            _ => return Err(format!("ожидалось «,» или «}}» на байте {pos}")),
        }
    }
}

fn parse_array(b: &[u8], mut pos: usize, depth: u32) -> Result<(Json, usize), String> {
    pos += 1; // '['
    let mut items = Vec::new();
    pos = skip_ws(b, pos);
    if b.get(pos) == Some(&b']') {
        return Ok((Json::Array(items), pos + 1));
    }
    loop {
        let (value, next) = parse_value(b, pos, depth)?;
        items.push(value);
        pos = skip_ws(b, next);
        match b.get(pos) {
            Some(b',') => {
                pos += 1;
            }
            Some(b']') => return Ok((Json::Array(items), pos + 1)),
            _ => return Err(format!("ожидалось «,» или «]» на байте {pos}")),
        }
    }
}

/// Строка с обязательными кавычками по обе стороны и стандартными
/// escape-последовательностями JSON (`\"`, `\\`, `\/`, `\n`, `\t`, `\r`,
/// `\b`, `\f`, `\uXXXX`). Суррогатные пары `\uXXXX\uXXXX` собираются в один
/// символ — без этого имя релиза с эмодзи или кириллицей в escaped-виде
/// разобралось бы в мусор вместо текста.
fn parse_string(b: &[u8], pos: usize) -> Result<(String, usize), String> {
    let mut pos = pos + 1; // открывающая кавычка
    let mut out = String::new();
    let mut pending_high_surrogate: Option<u16> = None;
    loop {
        let Some(&c) = b.get(pos) else {
            return Err("строка оборвалась до закрывающей кавычки".to_string());
        };
        match c {
            b'"' => {
                if pending_high_surrogate.is_some() {
                    out.push('\u{FFFD}');
                }
                return Ok((out, pos + 1));
            }
            b'\\' => {
                let Some(&esc) = b.get(pos + 1) else {
                    return Err("escape-последовательность оборвалась".to_string());
                };
                match esc {
                    b'"' => {
                        out.push('"');
                        pos += 2;
                    }
                    b'\\' => {
                        out.push('\\');
                        pos += 2;
                    }
                    b'/' => {
                        out.push('/');
                        pos += 2;
                    }
                    b'n' => {
                        out.push('\n');
                        pos += 2;
                    }
                    b't' => {
                        out.push('\t');
                        pos += 2;
                    }
                    b'r' => {
                        out.push('\r');
                        pos += 2;
                    }
                    b'b' => {
                        out.push('\u{8}');
                        pos += 2;
                    }
                    b'f' => {
                        out.push('\u{c}');
                        pos += 2;
                    }
                    b'u' => {
                        let hex = b
                            .get(pos + 2..pos + 6)
                            .ok_or("«\\u» без четырёх шестнадцатеричных знаков")?;
                        let hex = std::str::from_utf8(hex)
                            .map_err(|_| "«\\uXXXX» не ASCII".to_string())?;
                        let unit = u16::from_str_radix(hex, 16)
                            .map_err(|_| "«\\uXXXX» не шестнадцатеричное число".to_string())?;
                        pos += 6;
                        if let Some(high) = pending_high_surrogate.take() {
                            if (0xDC00..=0xDFFF).contains(&unit) {
                                let c = 0x10000
                                    + ((high as u32 - 0xD800) << 10)
                                    + (unit as u32 - 0xDC00);
                                out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                            } else {
                                // Высокий суррогат без пары — не наш случай
                                // (ответ API этого не порождает), но и
                                // ронять разбор из-за него незачем.
                                out.push('\u{FFFD}');
                                if let Some(c) = char::from_u32(unit as u32) {
                                    out.push(c);
                                }
                            }
                        } else if (0xD800..=0xDBFF).contains(&unit) {
                            pending_high_surrogate = Some(unit);
                        } else if let Some(c) = char::from_u32(unit as u32) {
                            out.push(c);
                        }
                    }
                    other => {
                        return Err(format!("неизвестный escape «\\{}»", other as char));
                    }
                }
            }
            _ => {
                // Байты UTF-8 копируются как есть — ответ API уже валидный
                // UTF-8 (HTTP-уровень это гарантирует через `Content-Type`),
                // многобайтовые последовательности не escape-ятся отдельно.
                let ch_len = utf8_len(c);
                let Some(slice) = b.get(pos..pos + ch_len) else {
                    return Err("оборванная UTF-8 последовательность в строке".to_string());
                };
                let s = std::str::from_utf8(slice)
                    .map_err(|_| "невалидный UTF-8 в строке".to_string())?;
                out.push_str(s);
                pos += ch_len;
            }
        }
    }
}

fn utf8_len(first_byte: u8) -> usize {
    if first_byte & 0x80 == 0 {
        1
    } else if first_byte & 0xE0 == 0xC0 {
        2
    } else if first_byte & 0xF0 == 0xE0 {
        3
    } else {
        4
    }
}

fn parse_number(b: &[u8], pos: usize) -> Result<(Json, usize), String> {
    let start = pos;
    let mut pos = pos;
    if b.get(pos) == Some(&b'-') {
        pos += 1;
    }
    while b.get(pos).is_some_and(u8::is_ascii_digit) {
        pos += 1;
    }
    if b.get(pos) == Some(&b'.') {
        pos += 1;
        while b.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
    }
    if matches!(b.get(pos), Some(b'e' | b'E')) {
        pos += 1;
        if matches!(b.get(pos), Some(b'+' | b'-')) {
            pos += 1;
        }
        while b.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
    }
    let text = std::str::from_utf8(&b[start..pos]).map_err(|_| "число не ASCII".to_string())?;
    let n: f64 = text.parse().map_err(|_| format!("не число: «{text}»"))?;
    Ok((Json::Number(n), pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_empty_object() {
        assert_eq!(parse("{}").unwrap(), Json::Object(BTreeMap::new()));
    }

    #[test]
    fn parses_a_flat_object() {
        let v = parse(r#"{"tag_name": "v1.2.3", "draft": false}"#).unwrap();
        assert_eq!(v.get("tag_name").and_then(Json::as_str), Some("v1.2.3"));
        assert_eq!(v.get("draft"), Some(&Json::Bool(false)));
    }

    #[test]
    fn parses_nested_arrays_of_objects_like_the_real_api_shape() {
        let v = parse(
            r#"{
                "tag_name": "v0.2.0",
                "prerelease": false,
                "assets": [
                    {"name": "proxypilot-bridge.exe", "browser_download_url": "https://example.internal/bridge.exe"},
                    {"name": "proxypilot.exe", "browser_download_url": "https://example.internal/app.exe"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(v.get("tag_name").and_then(Json::as_str), Some("v0.2.0"));
        let assets = v.get("assets").and_then(Json::as_array).unwrap();
        assert_eq!(assets.len(), 2);
        let app = assets
            .iter()
            .find(|a| a.get("name").and_then(Json::as_str) == Some("proxypilot.exe"))
            .expect("наш ассет обязан найтись");
        assert_eq!(
            app.get("browser_download_url").and_then(Json::as_str),
            Some("https://example.internal/app.exe")
        );
    }

    #[test]
    fn unescapes_standard_sequences() {
        let v = parse(r#""line1\nline2\t\"quoted\"\\end""#).unwrap();
        assert_eq!(v.as_str(), Some("line1\nline2\t\"quoted\"\\end"));
    }

    #[test]
    fn unescapes_unicode_code_points() {
        // Кириллица через \uXXXX — так GitHub API отдаёт незаэкранированный
        // Unicode в некоторых клиентах/прокси.
        let v = parse(r#""Привет""#).unwrap();
        assert_eq!(v.as_str(), Some("Привет"));
    }

    #[test]
    fn unescapes_a_surrogate_pair() {
        // Эмодзи вне BMP — суррогатная пара.
        let v = parse(r#""😀""#).unwrap();
        assert_eq!(v.as_str(), Some("😀"));
    }

    #[test]
    fn a_literal_multibyte_utf8_string_is_copied_through() {
        let v = parse("\"Привет 😀\"").unwrap();
        assert_eq!(v.as_str(), Some("Привет 😀"));
    }

    #[test]
    fn rejects_truncated_json() {
        assert!(parse(r#"{"tag_name": "v1.0.0""#).is_err());
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse(r#"{"a": 1} garbage"#).is_err());
    }

    #[test]
    fn rejects_an_unterminated_string() {
        assert!(parse(r#"{"a": "unterminated"#).is_err());
    }

    #[test]
    fn parses_numbers_bool_and_null() {
        let v = parse(r#"{"n": -12.5e2, "t": true, "f": false, "z": null}"#).unwrap();
        assert_eq!(v.get("n"), Some(&Json::Number(-1250.0)));
        assert_eq!(v.get("t"), Some(&Json::Bool(true)));
        assert_eq!(v.get("f"), Some(&Json::Bool(false)));
        assert_eq!(v.get("z"), Some(&Json::Null));
    }

    #[test]
    fn a_confusing_string_field_does_not_fool_key_lookup() {
        // Строка внутри ЧУЖОГО поля, текстуально похожая на разметку
        // другого ключа, не должна подменить настоящее значение
        // `tag_name` — именно то, ради чего разбор полный, а не поиск
        // подстроки.
        let v = parse(r#"{"name": "\"tag_name\": \"fake\"", "tag_name": "v1.0.0"}"#).unwrap();
        assert_eq!(v.get("tag_name").and_then(Json::as_str), Some("v1.0.0"));
    }

    #[test]
    fn an_empty_array_response_parses_as_an_array() {
        // Форма реального ответа GitHub для репозитория без релизов
        // (`/releases`, не `/releases/latest`) — проверено вживую.
        let v = parse("[]").unwrap();
        assert_eq!(v.as_array(), Some(&[][..]));
    }

    // ---- Fix round 1 (задача 3): предел глубины и «враждебные» входы ----
    //
    // Вход сюда приходит из сети (`source::GithubSource`), поэтому не
    // «правильный формат, который мы разбираем», а произвольные байты,
    // которые обязаны либо разобраться, либо честно вернуть `Err` — и
    // никогда не запаниковать и не переполнить стек. Переполнение стека
    // здесь — это не «плохой тест», а `abort` без единого `Drop`, включая
    // `RestoreOnDrop`, который единственный возвращает системный прокси на
    // выходе процесса (докблок модуля выше и отчёт задачи).

    #[test]
    fn nesting_exactly_at_the_limit_still_parses() {
        let opens = "[".repeat(MAX_DEPTH as usize);
        let closes = "]".repeat(MAX_DEPTH as usize);
        let doc = format!("{opens}{closes}");
        assert!(parse(&doc).is_ok(), "ровно предел обязан разбираться");
    }

    #[test]
    fn nesting_one_level_past_the_limit_is_a_clean_error() {
        let opens = "[".repeat(MAX_DEPTH as usize + 1);
        let closes = "]".repeat(MAX_DEPTH as usize + 1);
        let doc = format!("{opens}{closes}");
        assert!(
            parse(&doc).is_err(),
            "предел+1 обязан быть отклонён явной ошибкой"
        );
    }

    #[test]
    fn nesting_far_past_the_limit_is_rejected_not_a_stack_overflow() {
        // На три порядка больше предела: если бы предела не существовало,
        // это ровно тот вход, что кладёт процесс `abort`'ом в обход
        // `RestoreOnDrop`. Тест проходит потому, что разбор отказывает
        // задолго до того, как рекурсия успела бы куда-то дотянуться — а
        // не потому, что процесс пережил переполнение (он бы не пережил).
        let opens = "[".repeat(50_000);
        assert!(parse(&opens).is_err());
    }

    #[test]
    fn deeply_nested_objects_are_bounded_the_same_way_as_arrays() {
        // Предел общий на оба вида вложенности — `parse_value` считает
        // глубину один раз для обоих, а не отдельно для `{` и для `[`.
        let mut doc = String::new();
        for i in 0..(MAX_DEPTH as usize + 5) {
            doc.push_str(&format!("{{\"a{i}\":"));
        }
        doc.push_str("null");
        for _ in 0..(MAX_DEPTH as usize + 5) {
            doc.push('}');
        }
        assert!(parse(&doc).is_err());
    }

    #[test]
    fn a_bare_minus_is_a_parse_error_not_a_panic() {
        assert!(parse("-").is_err());
    }

    #[test]
    fn a_lone_high_surrogate_does_not_panic() {
        assert!(parse(r#""\ud800""#).is_ok(), "не должно паниковать");
    }

    #[test]
    fn a_lone_low_surrogate_does_not_panic() {
        assert!(parse(r#""\udc00""#).is_ok(), "не должно паниковать");
    }

    #[test]
    fn a_truncated_unicode_escape_is_an_error_not_a_panic() {
        assert!(parse("\"\\u12").is_err());
        assert!(parse("\"\\u\"").is_err());
        assert!(parse("\"\\uZZZZ\"").is_err());
    }

    #[test]
    fn an_unknown_escape_is_an_error_not_a_panic() {
        assert!(parse(r#""\q""#).is_err());
    }

    #[test]
    fn a_trailing_backslash_is_an_error_not_a_panic() {
        assert!(parse("\"a\\").is_err());
    }

    #[test]
    fn an_absurdly_long_number_does_not_panic() {
        let huge = "9".repeat(400);
        assert!(parse(&huge).is_ok(), "переполнение f64 — не ошибка формата");
    }

    #[test]
    fn truncations_of_a_valid_document_at_every_byte_offset_never_panic() {
        // Строка целиком ASCII — каждый байтовый срез сам по себе валиден
        // как UTF-8, поэтому обрезка на любом байте не падает на индексации
        // ДО того, как вообще дойдёт до разбора; интересует именно разбор.
        let full = r#"{"tag_name": "v1.2.3", "assets": [{"name": "proxypilot.exe", "browser_download_url": "https://example.internal/a.exe", "meta": {"a": [1,2,3], "b": "x\ny", "u": "\ud83d\ude00"}}], "n": -12.5e-2}"#;
        assert!(full.is_ascii(), "тест полагается на срез по любому байту");
        for i in 0..=full.len() {
            let _ = parse(&full[..i]);
        }
    }

    /// Крошечный детерминированный ГСЧ (xorshift64) — свой, а не крейт
    /// `rand`: та же причина, по которой в этом файле нет `serde_json`
    /// (сеть к `crates.io` за новой зависимостью недоступна в этой
    /// песочнице, см. докблок модуля), плюс детерминированность даёт
    /// воспроизводимый прогон без отдельного механизма сохранения seed.
    struct Xorshift64(u64);

    impl Xorshift64 {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    #[test]
    fn random_json_flavoured_byte_soup_never_panics_or_hangs() {
        // Алфавит смещён в сторону синтаксиса JSON (скобки, кавычки,
        // цифры, ключевые слова, escape-символы) — случайные ASCII-буквы
        // почти всегда отвалились бы на первом же байте, а здесь генератор
        // с большей вероятностью попадает в интересные пограничные
        // состояния разбора (незакрытые строки, битые `\u`, обрубленные
        // числа, несбалансированные скобки).
        const ALPHABET: &[u8] = b"{}[]\":,truefalsenull0123456789.-eE \t\\/uU-+";
        let mut rng = Xorshift64(0x9E37_79B9_7F4A_7C15);
        for _ in 0..5_000 {
            let len = (rng.next() % 80) as usize;
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                let idx = (rng.next() as usize) % ALPHABET.len();
                s.push(ALPHABET[idx] as char);
            }
            // Единственное, что здесь проверяется, — что вызов вообще
            // вернулся (не запаниковал, не переполнил стек, не завис):
            // конкретный Ok/Err для мусорного входа не имеет смысла.
            let _ = parse(&s);
        }
    }

    #[test]
    fn random_byte_soup_outside_the_json_alphabet_never_panics() {
        // Второй прогон с полным алфавитом печатных ASCII-байт (не только
        // «похожих на JSON») — ловит то, что первый генератор мог не
        // затронуть чисто по распределению символов.
        let mut rng = Xorshift64(0x2545_F491_4F6C_DD1D);
        for _ in 0..5_000 {
            let len = (rng.next() % 80) as usize;
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                let byte = 0x20 + (rng.next() % 95) as u8; // печатный ASCII
                s.push(byte as char);
            }
            let _ = parse(&s);
        }
    }
}
