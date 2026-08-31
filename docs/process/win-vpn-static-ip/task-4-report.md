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

(Переименовано в fix round 1 ниже: `TunnelStatus` → `ProfileStatus`,
`status` → `profile_status` — само название вводило в заблуждение сильнее,
чем помогал докблок.)

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

## Fix round 1 (approved with fixes, commit поверх `e968d52`)

Ревью: без Critical, оба заявленных отступления от брифа приняты по
существу (изменённое требование к `install_profile`/`build_profile` — не
моя ошибка, а неисполнимая формулировка брифа; `TunnelStatus` как
разделение труда с `tunnel_state` — архитектурно верно, но неверно
названо). Семь находок, ни одной Critical: три Important, три Minor,
один пункт остаётся на стороне контроллера. Все закрыты одним коммитом
поверх `e968d52`.

1. **Important — переименование `TunnelStatus` → `ProfileStatus`,
   `status` → `profile_status`** (`openvpn.rs`). Причина ревью: тип и
   функция называли себя «туннель», отвечая на вопрос «есть ли файл» —
   `status(...).is_ok() == Installed` читалось бы как «подключено» тем,
   кто не открыл докблок. Варианты (`NotInstalled`/`Installed`) не
   менялись, только имена типа, функции и всех вызовов/тестов. Докблок
   `ProfileStatus` теперь прямо объясняет, почему имя не «Tunnel»:
   «Тип, названный «TunnelStatus», рядом с вариантом `Installed` выглядел
   бы как «туннель поднят» для того, кто читает только сигнатуру, а не
   докблок, — отсюда `ProfileStatus`».
2. **Important — докблок `connect`/`disconnect` теперь прямо говорит, что
   `Ok` означает «команда доставлена GUI», не «туннель поднят».** Раньше
   первая строка докблока `connect` («Поднимает наш туннель») читалась
   как обещание результата; сам факт асинхронности был виден только в
   докблоке `run_gui_command` и в `WinNetError::OpenVpnGuiLaunch` в
   `lib.rs`, а не в контракте самой публичной функции, на которую будет
   смотреть задача 7.
3. **Important — `cleanup` в тестах теперь проверяет `starts_with(std::env::temp_dir())`
   перед `remove_dir_all`.** Раньше функция удаляла `inst.config_dir` и
   родителя `inst.gui_exe` без всякой проверки — безопасно только потому,
   что все текущие вызовы передают временную фикстуру. Один copy-paste на
   настоящий `Installation` (например, при отладке живого сценария) снёс
   бы каталог `bin` установленного на машине OpenVPN. Теперь при выходе
   пути за `%TEMP%` `cleanup` — не ошибка, а тихий no-op, что для тестовой
   уборки безопаснее, чем падение.
4. **Minor — докблок `install_profile` теперь называет
   `build_and_install_profile` рекомендуемой точкой входа** и объясняет,
   что `install_profile` не проверяет происхождение `contents`: вызывающий
   код, собравший текст профиля в обход `ovpn_profile::build_profile`
   (например, конкатенацией строк), откроет второй путь мимо отказа на
   структурно битом источнике, и `install_profile` этого не поймает.
5. **Minor — добавлен тест `build_gui_command_survives_a_program_path_with_spaces`**:
   строит `Installation` с `gui_exe` под `...\Program Files\OpenVPN\bin\
   openvpn-gui.exe` (пробел в компоненте пути — обычное место установки)
   и проверяет, что `build_gui_command` передаёт путь и аргументы
   раздельно, не конкатенацией строк, где пробел развалил бы разбор.
6. **Minor — добавлен тест `install_profile_overwrites_an_existing_file_under_our_own_name`**:
   две последовательные записи под одним именем, вторая обязана заменить
   содержимое первой. Задача 5 перестраивает и перезаписывает профиль при
   каждой смене списка офисных подсетей — перезапись обычный ход дел,
   не редкий край.
7. **Minor, на стороне контроллера — ничего не менялось в этой задаче.**
   Ничто пока не определяет **наш** псевдоним интерфейса (`our_alias` в
   терминах `tunnel_state::our_tunnel_up`/`foreign_tunnel_up`, задача 3):
   этот параметр появится только вместе с тем, что реально поднимает
   адаптер (профиль OpenVPN, задача 5-7), и до того момента
   `our_tunnel_up`/`foreign_tunnel_up` физически нечем вызвать с верным
   значением. Контроллер зафиксировал это как открытый пункт для брифа
   задачи 7, не для этой задачи.

