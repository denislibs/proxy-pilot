//! Решение о профиле сети адаптера: статический адрес в офисе, DHCP вне
//! офиса, но никогда не то и другое молча поверх чужой настройки.
//!
//! Модуль не делает ввода-вывода вообще: не читает и не меняет `netsh`,
//! реестр, файлы, время — ничего платформенного. И вызывающий (задача 6,
//! служба статического IP на Windows), и тесты здесь передают состояние
//! адаптера как данные, а функция `decide_profile` только решает, что
//! сделать; выполняет решение уже другой крейт.
//!
//! Таблица решений (бриф задачи 5):
//!
//! ```text
//! в офисе, адрес задан, адаптер наш или на DHCP  -> статический адрес и офисные DNS
//! не в офисе, статику ставили мы                 -> DHCP
//! адрес не задан                                 -> не трогаем сеть вообще
//! адрес прописан не нами                         -> не трогаем: это чужая настройка
//! ```
//!
//! Последняя строка — главная: сбросить в DHCP статику, которую поставил
//! не наш инструмент, значит молча сломать чужую настройку машины в
//! момент смены сети. Человек, вручную прописавший адрес для доступа к
//! лабораторному стенду, не должен обнаружить, что мы его стёрли —
//! поэтому проверка «это не мы поставили» имеет приоритет над «мы в
//! офисе» и применяется в обе стороны (`foreign_static_address_is_never_reset`).

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// Офисный сетевой профиль — то, что применяется, когда мы решаем, что
/// адаптер должен получить статический адрес. Хранится в `Config`
/// (задача 5), поэтому умеет (де)сериализоваться; `#[serde(default)]` на
/// каждом поле — старый конфиг без этого раздела обязан читаться как
/// «профиль не настроен», а не как ошибка разбора.
///
/// Все поля, кроме DNS, — `Option`: пустой `office_ip` (или отсутствующий
/// `office_mask`) означает «профиль не настроен», и это ровно то состояние,
/// в котором `decide_profile` не трогает сеть вообще — то же правило, что
/// на macOS: инструмент не начинает распоряжаться сетью просто потому, что
/// его установили.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetProfile {
    pub office_ip: Option<Ipv4Addr>,
    pub office_mask: Option<Ipv4Addr>,
    /// Шлюз для проверки достижимости после применения статики (задача 6,
    /// спека 7.3) — необязателен здесь: без него `decide_profile` всё равно
    /// умеет решить «применять статику», просто без последующей защитной
    /// проверки, которая уже вне этого модуля.
    pub office_gateway: Option<Ipv4Addr>,
    pub office_dns: Vec<Ipv4Addr>,
}

/// Текущее состояние сетевого адаптера, как его видит вызывающий (в
/// проде — `netsh`/WMI на стороне службы, здесь — данные, собранные тестом
/// или фикстурой).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterConfig {
    /// Адаптер сейчас получает адрес по DHCP (а не по статике).
    pub dhcp: bool,
    /// Текущий статический адрес, если он есть (`None`, когда `dhcp == true`).
    pub addr: Option<Ipv4Addr>,
    /// Текущую статику — если она есть — поставили мы сами в прошлый раз.
    /// Это единственный признак, отличающий «наш адрес» от «человек прописал
    /// адрес для лабораторного стенда руками»: `decide_profile` обязана
    /// оставить второе в покое.
    pub set_by_us: bool,
}

/// Действие, которое нужно применить к адаптеру. Само действие ничего не
/// выполняет — это описание решения для вызывающего (задача 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileAction {
    /// Поставить статический адрес и офисные DNS.
    SetStatic {
        ip: Ipv4Addr,
        mask: Ipv4Addr,
        gateway: Option<Ipv4Addr>,
        dns: Vec<Ipv4Addr>,
    },
    /// Вернуть адаптер на DHCP.
    SetDhcp,
    /// Не трогать сеть вообще — профиль не настроен или адрес прописан не нами.
    LeaveAlone,
}

