//! Всё, что говорит с Windows: опознание сети, настройки прокси, события.
//!
//! Вынесено отдельным крейтом сознательно: `proxypilot-core` обязан
//! оставаться без платформенных зависимостей, а `proxypilot-bridge` —
//! переносимым (он говорит только на tokio).

pub mod autostart;
pub mod com;
pub mod events;
pub mod networks;
pub mod openvpn;
pub mod ovpn_profile;
pub mod sysproxy;
pub mod tunnel_state;

#[derive(Debug, thiserror::Error)]
pub enum WinNetError {
    #[error("ошибка Windows: {0}")]
    Windows(#[from] windows::core::Error),
    // Без #[from]: `?` протащил бы сюда любую будущую io::Error из любого
    // места этого крейта под один и тот же, для неё неверный текст. Только
    // явный `.map_err(WinNetError::CurrentExe)` в единственном месте, где
    // она сейчас нужна (`autostart::is_enabled`).
    #[error("не удалось определить путь к своему исполняемому файлу: {0}")]
    CurrentExe(std::io::Error),
    /// `Installation`, найденный ранее `openvpn::find_installation`, к
    /// моменту вызова одной из функций управления туннелем (задача 4)
    /// пропал: `gui_exe` больше не существует как файл. `Installation` не
    /// гарантирует актуальность дольше одного вызова — OpenVPN мог быть
    /// удалён между поиском установки и попыткой ей воспользоваться, и
    /// каждая из функций `openvpn::{install_profile, connect, disconnect,
    /// status}` проверяет это заново, а не доверяет старому результату
    /// поиска молча.
    #[error("OpenVPN не найден: {gui_exe:?} отсутствует на диске")]
    OpenVpnNotFound { gui_exe: std::path::PathBuf },
    /// `openvpn-gui.exe --command …` не удалось запустить — сам процесс
    /// не стартовал. Не путать с тем, что подключение не удалось: это
    /// узнать отсюда нельзя (см. докблок `openvpn::TunnelStatus`).
    #[error("не удалось запустить {exe:?}: {source}")]
    OpenVpnGuiLaunch {
        exe: std::path::PathBuf,
        source: std::io::Error,
    },
    /// Не удалось записать файл профиля в каталог конфигураций OpenVPN
    /// (`openvpn::install_profile`) — например, каталог недоступен на
    /// запись.
    #[error("не удалось записать профиль {path:?}: {source}")]
    ProfileWrite {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    /// `ovpn_profile::build_profile` отказалась собирать профиль из
    /// структурно битого источника — ошибка доходит досюда без изменений,
    /// не проглатывается (`openvpn::build_and_install_profile` — первый
    /// вызывающий `build_profile`).
    #[error("не удалось собрать профиль: {0}")]
    Profile(#[from] crate::ovpn_profile::ProfileError),
}
