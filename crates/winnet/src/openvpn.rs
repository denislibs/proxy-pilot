//! Поиск установленного OpenVPN: путь к `openvpn-gui.exe` и каталогу
//! конфигураций.
//!
//! Только чтение `HKLM\SOFTWARE\OpenVPN` — записи в HKLM здесь нет и не
//! будет: чтение не требует прав администратора, а весь продукт держит
//! инвариант «ни одного UAC в ядре» (см. `CLAUDE.md`).
//!
//! Отсутствие OpenVPN — это `Ok(None)`, а не ошибка: у половины получателей
//! его не будет, и падение здесь означало бы, что приложение не запускается
//! у людей, которым туннель вообще не нужен. Оба случая, из которых
//! складывается «нет», ведут к одному и тому же `None`: ключа `HKLM\SOFTWARE\
//! OpenVPN` может не быть вовсе (`open_key`), а если он есть — записанный в
//! нём (или подставленный по умолчанию) путь может указывать на давно
//! удалённую установку (`locate`, через проверку `gui_exe.is_file()`).
//! Проверяется именно файл, а не факт существования записи в реестре:
//! деинсталлятор оставляет ключ на месте, и `Installation`, указывающий на
//! несуществующий exe, обернулся бы невнятной ошибкой при первой попытке
//! подключения (Task 4) вместо честного «не установлен» здесь и сейчас.
//!
//! Реестр читается через `sysproxy::RegKey` — ту же обёртку, что и
//! `sysproxy`/`autostart`, только с другим корнем (`HKEY_LOCAL_MACHINE`
//! вместо `HKEY_CURRENT_USER`, ради чего `open()` и получил параметр
//! `root`): второй путь с сырым `HKEY` здесь не заводится.

use std::path::{Path, PathBuf};

use windows::core::{w, HRESULT, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_READ};

use crate::sysproxy::RegKey;
use crate::WinNetError;

const SUBKEY: PCWSTR = w!("SOFTWARE\\OpenVPN");
const BIN_DIR: PCWSTR = w!("bin_dir");
const CONFIG_DIR: PCWSTR = w!("config_dir");

/// Имя GUI-исполняемого файла OpenVPN — то же для инсталлятора реестра и
/// для стандартного пути; вынесено константой, чтобы не разойтись между
/// `locate` и возможной будущей правкой.
const GUI_EXE_NAME: &str = "openvpn-gui.exe";

/// Найденная установка OpenVPN: путь к GUI-исполняемому файлу (им управляет
/// Task 4) и к каталогу, где лежат пользовательские `.ovpn`-конфигурации.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub gui_exe: PathBuf,
    pub config_dir: PathBuf,
}

/// Ищет установленный OpenVPN. `Ok(None)` — не установлен; это не ошибка,
/// а такой же законный исход, как «установлен».
pub fn find_installation() -> Result<Option<Installation>, WinNetError> {
    let (bin_dir_value, config_dir_value) = match open_key(SUBKEY)? {
        Some(key) => (key.query_string(BIN_DIR)?, key.query_string(CONFIG_DIR)?),
        // Ключа нет вовсе — OpenVPN, скорее всего, не ставили. Пустые
        // строки заставляют `locate` ниже взять оба пути из стандартного
        // расположения, а не считать установку найденной по совпадению.
        None => (String::new(), String::new()),
    };
    Ok(locate(
        &bin_dir_value,
        &config_dir_value,
        &program_files_dir(),
    ))
}

