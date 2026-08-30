//! Системные настройки прокси (WinINET).
//!
//! Живут в HKCU, поэтому прав администратора не нужно и приложение
//! управляет ими само — в отличие от macOS-версии, где это делал человек.
//!
//! Плата за это — обязанность прибраться. Если процесс упадёт, в реестре
//! останется указатель на мёртвый слушатель, и пользователь окажется без
//! сети вообще: отказ хуже того, который мы лечим. Поэтому прежнее значение
//! сохраняется в конфиг ДО записи сюда и восстанавливается при старте.
//!
//! Что этим НЕ покрывается: WinHTTP (`netsh winhttp` — контекст служб, нужен
//! администратор), Firefox (свои настройки мимо WinINET), и приложения,
//! читающие `HTTP_PROXY` из окружения. Это не недоделка, а граница: расширить
//! её без UAC нельзя, поэтому UI обязан сказать об этом честно.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::Networking::WinInet::{
    InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_SAM_FLAGS, REG_SZ,
    REG_VALUE_TYPE,
};

use crate::WinNetError;

const SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");

const PROXY_ENABLE: PCWSTR = w!("ProxyEnable");
const PROXY_SERVER: PCWSTR = w!("ProxyServer");
const PROXY_OVERRIDE: PCWSTR = w!("ProxyOverride");

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SysProxy {
    pub enabled: bool,
    /// `127.0.0.1:3129` либо пусто
    pub server: String,
    /// список исключений в формате WinINET (через `;`)
    pub bypass: String,
}

/// Наш список исключений → формат WinINET.
///
/// Отличия от нашего: разделитель `;`, суффикс пишется как `*.local`,
/// и есть особый токен `<local>` — адреса без точки в имени.
pub fn to_bypass_string(no_proxy: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for raw in no_proxy.split(',') {
        let e = raw.trim();
        if e.is_empty() {
            continue;
        }
        if let Some(sfx) = e.strip_prefix('.') {
            // Голая точка суффикса не задаёт: «*.» для WinINET ничего не
            // значит, а мусор в списке исключений потом отлаживают часами.
            if sfx.is_empty() {
                continue;
            }
            parts.push(format!("*.{sfx}"));
        } else {
            parts.push(e.to_string());
        }
    }
    // `<local>` добавляем сами — но только если его ещё нет. На вход может
    // прийти уже готовый список WinINET (например, сохранённое значение
    // пользователя при восстановлении), и «…;<local>;<local>» — не то, что
    // мы имеем право записать в реестр.
    if !parts.iter().any(|p| p == "<local>") {
        parts.push("<local>".to_string());
    }
    parts.join(";")
}

/// Открытый ключ реестра, закрывающий себя сам.
///
/// Обёртка нужна не для красоты: `read`/`apply` выходят по `?` из середины,
/// и ручной `RegCloseKey` в конце функции пропустил бы все ошибочные пути.
/// В приложении из трея, живущем неделями, это была бы настоящая течь
/// хендлов. `Drop` закрывает ключ на любом выходе, включая панику.
///
/// `pub(crate)`, а не приватный: тот же тип и приёмы нужны `autostart`
/// (другой подключ HKCU, `Run` вместо `Internet Settings`) и `openvpn`
/// (другой корень целиком — `HKEY_LOCAL_MACHINE`, только чтение, ключ
/// `OpenVPN`, который сама Windows не создаёт и не гарантирует) — заводить
/// вторую байт-в-байт такую же обёртку означало бы копию, которая рано или
/// поздно разойдётся с этой. `open()` поэтому принимает и подключ, и
/// корень параметрами, а не хардкодит ни `SUBKEY` этого модуля, ни
/// `HKEY_CURRENT_USER` — то, что раньше было единственным жёстко
/// привязанным к `Internet Settings`/HKCU, стало параметром ради второго,
/// а теперь и третьего потребителя.
pub(crate) struct RegKey(HKEY);