### Переименование по крейту — проверено

`grep -rn "TunnelStatus\b" crates/` до правки находил использования
только в `openvpn.rs`/`lib.rs` — переименование не задело код за
пределами этой задачи (задачи 5-7 ещё не написаны и на `TunnelStatus`
не ссылались).

### Три команды CI после исправлений — полный вывод

`cargo test --all`, по крейтам:

```
     Running unittests src\main.rs (target\debug\deps\proxypilot-cbf9a0a06eececc8.exe)
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.58s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)
test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-b4606ab8698a901a.exe)
test result: ok. 135 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s

   Doc-tests proxypilot_bridge
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_winnet
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Итого: **379 passed, 0 failed, 3 ignored** (было 377 + 3 ignored; +2 —
находки 5 и 6 добавили по одному тесту каждая, находки 1-4 и 7 тестов не
добавляли). Целевой фильтр `cargo test -p proxypilot-winnet --lib openvpn`
даёт 25 тестов (было 23; +2), все зелёные, включая переименованные
`profile_status_reports_not_installed_when_the_profile_file_is_absent`,
`profile_status_reports_installed_when_the_profile_file_is_present`,
`profile_status_fails_clearly_when_openvpn_is_not_found`.

`cargo clippy --all-targets -- -D warnings`:

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.85s
```

Чисто.

`cargo fmt --all --check` — без вывода, exit code 0.

### Границы задачи — проверено заново после правок

- `grep -n "#\[allow" crates/winnet/src/openvpn.rs crates/winnet/src/lib.rs`
  — по-прежнему пусто.
- `grep -n "\.spawn()\|Command::new\|\.status()\|\.output()"
  crates/winnet/src/openvpn.rs` — по-прежнему ровно одна пара
  (`build_gui_command`/`run_gui_command`), без изменений с прошлого
  раунда.
- `%TEMP%` после полного прогона тестов не содержит осиротевших
  `proxypilot-test-openvpn-*`.
- Ни `openvpn-gui.exe`, ни живой `connect`/`disconnect`, ни запись в
  реестр (`HKLM`/`HKCU`) в этом раунде не выполнялись — ровно как и в
  первом.

## Fix round 2 (найдено инспекцией живой машины, поверх `6b2920b`)

### Баг

`install_profile`/`build_and_install_profile` писали профиль в
`Installation::config_dir` — тот самый каталог, что `find_installation`
берёт из `HKLM\SOFTWARE\OpenVPN\config_dir`. На обычной установке это
каталог под `Program Files`, доступный на запись только администратору и
`TrustedInstaller`; обычный пользователь получает туда лишь чтение. На
живой машине, где велась эта сессия, это подтвердилось напрямую: попытка
записать пробный файл в этот каталог отказала access denied, тогда как
`%USERPROFILE%\OpenVPN\config` — принял запись без вопросов. Итог: нажатие
«Собрать профиль» отказывало access denied, и вся фича туннеля была
недостижима с обычными правами пользователя.

Хуже того, докблок `settings_page::Tunnel::build_profile` утверждал
обратное: «прав администратора не требует: каталог конфигураций OpenVPN
доступен на запись обычному пользователю (иначе окно OpenVPN GUI,
рассчитанное на запуск без UAC, не смогло бы сохранять профили)». Рассуждение
звучало правдоподобно и было неверным по факту: OpenVPN GUI действительно
сохраняет профили без UAC, но не в системный каталог, а в
`%USERPROFILE%\OpenVPN\config` — и показывает профили из ОБОИХ каталогов
разом, не только из системного. Тот же разрыв затрагивал логи — но там,
в отличие от каталога конфигураций, код уже был правильным:
`winnet::tunnel_log` с самого начала читает `%USERPROFILE%\OpenVPN\log`
(fix round 1 задачи 7), и на живой связке живого туннеля это подтвердилось
ещё раз: `liveness` вернула `Up` для реально поднятого профиля, `Down` для
устаревшего лога и `NeverConnected` для нашего собственного. `tunnel_log.rs`
в этом раунде не менялся — только докблок `log_path` дополнен заметкой
(ниже).

### Правка

`crates/winnet/src/openvpn.rs`:

- `Installation` получила третье поле — `user_config_dir`
  (`%USERPROFILE%\OpenVPN\config`). `config_dir` (системный, из реестра) —
  остался, но его роль сузилась до ЧТЕНИЯ исходного `.ovpn`, который мог
  заранее положить туда администратор.