/// Единственная точка принятия решения о профиле сети. Чистая функция:
/// ввода-вывода нет, только сопоставление входов таблице из докблока модуля.
pub fn decide_profile(
    in_office: bool,
    profile: &NetProfile,
    current: &AdapterConfig,
) -> ProfileAction {
    // Правило «адрес не задан — не управляем» стоит первым и разом
    // отсекает всё остальное: не важно, где мы и что с адаптером, если
    // офисный профиль неполон. Требуем и адрес, и маску — со статикой без
    // маски `SetStatic` формировать не из чего, а угадывать маску (скажем,
    // /32 или /24) значило бы навязать сеть, которую никто не настраивал.
    let (Some(ip), Some(mask)) = (profile.office_ip, profile.office_mask) else {
        return ProfileAction::LeaveAlone;
    };

    // Правило «чужая статика не трогается» стоит вторым, до проверки
    // офиса, — оно действует одинаково и в офисе, и вне его. Чужая статика
    // — это НЕ DHCP и НЕ наша прошлая настройка.
    let is_foreign_static = !current.dhcp && !current.set_by_us;
    if is_foreign_static {
        return ProfileAction::LeaveAlone;
    }

    if in_office {
        // Дошли сюда — адаптер либо на DHCP, либо несёт нашу же прежнюю
        // статику. Оба случая безопасно перезаписать офисным профилем.
        ProfileAction::SetStatic {
            ip,
            mask,
            gateway: profile.office_gateway,
            dns: profile.office_dns.clone(),
        }
    } else if current.set_by_us && !current.dhcp {
        // Мы поставили статику, а сеть сменилась — возвращаем DHCP.
        ProfileAction::SetDhcp
    } else {
        // Уже DHCP (не важно, чьего происхождения) и мы не в офисе —
        // менять нечего.
        ProfileAction::LeaveAlone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn sample_profile() -> NetProfile {
        NetProfile {
            office_ip: Some(Ipv4Addr::new(203, 0, 113, 10)),
            office_mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
            office_gateway: Some(Ipv4Addr::new(203, 0, 113, 1)),
            office_dns: vec![
                Ipv4Addr::new(203, 0, 113, 53),
                Ipv4Addr::new(198, 51, 100, 53),
            ],
        }
    }

    fn empty_profile() -> NetProfile {
        NetProfile {
            office_ip: None,
            office_mask: None,
            office_gateway: None,
            office_dns: Vec::new(),
        }
    }

    fn our_static() -> AdapterConfig {
        AdapterConfig {
            dhcp: false,
            addr: Some(Ipv4Addr::new(203, 0, 113, 10)),
            set_by_us: true,
        }
    }

    fn foreign_static() -> AdapterConfig {
        AdapterConfig {
            dhcp: false,
            addr: Some(Ipv4Addr::new(198, 51, 100, 20)),
            set_by_us: false,
        }
    }

    fn on_dhcp() -> AdapterConfig {
        AdapterConfig {
            dhcp: true,
            addr: None,
            set_by_us: false,
        }
    }

    fn expected_static() -> ProfileAction {
        ProfileAction::SetStatic {
            ip: Ipv4Addr::new(203, 0, 113, 10),
            mask: Ipv4Addr::new(255, 255, 255, 0),
            gateway: Some(Ipv4Addr::new(203, 0, 113, 1)),
            dns: vec![
                Ipv4Addr::new(203, 0, 113, 53),
                Ipv4Addr::new(198, 51, 100, 53),
            ],
        }
    }

    #[test]
    fn decision_table_covers_every_combination() {
        // (в офисе, профиль задан, состояние адаптера, ожидаемое действие)
        let cases: Vec<(bool, bool, AdapterConfig, ProfileAction)> = vec![
            (true, true, our_static(), expected_static()),
            (true, true, on_dhcp(), expected_static()),
            (true, true, foreign_static(), ProfileAction::LeaveAlone),
            (false, true, our_static(), ProfileAction::SetDhcp),
            (false, true, on_dhcp(), ProfileAction::LeaveAlone),
            (false, true, foreign_static(), ProfileAction::LeaveAlone),
            (true, false, our_static(), ProfileAction::LeaveAlone),
            (true, false, on_dhcp(), ProfileAction::LeaveAlone),
            (true, false, foreign_static(), ProfileAction::LeaveAlone),
            (false, false, our_static(), ProfileAction::LeaveAlone),
            (false, false, on_dhcp(), ProfileAction::LeaveAlone),
            (false, false, foreign_static(), ProfileAction::LeaveAlone),
        ];

        for (in_office, has_profile, current, expected) in cases {
            let profile = if has_profile {
                sample_profile()
            } else {
                empty_profile()
            };
            let got = decide_profile(in_office, &profile, &current);
            assert_eq!(
                got, expected,
                "in_office={in_office} has_profile={has_profile} current={current:?}"
            );
        }
    }

    #[test]
    fn empty_office_address_means_we_do_not_manage_the_network() {
        let profile = empty_profile();
        for in_office in [true, false] {
            for current in [our_static(), foreign_static(), on_dhcp()] {
                assert_eq!(
                    decide_profile(in_office, &profile, &current),
                    ProfileAction::LeaveAlone,
                    "in_office={in_office} current={current:?}"
                );
            }
        }
    }

    #[test]
    fn foreign_static_address_is_never_reset() {
        let profile = sample_profile();
        for in_office in [true, false] {
            assert_eq!(
                decide_profile(in_office, &profile, &foreign_static()),
                ProfileAction::LeaveAlone,
                "in_office={in_office}"
            );
        }
    }

    #[test]
    fn address_without_a_mask_is_treated_as_not_configured() {
        // Профиль с адресом, но без маски — не полноценно настроен: без маски
        // некуда деть SetStatic. Безопаснее считать это «не управляем», чем
        // угадывать маску.
        let mut profile = sample_profile();
        profile.office_mask = None;
        assert_eq!(
            decide_profile(true, &profile, &on_dhcp()),
            ProfileAction::LeaveAlone
        );
    }

    #[test]
    fn mask_without_an_address_is_treated_as_not_configured() {
        let mut profile = sample_profile();
        profile.office_ip = None;
        assert_eq!(
            decide_profile(true, &profile, &on_dhcp()),
            ProfileAction::LeaveAlone
        );
    }
}