impl RegKey {
    /// Открывает подключ `root` (`HKEY_CURRENT_USER` или
    /// `HKEY_LOCAL_MACHINE`) с запрошенными правами. Права просим ровно те,
    /// что нужны: `apply` не должен уметь писать больше, чем пишет, `read`
    /// и весь `openvpn` — не должны требовать прав записи вообще (чтение
    /// `HKEY_LOCAL_MACHINE` прав администратора не требует, в отличие от
    /// записи туда).
    pub(crate) fn open(
        root: HKEY,
        subkey: PCWSTR,
        access: REG_SAM_FLAGS,
    ) -> Result<Self, WinNetError> {
        let mut hkey = HKEY::default();
        // SAFETY: `root` — один из предопределённых корней реестра
        // (`HKEY_CURRENT_USER` или `HKEY_LOCAL_MACHINE`), оба всегда валидны
        // и не нуждаются в закрытии сами по себе; `subkey` обязан быть
        // статической строкой с завершающим нулём (как `SUBKEY` в этом
        // модуле, в `autostart` и в `openvpn`, все заданы через `w!`) — это
        // контракт параметра, а не то, что проверяется в рантайме;
        // `phkresult` указывает на живую локальную переменную, которую API
        // заполняет только при успехе. Полученный хендл сразу переходит под
        // управление RegKey, чей Drop его закрывает.
        unsafe { RegOpenKeyExW(root, subkey, 0, access, &mut hkey) }.ok()?;
        Ok(Self(hkey))
    }

    /// Сырое значение: `None` — значения нет.
    ///
    /// Классический двойной вызов `RegQueryValueExW`: сначала за размером
    /// (без буфера), потом за данными. Между вызовами значение теоретически
    /// может вырасти — тогда второй вызов вернёт `ERROR_MORE_DATA`, и мы
    /// честно отдадим ошибку, а не обрезанные данные.
    fn query_raw(&self, name: PCWSTR) -> Result<Option<(REG_VALUE_TYPE, Vec<u8>)>, WinNetError> {
        let mut ty = REG_VALUE_TYPE(0);
        let mut needed: u32 = 0;

        // SAFETY: self.0 — открытый нами и ещё не закрытый ключ (закроется
        // только в Drop); name — статическая строка с нулём; буфер не
        // передаём, поэтому API лишь заполняет `ty` и `needed` — оба
        // указывают на живые локальные переменные.
        let rc =
            unsafe { RegQueryValueExW(self.0, name, None, Some(&mut ty), None, Some(&mut needed)) };
        if rc == ERROR_FILE_NOT_FOUND {
            // Машина, где прокси никогда не настраивали, просто не имеет
            // этого значения. Это не отказ, а «пусто».
            return Ok(None);
        }
        rc.ok()?;

        if needed == 0 {
            return Ok(Some((ty, Vec::new())));
        }

        let mut buf = vec![0u8; needed as usize];
        let mut cap = needed;
        // SAFETY: ключ и имя — как выше; `buf` выделен на `needed` байт, и в
        // `cap` лежит ровно эта длина, так что API не выйдет за границы
        // буфера; указатель получен из живого, не перемещаемого до конца
        // вызова `Vec`.
        let rc = unsafe {
            RegQueryValueExW(
                self.0,
                name,
                None,
                Some(&mut ty),
                Some(buf.as_mut_ptr()),
                Some(&mut cap),
            )
        };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        rc.ok()?;

        // API мог записать меньше, чем обещал первым вызовом.
        buf.truncate(cap as usize);
        Ok(Some((ty, buf)))
    }

    /// Строковое значение; отсутствующее — пустая строка.
    pub(crate) fn query_string(&self, name: PCWSTR) -> Result<String, WinNetError> {
        let Some((ty, bytes)) = self.query_raw(name)? else {
            return Ok(String::new());
        };
        if ty != REG_SZ && ty != REG_EXPAND_SZ {
            // Не отказываемся стартовать из-за чужого мусора в чужом ключе,
            // но и не делаем вид, что всё в порядке: разобрать это как строку
            // мы не можем, значит для нас настройки нет.
            tracing::warn!(
                value_type = ty.0,
                "значение в реестре не строкового типа, считаем пустым"
            );
            return Ok(String::new());
        }
        Ok(decode_utf16_sz(&bytes))
    }

