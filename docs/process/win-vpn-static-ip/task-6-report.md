# Задача 6 — Служба профиля сети: отчёт

Ветка `feat/vpn-static-ip`, база — `df443eb` (после задачи 5, fix round 1).

## Раскрытие: прерывание сессии

Работа над этой задачей была прервана сбоем API у среды выполнения (не связано с
содержанием задачи) в момент, когда на диске уже лежали все восемь файлов
`crates/netsvc/src/*.rs` кроме `main.rs` (тот был заглушкой `fn main() {}`), а
`cargo test -p proxypilot-netsvc` показывал 36 тестов зелёными. Контроллер
сессии передал верифицированный им самим снимок состояния и явно предупредил:
не сочинять заново красный прогон для кода, который уже существует и проходит.

Я проверил это утверждение, а не принял на веру: файл со изначальным красным
прогоном (`profile`/`netsh_cmd`/`safety`/`state`/`adapter::find_office_adapter`,
37 ошибок компиляции) уцелел в скретчпаде этой же сессии и совпадает дословно с
тем, что приведено ниже в разделе TDD. Он получен ДО реализации, в этой же
сессии, обычным порядком «дописал тесты → упало → реализовал → позеленело» — не
восстановлен и не переписан задним числом. Часть работы (`main.rs`, тесты для
`install.rs`, поле `iface` в `AppliedState`) была сделана уже после этого
разрыва, и для неё красный прогон снят обычным порядком, тоже приведён ниже.

## Что сделано

Новый крейт-бинарь `crates/netsvc/` (`proxypilot-netsvc`, и библиотека
`proxypilot_netsvc`), добавлен в члены workspace. Плюс команды
`install-service`/`uninstall-service` в `crates/app/src/main.rs` и зависимость
`proxypilot-netsvc` в `crates/app/Cargo.toml`.

### Модули крейта (все, кроме `main.rs`, покрыты тестами)

- **`profile.rs`** — разбор `%ProgramData%\ProxyPilot\profile.toml`,
  собственной копии службы. `ServiceProfile { office_networks: Vec<OfficeNetwork>,
  net_profile: NetProfile }` — оба типа взяты из `proxypilot_core` как есть, не
  продублированы. Отсутствие файла → `ServiceProfile::default()` (профиль не
  настроен, `decide_profile` не тронет сеть); битый TOML → ошибка, не паника.
- **`netsh_cmd.rs`** — построение `std::process::Command` для
  `netsh interface ipv4 set address`/`set dnsservers`/возврата в DHCP, БЕЗ
  исполнения. `commands_for_action(iface, &ProfileAction)` — единственная точка,
  где решение `decide_profile` превращается в команды; сама логика решения не
  продублирована.
- **`safety.rs`** — `evaluate_gateway(iface, gateway, is_reachable, log)`:
  решает, нужен ли откат в DHCP, и строит его команды; проверку достижимости и
  запись в лог получает замыканиями снаружи — ровно так тестируется без единого
  настоящего пакета и без подписчика `tracing` в тесте.
- **`state.rs`** — `AppliedState { ip, mask, iface }`, собственная память
  службы о последнем применённом ею значении в `%ProgramData%\ProxyPilot\applied.toml`.
  Источник `AdapterConfig::set_by_us` (задача 5): Windows не помечает адрес,
  поставленный `netsh`, ничьим владением, поэтому этот признак берётся не из
  адаптера, а из памяти службы. Поле `iface` — отступление от исходного
  наброска, объяснено ниже.
- **`adapter.rs`** — `find_office_adapter` (чистая функция, сопоставляет GUID
  офисной сети с дружественным именем адаптера — тем, что понимает
  `netsh ... name=`) плюс живое чтение: `gather_from_nlm()` (NLM
  `GetNetworkConnections` + `GetAdapterId`, join с `GetAdaptersAddresses` по
  GUID адаптера) и `current_ipv4_config(friendly_name)` (DHCP-флаг, адрес,
  маска, DNS того же адаптера). Оба живых вызова — чтение, ничего не меняют.
- **`install.rs`** — `install(exe_path)`/`uninstall()` через
  `OpenSCManagerW`/`CreateServiceW`/`OpenServiceW`/`DeleteService`. `install`
  регистрирует службу с `SERVICE_AUTO_START`, но НЕ запускает её —
  `StartServiceW` здесь не вызывается вовсе.
