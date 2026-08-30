//! Конфигурация.
//!
//! TOML, а не KEY=VALUE как на macOS: тот формат был продиктован тем, что
//! файл читал шелл. Здесь этого ограничения нет, а свойство безопасности
//! («конфиг разбирается, но никогда не исполняется») достаётся бесплатно.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mode::{ConnectedNetwork, Mode, Place, Upstreams};

pub const DEFAULT_NO_PROXY: &str = "localhost,127.0.0.1,::1,.local,\
169.254.0.0/16,192.168.0.0/16,10.0.0.0/8,172.16.0.0/12";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub bridge_port: u16,
    pub mode: Mode,
    pub socks_upstream: Option<String>,
    pub http_upstream: Option<String>,
    pub no_proxy: String,
    pub dial_timeout_ms: u64,
    pub head_timeout_ms: u64,
    pub max_connections: usize,
    /// Управлять ли системными настройками прокси Windows.
    ///
    /// Выключатель нужен тем, у кого прокси задаёт групповая политика или
    /// кто ходит через мост только явным `-x`: в этом случае трогать реестр
    /// мы не имеем права вообще.
    pub manage_system_proxy: bool,
    #[serde(default)]
    pub office_networks: Vec<OfficeNetwork>,
    /// Системные настройки прокси, какими они были ДО того, как мы их
    /// переписали. Пишется на диск раньше, чем меняется реестр: иначе
    /// убитый процесс оставил бы указатель на мёртвый слушатель и никакой
    /// возможности узнать, что там стояло раньше.
    ///
    /// Поле платформенно-нейтрально по составу (три скаляра) — переводом
    /// в `winnet::sysproxy::SysProxy` занимается приложение, а этот крейт
    /// остаётся без зависимостей от Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_sysproxy: Option<SavedSysProxy>,
}