    /// Как `query_string`, но не отбрасывает тип значения (`REG_SZ` или
    /// `REG_EXPAND_SZ`). Нужна только `autostart`'s тестовой инфраструктуре
    /// восстановления: если вернуть прежнее `REG_EXPAND_SZ`-значение обратно
    /// через `set_string` (она пишет только `REG_SZ`), `%VAR%` в нём
    /// перестанет раскрываться при следующем реальном чтении Windows — тип
    /// нужен, чтобы восстановить именно тем же типом (`set_string_as`).
    /// `read`/`apply` этим методом не пользуются, `query_string` тоже не
    /// меняется — обе функции читают то же самое через `query_raw`.
    ///
    /// За `test-registry`: единственный вызывающий — тестовая инфраструктура
    /// `autostart`, тоже за этой фичей; без гейта здесь в сборке без фичи
    /// у этого метода оказалось бы ноль вызывающих.
    #[cfg(feature = "test-registry")]
    pub(crate) fn query_string_with_type(
        &self,
        name: PCWSTR,
    ) -> Result<Option<(REG_VALUE_TYPE, String)>, WinNetError> {
        let Some((ty, bytes)) = self.query_raw(name)? else {
            return Ok(None);
        };
        if ty != REG_SZ && ty != REG_EXPAND_SZ {
            tracing::warn!(
                value_type = ty.0,
                "значение в реестре не строкового типа, считаем отсутствующим"
            );
            return Ok(None);
        }
        Ok(Some((ty, decode_utf16_sz(&bytes))))
    }