- **`main.rs`** — вход `StartServiceCtrlDispatcherW`, обработчик управления
  (`SERVICE_CONTROL_STOP`/`SHUTDOWN`), главный цикл. Подписка на смену сети —
  через `proxypilot_winnet::events::watch_network_changes` (тот же канал, что
  и в приложении), откачиваемая блокирующим `blocking_recv()` на отдельном
  потоке без рантайма tokio (см. «Отступления» ниже), плюс таймер на 30 секунд
  как подстраховка. Единственная точка исполнения `netsh` (`run_netsh`) и
  единственная точка ICMP-пинга шлюза (`gateway_reachable`, через
  `IcmpSendEcho`) — обе не достижимы ни одним тестом, что явно
  задокументировано в докблоках.

### `crates/app/src/main.rs`

`main()` теперь разбирает `argv` до вызова `run()`: `install-service` и
`uninstall-service` вызывают `proxypilot_netsvc::install::{install,uninstall}` и
возвращают код процесса, не показывая трей. Путь к `proxypilot-netsvc.exe`
берётся рядом с `proxypilot.exe` (`current_exe().parent()`).

## Что НЕ сделано в этой сессии — согласно брифу

- Служба не установлена и не запущена ни разу — ни `install-service`, ни
  `services.msc`, ни `sc.exe create`.
- `netsh interface ipv4 set address`/`set dnsservers` не выполнены НИ РАЗУ, ни
  в тесте, ни вручную. `netsh interface ipv4 show config` — выполнен, это
  чтение (см. ниже).
- ICMP-пинг реального шлюза (`gateway_reachable`) не вызван ни разу — только
  собран как код.
- `HKLM` не тронут, каталог конфигураций OpenVPN не тронут, туннель не
  поднимался.

## Read-only проверка, которую я выполнил

`netsh interface ipv4 show config` — read-only, разрешено брифом явно, если
нужно свериться с реальным форматом вывода. Использовано не для парсинга
вывода `netsh` (служба его не парсит — это отдельная задача, никак не
связанная с TOML-профилем), а для независимой проверки МОЕГО кода чтения
`GetAdaptersAddresses` (`adapter::current_ipv4_config`): временно добавленный
тест печатал структуру `CurrentIpv4Config` для одного реального Wi-Fi
адаптера этой машины и она была сверена глазами с тем, что показал `netsh` —
совпали DHCP-флаг, адрес, маска (по префиксу) и DNS-сервер. Тест и вывод
удалены сразу после проверки, реальные значения этой машины никуда не
записаны — в код, тесты и этот отчёт попадают только документационные
адреса RFC 5737 (`203.0.113.0/24`, `198.51.100.0/24`), по смыслу: сверка
проводилась на домашней Wi-Fi-сети этой машины, не на рабочей инфраструктуре.

## Отступления от брифа/наброска — заявлены сам

1. **`ServiceProfile` — не то же самое, что `NetProfile`.** Бриф говорит
   «служба читает свою копию профиля», но `netprofile::NetProfile` (задача 5)
   несёт только адрес/маску/шлюз/DNS — не список GUID офисных сетей, без
   которого нельзя решить «мы в офисе». Формат `profile.toml` службы поэтому
   — `office_networks + net_profile` вместе; я определил его сам, раз задача 5
   и `docs/design.md` §7.4 этого явно не специфицируют, и переиспользовал оба
   типа `proxypilot_core` без изменений.
2. **`AppliedState.iface`.** Не было в исходном наброске состояния службы (я
   не начинал с готового плана этого поля — оно всплыло при проектировании
   `main.rs::run_cycle`). Причина: `adapter::gather_from_nlm` находит адаптер
   офисной сети, ТОЛЬКО пока машина ещё физически в этой сети — как только
   она уходит, NLM перестаёт отдавать то подключение вовсе, а откатить
   статику в DHCP всё равно нужно на том же физическом адаптере. Без
   собственной памяти об имени адаптера сделать это средствами NLM
   невозможно.
3. **In-office ⇒ адаптер из NLM; out-of-office ⇒ адаптер из памяти службы.**
   Прямое следствие пункта 2, реализовано в `run_cycle` (`main.rs`).
