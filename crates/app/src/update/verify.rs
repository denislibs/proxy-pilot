//! Проверка подписи Authenticode скачанного файла перед заменой.
//!
//! Единственное, что отличает обновление от доставки чужого исполняемого
//! файла прямо в процесс, который правит ключи прокси в реестре и держит
//! слушающий сокет (`docs/process/win-delivery/task-3-brief.md`). Отсутствие
//! подписи и неверная подпись — один и тот же исход: отказ, не «установлено
//! без проверки» и не «молча пропущено». Здесь нет ни одного пути,
//! возвращающего успех при отсутствующей или неверной подписи, и нет
//! конфигурационного флага, который эту функцию обходил бы: единственный
//! выключатель продукта (`Config::check_for_updates`) относится к сетевому
//! ОПРОСУ релизов, не к проверке подписи уже скачанного файла — см. докблок
//! `update` о разнице.
//!
//! `WinVerifyTrust` — системный API (`wintrust.dll`), есть на любой Windows
//! из коробки, в отличие от `signtool.exe`, который живёт только в Windows
//! SDK и на машине получателя обновления отсутствует.
//!
//! Отзыв сертификата НЕ проверяется (`WTD_REVOKE_NONE` +
//! `WTD_CACHE_ONLY_URL_RETRIEVAL`): проверка отзыва сама требует сетевого
//! обращения к CRL/OCSP, а весь смысл вызывать эту функцию из фоновой
//! проверки обновлений — не зависеть от сети в непредсказуемый момент.
//! Хэш файла и цепочка доверия до корня проверяются полностью и без
//! исключений — сознательно принесённый в жертву предсказуемости сценарий
//! «сертификат отозван уже после публикации» не открывает дыру: подделанная
//! или посторонняя подпись всё равно отклоняется.
//!
//! **Сегодня сертификата подписи продукта не существует** (`docs/process/win-delivery/progress.md`,
//! задача 2). Это не значит, что проверка здесь работает наполовину или
//! отключена — значит, что для любого файла, скачанного из настоящего
//! релиза `denislibs/proxy-pilot` СЕГОДНЯ, эта функция вернёт отказ, и
//! обновление не установится. Это ожидаемое и правильное поведение, а не
//! дефект: как только сертификат появится и подписанный exe пройдёт по
//! конвейеру задачи 2/4, эта же функция без единой правки начнёт принимать
//! его.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Security::WinTrust::{
    WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
    WINTRUST_DATA_UICONTEXT, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE,
    WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
};

