//! Живость нашего туннеля по собственному логу `openvpn-gui.exe` —
//! единственному источнику, который в проде оказался и надёжным, и
//! ключуемым по тому, чем мы действительно владеем: имени профиля.
//!
//! # Как этот модуль появился (fix round 1, задача 7)
//!
//! Первая версия задачи 7 брала `our_alias = TUNNEL_PROFILE_NAME`
//! («proxypilot-office») и передавала его в
//! `tunnel_state::{our_tunnel_up, foreign_tunnel_up}` как псевдоним
//! адаптера. Ревью прочло реальные адаптеры этой машины (`Get-NetAdapter`,
//! только чтение) и показало: OpenVPN называет адаптер по ДРАЙВЕРУ
//! («OpenVPN Wintun» / «Wintun Userspace Tunnel», «TAP-Windows Adapter
//! V9», «OpenVPN Data Channel Offload»), а не по имени соединения —
//! никакой реальный адаптер никогда не совпадёт со строкой, которую мы
//! сами придумали для файла профиля. Последствие было хуже, чем «статус
//! неизвестен»: `our_tunnel_up` возвращала `false` НАВСЕГДА, а
//! `foreign_tunnel_up` — с тем же негодным alias — классифицировала НАШ
//! ЖЕ поднятый туннель как чужой, и правило «не трогать чужой туннель»
//! запрещало и подъём, и опускание разом. Дедлок, который задача 3 писала
//! `same_alias`/`our_tunnel_up` ровно затем, чтобы не допустить, вернулся
//! через ЗНАЧЕНИЕ, а не через сравнение.
//!
//! Тот же read-only просмотр машины нашёл рабочий источник: собственный
//! README инсталляции OpenVPN (`Program Files\OpenVPN\log\README.txt`)
//! прямо говорит — «Logs for connections started by the GUI are kept in
//! %USERPROFILE%\OpenVPN\log». Проверено на реальных файлах этой машины
//! (не наших — оставшихся от прежних подключений её пользователя, никакие
//! их имена или содержимое сюда не попали, только форма): время СОЗДАНИЯ
//! файла на годы старше времени последней ЗАПИСИ, а первая строка
//! содержимого датирована временем последней записи — то есть
//! `openvpn-gui.exe` открывает лог заново (усекая) на каждой попытке
//! подключения, а не копит его через сессии. Это подтверждает: файл в
//! любой момент содержит ровно ОДНУ, последнюю попытку — то, что нужно,
//! чтобы «нашли маркер завершения после маркера подъёма» значило именно
//! «сейчас не поднят», а не «когда-то давно отключились».
//!
//! Имя лога — `<имя профиля>.log`, то самое имя, которым мы САМИ владеем:
//! мы придумали его, мы кладём под ним файл `.ovpn`
//! (`openvpn::install_profile`), мы передаём его в
//! `--command connect|disconnect` (`openvpn::connect`/`disconnect`). Это
//! не альтернативная догадка вместо адаптера — это единственный
//! идентификатор в этой цепочке, которым распоряжаемся мы, а не Windows и
//! не драйвер VPN.
//!
//! # Честные пределы
//!
//! Каталог лога читается из `HKCU\Software\OpenVPN-GUI\log_dir`, а без
//! этого значения — берётся задокументированный дефолт
//! `%USERPROFILE%\OpenVPN\log`. Маркеры («Initialization Sequence
//! Completed» и строки о выходе процесса) — стабильные строковые литералы
//! из исходников самого OpenVPN, не специфичные для какой-либо
//! инсталляции. Но: если процесс убит без штатного выхода (обрыв питания,
//! `taskkill /F`, сон машины в неудачный момент), лог не допишет маркер
//! остановки, и эта функция в одиночку продолжила бы выдавать `Up` дольше,
//! чем это правда — этот модуль сам по себе такое отличить не может.
//!
//! **Round 2 задачи 7 закрыл эту дыру НЕ здесь, а у вызывающего.**
//! `settings_page::Tunnel::snapshot` (`crates/app`) берёт `Up`/`Down`
//! отсюда И `tunnel_state::any_tunnel_carries` (несёт ли хоть один
//! туннельный адаптер наши офисные маршруты) и требует ОБОИХ разом:
//! если процесс убит без выхода, лог продолжает врать «поднято», но
//! маршруты уходят вместе с процессом почти сразу — и `any_tunnel_carries`
//! становится `false`, гася ложный «поднято» ещё до того, как об этом
//! узнал бы человек. Один этот модуль такого знать не может (он не видит
//! таблицу маршрутов), поэтому объединение — не опциональное усиление, а
//! обязательная вторая половина решения.

use std::path::{Path, PathBuf};

use windows::core::{w, PCWSTR};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, KEY_READ};

use crate::sysproxy::RegKey;
use crate::WinNetError;