    /// `REG_DWORD`; отсутствующее — 0.
    fn query_dword(&self, name: PCWSTR) -> Result<u32, WinNetError> {
        let Some((ty, bytes)) = self.query_raw(name)? else {
            return Ok(0);
        };
        if ty != REG_DWORD || bytes.len() < 4 {
            tracing::warn!(
                value_type = ty.0,
                len = bytes.len(),
                "ProxyEnable не похож на REG_DWORD, считаем выключенным"
            );
            return Ok(0);
        }
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn set_dword(&self, name: PCWSTR, value: u32) -> Result<(), WinNetError> {
        let bytes = value.to_le_bytes();
        // SAFETY: ключ открыт с KEY_WRITE и жив; name — статическая строка с
        // нулём; срез из четырёх байт живёт дольше вызова, и его длина
        // передаётся API самой обёрткой windows-rs.
        unsafe { RegSetValueExW(self.0, name, 0, REG_DWORD, Some(&bytes[..])) }.ok()?;
        Ok(())
    }

    pub(crate) fn set_string(&self, name: PCWSTR, value: &str) -> Result<(), WinNetError> {
        let bytes = encode_utf16_sz(value);
        // SAFETY: ключ открыт с KEY_WRITE и жив; name — статическая строка с
        // нулём; буфер живёт до конца вызова, а его длина (уже включающая
        // завершающий нулевой символ) передаётся API из самого среза.
        unsafe { RegSetValueExW(self.0, name, 0, REG_SZ, Some(&bytes)) }.ok()?;
        Ok(())
    }

    /// Как `set_string`, но тип значения — параметр, а не всегда `REG_SZ`.
    /// Тело намеренно не переиспользует `set_string` через делегирование:
    /// так `set_string`, а с ней и её вызывающие (`apply` в этом модуле и
    /// `autostart::enable_at`), остаются нетронутыми ни строкой. Нужна
    /// только `autostart`'s тестовому восстановлению — вернуть значение,
    /// бывшее `REG_EXPAND_SZ`, тем же типом, а не понизить его до `REG_SZ`.
    ///
    /// За `test-registry` по той же причине, что и у `query_string_with_type`
    /// выше.
    #[cfg(feature = "test-registry")]
    pub(crate) fn set_string_as(
        &self,
        name: PCWSTR,
        value: &str,
        value_type: REG_VALUE_TYPE,
    ) -> Result<(), WinNetError> {
        let bytes = encode_utf16_sz(value);
        // SAFETY: ключ открыт с KEY_WRITE и жив; name — статическая строка с
        // нулём; буфер живёт до конца вызова, а его длина (уже включающая
        // завершающий нулевой символ) передаётся API из самого среза.
        unsafe { RegSetValueExW(self.0, name, 0, value_type, Some(&bytes)) }.ok()?;
        Ok(())
    }

    /// Удаляет значение `name`. Отсутствие значения — не ошибка: для
    /// `autostart::disable`, единственного вызывающего этого метода, это
    /// ровно то состояние, которого он добивается. `read`/`apply` этим
    /// методом не пользуются — прокси-настройки никогда не удаляют, только
    /// выключают через `ProxyEnable`.
    pub(crate) fn delete_value(&self, name: PCWSTR) -> Result<(), WinNetError> {
        // SAFETY: ключ открыт с KEY_WRITE и жив; `name` — строка с нулём на
        // тех же условиях, что и в `query_string`/`set_string` выше.
        let rc = unsafe { RegDeleteValueW(self.0, name) };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        rc.ok()?;
        Ok(())
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: хендл получен от RegOpenKeyExW, принадлежит только этому
        // значению (RegKey не Copy и не Clone) и закрывается ровно один раз —
        // после Drop им уже никто не пользуется. Результат игнорируем
        // сознательно: из Drop его некуда вернуть, а падать при закрытии
        // ключа хуже, чем не заметить.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// `REG_SZ` → `String`. Завершающий нулевой символ входит в длину значения,
/// поэтому режем строку по нему, иначе в конфиг уедет `"...\0"`.
fn decode_utf16_sz(bytes: &[u8]) -> String {
    // `as_chunks` вместо `chunks_exact(2)`: то же самое поведение (лишний
    // байт с нечётной длины молча отбрасывается в обоих случаях), но
    // `clippy::chunks_exact_to_as_chunks` требует эту форму начиная с MSRV
    // 1.88 (fix round 5).
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// `String` → байты `REG_SZ`: UTF-16LE с завершающим нулём.
///
/// Нуль обязателен. Без него `reg query` покажет вроде бы правильное
/// значение, а приложения, читающие его через WinINET, получат строку с
/// мусором на конце — ошибка, которую очень трудно увидеть глазами.
fn encode_utf16_sz(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity((s.len() + 1) * 2);
    for unit in s.encode_utf16().chain(std::iter::once(0)) {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// Текущие системные настройки прокси.
///
/// Отсутствующее значение — не ошибка: на машине, где прокси никогда не
/// настраивали, `ProxyServer` просто нет, и это пустая строка.
pub fn read() -> Result<SysProxy, WinNetError> {
    let key = RegKey::open(HKEY_CURRENT_USER, SUBKEY, KEY_READ)?;
    Ok(SysProxy {
        enabled: key.query_dword(PROXY_ENABLE)? != 0,
        server: key.query_string(PROXY_SERVER)?,
        bypass: key.query_string(PROXY_OVERRIDE)?,
    })
}

/// Записывает настройки и уведомляет уже запущенные приложения.
///
/// Если `Err` вернулся из уведомления — то есть уже ПОСЛЕ записи, — реестр
/// **изменён**. Вызывающий обязан различать это и «ничего не записано»:
/// откат, если он нужен, придётся делать явно.
///
/// Строки всегда пишутся как `REG_SZ`. Значение, лежавшее в реестре как
/// `REG_EXPAND_SZ`, после нашей записи станет `REG_SZ`: содержимое
/// сохраняется, тип — нет.
pub fn apply(p: &SysProxy) -> Result<(), WinNetError> {
    {
        let key = RegKey::open(HKEY_CURRENT_USER, SUBKEY, KEY_WRITE)?;
        // Порядок записи не случаен: сначала адрес и исключения, выключатель —
        // последним. Оборвись запись посередине (ключ удалили, политика,
        // квота) — останется выключенный прокси со свежим адресом, безопасная
        // сторона. При обратном порядке трафик был бы уже включён и направлен
        // по СТАРОМУ адресу, причём без уведомлений: ровно та потеря сети,
        // ради предотвращения которой существует этот модуль.
        key.set_string(PROXY_SERVER, &p.server)?;
        key.set_string(PROXY_OVERRIDE, &p.bypass)?;
        key.set_dword(PROXY_ENABLE, u32::from(p.enabled))?;
        // Ключ закрывается здесь, до уведомления: пусть читатели видят уже
        // записанное, а не наш открытый на запись хендл.
    }

    // Без этих двух вызовов уже запущенные приложения продолжат ходить по
    // старым настройкам до перезапуска — снаружи это выглядит как «функция
    // не работает», и именно так о ней чаще всего и сообщают как о поломке.
    // SETTINGS_CHANGED говорит «настройки поменялись», REFRESH — «перечитайте
    // их сейчас»; поодиночке ни того, ни другого не хватает.
    //
    // SAFETY: дескриптор сессии не передаём (None — глобальное уведомление,
    // как и предписано документацией для этих двух опций), буфера нет,
    // и длина буфера соответственно 0.
    let changed = unsafe { InternetSetOptionW(None, INTERNET_OPTION_SETTINGS_CHANGED, None, 0) };
    // SAFETY: то же самое. Зовём независимо от исхода предыдущего: отказ
    // первого — не повод оставить приложения ещё и без перечитывания.
    // Наружу отдаём первую по порядку ошибку.
    let refreshed = unsafe { InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0) };
    changed?;
    refreshed?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_string_uses_semicolons_and_keeps_local_token() {
        // WinINET разделяет точкой с запятой, а не запятой, и понимает
        // особый токен <local> для адресов без точки.
        let s = to_bypass_string("localhost,127.0.0.1,.local,192.168.0.0/16");
        assert!(s.contains(';'), "получили: {s}");
        assert!(!s.contains(','), "запятых остаться не должно: {s}");
        assert!(s.contains("<local>"), "локальные имена без точки: {s}");
    }

    #[test]
    fn bypass_string_converts_dot_suffix_to_wildcard() {
        // «.local» в нашем формате — суффикс; WinINET ждёт «*.local».
        let s = to_bypass_string(".local");
        assert!(s.contains("*.local"), "получили: {s}");
    }

    #[test]
    fn bypass_string_skips_empty_entries() {
        let s = to_bypass_string("localhost,,  ,127.0.0.1");
        assert!(!s.contains(";;"), "получили: {s}");
    }

    #[test]
    fn bypass_string_does_not_duplicate_an_existing_local_token() {
        // На вход может прийти уже готовый список WinINET — например,
        // сохранённое значение пользователя при восстановлении.
        let s = to_bypass_string("localhost,<local>");
        assert_eq!(s.matches("<local>").count(), 1, "получили: {s}");
    }

    #[test]
    fn bypass_string_skips_a_bare_dot() {
        // «.» не задаёт суффикса: «*.» WinINET ничего не значит.
        let s = to_bypass_string("localhost,.");
        assert_eq!(s, "localhost;<local>", "получили: {s}");
    }

    #[test]
    fn reg_sz_bytes_end_with_a_utf16_nul() {
        // Самая незаметная ошибка в этом модуле: значение без завершающего
        // нуля выглядит правильным в `reg query` и ломается в приложениях.
        let b = encode_utf16_sz("ab");
        assert_eq!(b, vec![b'a', 0, b'b', 0, 0, 0], "получили: {b:?}");
    }

    #[test]
    fn reg_sz_bytes_of_an_empty_string_are_just_the_nul() {
        assert_eq!(encode_utf16_sz(""), vec![0, 0]);
    }

    #[test]
    fn decoding_drops_the_terminating_nul() {
        // Реестр отдаёт длину вместе с нулём; если его не срезать, он уедет
        // в конфиг и вернётся оттуда в реестр уже внутри строки.
        assert_eq!(
            decode_utf16_sz(&encode_utf16_sz("127.0.0.1:3129")),
            "127.0.0.1:3129"
        );
        assert_eq!(decode_utf16_sz(&[]), "");
    }

    #[cfg(windows)]
    #[test]
    fn reading_current_settings_does_not_fail() {
        // Смоук на живой машине: ключ существует всегда, даже когда прокси
        // выключен. Ничего не меняем — только читаем.
        let s = read().expect("HKCU Internet Settings обязан читаться");
        // enabled может быть любым; проверяем лишь, что структура заполнена
        let _ = (s.enabled, s.server.len(), s.bypass.len());
    }
}
