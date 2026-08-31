# Task 4 — Управление туннелем — отчёт

Branch: `feat/vpn-static-ip`, base HEAD: `3bc64f2`.

## Что сделано

`crates/winnet/src/openvpn.rs` дописан (не переписан) — `find_installation`,
`locate`, `open_key`, `read_registry_values` из задачи 1 не тронуты
структурно, изменены только три их doc-комментария, называвшие «Task 4»
как будущее место — теперь называют конкретные функции ниже (стало
неверно, что это ссылка вперёд; правило CLAUDE.md «комментарий, разошедшийся
с кодом, — дефект»).

```rust
pub fn install_profile(inst: &Installation, name: &str, contents: &str) -> Result<PathBuf, WinNetError>
pub fn build_and_install_profile(inst: &Installation, name: &str, source: &str, routes: &[Ipv4Net]) -> Result<PathBuf, WinNetError>
pub fn connect(inst: &Installation, name: &str) -> Result<(), WinNetError>
pub fn disconnect(inst: &Installation, name: &str) -> Result<(), WinNetError>
pub fn status(inst: &Installation, name: &str) -> Result<TunnelStatus, WinNetError>

pub enum TunnelStatus { NotInstalled, Installed }
```

`crates/winnet/src/lib.rs` — четыре новых варианта `WinNetError`:
`OpenVpnNotFound`, `OpenVpnGuiLaunch`, `ProfileWrite`, `Profile` (последний
— `#[from] ovpn_profile::ProfileError`).

### Отклонение от списка «Produces» в брифе

Бриф перечисляет четыре функции. Добавлена пятая, не заявленная в списке:
`build_and_install_profile(inst, name, source, routes)`. Причина —
инструкция задачи прямо говорит: «Task 2 — `build_profile`… Ты — её первый
вызывающий. Не проглатывай эту ошибку», а `install_profile(inst, name,
contents: &str)` в буквальном виде из брифа принимает уже готовый текст и
`build_profile` не вызывает вовсе. Без отдельной функции требование
«ты — первый вызывающий `build_profile`, ошибка не проглатывается» было бы
невыполнимо кодом этой задачи. `build_and_install_profile` — единственное
место в крейте, вызывающее `ovpn_profile::build_profile`; её `Result`
пробрасывается через `?` (`WinNetError::Profile`, `#[from]`), и при отказе
сборки на диск не пишется ничего — проверено тестом
(`build_and_install_profile_propagates_a_profile_error_without_writing_anything`).
`install_profile` в буквальной форме брифа тоже реализована — оба пути
делят один и тот же код записи на диск.

### Почему `connect`/`disconnect`/`status`/`install_profile` берут `&Installation`, а приёмка всё равно требует отказа «OpenVPN не найден»

`Installation` не гарантирует актуальность дольше одного вызова:
`find_installation` проверяла `gui_exe.is_file()` в момент поиска, но
OpenVPN мог быть удалён между тем моментом и вызовом любой из четырёх
функций. Общая проверка `ensure_still_installed` (`gui_exe.is_file()`)
стоит первой строкой во всех четырёх — расходиться в ней означало бы, что
одни функции отказывают честно, а другие тихо пытаются писать/запускать
процесс по пути, которого больше нет. `WinNetError::OpenVpnNotFound` несёт
сам путь.

### Командная строка

`build_gui_command(inst, verb, name) -> std::process::Command` — чистая
функция, только конструирует `Command`, ничего не запускает. `connect` и
`disconnect` вызывают её и затем `.spawn()` (не `.status()`/`.output()`):
при уже запущенном GUI `--command` — почти мгновенный обмен через
именованный канал с интерактивной службой, а если GUI ещё не запущен, сам
процесс становится долгоживущим окном в трее — ждать его завершения
означало бы заблокироваться на неопределённое время. `Command::spawn`
вызывается ровно в одном месте всего модуля (`run_gui_command`); нигде в
тестах он не достигается — до него везде стоит `ensure_still_installed`, а
фикстуры для тестов на командную строку намеренно не проходят её
(см. TDD evidence и раздел «Что не выполнялось»).

### `install_profile`