/// Отказ проверки подписи. Само значение — то, что попадёт в лог и в текст
/// на странице настроек: человек, увидевший «обновление не установлено»,
/// вправе узнать, почему.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError(pub String);

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Проверяет подпись Authenticode файла по пути. `Ok(())` — подпись
/// присутствует, действительна и ведёт к доверенному корню. Любой другой
/// исход — `Err`, без различения «нет подписи» и «подпись неверна» на
/// уровне типа: обеим сторонам вызывающий обязан отказать одинаково
/// (докблок модуля), и заводить второй вариант ради различения значило бы
/// оставить лазейку «а если просто нет подписи — ну ладно».
pub fn verify_authenticode(path: &Path) -> Result<(), VerifyError> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        hFile: windows::Win32::Foundation::HANDLE::default(),
        pgKnownSubject: std::ptr::null_mut(),
    };

    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        pPolicyCallbackData: std::ptr::null_mut(),
        pSIPClientData: std::ptr::null_mut(),
        dwUIChoice: WTD_UI_NONE,
        // Ни одного сетевого обращения — см. докблок модуля.
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        hWVTStateData: windows::Win32::Foundation::HANDLE::default(),
        pwszURLReference: windows::core::PWSTR::null(),
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        dwUIContext: WINTRUST_DATA_UICONTEXT(0),
        pSignatureSettings: std::ptr::null_mut(),
    };

    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;

    // SAFETY: `file_info`/`data` живут до конца блока, длиннее самого
    // вызова; `wide` (буфер пути) живёт до конца функции, дольше, чем на
    // него ссылается `file_info.pcwszFilePath`, которым пользуется только
    // этот вызов. `hwnd = None` — без UI, что и требует `WTD_UI_NONE` в
    // `dwUIChoice`. Второй вызов с `WTD_STATEACTION_CLOSE` обязателен по
    // документации `WinVerifyTrust`: первый вызов при
    // `WTD_STATEACTION_VERIFY` может выделить `hWVTStateData`, и не закрыть
    // его — утечь дескриптор на каждой проверке (а проверка происходит на
    // каждом скачанном обновлении и при каждом запуске с ожидающим
    // обновлением).
    let status = unsafe { WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as _) };

    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: то же самое struct, тот же указатель на `file_info` внутри
    // (ещё жив — `file_info` не выходит из области видимости раньше этой
    // строки); закрывающий вызов документирован как обязанный получить
    // структуру в том же виде, только с изменённым `dwStateAction`.
    let _ = unsafe { WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as _) };

    if status == 0 {
        Ok(())
    } else {
        Err(VerifyError(format!(
            "подпись не подтверждена (WinVerifyTrust: {})",
            HRESULT(status).message()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Гарантированно присутствующий на любой Windows (в т. ч. Server Core)
    /// подписанный Microsoft файл — не создаём и не коммитим ничего своего,
    /// читаем то, что уже есть в системе. Копируется, а не проверяется на
    /// месте: тест ниже портит копию побайтово, а System32 трогать нельзя
    /// ни при каких обстоятельствах.
    fn system_signed_file() -> std::path::PathBuf {
        std::path::PathBuf::from(r"C:\Windows\System32\kernel32.dll")
    }

    fn temp_copy(src: &std::path::Path, name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("proxypilot-test-verify");
        std::fs::create_dir_all(&dir).unwrap();
        let dst = dir.join(name);
        std::fs::copy(src, &dst).expect("исходный файл обязан читаться");
        dst
    }

    #[test]
    fn a_genuinely_signed_system_file_is_accepted() {
        // Не только «отказывает всегда» — реальный, честно подписанный
        // Microsoft-файл обязан пройти без единой правки.
        let path = temp_copy(&system_signed_file(), "kernel32-intact.dll");
        let result = verify_authenticode(&path);
        assert!(result.is_ok(), "получили: {result:?}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_file_without_any_signature_is_refused() {
        // Собственный тестовый бинарь этой сборки — настоящий PE, без
        // Authenticode-подписи (сборка отладочная, шаг подписи задачи 2
        // требует секрета, которого на этой машине нет).
        let exe = std::env::current_exe().expect("у процесса есть свой путь");
        let result = verify_authenticode(&exe);
        assert!(result.is_err(), "неподписанный файл обязан быть отклонён");
    }

    #[test]
    fn a_tampered_signature_is_refused() {
        let path = temp_copy(&system_signed_file(), "kernel32-tampered.dll");
        {
            use std::io::{Read, Seek, SeekFrom, Write};
            let mut f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            // Байт в теле кода (не в самом хвосте, где обычно лежит только
            // таблица сертификатов) — хэш, который покрывает подпись,
            // обязан включать эту область.
            f.seek(SeekFrom::Start(4096)).unwrap();
            let mut byte = [0u8; 1];
            f.read_exact(&mut byte).unwrap();
            byte[0] ^= 0xFF;
            f.seek(SeekFrom::Start(4096)).unwrap();
            f.write_all(&byte).unwrap();
        }
        let result = verify_authenticode(&path);
        assert!(
            result.is_err(),
            "испорченная подпись обязана быть отклонена"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_is_refused_not_panicking() {
        let missing = std::env::temp_dir().join("proxypilot-test-verify-does-not-exist.exe");
        let result = verify_authenticode(&missing);
        assert!(result.is_err());
    }
}
