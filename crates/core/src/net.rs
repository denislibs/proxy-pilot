//! IPv4-подсеть в нотации CIDR (`10.0.0.0/8`).
//!
//! Живёт в `core`, а не в `winnet`, хотя первый потребитель —
//! split-tunnel профиль OpenVPN (`winnet::ovpn_profile`): тип нужен ещё в
//! трёх местах (конфиг офисных подсетей, bypass-список моста), он
//! платформенно-нейтрален — четыре байта и длина префикса, — а `core`
//! обязан остаться без зависимостей от Windows (CLAUDE.md). Держать его в
//! `winnet` означало бы тянуть Windows-крейт в `core` ради структуры из
//! двух полей.

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Net {
    pub addr: Ipv4Addr,
    pub prefix: u8,
}

/// Сериализуется той же строкой, что и `Display` («10.0.0.0/8»), а не
/// таблицей `{ addr, prefix }` — производный `#[derive(Serialize)]` дал бы
/// именно вложенную таблицу, а конфиг (задача 5) обязан хранить подсеть
/// одной строкой, той же, что печатает страница настроек и что понимает
/// `FromStr`.
impl Serialize for Ipv4Net {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

/// Обратная сторона `Serialize` выше — тем же `FromStr`, что и разбор из
/// текста, поэтому маскировка хостовых битов и вся строгость формата
/// (без `+8`, без `/08`) действуют одинаково что при чтении конфига, что
/// при разборе значения, введённого руками.
impl<'de> Deserialize<'de> for Ipv4Net {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Маска сети, посчитанная из длины префикса — арифметикой, а не таблицей
/// «префикс → маска по целым октетам». На macOS-версии такая таблица была
/// целым классом ошибок: `/14` в ней молча превращался в `/24`, потому что
/// таблица знала только границы октетов. Здесь такой промежуточной
/// структуры нет вовсе — посчитать нечего забыть.
///
/// `bits` сверх 32 не паникует, а насыщается до 32 (маска на один хост):
/// сам тип `Ipv4Net` такое значение не пропустит («PrefixTooLarge»), но
/// функция не обязана разделять с ним предположение о валидности входа.
pub fn mask_of(bits: u8) -> Ipv4Addr {
    let bits = bits.min(32);
    // Сдвиг на 32 — паника в debug-сборке (переполнение сдвига), поэтому
    // /0 считается отдельно, тем же приёмом, что и в bypass.rs.
    let mask: u32 = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - bits)
    };
    Ipv4Addr::from(mask)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Ipv4NetParseError {
    #[error("нет «/» в «{0}» — формат «адрес/длина-префикса»")]
    MissingSlash(String),
    #[error("не разобрался адрес «{0}»")]
    Addr(String),
    #[error("не разобралась длина префикса «{0}»")]
    Prefix(String),
    #[error("длина префикса {0} больше 32")]
    PrefixTooLarge(u8),
}

/// Строгий разбор длины префикса: только ASCII-цифры, без ведущего `+` и
/// без ведущего нуля длиннее одного знака («/08»). Стандартный
/// `u8::from_str` пропускает оба случая молча (`"+8"` и `"08"` обе
/// разбираются в `8`) — а `Display` такую запись никогда не порождает.
/// Раз round-trip обязан быть точным (`FromStr` ↔ `Display`), эквивалентные,
/// но не канонические представления на входе принимать нельзя: иначе один
/// и тот же адрес в конфиге и в UI мог бы напечататься по-разному в
/// зависимости от того, кто его туда записал.
fn parse_prefix(s: &str) -> Result<u8, Ipv4NetParseError> {
    let bad = || Ipv4NetParseError::Prefix(s.to_string());
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    if s.len() > 1 && s.starts_with('0') {
        return Err(bad());
    }
    s.parse::<u8>().map_err(|_| bad())
}

/// `"10.0.0.0/8"` → `Ipv4Net`. Обратное — `Display` ниже; пара обязана
/// быть точным round-trip'ом, потому что этим же текстом подсети хранятся
/// в TOML-конфиге (задача 5).
impl FromStr for Ipv4Net {
    type Err = Ipv4NetParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (addr_s, prefix_s) = s
            .split_once('/')
            .ok_or_else(|| Ipv4NetParseError::MissingSlash(s.to_string()))?;
        let addr = addr_s
            .parse::<Ipv4Addr>()
            .map_err(|_| Ipv4NetParseError::Addr(addr_s.to_string()))?;
        let prefix = parse_prefix(prefix_s)?;
        if prefix > 32 {
            return Err(Ipv4NetParseError::PrefixTooLarge(prefix));
        }
        // Биты хоста маскируются уже на разборе — одно каноническое
        // представление для одной подсети. Без этого «10.1.2.3/24» и
        // «10.1.2.0/24» были бы двумя разными значениями `Ipv4Net` с
        // одинаковым смыслом маршрута, `Display` не был бы предсказуем, а
        // страница настроек (задача 7), эхом показывая понятое значение,
        // молча подменяла бы то, что человек напечатал — тот же класс
        // ошибки, что и «/14 незаметно стало /24» в маске, только на входе.
        //
        // `ovpn_profile::build_profile` (winnet) маскирует адрес ещё раз,
        // при формировании `route` — не потому, что здесь ненадёжно, а
        // потому, что поля `Ipv4Net` публичны и конструктор в обход
        // `FromStr` (`Ipv4Net { addr, prefix }` напрямую) эту маскировку
        // не проходит. Оба места защищаются каждое само за себя.
        let masked = Ipv4Addr::from(u32::from(addr) & u32::from(mask_of(prefix)));
        Ok(Ipv4Net {
            addr: masked,
            prefix,
        })
    }
}