Пишет ровно один файл — `<config_dir>/<name>.ovpn`; остальное содержимое
каталога не читается и не перечисляется. Каталог конфигураций создаётся
(`create_dir_all`), если его ещё нет — это не признак «OpenVPN не
установлен» (докблок `locate`, унаследованный от задачи 1: «конфигураций
пока нет» — законное состояние). `name` без расширения — то же самое
значение, что идёт в `--command connect|disconnect <name>` (спека 8.3):
один параметр на оба места вместо двух независимых имён, которые могли бы
разойтись.

### `status` и унаследованное ограничение задачи 3

`TunnelStatus` различает только «файл профиля на диске есть/нет» —
**не** «туннель поднят/опущен». Причина — не пробел в реализации, а
осознанная граница: у `openvpn-gui.exe` нет синхронного текстового запроса
состояния (`docs/design.md` §8.3 называет только `connect`/`disconnect`).
Единственный источник живого состояния — таблица маршрутов через
`tunnel_state::our_tunnel_up` (задача 3), а её собственный докблок прямо
предупреждает: псевдоним интерфейса Windows не устойчивый идентификатор,
пользователь может переименовать адаптер, и разные API форматируют его
alias по-разному. Смешивать факт «файл на диске есть» с предположением
«значит, поднято» значило бы выдавать одно за другое — `status` в этой
задаче сознательно этого не делает и говорит об этом в doc-комментарии
`TunnelStatus`, а не только здесь в отчёте.

## TDD evidence

### RED — полный вывод `cargo test -p proxypilot-winnet --lib`, тестовый код добавлен, реализация — нет

Настоящий прогон, не реконструкция: 14 новых тестов (все в
`openvpn::tests`) были дописаны первыми и ссылались на `install_profile`,
`build_and_install_profile`, `connect`, `disconnect`, `status`,
`TunnelStatus`, `build_gui_command`, `WinNetError::OpenVpnNotFound` и
`WinNetError::Profile` — ни один из которых ещё не существовал. Модуль
`openvpn.rs` уже существовал (задача 1), поэтому падение сразу дало
настоящие ошибки типов, а не «file not found for module»:

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
error[E0425]: cannot find function `build_gui_command` in this scope
   --> crates\winnet\src\openvpn.rs:439:19
    |