- `find_installation` резолвит `user_config_dir` через новую
  `resolve_user_config_dir(Option<PathBuf>)` — чистую функцию, принимающую
  уже прочитанный (или отсутствующий) `%USERPROFILE%`, а не читающую
  окружение процесса сама (та же причина, что и у `program_files_dir` —
  тестируемость без мутации глобального состояния). Если установка OpenVPN
  найдена, а `%USERPROFILE%` не задан — `find_installation` возвращает
  `Err(WinNetError::UserProfileNotFound)`, а не молча подставляет
  `config_dir`, что гарантированно кончилось бы тем же access denied чуть
  позже. Машину без OpenVPN эта проверка не задевает: резолв происходит
  только после того, как установка уже найдена.
- `locate` (чистая функция, тестируется без реестра) не резолвит
  `user_config_dir` вовсе — возвращает промежуточный `SystemPaths { gui_exe,
  config_dir }`, из которого `find_installation` достраивает полный
  `Installation`. Так `locate` остаётся testable ровно теми же аргументами,
  что и раньше.
- `install_profile` пишет в `inst.user_config_dir`, создавая его при
  отсутствии — `inst.config_dir` эта функция больше не трогает вовсе (ни на
  чтение, ни на запись, ни на создание).
- Новая `find_config_file(inst, filename)` ищет файл в обоих каталогах,
  предпочитая пользовательский (обычный пользователь способен положить файл
  только туда — в `config_dir` он может только читать). Используется
  вызывающим кодом (`crates/app/src/main.rs`) для поиска исходного `.ovpn`.

`crates/winnet/src/lib.rs` — новый вариант `WinNetError::UserProfileNotFound`.

`crates/app/src/main.rs` (`WinTunnel::build_profile`) — путь к исходному
`.ovpn` теперь ищется через `openvpn::find_config_file` в обоих каталогах,
а не жёстко в `inst.config_dir`; докблок `TUNNEL_SOURCE_FILE` переписан под
это.

`crates/app/src/settings_page.rs` — докблок `Tunnel::build_profile`
исправлен: явно называет `user_config_dir` местом записи и прямо
опровергает прежнее (неверное) утверждение про доступность на запись
системного каталога, вместо того чтобы просто убрать неверную фразу молча.

`crates/winnet/src/tunnel_log.rs` — код не менялся; в докблок `log_path`
добавлена заметка на будущее: OpenVPN GUI вырезает точки из имени профиля
при выборе имени файла лога, так что профиль с точкой в имени писал бы лог
под именем без неё. `TUNNEL_PROFILE_NAME` точек не содержит, поэтому сейчас
это не расхождение — только предупреждение для будущего выбора имени.

Никаких `#[allow(...)]` не добавлено, новых `unsafe`-блоков нет.

### TDD — RED, полный вывод `cargo test --all` до правки (тестовый код

уже переписан под `user_config_dir`/`find_config_file`/
`resolve_user_config_dir`, реализация — ещё нет):

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
   Compiling proxypilot-netsvc v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\netsvc)
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
error[E0560]: struct `openvpn::Installation` has no field named `user_config_dir`
   --> crates\winnet\src\openvpn.rs:600:13
    |
600 |             user_config_dir,
    |             ^^^^^^^^^^^^^^^ `openvpn::Installation` does not have this field
    |
    = note: all struct fields are already assigned

error[E0560]: struct `openvpn::Installation` has no field named `user_config_dir`
   --> crates\winnet\src\openvpn.rs:618:13
    |
618 |             user_config_dir,
    |             ^^^^^^^^^^^^^^^ `openvpn::Installation` does not have this field
    |
    = note: all struct fields are already assigned

error[E0609]: no field `user_config_dir` on type `&openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:637:29
    |
637 |         if is_scratch(&inst.user_config_dir) {
    |                             ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
637 -         if is_scratch(&inst.user_config_dir) {
637 +         if is_scratch(&inst.config_dir) {
    |

error[E0609]: no field `user_config_dir` on type `&openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:638:46
    |
638 |             let _ = fs::remove_dir_all(&inst.user_config_dir);
    |                                              ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
638 -             let _ = fs::remove_dir_all(&inst.user_config_dir);
638 +             let _ = fs::remove_dir_all(&inst.config_dir);
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:713:31
    |
