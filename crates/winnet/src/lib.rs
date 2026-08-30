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
}