/// Открывает `HKLM\<subkey>` на чтение; `Ok(None)`, если такого подключа нет
/// — это ожидаемое состояние машины без OpenVPN, а не отказ реестра.
/// `subkey`, а не жёстко `SUBKEY` этого модуля — ради теста, проверяющего
/// именно этот контракт на заведомо несуществующем имени, не трогая
/// настоящий `HKLM\SOFTWARE\OpenVPN`.
fn open_key(subkey: PCWSTR) -> Result<Option<RegKey>, WinNetError> {
    match RegKey::open(HKEY_LOCAL_MACHINE, subkey, KEY_READ) {
        Ok(key) => Ok(Some(key)),
        Err(WinNetError::Windows(e)) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// `%ProgramFiles%` — корень стандартного пути установки, если в реестре
/// нет своего значения. Переменная всегда есть в окружении Windows;
/// жёсткий путь на случай её отсутствия — не тот случай, ради которого
/// стоит превращать поиск OpenVPN в ошибку.
fn program_files_dir() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
}

fn standard_bin_dir(program_files: &Path) -> PathBuf {
    program_files.join("OpenVPN").join("bin")
}

fn standard_config_dir(program_files: &Path) -> PathBuf {
    program_files.join("OpenVPN").join("config")
}

/// Собирает `Installation` из уже прочитанных значений реестра (или их
/// отсутствия) и стандартного расположения — чистая функция, без
/// обращения к реестру, поэтому проверяемая напрямую тестами без реального
/// `HKLM\SOFTWARE\OpenVPN`.
///
/// Пустая строка в `bin_dir_value`/`config_dir_value` — то же самое «нет
/// значения», что возвращает `RegKey::query_string` и для отсутствующего
/// значения внутри существующего ключа, и (руками `find_installation`) для
/// вовсе отсутствующего ключа: в обоих случаях соответствующий путь берётся
/// из стандартного расположения. Значения, которые ЕСТЬ, используются как
/// есть, даже если они не совпадают со стандартным расположением — реестр
/// может указывать на нестандартную установку, и переопределять его в этом
/// случае нельзя.
///
/// `gui_exe` обязан существовать как файл — иначе `None`: ключ реестра,
/// оставленный деинсталлятором, или стандартный путь без установки не
/// должны выглядеть как рабочая установка. Для `config_dir` та же проверка
/// не нужна: отсутствующий каталог конфигураций — это просто «конфигураций
/// пока нет», а не «OpenVPN не установлен», и его находит Task 4 при
/// перечислении `.ovpn`-файлов.
fn locate(
    bin_dir_value: &str,
    config_dir_value: &str,
    program_files: &Path,
) -> Option<Installation> {
    let bin_dir = if bin_dir_value.is_empty() {
        standard_bin_dir(program_files)
    } else {
        PathBuf::from(bin_dir_value)
    };
    let config_dir = if config_dir_value.is_empty() {
        standard_config_dir(program_files)
    } else {
        PathBuf::from(config_dir_value)
    };

    let gui_exe = bin_dir.join(GUI_EXE_NAME);
    if !gui_exe.is_file() {
        return None;
    }
    Some(Installation {
        gui_exe,
        config_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn locate_finds_installation_when_registry_bin_dir_has_the_gui_exe() {
        let bin_dir = unique_temp_dir("bin-exists");
        fs::write(bin_dir.join("openvpn-gui.exe"), b"stub").unwrap();
        let config_dir = unique_temp_dir("cfg-verbatim");

        let found = locate(
            &bin_dir.display().to_string(),
            &config_dir.display().to_string(),
            Path::new(r"C:\unused"),
        );

        let inst = found.expect("gui exe существует — обязан найтись");
        assert_eq!(inst.gui_exe, bin_dir.join("openvpn-gui.exe"));
        assert_eq!(inst.config_dir, config_dir);

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn locate_returns_none_when_the_registry_bin_dir_has_no_gui_exe() {
        // Ключ реестра остался от удалённой установки: каталог указан,
        // но exe в нём больше нет.
        let bin_dir = unique_temp_dir("bin-empty");
        let found = locate(&bin_dir.display().to_string(), "", Path::new(r"C:\unused"));
        assert!(found.is_none());
        let _ = fs::remove_dir_all(&bin_dir);
    }

    #[test]
    fn locate_returns_none_when_the_registry_bin_dir_does_not_exist_on_disk() {
        let bin_dir = std::env::temp_dir().join("proxypilot-test-openvpn-missing-entirely");
        let _ = fs::remove_dir_all(&bin_dir); // гарантированно нет на диске
        let found = locate(&bin_dir.display().to_string(), "", Path::new(r"C:\unused"));
        assert!(found.is_none());
    }

    #[test]
    fn locate_falls_back_to_the_standard_bin_dir_when_the_registry_value_is_empty() {
        let program_files = unique_temp_dir("pf-bin-fallback");
        let bin_dir = program_files.join("OpenVPN").join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("openvpn-gui.exe"), b"stub").unwrap();

        let found = locate("", "", &program_files);
        let inst = found.expect("стандартный путь содержит exe — обязан найтись");
        assert_eq!(inst.gui_exe, bin_dir.join("openvpn-gui.exe"));
        assert_eq!(
            inst.config_dir,
            program_files.join("OpenVPN").join("config")
        );

        let _ = fs::remove_dir_all(&program_files);
    }

    #[test]
    fn locate_falls_back_to_the_standard_config_dir_when_the_registry_value_is_empty() {
        let program_files = unique_temp_dir("pf-cfg-fallback");
        let bin_dir = program_files.join("OpenVPN").join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("openvpn-gui.exe"), b"stub").unwrap();

        let found = locate(&bin_dir.display().to_string(), "", &program_files);
        let inst = found.expect("exe существует — обязан найтись");
        assert_eq!(
            inst.config_dir,
            program_files.join("OpenVPN").join("config")
        );

        let _ = fs::remove_dir_all(&program_files);
    }

    #[test]
    fn open_key_is_none_for_a_subkey_that_does_not_exist() {
        // Ключ гарантированно отсутствует: не «OpenVPN», а заведомо
        // несуществующее имя. Проверяем контракт «нет ключа — Ok(None), а
        // не ошибка», не трогая настоящий HKLM\SOFTWARE\OpenVPN.
        let missing = windows::core::w!("Software\\ProxyPilotDefinitelyDoesNotExist12345");
        let result = open_key(missing).expect("отсутствие ключа — не ошибка");
        assert!(result.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn finding_the_real_installation_does_not_fail() {
        // Смоук на живой машине: OpenVPN может быть установлен или нет —
        // оба исхода допустимы, отказ (Err) — нет.
        let found = find_installation().expect("поиск обязан не падать в любом случае");
        match &found {
            Some(inst) => {
                println!(
                    "OpenVPN найден: gui_exe={:?} config_dir={:?}",
                    inst.gui_exe, inst.config_dir
                );
                assert!(
                    inst.gui_exe.is_file(),
                    "нашли путь, но файла нет: {:?}",
                    inst.gui_exe
                );
            }
            None => println!("OpenVPN не найден на этой машине"),
        }
    }

    /// Уникальный временный каталог для теста; на входе гарантированно
    /// чист (на случай, если прошлый прогон упал и не убрал за собой).
    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "proxypilot-test-openvpn-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("временный каталог теста обязан создаваться");
        dir
    }
}