439 |         let cmd = build_gui_command(&inst, "connect", "proxypilot-office");
    |                   ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_gui_command` in this scope
   --> crates\winnet\src\openvpn.rs:449:19
    |
449 |         let cmd = build_gui_command(&inst, "disconnect", "proxypilot-office");
    |                   ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `connect` in this scope
   --> crates\winnet\src\openvpn.rs:460:19
    |
460 |         let err = connect(&inst, "proxypilot-office").expect_err("gui_exe отсутствует на диске");
    |                   ^^^^^^^ not found in this scope

error[E0599]: no variant named `OpenVpnNotFound` found for enum `WinNetError`
   --> crates\winnet\src\openvpn.rs:461:44
    |
461 |         assert!(matches!(err, WinNetError::OpenVpnNotFound { .. }));
    |                                            ^^^^^^^^^^^^^^^ variant not found in `WinNetError`
    |
   ::: crates\winnet\src\lib.rs:17:1
    |
 17 | pub enum WinNetError {
    | -------------------- variant `OpenVpnNotFound` not found here

error[E0425]: cannot find function `disconnect` in this scope
   --> crates\winnet\src\openvpn.rs:470:13
    |
470 |             disconnect(&inst, "proxypilot-office").expect_err("gui_exe отсутствует на диске");
    |             ^^^^^^^^^^ not found in this scope

error[E0599]: no variant named `OpenVpnNotFound` found for enum `WinNetError`
   --> crates\winnet\src\openvpn.rs:471:44
    |
471 |         assert!(matches!(err, WinNetError::OpenVpnNotFound { .. }));
    |                                            ^^^^^^^^^^^^^^^ variant not found in `WinNetError`
    |
   ::: crates\winnet\src\lib.rs:17:1
    |
 17 | pub enum WinNetError {
    | -------------------- variant `OpenVpnNotFound` not found here

error[E0425]: cannot find function `install_profile` in this scope
   --> crates\winnet\src\openvpn.rs:478:20
    |
478 |         let path = install_profile(&inst, "proxypilot-office", "client\ndev tun\n")
    |                    ^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `install_profile` in this scope
   --> crates\winnet\src\openvpn.rs:492:9
    |
492 |         install_profile(&inst, "proxypilot-office", "наш профиль\n")
    |         ^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `install_profile` in this scope
   --> crates\winnet\src\openvpn.rs:528:20
    |
528 |         let path = install_profile(&inst, "proxypilot-office", "содержимое профиля\n")
    |                    ^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `install_profile` in this scope
   --> crates\winnet\src\openvpn.rs:544:20
    |
544 |         let path = install_profile(&inst, "proxypilot-office", "x\n")
    |                    ^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `install_profile` in this scope
   --> crates\winnet\src\openvpn.rs:554:19
    |
554 |         let err = install_profile(&inst, "proxypilot-office", "x\n")
    |                   ^^^^^^^^^^^^^^^ not found in this scope

error[E0599]: no variant named `OpenVpnNotFound` found for enum `WinNetError`
   --> crates\winnet\src\openvpn.rs:556:44
    |
556 |         assert!(matches!(err, WinNetError::OpenVpnNotFound { .. }));
    |                                            ^^^^^^^^^^^^^^^ variant not found in `WinNetError`
    |
   ::: crates\winnet\src\lib.rs:17:1
    |
 17 | pub enum WinNetError {
    | -------------------- variant `OpenVpnNotFound` not found here

error[E0425]: cannot find function `status` in this scope
   --> crates\winnet\src\openvpn.rs:567:19
    |
567 |         let got = status(&inst, "proxypilot-office").expect("статус обязан читаться");
    |                   ^^^^^^ not found in this scope

error[E0433]: cannot find type `TunnelStatus` in this scope
   --> crates\winnet\src\openvpn.rs:568:25
    |
568 |         assert_eq!(got, TunnelStatus::NotInstalled);
    |                         ^^^^^^^^^^^^ use of undeclared type `TunnelStatus`

error[E0425]: cannot find function `install_profile` in this scope
   --> crates\winnet\src\openvpn.rs:575:9
    |
575 |         install_profile(&inst, "proxypilot-office", "x\n").unwrap();
    |         ^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `status` in this scope
   --> crates\winnet\src\openvpn.rs:576:19
    |
576 |         let got = status(&inst, "proxypilot-office").expect("статус обязан читаться");
    |                   ^^^^^^ not found in this scope

error[E0433]: cannot find type `TunnelStatus` in this scope
   --> crates\winnet\src\openvpn.rs:577:25
    |
577 |         assert_eq!(got, TunnelStatus::Installed);
    |                         ^^^^^^^^^^^^ use of undeclared type `TunnelStatus`

error[E0425]: cannot find function `status` in this scope
   --> crates\winnet\src\openvpn.rs:585:19
    |
585 |         let err = status(&inst, "proxypilot-office").expect_err("gui_exe отсутствует на диске");
    |                   ^^^^^^ not found in this scope

error[E0599]: no variant named `OpenVpnNotFound` found for enum `WinNetError`
   --> crates\winnet\src\openvpn.rs:586:44
    |
586 |         assert!(matches!(err, WinNetError::OpenVpnNotFound { .. }));
    |                                            ^^^^^^^^^^^^^^^ variant not found in `WinNetError`
    |
   ::: crates\winnet\src\lib.rs:17:1
    |
 17 | pub enum WinNetError {
    | -------------------- variant `OpenVpnNotFound` not found here

error[E0425]: cannot find function `build_and_install_profile` in this scope
   --> crates\winnet\src\openvpn.rs:601:20
    |
601 |         let path = build_and_install_profile(&inst, "proxypilot-office", source, &routes())
    |                    ^^^^^^^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_and_install_profile` in this scope
   --> crates\winnet\src\openvpn.rs:619:19
    |
619 |         let err = build_and_install_profile(&inst, "proxypilot-office", broken_source, &[])
    |                   ^^^^^^^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0599]: no variant, associated function, or constant named `Profile` found for enum `WinNetError` in the current scope
   --> crates\winnet\src\openvpn.rs:621:44
    |