impl fmt::Display for Ipv4Net {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_of_zero_is_all_zero() {
        assert_eq!(mask_of(0), Ipv4Addr::new(0, 0, 0, 0));
    }

    #[test]
    fn mask_of_one_bit() {
        assert_eq!(mask_of(1), Ipv4Addr::new(128, 0, 0, 0));
    }

    #[test]
    fn mask_of_eight_bits_is_a_full_octet() {
        assert_eq!(mask_of(8), Ipv4Addr::new(255, 0, 0, 0));
    }

    #[test]
    fn mask_of_fourteen_bits_does_not_round_to_a_full_octet() {
        // На macOS-версии здесь был класс ошибок: /14 молча превращалось
        // в /24 из-за таблицы масок по целым октетам.
        assert_eq!(mask_of(14), Ipv4Addr::new(255, 252, 0, 0));
    }

    #[test]
    fn mask_of_twenty_four_bits_is_three_full_octets() {
        assert_eq!(mask_of(24), Ipv4Addr::new(255, 255, 255, 0));
    }

    #[test]
    fn mask_of_thirty_one_bits_leaves_a_single_host_bit() {
        assert_eq!(mask_of(31), Ipv4Addr::new(255, 255, 255, 254));
    }

    #[test]
    fn mask_of_thirty_two_bits_is_a_single_host() {
        assert_eq!(mask_of(32), Ipv4Addr::new(255, 255, 255, 255));
    }

    #[test]
    fn parse_and_display_roundtrip() {
        let net = Ipv4Net::from_str("10.0.0.0/8").expect("должен разобраться");
        assert_eq!(net.addr, Ipv4Addr::new(10, 0, 0, 0));
        assert_eq!(net.prefix, 8);
        assert_eq!(net.to_string(), "10.0.0.0/8");
    }

    /// Обёртка нужна только затем, что `toml::to_string` не сериализует
    /// голое значение верхнего уровня — самой `Ipv4Net` это не касается.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Wrapper {
        net: Ipv4Net,
    }

    #[test]
    fn serde_uses_the_same_compact_string_as_display() {
        // Задача 5 хранит подсети в TOML-конфиге этой же строкой — не
        // вложенной таблицей `{ addr, prefix }`, которую дал бы производный
        // `#[derive(Serialize)]`.
        let w = Wrapper {
            net: Ipv4Net::from_str("203.0.113.0/24").expect("должен разобраться"),
        };
        let text = toml::to_string(&w).expect("должен сериализоваться");
        assert_eq!(text.trim(), "net = \"203.0.113.0/24\"");
    }

    #[test]
    fn serde_roundtrips_through_toml() {
        let original = Wrapper {
            net: Ipv4Net::from_str("198.51.100.0/24").expect("должен разобраться"),
        };
        let text = toml::to_string(&original).expect("должен сериализоваться");
        let back: Wrapper = toml::from_str(&text).expect("должен разобраться");
        assert_eq!(back.net, original.net);
    }

    #[test]
    fn serde_rejects_an_invalid_subnet_string() {
        let bad = "net = \"not-a-subnet\"";
        assert!(toml::from_str::<Wrapper>(bad).is_err());
    }

    #[test]
    fn parse_masks_host_bits() {
        // "10.1.2.3/24" несёт биты хоста (".3") — они не часть маршрута.
        let net = Ipv4Net::from_str("10.1.2.3/24").expect("должен разобраться");
        assert_eq!(net.addr, Ipv4Addr::new(10, 1, 2, 0));
        assert_eq!(net.to_string(), "10.1.2.0/24");
    }

    #[test]
    fn parse_accepts_a_bare_zero_prefix() {
        let net = Ipv4Net::from_str("203.0.113.5/0").expect("должен разобраться");
        assert_eq!(net.addr, Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(net.prefix, 0);
    }

    #[test]
    fn parse_rejects_a_prefix_over_thirty_two() {
        assert!(Ipv4Net::from_str("203.0.113.0/33").is_err());
    }

    #[test]
    fn parse_rejects_a_missing_prefix() {
        assert!(Ipv4Net::from_str("203.0.113.0").is_err());
    }

    #[test]
    fn parse_rejects_a_non_numeric_prefix() {
        assert!(Ipv4Net::from_str("203.0.113.0/abc").is_err());
    }

    #[test]
    fn parse_rejects_a_malformed_address() {
        assert!(Ipv4Net::from_str("not-an-address/8").is_err());
    }

    #[test]
    fn parse_rejects_a_leading_plus_prefix() {
        // `u8::from_str("+8")` разбирается молча — здесь это не то же
        // самое, что каноническое "8", и `Display` "+8" никогда не породит.
        assert!(Ipv4Net::from_str("203.0.113.0/+8").is_err());
    }

    #[test]
    fn parse_rejects_a_leading_zero_prefix() {
        assert!(Ipv4Net::from_str("203.0.113.0/08").is_err());
    }

    #[test]
    fn parse_rejects_whitespace_around_the_prefix() {
        assert!(Ipv4Net::from_str("203.0.113.0/ 8").is_err());
    }

    #[test]
    fn parse_rejects_five_octets() {
        assert!(Ipv4Net::from_str("1.2.3.4.5/8").is_err());
    }

    #[test]
    fn parse_rejects_a_prefix_that_does_not_fit_a_byte() {
        assert!(Ipv4Net::from_str("203.0.113.0/256").is_err());
    }

    #[test]
    fn parse_rejects_a_second_slash() {
        assert!(Ipv4Net::from_str("203.0.113.0/8/9").is_err());
    }
}