4. **Определение «мы в офисе» переиспользует `Config::place_for`.** Вместо
   того чтобы заново писать сопоставление GUID подключённых сетей со списком
   офисных (регистронезависимое сравнение, откат к первой подключённой при
   отсутствии совпадения), `run_cycle` строит временный `Config { office_networks:
   ..., ..Default::default() }` и зовёт уже протестированный `place_for`.
   Не дублирование логики сопоставления, которое иначе разошлось бы с
   `core::config` независимым багом.
5. **Подписка на сеть без рантайма tokio.** `winnet::events::watch_network_changes`
   сама поднимает выделенный поток с циклом сообщений и не требует рантайма —
   в самой функции это явно предусмотрено («рантайм tokio недоступен, сторож
   закрытия канала не заведён»). А вот `debounce()` внутри делает
   `tokio::spawn` и без рантайма паникует. Заводить целый `tokio::runtime`
   в процессе службы ради одной функции схлопывания пачки событий я счёл
   неоправданным: `decide_profile` и так идемпотентна (`LeaveAlone` на
   уже верном состоянии, задача 5) — необработанная пачка дублей стоит
   нескольких лишних чтений `GetAdaptersAddresses`, не лишних записей в
   адаптер. Поэтому в `main.rs` — свой канал `std::sync::mpsc`, поток-мост
   до `winnet::events` через `blocking_recv()`, и отдельный поток-таймер на
   30 секунд как подстраховка (тот же смысл, что `REEVALUATE_PERIOD` в
   приложении).
6. **Достижимость шлюза — через `IcmpSendEcho` (сырой ICMP), а не TCP.**
   В брифе не специфицировано; ICMP — то, что документация Microsoft называет
   стандартным способом для такой проверки, и единственный протокол, не
   зависящий от того, какие порты открыты на шлюзе.

## Приёмка брифа — построчно

- [x] Тесты на разбор `profile.toml` службы — `profile.rs`, 6 тестов.
- [x] Тесты на формирование команд `netsh` (адрес, маска, шлюз, DNS; несколько
      DNS) — `netsh_cmd.rs`, 10 тестов.
- [x] Тест на путь отката: недостижимый шлюз → команда возврата в DHCP + запись
      в лог — `safety.rs::unreachable_gateway_produces_the_dhcp_restore_commands_and_a_log_entry`.
- [x] Логика решения — только `proxypilot_core::netprofile::decide_profile`, не
      продублирована; единственная точка встречи с командами —
      `netsh_cmd::commands_for_action`.
- [x] Крейт в членах workspace, три проверки CI проходят (ниже).

## TDD: исходный красный прогон (эта же сессия, до реализации)

`profile.rs`, `netsh_cmd.rs`, `safety.rs`, `state.rs`,
`adapter.rs::find_office_adapter` — тесты написаны первыми, стабы отсутствовали
вовсе. Команда: `cargo test -p proxypilot-netsvc --lib`. Вывод — дословно,
без сокращений:

```
   Compiling proxypilot-netsvc v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\netsvc)
error[E0432]: unresolved imports `windows::Win32::System::Services::DeleteServiceW`, `windows::Win32::System::Services::SERVICE_DELETE`
  --> crates\netsvc\src\install.rs:29:41
   |
29 |     CloseServiceHandle, CreateServiceW, DeleteServiceW, OpenSCManagerW, OpenServiceW,
   |                                         ^^^^^^^^^^^^^^ no `DeleteServiceW` in `Win32::System::Services`
30 |     ENUM_SERVICE_TYPE, SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE, SERVICE_AUTO_START,
31 |     SERVICE_DELETE, SERVICE_ERROR_NORMAL, SERVICE_WIN32_OWN_PROCESS,
   |     ^^^^^^^^^^^^^^ no `SERVICE_DELETE` in `Win32::System::Services`
   |
help: a similar name exists in the module
   |
29 -     CloseServiceHandle, CreateServiceW, DeleteServiceW, OpenSCManagerW, OpenServiceW,
29 +     CloseServiceHandle, CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW,
   |

warning: unused import: `std::collections::HashMap`
  --> crates\netsvc\src\adapter.rs:25:5
   |
25 | use std::collections::HashMap;
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

warning: unused import: `PathBuf`
  --> crates\netsvc\src\profile.rs:13:23
   |
13 | use std::path::{Path, PathBuf};
   |                       ^^^^^^^

warning: unused import: `PathBuf`
  --> crates\netsvc\src\state.rs:26:23
   |
26 | use std::path::{Path, PathBuf};
   |                       ^^^^^^^

error[E0425]: cannot find function `find_office_adapter` in this scope
  --> crates\netsvc\src\adapter.rs:55:19
   |
55 |         let got = find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}");
   |                   ^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `find_office_adapter` in this scope
  --> crates\netsvc\src\adapter.rs:63:13
   |
63 |             find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}"),
   |             ^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `find_office_adapter` in this scope
  --> crates\netsvc\src\adapter.rs:71:13
   |
71 |             find_office_adapter(&[], "{AAAA0000-0000-0000-0000-000000000001}"),
   |             ^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `find_office_adapter` in this scope
  --> crates\netsvc\src\adapter.rs:82:13
   |
82 |             find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}"),
   |             ^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `find_office_adapter` in this scope
  --> crates\netsvc\src\adapter.rs:98:13
   |
98 |             find_office_adapter(&adapters, "{AAAA0000-0000-0000-0000-000000000001}"),
   |             ^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0610]: `u32` is a primitive type and therefore doesn't have fields
   --> crates\netsvc\src\install.rs:101:32
    |
101 |             SC_MANAGER_CONNECT.0,
    |                                ^

error[E0425]: cannot find function `set_static_address_command` in this scope
  --> crates\netsvc\src\netsh_cmd.rs:29:19
   |
29 |         let cmd = set_static_address_command(
   |                   ^^^^^^^^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_static_address_command` in this scope
  --> crates\netsvc\src\netsh_cmd.rs:54:19
   |