713 |         assert_eq!(path, inst.user_config_dir.join("proxypilot-office.ovpn"));
    |                               ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
713 -         assert_eq!(path, inst.user_config_dir.join("proxypilot-office.ovpn"));
713 +         assert_eq!(path, inst.config_dir.join("proxypilot-office.ovpn"));
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:751:34
    |
751 |         fs::create_dir_all(&inst.user_config_dir).unwrap();
    |                                  ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
751 -         fs::create_dir_all(&inst.user_config_dir).unwrap();
751 +         fs::create_dir_all(&inst.config_dir).unwrap();
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:752:30
    |
752 |         let neighbour = inst.user_config_dir.join("my-existing-work-profile.ovpn");
    |                              ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
752 -         let neighbour = inst.user_config_dir.join("my-existing-work-profile.ovpn");
752 +         let neighbour = inst.config_dir.join("my-existing-work-profile.ovpn");
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:762:52
    |
762 |         let mut names: Vec<_> = fs::read_dir(&inst.user_config_dir)
    |                                                    ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
762 -         let mut names: Vec<_> = fs::read_dir(&inst.user_config_dir)
762 +         let mut names: Vec<_> = fs::read_dir(&inst.config_dir)
    |

error[E0560]: struct `openvpn::Installation` has no field named `user_config_dir`
   --> crates\winnet\src\openvpn.rs:806:13
    |
806 |             user_config_dir: user_config_dir.clone(),
    |             ^^^^^^^^^^^^^^^ `openvpn::Installation` does not have this field
    |
    = note: all struct fields are already assigned

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:823:34
    |
823 |         fs::remove_dir_all(&inst.user_config_dir).unwrap();
    |                                  ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
823 -         fs::remove_dir_all(&inst.user_config_dir).unwrap();
823 +         fs::remove_dir_all(&inst.config_dir).unwrap();
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:824:23
    |
824 |         assert!(!inst.user_config_dir.exists());
    |                       ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
824 -         assert!(!inst.user_config_dir.exists());
824 +         assert!(!inst.config_dir.exists());
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:839:19
    |
839 |             !inst.user_config_dir.join("proxypilot-office.ovpn").exists(),
    |                   ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
839 -             !inst.user_config_dir.join("proxypilot-office.ovpn").exists(),
839 +             !inst.config_dir.join("proxypilot-office.ovpn").exists(),
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:904:19
    |
904 |             !inst.user_config_dir.join("proxypilot-office.ovpn").exists(),
    |                   ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
904 -             !inst.user_config_dir.join("proxypilot-office.ovpn").exists(),
904 +             !inst.config_dir.join("proxypilot-office.ovpn").exists(),
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:919:34
    |
919 |         fs::create_dir_all(&inst.user_config_dir).unwrap();
    |                                  ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
919 -         fs::create_dir_all(&inst.user_config_dir).unwrap();
919 +         fs::create_dir_all(&inst.config_dir).unwrap();
    |

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:922:18
    |
922 |             inst.user_config_dir.join("source.ovpn"),
    |                  ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
922 -             inst.user_config_dir.join("source.ovpn"),
922 +             inst.config_dir.join("source.ovpn"),
    |

error[E0425]: cannot find function `find_config_file` in this scope
   --> crates\winnet\src\openvpn.rs:927:21
    |
927 |         let found = find_config_file(&inst, "source.ovpn").expect("файл есть в обоих местах");
    |                     ^^^^^^^^^^^^^^^^ not found in this scope

error[E0609]: no field `user_config_dir` on type `openvpn::Installation`
   --> crates\winnet\src\openvpn.rs:928:32
    |
928 |         assert_eq!(found, inst.user_config_dir.join("source.ovpn"));
    |                                ^^^^^^^^^^^^^^^ unknown field
    |
help: a field with a similar name exists
    |
928 -         assert_eq!(found, inst.user_config_dir.join("source.ovpn"));
928 +         assert_eq!(found, inst.config_dir.join("source.ovpn"));
    |

error[E0425]: cannot find function `find_config_file` in this scope
   --> crates\winnet\src\openvpn.rs:939:13
    |
939 |             find_config_file(&inst, "source.ovpn").expect("файл есть в системном каталоге");
    |             ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `find_config_file` in this scope
   --> crates\winnet\src\openvpn.rs:947:17
    |
947 |         assert!(find_config_file(&inst, "source.ovpn").is_none());
    |                 ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `resolve_user_config_dir` in this scope
   --> crates\winnet\src\openvpn.rs:953:19
    |
