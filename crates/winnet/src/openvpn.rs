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
//! подключения (`connect`/`disconnect` ниже) вместо честного «не установлен»
//! здесь и сейчас.
//!
//! Реестр читается через `sysproxy::RegKey` — ту же обёртку, что и
//! `sysproxy`/`autostart`, только с другим корнем (`HKEY_LOCAL_MACHINE`
//! вместо `HKEY_CURRENT_USER`, ради чего `open()` и получил параметр
//! `root`): второй путь с сырым `HKEY` здесь не заводится.
//!
//! Представление реестра (32- или 64-битное) наследуется от битности
//! процесса — этот код не просит `KEY_WOW64_64KEY`/`_32KEY` явно. 32-битный
//! OpenVPN на 64-битной Windows регистрируется под
//! `HKLM\SOFTWARE\WOW6432Node\OpenVPN` и ставится в `Program Files (x86)`;
//! ни чтение реестра, ни запасной `%ProgramFiles%`-путь его не найдут. Это
//! осознанно не исправлено: OpenVPN 2.6+ (актуальная линейка на момент
//! написания) только 64-битный, свидетельств 32-битной установки у кого-то
//! из адресатов нет, а отказ здесь безопасен — честное «не установлен»
//! вместо порчи данных. Если такой отчёт придёт, здесь нужен запасной
//! `RegKey::open` с `KEY_READ | KEY_WOW64_32KEY` — сама обёртка это уже
//! умеет через параметр `access`, добавлять для этого нечего, кроме самой
//! попытки.

use std::path::{Path, PathBuf};
use std::process::Command;

use proxypilot_core::net::Ipv4Net;
use windows::core::{w, HRESULT, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_READ};

use crate::sysproxy::RegKey;
use crate::{ovpn_profile, WinNetError};

const SUBKEY: PCWSTR = w!("SOFTWARE\\OpenVPN");
const BIN_DIR: PCWSTR = w!("bin_dir");
const CONFIG_DIR: PCWSTR = w!("config_dir");

/// Имя GUI-исполняемого файла OpenVPN — то же для инсталлятора реестра и
/// для стандартного пути; вынесено константой, чтобы не разойтись между
/// `locate` и возможной будущей правкой.
const GUI_EXE_NAME: &str = "openvpn-gui.exe";

/// Найденная установка OpenVPN: путь к GUI-исполняемому файлу (им управляют
/// `connect`/`disconnect`/`profile_status` ниже), системный каталог
/// конфигураций `config_dir` (из `HKLM\SOFTWARE\OpenVPN\config_dir` или
/// стандартный `%ProgramFiles%\OpenVPN\config`) и пользовательский
/// `user_config_dir` (`%USERPROFILE%\OpenVPN\config`).
///
/// **Разница между ними — не косметика, а вопрос прав на запись.**
/// `config_dir` на обычной установке лежит под `Program Files` и открыт на
/// запись только администратору и `TrustedInstaller` — обычный пользователь
/// его только читает: там может лежать исходный `.ovpn`, который выдал
/// администратор сети (сертификаты, адрес сервера), но положить туда файл
/// самостоятельно, без UAC, он не может. `user_config_dir` — ровно то место,
/// куда сам OpenVPN GUI сохраняет профили без запроса прав (задокументировано
/// в собственном README установки OpenVPN и подтверждено на живой машине:
/// запись тестового файла в `config_dir` отказывает access denied, в
/// `user_config_dir` — проходит). GUI при этом показывает профили из ОБОИХ
/// каталогов разом — значит и наше чтение источника (`find_config_file`
/// ниже) обязано смотреть в оба, а не только в `config_dir`.
///
/// `install_profile` ниже пишет исключительно в `user_config_dir` — см. её
/// докблок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub gui_exe: PathBuf,
    pub config_dir: PathBuf,
    pub user_config_dir: PathBuf,
}

/// То, что `locate` знает сама — координаты установки без обращения к
/// `%USERPROFILE%`. Отдельный от [`Installation`] тип специально: `locate`
/// остаётся чистой функцией с теми же аргументами, что и раньше (ради тех
/// же тестов), а резолв `user_config_dir`, который умеет ОТКАЗЫВАТЬ
/// (`WinNetError::UserProfileNotFound`), происходит только в
/// `find_installation` — и только когда установка вообще найдена: машине
/// без OpenVPN не с чего спотыкаться о переменную окружения, которая ей не
/// понадобится.
struct SystemPaths {
    gui_exe: PathBuf,
    config_dir: PathBuf,
}

/// Ищет установленный OpenVPN. `Ok(None)` — не установлен; это не ошибка,
/// а такой же законный исход, как «установлен». Если установка найдена, но
/// `%USERPROFILE%` не резолвится — это уже `Err`
/// (`WinNetError::UserProfileNotFound`, докблок [`Installation`]): без него
/// `install_profile` ниже не знает, куда обязана писать, и подставлять
/// вместо него `config_dir` означало бы гарантированный access denied чуть
/// позже вместо честного отказа здесь и сейчас.
pub fn find_installation() -> Result<Option<Installation>, WinNetError> {
    let (bin_dir_value, config_dir_value) = match open_key(SUBKEY)? {
        Some(key) => read_registry_values(&key)?,
        // Ключа нет вовсе — OpenVPN, скорее всего, не ставили. Пустые
        // строки заставляют `locate` ниже взять оба пути из стандартного
        // расположения, а не считать установку найденной по совпадению.
        None => (String::new(), String::new()),
    };
    let Some(found) = locate(&bin_dir_value, &config_dir_value, &program_files_dir()) else {
        return Ok(None);
    };
    let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    Ok(Some(Installation {
        gui_exe: found.gui_exe,
        config_dir: found.config_dir,
        user_config_dir: resolve_user_config_dir(user_profile)?,
    }))
}