/// Снимок системных настроек прокси для конфига.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSysProxy {
    pub enabled: bool,
    pub server: String,
    pub bypass: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeNetwork {
    /// GUID сети в канонической форме, как его отдаёт NLM.
    pub id: String,
    /// Человекочитаемое имя — только для UI, в сравнении не участвует:
    /// пользователь может переименовать сеть, а идентификатор останется.
    #[serde(default)]
    pub name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bridge_port: 3129,
            mode: Mode::Auto,
            socks_upstream: None,
            http_upstream: None,
            no_proxy: DEFAULT_NO_PROXY.to_string(),
            dial_timeout_ms: 3000,
            head_timeout_ms: 10_000,
            max_connections: 512,
            manage_system_proxy: true,
            office_networks: Vec::new(),
            saved_sysproxy: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("не разобрался конфиг: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("не сериализовался конфиг: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("ошибка работы с файлом конфига: {0}")]
    Io(#[from] std::io::Error),
    #[error("недопустимое значение в конфиге: {0}")]
    Invalid(String),
    #[error("не нашёл каталог конфигурации пользователя")]
    NoConfigDir,
}

/// Верхний предел на число соединений. Tokio паникует, если запросить
/// у семафора больше `Semaphore::MAX_PERMITS`; конфиг правится руками,
/// поэтому значение обязано проверяться при загрузке, а не при старте моста.
const MAX_CONNECTIONS_CEILING: usize = 65_536;

impl Config {
    /// `%APPDATA%\ProxyPilot\config.toml`
    ///
    /// Через BaseDirs, а не ProjectDirs: последний на Windows дописывает
    /// собственный подкаталог `config`, и путь переставал бы совпадать
    /// с тем, что обещает спека и что человек увидит в инструкции.
    pub fn path() -> Option<PathBuf> {
        directories::BaseDirs::new().map(|d| d.config_dir().join("ProxyPilot").join("config.toml"))
    }

    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::path().ok_or(ConfigError::NoConfigDir)?;
        Self::load_from(&path)
    }

    /// Отсутствие файла — это первый запуск, а не ошибка: возвращаем дефолты.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(ConfigError::Io(e)),
        };
        let cfg = Self::from_toml(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::path().ok_or(ConfigError::NoConfigDir)?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(ConfigError::Io)?;
        }
        // Пишем во временный файл и переименовываем: операция атомарна,
        // и сбой/отключение питания не оставит обрезанный конфиг.
        //
        // И с явным `sync_all` перед переименованием: без него запись живёт
        // в кэше страниц, а `saved_sysproxy` обязан лежать на диске РАНЬШЕ,
        // чем мы перепишем системный прокси в реестре. Порядок «сохранили,
        // потом записали» без сброса на диск — это порядок только в
        // намерениях, но не после внезапного отключения питания.
        let tmp_path = path.with_extension("toml.tmp");
        write_and_sync(&tmp_path, &self.to_toml()).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            ConfigError::Io(e)
        })?;
        std::fs::rename(&tmp_path, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            ConfigError::Io(e)
        })
    }

    /// Значения из файла недоверенные. Каждая проверка здесь соответствует
    /// месту, которое иначе упало бы позже и непонятнее.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.bridge_port < 1024 {
            return Err(ConfigError::Invalid(format!(
                "bridge_port {}: порт ниже 1024 требует прав администратора",
                self.bridge_port
            )));
        }
        if self.max_connections == 0 || self.max_connections > MAX_CONNECTIONS_CEILING {
            return Err(ConfigError::Invalid(format!(
                "max_connections {}: допустимо от 1 до {MAX_CONNECTIONS_CEILING}",
                self.max_connections
            )));
        }
        for (name, value) in [
            ("socks_upstream", &self.socks_upstream),
            ("http_upstream", &self.http_upstream),
        ] {
            if let Some(v) = value {
                if !validate_upstream(v) {
                    return Err(ConfigError::Invalid(format!(
                        "{name} «{v}»: нужен формат host:port"
                    )));
                }
            }
        }
        for (i, o) in self.office_networks.iter().enumerate() {
            // Пустой id никогда ни с чем не совпадёт — запись мертва, но
            // молча: пользователь не поймёт, почему офисная сеть не признаётся.
            // Индекс обязателен: имя тоже может быть пустым, и тогда сообщение
            // без индекса не укажет вообще ни на что.
            if o.id.is_empty() {
                let name_suffix = if o.name.is_empty() {
                    String::new()
                } else {
                    format!(" «{}»", o.name)
                };
                return Err(ConfigError::Invalid(format!(
                    "office_networks[{i}]{name_suffix}: пустой id"
                )));
            }
        }
        Ok(())
    }

    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("Config всегда сериализуем")
    }

    pub fn upstreams(&self) -> Upstreams {
        Upstreams {
            socks: self.socks_upstream.clone(),
            http: self.http_upstream.clone(),
        }
    }

    /// Где мы, судя по списку подключённых сетей.
    ///
    /// Пустой список офисов означает «не знаем» и трактуется как «не офис»:
    /// считать иначе значило бы гнать весь трафик через прокси в любой сети.
    ///
    /// Сравнение — только по `id`; имя проносится в `Place` нетронутым, ради
    /// UI. Имя пользователь может переименовать в любой момент, а решение,
    /// зависящее от переименования, — это ровно та эвристика, от которой
    /// уводит спека 2.3.
    pub fn place_for(&self, connected: &[ConnectedNetwork]) -> Place {
        let office = connected.iter().find(|n| {
            self.office_networks
                .iter()
                .any(|o| o.id.eq_ignore_ascii_case(&n.id))
        });
        // Не в офисе — показываем первую подключённую сеть: человеку в UI
        // нужно видеть, по какой именно сети принято решение «мы снаружи».
        let chosen = office.or_else(|| connected.first());
        Place {
            in_office: office.is_some(),
            network: chosen.map(|n| n.id.clone()),
            network_name: chosen.map(|n| n.name.clone()),
        }
    }
}

