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

use windows::core::{w, Error as WinError, HRESULT, PCWSTR};
use windows::Win32::Foundation::{LocalFree, BOOL, HLOCAL, NO_ERROR};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR,
};
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
    #[error("не удалось создать {0:?}: {1}")]
    CreateDir(std::path::PathBuf, std::io::Error),
    #[error("не удалось выставить права доступа на каталог данных службы: {0}")]
    Acl(WinError),
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

/// Создаёт `%ProgramData%\ProxyPilot` (если его ещё нет) и ставит на него
/// явный защищённый DACL: SYSTEM и встроенные администраторы — полный
/// доступ, встроенные пользователи — только чтение.
///
/// Ревью round 2 (задача 6), Important №9: без этого каталог наследует ACE
/// родителя — `%ProgramData%` по умолчанию даёт запись группе `Users` — а
/// именно здесь лежат `profile.toml` и `applied.toml`, которые читает
/// системная служба. Незащищённый каталог был бы ровно тем каналом
/// подмены, ради закрытия которого сама служба и написана (докблок
/// крейта — «читать пользовательский файл значило бы дать кому угодно
/// диктовать сетевые настройки системной службе»; открытый на запись
/// каталог ProgramData — тот же дефект под другим именем). Вызывается из
/// `install`, который и так уже требует администратора — второго UAC это
/// не добавляет.
fn secure_program_data_dir() -> Result<(), InstallError> {
    let dir = crate::profile::program_data_dir().join("ProxyPilot");
    std::fs::create_dir_all(&dir).map_err(|e| InstallError::CreateDir(dir.clone(), e))?;

    // D:P — защищённый DACL (не наследует ACE родителя); SY — LocalSystem,
    // BA — встроенные администраторы (полный доступ, FA), BU — встроенные
    // пользователи (только чтение, FR). OICI — наследуется файлами и
    // подкаталогами, которые появятся здесь позже (`logs\`).
    let sddl = w!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FR;;;BU)");
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl` — статическая, корректно завершённая нулём
    // wide-строка; `descriptor` — живая переменная, которую функция
    // заполняет указателем на память, выделенную ЕЮ САМОЙ (освобождается
    // ниже через `LocalFree` — так документирована эта функция).
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl,
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(InstallError::Acl)?;

    let mut dacl_present = BOOL(0);
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut dacl_defaulted = BOOL(0);
    // SAFETY: `descriptor` — валидный дескриптор, только что созданный
    // выше; три указателя на выходные параметры — живые локальные
    // переменные. Сам DACL, который эта функция отдаёт через `dacl`, —
    // часть памяти `descriptor`, а не отдельная аллокация: освобождать его
    // отдельно не нужно и нельзя, только весь `descriptor` целиком ниже.
    let dacl_lookup = unsafe {
        GetSecurityDescriptorDacl(
            descriptor,
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        )
    };

    let result = dacl_lookup.and_then(|()| {
        // SAFETY: `dir_wide` жива до конца этого вызова; `dacl` указывает
        // внутрь `descriptor`, который жив до `LocalFree` ниже, то есть до
        // конца этой функции — переживает сам вызов `SetNamedSecurityInfoW`.
        let dir_wide = wide(&dir.display().to_string());
        let rc = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR::from_raw(dir_wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl as *const ACL),
                None,
            )
        };
        if rc == NO_ERROR {
            Ok(())
        } else {
            Err(WinError::from_hresult(HRESULT::from_win32(rc.0)))
        }
    });

    // SAFETY: `descriptor.0` получен от
    // `ConvertStringSecurityDescriptorToSecurityDescriptorW` выше,
    // документация которой прямо предписывает освобождать его именно
    // `LocalFree`; используется здесь в последний раз, независимо от
    // исхода вызовов выше — освобождение не должно зависеть от того,
    // удался ли `SetNamedSecurityInfoW`.
    let _ = unsafe { LocalFree(HLOCAL(descriptor.0)) };

    result.map_err(InstallError::Acl)
}

/// Регистрирует службу `ProxyPilotNetProfile`, запускающую `exe_path` при
/// каждой загрузке Windows от LocalSystem. См. докблок модуля про то,
/// почему это не запускает саму службу.
pub fn install(exe_path: &Path) -> Result<(), InstallError> {
    secure_program_data_dir()?;

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

/// Если служба когда-то поставила статику, возвращает адаптер на DHCP —
/// последний шанс это сделать: после `DeleteService` ниже возвращать
/// станет некому. Ревью round 2 (задача 6), Important №8 — с явной
/// оговоркой контроллера: `stop` НЕ обязан откатывать (это обычно просто
/// перезагрузка, и профиль применится заново при следующем старте), но
/// `uninstall` обязан, потому что для него следующего раза не будет —
/// ноутбук, покинувший офис уже без службы, держал бы офисную статику
/// вечно.
///
/// Печатает результат в stdout/stderr, а не логирует через `tracing`: это
/// разовая команда человека из терминала (`uninstall-service`), а не
/// операционный журнал службы, и подписчика `tracing` здесь никто не
/// поднимал (докблок `crates/app/src/main.rs`).
fn revert_to_dhcp_before_removal() {
    let applied = crate::state::load_from(&crate::state::path());
    let (Some(_ip), Some(guid)) = (applied.ip, applied.iface_guid.as_deref()) else {
        // Мы ничего не ставили (или уже откатили раньше) — сети трогать
        // нечего.
        return;
    };
    match crate::adapter::friendly_name_for_guid(guid) {
        Ok(Some(alias)) => {
            let cmds = crate::netsh_cmd::dhcp_restore_commands(&alias);
            if crate::exec::run_netsh_batch(cmds) {
                let _ = crate::state::save_to(
                    &crate::state::path(),
                    &crate::state::AppliedState::default(),
                );
                println!("Адаптер «{alias}» возвращён на DHCP перед удалением службы.");
            } else {
                eprintln!(
                    "ВНИМАНИЕ: не удалось вернуть адаптер «{alias}» на DHCP перед удалением \
                     службы. Сделайте это вручную: netsh interface ipv4 set address \
                     name=\"{alias}\" source=dhcp && netsh interface ipv4 set dnsservers \
                     name=\"{alias}\" source=dhcp"
                );
            }
        }
        Ok(None) => {
            eprintln!(
                "ВНИМАНИЕ: служба помнит применённую статику, но адаптер с GUID {guid} \
                 сейчас не найден — проверьте сетевые настройки вручную перед удалением \
                 службы."
            );
        }
        Err(e) => {
            eprintln!(
                "ВНИМАНИЕ: не удалось определить адаптер для отката DHCP перед удалением \
                 службы: {e}. Проверьте сетевые настройки вручную."
            );
        }
    }
}

/// Снимает регистрацию службы. Перед этим — откат в DHCP, если служба
/// что-то ставила (`revert_to_dhcp_before_removal`). Не останавливает саму
/// службу, если она в этот момент запущена (`ControlService`/`StopService`
/// здесь не вызываются) — SCM сам помечает службу к удалению и убирает
/// запись, когда последний открытый хендл на неё закроется; человек,
/// вызвавший `uninstall-service` на живой службе, увидит это поведение SCM
/// как есть, а не подмену от нас.
pub fn uninstall() -> Result<(), InstallError> {
    revert_to_dhcp_before_removal();

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