953 |         let err = resolve_user_config_dir(None)
    |                   ^^^^^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0599]: no variant, associated function, or constant named `UserProfileNotFound` found for enum `WinNetError` in the current scope
   --> crates\winnet\src\openvpn.rs:955:44
    |
955 |         assert!(matches!(err, WinNetError::UserProfileNotFound));
    |                                            ^^^^^^^^^^^^^^^^^^^ variant, associated function, or constant not found in `WinNetError`
    |
   ::: crates\winnet\src\lib.rs:19:1
    |
 19 | pub enum WinNetError {
    | -------------------- variant, associated function, or constant `UserProfileNotFound` not found for this enum

error[E0425]: cannot find function `resolve_user_config_dir` in this scope
   --> crates\winnet\src\openvpn.rs:963:19
    |
963 |         let got = resolve_user_config_dir(Some(profile.clone()))
    |                   ^^^^^^^^^^^^^^^^^^^^^^^ not found in this scope

Some errors have detailed explanations: E0425, E0560, E0599, E0609.
For more information about an error, try `rustc --explain E0425`.
error: could not compile `proxypilot-winnet` (lib test) due to 22 previous errors
warning: build failed, waiting for other jobs to finish...
```

### GREEN — `cargo test -p proxypilot-winnet --lib openvpn`, после реализации

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.06s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-09ac8735ffd742da.exe)

running 31 tests
test autostart::tests::points_at_matches_the_real_run_shape_of_openvpn_gui ... ok
test openvpn::tests::find_installation_reads_bin_dir_and_config_dir_into_the_right_slots ... ok
test openvpn::tests::finding_the_real_installation_does_not_fail ... ok
test openvpn::tests::connect_fails_clearly_when_openvpn_is_not_found ... ok
test openvpn::tests::disconnect_fails_clearly_when_openvpn_is_not_found ... ok
test openvpn::tests::install_profile_fails_clearly_when_openvpn_is_not_found ... ok
test openvpn::tests::build_gui_command_for_disconnect_targets_our_profile_by_name ... ok
test openvpn::tests::build_gui_command_for_connect_targets_our_profile_by_name ... ok
test openvpn::tests::build_and_install_profile_propagates_a_profile_error_without_writing_anything ... ok
test openvpn::tests::locate_returns_none_when_the_registry_bin_dir_does_not_exist_on_disk ... ok
test openvpn::tests::open_key_is_none_for_a_subkey_that_does_not_exist ... ok
test openvpn::tests::find_config_file_returns_none_when_absent_from_both ... ok
test openvpn::tests::build_gui_command_survives_a_program_path_with_spaces ... ok
test openvpn::tests::locate_returns_none_when_the_registry_bin_dir_has_no_gui_exe ... ok
test openvpn::tests::install_profile_creates_the_user_config_dir_if_it_does_not_exist_yet ... ok
test openvpn::tests::resolve_user_config_dir_fails_clearly_when_userprofile_is_unset ... ok
test openvpn::tests::resolve_user_config_dir_joins_openvpn_config_onto_the_profile ... ok
test openvpn::tests::build_and_install_profile_writes_the_built_profile ... ok
test openvpn::tests::find_config_file_falls_back_to_the_system_directory ... ok
test openvpn::tests::locate_falls_back_to_the_standard_bin_dir_when_the_registry_value_is_empty ... ok
test openvpn::tests::locate_falls_back_to_the_standard_config_dir_when_the_registry_value_is_empty ... ok
test openvpn::tests::locate_finds_installation_when_registry_bin_dir_has_the_gui_exe ... ok
test openvpn::tests::install_profile_does_not_touch_neighbouring_files ... ok
test openvpn::tests::profile_status_fails_clearly_when_openvpn_is_not_found ... ok
test openvpn::tests::install_profile_round_trips_a_user_config_dir_with_spaces ... ok
test openvpn::tests::install_profile_never_writes_into_the_system_config_dir ... ok
test openvpn::tests::install_profile_overwrites_an_existing_file_under_our_own_name ... ok
test openvpn::tests::find_config_file_prefers_the_user_directory_when_present_in_both ... ok
test openvpn::tests::profile_status_reports_not_installed_when_the_profile_file_is_absent ... ok
test openvpn::tests::install_profile_writes_under_our_own_name ... ok
test openvpn::tests::profile_status_reports_installed_when_the_profile_file_is_present ... ok

test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 125 filtered out; finished in 0.04s
```

19 из этих 31 — новые или переписанные тесты этого раунда
(`find_config_file_*` × 3, `resolve_user_config_dir_*` × 2,
`install_profile_never_writes_into_the_system_config_dir`,
`install_profile_writes_under_our_own_name`,
`install_profile_does_not_touch_neighbouring_files`,
`install_profile_round_trips_a_user_config_dir_with_spaces`,
`install_profile_creates_the_user_config_dir_if_it_does_not_exist_yet`,
`install_profile_fails_clearly_when_openvpn_is_not_found`,
`profile_status_fails_clearly_when_openvpn_is_not_found`,
`connect_fails_clearly_when_openvpn_is_not_found`,
`disconnect_fails_clearly_when_openvpn_is_not_found`,
`finding_the_real_installation_does_not_fail` — переписаны под третье
поле `Installation`, не только добавлены), остальные 12 — прежние тесты
задачи 1/4, не тронутые логикой этого раунда, зелёные без изменений.

### Три команды CI — полный вывод, финальная проверка на HEAD (`11d18fc`)

`cargo test --all` — сводка по каждому из shared-тестов крейта (полный
листинг тестов `winnet` — 154 теста — не дублируется здесь: он совпадает
построчно с GREEN-прогоном фильтра `openvpn` выше плюс остальные,
не относящиеся к этой задаче, тесты `autostart`/`sysproxy`/`routes`/
`tunnel_log`/`tunnel_state`/`ovpn_profile`, которые эта задача не меняла):

```
running 147 tests
test result: ok. 147 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.61s

running 69 tests
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

running 86 tests
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 43 tests
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 154 tests
test result: ok. 154 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s

Doc-tests proxypilot_bridge: 0 passed; 0 failed
Doc-tests proxypilot_core: 0 passed; 0 failed
Doc-tests proxypilot_netsvc: 0 passed; 0 failed
Doc-tests proxypilot_winnet: 0 passed; 0 failed
```

Итого: **501 passed, 0 failed, 3 ignored** по всему workspace (winnet:
154, было 133 в конце round 1 — +21, соответствует новым/переписанным
тестам openvpn выше плюс 2 уже ignored-теста этого крейта, не относящихся
к задаче).

`cargo clippy --all-targets -- -D warnings`:

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-netsvc v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\netsvc)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.43s
```

Чисто.

`cargo fmt --all --check` — без вывода, exit code 0 (после одного прогона
`cargo fmt --all`, который перестроил перенос строк в двух местах —
сигнатуру `locate` и одну тестовую ассерцию, чинил не логику).

### Проверено на живой машине (только чтение — CLAUDE.md, «Живые проверки»)

- `HKLM\SOFTWARE\OpenVPN`: `config_dir` указывает на каталог под `Program
  Files`; `%USERPROFILE%\OpenVPN\config` и `%USERPROFILE%\OpenVPN\log` оба
  существуют.
- `icacls` на системном каталоге конфигураций: полный доступ только у
  `TrustedInstaller`, `SYSTEM` и группы администраторов; обычный
  пользователь — только чтение/выполнение. `icacls` на
  `%USERPROFILE%\OpenVPN\config`: полный доступ у самой учётной записи.
  Ни один из этих каталогов в файл репозитория не копировался — здесь
  описан вывод, не сам вывод.
- Ни один файл не был записан ни в системный, ни в пользовательский
  каталог OpenVPN на реальной машине — обе проверки выше read-only
  (`icacls`, `Get-ItemProperty`). Все тесты, которые пишут на диск,
  используют временные каталоги под `%TEMP%`, никогда не настоящие
  `Program Files`/`%USERPROFILE%`. `openvpn-gui.exe` не запускался, туннель
  не трогался — на машине, где велась сессия, уже был поднят рабочий
  туннель пользователя.

### Оговорка о коммите

Правка этого раунда физически попала в коммит `11d18fc` («docs: инструкция
по настройке») — рабочая копия совпала по времени с не связанной с этой
задачей правкой документации в этой же сессии, и `git add -A` той правки
подхватил уже готовые, но ещё не закоммиченные изменения этого раунда.
Содержимое коммита корректно и проверено (см. `git show --stat 11d18fc` —
те же пять файлов `crates/`, тот же диф, что описан выше); сообщение
коммита к этой правке не относится. Историю не переписывали.
