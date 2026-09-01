//! Сравнение версии приложения с опубликованным тегом релиза.
//!
//! Тег — `vMAJOR.MINOR.PATCH`, опционально с суффиксом предрелиза через
//! дефис (`v0.2.0-rc.1`): semver-подобно, но не semver целиком — сборочные
//! метаданные (`+...`) не разбираются, потому что конвейер релиза (задача 4,
//! `.github/workflows/release.yml`) их никогда не публикует, а разбирать
//! формат, которым никто не пользуется, незачем.
//!
//! Модуль не делает сетевых запросов и не трогает диск — вся логика здесь
//! чистая, поэтому и проверяется исчерпывающе юнит-тестами (приёмка задачи
//! 3: предрелизы, «текущая новее опубликованной», равенство, мусор в теге,
//! тег, не являющийся версией вовсе).

use std::cmp::Ordering;

/// Версия, разобранная из тега.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Суффикс предрелиза без ведущего дефиса (`"rc.1"`); `None` — «полный»
    /// релиз.
    pub pre: Option<String>,
}

impl Version {
    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }
}

/// Порядок по значимости semver §11: числа старше, а при равных числах
/// релиз БЕЗ предрелиза старше версии с ним (`1.2.3` > `1.2.3-rc.1`); если
/// предрелиз есть у обеих — тай-брейк по строке суффикса. Полной
/// semver-семантики сравнения предрелизов (числовые идентификаторы через
/// точку) здесь нет: конвейер задачи 4 предрелизов не публикует вовсе, и
/// `decide` ниже в принципе никогда не предлагает такой тег к установке —
/// усложнять сравнение внутри непредлагаемой ветки незачем.
impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Разбор тега вида `v1.2.3` или `v1.2.3-rc.1`. `None` — тег не версия
/// вовсе, и вызывающий обязан прочитать это как честное «не знаю», а не
/// подставить нулевую версию: нулевая версия сравнивалась бы как «сильно
/// младше», и мусорный тег молча выглядел бы как «версия ниже некуда».
pub fn parse_tag(tag: &str) -> Option<Version> {
    let body = tag.strip_prefix('v').unwrap_or(tag);
    let (numeric, pre) = match body.split_once('-') {
        Some((n, p)) => (n, Some(p.to_string())),
        None => (body, None),
    };
    let mut parts = numeric.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    // Ровно три числовые части — «1.2.3.4» не тег версии этого проекта, и
    // молчаливое обрезание лишнего значило бы принять то, чего сборка
    // задачи 4 никогда не публикует.
    if parts.next().is_some() {
        return None;
    }
    if let Some(p) = &pre {
        // «v1.2.3-» — висячий дефис, не суффикс предрелиза.
        if p.is_empty() {
            return None;
        }
    }
    Some(Version {
        major,
        minor,
        patch,
        pre,
    })
}

/// Итог сравнения текущей версии приложения с опубликованным тегом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Опубликованная версия строго новее и не предрелиз — тег стоит
    /// предлагать к установке.
    Available(Version),
    UpToDate,
    /// Собственная версия новее опубликованной — тег откатили назад, или
    /// это сборка вперёд последнего релиза. Ставить нечего.
    CurrentIsNewer,
    /// Опубликованный тег — предрелиз. Продукт предрелизы не предлагает
    /// никогда, независимо от числовой части версии.
    PublishedIsPrerelease(Version),
    /// Хотя бы один из тегов не разобрался как версия. Не «версия ниже
    /// некуда» — честное «не знаю», ничего не предлагается.
    Unrecognized,
}