621 |         assert!(matches!(err, WinNetError::Profile(_)));
    |                                            ^^^^^^^ variant, associated function, or constant not found in `WinNetError`
    |
   ::: crates\winnet\src\lib.rs:17:1
    |
 17 | pub enum WinNetError {
    | -------------------- variant, associated function, or constant `Profile` not found for this enum

Some errors have detailed explanations: E0425, E0433, E0599.
For more information about an error, try `rustc --explain E0425`.
error: could not compile `proxypilot-winnet` (lib test) due to 22 previous errors
```

### GREEN — `cargo test -p proxypilot-winnet --lib openvpn`, после реализации

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.80s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-09ac8735ffd742da.exe)

running 23 tests
test autostart::tests::points_at_matches_the_real_run_shape_of_openvpn_gui ... ok
test openvpn::tests::finding_the_real_installation_does_not_fail ... ok
test openvpn::tests::find_installation_reads_bin_dir_and_config_dir_into_the_right_slots ... ok
test openvpn::tests::locate_returns_none_when_the_registry_bin_dir_does_not_exist_on_disk ... ok
test openvpn::tests::open_key_is_none_for_a_subkey_that_does_not_exist ... ok
test openvpn::tests::connect_fails_clearly_when_openvpn_is_not_found ... ok
test openvpn::tests::locate_returns_none_when_the_registry_bin_dir_has_no_gui_exe ... ok
test openvpn::tests::status_fails_clearly_when_openvpn_is_not_found ... ok
test openvpn::tests::install_profile_fails_clearly_when_openvpn_is_not_found ... ok
test openvpn::tests::disconnect_fails_clearly_when_openvpn_is_not_found ... ok
test openvpn::tests::build_gui_command_for_disconnect_targets_our_profile_by_name ... ok
test openvpn::tests::build_gui_command_for_connect_targets_our_profile_by_name ... ok
test openvpn::tests::build_and_install_profile_propagates_a_profile_error_without_writing_anything ... ok
test openvpn::tests::locate_finds_installation_when_registry_bin_dir_has_the_gui_exe ... ok
test openvpn::tests::locate_falls_back_to_the_standard_bin_dir_when_the_registry_value_is_empty ... ok
test openvpn::tests::locate_falls_back_to_the_standard_config_dir_when_the_registry_value_is_empty ... ok
test openvpn::tests::install_profile_creates_the_config_dir_if_it_does_not_exist_yet ... ok
test openvpn::tests::status_reports_not_installed_when_the_profile_file_is_absent ... ok
test openvpn::tests::status_reports_installed_when_the_profile_file_is_present ... ok
test openvpn::tests::install_profile_does_not_touch_neighbouring_files ... ok
test openvpn::tests::build_and_install_profile_writes_the_built_profile ... ok
test openvpn::tests::install_profile_writes_under_our_own_name ... ok
test openvpn::tests::install_profile_round_trips_a_config_dir_with_spaces ... ok

test result: ok. 23 passed; 0 failed; 0 measured; 0 filtered out; finished in 0.03s
```

Прогон с фильтром `openvpn` матчит 23 теста, не только новые: 14 новых
(список в предыдущем абзаце) плюс 9 уже существовавших — 8 из
`openvpn::tests` (задача 1: `find_installation_reads_bin_dir_and_config_dir_into_the_right_slots`,
`finding_the_real_installation_does_not_fail`,
`locate_returns_none_when_the_registry_bin_dir_does_not_exist_on_disk`,
`open_key_is_none_for_a_subkey_that_does_not_exist`,
`locate_returns_none_when_the_registry_bin_dir_has_no_gui_exe`,
`locate_finds_installation_when_registry_bin_dir_has_the_gui_exe`,
`locate_falls_back_to_the_standard_bin_dir_when_the_registry_value_is_empty`,
`locate_falls_back_to_the_standard_config_dir_when_the_registry_value_is_empty`)
и одна `autostart::tests::points_at_matches_the_real_run_shape_of_openvpn_gui`
(имя содержит подстроку `openvpn`, к этой задаче не относится). Все 23 —
зелёные; ни одна из 9 старых не редактировалась.

## Три команды CI — полный вывод, после форматирования

### `cargo test --all`

Помимо 14 новых тестов этой задачи (`openvpn::tests::*`) прогнаны все
тесты крейтов `app`, `bridge`, `core`, `winnet` без единого изменения в
них. Полная сводка по крейтам:

```
     Running unittests src\main.rs (target\debug\deps\proxypilot-cbf9a0a06eececc8.exe)
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.61s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)
test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-b4606ab8698a901a.exe)
test result: ok. 133 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s

   Doc-tests proxypilot_bridge
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_winnet
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Итого: **377 passed, 0 failed, 3 ignored** (было 363 + 3 ignored перед
задачей — `progress.md`, Task 3 complete). Прирост 377 − 363 = 14, ровно
столько новых `#[test]` эта задача добавила (проверено отдельно:
`git diff crates/winnet/src/openvpn.rs | grep -c '#\[test\]'` → 14; список
— в предыдущем разделе). Никакой из прежних 363 тестов не менялся и не
удалялся.

