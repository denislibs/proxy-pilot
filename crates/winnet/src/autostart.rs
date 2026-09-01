//! Автозапуск с Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
//! значение `ProxyPilot`.
//!
//! `HKCU`, не `HKLM`: второй требует администратора, а весь план обязан
//! обходиться без UAC — единственная функция, которой позволено его просить
//! (статический IP), появится позже и отдельно. `Run` под HKCU пользователь
//! и так может редактировать сам, поэтому права те же, что у `sysproxy`.
//!
//! Путь исполняемого файла ПИШЕТСЯ в кавычках: без них Windows разбирает
//! `C:\Program Files\ProxyPilot\proxypilot.exe` по первому пробелу и получает
//! команду на запуск несуществующего `C:\Program`. Но ЧИТАЕТСЯ запись
//! значительно терпимее — потому что значение `Run` это КОМАНДНАЯ СТРОКА, а
//! не путь: инсталлятор, более старая сборка или человек руками обычно
//! пишут без кавычек, в произвольном регистре и почти всегда с аргументами
//! (`"C:\...\app.exe" --min`, `C:\...\app.exe -autostart` — на машине, где
//! это писалось, такими были 6 из 10 записей `Run`, а три — ещё и с
//! пробелом в самом пути, потому что лежат в `C:\Program Files\...`, — то
//! есть в том самом месте по умолчанию, куда установится и этот продукт).
//!
//! Разбор командной строки по этому пробелу и завёл сюда трижды подряд:
//! сравнение по байтам не узнавало запись без кавычек и без кавычек и в
//! другом регистре; узнав их, `split_whitespace().next()` резало путь с
//! пробелом по первому же пробелу (`C:\Program` вместо всего пути) — то
//! есть ломался ровно путь по умолчанию для установки этого же продукта.
//! Причина, по которой один и тот же класс дефекта возвращался: `Run` без
//! кавычек — это НАСТОЯЩАЯ неоднозначность («где кончается путь и
//! начинаются аргументы» без обращения к файловой системе не решить —
//! ровно это и делает `CreateProcess`, пробуя каждый пробел слева
//! направо), и попытка её разобрать снова и снова находила новый пример,
//! на котором разбор ошибался. Поэтому `is_enabled` не разбирает командную
//! строку вовсе: она не спрашивает «что это за программа», а спрашивает
//! «наш ли это exe», а путь уже известен заранее — значит годится прямая
//! проверка префикса с границей слова (см. `matches_prefix_boundary`), а
//! не попытка угадать конец пути.
//!
//! Тот же ход применён на шаг ниже: `matches_prefix_boundary` сравнивает
//! НАПИСАНИЕ, а не файл, и поэтому слеп к разным написаниям одного и того
//! же пути — прямые слэши (`C:/Program Files/...`), сегменты `.`/`..`, 8.3
//! короткое имя (`PROGRA~1`). Хуже того: `env::current_exe()` возвращает
//! путь в ТОЙ ФОРМЕ, в которой был запущен процесс — включи человек
//! автозапуск из короткого пути (ярлык, консоль), `enable` запишет в `Run`
//! именно короткую форму, а следующий запуск из проводника резолвится в
//! длинную, и наша же запись перестаёт себя узнавать: ни инсталлятор, ни
//! правка руками для этого не нужны. Поэтому сравнение сначала спрашивает
//! файловую систему, как и `CreateProcess`: обе стороны прогоняются через
//! `fs::canonicalize` (см. `matches_exe_by_identity`), и совпадают, если
//! резолвятся в один и тот же файл. Устаревшая запись, указывающая на
//! удалённый файл, не резолвится вовсе — и это `false`, а не отказ:
//! `matches_prefix_boundary` остаётся ПОДСТРАХОВКОЙ ровно на этот случай
//! (несуществующий файл, значит сравнивать нечего файловой системой —
//! сравниваем написание, как раньше).
//!
//! Это закрывает разные НАПИСАНИЯ пути, называющего наш файл — не любую
//! команду, которая его в итоге ЗАПУСКАЕТ. `C:\Windows\explorer` (без
//! `.exe`, который дописал бы `CreateProcess`) или обёртка вида `cmd /c
//! start "" "...\proxypilot.exe"` этим механизмом не распознаются: чтобы
//! такая запись вообще оказалась в `Run` под именем `ProxyPilot`, кто-то
//! третий должен был написать её руками — `enable` никогда не пишет ни ту,
//! ни другую форму. Риск от этого не нулевой, но кратно ниже прежнего: там
//! ложноотрицательным становился путь, который писали МЫ САМИ.
//!
//! Кавычки, если они есть, снимаются однозначно (граница — следующая
//! кавычка), переменные окружения раскрываются (см. `expand_env`),
//! сравнение написаний — без учёта регистра. Отсутствие совпадения после
//! всего этого — не только «перенесли/переустановили exe», но могло быть и
//! «настоящая рабочая запись, которую мы просто не узнали» — второе хуже,
//! потому что тумблер показывает «выключено» при работающем автозапуске, и
//! `apply_autostart` (settings_page.rs), считая, что менять нечего, не
//! пишет ничего в ответ на снятую галочку: человек не может выключить
//! автозапуск через интерфейс.
//!
//! Обёртка над `HKEY` — `sysproxy::RegKey`, а не своя копия: единственное,
//! что было жёстко привязано к `Internet Settings`, — подключ в `open()`, и
//! он вынесен параметром именно ради этого второго потребителя. Корень же
//! (`HKEY_CURRENT_USER`) стал параметром позже, ради третьего потребителя —
//! `openvpn`, читающего `HKEY_LOCAL_MACHINE` (см. докблок `sysproxy::RegKey`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use windows::core::{w, PCWSTR};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
#[cfg(feature = "test-registry")]
use windows::Win32::System::Registry::REG_VALUE_TYPE;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

use crate::sysproxy::RegKey;
use crate::WinNetError;

const SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("ProxyPilot");

