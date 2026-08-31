//! Установка и снятие службы `ProxyPilotNetProfile` через Service Control
//! Manager.
//!
//! Единственные вызывающие этого модуля во всём продукте — обработчики
//! `install-service`/`uninstall-service` в приложении
//! (`crates/app/src/main.rs`), и оба требуют прав администратора. Это
//! единственный запрос UAC во всём продукте (`CLAUDE.md`, «Права
//! администратора») — намеренно однократный и необязательный: кто не
//! просит статику, тот не видит ни одного диалога.
//!
//! Контроллер сессии прямо запрещает выполнять эту регистрацию на машине,
//! где идёт разработка (`CLAUDE.md`, «Живые проверки, которые не делает
//! агент» — «установка службы профиля сети» названа отдельным пунктом).
//! По той же причине модуль не покрыт автотестами: проверить «служба
//! зарегистрирована в SCM», не регистрируя её, нельзя, а регистрировать
//! здесь запрещено. Правильно написанный код и код, который ни разу не
//! запускался, выглядят одинаково — единственное, что доказывает разницу,
//! это ручной прогон человеком (см. отчёт задачи).
//!
//! `install` не запускает службу: `CreateServiceW` только регистрирует её с
//! `SERVICE_AUTO_START`, `StartServiceW` здесь не вызывается вовсе — первый
//! пуск делает сам SCM при следующей перезагрузке или человек вручную из
//! `services.msc`/`Start-Service`.

use std::path::Path;

use windows::core::{Error as WinError, PCWSTR};
use windows::Win32::System::Services::{
    CloseServiceHandle, CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW, SC_HANDLE,
    SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
    SERVICE_ERROR_NORMAL, SERVICE_WIN32_OWN_PROCESS,
};

use crate::{SERVICE_DISPLAY_NAME, SERVICE_NAME};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("не удалось открыть диспетчер служб: {0}")]
    OpenManager(WinError),
    #[error("не удалось создать службу {SERVICE_NAME}: {0}")]
    Create(WinError),
    #[error("не удалось открыть службу {SERVICE_NAME} для удаления: {0}")]
    OpenService(WinError),
    #[error("не удалось удалить службу {SERVICE_NAME}: {0}")]
    Delete(WinError),
}

/// UTF-16 строка с завершающим нулём — обязательный формат для `PCWSTR`.
/// Возвращает `Vec<u16>`, а не сырой указатель: буфер обязан жить до конца
/// вызова, который получит указатель на него, и владение в `Vec` это и
/// обеспечивает.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Путь к `exe_path` в кавычках — то, что уходит в `lpBinaryPathName`
/// `CreateServiceW`. Кавычки обязательны: место установки по умолчанию
/// несёт пробелы («C:\Program Files\...»), и SCM без кавычек читает первый
/// пробел как конец пути к исполняемому файлу — тот же класс ошибки, что
/// описан в докблоке `winnet::openvpn::build_gui_command`, только для
/// командной строки службы, а не аргументов процесса. Вынесена отдельно от
/// `install`, чтобы это конкретное построение строки — единственная часть
/// модуля, не требующая ни SCM, ни `unsafe`, — было чем проверить тестом:
/// сама регистрация не тестируется автотестами вовсе (докблок модуля).
fn quoted_binary_path(exe_path: &Path) -> String {
    format!("\"{}\"", exe_path.display())
}

struct ScHandleGuard(SC_HANDLE);

impl Drop for ScHandleGuard {
    fn drop(&mut self) {
        // SAFETY: `self.0` получен от `OpenSCManagerW`/`CreateServiceW`/
        // `OpenServiceW` и ещё не закрывался — `Drop` случается ровно один
        // раз. Ошибку закрытия игнорируем: разбирать хендл больше нечем, а
        // падать в `Drop` нельзя.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

/// Регистрирует службу `ProxyPilotNetProfile`, запускающую `exe_path` при
/// каждой загрузке Windows от LocalSystem. См. докблок модуля про то,
/// почему это не запускает саму службу.
pub fn install(exe_path: &Path) -> Result<(), InstallError> {
    // SAFETY: оба параметра машины/базы — `PCWSTR::null()`, что означает
    // «эта машина, база SERVICES_ACTIVE_DATABASE по умолчанию»;
    // запрошенные права — минимум, достаточный для `CreateServiceW` ниже.
    let manager =
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CREATE_SERVICE) }
            .map_err(InstallError::OpenManager)?;
    let manager = ScHandleGuard(manager);