const GUI_KEY: PCWSTR = w!("Software\\OpenVPN-GUI");
const LOG_DIR_VALUE: PCWSTR = w!("log_dir");

/// Строка, которую OpenVPN печатает ровно один раз за успешно
/// установленное соединение — стабильна в исходниках OpenVPN уже больше
/// десяти лет, на неё полагаются сторонние скрипты мониторинга повсюду.
const UP_MARKER: &str = "Initialization Sequence Completed";

/// Строки, которыми OpenVPN сопровождает завершение или перезапуск
/// процесса (получен `SIGTERM`/`SIGINT`/`SIGHUP`) — тоже литералы из
/// исходников OpenVPN, не текст этой инсталляции. `restarting`
/// (мягкий SIGHUP) обрабатывается тем же маркером, что и жёсткий выход:
/// пока в логе после него нет НОВОГО `UP_MARKER`, честнее считать себя не
/// поднятыми, чем угадывать, что перезапуск уже успел закончиться.
const DOWN_MARKERS: [&str; 2] = ["received, process exiting", "received, process restarting"];

/// Живость нашего профиля по его собственному логу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelLiveness {
    /// Файла лога ещё нет — подключение этого профиля ни разу не
    /// пытались поднять (или профиль только что собран).
    NeverConnected,
    /// Файл есть, но последнее, что в нём видно, — это не «поднято»:
    /// либо `UP_MARKER` вовсе не встретился, либо после него нашёлся один
    /// из `DOWN_MARKERS`.
    Down,
    /// `UP_MARKER` — последнее содержательное событие: после него в логе
    /// нет ни одного маркера остановки/перезапуска.
    Up,
}

/// Разбирает уже прочитанный текст лога — чистая функция ради теста,
/// отдельно от чтения файла и реестра.
pub fn classify_log(text: &str) -> TunnelLiveness {
    let Some(up_at) = text.rfind(UP_MARKER) else {
        return TunnelLiveness::Down;
    };
    let after_up = &text[up_at..];
    if DOWN_MARKERS.iter().any(|m| after_up.contains(m)) {
        TunnelLiveness::Down
    } else {
        TunnelLiveness::Up
    }
}

/// Каталог, куда `openvpn-gui.exe` пишет логи GUI-подключений.
/// `HKCU\Software\OpenVPN-GUI\log_dir`, если он задан и непуст — GUI
/// умеет его переопределять; иначе задокументированный дефолт
/// `%USERPROFILE%\OpenVPN\log`. Отсутствие ключа или значения не отказ —
/// это как раз обычная (наблюдаемая на этой машине) конфигурация.
fn log_dir() -> PathBuf {
    let overridden = RegKey::open(HKEY_CURRENT_USER, GUI_KEY, KEY_READ)
        .ok()
        .and_then(|key| key.query_string(LOG_DIR_VALUE).ok())
        .filter(|v| !v.is_empty());
    match overridden {
        Some(dir) => PathBuf::from(dir),
        None => default_log_dir(),
    }
}

fn default_log_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\"))
        .join("OpenVPN")
        .join("log")
}

/// Путь к логу конкретного профиля. Отдельная функция ради теста:
/// собственно чтение файла подставляет чужой каталог, минуя реестр и
/// `%USERPROFILE%`.
///
/// **На заметку при выборе будущего имени профиля.** OpenVPN GUI сам
/// вырезает точки из имени профиля при выборе имени файла лога — профиль
/// `my.profile` пишет лог не в `my.profile.log`, а в `myprofile.log`. Эта
/// функция такого вырезания не делает — она просто дописывает `.log` к
/// тому, что получила. У `TUNNEL_PROFILE_NAME` (`settings_page.rs`,
/// «proxypilot-office») точек нет, так что здесь расхождения сейчас не
/// возникает, но если однажды имя профиля обзаведётся точкой — `log_path`
/// перестанет совпадать с тем, что реально пишет GUI, и `liveness` начнёт
/// молча возвращать `NeverConnected` вместо настоящего состояния.
fn log_path(dir: &Path, profile_name: &str) -> PathBuf {
    dir.join(format!("{profile_name}.log"))
}

/// Живость профиля `profile_name` по его логу. `Err` — лог существует, но
/// прочитать не удалось (права доступа, диск); отсутствие файла — не
/// ошибка, а [`TunnelLiveness::NeverConnected`].
pub fn liveness(profile_name: &str) -> Result<TunnelLiveness, WinNetError> {
    liveness_in(&log_dir(), profile_name)
}

