//! Жизненный цикл системного прокси.
//!
//! Самая опасная часть приложения. Записав в HKCU указатель на себя, мы
//! становимся единственной дорогой в интернет для всего, что ходит через
//! WinINET, — и если процесс уйдёт, не прибравшись, машина останется без
//! сети. Отсюда три правила, каждое из которых здесь и реализовано:
//!
//! 1. Исходное значение ложится на диск (в конфиг, с `sync_all`) ДО того,
//!    как мы тронем реестр.
//! 2. Восстановление идёт по любому пути выхода: штатному, паническому и
//!    закрытию консоли. Для этого сохранённое значение живёт в глобальной
//!    ячейке, а не в стеке `main`.
//! 3. На старте распознаётся наш же след в реестре, оставшийся от убитого
//!    процесса: принять его за «настройки пользователя» значило бы навсегда
//!    закрепить указатель на мёртвый слушатель как «исходное состояние».

use std::sync::{Mutex, PoisonError};

use proxypilot_core::config::{Config, SavedSysProxy};
use proxypilot_winnet::sysproxy::{self, SysProxy};
use tracing::{error, info, warn};

/// Что стояло в системных настройках до нас. Глобальная — потому что
/// восстанавливать приходится из обработчика закрытия консоли, куда ничего
/// передать нельзя.
///
/// Значение забирается через `take()`: восстановление обязано случиться
/// ровно один раз, сколько бы путей выхода ни сработало одновременно.
static ORIGINAL: Mutex<Option<Taken>> = Mutex::new(None);

/// Что мы записали и что обязаны вернуть.
struct Taken {
    /// Настройки пользователя до нашей записи.
    original: SysProxy,
    /// Порт, на который мы направили системный прокси. Нужен, чтобы при
    /// восстановлении отличить нашу запись от чужой: пока мы работали,
    /// значение мог поменять GPO или сам пользователь.
    our_port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("не прочитать системные настройки прокси: {0}")]
    Read(String),
    #[error("не сохранить исходные настройки в конфиг: {0}")]
    Save(String),
    #[error("не записать системные настройки прокси: {0}")]
    Apply(String),
}

/// Указывает ли текущая системная настройка на НАШ слушатель.
///
/// Смысл — «в реестре стоит наш адрес». Того, что моста нет, функция знать
/// не может и не должна: это устанавливает вызывающий тем, что успешно занял
/// порт (см. `main`), — если бы мост работал, порт был бы занят.
pub fn is_stale_pointer(current: &SysProxy, our_port: u16) -> bool {
    // Выключенный прокси никого не ломает: восстанавливать нечего, а принять
    // его за наш след значило бы затереть чужую настройку.
    current.enabled && server_points_at_port(&current.server, our_port)
}

/// Указывает ли `ProxyServer` на наш адрес — без оглядки на выключатель.
///
/// Отдельно от `is_stale_pointer`, потому что при восстановлении выключатель
/// как раз не важен: если пользователь снял галочку, пока мы работали, адрес
/// в реестре всё равно остался нашим, и убрать его — по-прежнему наше дело.
fn server_points_at_port(server: &str, our_port: u16) -> bool {
    server
        .split(';')
        .any(|part| points_at_us(part.trim(), our_port))
}

