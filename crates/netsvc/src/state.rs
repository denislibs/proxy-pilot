//! Признак «эту статику поставили мы» — единственная часть
//! `proxypilot_core::netprofile::AdapterConfig`, которую нельзя прочитать
//! прямо из состояния адаптера: Windows не помечает адрес, поставленный
//! `netsh`, ничьим владением. Источник этого признака — не сам адаптер, а
//! то, что служба сама записала после последнего успешного применения
//! статики: `%ProgramData%\ProxyPilot\applied.toml`.
//!
//! Это НЕ конфигурация и не вход для решения `decide_profile` — это
//! собственная память службы о своём последнем действии, нужная только
//! чтобы отличить «мы уже это ставили» от «кто-то прописал руками ровно
//! такой же адрес». Совпадение с профилем не единственный сигнал именно по
//! этой причине: если бы `set_by_us` вычислялся просто как «текущий адрес
//! равен office_ip», то офисная статика, прописанная человеком вручную (не
//! нашей службой), выглядела бы неотличимо от нашей — а «чужая статика
//! никогда не сбрасывается» (задача 5, `foreign_static_address_is_never_reset`)
//! как раз про этот случай.
//!
//! Отказ прочитать файл (испорчен, отсутствует) трактуется как «неизвестно,
//! мы ли ставили» — `AppliedState::default()`, то есть `set_by_us` вернёт
//! `false`. Это безопасная сторона ошибки: `decide_profile` тогда увидит
//! чужую (с её точки зрения) статику и не тронет её — то же самое
//! консервативное правило, что и во всём остальном проекте (см., например,
//! `openvpn.rs`: «отказ безопасен» про `WOW6432Node`).

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppliedState {
    pub ip: Option<Ipv4Addr>,
    pub mask: Option<Ipv4Addr>,
    /// GUID адаптера, на который это было поставлено — не «дружественное»
    /// (переименуемое) имя. Нужен отдельно от текущего опознания через NLM
    /// (`adapter::gather_from_nlm`), потому что тот путь работает, только
    /// пока адаптер ещё числится в офисной сети: как только машина покидает
    /// офис, NLM перестаёт отдавать это подключение вовсе, а откатывать
    /// статику в DHCP всё ещё нужно на том же самом физическом адаптере.
    /// Источник истины на этот момент — только собственная память службы.
    ///
    /// GUID, а не `FriendlyName` (ревью round 2, задача 3 уже устанавливала
    /// это ограничение для алиасов интерфейсов): человек может переименовать
    /// подключение в любой момент, в том числе между применением статики и
    /// откатом. Переименование, случившееся именно в этом окне, сделало бы
    /// `current_ipv4_config` по сохранённому имени молчаливым `None`
    /// (`main.rs::run_cycle` тогда просто не находит, что откатывать), а
    /// GUID адаптера Windows не меняет никогда. Алиас для самой команды
    /// `netsh ... name=` резолвится заново в момент применения
    /// (`adapter::friendly_name_for_guid`), а не берётся отсюда.
    pub iface_guid: Option<String>,
}

/// Наша ли текущая статика адаптера — сравнением с тем, что мы сами
/// записали после последнего успешного применения. См. докблок модуля про
/// то, почему это не просто «совпадает с профилем».
pub fn set_by_us(
    applied: &AppliedState,
    current_addr: Option<Ipv4Addr>,
    current_mask: Option<Ipv4Addr>,
) -> bool {
    applied.ip.is_some() && applied.ip == current_addr && applied.mask == current_mask
}

fn path_under(program_data: &Path) -> PathBuf {
    program_data.join("ProxyPilot").join("applied.toml")
}

pub fn path() -> PathBuf {
    path_under(&crate::profile::program_data_dir())
}

/// Любой отказ чтения (файла нет, испорчен) читается как дефолт — докблок
/// модуля объясняет, почему это безопасная сторона ошибки.
pub fn load_from(path: &Path) -> AppliedState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_to(path: &Path, state: &AppliedState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string(state).expect("AppliedState всегда сериализуем");
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    #[test]
    fn matches_current_address_and_mask_is_our_own() {
        let applied = AppliedState {
            ip: Some(addr(203, 0, 113, 10)),
            mask: Some(addr(255, 255, 255, 0)),
            ..Default::default()
        };
        assert!(set_by_us(
            &applied,
            Some(addr(203, 0, 113, 10)),
            Some(addr(255, 255, 255, 0))
        ));
    }

    #[test]
    fn a_different_current_address_is_not_ours() {
        // Человек мог сам прописать статику после того, как мы сбросили
        // свою в DHCP — совпадение с ПРОШЛЫМ нашим значением не делает её
        // нашей сейчас.
        let applied = AppliedState {
            ip: Some(addr(203, 0, 113, 10)),
            mask: Some(addr(255, 255, 255, 0)),
            ..Default::default()
        };
        assert!(!set_by_us(
            &applied,
            Some(addr(203, 0, 113, 99)),
            Some(addr(255, 255, 255, 0))
        ));
    }

    #[test]
    fn a_different_current_mask_is_not_ours() {
        let applied = AppliedState {
            ip: Some(addr(203, 0, 113, 10)),
            mask: Some(addr(255, 255, 255, 0)),
            ..Default::default()
        };
        assert!(!set_by_us(
            &applied,
            Some(addr(203, 0, 113, 10)),
            Some(addr(255, 255, 0, 0))
        ));
    }

    #[test]
    fn no_recorded_state_at_all_is_never_ours() {
        // Свежая установка службы (или испорченный/удалённый applied.toml)
        // — безопасная сторона ошибки: считаем текущую статику чужой, а не
        // своей, и не трогаем её (докблок модуля).
        assert!(!set_by_us(
            &AppliedState::default(),
            Some(addr(203, 0, 113, 10)),
            Some(addr(255, 255, 255, 0))
        ));
    }

    #[test]
    fn a_missing_state_file_reads_as_the_default() {
        let dir = std::env::temp_dir().join("proxypilot-netsvc-test-state-missing");
        let path = dir.join("nope.toml");
        assert_eq!(load_from(&path), AppliedState::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join("proxypilot-netsvc-test-state-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("applied.toml");
        let state = AppliedState {
            ip: Some(addr(203, 0, 113, 10)),
            mask: Some(addr(255, 255, 255, 0)),
            iface_guid: Some("{AAAA0000-0000-0000-0000-000000000001}".to_string()),
        };
        save_to(&path, &state).expect("запись обязана удаться");
        assert_eq!(load_from(&path), state);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupted_state_file_reads_as_the_default_not_a_panic() {
        // Отказ безопасен: испорченный файл памяти службы не должен ронять
        // цикл принятия решения, только сбрасывать его к «не наша статика».
        let dir = std::env::temp_dir().join("proxypilot-netsvc-test-state-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("applied.toml");
        std::fs::write(&path, "это не toml =").unwrap();
        assert_eq!(load_from(&path), AppliedState::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_lives_under_program_data() {
        let p = path_under(Path::new(r"C:\ProgramData"));
        let s = p.to_string_lossy().replace('/', "\\");
        assert_eq!(s, r"C:\ProgramData\ProxyPilot\applied.toml");
    }
}