/// Сравнивает версию приложения (`CARGO_PKG_VERSION`, без ведущего `v`) с
/// тегом релиза (`tag_name` из GitHub Releases API, с ведущим `v`).
pub fn decide(current: &str, published_tag: &str) -> Decision {
    let Some(cur) = parse_tag(current) else {
        return Decision::Unrecognized;
    };
    let Some(published) = parse_tag(published_tag) else {
        return Decision::Unrecognized;
    };
    if published.is_prerelease() {
        return Decision::PublishedIsPrerelease(published);
    }
    match published.cmp(&cur) {
        Ordering::Greater => Decision::Available(published),
        Ordering::Equal => Decision::UpToDate,
        Ordering::Less => Decision::CurrentIsNewer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_tag() {
        let v = parse_tag("v1.2.3").expect("должен разобраться");
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert!(!v.is_prerelease());
    }

    #[test]
    fn parses_a_tag_without_the_leading_v() {
        // CARGO_PKG_VERSION приходит без `v` — decide() обязан принимать
        // оба вида не как два разных формата, а как один разбор.
        let v = parse_tag("1.2.3").expect("должен разобраться");
        assert_eq!((v.major, v.minor, v.patch), (1, 2, 3));
    }

    #[test]
    fn parses_a_prerelease_suffix() {
        let v = parse_tag("v0.2.0-rc.1").expect("должен разобраться");
        assert_eq!((v.major, v.minor, v.patch), (0, 2, 0));
        assert!(v.is_prerelease());
        assert_eq!(v.pre.as_deref(), Some("rc.1"));
    }

    #[test]
    fn rejects_a_dangling_hyphen() {
        assert!(parse_tag("v1.2.3-").is_none());
    }

    #[test]
    fn rejects_extra_numeric_segments() {
        assert!(parse_tag("v1.2.3.4").is_none());
    }

    #[test]
    fn rejects_too_few_numeric_segments() {
        assert!(parse_tag("v1.2").is_none());
    }

    #[test]
    fn rejects_a_tag_that_is_not_a_version_at_all() {
        for tag in ["latest", "release-notes", "win-v1.2.3", ""] {
            assert!(
                parse_tag(tag).is_none(),
                "«{tag}» не должен разбираться как версия"
            );
        }
    }

    #[test]
    fn rejects_non_numeric_segments() {
        assert!(parse_tag("v1.x.3").is_none());
    }

    // ---- decide() — приёмка задачи 3 ----

    #[test]
    fn a_newer_published_version_is_available() {
        assert_eq!(
            decide("1.2.3", "v1.3.0"),
            Decision::Available(parse_tag("v1.3.0").unwrap())
        );
    }

    #[test]
    fn an_equal_published_version_is_up_to_date() {
        assert_eq!(decide("1.2.3", "v1.2.3"), Decision::UpToDate);
    }

    #[test]
    fn a_current_version_newer_than_published_is_reported_as_such() {
        // Тег откатили назад, или это сборка вперёд релиза — ставить
        // нечего, но и молчать о причине нельзя (иначе выглядит как отказ
        // сети, а не как «вы уже впереди»).
        assert_eq!(decide("2.0.0", "v1.9.9"), Decision::CurrentIsNewer);
    }

    #[test]
    fn a_prerelease_tag_is_never_offered_even_when_numerically_newer() {
        let d = decide("1.2.3", "v1.3.0-rc.1");
        assert!(matches!(d, Decision::PublishedIsPrerelease(_)), "{d:?}");
    }

    #[test]
    fn a_prerelease_tag_numerically_older_is_still_reported_as_prerelease() {
        // Приоритет проверки: «это предрелиз» решается раньше сравнения
        // чисел — иначе ветка эквивалента/старее замаскировала бы факт
        // «опубликован именно предрелиз».
        let d = decide("2.0.0", "v1.3.0-rc.1");
        assert!(matches!(d, Decision::PublishedIsPrerelease(_)), "{d:?}");
    }

    #[test]
    fn a_malformed_tag_is_unrecognized_not_treated_as_no_update() {
        assert_eq!(decide("1.2.3", "v1.2"), Decision::Unrecognized);
        assert_eq!(decide("1.2.3", "1.2.3.4"), Decision::Unrecognized);
    }

    #[test]
    fn a_tag_that_is_not_a_version_at_all_is_unrecognized() {
        assert_eq!(decide("1.2.3", "latest"), Decision::Unrecognized);
        assert_eq!(decide("1.2.3", ""), Decision::Unrecognized);
    }

    #[test]
    fn a_broken_current_version_is_unrecognized_rather_than_panicking() {
        // CARGO_PKG_VERSION собственной сборки обязан разбираться всегда,
        // но упасть здесь — хуже, чем сказать честное «не знаю»: это не
        // сеть, а не должно ронять фоновую проверку обновлений.
        assert_eq!(decide("не-версия", "v1.2.3"), Decision::Unrecognized);
    }

    #[test]
    fn ordering_treats_a_full_release_as_newer_than_its_own_prerelease() {
        let release = parse_tag("v1.2.3").unwrap();
        let prerelease = parse_tag("v1.2.3-rc.1").unwrap();
        assert!(release > prerelease);
    }

    #[test]
    fn ordering_breaks_ties_between_prereleases_lexicographically() {
        let rc1 = parse_tag("v1.2.3-rc.1").unwrap();
        let rc2 = parse_tag("v1.2.3-rc.2").unwrap();
        assert!(rc2 > rc1);
    }
}