/// Одна запись `ProxyServer`: либо голый `host:port`, либо форма WinINET для
/// отдельного протокола — `http=host:port`.
fn points_at_us(part: &str, our_port: u16) -> bool {
    let addr = part.split_once('=').map_or(part, |(_, a)| a);
    let Some((host, port)) = addr.rsplit_once(':') else {
        return false;
    };
    matches!(port.parse::<u16>(), Ok(p) if p == our_port) && is_loopback_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    // IPv6 в `ProxyServer` пишется в скобках.
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn to_saved(p: &SysProxy) -> SavedSysProxy {
    SavedSysProxy {
        enabled: p.enabled,
        server: p.server.clone(),
        bypass: p.bypass.clone(),
    }
}

fn from_saved(s: &SavedSysProxy) -> SysProxy {
    SysProxy {
        enabled: s.enabled,
        server: s.server.clone(),
        bypass: s.bypass.clone(),
    }
}

/// Сохраняет исходные настройки и направляет системный прокси на нас.
///
/// Порядок здесь и есть весь смысл функции: прочитать → записать исходное на
/// диск → и только потом тронуть реестр. Вызывать строго ПОСЛЕ того, как
/// слушатель поднят: иначе между записью в реестр и первым `accept` остаётся
/// окно, в котором система уже шлёт трафик туда, где никто не слушает.
///
/// Возвращает то, что реестр говорил ДО этого вызова (то самое `current`, не
/// `original` — стирание следов мёртвого процесса ниже к этому значению не
/// применяется). Это единственный момент во всём запуске, когда «что было в
/// реестре» ещё не переписано нашей собственной записью; вызывающий
/// (`main.rs`) передаёт его дальше в диагностику, потому что второе чтение
/// после этой функции показало бы уже наш адрес и замаскировало бы ровно то,
/// что диагностика обязана заметить.
pub fn take_over(cfg: &mut Config, port: u16) -> Result<SysProxy, ProxyError> {
    let current = sysproxy::read().map_err(|e| ProxyError::Read(e.to_string()))?;
    let before = current.clone();

    let original = if is_stale_pointer(&current, port) {
        // В реестре наш адрес, а моста нет — прошлый процесс убили. То, что
        // сейчас в реестре, принадлежит НАМ, а не пользователю; сохранить
        // это как «исходное» значило бы закрепить указатель на мёртвый
        // слушатель навсегда.
        match cfg.saved_sysproxy.as_ref() {
            Some(saved) => {
                warn!(
                    server = %current.server,
                    "в реестре остался наш адрес от прошлого запуска, \
                     исходные настройки берём из конфига"
                );
                from_saved(saved)
            }
            None => {
                // Конфиг пуст, хотя запись в реестр была: сохранённого
                // значения не осталось (конфиг стёрли, профиль переехал).
                // Вернуть пользователю его настройки мы уже не можем;
                // лучшее из доступного — выключенный прокси: он никуда не
                // указывает и потому никого не оставляет без сети.
                error!(
                    server = %current.server,
                    "в реестре наш адрес, но исходных настроек в конфиге нет; \
                     при выходе вернём выключенный прокси"
                );
                SysProxy::default()
            }
        }
    } else {
        current
    };

    // На диск — раньше реестра. `Config::save` пишет во временный файл,
    // сбрасывает его на носитель (`sync_all`) и переименовывает: обрезанного
    // конфига после сбоя не будет никогда.
    //
    // Остаточное окно всё же есть, и честнее его назвать: запись в каталог,
    // которую делает переименование, мы не сбрасываем (для этого пришлось бы
    // открывать и синхронизировать сам каталог). Отключение питания в
    // промежутке между `rename` и сбросом метаданных ФС может оставить конфиг
    // со старым содержимым. Окно измеряется задержкой журнала NTFS, а платой
    // за его закрытие была бы синхронизация каталога на каждое сохранение
    // режима; для «убили процесс» — того случая, ради которого всё это
    // затевалось, — хватает и текущей гарантии, потому что файл к моменту
    // записи в реестр уже на носителе.
    cfg.saved_sysproxy = Some(to_saved(&original));
    cfg.save().map_err(|e| ProxyError::Save(e.to_string()))?;

    // В глобальную ячейку — тоже раньше записи в реестр: `apply` может
    // отказать уже ПОСЛЕ записи (модуль sysproxy это прямо оговаривает),
    // и выход всё равно обязан вернуть исходное.
    *ORIGINAL.lock().unwrap_or_else(PoisonError::into_inner) = Some(Taken {
        original: original.clone(),
        our_port: port,
    });

    let ours = SysProxy {
        enabled: true,
        server: format!("127.0.0.1:{port}"),
        bypass: sysproxy::to_bypass_string(&cfg.no_proxy),
    };
    match sysproxy::apply(&ours) {
        Ok(()) => {
            info!(server = %ours.server, "системный прокси направлен на мост");
            Ok(before)
        }
        Err(e) => Err(ProxyError::Apply(e.to_string())),
    }
}

/// Возвращает системный прокси в то состояние, что было до запуска.
///
/// Идемпотентна: сохранённое значение забирается из ячейки, поэтому второй
/// вызов (штатный выход, а следом `Drop` стража) ничего не делает.
pub fn restore() {
    let saved = ORIGINAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let Some(Taken { original, our_port }) = saved else {
        return;
    };

    // Пока мы работали, значение мог поменять кто-то ещё: обновление
    // групповой политики, другой инструмент, сам пользователь. Слепо
    // накатить снимок значило бы тихо отменить чужое осознанное изменение.
    // Восстанавливаем, только если в реестре всё ещё наш адрес.
    //
    // Сюда же попадает и вторая ситуация: наша запись вообще не легла —
    // первый же `set_string` в `apply` отказал, и в реестре по-прежнему
    // значение пользователя. Поведение то же (не писать ничего), поэтому
    // сообщение не берётся утверждать, КТО изменил значение: оно
    // констатирует только то, что достоверно известно.
    match sysproxy::read() {
        Ok(current) if !server_points_at_port(&current.server, our_port) => {
            warn!(
                current = %current.server,
                enabled = current.enabled,
                "системные настройки прокси не указывают на нас — оставляем как есть"
            );
            return;
        }
        Ok(_) => {}
        // Не прочитали — но это не повод оставить машину с указателем на
        // мёртвый слушатель. Восстанавливаем вслепую: риск отменить чужое
        // изменение меньше риска отсутствия сети.
        Err(e) => warn!(
            error = %e,
            concat!(
                "не прочитать текущие настройки перед восстановлением, ",
                "восстанавливаем сохранённые"
            )
        ),
    }

    match sysproxy::apply(&original) {
        Ok(()) => info!(
            enabled = original.enabled,
            server = %original.server,
            "системный прокси восстановлен"
        ),
        // Единственное место во всём приложении, где отказ означает
        // «пользователь остался без сети». Молчать нельзя даже здесь.
        Err(e) => error!(
            error = %e,
            enabled = original.enabled,
            server = %original.server,
            "НЕ УДАЛОСЬ восстановить системный прокси; поправьте вручную в \
             HKCU Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings"
        ),
    }
}

/// Восстанавливает системный прокси при разрушении — в том числе при
/// раскрутке стека после паники, где никакой явный код выхода не сработает.
pub struct RestoreOnDrop;

impl Drop for RestoreOnDrop {
    fn drop(&mut self) {
        restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_registry_pointing_at_us_without_a_bridge_is_detected() {
        // Прошлый процесс убили: в реестре наш адрес, моста нет.
        assert!(is_stale_pointer(
            &SysProxy {
                enabled: true,
                server: "127.0.0.1:3129".into(),
                bypass: String::new(),
            },
            3129
        ));
        assert!(!is_stale_pointer(
            &SysProxy {
                enabled: true,
                server: "10.0.0.2:3128".into(),
                bypass: String::new(),
            },
            3129
        ));
    }

    #[test]
    fn a_disabled_pointer_at_our_address_is_not_stale() {
        // Выключенный прокси никого не ломает — восстанавливать нечего,
        // а принять его за наш след значило бы затереть чужую настройку.
        assert!(!is_stale_pointer(
            &SysProxy {
                enabled: false,
                server: "127.0.0.1:3129".into(),
                bypass: String::new(),
            },
            3129
        ));
    }

    #[test]
    fn our_address_on_another_port_is_not_ours() {
        assert!(!is_stale_pointer(
            &SysProxy {
                enabled: true,
                server: "127.0.0.1:8080".into(),
                bypass: String::new(),
            },
            3129
        ));
    }

    #[test]
    fn the_per_protocol_form_is_recognised_too() {
        // WinINET допускает «http=host:port;https=host:port» — если там наш
        // адрес, это тоже наш след.
        assert!(is_stale_pointer(
            &SysProxy {
                enabled: true,
                server: "http=127.0.0.1:3129;https=127.0.0.1:3129".into(),
                bypass: String::new(),
            },
            3129
        ));
    }

    #[test]
    fn localhost_by_name_is_ours_as_well() {
        assert!(is_stale_pointer(
            &SysProxy {
                enabled: true,
                server: "localhost:3129".into(),
                bypass: String::new(),
            },
            3129
        ));
    }

    #[test]
    fn a_pointer_at_us_is_recognised_even_with_the_switch_off() {
        // Это не то же самое, что `is_stale_pointer`: при восстановлении
        // выключатель не важен — адрес в реестре всё равно наш, и убирать
        // его наше дело.
        assert!(server_points_at_port("127.0.0.1:3129", 3129));
        assert!(server_points_at_port(
            "http=localhost:3129;https=[::1]:3129",
            3129
        ));
        assert!(!server_points_at_port("203.0.113.10:3128", 3129));
        assert!(!server_points_at_port("", 3129));
    }

    #[test]
    fn the_real_corporate_setting_of_this_machine_is_left_alone() {
        // Настоящее значение с машины, на которой это писалось. Ошибочное
        // «да» здесь означало бы, что мы приняли настройки пользователя за
        // свои и потеряли их навсегда.
        assert!(!is_stale_pointer(
            &SysProxy {
                enabled: false,
                server: "203.0.113.10:3128".into(),
                bypass: "192.168.*;lo.example.internal;<local>".into(),
            },
            3129
        ));
    }
}