/// Путь в кавычках — так, как его ждёт `Run` от НАШЕЙ записи: без них путь с
/// пробелами Windows разберёт по первому пробелу. Требование только на
/// запись — при чтении кавычки не нужны вовсе (см. `points_at`).
fn quote(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Раскрывает переменные окружения (`%ProgramFiles%\...` → реальный путь) —
/// то же самое, что Windows делает сама, разворачивая `Run` перед запуском.
/// Вызывается безусловно, не только для значений типа `REG_EXPAND_SZ`:
/// `query_string` в `sysproxy::RegKey` уже отдаёт `REG_SZ` и `REG_EXPAND_SZ`
/// одной и той же строкой, не сообщая тип, а различать их здесь незачем —
/// раскрытие строки, в которой раскрывать нечего, не меняет её: Windows
/// трогает только настоящие токены `%ИМЯ%`, остальное копирует как есть.
/// Наши же собственные значения (пишет их только `quote` выше) никогда не
/// содержат `%`, так что для них это гарантированно no-op.
fn expand_env(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let src: Vec<u16> = raw.encode_utf16().chain(std::iter::once(0)).collect();
    let src_ptr = PCWSTR::from_raw(src.as_ptr());

    // SAFETY: `src` — валидный, живой на весь блок буфер с завершающим
    // нулём; вызов без `lpdst` — только чтобы узнать нужный размер, как и
    // предписано контрактом `ExpandEnvironmentStringsW`.
    let needed = unsafe { ExpandEnvironmentStringsW(src_ptr, None) };
    if needed == 0 {
        return raw.to_string();
    }

    let mut buf = vec![0u16; needed as usize];
    // SAFETY: тот же самый живой `src`/`src_ptr`; `buf` выделен ровно на
    // `needed` элементов — ту длину, что вернул первый вызов, и её же несёт
    // переданный срез.
    let written = unsafe { ExpandEnvironmentStringsW(src_ptr, Some(&mut buf)) };
    if written == 0 || written as usize > buf.len() {
        return raw.to_string();
    }
    // `written` включает завершающий нуль.
    let end = buf[..written as usize]
        .iter()
        .position(|&u| u == 0)
        .unwrap_or_else(|| (written as usize).saturating_sub(1));
    String::from_utf16_lossy(&buf[..end])
}

/// `candidate` — то, что мы посчитали путём программы целиком, без
/// аргументов. Сравнение без учёта регистра: путь может лежать под именем
/// пользователя не из ASCII (продукт русскоязычный), `to_lowercase()`, а не
/// `eq_ignore_ascii_case`, ловит и такие различия тоже. Строгое сравнение
/// написаний — специально не через какую-либо Unicode-эквивалентность
/// регистра сверх `to_lowercase()`: например, `ß` и `SS` остаются разными
/// строками, и это верно — конфлировать их значило бы путать буквально
/// разные пути.
fn matches_exe(candidate: &str, exe: &Path) -> bool {
    candidate.to_lowercase() == exe.display().to_string().to_lowercase()
}

/// Проверяет, что `raw` (без кавычек) НАЧИНАЕТСЯ с пути `exe`, и сразу после
/// совпавшего префикса — конец строки или пробел.
///
/// Это и есть «match, не parse»: `Run` без кавычек — командная строка, где
/// граница между путём и аргументами по-настоящему неоднозначна без
/// обращения к файловой системе (ровно это делает `CreateProcess`, пробуя
/// каждый пробел слева направо). Мы вместо этого не спрашиваем «что это за
/// программа» — мы уже знаем путь `exe` и спрашиваем только «начинается ли
/// команда с него». Граница по пробелу или концу строки обязательна: без
/// неё `proxypilot.exe.bak` сошёл бы за `proxypilot.exe`, потому что второй
/// путь — префикс первого.
///
/// Длина, по которой ищется граница, — байтовая длина ОРИГИНАЛЬНОГО `exe`,
/// не результата `to_lowercase()`: у последнего в редких не-буквенных
/// Unicode-случаях длина в байтах может отличаться, и резать `raw` по ней
/// значило бы резать не по границе символа. `raw.get(..len)` возвращает
/// `None`, если `raw` короче или граница попадает внутрь символа — оба
/// случая корректно дают «не совпало», без паники.
///
/// Это ПОДСТРАХОВКА, а не первый шаг: сравнивает написания, а значит слепа
/// к разным написаниям одного файла (прямые слэши, `.`/`..`-сегменты, 8.3
/// короткое имя) — их ловит `matches_exe_by_identity`, вызываемый раньше.
/// Нужна на случай, когда файла уже нет (`fs::canonicalize` не резолвится
/// ни для чего) — тогда сравнивать остаётся только написание.
fn matches_prefix_boundary(raw: &str, exe: &Path) -> bool {
    let exe_str = exe.display().to_string();
    let Some(candidate) = raw.get(..exe_str.len()) else {
        return false;
    };
    if candidate.to_lowercase() != exe_str.to_lowercase() {
        return false;
    }
    match raw[exe_str.len()..].chars().next() {
        None => true,
        Some(c) => c.is_whitespace(),
    }
}

/// Резолвит `candidate` файловой системой и сравнивает с уже резолвленным
/// `exe_canonical` — тем же приёмом, каким `CreateProcess` разрешает путь
/// перед запуском. Так совпадают разные НАПИСАНИЯ одного файла (прямые
/// слэши, `.`/`..`-сегменты, 8.3 короткое имя `PROGRA~1`), которые
/// `matches_prefix_boundary` как строки никогда не совпадут.
///
/// Оба пути обязаны быть уже канонизированы ДО сравнения — `PathBuf`,
/// полученный из `fs::canonicalize`, на Windows несёт префикс `\\?\`
/// (verbatim-путь); сравнивать такой с сырым, не канонизированным путём
/// нельзя: они не совпадут никогда, даже если это один и тот же файл.
///
/// `false`, если `candidate` не резолвится вовсе — несуществующий файл
/// (устаревшая запись) корректно не совпадает ни с чем, а не превращается
/// в ошибку; и `false`, если `candidate` резолвится, но в ДРУГОЙ,
/// существующий файл — коллизия по написанию с чем-то реальным тоже не
/// должна сойти за совпадение.
fn matches_exe_by_identity(candidate: &str, exe_canonical: Option<&Path>) -> bool {
    let Some(exe_canonical) = exe_canonical else {
        return false;
    };
    match fs::canonicalize(candidate) {
        Ok(candidate_canonical) => candidate_canonical == exe_canonical,
        Err(_) => false,
    }
}

/// Все точки, где неквотированная командная строка могла бы оборваться в
/// путь к файлу: сразу перед каждым пробельным разделителем, плюс строка
/// целиком. Тот же перебор, что делает сам `CreateProcess`, пытаясь
/// раскрыть неоднозначную командную строку в существующий файл.
fn whitespace_boundary_prefixes(raw: &str) -> impl Iterator<Item = &str> {
    raw.char_indices()
        .filter(|&(_, c)| c.is_whitespace())
        .map(move |(i, _)| &raw[..i])
        .chain(std::iter::once(raw))
}

/// Сравнивает сырое значение реестра (`""` — значения нет) с путём текущего
/// исполняемого файла. Не требует байтового совпадения с тем, что пишет
/// `enable`.
///
/// Пробелы по краям снимаются перед всем остальным: без этого значение вида
/// ` "C:\...\proxypilot.exe"` (пробел перед кавычкой) не опозналось бы как
/// квотированное вовсе — `strip_prefix('"')` требует, чтобы кавычка шла
/// первым байтом.
///
/// Кавычка в начале — случай однозначный: программа это всё до следующей
/// кавычки (аргументы после неё достаются программе, и разбор здесь ничем
/// не рискует, потому что кавычки сами объявляют границу). Без кавычки —
/// неоднозначный случай: пробуем каждую границу по пробелу как отдельного
/// кандидата на путь программы (см. `whitespace_boundary_prefixes`).
///
/// В обоих случаях кандидат сначала проверяется по идентичности файла
/// (`matches_exe_by_identity`), и только если файл не резолвится —
/// сравнением написаний (`matches_exe`/`matches_prefix_boundary`).
fn points_at(raw: &str, exe: &Path) -> bool {
    let raw = raw.trim();
    if raw.is_empty() {
        return false;
    }
    let exe_canonical: Option<PathBuf> = fs::canonicalize(exe).ok();

    if let Some(rest) = raw.strip_prefix('"') {
        let program = match rest.find('"') {
            Some(end) => &rest[..end],
            // Незакрытая кавычка — испорченное значение; берём что есть,
            // а не отказываемся разбирать вовсе.
            None => rest,
        };
        return matches_exe_by_identity(program, exe_canonical.as_deref())
            || matches_exe(program, exe);
    }

    for candidate in whitespace_boundary_prefixes(raw) {
        if matches_exe_by_identity(candidate, exe_canonical.as_deref()) {
            return true;
        }
    }
    matches_prefix_boundary(raw, exe)
}

/// Проверка «включён ли автозапуск» относительно произвольного подключа —
/// нужна тестам (см.
/// `tests::enable_disable_and_is_enabled_round_trip_against_a_private_scratch_key`),
/// которым нужен настоящий реестр, но не настоящий `Run`. Продакшн видит
/// только `is_enabled()` ниже, зашитую на `SUBKEY`.
fn is_enabled_at(subkey: PCWSTR, exe: &Path) -> Result<bool, WinNetError> {
    // `HKCU\...\Run` не гарантированно существует: на свежем профиле,
    // где автозапуск не включали ни для одной программы, этого подключа
    // попросту нет (найдено CI на `windows-latest` — не у каждого образа он
    // есть). Отсутствие ключа значит «нет ни одной записи автозапуска», а
    // значит и нашей — это `false`, такой же честный ответ, как и то, что
    // `query_string` уже отдаёт для отсутствующего ЗНАЧЕНИЯ внутри
    // существующего ключа. Любая другая ошибка (нет прав, битый куст)
    // по-прежнему пробрасывается наружу через `open_if_exists`.
    let Some(key) = RegKey::open_if_exists(HKEY_CURRENT_USER, subkey, KEY_READ)? else {
        return Ok(false);
    };
    let raw = key.query_string(VALUE_NAME)?;
    Ok(points_at(&expand_env(&raw), exe))
}

/// Включён ли автозапуск именно для этого исполняемого файла.
///
/// Наличия значения недостаточно: перенесённый или переустановленный exe
/// оставил бы в реестре запись, указывающую в никуда, а тумблер продолжал бы
/// показывать «включено», хотя автозапуск на деле не сработает.
pub fn is_enabled() -> Result<bool, WinNetError> {
    // `current_exe()` — единственное место, где эта функция вообще может
    // вернуть ошибку, не связанную с Windows API, поэтому явный `map_err`
    // вместо `#[from]` в WinNetError: `?` через `From` подхватывал бы любую
    // будущую `io::Error` в этом крейте под один и тот же, неверный для неё
    // текст «не удалось определить путь к своему исполняемому файлу».
    let exe = env::current_exe().map_err(WinNetError::CurrentExe)?;
    is_enabled_at(SUBKEY, &exe)
}

fn enable_at(subkey: PCWSTR, exe: &Path) -> Result<(), WinNetError> {
    // `open_or_create`, не `open`: на свежем профиле, где ключ `Run` ещё
    // никогда не создавался (см. докблок `is_enabled_at` выше), включение
    // автозапуска обязано само завести ключ — так же, как это делает
    // Windows при первой ручной записи в `Run`, — а не отказывать там, где
    // до сих пор просто никто не писал.
    let key = RegKey::open_or_create(HKEY_CURRENT_USER, subkey, KEY_WRITE)?;
    key.set_string(VALUE_NAME, &quote(exe))
}

/// Включает автозапуск: пишет в `Run` путь `exe` в кавычках.
///
/// `exe`, а не «взять `current_exe()` самим», намеренно — так вызывающая
/// сторона решает, какой путь класть в реестр, и это же значение можно
/// подставить в тесте вместо реального пути к тестовому бинарнику.
pub fn enable(exe: &Path) -> Result<(), WinNetError> {
    enable_at(SUBKEY, exe)
}

fn disable_at(subkey: PCWSTR) -> Result<(), WinNetError> {
    // Отсутствие ключа `Run` — то же самое «автозапуска нет», что и
    // отсутствие в нём значения `ProxyPilot`: docblock `disable` ниже
    // обещает идемпотентность для второго случая, и первый ничем не хуже —
    // выключать нечего в обоих. `open_if_exists`, как и в `is_enabled_at`.
    let Some(key) = RegKey::open_if_exists(HKEY_CURRENT_USER, subkey, KEY_WRITE)? else {
        return Ok(());
    };
    key.delete_value(VALUE_NAME)
}

/// Выключает автозапуск: удаляет значение `ProxyPilot`. Идемпотентно —
/// повторный вызов, как и вызов при уже выключенном автозапуске (в том
/// числе когда `HKCU\...\Run` целиком ещё не создан), не ошибка.
pub fn disable() -> Result<(), WinNetError> {
    disable_at(SUBKEY)
}

/// Сырое значение `ProxyPilot` вместе с его типом реестра (`REG_SZ.0` или
/// `REG_EXPAND_SZ.0`, как простое число — оно не значит ничего, кроме «верни
/// потом это же значение в `restore_raw_value_for_tests`»); `("", _)` —
/// значения нет, тип в этом случае не важен.
///
/// НЕ для продакшн-логики — `is_enabled`/`enable`/`disable` покрывают её
/// целиком без утечки формата наружу. Существует только для тестов ВНЕ
/// этого крейта, которым нужно снять и вернуть на место прежнее состояние
/// настоящего `Run` вокруг теста, который его настоящим образом трогает (см.
/// `WinAutostart`'s ignored-тест в `crates/app/src/main.rs`). Без доступа к
/// сырой строке такой тест не смог бы отличить «раньше было пусто» от
/// «раньше была чужая запись, указывающая куда-то ещё», и терял бы вторую
/// при восстановлении. Тип нужен по той же причине, только на шаг тоньше:
/// инсталляторы часто пишут `REG_EXPAND_SZ` (`%ProgramFiles%\...`), и без
/// сохранения типа восстановление откатило бы такую запись в `REG_SZ` —
/// `%VAR%` в ней перестал бы раскрываться при следующем реальном чтении
/// Windows, хотя сам текст остался бы прежним (fix round 4, Minor 4).
///
/// За флагом `test-registry`, а не просто без документации в примерах: пара
/// к этой функции ниже пишет в реестр ПРОИЗВОЛЬНУЮ командную строку без
/// проверок, которых требуют `enable`/`disable` — то есть строго больше
/// возможностей, чем есть у честного пути. `pub(crate)` здесь не подходит —
/// вызывающий, `main.rs`, в другом крейте; `#[doc(hidden)]` лишь прячет из
/// документации, не из собранного бинарника. Флаг же — благодаря
/// `resolver = "2"` в `win/Cargo.toml` — не унифицируется в сборку
/// продакшн-бинарника из `[dev-dependencies]` вызывающего крейта: там этой
/// функции физически нет в скомпилированном коде, а не только в примерах.
#[cfg(feature = "test-registry")]
pub fn raw_value_for_tests() -> Result<(String, u32), WinNetError> {
    raw_value_at(SUBKEY)
}

/// Возвращает `ProxyPilot` в состояние `previous` тем же `value_type`
/// (пустая строка — удаляет значение, тип в этом случае не важен). Пара к
/// [`raw_value_for_tests`], тоже только за `test-registry` и по той же
/// причине.
#[cfg(feature = "test-registry")]
pub fn restore_raw_value_for_tests(previous: &str, value_type: u32) -> Result<(), WinNetError> {
    restore_raw_value_at(SUBKEY, previous, value_type)
}

/// `u32`, а не `windows::...::REG_VALUE_TYPE`, в публичной сигнатуре двух
/// функций выше: вызывающий (`main.rs`, другой крейт) не должен заводить
/// зависимость от `windows` и его фич ради типа, который он всё равно
/// только проносит туда и обратно, не заглядывая внутрь.
///
/// За тем же `test-registry`, что и вызывающие их `pub fn` выше: без
/// фичи эти два — единственные вызывающие `RegKey::query_string_with_type`/
/// `set_string_as` в сборке продакшн-бинарника, и без гейта здесь у тех
/// оказалось бы ноль вызывающих (`dead_code`), несмотря на реальных
/// вызывающих под фичей.
#[cfg(feature = "test-registry")]
fn raw_value_at(subkey: PCWSTR) -> Result<(String, u32), WinNetError> {
    let key = RegKey::open(HKEY_CURRENT_USER, subkey, KEY_READ)?;
    Ok(match key.query_string_with_type(VALUE_NAME)? {
        Some((ty, value)) => (value, ty.0),
        None => (String::new(), 0),
    })
}

#[cfg(feature = "test-registry")]
fn restore_raw_value_at(
    subkey: PCWSTR,
    previous: &str,
    value_type: u32,
) -> Result<(), WinNetError> {
    let key = RegKey::open(HKEY_CURRENT_USER, subkey, KEY_WRITE)?;
    if previous.is_empty() {
        key.delete_value(VALUE_NAME)
    } else {
        key.set_string_as(VALUE_NAME, previous, REG_VALUE_TYPE(value_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, HKEY, HKEY_CURRENT_USER, REG_CREATED_NEW_KEY,
        REG_CREATE_KEY_DISPOSITION, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE,
    };

    #[test]
    fn quote_wraps_the_path_in_double_quotes() {
        let q = quote(Path::new(r"C:\Program Files\ProxyPilot\proxypilot.exe"));
        assert_eq!(q, "\"C:\\Program Files\\ProxyPilot\\proxypilot.exe\"");
    }

    #[test]
    fn quoted_path_with_spaces_round_trips_through_points_at() {
        // Ровно тот случай, ради которого нужны кавычки на запись: путь с
        // пробелом, записанный нами же.
        let exe = Path::new(r"C:\Program Files\ProxyPilot\proxypilot.exe");
        let written = quote(exe);
        assert!(points_at(&written, exe));
    }

    #[test]
    fn points_at_is_false_when_registry_is_empty() {
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        assert!(!points_at("", exe));
    }

    #[test]
    fn points_at_is_false_for_a_different_executable() {
        // Перенесли или переустановили exe: значение в реестре по-прежнему
        // указывает на старое место. Тумблер обязан показать «выключено»,
        // а не соврать, что автозапуск работает.
        let old = quote(Path::new(r"C:\ProxyPilot\old\proxypilot.exe"));
        let new_exe = Path::new(r"C:\ProxyPilot\new\proxypilot.exe");
        assert!(!points_at(&old, new_exe));
    }

    #[test]
    fn points_at_is_false_for_a_different_executable_even_in_a_different_case() {
        // Регистронезависимость не должна давать ложных срабатываний на
        // действительно разных путях — только на разном написании одного.
        let old = quote(Path::new(r"C:\PROXYPILOT\OLD\proxypilot.exe"));
        let new_exe = Path::new(r"c:\proxypilot\new\proxypilot.exe");
        assert!(!points_at(&old, new_exe));
    }

    #[test]
    fn points_at_is_false_when_the_value_is_only_a_path_prefix() {
        // "C:\ProxyPilot\proxy" не равно "C:\ProxyPilot\proxypilot.exe" —
        // ни разбор командной строки, ни регистронезависимость не должны
        // сделать префикс похожим на полное совпадение.
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        assert!(!points_at(r"C:\ProxyPilot\proxy", exe));
    }

    #[test]
    fn points_at_matches_an_unquoted_value_pointing_at_the_same_exe() {
        // Значение без кавычек — обычный вид записи, которую оставляет
        // инсталлятор или человек руками, а не повреждение нашей. Если бы
        // это читалось как «выключено», тумблер лгал бы при живом, рабочем
        // автозапуске, а `apply_autostart` не писал бы в реестр ничего в
        // ответ на снятую человеком галочку — критическая находка №1.
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let unquoted = exe.display().to_string();
        assert!(points_at(&unquoted, exe));
    }

    #[test]
    fn points_at_ignores_case_differences_in_the_same_path() {
        // Windows не различает регистр в путях, и реестр его не
        // нормализует — инсталлятор мог записать диск строчной буквой.
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let differently_cased = "\"c:\\PROXYPILOT\\ProxyPilot.EXE\"".to_string();
        assert!(points_at(&differently_cased, exe));
    }

    #[test]
    fn points_at_matches_a_quoted_value_with_trailing_arguments() {
        // "C:\...\proxypilot.exe" --min — обычный вид живой записи с
        // аргументом. Находка fix round 2: раньше сравнивалось всё сырое
        // значение целиком, включая " --min", и совпадения не было никогда.
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let raw = format!("{} --min", quote(exe));
        assert!(points_at(&raw, exe));
    }

    #[test]
    fn points_at_matches_an_unquoted_value_with_trailing_arguments() {
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let raw = format!("{} -autostart", exe.display());
        assert!(points_at(&raw, exe));
    }

    #[test]
    fn points_at_matches_an_unquoted_spaced_path_with_no_arguments() {
        // Находка A, в третий раз: путь по умолчанию для установки этого
        // же продукта (`C:\Program Files\...`), без кавычек, без
        // аргументов. `split_whitespace().next()` резал его до `C:\Program`
        // и никогда не находил совпадения — именно этот случай был назван
        // в докблоке модуля как доказательство ещё до того, как для него
        // появился тест.
        let exe = Path::new(r"C:\Program Files\ProxyPilot\proxypilot.exe");
        let raw = exe.display().to_string();
        assert!(points_at(&raw, exe));
    }

    #[test]
    fn points_at_matches_an_unquoted_spaced_path_with_trailing_arguments() {
        let exe = Path::new(r"C:\Program Files\ProxyPilot\proxypilot.exe");
        let raw = format!("{} -autostart", exe.display());
        assert!(points_at(&raw, exe));
    }

    #[test]
    fn points_at_is_false_when_an_unquoted_value_merely_shares_a_prefix() {
        // Обратная сторона проверки границы: "...\proxypilot.exe.bak" не
        // равно "...\proxypilot.exe" — граница после совпавшего префикса
        // обязана быть концом строки или пробелом, а не любым следующим
        // символом, иначе `matches_prefix_boundary` сама стала бы новым
        // источником ложных срабатываний.
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        assert!(!points_at(r"C:\ProxyPilot\proxypilot.exe.bak", exe));
    }

    // Три формы записи из настоящего Run этой машины на момент ревью — без
    // кавычек, с пробелом в пути, названы явно в докблоке модуля как
    // причина, по которой находка A потребовала третьего исправления.
    // Значения здесь не совпадают с содержимым Run (это было бы совпадением
    // с реальным чужим ПО), а лишь повторяют их ФОРМУ для нашего же exe.

    #[test]
    fn points_at_matches_the_real_run_shape_of_openvpn_gui() {
        // Без кавычек, пробел в "Program Files", без аргументов.
        let exe = Path::new(r"C:\Program Files\OpenVPN\bin\openvpn-gui.exe");
        assert!(points_at(
            r"C:\Program Files\OpenVPN\bin\openvpn-gui.exe",
            exe
        ));
    }

    #[test]
    fn points_at_matches_the_real_run_shape_of_docker_desktop() {
        // Без кавычек, пробел не только в "Program Files", но и в самом
        // имени файла ("Docker Desktop.exe") — то есть внутри последнего
        // «слова» строки, а не только в середине пути.
        let exe = Path::new(r"C:\Program Files\Docker\Docker\Docker Desktop.exe");
        assert!(points_at(
            r"C:\Program Files\Docker\Docker\Docker Desktop.exe",
            exe
        ));
    }

    #[test]
    fn points_at_matches_the_real_run_shape_of_download_master() {
        // Без кавычек, пробел в "Program Files (x86)", плюс аргумент
        // "-autorun".
        let exe = Path::new(r"C:\Program Files (x86)\Download Master\dmaster.exe");
        assert!(points_at(
            r"C:\Program Files (x86)\Download Master\dmaster.exe -autorun",
            exe
        ));
    }

    // Находка round 4: matches_prefix_boundary сравнивает НАПИСАНИЕ, а не
    // файл, и слепа к разным написаниям одного и того же файла. Ниже —
    // три таких написания, все требуют РЕАЛЬНОГО файла на диске (иначе
    // `fs::canonicalize` не резолвится, и сравнивать через файловую систему
    // нечего) — используют собственный тестовый бинарник этого же прогона.

    #[test]
    fn points_at_matches_forward_slashes_via_filesystem_identity() {
        let exe = env::current_exe().expect("текущий бинарник обязан существовать");
        let raw = exe.display().to_string().replace('\\', "/");
        assert_ne!(
            raw,
            exe.display().to_string(),
            "тест ничего не проверяет без /: {raw}"
        );
        assert!(points_at(&raw, &exe));
    }

    #[test]
    fn points_at_matches_a_path_with_dot_dot_segments_via_filesystem_identity() {
        let exe = env::current_exe().expect("текущий бинарник обязан существовать");
        let parent = exe.parent().expect("exe обязан лежать в какой-то папке");
        let parent_name = parent
            .file_name()
            .expect("родительская папка обязана иметь имя");
        let file_name = exe.file_name().expect("exe обязан иметь имя файла");
        let raw = parent
            .join("..")
            .join(parent_name)
            .join(file_name)
            .display()
            .to_string();
        assert_ne!(
            raw,
            exe.display().to_string(),
            "тест ничего не проверяет без ..: {raw}"
        );
        assert!(points_at(&raw, &exe));
    }

    #[test]
    fn points_at_is_false_when_the_prefix_collision_file_genuinely_exists() {
        // Находка round 4: сравнение теперь опирается на файловую систему,
        // а не только на строки — обязано остаться `false`, даже когда
        // файл с именем-коллизией РЕАЛЬНО существует (значит проходит
        // `fs::canonicalize`), но резолвится в другой, не наш, файл.
        //
        // Уборка — через `Drop`, как и везде в этом файле (fix round 5):
        // голый `let _ = fs::remove_file(...)` на пути только успеха
        // оставил бы файл рядом с тестовым бинарником при панике где-то
        // выше по тесту.
        struct RemoveOnDrop(PathBuf);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }

        let exe = env::current_exe().expect("текущий бинарник обязан существовать");
        let collision = exe.with_extension("exe.bak2");
        fs::copy(&exe, &collision).expect("тестовый файл-коллизия обязан создаваться");
        let _cleanup = RemoveOnDrop(collision.clone());

        // Без этой проверки тест остался бы зелёным, даже если бы ветка
        // identity-сравнения для `collision` никогда не срабатывала (файл
        // не резолвился бы, и `false` получался бы просто по умолчанию, а
        // не потому, что коллизия распознана и отвергнута).
        assert!(
            fs::canonicalize(&collision).is_ok(),
            "файл-коллизия обязан существовать и резолвиться файловой системой"
        );

        let raw = collision.display().to_string();
        assert!(
            !points_at(&raw, &exe),
            "raw = {raw} не должно совпасть с {}",
            exe.display()
        );
    }

    #[test]
    fn points_at_is_false_for_a_prefix_collision_with_a_trailing_digit() {
        let exe = Path::new(r"C:\ProxyPilot\proxy.exe");
        assert!(!points_at(r"C:\ProxyPilot\proxy.exe2", exe));
    }

    #[test]
    fn points_at_is_false_for_a_prefix_collision_with_a_trailing_path_segment() {
        let exe = Path::new(r"C:\ProxyPilot\pp.exe");
        assert!(!points_at(r"C:\ProxyPilot\pp.exe_old\", exe));
    }

    #[test]
    fn points_at_trims_leading_whitespace_before_looking_for_a_quote() {
        // Находка round 4, Minor 1: `raw.strip_prefix('"')` требует, чтобы
        // кавычка шла первым байтом — без предварительного `trim()` значение
        // с пробелом перед кавычкой опознавалось бы как НЕквотированное.
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let raw = format!(" {}", quote(exe));
        assert!(points_at(&raw, exe));
    }

    #[test]
    fn points_at_trims_trailing_whitespace_on_an_unquoted_value() {
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let raw = format!("{}  ", exe.display());
        assert!(points_at(&raw, exe));
    }

    #[test]
    fn points_at_trims_trailing_whitespace_on_a_quoted_value() {
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let raw = format!("{}  ", quote(exe));
        assert!(points_at(&raw, exe));
    }

    #[test]
    fn points_at_treats_a_tab_as_a_valid_separator() {
        // `char::is_whitespace()` считает `\t` пробельным символом наравне
        // с пробелом — граница в `matches_prefix_boundary` уже это ловит,
        // тест лишь закрепляет.
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let raw = format!("{}\t-min", exe.display());
        assert!(points_at(&raw, exe));
    }

    #[test]
    fn points_at_is_false_for_a_whitespace_only_value() {
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        assert!(!points_at("   ", exe));
    }

    #[test]
    fn points_at_is_false_for_an_arguments_only_value() {
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        assert!(!points_at("--min", exe));
    }

    #[test]
    fn points_at_is_false_for_an_unclosed_quote_with_trailing_content() {
        // Незакрытая кавычка — испорченное значение: всё после открывающей
        // кавычки, включая то, что выглядит как аргумент, становится ОДНОЙ
        // строкой "программы" целиком, включая пробел перед "-min", и не
        // совпадает с чистым путём. Это совпадает с тем, как повело бы себя
        // само Windows: `CreateProcess` тоже не нашёл бы такой файл.
        let exe = Path::new(r"C:\ProxyPilot\proxypilot.exe");
        let raw = format!("\"{} -min", exe.display());
        assert!(!points_at(&raw, exe));
    }

    #[test]
    fn points_at_matches_a_cyrillic_path_regardless_of_case() {
        // Профиль пользователя с кириллицей в пути — обычное дело для
        // приложения, которое ставится не только на машины с ASCII-именами.
        // `to_lowercase()`, а не `eq_ignore_ascii_case`, обязан свернуть
        // регистр и здесь — докблок `matches_exe` про это заявляет давно, а
        // до этого раунда никто не проверял.
        let exe = Path::new(r"C:\Users\Пользователь\ProxyPilot\proxypilot.exe");
        let differently_cased = r"c:\users\ПОЛЬЗОВАТЕЛЬ\proxypilot\proxypilot.exe";
        assert!(points_at(differently_cased, exe));

        let quoted = format!("\"{differently_cased}\"");
        assert!(points_at(&quoted, exe));
    }

    #[test]
    fn points_at_does_not_conflate_sharp_s_with_double_s() {
        // `ß`, поднятый в верхний регистр, часто даёт "SS", но обратное
        // неверно: `to_lowercase()` не превращает "SS" в "ß". Сравнение
        // обязано остаться строгим — иначе разные по буквам пути стали бы
        // неотличимы.
        let exe = Path::new(r"C:\Users\Weiß\proxypilot.exe");
        let different_word = r"C:\Users\Weiss\proxypilot.exe";
        assert!(!points_at(different_word, exe));
    }

    #[test]
    fn expand_env_then_points_at_matches_an_unquoted_spaced_variable_expansion() {
        // Комбинация всех трёх находок сразу — переменная окружения,
        // раскрывающаяся в путь с пробелом, без кавычек, с аргументом —
        // ровно то, что `is_enabled_at` получает на входе, раскрывая
        // ПЕРЕД разбором.
        //
        // `%ProgramFiles%`, а не своя переменная через `std::env::set_var`:
        // fix round 5 — `set_var` мутирует окружение ПРОЦЕССА целиком, а
        // тесты этого же файла выполняются в общих потоках одного процесса
        // параллельно; `set_var`/`remove_var` здесь были бы гонкой данных
        // (это ровно то, из-за чего в редакции 2024 обе функции помечены
        // `unsafe` — компилируется только потому, что этот крейт на 2021).
        // `ProgramFiles` — стандартная, всегда существующая на Windows
        // переменная, и мы её только ЧИТАЕМ: гонки нет, а пробел в её
        // значении ("C:\Program Files") есть всегда, независимо от языка
        // системы — физическое имя папки Microsoft не локализует.
        let program_files =
            std::env::var("ProgramFiles").expect("ProgramFiles обязана быть в окружении Windows");
        let exe = PathBuf::from(&program_files)
            .join("ProxyPilot")
            .join("proxypilot.exe");
        let raw = expand_env(r"%ProgramFiles%\ProxyPilot\proxypilot.exe -min");

        assert!(!raw.contains('%'), "переменная не раскрылась: {raw}");
        assert!(points_at(&raw, &exe));
    }

    #[test]
    fn expand_env_is_a_no_op_for_strings_without_variables() {
        let s = r"C:\ProxyPilot\proxypilot.exe";
        assert_eq!(expand_env(s), s);
    }

    #[test]
    fn expand_env_resolves_a_variable_that_installers_actually_use() {
        // `SystemRoot` есть на любой Windows — это папка с самой Windows;
        // именно такими токенами (`%ProgramFiles%` и подобными) инсталляторы
        // и пишут `Run` как `REG_EXPAND_SZ`.
        let expanded = expand_env(r"%SystemRoot%\explorer.exe");
        assert!(
            !expanded.contains('%'),
            "переменная не раскрылась: {expanded}"
        );
        assert!(
            expanded.to_lowercase().ends_with(r"\explorer.exe"),
            "получили: {expanded}"
        );
    }

    #[test]
    fn expand_env_of_an_empty_string_is_empty() {
        assert_eq!(expand_env(""), "");
    }

    /// Страж подключа-песочницы: создаёт его при входе, удаляет при
    /// выходе — включая панику, чтобы упавший тест не оставил в реестре
    /// машины, где он запускался, висящий ключ `ProxyPilotAutostartSelfTest`.
    ///
    /// Имя подключа несёт PID процесса, а не постоянно: fix round 3,
    /// Minor 3 — фиксированное имя гонится в двух параллельных прогонах
    /// `cargo test` (обычная оболочка разработчика и rust-analyzer, оба
    /// одновременно) как гонка данных за один и тот же подключ, где `Drop`
    /// одного удаляет ключ, пока второй ещё в середине проверки.
    ///
    /// Буфер UTF-16 хранится в самом страже, а не как статическая `PCWSTR`
    /// (`w!`, как `SUBKEY`/`VALUE_NAME` выше) — потому что PID известен
    /// только в рантайме, а `w!` работает лишь на этапе компиляции.
    struct TestSubkeyGuard {
        subkey_utf16: Vec<u16>,
        /// `true`, только если подключ создан ЭТИМ вызовом `new`, а не уже
        /// существовал (fix round 3, Minor 2 — `RegCreateKeyW`, использованный
        /// раньше, тихо открывает уже существующий подключ, если тот
        /// почему-то есть, и `Drop` удалил бы чужой ключ, ничего об этом не
        /// зная; `RegCreateKeyExW` сообщает через `lpdwDisposition`, кто из
        /// двух случаев произошёл).
        created: bool,
    }

    impl TestSubkeyGuard {
        fn new() -> Self {
            let name = format!(
                "Software\\ProxyPilotAutostartSelfTest-{}",
                std::process::id()
            );
            let subkey_utf16: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let subkey_ptr = PCWSTR::from_raw(subkey_utf16.as_ptr());

            let mut hkey = HKEY::default();
            let mut disposition = REG_CREATE_KEY_DISPOSITION(0);
            // SAFETY: HKEY_CURRENT_USER — предопределённый корень, всегда
            // валиден; `subkey_ptr` указывает на `subkey_utf16` — живой на
            // весь этот блок буфер с завершающим нулём; класс и атрибуты
            // безопасности не нужны — `PCWSTR::null()` и `None`;
            // `phkresult`/`lpdwdisposition` — живые локальные переменные,
            // которые API заполняет только при успехе.
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
                    Some(&mut disposition),
                )
            }
            .ok()
            .expect("тестовый подключ обязан создаваться");
            // SAFETY: хендл только что получен от RegCreateKeyExW и больше
            // никому не нужен — RegKey::open ниже откроет тот же путь заново
            // своим собственным хендлом.
            let _ = unsafe { RegCloseKey(hkey) };
            Self {
                subkey_utf16,
                created: disposition == REG_CREATED_NEW_KEY,
            }
        }

        fn subkey(&self) -> PCWSTR {
            PCWSTR::from_raw(self.subkey_utf16.as_ptr())
        }
    }

    impl Drop for TestSubkeyGuard {
        fn drop(&mut self) {
            if !self.created {
                // Подключ с этим именем уже существовал ДО вызова `new` —
                // не наш, и удалять его не наше дело (Minor 2 выше).
                return;
            }
            // SAFETY: HKEY_CURRENT_USER — предопределённый корень;
            // `self.subkey_utf16` — поле этого же значения, живо до конца
            // `drop`. Удаляем подключ целиком, а не только значение —
            // песочница создана этим же вызовом `new` (иначе `created` было
            // бы `false`) и ни для чего другого не нужна. Ошибку игнорируем
            // сознательно: падать при уборке за собой хуже, чем оставить
            // пустой подключ.
            let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, self.subkey()) };
        }
    }

    #[test]
    fn enable_disable_and_is_enabled_round_trip_against_a_private_scratch_key() {
        // По контрасту с `enable_then_disable_round_trip_on_the_real_registry`
        // ниже: тот же самый код (`enable_at`/`disable_at`/`is_enabled_at`,
        // а публичные `enable`/`is_enabled`/`disable` — их тонкие обёртки на
        // `SUBKEY`), но проверенный на собственной песочнице, а не на
        // настоящем `Run` этой машины — поэтому гоняется в каждом обычном
        // прогоне, а не только руками. Без этого теста `enable`/`disable`/
        // `RegKey::delete_value` не выполнялись бы вовсе, пока кто-то не
        // запустит `--ignored` руками.
        let guard = TestSubkeyGuard::new();
        let subkey = guard.subkey();
        let exe = env::current_exe().expect("тестовый бинарник обязан резолвиться в путь");

        assert!(
            !is_enabled_at(subkey, &exe).expect("is_enabled_at обязан читаться"),
            "песочница только что создана — значения быть не должно"
        );

        enable_at(subkey, &exe).expect("enable_at обязан пройти");
        assert!(
            is_enabled_at(subkey, &exe).expect("is_enabled_at обязан читаться"),
            "после enable_at тумблер обязан показывать «включено»"
        );

        disable_at(subkey).expect("disable_at обязан пройти");
        assert!(
            !is_enabled_at(subkey, &exe).expect("is_enabled_at обязан читаться"),
            "после disable_at тумблер обязан показывать «выключено»"
        );
    }

    /// Как `TestSubkeyGuard`, но НЕ создаёт подключ при входе — нужен
    /// тестам missing-key пути (найдено CI: `HKCU\...\Run` не гарантированно
    /// существует), которым требуется имя заведомо ОТСУТСТВУЮЩЕГО подключа,
    /// а не готовая песочница. `Drop` всё равно пытается удалить его: если
    /// тестируемый код сам создал подключ (`enable_at`), убрать его нужно
    /// так же, как обычную песочницу; если не создал (тест только читал или
    /// удалял отсутствующий ключ) — `RegDeleteKeyW` вернёт
    /// `ERROR_FILE_NOT_FOUND`, что и так молча игнорируется, как и в
    /// `TestSubkeyGuard` выше.
    ///
    /// PID в имени по той же причине, что и у `TestSubkeyGuard` (fix round
    /// 3, Minor 3) — параллельные прогоны `cargo test` не должны делить
    /// один и тот же подключ; `label` вдобавок различает подключи НЕСКОЛЬКИХ
    /// тестов внутри одного и того же процесса теста.
    struct AbsentSubkeyGuard {
        subkey_utf16: Vec<u16>,
    }

    impl AbsentSubkeyGuard {
        fn new(label: &str) -> Self {
            let name = format!(
                "Software\\ProxyPilotAutostartSelfTest-{label}-{}",
                std::process::id()
            );
            let subkey_utf16: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            Self { subkey_utf16 }
        }

        fn subkey(&self) -> PCWSTR {
            PCWSTR::from_raw(self.subkey_utf16.as_ptr())
        }
    }

    impl Drop for AbsentSubkeyGuard {
        fn drop(&mut self) {
            // SAFETY: HKEY_CURRENT_USER — предопределённый корень;
            // `self.subkey_utf16` — поле этого же значения, живо до конца
            // `drop`. Ошибку игнорируем сознательно — как и в `Drop` для
            // `TestSubkeyGuard` выше, и по той же причине: ключа могло не
            // быть создано вовсе, и это ожидаемо, не отказ уборки.
            let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, self.subkey()) };
        }
    }

    #[test]
    fn is_enabled_at_is_ok_false_when_the_registry_key_itself_is_missing() {
        // Находка CI на windows-latest: `HKCU\...\Run` не гарантированно
        // существует на свежем профиле — `RegKey::open` возвращала бы
        // ошибку открытия, и `is_enabled_at` пробрасывала бы её наружу
        // (панику видел `WinAutostart::is_enabled` в `crates/app/src/main.rs`)
        // там, где честный ответ — «нет ни одной записи автозапуска, значит
        // и нашей нет» — то же самое `Ok(false)`, что уже даёт отсутствие
        // ЗНАЧЕНИЯ внутри существующего ключа.
        let guard = AbsentSubkeyGuard::new("is-enabled-missing");
        let exe = env::current_exe().expect("тестовый бинарник обязан резолвиться в путь");
        assert!(!is_enabled_at(guard.subkey(), &exe)
            .expect("отсутствие ключа обязано быть Ok(false), не ошибкой"));
    }

    #[test]
    fn enable_at_creates_the_missing_registry_key_and_writes_the_value() {
        // Самая заметная из трёх находок этого раунда: без `open_or_create`
        // включить автозапуск на свежем профиле было нельзя вообще никак —
        // `enable_at` валилась бы там, где сама Windows просто заводит
        // `Run` при первой записи в него.
        let guard = AbsentSubkeyGuard::new("enable-creates");
        let exe = env::current_exe().expect("тестовый бинарник обязан резолвиться в путь");

        enable_at(guard.subkey(), &exe)
            .expect("enable_at обязан создать отсутствующий подключ и записать значение");
        assert!(
            is_enabled_at(guard.subkey(), &exe).expect("is_enabled_at обязан читаться"),
            "после enable_at на свежесозданном подключе тумблер обязан показывать «включено»"
        );
    }

    #[test]
    fn disable_at_is_ok_when_the_registry_key_itself_is_missing() {
        // Docblock `disable` обещает идемпотентность и для случая «уже
        // выключено» — отсутствие подключа `Run` целиком является ровно
        // этим случаем, а не поводом для ошибки.
        let guard = AbsentSubkeyGuard::new("disable-missing");
        assert!(
            disable_at(guard.subkey()).is_ok(),
            "отсутствие ключа обязано быть Ok(()), не ошибкой"
        );
    }

    #[test]
    fn is_enabled_at_surfaces_a_non_missing_key_error_instead_of_treating_it_as_false() {
        // Обратная сторона трёх тестов выше: различать «ключа нет»
        // (`ERROR_FILE_NOT_FOUND`, честное «выключено») обязаны именно от
        // этого конкретного кода ошибки, а не от любой ошибки открытия —
        // иначе, скажем, отказ в доступе или битый куст стали бы
        // неотличимы от «автозапуска нет»: тихая ложь вместо громкой
        // ошибки, ровно то, чего это исправление обязано избежать.
        //
        // Имя подключа длиннее 255 символов (предел одного компонента пути
        // реестра, `MAX_KEY_LENGTH`) Windows отвергает при разборе имени —
        // заведомо не тем же кодом, что и «подключа с таким именем нет».
        let subkey_name = format!("Software\\{}", "x".repeat(300));
        let subkey_utf16: Vec<u16> = subkey_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let subkey = PCWSTR::from_raw(subkey_utf16.as_ptr());
        let exe = env::current_exe().expect("тестовый бинарник обязан резолвиться в путь");

        let err = is_enabled_at(subkey, &exe)
            .expect_err("слишком длинное имя подключа обязано быть ошибкой, а не Ok(false)");
        match err {
            WinNetError::Windows(e) => assert_ne!(
                e.code(),
                windows::core::HRESULT::from_win32(
                    windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.0
                ),
                "тест обязан бить не в отсутствие ключа, а в другую ошибку: {e}"
            ),
            other => panic!("ожидалась WinNetError::Windows, получили: {other}"),
        }
    }

    #[test]
    fn restore_raw_value_preserves_the_original_reg_expand_sz_type() {
        // Находка round 4, Minor 4: раньше восстановление писало через
        // `set_string` (всегда `REG_SZ`) безусловно — `REG_EXPAND_SZ`
        // (обычный тип для инсталляторских записей вида `%ProgramFiles%\...`)
        // откатывался бы в `REG_SZ`, и `%VAR%` в восстановленном значении
        // переставал бы раскрываться при следующем реальном чтении Windows.
        let guard = TestSubkeyGuard::new();
        let subkey = guard.subkey();

        let key = RegKey::open(HKEY_CURRENT_USER, subkey, KEY_WRITE)
            .expect("подключ обязан открываться на запись");
        key.set_string_as(VALUE_NAME, r"%SystemRoot%\explorer.exe", REG_EXPAND_SZ)
            .expect("исходное REG_EXPAND_SZ-значение обязано писаться");

        let (value, value_type) = raw_value_at(subkey).expect("чтение обязано пройти");
        restore_raw_value_at(subkey, &value, value_type).expect("восстановление обязано пройти");

        let key = RegKey::open(HKEY_CURRENT_USER, subkey, KEY_READ)
            .expect("подключ обязан открываться на чтение");
        let (restored_type, restored_value) = key
            .query_string_with_type(VALUE_NAME)
            .expect("чтение обязано пройти")
            .expect("значение обязано остаться на месте");
        assert_eq!(
            restored_type, REG_EXPAND_SZ,
            "тип обязан остаться REG_EXPAND_SZ, а не понизиться до REG_SZ"
        );
        assert_eq!(restored_value, r"%SystemRoot%\explorer.exe");
    }

    #[test]
    #[ignore = "трогает настоящий Run этой машины: гонять только руками"]
    fn enable_then_disable_round_trip_on_the_real_registry() {
        // Живой реестр, но по возможности безопасно: значение принадлежит
        // только нам (`ProxyPilot`), а прежнее содержимое ключа сохраняется
        // и восстанавливается. Не в обычном прогоне `cargo test` — как и
        // `events::watch_a_real_network_change` — по той же причине, что и
        // finding №3 ревью: обычный прогон может быть прерван (Ctrl+C,
        // паника где-то ещё в процессе тестов), и тогда живой автозапуск,
        // указывающий на тестовый бинарник, остался бы стоять в системе
        // человека, который эту ветку не писал и не просил. Покрытие того
        // же кода на КАЖДОМ прогоне даёт
        // `enable_disable_and_is_enabled_round_trip_against_a_private_scratch_key`
        // выше — против собственной песочницы, не настоящего `Run`.
        struct RestorePrevious(String, u32);
        impl Drop for RestorePrevious {
            fn drop(&mut self) {
                // Не паникуем даже здесь: `Drop` этого стража может
                // отрабатывать во время уже идущей паники (если сама
                // проверка ниже упала), а падение внутри `Drop` во время
                // паники — это abort процесса, а не просто ещё одна ошибка.
                if let Err(e) = restore_raw_value_for_tests(&self.0, self.1) {
                    eprintln!("не удалось восстановить прежнее значение автозапуска: {e}");
                }
            }
        }

        let (previous, previous_type) =
            raw_value_for_tests().expect("Run обязан читаться перед тестом");
        let _restore = RestorePrevious(previous, previous_type);

        let exe = env::current_exe().expect("тестовый бинарник обязан резолвиться в путь");

        enable(&exe).expect("enable обязан пройти без прав администратора");
        assert!(
            is_enabled().expect("is_enabled обязан читаться"),
            "после enable тумблер обязан показывать «включено»"
        );

        disable().expect("disable обязан пройти");
        assert!(
            !is_enabled().expect("is_enabled обязан читаться"),
            "после disable тумблер обязан показывать «выключено»"
        );
    }
}