54 |         let cmd = set_static_address_command(
   |                   ^^^^^^^^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_static_address_command` in this scope
  --> crates\netsvc\src\netsh_cmd.rs:73:19
   |
73 |         let cmd = set_static_address_command(
   |                   ^^^^^^^^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_dns_commands` in this scope
  --> crates\netsvc\src\netsh_cmd.rs:84:20
   |
84 |         let cmds = set_dns_commands("OfficeAdapter", &[Ipv4Addr::new(203, 0, 113, 53)]);
   |                    ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_dns_commands` in this scope
   --> crates\netsvc\src\netsh_cmd.rs:109:20
    |
109 |         let cmds = set_dns_commands("OfficeAdapter", &dns);
    |                    ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_dns_commands` in this scope
   --> crates\netsvc\src\netsh_cmd.rs:156:20
    |
156 |         let cmds = set_dns_commands("OfficeAdapter", &[]);
    |                    ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `dhcp_restore_commands` in this scope
   --> crates\netsvc\src\netsh_cmd.rs:163:20
    |
163 |         let cmds = dhcp_restore_commands("OfficeAdapter");
    |                    ^^^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `commands_for_action` in this scope
   --> crates\netsvc\src\netsh_cmd.rs:182:17
    |
182 |         assert!(commands_for_action("OfficeAdapter", &ProfileAction::LeaveAlone).is_empty());
    |                 ^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `commands_for_action` in this scope
   --> crates\netsvc\src\netsh_cmd.rs:187:26
    |
187 |         let via_action = commands_for_action("OfficeAdapter", &ProfileAction::SetDhcp);
    |                          ^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `dhcp_restore_commands` in this scope
   --> crates\netsvc\src\netsh_cmd.rs:188:22
    |
188 |         let direct = dhcp_restore_commands("OfficeAdapter");
    |                      ^^^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `commands_for_action` in this scope
   --> crates\netsvc\src\netsh_cmd.rs:206:20
    |
206 |         let cmds = commands_for_action("OfficeAdapter", &action);
    |                    ^^^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `load_from` in this scope
  --> crates\netsvc\src\profile.rs:50:19
   |
50 |         let got = load_from(&path).expect("отсутствие файла — не ошибка");
   |                   ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `load_from` in this scope
  --> crates\netsvc\src\profile.rs:76:19
   |
76 |         let got = load_from(&path).expect("корректный файл обязан разобраться");
   |                   ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `load_from` in this scope
   --> crates\netsvc\src\profile.rs:112:19
    |
112 |         let got = load_from(&path).expect("частичный файл обязан разобраться");
    |                   ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `load_from` in this scope
   --> crates\netsvc\src\profile.rs:126:19
    |
126 |         let err = load_from(&path).expect_err("битый toml обязан быть ошибкой");
    |                   ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `path_under` in this scope
   --> crates\netsvc\src\profile.rs:135:17
    |
135 |         let p = path_under(Path::new(r"C:\ProgramData"));
    |                 ^^^^^^^^^^ not found in this scope

error[E0423]: expected function, found built-in attribute `path`
   --> crates\netsvc\src\profile.rs:146:28
    |
146 |         let service_path = path();
    |                            ^^^^
    |
help: you might have meant to use `:` for type annotation
    |
146 -         let service_path = path();
146 +         let service_path: path();
    |

error[E0425]: cannot find function `evaluate_gateway` in this scope
  --> crates\netsvc\src\safety.rs:43:31
   |
43 |         let (outcome, cmds) = evaluate_gateway(
   |                               ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `evaluate_gateway` in this scope
  --> crates\netsvc\src\safety.rs:63:31
   |
63 |         let (outcome, cmds) = evaluate_gateway(
   |                               ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `evaluate_gateway` in this scope
  --> crates\netsvc\src\safety.rs:77:31
   |
77 |         let (outcome, cmds) = evaluate_gateway(
   |                               ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `evaluate_gateway` in this scope
   --> crates\netsvc\src\safety.rs:108:9
    |
108 |         evaluate_gateway(
    |         ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_by_us` in this scope
  --> crates\netsvc\src\state.rs:51:17
   |
51 |         assert!(set_by_us(
   |                 ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_by_us` in this scope
  --> crates\netsvc\src\state.rs:67:18
   |
67 |         assert!(!set_by_us(
   |                  ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_by_us` in this scope
  --> crates\netsvc\src\state.rs:80:18
   |
80 |         assert!(!set_by_us(
   |                  ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `set_by_us` in this scope
  --> crates\netsvc\src\state.rs:92:18
   |
92 |         assert!(!set_by_us(
   |                  ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `load_from` in this scope
   --> crates\netsvc\src\state.rs:103:20
    |
103 |         assert_eq!(load_from(&path), AppliedState::default());
    |                    ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `save_to` in this scope
   --> crates\netsvc\src\state.rs:115:9
    |
115 |         save_to(&path, &state).expect("запись обязана удаться");
    |         ^^^^^^^ not found in this scope

error[E0425]: cannot find function `load_from` in this scope
   --> crates\netsvc\src\state.rs:116:20
    |
116 |         assert_eq!(load_from(&path), state);
    |                    ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `load_from` in this scope
   --> crates\netsvc\src\state.rs:128:20
    |
128 |         assert_eq!(load_from(&path), AppliedState::default());
    |                    ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `path_under` in this scope
   --> crates\netsvc\src\state.rs:134:17
    |
134 |         let p = path_under(Path::new(r"C:\ProgramData"));
    |                 ^^^^^^^^^^ not found in this scope

Some errors have detailed explanations: E0423, E0425, E0432, E0610.
For more information about an error, try `rustc --explain E0423`.
warning: `proxypilot-netsvc` (lib test) generated 3 warnings
error: could not compile `proxypilot-netsvc` (lib test) due to 37 previous errors; 3 warnings emitted
```

После реализации всех недостающих функций тот же прогон — 33 теста зелёных
(позже, с добавлением живых смоук-тестов `adapter.rs` и живой связки в NLM —
36).

## TDD: `install.rs` — `wide()`/`quoted_binary_path()` (после разрыва сессии)

Сама регистрация службы (`install`/`uninstall`) не тестируется в принципе —
докблок модуля объясняет почему (проверить «служба зарегистрирована в SCM», не
регистрируя её, нельзя). Но две чистые функции внутри — `wide()` (UTF-16 с
завершающим нулём) и `quoted_binary_path()` (путь в кавычках для
`lpBinaryPathName`) — обычные функции без `unsafe`, и для них я написал тесты и
подтвердил красный прогон, временно испортив обе реализации (`wide` →
`Vec::new()`, `quoted_binary_path` → `String::new()`), затем вернул исходный
код. Команда: `cargo test -p proxypilot-netsvc --lib install::`. Вывод —
дословно:

```
   Compiling proxypilot-netsvc v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\netsvc)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.33s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_netsvc-161a79d8727a6a10.exe)

running 5 tests
test install::tests::wide_round_trips_non_ascii_text ... FAILED
test install::tests::quoted_binary_path_wraps_a_path_without_spaces_too ... FAILED
test install::tests::quoted_binary_path_wraps_a_path_with_spaces ... FAILED
test install::tests::wide_appends_exactly_one_trailing_zero ... FAILED
test install::tests::wide_of_empty_string_is_just_the_terminator ... FAILED

failures:

---- install::tests::wide_round_trips_non_ascii_text stdout ----

thread 'install::tests::wide_round_trips_non_ascii_text' (32320) panicked at crates\netsvc\src\install.rs:180:44:
attempt to subtract with overflow

---- install::tests::quoted_binary_path_wraps_a_path_without_spaces_too stdout ----

thread 'install::tests::quoted_binary_path_wraps_a_path_without_spaces_too' (3412) panicked at crates\netsvc\src\install.rs:199:9:
assertion `left == right` failed
  left: ""
 right: "\"C:\\ProxyPilot\\proxypilot-netsvc.exe\""

---- install::tests::quoted_binary_path_wraps_a_path_with_spaces stdout ----

thread 'install::tests::quoted_binary_path_wraps_a_path_with_spaces' (2444) panicked at crates\netsvc\src\install.rs:188:9:
assertion `left == right` failed
  left: ""
 right: "\"C:\\Program Files\\ProxyPilot\\proxypilot-netsvc.exe\""
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- install::tests::wide_appends_exactly_one_trailing_zero stdout ----

thread 'install::tests::wide_appends_exactly_one_trailing_zero' (36472) panicked at crates\netsvc\src\install.rs:166:9:
assertion `left == right` failed
  left: []
 right: [65, 98, 0]

---- install::tests::wide_of_empty_string_is_just_the_terminator stdout ----

thread 'install::tests::wide_of_empty_string_is_just_the_terminator' (32604) panicked at crates\netsvc\src\install.rs:171:9:
assertion `left == right` failed
  left: []
 right: [0]


failures:
    install::tests::quoted_binary_path_wraps_a_path_with_spaces
    install::tests::quoted_binary_path_wraps_a_path_without_spaces_too
    install::tests::wide_appends_exactly_one_trailing_zero
    install::tests::wide_of_empty_string_is_just_the_terminator
    install::tests::wide_round_trips_non_ascii_text

test result: FAILED. 0 passed; 5 failed; 0 ignored; 0 measured; 36 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p proxypilot-netsvc --lib`
```

Реализация возвращена, все 5 тестов позеленели, итого в `proxypilot-netsvc`
41 тест.

## Три проверки CI (полный прогон по всему workspace, после реализации)

### `cargo test --all`

Итоги по каждому исполняемому набору (полный протокол — 514 строк,
воспроизводится командой `cargo test --all` на этой же ветке):

```
proxypilot-app (src/main.rs, юнит-тесты)....... ok. 105 passed; 0 failed; 1 ignored
proxypilot-bridge (src/lib.rs).................. ok. 69 passed; 0 failed; 0 ignored
proxypilot-bridge (src/main.rs, юнит-тесты)..... ok. 0 passed; 0 failed; 0 ignored
proxypilot-bridge (tests/cli.rs)................ ok. 2 passed; 0 failed; 0 ignored
proxypilot-core (src/lib.rs).................... ok. 86 passed; 0 failed; 0 ignored
proxypilot-netsvc (src/lib.rs).................. ok. 41 passed; 0 failed; 0 ignored
proxypilot-netsvc (src/main.rs, юнит-тесты)..... ok. 0 passed; 0 failed; 0 ignored
proxypilot-winnet (src/lib.rs).................. ok. 135 passed; 0 failed; 2 ignored
Doc-tests (4 крейта)............................ ok. 0 passed каждый
```

Итого: **438 тестов пройдено, 0 упало, 3 пропущено** (два в `winnet` требуют
живого переключения Wi-Fi/реального `Run`, один в `app` трогает настоящий
`Run` этой машины — все три помечены `#[ignore]` заранее, не мной). Полный
хвост прогона (netsvc и winnet целиком, дословно):

```
     Running unittests src\lib.rs (target\debug\deps\proxypilot_netsvc-6e272e88f1aee7cb.exe)

running 41 tests
test adapter::tests::a_docking_station_second_nic_does_not_confuse_the_match ... ok
test adapter::tests::finds_the_adapter_connected_to_the_office_network ... ok
test adapter::tests::empty_adapter_list_is_none ... ok
test install::tests::quoted_binary_path_wraps_a_path_without_spaces_too ... ok
test adapter::tests::no_matching_network_is_none ... ok
test install::tests::quoted_binary_path_wraps_a_path_with_spaces ... ok
test adapter::tests::matching_is_case_insensitive ... ok
test install::tests::wide_appends_exactly_one_trailing_zero ... ok
test install::tests::wide_of_empty_string_is_just_the_terminator ... ok
test install::tests::wide_round_trips_non_ascii_text ... ok
test netsh_cmd::tests::commands_for_leave_alone_is_empty ... ok
test netsh_cmd::tests::commands_for_set_dhcp_action_matches_dhcp_restore ... ok
test netsh_cmd::tests::dhcp_restore_resets_both_address_and_dns ... ok
test netsh_cmd::tests::commands_for_set_static_action_bundles_address_and_dns ... ok
test netsh_cmd::tests::empty_dns_list_falls_back_to_dhcp_source ... ok
test netsh_cmd::tests::interface_alias_with_spaces_stays_one_argument ... ok
test netsh_cmd::tests::several_dns_servers_the_rest_are_added_with_increasing_index ... ok
test netsh_cmd::tests::single_dns_server_is_set_as_primary ... ok
test netsh_cmd::tests::static_address_command_carries_address_mask_and_gateway ... ok
test netsh_cmd::tests::static_address_command_omits_gateway_when_none ... ok
test profile::tests::a_missing_file_is_an_unmanaged_default_profile ... ok
test profile::tests::path_lives_under_program_data_not_the_user_profile ... ok
test safety::tests::no_gateway_configured_skips_the_check_entirely ... ok
test safety::tests::reachable_gateway_needs_no_rollback_and_no_log_entry ... ok
test safety::tests::the_closure_is_called_with_the_configured_gateway ... ok
test safety::tests::unreachable_gateway_produces_the_dhcp_restore_commands_and_a_log_entry ... ok
test state::tests::a_different_current_address_is_not_ours ... ok
test state::tests::matches_current_address_and_mask_is_our_own ... ok
test state::tests::a_missing_state_file_reads_as_the_default ... ok
test state::tests::a_different_current_mask_is_not_ours ... ok
test state::tests::no_recorded_state_at_all_is_never_ours ... ok
test profile::tests::malformed_toml_is_an_error_not_a_panic ... ok
test state::tests::path_lives_under_program_data ... ok
test profile::tests::missing_optional_fields_fall_back_to_defaults ... ok
test state::tests::a_corrupted_state_file_reads_as_the_default_not_a_panic ... ok
test profile::tests::parses_office_networks_and_net_profile_together ... ok
test state::tests::save_then_load_round_trips ... ok
test profile::tests::service_profile_path_differs_from_the_user_config_path ... ok
test adapter::tests::reading_current_ipv4_config_of_a_nonexistent_adapter_is_none_not_an_error ... ok
test adapter::tests::reading_current_ipv4_config_does_not_fail_for_a_real_connected_adapter ... ok
test adapter::tests::gathering_from_nlm_does_not_fail_on_a_real_machine ... ok

test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

     Running unittests src\main.rs (target\debug\deps\proxypilot_netsvc-efc38d7131b41e43.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

(остальные 397 тестов — задачи 1-5, не изменялись этой задачей; полный
протокол воспроизводим командой выше на этой ветке.)

### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-netsvc v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\netsvc)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.89s
```

Чисто, без единого предупреждения, по всему workspace.

### `cargo fmt --all --check`

Пустой вывод, код возврата 0 — чисто.

## `#[allow(...)]` и `unsafe`

Ни одного `#[allow(...)]` в новых файлах. Каждый блок `unsafe` (40 вхождений в
`adapter.rs`/`install.rs`/`main.rs`) несёт `// SAFETY:` — включая обе
`unsafe extern "system" fn` (`ffi_service_main`, `ffi_control_handler`),
хотя формально это не «блок `unsafe {}» {}», а функция, чья сигнатура обязана
быть `unsafe` под контракт FFI-обратного вызова Windows: пояснено отдельно, что
именно требует такой сигнатуры и что тело не разыменовывает свои сырые
указатели без нужды.

## Что человек обязан проверить руками — и почему это не сделал агент

Всё, что ниже, требует либо UAC, либо реальной смены сети в офисе, либо
исполнения `netsh`/пинга — три вещи, которые эта сессия не выполняет ни при
каких обстоятельствах (см. `CLAUDE.md`, «Живые проверки, которые не делает
агент»). **Первый живой прогон обязан пройти при человеке, в офисной сети, а
не в фоне** — вся страховка (пункт 3 ниже) проверяется в этот момент впервые
по-настоящему, и откатывать в случае ошибки будет некому, если это случится
без присмотра.

1. **Установка.** `proxypilot.exe install-service` от администратора (один
   UAC). Проверить: `services.msc` показывает `ProxyPilot Net Profile`
   (`ProxyPilotNetProfile`), тип запуска — «Авто». Служба НЕ запущена сразу
   после установки (по конструкции — `install` не вызывает `StartServiceW`).
2. **Первый пуск.** Запустить вручную (`Start-Service ProxyPilotNetProfile`
   или из `services.msc`) при пустом/ненастроенном `%ProgramData%\ProxyPilot\profile.toml`
   — служба обязана подняться и не трогать сеть вовсе (профиль не настроен).
   Проверить лог `%ProgramData%\ProxyPilot\logs\netsvc.*.log`: строка о
   старте, ни одной команды `netsh`.
3. **Основной сценарий, в офисе, при человеке.** Положить настоящий
   `profile.toml` (GUID реальной офисной сети + адрес/маска/шлюз/DNS),
   перезапустить службу, подключиться к офисной сети:
   - адаптер обязан получить именно тот адрес и DNS, что в профиле;
   - выйти из офисной сети — адрес обязан вернуться на DHCP;
   - вернуться в офис второй раз — адрес обязан выставиться заново (это как
     раз путь `set_by_us` через `applied.toml`, который не проверить без
     реальной службы);
   - **страховка**: временно указать в профиле заведомо недостижимый шлюз
     (документационный, например из RFC 5737) — служба обязана применить
     статику, не достучаться до шлюза и откатить в DHCP сама, с записью в
     лог. Это единственный способ по-настоящему проверить код
     `gateway_reachable`/`evaluate_gateway` в связке — как раз то новое, что
     здесь заведено впервые.
4. **Ручная статика человека не трогается.** На адаптере вне офисного
   профиля (или до установки офисного адреса службой) прописать руками
   произвольный статический адрес — заново поднятая служба обязана оставить
   его в покое (`foreign_static_address_is_never_reset`, задача 5,
   в связке с `state::set_by_us`, который для чужой статики вернёт `false`).
5. **Докстанция / вторая карта.** Если есть возможность — подключить машину
   одновременно по Wi-Fi и через докстанцию/Ethernet, убедиться, что
   статика ставится на тот адаптер, что реально несёт офисную сеть (через
   NLM), а не на первый попавшийся.
6. **Удаление.** `proxypilot.exe uninstall-service` — служба пропадает из
   `services.msc`. Если она была запущена в момент удаления — проверить
   поведение соответствует стандартному поведению `sc.exe delete` (служба
   помечается к удалению, реально исчезает после остановки/перезапуска —
   `uninstall` не останавливает её сама, см. докблок `install.rs`).
7. **Логи.** Ежедневная ротация `netsvc.*.log` в `%ProgramData%\ProxyPilot\logs\`
   — тем же приёмом, что и лог приложения, но не проверялась на второй день
   ни разу (нужно реальное время).

## Данные

В диффе, в этом отчёте и во всех сообщениях коммитов — только документационные
диапазоны RFC 5737 (`203.0.113.0/24`, `198.51.100.0/24`) и обобщённые
иллюстрации. Единственная живая проверка на этой машине (`netsh interface ipv4
show config`, раздел выше) описана по смыслу («домашняя Wi-Fi-сеть этой
машины»), без единого настоящего адреса, имени сети или адаптера.
