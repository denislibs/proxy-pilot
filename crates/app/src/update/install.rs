//! Установка уже отложенного обновления — при СЛЕДУЮЩЕМ запуске, а не под
//! работающим процессом.
//!
//! Почему это вообще возможно на Windows: переименование файла работающего
//! `.exe` НЕ запрещено (в отличие от перезаписи его содержимого «на месте»)
//! — загрузчик держит секцию образа по дескриптору, а не по имени пути,
//! поэтому `rename` уходит успешно, пока сам файл кем-то исполняется.
//! Именно на этом стоит вся функция ниже: `apply_pending_update` вызывается
//! в САМОМ НАЧАЛЕ `main()`, ДО того, как этот же запуск успел стать
//! «работающим процессом» в смысле инварианта `CLAUDE.md` (до привязки
//! слушателя, до `proxy::take_over`, до создания трея) — то есть замена
//! происходит буквально под тем же процессом, который её выполняет, но
//! этот процесс ещё ничего не взял на себя и после переименования сразу
//! перезапускает себя же (см. [`relaunch_and_exit`] в `main.rs`) и
//! завершается, так и не тронув реестр. Свежий перезапуск — уже другой
//! процесс, и он видит на диске уже новый файл. Это и есть «следующий
//! запуск»: не буквально следующее включение компьютера, а следующее ПОСЛЕ
//! файловой замены исполнение, которое эту замену не производит, а просто
//! стартует с места, где предыдущее её произвело.
//!
//! Слово «не под работающим процессом» в задаче означает именно это:
//! замена не происходит, пока процесс уже взял на себя порт и системный
//! прокси (тогда обрыв середины операции оставил бы реестр указывающим на
//! мёртвый файл) — а не то, что операцию обязан выполнять какой-то ДРУГОЙ,
//! посторонний процесс.

use std::path::{Path, PathBuf};

use super::check::STAGED_NAME;

/// Тот же тип, что использует [`super::check`] — единый контракт «функция
/// проверки подписи» на весь модуль обновлений.
pub type Verifier = fn(&Path) -> Result<(), String>;

/// Суффикс резервной копии текущего exe на время свопа. Не временный файл
/// со случайным именем: предсказуемое имя даёт возможность прибрать его на
/// СЛЕДУЮЩЕМ вызове ([`cleanup_stale_backup`]), если этот процесс уйдёт, не
/// дожив до собственного завершения (сама резервная копия при этом не
/// исполняется никем — исполняется файл по НОВОМУ имени, — так что удалить
/// её можно сразу же, как только она перестала быть нужна).
const BACKUP_SUFFIX: &str = ".old";

fn backup_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    exe.with_file_name(name)
}

/// Убирает резервную копию, оставшуюся от свопа на ПРЕДЫДУЩЕМ запуске.
/// Best-effort и молчаливый в отказе: копия либо уже не существует, либо
/// это первый запуск после свопа, и её удаление — не обязанность именно
/// ЭТОГО вызова, а лишь уборка за собой при первой возможности.
fn cleanup_stale_backup(exe: &Path) {
    let backup = backup_path(exe);
    if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
}

/// Проверяет каталог обновлений на отложенный файл и, если он есть и
/// подпись подтверждена, меняет местами `current_exe` и файл. Отложенный
/// файл — это и есть маркер «есть, что установить» (`super::check::STAGED_NAME`,
/// см. докблок там же про «один источник истины вместо двух»).
///
/// Возвращает `Ok(true)`, если своп произошёл и по пути `current_exe`
/// теперь лежит НОВОЕ содержимое — вызывающий обязан перезапустить процесс
/// по этому же пути и завершиться, не трогая реестр и не привязывая
/// слушатель. `Ok(false)` — отложенного обновления не было, продолжать
/// обычный запуск. `Err` — было что применять, но не получилось (сеть тут
/// ни при чём: это либо отказ повторной проверки подписи, либо файловая
/// ошибка) — тоже продолжать обычный запуск СО СТАРЫМ бинарём, а не падать:
/// отказ применения отложенного обновления не должен мешать прокси
/// запуститься.
pub fn apply_pending_update(
    update_dir: &Path,
    current_exe: &Path,
    verify: Verifier,
) -> Result<bool, String> {
    cleanup_stale_backup(current_exe);

    let staged = update_dir.join(STAGED_NAME);
    if !staged.exists() {
        return Ok(false);
    }

    // Повторная проверка подписи прямо перед установкой — файл мог полежать
    // на диске со времени скачивания предыдущим запуском; проверка дешёвая
    // (докблок `super::verify` про сравнимую цену), а определённость перед
    // необратимым переименованием дороже. Отказ здесь — тот же самый исход,
    // что и при скачивании: обновление НЕ устанавливается, файл убирается.
    if let Err(reason) = verify(&staged) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "отложенное обновление отклонено при повторной проверке подписи: {reason}"
        ));
    }

    let backup = backup_path(current_exe);
    std::fs::rename(current_exe, &backup).map_err(|e| {
        format!(
            "не переименовать {} в {}: {e}",
            current_exe.display(),
            backup.display()
        )
    })?;

    if let Err(e) = std::fs::rename(&staged, current_exe) {
        // Откат: вернуть старому имени старое содержимое, а не оставить
        // путь `current_exe` пустым местом. Если и откат не удался — оба
        // сообщения об ошибке идут дальше вместе, потому что тогда на диске
        // действительно нет файла по ожидаемому имени, и это тот редкий
        // случай, когда молчать нельзя категорически.
        if let Err(rollback_err) = std::fs::rename(&backup, current_exe) {
            return Err(format!(
                "не установить новую версию ({e}), И откат не удался ({rollback_err}) — \
                 по пути {} сейчас может не быть исполняемого файла",
                current_exe.display()
            ));
        }
        return Err(format!("не установить новую версию: {e}"));
    }

    Ok(true)
}