/// Вычисляет `user_config_dir` из уже прочитанного (или отсутствующего)
/// `%USERPROFILE%` — чистая функция ради теста
/// (`resolve_user_config_dir_fails_clearly_when_userprofile_is_unset`):
/// подставлять реальное окружение процесса в тест значило бы либо трогать
/// настоящий профиль пользователя этой машины, либо мутировать глобальную
/// переменную окружения параллельно с другими тестами. `None` — тот самый
/// редкий, но реальный случай (служба, нестандартная среда запуска), когда
/// переменной в окружении процесса нет вовсе.
fn resolve_user_config_dir(user_profile: Option<PathBuf>) -> Result<PathBuf, WinNetError> {
    let profile = user_profile.ok_or(WinNetError::UserProfileNotFound)?;
    Ok(profile.join("OpenVPN").join("config"))
}

/// Читает `bin_dir` и `config_dir` из уже открытого ключа, в этом порядке.
/// Вынесена отдельной функцией — а не оставлена кортежным выражением прямо
/// в `find_installation` — специально ради теста
/// (`find_installation_reads_bin_dir_and_config_dir_into_the_right_slots`),
/// который ловит случайную перестановку `BIN_DIR`/`CONFIG_DIR` местами. Без
/// этого разделения перестановку не поймал бы ни один тест: `locate`
/// тестируется напрямую, с уже верно расставленными аргументами, а живой
/// смоук на этой машине при перестановке просто получил бы `None` вместо
/// `Some` (`bin_dir` тогда указывал бы на каталог с конфигурациями, где
/// `openvpn-gui.exe` нет) и молча прошёл бы — он допускает оба исхода.
fn read_registry_values(key: &RegKey) -> Result<(String, String), WinNetError> {
    Ok((key.query_string(BIN_DIR)?, key.query_string(CONFIG_DIR)?))
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
/// не нужна: отсутствующий системный каталог конфигураций — это просто
/// «администратор там пока ничего не оставил», а не «OpenVPN не
/// установлен» — этот каталог только читается (докблок [`Installation`]),
/// и `install_profile` в него не пишет и не создаёт его.
fn locate(
    bin_dir_value: &str,
    config_dir_value: &str,
    program_files: &Path,
) -> Option<SystemPaths> {
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
    Some(SystemPaths {
        gui_exe,
        config_dir,
    })
}

/// Расширение файла профиля в каталоге конфигураций OpenVPN —
/// `openvpn-gui.exe` подбирает конфигурации именно по `*.ovpn` в этом
/// каталоге.
const PROFILE_EXTENSION: &str = "ovpn";

/// Известно ли ещё, что `Installation` указывает на настоящую установку
/// OpenVPN. `find_installation` проверяет `gui_exe.is_file()` один раз в
/// момент поиска; между тем моментом и вызовом любой из функций ниже
/// OpenVPN мог быть удалён — проверяем заново, а не доверяем однажды
/// найденному пути молча. Общая точка для всех четырёх функций задачи 4:
/// расходиться в этой проверке означало бы, что одни отказывают честно,
/// а другие — тихой попыткой записи в путь, которого больше нет.
fn ensure_still_installed(inst: &Installation) -> Result<(), WinNetError> {
    if !inst.gui_exe.is_file() {
        return Err(WinNetError::OpenVpnNotFound {
            gui_exe: inst.gui_exe.clone(),
        });
    }
    Ok(())
}

/// Путь к нашему профилю в ПОЛЬЗОВАТЕЛЬСКОМ каталоге конфигураций OpenVPN
/// (`user_config_dir`, не `config_dir` — докблок [`Installation`]). `name`
/// — то же самое значение, что уходит в `--command connect|disconnect
/// <name>` (`build_gui_command`, спека 8.3): один параметр на оба места, а
/// не два независимых имени (файла и команды), которые могли бы
/// разойтись.
fn profile_path(inst: &Installation, name: &str) -> PathBuf {
    inst.user_config_dir
        .join(format!("{name}.{PROFILE_EXTENSION}"))
}

/// Кладёт готовый текст профиля под собственным именем `name` в
/// ПОЛЬЗОВАТЕЛЬСКИЙ каталог конфигураций OpenVPN — `inst.user_config_dir`,
/// НЕ `inst.config_dir`. Существующие профили пользователя не читаются, не
/// перемещаются, не переименовываются и не удаляются — пишется ровно один
/// файл `<name>.ovpn`, остальное содержимое каталога вообще не
/// просматривается.
///
/// **Почему не `config_dir`.** `config_dir` — системный каталог
/// (`HKLM\SOFTWARE\OpenVPN\config_dir`, на обычной установке — под `Program
/// Files`) — открыт на запись только администратору и `TrustedInstaller`;
/// обычный пользователь получает туда только чтение. Прежняя версия этой
/// функции писала именно туда и предполагала обратное — что каталог
/// доступен на запись, раз сам OpenVPN GUI сохраняет профили без UAC.
/// Предположение было по форме правдоподобным и по факту неверным: GUI
/// действительно сохраняет профили без UAC, но не в `config_dir`, а в
/// `user_config_dir` (`%USERPROFILE%\OpenVPN\config`) — что и подтвердила
/// проверка на живой машине (запись пробного файла в `config_dir` отказала
/// access denied, в `user_config_dir` — прошла). Прав администратора эта
/// функция по-прежнему не требует: она вообще не пытается писать туда, где
/// их бы потребовалось.
///
/// Пользовательский каталог создаётся, если его ещё нет: отсутствие
/// каталога само по себе не признак «OpenVPN не установлен» (см. докблок
/// `locate` выше), а `install_profile` вполне может оказаться первым, что
/// кладёт в него хоть что-то — как это делает и сам OpenVPN GUI при первом
/// сохранённом профиле.
///
/// `contents` здесь принимается уже готовым, без проверки, что это вообще
/// прошло через `ovpn_profile::build_profile`: эта функция не знает и не
/// обязана знать, откуда взялся текст. Обычный вызывающий код — не эта
/// функция напрямую, а [`build_and_install_profile`] ниже: она одна во
/// всём крейте зовёт `build_profile` и пробрасывает её отказ на структурно
/// битом источнике. Вызывающий, который соберёт `contents` сам в обход
/// `build_profile` (например, строковой конкатенацией) и передаст сюда,
/// откроет второй путь мимо этого отказа — `install_profile` его не
/// поймает, потому что ей нечем отличить корректно собранный профиль от
/// самодельного текста.
pub fn install_profile(
    inst: &Installation,
    name: &str,
    contents: &str,
) -> Result<PathBuf, WinNetError> {
    ensure_still_installed(inst)?;
    std::fs::create_dir_all(&inst.user_config_dir).map_err(|source| WinNetError::ProfileWrite {
        path: inst.user_config_dir.clone(),
        source,
    })?;
    let path = profile_path(inst, name);
    std::fs::write(&path, contents).map_err(|source| WinNetError::ProfileWrite {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Ищет файл `filename` среди обоих каталогов конфигураций OpenVPN — сперва
/// в пользовательском (`user_config_dir`), и только если там пусто — в
/// системном (`config_dir`). Нужна для чтения ИСХОДНЫХ файлов (например,
/// `.ovpn` с сертификатами и адресом сервера, из которого
/// `build_and_install_profile` собирает наш профиль) — `install_profile`
/// сама по себе только пишет и этой функцией не пользуется.
///
/// Явное пользовательское расположение выигрывает нарочно: обычный
/// пользователь способен положить файл только в `user_config_dir` (в
/// `config_dir` он может только читать — докблок [`Installation`]), поэтому
/// если файл почему-то оказался в обоих местах, актуальнее считается тот,
/// что мог положить сам пользователь только что, а не более старая копия
/// из системного каталога, которую администратор мог оставить при
/// установке.
pub fn find_config_file(inst: &Installation, filename: &str) -> Option<PathBuf> {
    let user_path = inst.user_config_dir.join(filename);
    if user_path.is_file() {
        return Some(user_path);
    }
    let system_path = inst.config_dir.join(filename);
    if system_path.is_file() {
        return Some(system_path);
    }
    None
}

/// Собирает профиль (`ovpn_profile::build_profile`) и кладёт его под
/// `name` тем же путём, что и `install_profile`. Единственное место в
/// этом крейте, вызывающее `build_profile`: задача 2 сознательно сделала
/// её отказывающейся на структурно битом источнике (незакрытый
/// inline-блок) вместо того, чтобы гадать — здесь этот `Err` доходит до
/// вызывающего кода как есть, через `?`, и ничего не пишется на диск,
/// если сборка не удалась.
pub fn build_and_install_profile(
    inst: &Installation,
    name: &str,
    source: &str,
    routes: &[Ipv4Net],
) -> Result<PathBuf, WinNetError> {
    let contents = ovpn_profile::build_profile(source, routes)?;
    install_profile(inst, name, &contents)
}

/// Строит `Command` для `openvpn-gui.exe --command <verb> <name>`, не
/// запуская его. Чистая функция ради теста: конструирование командной
/// строки проверяется без побочных эффектов, отдельно от решения,
/// запускать процесс или нет.
fn build_gui_command(inst: &Installation, verb: &str, name: &str) -> Command {
    let mut cmd = Command::new(&inst.gui_exe);
    cmd.arg("--command").arg(verb).arg(name);
    cmd
}

/// Запускает `openvpn-gui.exe --command <verb> <name>` и не ждёт
/// завершения процесса (`spawn`, а не `status`/`output`): при уже
/// запущенном GUI это почти мгновенный обмен через именованный канал с
/// интерактивной службой, а если GUI ещё не запущен — сам процесс
/// становится долгоживущим окном в трее, и дождаться его завершения
/// значило бы заблокироваться на неопределённое время, вплоть до выхода
/// пользователя из OpenVPN GUI.
fn run_gui_command(inst: &Installation, verb: &str, name: &str) -> Result<(), WinNetError> {
    ensure_still_installed(inst)?;
    build_gui_command(inst, verb, name)
        .spawn()
        .map_err(|source| WinNetError::OpenVpnGuiLaunch {
            exe: inst.gui_exe.clone(),
            source,
        })?;
    Ok(())
}

/// Поднимает наш туннель: `openvpn-gui.exe --command connect <name>`.
/// Строго через GUI, не `openvpn.exe` напрямую — запуск в обход
/// интерактивной службы не добавляет маршруты (докблок модуля,
/// `docs/design.md` §2.4/8.3).
///
/// `Ok` здесь значит только «команда доставлена GUI и его процесс
/// стартовал» (`run_gui_command` не ждёт результата, см. её докблок) — не
/// «туннель поднят». Само подключение асинхронно и может ещё не
/// завершиться, когда эта функция уже вернула `Ok`, а то и провалиться
/// позже (неверный пароль, недоступный сервер) без единого способа узнать
/// об этом отсюда. Живость — вопрос к `tunnel_log::liveness(name)` (лог
/// самого `openvpn-gui.exe` для этого профиля), не к результату этого
/// вызова и не к таблице маршрутов: имя интерфейса Windows, которое
/// показала бы `tunnel_state`, не привязано к имени профиля вовсе (round
/// 1 задачи 7 полагался на это и ошибся — см. докблок `tunnel_log`).
pub fn connect(inst: &Installation, name: &str) -> Result<(), WinNetError> {
    run_gui_command(inst, "connect", name)
}

/// Опускает наш туннель: `openvpn-gui.exe --command disconnect <name>`.
/// `Ok` здесь означает то же, что и у [`connect`] — команда доставлена, а
/// не то, что туннель уже опущен.
pub fn disconnect(inst: &Installation, name: &str) -> Result<(), WinNetError> {
    run_gui_command(inst, "disconnect", name)
}

/// Что этот вызов знает о профиле `name` на диске, не запуская ничего и не
/// читая таблицу маршрутов.
///
/// Название нарочно не «tunnel»/«туннель»: этот вызов отвечает ровно на
/// один вопрос — «файл профиля `<name>.ovpn` есть в каталоге конфигураций
/// OpenVPN?» — и не пытается сказать, поднято ли само подключение. У
/// `openvpn-gui.exe` нет синхронного текстового запроса состояния —
/// только `connect` и `disconnect` (докблок модуля, `docs/design.md`
/// §8.3). Живое состояние — `tunnel_log::liveness(name)`, разбор
/// собственного лога `openvpn-gui.exe` для этого профиля (ключуется
/// именем, которым владеем мы сами, — не именем интерфейса адаптера,
/// которое назначает Windows/драйвер VPN и с профилем никак не связывает;
/// см. докблок `tunnel_log` про то, как первая версия задачи 7 на этом
/// ошиблась). Тип, названный «TunnelStatus», рядом с вариантом
/// `Installed` выглядел бы как «туннель поднят» для того, кто читает
/// только сигнатуру, а не докблок, — отсюда `ProfileStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileStatus {
    /// Файла `<name>.ovpn` в каталоге конфигураций нет — профиль ещё не
    /// установлен (или пользователь его удалил).
    NotInstalled,
    /// Файл профиля есть на диске. Поднято ли сейчас само подключение —
    /// эта функция не знает, см. докблок [`ProfileStatus`].
    Installed,
}

/// Установлен ли профиль `name` на диске каталога конфигураций OpenVPN.
/// См. докблок [`ProfileStatus`] про то, чего эта функция сознательно не
/// утверждает.
pub fn profile_status(inst: &Installation, name: &str) -> Result<ProfileStatus, WinNetError> {
    ensure_still_installed(inst)?;
    Ok(if profile_path(inst, name).is_file() {
        ProfileStatus::Installed
    } else {
        ProfileStatus::NotInstalled
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE,
    };

    /// Одноразовый подключ HKCU для теста ниже: `find_installation` сама
    /// читает только `HKLM`, а писать туда этому крейту нельзя ни при каких
    /// условиях (см. докблок модуля и `CLAUDE.md`) — даже собственный,
    /// потом же удаляемый подключ. HKCU для тестовой записи можно: тем же
    /// приёмом уже пользуется `autostart::tests::TestSubkeyGuard`, здесь —
    /// его младшая копия ровно под то, что нужно `read_registry_values`
    /// (ей всё равно, из-под какого корня открыт `RegKey` — она лишь читает
    /// по именам значений).
    struct ScratchKey {
        subkey_utf16: Vec<u16>,
    }

    impl ScratchKey {
        fn new() -> Self {
            let name = format!("Software\\ProxyPilotOpenvpnSelfTest-{}", std::process::id());
            let subkey_utf16: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let subkey_ptr = PCWSTR::from_raw(subkey_utf16.as_ptr());

            let mut hkey = HKEY::default();
            // SAFETY: HKEY_CURRENT_USER — предопределённый корень, всегда
            // валиден; `subkey_ptr` указывает на `subkey_utf16` — живой на
            // весь этот вызов буфер с завершающим нулём; класс и атрибуты
            // безопасности не нужны — `PCWSTR::null()` и `None`;
            // `phkresult` указывает на живую локальную переменную, которую
            // API заполняет только при успехе.
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    subkey_ptr,
                    0,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_WRITE,
                    None,
                    &mut hkey,
                    None,
                )
            }
            .ok()
            .expect("тестовый подключ обязан создаваться");
            // SAFETY: хендл только что получен от RegCreateKeyExW и больше
            // никому не нужен — RegKey::open ниже откроет тот же путь
            // заново своим собственным хендлом.
            let _ = unsafe { RegCloseKey(hkey) };
            Self { subkey_utf16 }
        }

        fn subkey(&self) -> PCWSTR {
            PCWSTR::from_raw(self.subkey_utf16.as_ptr())
        }
    }

    impl Drop for ScratchKey {
        fn drop(&mut self) {
            // SAFETY: HKEY_CURRENT_USER — предопределённый корень;
            // `self.subkey_utf16` — поле этого же значения, живо до конца
            // `drop`. Имя несёт PID процесса, поэтому это гарантированно
            // подключ, созданный `new` этим же прогоном — удаление не
            // заденет ничьи чужие данные. Ошибку игнорируем сознательно:
            // падать при уборке за собой хуже, чем оставить пустой подключ.
            let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, self.subkey()) };
        }
    }

    #[test]
    fn find_installation_reads_bin_dir_and_config_dir_into_the_right_slots() {
        // Раньше ничего не проверяло порядок: `read_registry_values`
        // возвращает кортеж `(bin_dir, config_dir)` позиционно, и случайная
        // перестановка `BIN_DIR`/`CONFIG_DIR` местами не уронила бы ни один
        // из тестов `locate` (они передают уже правильно расставленные
        // строки) и не была бы поймана живым смоуком (при перестановке он
        // просто получил бы `None`, а `None` — допустимый исход и там).
        // Здесь под собственными именами значений пишутся два заведомо
        // разных, легко различимых пути, и проверяется, что каждый остался
        // в своей роли.
        let scratch = ScratchKey::new();
        {
            let key = RegKey::open(HKEY_CURRENT_USER, scratch.subkey(), KEY_WRITE)
                .expect("тестовый подключ обязан открываться на запись");
            key.set_string(BIN_DIR, r"C:\ProxyPilotTest\bin-marker")
                .expect("bin_dir обязан записаться");
            key.set_string(CONFIG_DIR, r"C:\ProxyPilotTest\config-marker")
                .expect("config_dir обязан записаться");
        }

        let key = RegKey::open(HKEY_CURRENT_USER, scratch.subkey(), KEY_READ)
            .expect("тестовый подключ обязан открываться на чтение");
        let (bin_dir_value, config_dir_value) =
            read_registry_values(&key).expect("значения обязаны читаться");

        assert_eq!(bin_dir_value, r"C:\ProxyPilotTest\bin-marker");
        assert_eq!(config_dir_value, r"C:\ProxyPilotTest\config-marker");
    }

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

    #[test]
    fn finding_the_real_installation_does_not_fail() {
        // Смоук на живой машине: OpenVPN может быть установлен или нет —
        // оба исхода допустимы, отказ (Err) — нет. Если установлен,
        // `%USERPROFILE%` на реальной интерактивной сессии Windows всегда
        // задан, так что `user_config_dir` тоже обязан резолвиться, а не
        // проваливаться `UserProfileNotFound`.
        let found = find_installation().expect("поиск обязан не падать в любом случае");
        match &found {
            Some(inst) => {
                println!(
                    "OpenVPN найден: gui_exe={:?} config_dir={:?} user_config_dir={:?}",
                    inst.gui_exe, inst.config_dir, inst.user_config_dir
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

    // ---- Задача 4: install_profile / connect / disconnect / profile_status ----
    //
    // Ни один из этих тестов не запускает openvpn-gui.exe и не поднимает
    // туннель — контроллер сессии прямо запрещает живой прогон на этой
    // машине (CLAUDE.md, «Живые проверки, которые не делает агент»).
    // `connect`/`disconnect` проверяются на уровне построения команды
    // (`build_gui_command` — чистая функция, ничего не запускает) и на
    // отказе при несуществующем `gui_exe`: `ensure_still_installed`
    // обрывает выполнение раньше, чем дошло бы до `Command::spawn`.

    use proxypilot_core::net::Ipv4Net;
    use std::str::FromStr;

    /// `Installation`, чей `gui_exe` заведомо не существует на диске —
    /// для проверки отказа «OpenVPN не найден». `name` рождает два разных
    /// временных каталога (системный и пользовательский) — им всё равно
    /// не суждено использоваться дальше `ensure_still_installed`.
    fn fake_installation_with_missing_gui(name: &str) -> Installation {
        let config_dir = unique_temp_dir(&format!("{name}-config"));
        let user_config_dir = unique_temp_dir(&format!("{name}-user-config"));
        Installation {
            gui_exe: config_dir.join("definitely-does-not-exist-openvpn-gui.exe"),
            config_dir,
            user_config_dir,
        }
    }

    /// `Installation` с настоящим (хоть и пустым) файлом на месте
    /// `gui_exe`, чтобы `ensure_still_installed` пропускала её дальше —
    /// сам файл никогда не запускается. Оба каталога конфигураций — под
    /// `std::env::temp_dir()`, никогда не настоящие `Program Files`/
    /// `%USERPROFILE%` этой машины (см. `cleanup` ниже про то, почему это
    /// не просто соглашение, а проверяемое условие).
    fn installation_with_stub_gui(name: &str) -> Installation {
        let bin_dir = unique_temp_dir(&format!("{name}-bin"));
        fs::write(bin_dir.join(GUI_EXE_NAME), b"stub").unwrap();
        let config_dir = unique_temp_dir(&format!("{name}-config"));
        let user_config_dir = unique_temp_dir(&format!("{name}-user-config"));
        Installation {
            gui_exe: bin_dir.join(GUI_EXE_NAME),
            config_dir,
            user_config_dir,
        }
    }

    /// Убирает временные каталоги теста. Каждый вызывающий здесь передаёт
    /// фикстуру из `installation_with_stub_gui`/`unique_temp_dir`, то есть
    /// пути всегда лежат под `std::env::temp_dir()` — но эта функция
    /// удаляет рекурсивно, а `Installation` может быть настоящей (задачи
    /// 5-7 читают её из `find_installation`). Один неверный copy-paste на
    /// реальную установку без этой проверки снёс бы каталог `bin`
    /// настоящего OpenVPN на машине разработчика. Проверка — не
    /// перестраховка «на всякий случай», а единственное, что отличает эту
    /// функцию от опасной: без неё она безопасна только по соглашению
    /// вызывающих, а не по построению.
    fn cleanup(inst: &Installation) {
        let is_scratch = |p: &Path| p.starts_with(std::env::temp_dir());
        if is_scratch(&inst.config_dir) {
            let _ = fs::remove_dir_all(&inst.config_dir);
        }
        if is_scratch(&inst.user_config_dir) {
            let _ = fs::remove_dir_all(&inst.user_config_dir);
        }
        if let Some(bin_dir) = inst.gui_exe.parent() {
            if is_scratch(bin_dir) {
                let _ = fs::remove_dir_all(bin_dir);
            }
        }
    }

    #[test]
    fn build_gui_command_for_connect_targets_our_profile_by_name() {
        let inst = installation_with_stub_gui("cmd-connect");
        let cmd = build_gui_command(&inst, "connect", "proxypilot-office");
        assert_eq!(cmd.get_program(), inst.gui_exe.as_os_str());
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args, ["--command", "connect", "proxypilot-office"]);
        cleanup(&inst);
    }

    #[test]
    fn build_gui_command_for_disconnect_targets_our_profile_by_name() {
        let inst = installation_with_stub_gui("cmd-disconnect");
        let cmd = build_gui_command(&inst, "disconnect", "proxypilot-office");
        assert_eq!(cmd.get_program(), inst.gui_exe.as_os_str());
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args, ["--command", "disconnect", "proxypilot-office"]);
        cleanup(&inst);
    }

    #[test]
    fn build_gui_command_survives_a_program_path_with_spaces() {
        // Место установки по умолчанию — "C:\Program Files\OpenVPN\bin\
        // openvpn-gui.exe": пробел в компоненте пути. `Command::arg`
        // передаёт каждый аргумент отдельным значением, не конкатенацией
        // строк, поэтому классическая проблема разбора командной строки
        // с пробелами здесь структурно невозможна — тест это фиксирует,
        // а не только полагается на устройство `std::process::Command`.
        let base = unique_temp_dir("cmd-spaces");
        let bin_dir = base.join("Program Files").join("OpenVPN").join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join(GUI_EXE_NAME), b"stub").unwrap();
        let inst = Installation {
            gui_exe: bin_dir.join(GUI_EXE_NAME),
            config_dir: base.join("Program Files").join("OpenVPN").join("config"),
            user_config_dir: base.join("Jane Doe").join("OpenVPN").join("config"),
        };

        let cmd = build_gui_command(&inst, "connect", "proxypilot-office");
        assert_eq!(cmd.get_program(), inst.gui_exe.as_os_str());
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args, ["--command", "connect", "proxypilot-office"]);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn connect_fails_clearly_when_openvpn_is_not_found() {
        let inst = fake_installation_with_missing_gui("connect-missing-gui");
        let err = connect(&inst, "proxypilot-office").expect_err("gui_exe отсутствует на диске");
        assert!(matches!(err, WinNetError::OpenVpnNotFound { .. }));
        cleanup(&inst);
    }

    #[test]
    fn disconnect_fails_clearly_when_openvpn_is_not_found() {
        let inst = fake_installation_with_missing_gui("disconnect-missing-gui");
        let err = disconnect(&inst, "proxypilot-office").expect_err("gui_exe отсутствует на диске");
        assert!(matches!(err, WinNetError::OpenVpnNotFound { .. }));
        cleanup(&inst);
    }

    #[test]
    fn install_profile_writes_under_our_own_name() {
        let inst = installation_with_stub_gui("install-basic");
        let path = install_profile(&inst, "proxypilot-office", "client\ndev tun\n")
            .expect("запись профиля обязана удаться");
        assert_eq!(path, inst.user_config_dir.join("proxypilot-office.ovpn"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "client\ndev tun\n");
        // Системный каталог (в жизни — под Program Files, недоступный на
        // запись обычному пользователю) `installation_with_stub_gui` уже
        // создал пустым для чистоты теста — install_profile не должен
        // положить в него ни одного файла.
        assert_eq!(fs::read_dir(&inst.config_dir).unwrap().count(), 0);
        cleanup(&inst);
    }

    #[test]
    fn install_profile_never_writes_into_the_system_config_dir() {
        // Даже если системный каталог существует и уже что-то содержит
        // (как на реальной машине — файл, положенный туда
        // администратором), install_profile не должен ни писать туда, ни
        // трогать то, что там уже лежит: запись идёт только в
        // user_config_dir.
        let inst = installation_with_stub_gui("install-system-untouched");
        fs::create_dir_all(&inst.config_dir).unwrap();
        let system_marker = inst.config_dir.join("admin-placed-profile.ovpn");
        fs::write(&system_marker, "профиль администратора\n").unwrap();

        install_profile(&inst, "proxypilot-office", "наш профиль\n")
            .expect("запись обязана удаться");

        assert_eq!(
            fs::read_to_string(&system_marker).unwrap(),
            "профиль администратора\n"
        );
        assert!(
            !inst.config_dir.join("proxypilot-office.ovpn").exists(),
            "наш профиль не должен появляться в системном каталоге"
        );
        cleanup(&inst);
    }

    #[test]
    fn install_profile_does_not_touch_neighbouring_files() {
        let inst = installation_with_stub_gui("install-neighbours");
        fs::create_dir_all(&inst.user_config_dir).unwrap();
        let neighbour = inst.user_config_dir.join("my-existing-work-profile.ovpn");
        fs::write(&neighbour, "исходный пользовательский профиль\n").unwrap();

        install_profile(&inst, "proxypilot-office", "наш профиль\n")
            .expect("запись профиля обязана удаться");

        assert_eq!(
            fs::read_to_string(&neighbour).unwrap(),
            "исходный пользовательский профиль\n"
        );
        let mut names: Vec<_> = fs::read_dir(&inst.user_config_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "my-existing-work-profile.ovpn".to_string(),
                "proxypilot-office.ovpn".to_string(),
            ]
        );
        cleanup(&inst);
    }

    #[test]
    fn install_profile_overwrites_an_existing_file_under_our_own_name() {
        // Задача 5 перестраивает и перезаписывает наш профиль при каждой
        // смене списка офисных подсетей — перезапись поверх старой версии
        // обычный ход дел, не редкий край.
        let inst = installation_with_stub_gui("install-overwrite");
        install_profile(&inst, "proxypilot-office", "версия 1\n")
            .expect("первая запись обязана удаться");

        let path = install_profile(&inst, "proxypilot-office", "версия 2\n")
            .expect("повторная запись обязана удаться");

        assert_eq!(fs::read_to_string(&path).unwrap(), "версия 2\n");
        cleanup(&inst);
    }

    #[test]
    fn install_profile_round_trips_a_user_config_dir_with_spaces() {
        // %USERPROFILE% нередко содержит пробел в имени учётной записи
        // ("C:\Users\Jane Doe"), а от него наследуется весь путь
        // "...\OpenVPN\config" — пробел в компоненте пути, не редкий край.
        let base = unique_temp_dir("install-spaces");
        let bin_dir = base.join("Program Files").join("OpenVPN").join("bin");
        let user_config_dir = base.join("Jane Doe").join("OpenVPN").join("config");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join(GUI_EXE_NAME), b"stub").unwrap();
        let inst = Installation {
            gui_exe: bin_dir.join(GUI_EXE_NAME),
            config_dir: base.join("Program Files").join("OpenVPN").join("config"),
            user_config_dir: user_config_dir.clone(),
        };

        let path = install_profile(&inst, "proxypilot-office", "содержимое профиля\n")
            .expect("путь с пробелом обязан работать");
        assert_eq!(path, user_config_dir.join("proxypilot-office.ovpn"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "содержимое профиля\n");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn install_profile_creates_the_user_config_dir_if_it_does_not_exist_yet() {
        let inst = installation_with_stub_gui("install-create-dir");
        // user_config_dir специально удаляем — как и объяснено в докблоке
        // `install_profile`, "конфигураций пока нет" не значит "не
        // установлен".
        fs::remove_dir_all(&inst.user_config_dir).unwrap();
        assert!(!inst.user_config_dir.exists());

        let path = install_profile(&inst, "proxypilot-office", "x\n")
            .expect("каталог конфигураций обязан создаться сам");
        assert!(path.is_file());
        cleanup(&inst);
    }

    #[test]
    fn install_profile_fails_clearly_when_openvpn_is_not_found() {
        let inst = fake_installation_with_missing_gui("install-missing-gui");
        let err = install_profile(&inst, "proxypilot-office", "x\n")
            .expect_err("gui_exe отсутствует на диске");
        assert!(matches!(err, WinNetError::OpenVpnNotFound { .. }));
        assert!(
            !inst.user_config_dir.join("proxypilot-office.ovpn").exists(),
            "профиль не должен писаться, если установка не подтверждена"
        );
        cleanup(&inst);
    }

    #[test]
    fn profile_status_reports_not_installed_when_the_profile_file_is_absent() {
        let inst = installation_with_stub_gui("status-absent");
        let got = profile_status(&inst, "proxypilot-office").expect("статус обязан читаться");
        assert_eq!(got, ProfileStatus::NotInstalled);
        cleanup(&inst);
    }

    #[test]
    fn profile_status_reports_installed_when_the_profile_file_is_present() {
        let inst = installation_with_stub_gui("status-present");
        install_profile(&inst, "proxypilot-office", "x\n").unwrap();
        let got = profile_status(&inst, "proxypilot-office").expect("статус обязан читаться");
        assert_eq!(got, ProfileStatus::Installed);
        cleanup(&inst);
    }

    #[test]
    fn profile_status_fails_clearly_when_openvpn_is_not_found() {
        let inst = fake_installation_with_missing_gui("status-missing-gui");
        let err =
            profile_status(&inst, "proxypilot-office").expect_err("gui_exe отсутствует на диске");
        assert!(matches!(err, WinNetError::OpenVpnNotFound { .. }));
        cleanup(&inst);
    }

    fn routes() -> Vec<Ipv4Net> {
        // RFC 5737 — документационный диапазон, не настоящая офисная сеть
        // (CLAUDE.md).
        vec![Ipv4Net::from_str("203.0.113.0/24").unwrap()]
    }

    #[test]
    fn build_and_install_profile_writes_the_built_profile() {
        let inst = installation_with_stub_gui("build-install-ok");
        let source = "client\ndev tun\n";

        let path = build_and_install_profile(&inst, "proxypilot-office", source, &routes())
            .expect("корректный источник обязан собраться и записаться");

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("client"));
        assert!(written.contains("route 203.0.113.0 255.255.255.0"));
        cleanup(&inst);
    }

    #[test]
    fn build_and_install_profile_propagates_a_profile_error_without_writing_anything() {
        // Незакрытый inline-блок — build_profile обязана отказаться, а не
        // выдать правдоподобный, но битый результат (задача 2). Эта
        // функция — первый вызывающий, и ошибка обязана дойти до
        // вызывающего кода, а не быть проглоченной.
        let inst = installation_with_stub_gui("build-install-err");
        let broken_source = "client\n<ca>\nCERT\n";

        let err = build_and_install_profile(&inst, "proxypilot-office", broken_source, &[])
            .expect_err("незакрытый inline-блок обязан быть отказом, не догадкой");
        assert!(matches!(err, WinNetError::Profile(_)));
        assert!(
            !inst.user_config_dir.join("proxypilot-office.ovpn").exists(),
            "ничего не должно записываться при отказе сборки"
        );
        cleanup(&inst);
    }

    // ---- Fix: install_profile писал в системный каталог (Program Files),
    // недоступный на запись обычному пользователю. Профиль кладётся в
    // user_config_dir; find_config_file читает исходный .ovpn из обоих
    // мест, предпочитая явно пользовательское расположение.

    #[test]
    fn find_config_file_prefers_the_user_directory_when_present_in_both() {
        let inst = installation_with_stub_gui("find-both");
        fs::create_dir_all(&inst.config_dir).unwrap();
        fs::create_dir_all(&inst.user_config_dir).unwrap();
        fs::write(inst.config_dir.join("source.ovpn"), "системная версия\n").unwrap();
        fs::write(
            inst.user_config_dir.join("source.ovpn"),
            "пользовательская версия\n",
        )
        .unwrap();

        let found = find_config_file(&inst, "source.ovpn").expect("файл есть в обоих местах");
        assert_eq!(found, inst.user_config_dir.join("source.ovpn"));
        cleanup(&inst);
    }

    #[test]
    fn find_config_file_falls_back_to_the_system_directory() {
        let inst = installation_with_stub_gui("find-system-only");
        fs::create_dir_all(&inst.config_dir).unwrap();
        fs::write(inst.config_dir.join("source.ovpn"), "системная версия\n").unwrap();

        let found = find_config_file(&inst, "source.ovpn").expect("файл есть в системном каталоге");
        assert_eq!(found, inst.config_dir.join("source.ovpn"));
        cleanup(&inst);
    }

    #[test]
    fn find_config_file_returns_none_when_absent_from_both() {
        let inst = installation_with_stub_gui("find-neither");
        assert!(find_config_file(&inst, "source.ovpn").is_none());
        cleanup(&inst);
    }

    #[test]
    fn resolve_user_config_dir_fails_clearly_when_userprofile_is_unset() {
        let err = resolve_user_config_dir(None)
            .expect_err("отсутствие %USERPROFILE% обязано быть явным отказом");
        assert!(matches!(err, WinNetError::UserProfileNotFound));
    }

    #[test]
    fn resolve_user_config_dir_joins_openvpn_config_onto_the_profile() {
        // Не настоящий домашний каталог — придуманный путь ради теста
        // (CLAUDE.md, «Данные»).
        let profile = PathBuf::from(r"C:\Users\ExampleUser");
        let got = resolve_user_config_dir(Some(profile.clone()))
            .expect("заданный профиль обязан резолвиться");
        assert_eq!(got, profile.join("OpenVPN").join("config"));
    }
}