    let name = wide(SERVICE_NAME);
    let display = wide(SERVICE_DISPLAY_NAME);
    let binary_path = wide(&quoted_binary_path(exe_path));

    // SAFETY: `manager.0` — валидный хендл, открытый выше и живой до конца
    // этого вызова; все строковые буферы (`name`, `display`, `binary_path`)
    // живы до конца вызова; необязательные параметры (группа загрузки, тег,
    // зависимости, учётная запись, пароль) — `PCWSTR::null()`/`None`, что
    // означает LocalSystem без пароля и без зависимостей — ровно то, что
    // требует докблок крейта («служба работает от LocalSystem»).
    let service = unsafe {
        CreateServiceW(
            manager.0,
            PCWSTR::from_raw(name.as_ptr()),
            PCWSTR::from_raw(display.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR::from_raw(binary_path.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
    }
    .map_err(InstallError::Create)?;
    // Хендл только что созданной службы больше не нужен — регистрация уже
    // произошла, ждать от неё больше нечего.
    let _ = ScHandleGuard(service);

    Ok(())
}

/// Снимает регистрацию службы. Не останавливает её, если она в этот момент
/// запущена (`ControlService`/`StopService` здесь не вызываются) — SCM сам
/// помечает службу к удалению и убирает запись, когда последний открытый
/// хендл на неё закроется; человек, вызвавший `uninstall-service` на живой
/// службе, увидит это поведение SCM как есть, а не подмену от нас.
pub fn uninstall() -> Result<(), InstallError> {
    // SAFETY: см. `install` — те же предопределённые константы для машины
    // и базы по умолчанию.
    let manager = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map_err(InstallError::OpenManager)?;
    let manager = ScHandleGuard(manager);

    let name = wide(SERVICE_NAME);
    // SAFETY: `manager.0` — валидный хендл выше; `name` жив до конца этого
    // вызова. Win32 не заводит отдельного «права на удаление» для служб —
    // документированный набор прав для `DeleteService` в MSDN просто
    // `DELETE`, и открывать с полным `SERVICE_ALL_ACCESS` здесь — обычная
    // практика (то же самое делает, например, `sc.exe delete`).
    let service = unsafe {
        OpenServiceW(
            manager.0,
            PCWSTR::from_raw(name.as_ptr()),
            SERVICE_ALL_ACCESS,
        )
    }
    .map_err(InstallError::OpenService)?;
    let service = ScHandleGuard(service);

    // SAFETY: `service.0` — валидный хендл, только что открытый с правом
    // на удаление.
    unsafe { DeleteService(service.0) }.map_err(InstallError::Delete)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_appends_exactly_one_trailing_zero() {
        let w = wide("Ab");
        assert_eq!(w, vec![b'A' as u16, b'b' as u16, 0]);
    }

    #[test]
    fn wide_of_empty_string_is_just_the_terminator() {
        assert_eq!(wide(""), vec![0]);
    }

    #[test]
    fn wide_round_trips_non_ascii_text() {
        // Отображаемое имя службы — ASCII, но сама функция обязана
        // работать и на кириллице (путь к exe пользователя может лежать
        // под профилем с русским именем).
        let w = wide("Офис");
        let back = String::from_utf16(&w[..w.len() - 1]).expect("обязано разобраться");
        assert_eq!(back, "Офис");
        assert_eq!(*w.last().unwrap(), 0);
    }

    #[test]
    fn quoted_binary_path_wraps_a_path_with_spaces() {
        let p = Path::new(r"C:\Program Files\ProxyPilot\proxypilot-netsvc.exe");
        assert_eq!(
            quoted_binary_path(p),
            r#""C:\Program Files\ProxyPilot\proxypilot-netsvc.exe""#
        );
    }

    #[test]
    fn quoted_binary_path_wraps_a_path_without_spaces_too() {
        // Кавычки всегда, не только когда путь их «требует» — постоянство
        // проще и не заставляет функцию решать, нужны ли они в этот раз.
        let p = Path::new(r"C:\ProxyPilot\proxypilot-netsvc.exe");
        assert_eq!(
            quoted_binary_path(p),
            r#""C:\ProxyPilot\proxypilot-netsvc.exe""#
        );
    }
}