/// То же самое, но с явным каталогом — тестовый вход, минующий реестр и
/// `%USERPROFILE%` этой машины.
fn liveness_in(dir: &Path, profile_name: &str) -> Result<TunnelLiveness, WinNetError> {
    let path = log_path(dir, profile_name);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(classify_log(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TunnelLiveness::NeverConnected),
        Err(source) => Err(WinNetError::TunnelLogRead { path, source }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_with_no_up_marker_is_down() {
        assert_eq!(classify_log(""), TunnelLiveness::Down);
        assert_eq!(classify_log("some unrelated line\n"), TunnelLiveness::Down);
    }

    #[test]
    fn a_log_ending_right_after_the_up_marker_is_up() {
        let log = format!("2026-08-29 22:45:46 client\n2026-08-29 22:45:58 {UP_MARKER}\n");
        assert_eq!(classify_log(&log), TunnelLiveness::Up);
    }

    #[test]
    fn a_log_that_exited_after_coming_up_is_down() {
        let log = format!(
            "2026-08-29 22:45:58 {UP_MARKER}\n2026-08-29 22:46:10 SIGTERM[hard,init] received, process exiting\n"
        );
        assert_eq!(classify_log(&log), TunnelLiveness::Down);
    }

    #[test]
    fn a_soft_restart_after_coming_up_is_down_until_a_new_up_marker() {
        // SIGHUP — мягкий перезапуск: пока после него нет НОВОГО
        // UP_MARKER, честнее Down, чем угадывать, что переподключение уже
        // завершилось.
        let log = format!(
            "2026-08-29 22:45:58 {UP_MARKER}\n2026-08-29 22:46:10 SIGHUP[soft,init] received, process restarting\n"
        );
        assert_eq!(classify_log(&log), TunnelLiveness::Down);
    }

    #[test]
    fn a_reconnect_after_a_restart_is_up_again() {
        // Тот же файл (усекается заново только на СЛЕДУЮЩЕЙ попытке
        // подключения, не на мягком перезапуске внутри одной) несёт два
        // цикла: down -> up -> restart -> up. Последнее содержательное
        // событие — второй UP_MARKER, после которого ничего нет.
        let log = format!(
            "2026-08-29 22:45:50 {UP_MARKER}\n\
             2026-08-29 22:45:55 SIGHUP[soft,init] received, process restarting\n\
             2026-08-29 22:46:02 {UP_MARKER}\n"
        );
        assert_eq!(classify_log(&log), TunnelLiveness::Up);
    }

    #[test]
    fn only_the_last_up_marker_counts() {
        // up -> exit -> (файл теоретически мог бы начать копиться дальше)
        // — если бы после последнего UP_MARKER всё же был down-маркер,
        // результат обязан остаться Down, не Up от более раннего цикла.
        let log = format!(
            "2026-08-29 20:00:00 {UP_MARKER}\n\
             2026-08-29 20:05:00 SIGTERM[hard,init] received, process exiting\n\
             2026-08-29 22:45:50 {UP_MARKER}\n\
             2026-08-29 22:46:10 SIGTERM[hard,init] received, process exiting\n"
        );
        assert_eq!(classify_log(&log), TunnelLiveness::Down);
    }

    #[test]
    fn a_missing_log_file_means_never_connected_not_an_error() {
        let dir = std::env::temp_dir().join("proxypilot-test-tunnel-log-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let got = liveness_in(&dir, "definitely-no-such-profile").expect("отсутствие — не ошибка");
        assert_eq!(got, TunnelLiveness::NeverConnected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_present_log_file_is_classified_from_its_contents() {
        let dir = std::env::temp_dir().join("proxypilot-test-tunnel-log-present");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            log_path(&dir, "proxypilot-office"),
            format!("2026-08-29 22:45:58 {UP_MARKER}\n"),
        )
        .unwrap();
        let got = liveness_in(&dir, "proxypilot-office").expect("файл обязан читаться");
        assert_eq!(got, TunnelLiveness::Up);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_profiles_do_not_see_each_others_log() {
        let dir = std::env::temp_dir().join("proxypilot-test-tunnel-log-scoped");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            log_path(&dir, "someone-elses-profile"),
            format!("2026-08-29 22:45:58 {UP_MARKER}\n"),
        )
        .unwrap();
        let got =
            liveness_in(&dir, "proxypilot-office").expect("отсутствие своего лога — не ошибка");
        assert_eq!(got, TunnelLiveness::NeverConnected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- Смоук на живой машине: только чтение ----
    // Имя профиля заведомо не существует ни у одного реального
    // подключения этой машины — не задевает чужие логи (`README.txt`
    // инсталляции подтверждает: лог живёт в `%USERPROFILE%\OpenVPN\log`,
    // а этого имени там нет и не будет).

    #[test]
    fn liveness_does_not_fail_on_a_real_machine_for_a_profile_that_was_never_connected() {
        let got = liveness("proxypilot-office-liveness-smoke-2c9f7e")
            .expect("отсутствующий лог — не ошибка, а NeverConnected");
        assert_eq!(got, TunnelLiveness::NeverConnected);
    }
}
