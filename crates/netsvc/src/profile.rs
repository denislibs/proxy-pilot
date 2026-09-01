//! Разбор `%ProgramData%\ProxyPilot\profile.toml` — собственной копии
//! службы, отдельной от пользовательского `%APPDATA%\ProxyPilot\config.toml`
//! (см. докблок крейта: служба работает от LocalSystem, и читать
//! пользовательский файл значило бы дать любому с доступом на запись к нему
//! диктовать сетевые настройки системной службе).
//!
//! Формат — подмножество полей `Config` (задача 5), нужных именно здесь:
//! список офисных сетей NLM (чтобы решить «мы в офисе») и сам профиль
//! статики. Оба типа приходят из `proxypilot_core` как есть — тем же
//! приёмом, что и весь остальной проект: решение и данные не дублируются,
//! только собираются заново в нужном здесь сочетании.

use std::path::{Path, PathBuf};

use proxypilot_core::config::OfficeNetwork;
use proxypilot_core::netprofile::NetProfile;
use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ServiceProfile {
    /// GUID офисных сетей NLM — то же самое, чем `Config::office_networks`
    /// решает «мы в офисе» на стороне приложения (`Config::place_for`).
    pub office_networks: Vec<OfficeNetwork>,
    /// Адрес, маска, шлюз, DNS — решение о них уже целиком в
    /// `proxypilot_core::netprofile::decide_profile` (задача 5), эта
    /// структура лишь несёт данные для него.
    pub net_profile: NetProfile,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("не разобрался profile.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("ошибка чтения profile.toml: {0}")]
    Io(std::io::Error),
}

/// `%ProgramData%` — переменная окружения всегда есть на Windows; жёсткий
/// путь на случай её отсутствия не стоит того, чтобы превращать чтение
/// профиля в ошибку. Тот же приём, что и `program_files_dir` в
/// `winnet::openvpn`.
pub fn program_data_dir() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

/// Чистая функция ради теста — тем же приёмом, что и `standard_bin_dir` в
/// `winnet::openvpn::locate`: путь строится из явно переданного корня, без
/// обращения к переменным окружения, так что тест проверяет саму
/// подстановку, а не то, что стоит в окружении машины прямо сейчас.
fn path_under(program_data: &Path) -> PathBuf {
    program_data.join("ProxyPilot").join("profile.toml")
}

pub fn path() -> PathBuf {
    path_under(&program_data_dir())
}

/// Отсутствие файла — не ошибка, а «профиль ещё не настроен» (первый
/// запуск службы до того, как приложение хоть раз сохранило настройки) —
/// `ServiceProfile::default()`, в котором `net_profile.office_ip` пуст, а
/// значит `decide_profile` (задача 5) не тронет сеть вообще. Тот же приём,
/// что и `Config::load_from` на стороне приложения.
pub fn load_from(path: &Path) -> Result<ServiceProfile, ProfileError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ServiceProfile::default());
        }
        Err(e) => return Err(ProfileError::Io(e)),
    };
    Ok(toml::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn a_missing_file_is_an_unmanaged_default_profile() {
        // Отсутствие файла — не ошибка, а первый запуск/ещё не настроено.
        // Тот же приём, что и `Config::load_from` на стороне приложения.
        let dir = std::env::temp_dir().join("proxypilot-netsvc-test-missing");
        let path = dir.join("nope.toml");
        let got = load_from(&path).expect("отсутствие файла — не ошибка");
        assert_eq!(got, ServiceProfile::default());
        assert!(got.net_profile.office_ip.is_none());
    }

    #[test]
    fn parses_office_networks_and_net_profile_together() {
        let dir = std::env::temp_dir().join("proxypilot-netsvc-test-full");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.toml");
        std::fs::write(
            &path,
            r#"
[[office_networks]]
id = "{AAAA0000-0000-0000-0000-000000000001}"
name = "Офис"

[net_profile]
office_ip = "203.0.113.10"
office_mask = "255.255.255.0"
office_gateway = "203.0.113.1"
office_dns = ["203.0.113.53", "198.51.100.53"]
"#,
        )
        .unwrap();

        let got = load_from(&path).expect("корректный файл обязан разобраться");
        assert_eq!(got.office_networks.len(), 1);
        assert_eq!(
            got.office_networks[0].id,
            "{AAAA0000-0000-0000-0000-000000000001}"
        );
        assert_eq!(
            got.net_profile.office_ip,
            Some(Ipv4Addr::new(203, 0, 113, 10))
        );
        assert_eq!(
            got.net_profile.office_mask,
            Some(Ipv4Addr::new(255, 255, 255, 0))
        );
        assert_eq!(
            got.net_profile.office_gateway,
            Some(Ipv4Addr::new(203, 0, 113, 1))
        );
        assert_eq!(
            got.net_profile.office_dns,
            vec![
                Ipv4Addr::new(203, 0, 113, 53),
                Ipv4Addr::new(198, 51, 100, 53)
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_optional_fields_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join("proxypilot-netsvc-test-partial");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.toml");
        std::fs::write(
            &path,
            r#"
[[office_networks]]
id = "{AAAA0000-0000-0000-0000-000000000001}"
"#,
        )
        .unwrap();

        let got = load_from(&path).expect("частичный файл обязан разобраться");
        assert_eq!(got.office_networks.len(), 1);
        assert_eq!(got.office_networks[0].name, "");
        assert_eq!(got.net_profile, NetProfile::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_toml_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join("proxypilot-netsvc-test-broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.toml");
        std::fs::write(&path, "это не toml =").unwrap();

        let err = load_from(&path).expect_err("битый toml обязан быть ошибкой");
        assert!(matches!(err, ProfileError::Parse(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_lives_under_program_data_not_the_user_profile() {
        // Ключевое свойство задачи (докблок модуля и `docs/design.md`
        // §7.4): служба не читает пользовательский конфиг.
        let p = path_under(Path::new(r"C:\ProgramData"));
        let s = p.to_string_lossy().replace('/', "\\");
        assert_eq!(s, r"C:\ProgramData\ProxyPilot\profile.toml");
    }

    #[test]
    fn service_profile_path_differs_from_the_user_config_path() {
        // Прямая проверка утверждения «не пользовательский файл»: путь
        // службы не обязан существовать в момент теста (директория
        // ProgramData может не иметь ProxyPilot вовсе), но обязан отличаться
        // от `Config::path()` даже без похода на диск.
        let service_path = path();
        if let Some(user_path) = proxypilot_core::config::Config::path() {
            assert_ne!(service_path, user_path);
        }
        assert!(service_path.ends_with("profile.toml"));
    }
}