/// Пишет файл и дожидается, пока данные окажутся на носителе.
fn write_and_sync(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// Апстрим задаётся как `host:port`. Порт обязателен и должен быть валидным:
/// пустая строка означает «режим выключен» и проверяется отдельно вызывающим.
pub fn validate_upstream(s: &str) -> bool {
    let Some((host, port)) = s.rsplit_once(':') else {
        return false;
    };
    if host.is_empty() {
        return false;
    }
    matches!(port.parse::<u16>(), Ok(p) if p > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Mode;

    #[test]
    fn defaults_match_the_spec() {
        let c = Config::default();
        assert_eq!(c.bridge_port, 3129);
        assert_eq!(c.mode, Mode::Auto);
        assert_eq!(c.dial_timeout_ms, 3000);
        assert_eq!(c.head_timeout_ms, 10_000);
        assert_eq!(c.max_connections, 512);
        assert!(c.socks_upstream.is_none());
        assert!(c.http_upstream.is_none());
    }

    #[test]
    fn default_no_proxy_covers_local_ranges() {
        let c = Config::default();
        for host in [
            "localhost",
            "127.0.0.1",
            "printer.local",
            "192.168.1.1",
            "10.1.2.3",
        ] {
            assert!(
                crate::bypass::BypassList::parse(&c.no_proxy).matches(host),
                "{host} должен быть в bypass по умолчанию"
            );
        }
    }

    #[test]
    fn roundtrip_through_toml_preserves_everything() {
        let c = Config {
            socks_upstream: Some("203.0.113.10:9999".into()),
            http_upstream: Some("203.0.113.10:3128".into()),
            mode: Mode::Socks,
            bridge_port: 3130,
            ..Default::default()
        };

        let parsed = Config::from_toml(&c.to_toml()).expect("должен разобраться");
        assert_eq!(parsed.socks_upstream, c.socks_upstream);
        assert_eq!(parsed.http_upstream, c.http_upstream);
        assert_eq!(parsed.mode, c.mode);
        assert_eq!(parsed.bridge_port, c.bridge_port);
    }

    #[test]
    fn the_saved_system_proxy_survives_a_roundtrip() {
        // Это значение — единственный след того, что стояло у пользователя
        // до нас. Потерять его при перечитывании конфига значит потерять
        // возможность вернуть машине сеть.
        let c = Config {
            saved_sysproxy: Some(SavedSysProxy {
                enabled: false,
                server: "203.0.113.10:3128".into(),
                bypass: "192.168.*;<local>".into(),
            }),
            ..Default::default()
        };
        let parsed = Config::from_toml(&c.to_toml()).expect("должен разобраться");
        assert_eq!(parsed.saved_sysproxy, c.saved_sysproxy);
    }

    #[test]
    fn managing_the_system_proxy_is_on_by_default_and_switchable() {
        assert!(Config::default().manage_system_proxy);
        let c = Config::from_toml("manage_system_proxy = false").expect("должен разобраться");
        assert!(!c.manage_system_proxy);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // Конфиг мог прийти от версии постарше — недостающее берём из дефолтов.
        let c = Config::from_toml("bridge_port = 3131").expect("должен разобраться");
        assert_eq!(c.bridge_port, 3131);
        assert_eq!(c.mode, Mode::Auto);
        assert_eq!(c.max_connections, 512);
    }

    #[test]
    fn broken_toml_is_an_error_not_a_panic() {
        assert!(Config::from_toml("это не toml =").is_err());
    }

    #[test]
    fn upstream_format_is_validated() {
        assert!(validate_upstream("203.0.113.10:9999"));
        assert!(validate_upstream("proxy.company.kz:3128"));
        assert!(!validate_upstream("203.0.113.10"));
        assert!(!validate_upstream("203.0.113.10:"));
        assert!(!validate_upstream("203.0.113.10:0"));
        assert!(!validate_upstream("203.0.113.10:70000"));
        assert!(!validate_upstream(""));
    }

    #[test]
    fn upstreams_view_is_built_from_config() {
        let c = Config {
            socks_upstream: Some("10.0.0.2:9999".into()),
            ..Default::default()
        };
        let u = c.upstreams();
        assert_eq!(u.socks.as_deref(), Some("10.0.0.2:9999"));
        assert!(u.http.is_none());
    }

    #[test]
    fn config_path_matches_what_the_spec_promises() {
        // Путь попадает в инструкции и в поддержку; расхождение с обещанным
        // означает, что человек будет править не тот файл.
        let Some(p) = Config::path() else { return };
        let s = p.to_string_lossy().replace('/', "\\");
        assert!(s.ends_with("\\ProxyPilot\\config.toml"), "получили: {s}");
        assert!(!s.contains("\\config\\config.toml"), "лишний сегмент: {s}");
    }

    #[test]
    fn validate_rejects_a_port_below_the_privileged_range() {
        let c = Config {
            bridge_port: 80,
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_an_absurd_connection_limit() {
        let c = Config {
            max_connections: usize::MAX,
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_a_zero_connection_limit() {
        let c = Config {
            max_connections: 0,
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_a_malformed_upstream() {
        let c = Config {
            socks_upstream: Some("нет-порта".into()),
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_rejects_an_office_network_with_empty_id() {
        // Запись с пустым id никогда ни с чем не совпадёт — это не «ничего
        // не делает», а «тихо не работает», и человек не догадается почему.
        let c = Config {
            office_networks: vec![OfficeNetwork {
                id: String::new(),
                name: "Офис".into(),
            }],
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn validate_accepts_the_defaults() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn load_from_a_missing_file_yields_defaults() {
        let dir = std::env::temp_dir().join("proxypilot-test-missing");
        let path = dir.join("nope.toml");
        let c = Config::load_from(&path).expect("отсутствие файла — не ошибка");
        assert_eq!(c, Config::default());
    }

    #[test]
    fn save_then_load_roundtrips_through_a_real_file() {
        let dir = std::env::temp_dir().join("proxypilot-test-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let c = Config {
            socks_upstream: Some("203.0.113.10:9999".into()),
            bridge_port: 3130,
            ..Default::default()
        };
        c.save_to(&path).expect("должно сохраниться");

        let back = Config::load_from(&path).expect("должно прочитаться");
        assert_eq!(back, c);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_from_an_invalid_file_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join("proxypilot-test-invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "max_connections = 0\n").unwrap();
        assert!(Config::load_from(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    fn office_cfg() -> Config {
        Config {
            office_networks: vec![
                OfficeNetwork {
                    id: "{AAAA0000-0000-0000-0000-000000000001}".into(),
                    name: "Офис".into(),
                },
                OfficeNetwork {
                    id: "{AAAA0000-0000-0000-0000-000000000002}".into(),
                    name: "Офис-2".into(),
                },
            ],
            ..Default::default()
        }
    }

    /// Подключённая сеть для теста. Имя намеренно не совпадает с именем из
    /// `office_networks`: `place_for` обязана нести то имя, которое сейчас
    /// показывает Windows, а не то, что человек однажды записал в конфиг.
    fn net(id: &str, name: &str) -> ConnectedNetwork {
        ConnectedNetwork {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn place_is_office_when_a_connected_network_matches() {
        let p = office_cfg().place_for(&[net(
            "{AAAA0000-0000-0000-0000-000000000002}",
            "OFFICE-WIFI-2",
        )]);
        assert!(p.in_office);
        assert_eq!(
            p.network.as_deref(),
            Some("{AAAA0000-0000-0000-0000-000000000002}")
        );
        // Спека 6.1: рядом с GUID окну настроек нужно имя, иначе кнопку
        // «эта сеть — офис» нечем подписать.
        assert_eq!(p.network_name.as_deref(), Some("OFFICE-WIFI-2"));
    }

    #[test]
    fn place_is_not_office_for_an_unknown_network() {
        let p = office_cfg().place_for(&[net(
            "{BBBB0000-0000-0000-0000-000000000000}",
            "Домашний Wi-Fi",
        )]);
        assert!(!p.in_office);
        assert_eq!(
            p.network.as_deref(),
            Some("{BBBB0000-0000-0000-0000-000000000000}")
        );
        // Имя нужно и снаружи офиса — именно ту сеть человек и захочет
        // отметить как офисную, если попал сюда по ошибке в конфиге.
        assert_eq!(p.network_name.as_deref(), Some("Домашний Wi-Fi"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        // GUID из реестра и из конфига могут отличаться регистром — это
        // один и тот же идентификатор, и различать их было бы ловушкой.
        let p = office_cfg().place_for(&[net("{aaaa0000-0000-0000-0000-000000000001}", "Офис")]);
        assert!(p.in_office);
    }

    #[test]
    fn the_name_never_decides_anything() {
        // Имя — только для показа. Сеть, названную ровно как офисная, но с
        // чужим GUID, офисом считать нельзя: иначе достаточно назвать свою
        // точку доступа «Офис», чтобы увести на неё корпоративный маршрут.
        let p = office_cfg().place_for(&[net("{DDDD0000-0000-0000-0000-000000000000}", "Офис")]);
        assert!(!p.in_office);
        assert_eq!(p.network_name.as_deref(), Some("Офис"));
    }

    #[test]
    fn no_network_at_all_is_not_office() {
        let p = office_cfg().place_for(&[]);
        assert!(!p.in_office);
        assert!(p.network.is_none());
        assert!(p.network_name.is_none());
    }

    #[test]
    fn without_configured_offices_nothing_is_office() {
        // Пустой список — «мы не знаем, где находимся». Считать это офисом
        // означало бы гнать весь трафик через прокси в любой сети.
        let p =
            Config::default().place_for(&[net("{AAAA0000-0000-0000-0000-000000000001}", "Офис")]);
        assert!(!p.in_office);
    }

    #[test]
    fn several_connected_networks_office_wins() {
        // Ноутбук может быть одновременно в Wi-Fi и в доке по кабелю.
        // Если хоть одна из них офисная — мы в офисе.
        let p = office_cfg().place_for(&[
            net("{CCCC0000-0000-0000-0000-000000000000}", "Гостевая"),
            net("{AAAA0000-0000-0000-0000-000000000001}", "OFFICE-WIFI"),
        ]);
        assert!(p.in_office);
        assert_eq!(
            p.network.as_deref(),
            Some("{AAAA0000-0000-0000-0000-000000000001}")
        );
        // Не «Гостевая»: показывать надо ту сеть, по которой принято
        // решение, а не первую попавшуюся.
        assert_eq!(p.network_name.as_deref(), Some("OFFICE-WIFI"));
    }
}