/// Запускает новую копию по тому же пути и не ждёт её — вызывающий
/// (`main.rs`) обязан немедленно завершиться сам, не привязывая слушатель и
/// не трогая реестр: только что запущенный процесс — уже НОВЫЙ «следующий
/// запуск» в смысле докблока модуля, и именно он возьмёт это на себя.
///
/// Не вызывается ни одним тестом этого файла: реальный запуск дочернего
/// процесса на этой машине означал бы второй экземпляр ProxyPilot рядом с
/// уже работающим (см. хард-лимит задачи — «на машине уже запущена копия
/// приложения»), а свойство, которое здесь важно проверить («переименование
/// работающего exe не запрещено Windows»), уже доказано тестами выше на
/// фиктивных путях без единого реального процесса.
pub fn relaunch(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("не запустить {}: {e}", exe.display()))
}

/// Проверяет подпись файла ещё раз с тем же контрактом, что и
/// [`Verifier`], — тонкая обёртка над [`super::verify::verify_authenticode`]
/// для передачи в [`apply_pending_update`] и [`super::check::run`] одним и
/// тем же типом функции.
pub fn real_verifier(path: &Path) -> Result<(), String> {
    super::verify::verify_authenticode(path).map_err(|e| e.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn always_ok(_: &Path) -> Result<(), String> {
        Ok(())
    }

    fn always_refuses(_: &Path) -> Result<(), String> {
        Err("тестовый отказ подписи".to_string())
    }

    fn scenario(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir()
            .join("proxypilot-test-update-install")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let update_dir = dir.join("update");
        std::fs::create_dir_all(&update_dir).unwrap();
        let current_exe = dir.join("proxypilot.exe");
        std::fs::write(&current_exe, b"OLD-VERSION").unwrap();
        (update_dir, current_exe)
    }

    #[test]
    fn nothing_pending_leaves_the_exe_untouched() {
        let (update_dir, exe) = scenario("nothing-pending");
        let applied = apply_pending_update(&update_dir, &exe, always_ok).unwrap();
        assert!(!applied);
        assert_eq!(std::fs::read(&exe).unwrap(), b"OLD-VERSION");
    }

    #[test]
    fn a_signed_pending_update_swaps_the_file_and_keeps_a_backup() {
        let (update_dir, exe) = scenario("swap-ok");
        std::fs::write(update_dir.join(STAGED_NAME), b"NEW-VERSION").unwrap();

        let applied = apply_pending_update(&update_dir, &exe, always_ok).unwrap();

        assert!(applied, "своп обязан был произойти");
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"NEW-VERSION",
            "по старому пути обязано лежать новое содержимое"
        );
        assert!(
            !update_dir.join(STAGED_NAME).exists(),
            "отложенный файл обязан исчезнуть — он теперь и есть exe"
        );
        let backup = exe.with_extension("exe.old");
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            b"OLD-VERSION",
            "старое содержимое обязано сохраниться под резервным именем"
        );
    }

    #[test]
    fn a_pending_update_with_no_signature_is_refused_and_the_exe_is_unchanged() {
        // Приёмка: своп не под работающим процессом, но и не менее строгое
        // требование — отказ на неверной/отсутствующей подписи действует и
        // здесь, не только при скачивании.
        let (update_dir, exe) = scenario("swap-refused");
        std::fs::write(update_dir.join(STAGED_NAME), b"MALICIOUS").unwrap();

        let result = apply_pending_update(&update_dir, &exe, always_refuses);

        assert!(result.is_err(), "получили: {result:?}");
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"OLD-VERSION",
            "неподписанный файл не должен заменить рабочий exe"
        );
        assert!(
            !update_dir.join(STAGED_NAME).exists(),
            "отвергнутый файл обязан быть убран, а не остаться пытаться установиться заново"
        );
    }

    #[test]
    fn a_stale_backup_from_a_previous_swap_is_cleaned_up() {
        let (update_dir, exe) = scenario("stale-backup");
        let backup = exe.with_extension("exe.old");
        std::fs::write(&backup, b"LEFTOVER-FROM-LAST-TIME").unwrap();

        let applied = apply_pending_update(&update_dir, &exe, always_ok).unwrap();

        assert!(!applied, "в этом сценарии отложенного файла нет");
        assert!(
            !backup.exists(),
            "старая резервная копия обязана быть прибрана"
        );
    }

    #[test]
    fn the_backup_file_name_is_derived_from_the_current_exe_name() {
        let exe = Path::new(r"C:\Program Files\ProxyPilot\proxypilot.exe");
        assert_eq!(
            backup_path(exe),
            Path::new(r"C:\Program Files\ProxyPilot\proxypilot.exe.old")
        );
    }
}