Три ignored-теста (`win_autostart_set_round_trips_through_the_real_registry`,
`autostart::tests::enable_then_disable_round_trip_on_the_real_registry`,
`events::tests::watch_a_real_network_change`) не имеют отношения к этой
задаче и не запускались.

### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.17s
```

Чисто, без единого предупреждения.

### `cargo fmt --all --check`

Первый прогон нашёл одно расхождение (перенос строки в
`disconnect_fails_clearly_when_openvpn_is_not_found`), поправлено
`cargo fmt --all`. Повторный `--check`:

```
(без вывода, exit code 0)
```

## Проверено вручную по границам задачи

- `grep -n "#\[allow" crates/winnet/src/openvpn.rs crates/winnet/src/lib.rs`
  — пусто, `#[allow(...)]` не добавлялся.
- `grep -n "unsafe" crates/winnet/src/openvpn.rs` — три вхождения, все
  в тестовой песочнице реестра задачи 1 (`ScratchKey`), ни одного нового в
  коде этой задачи. Задача 4 не добавила ни одного `unsafe`-блока — вся
  работа через `std::fs`/`std::process`.
- `grep -n "\.spawn()\|Command::new\|\.status()\|\.output()"
  crates/winnet/src/openvpn.rs` — ровно одна пара: `Command::new` в
  `build_gui_command`, `.spawn()` в `run_gui_command`. Других мест запуска
  процессов нет.
- Ни одной записи в `HKLM` или `HKCU` эта задача не добавила — весь код
  работает с файловой системой (`std::fs`) и одним дочерним процессом.
- После полного прогона тестов каталог `%TEMP%` не содержит осиротевших
  `proxypilot-test-openvpn-*` — каждый тест убирает за собой (`cleanup`/
  `remove_dir_all`), проверено `ls $TEMP | grep proxypilot-test-openvpn`
  → 0 совпадений.

## Что НЕ выполнялось на этой машине

Ни разу за всю задачу: `openvpn-gui.exe --command connect`,
`openvpn-gui.exe --command disconnect`, любой другой запуск
`openvpn-gui.exe` (в том числе `--help` или проверка версии — не
понадобилось: `Command::spawn` в тестах не достигается вовсе, до него
везде стоит проверка `ensure_still_installed`, которая отказывает раньше).
Ни один тест не подключался к реальной установленной на этой машине копии
OpenVPN и не писал в её настоящий каталог `config` — все файловые тесты
работают на временных каталогах, созданных и удалённых самим тестом.
Изменения в реестр (`HKLM`/`HKCU`) не вносились.

## Ручная проверка — вынесена человеку

Контроллер сессии прямо запрещает live-прогон `openvpn-gui.exe --command
connect|disconnect` на этой же машине (CLAUDE.md, «Живые проверки, которые
не делает агент»; см. также прошлый рулинг в `progress.md`). Человек
должен проверить руками, в этом порядке:

1. Поднять туннель из приложения (после того как задачи 5-7 подключат
   `install_profile`/`connect` к UI) — убедиться, что `openvpn-gui.exe`
   показывает подключённый статус для профиля `proxypilot-office`.
2. Убедиться, что внутренний офисный ресурс (например, git или
   dev-сервер) стал доступен.
3. Сравнить внешний IP до и после подъёма туннеля — он обязан остаться
   домашним (split-tunnel, не full-tunnel): проверка через любой сервис
   «какой у меня IP» до и после.
4. Опустить туннель и убедиться, что офисный ресурс снова недоступен, а
   остальной трафик всё это время не прерывался.

## Файлы

- `crates/winnet/src/openvpn.rs` — дописан: пять публичных функций,
  `TunnelStatus`, 14 новых тестов. Три doc-комментария, ссылавшиеся на
  «Task 4» как на будущее, поправлены на конкретные имена.
- `crates/winnet/src/lib.rs` — четыре новых варианта `WinNetError`.
