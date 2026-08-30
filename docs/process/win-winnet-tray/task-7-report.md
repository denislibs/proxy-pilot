# Task 7 — События смены сети — отчёт

**Статус:** DONE
**Коммит:** `3b4018e` — `feat(win): события смены сети через NLM со схлопыванием пачки`
**Ветка:** `feat/windows-rust` (родитель `aedd4c0`)

## Что сделано

- `win/crates/winnet/src/events.rs` (новый): `NetworkChange`, `debounce`,
  `watch_network_changes`, COM-приёмник `NlmSink` через `#[implement]`,
  цикл сообщений, запасной канал `NotifyIpInterfaceChange`.
- `win/crates/winnet/src/lib.rs`: добавлен `pub mod events;` между `com` и
  `networks` (алфавитный порядок сохранён, файл больше ничем не тронут).
- `win/crates/winnet/Cargo.toml`: `tokio`, `windows-core`, фичи `windows`
  (`implement`, `Win32_System_Threading`, `Win32_UI_WindowsAndMessaging`,
  `Win32_NetworkManagement_IpHelper`, `Win32_NetworkManagement_Ndis`,
  `Win32_Networking_WinSock`); dev-зависимость `tracing-subscriber` — только
  для `#[ignore]`-теста ручной проверки.
- `win/Cargo.toml`: в workspace-зависимости добавлен `windows-core = "0.58"`.

### Почему понадобился `windows-core`

Макрос `#[implement]` разворачивается в абсолютные пути `::windows_core::…`.
Крейт `windows` внутри себя делает `pub use windows_core as core`, но
абсолютный путь в потребителе так не резолвится — `windows-core` обязан быть
прямой зависимостью. Версия та же (0.58), это буквально тот же крейт из
графа, дублирования типов нет.

## Устройство подписки

`watch_network_changes()` поднимает выделенный поток
(`proxypilot-netwatch`), который:

1. заводит **свой** `ComGuard` — апартамент привязан к потоку, страж
   вызывающей стороны сюда не годится;
2. `CoCreateInstance(&NetworkListManager)` сразу в
   `IConnectionPointContainer`;
3. `FindConnectionPoint(&INetworkListManagerEvents::IID)`;
4. `Advise(&sink)`, где `sink` — `#[implement(INetworkListManagerEvents)]`;
   кука сохраняется рядом с точкой подключения;
5. создаёт очередь сообщений (`PeekMessageW` с `PM_NOREMOVE`) **до** того,
   как отдаёт наружу свой thread id, иначе просьба остановиться могла бы
   прилететь в поток без очереди и пропасть;
6. отчитывается вызывающей стороне через `std::sync::mpsc` — успех или
   `WinNetError`;
7. крутит `GetMessageW`/`DispatchMessageW`.

Без пункта 7 приёмник не вызывается вообще: COM доставляет апартаментные
вызовы оконными сообщениями. Это ровно тот отказ, который выглядит как
рабочий код (см. ручную проверку ниже — она его и ловит).

**Остановка.** Приёмник на закрытом канале и задача-сторож
(`Sender::closed()`) шлют потоку `WM_APP+1`. Поток выходит из цикла сам,
вызывает `Unadvise(cookie)` и только потом отпускает `ComGuard`. Никаких
`TerminateThread`.

**Неблокирующий приёмник.** Только `try_send`. `Full` — событие
выбрасывается молча (потребитель пересчитывает решение целиком, один
пересчёт покроет обе смены). `Closed` — просьба остановиться. Ни
аллокаций, ни логирования, ни `unwrap` внутри COM-обратного вызова:
раскрутка через FFI недопустима.

**Запасной канал.** Любой отказ на пути выше — `warn!` с причиной и
`NotifyIpInterfaceChange(AF_UNSPEC, …)`. Отображение уведомлений:
`MibAddInstance → NetworkAdded`, `MibDeleteInstance → Connectivity`
(пропал интерфейс — это про связность), остальное →
`NetworkPropertyChanged`. Контекст обратного вызова утекает сознательно
(одна маленькая аллокация на процесс, только на запасном пути): гарантий,
что `CancelMibChangeNotify2` дожидается уже начатых вызовов, документация
не даёт, а гонка с чтением освобождённой памяти дороже утечки.

Опроса по таймеру нет ни в одном из путей.

## TDD

### RED — до реализации

`win/crates/winnet/src/events.rs` содержал только модуль тестов из брифа.

```
$ cd win && cargo test -p proxypilot-winnet events
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
error[E0425]: cannot find type `NetworkChange` in this scope
  --> crates\winnet\src\events.rs:37:53
   |
37 |         let (tx, rx) = tokio::sync::mpsc::channel::<NetworkChange>(1);
   |                                                     ^^^^^^^^^^^^^ not found in this scope
   |
help: you might be missing a type parameter
   |
36 |     async fn closing_the_source_closes_the_output<NetworkChange>() {
   |                                                  +++++++++++++++

warning: unused import: `super::*`
 --> crates\winnet\src\events.rs:3:9
  |
3 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0425]: cannot find function `debounce` in this scope
  --> crates\winnet\src\events.rs:10:23
   |
10 |         let mut out = debounce(rx, Duration::from_millis(100));
   |                       ^^^^^^^^ not found in this scope

error[E0433]: cannot find type `NetworkChange` in this scope
  --> crates\winnet\src\events.rs:13:21
   |
13 |             tx.send(NetworkChange::Connectivity).await.unwrap();
   |                     ^^^^^^^^^^^^^ use of undeclared type `NetworkChange`

error[E0425]: cannot find function `debounce` in this scope
  --> crates\winnet\src\events.rs:24:23
   |
24 |         let mut out = debounce(rx, Duration::from_millis(50));
   |                       ^^^^^^^^ not found in this scope

error[E0433]: cannot find type `NetworkChange` in this scope
  --> crates\winnet\src\events.rs:26:17
   |
26 |         tx.send(NetworkChange::Connectivity).await.unwrap();
   |                 ^^^^^^^^^^^^^ use of undeclared type `NetworkChange`

error[E0433]: cannot find type `NetworkChange` in this scope
  --> crates\winnet\src\events.rs:30:17
   |
30 |         tx.send(NetworkChange::NetworkPropertyChanged).await.unwrap();
   |                 ^^^^^^^^^^^^^ use of undeclared type `NetworkChange`

error[E0425]: cannot find function `debounce` in this scope
  --> crates\winnet\src\events.rs:38:23
   |
38 |         let mut out = debounce(rx, Duration::from_millis(10));
   |                       ^^^^^^^^ not found in this scope

Some errors have detailed explanations: E0425, E0433.
For more information about an error, try `rustc --explain E0425`.
warning: `proxypilot-winnet` (lib test) generated 1 warning
error: could not compile `proxypilot-winnet` (lib test) due to 7 previous errors; 1 warning emitted
```

### GREEN — после реализации схлопывания

```
$ cargo test -p proxypilot-winnet events
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.56s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-1a122af83625ba94.exe)

running 3 tests
test events::tests::a_burst_collapses_to_one_event ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test events::tests::events_further_apart_than_the_window_both_pass ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 16 filtered out; finished in 0.13s
```

## Ручная проверка — событие реально приходит

Проверка живёт `#[ignore]`-тестом `events::tests::watch_a_real_network_change`
(в CI не гоняется). Он разветвляет поток событий: в лог идёт и сырая пачка от
NLM, и то, что осталось после схлопывания окном в 2 секунды.

Физическое воздействие: `netsh wlan disconnect`, через 8 секунд
`netsh wlan connect name=KZTK-38455_5G` (Wi-Fi MediaTek MT7921, Windows 11
26200). Два физических переключения — отключение и обратное подключение.

```
$ ./target/debug/deps/proxypilot_winnet-d431b601035c531c.exe --ignored --nocapture watch_a_real
running 1 test
2026-08-30T00:51:04.579843Z  INFO proxypilot_winnet::events::tests: ждём смены сети 45 секунд: выключите и включите Wi-Fi
2026-08-30T00:51:04.579832Z DEBUG proxypilot_winnet::events: подписка на события сети поднята thread=29684 source="nlm"
2026-08-30T00:51:08.907825Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T00:51:08.908228Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=Connectivity номер=1
2026-08-30T00:51:17.011753Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T00:51:17.012007Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=Connectivity номер=2
2026-08-30T00:51:17.020115Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T00:51:17.020298Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T00:51:17.629654Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T00:51:49.588003Z  INFO proxypilot_winnet::events::tests: окно наблюдения закрыто всего=2
test events::tests::watch_a_real_network_change ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 45.02s
```

Читается так:

- `source="nlm"` — работает основной путь, не запасной;
- отключение (00:51:08) — 1 сырое событие, 1 строка наружу;
- подключение (00:51:17) — **4 сырых события** (`.011`, `.020`, `.020`,
  `.629`) и **1 строка наружу**. Пачка схлопнулась;
- итого два физических переключения — ровно две строки.

### Разбор подписки при выбросе приёмника

Временная проба (в коммит не входит): подписаться, уронить `Receiver`,
подождать три секунды.

```
running 1 test
2026-08-30T00:46:03.099711Z  INFO proxypilot_winnet::events::tests: роняем приёмник
2026-08-30T00:46:03.099700Z DEBUG proxypilot_winnet::events: подписка на события сети поднята thread=10944 source="nlm"
2026-08-30T00:46:03.102520Z DEBUG proxypilot_winnet::events: подписка на события сети снята thread=10944
2026-08-30T00:46:06.106718Z  INFO proxypilot_winnet::events::tests: три секунды спустя
```

Строка «снята» печатается после `Unadvise` и выхода из цикла сообщений, и
без предупреждения «Unadvise не удался» — то есть кука снялась чисто, поток
завершился сам за 3 мс, а не был убит и не остался висеть.

### Запасной канал тоже проверен вживую

Временная проба (в коммит не входит): `subscribe_nlm` принудительно
возвращает ошибку, воздействие на Wi-Fi то же самое.

```
running 1 test
2026-08-30T00:46:38.423424Z  WARN proxypilot_winnet::events: подписка на события NLM не поднялась, переходим на NotifyIpInterfaceChange error=ошибка Windows: ПРОБА: NLM отключён вручную (0x80004005)
2026-08-30T00:46:38.425276Z DEBUG proxypilot_winnet::events: подписка на события сети поднята thread=16320 source="iphelper"
2026-08-30T00:46:38.425340Z  INFO proxypilot_winnet::events::tests: ждём смены сети 45 секунд: выключите и включите Wi-Fi
2026-08-30T00:46:42.740741Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:42.740827Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:42.741276Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=NetworkPropertyChanged номер=1
2026-08-30T00:46:42.968544Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:42.969009Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:50.837764Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:50.838077Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:50.838213Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=NetworkPropertyChanged номер=2
2026-08-30T00:46:50.864545Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:50.865892Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:50.869903Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:50.882129Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:50.882235Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:50.922823Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:51.881616Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:46:51.882725Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T00:47:23.438592Z  INFO proxypilot_winnet::events::tests: окно наблюдения закрыто всего=2
test events::tests::watch_a_real_network_change ... ok
```

13 сырых уведомлений IP Helper → 2 строки наружу. Причина перехода на
запасной канал написана в логе явно, `source="iphelper"` тоже виден.

## CI-команды

```
$ cargo test --all
test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 19 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo clippy --all-targets -- -D warnings
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.41s
exit=0

$ cargo fmt --all --check
exit=0
```

Было 114 тестов, стало 117 (+3 на схлопывание) и один `#[ignore]` — ручная
проверка подписки. Предупреждений нет, `#[allow]` не добавлено ни одного.
Каждый `unsafe` снабжён `// SAFETY:`.

Развёрнутый прогон `proxypilot-winnet` для наглядности:

```
running 20 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test networks::tests::category_maps_every_documented_value ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test events::tests::a_burst_collapses_to_one_event ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok
test events::tests::events_further_apart_than_the_window_both_pass ... ok

test result: ok. 19 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s
```

## Ограничения, о которых стоит знать следующей задаче

1. `watch_network_changes()` — синхронная и на пару миллисекунд блокируется,
   дожидаясь от потока ответа «подписка поднялась». Иначе про отказ пришлось
   бы догадываться по молчанию канала. Замерено: 2–3 мс.
2. Задача-сторож (`Sender::closed()`) держит клон отправителя, поэтому
   `recv()` у потребителя никогда не увидит конца канала. Это сознательно:
   единственный способ закрыть канал — выбросить приёмник, после чего
   слушать всё равно некому. Если рантайма tokio при вызове нет, сторож не
   заводится и разбор подписки случится по первому же событию на закрытом
   канале; подписке это не мешает.
3. `NetworkChange::NetworkAdded` и `NetworkPropertyChanged` порождаются
   только запасным каналом: `INetworkListManagerEvents` отдаёт ровно один
   вид события. Подписываться дополнительно на `INetworkEvents` не стал —
   приёмочные критерии этого не требовали, а лишняя точка подключения это
   лишний риск.
4. UAC не задействован: ни одного вызова, требующего повышения. Проверка
   гонялась под обычной учётной записью.

---

# Отчёт по правкам ревью

**Коммит:** `209753b` — `fix(win): задний фронт схлопывания, честный конец
канала и безопасная остановка`

Правки затрагивают только `win/crates/winnet/src/events.rs`.

## FINDING 1 — задний фронт схлопывания

`debounce` был исключительно передним фронтом: всё, что приходило в окне,
выбрасывалось, и наружу не уходило ничего. В прошлом ручном логе решение
считалось за 617 мс до последнего события пачки — по недоустоявшейся сети,
и таким и оставалось до следующего физического переключения.

Теперь задача копит последнее событие окна и по его закрытии отправляет его
наружу. Дедлайн отсчитывается от переднего фронта и не продлевается — пачка
не может держать выход бесконечно. Худший случай — две строки на пачку.

Тест `a_burst_collapses_to_one_event` утверждал дефект («после первого не
приходит ничего») и переименован в
`a_burst_collapses_to_its_first_and_last_event`: первое событие проходит,
последнее проходит, середина схлопывается, и только потом канал
закрывается. Добавлен `the_trailing_event_is_the_last_one_of_the_burst` —
он проверяет, что задний фронт несёт именно последнее событие пачки, а не
какое-нибудь из середины.

## FINDING 2 — `debounce` больше не удлиняет жизнь подписки

Оба ожидания в задаче `debounce` теперь `tokio::select!` против
`tx.closed()`. Выброшенный потребителем приёмник замечается немедленно,
задача возвращается, роняет `rx` подписки, у сторожа разрешается
`probe.closed()`, летит стоп-сообщение, поток снимает `Advise` и выходит.
Ни одного события для этого не требуется.

Новый тест `dropping_the_debounced_receiver_releases_the_source`: источник
обязан освободиться в течение 5 секунд без единого события. На старом коде
он бы висел до таймаута.

Доккомментарий `watch_network_changes` теперь прямо говорит, что `debounce`
договор не рвёт.

## FINDING 3 — смерть потока видна потребителю

- `pump_messages` возвращает `PumpExit`: `Stopped` (штатно), `Quit` (чужой
  `WM_QUIT` — мы его не шлём), `Failed(WinError)` (`GetMessageW == -1`, код
  снимается сразу через `WinError::from_win32()`). Первый пишется в
  `debug`, два остальных — в `warn`.
- Сторож теперь ждёт `select!` между `probe.closed()` и `oneshot`, который
  поток шлёт последней строкой. По завершении задачи сильный отправитель
  сторожа роняется.
- Отправители в приёмнике NLM и в утёкшем контексте IP Helper стали
  `WeakSender`. Раньше утёкший контекст держал канал открытым навсегда, и
  на запасном пути конец канала не наступил бы никогда.

Проба (в коммит не входит): `pump_messages` немедленно возвращает `Failed`,
потребитель держит приёмник.

```
running 1 test
2026-08-30T01:09:52.472220Z DEBUG proxypilot_winnet::events: подписка на события сети поднята thread=28668 source="nlm"
2026-08-30T01:09:52.474800Z  WARN proxypilot_winnet::events: GetMessageW отказала, подписка на смену сети прекращена thread=28668 error=Неверный дескриптор. (0x80070006)
2026-08-30T01:09:52.475197Z  INFO proxypilot_winnet::events::tests: что вернул recv после самопроизвольной смерти потока r=Ok(None)
test events::tests::temp_thread_death_probe ... ok
```

`warn` с причиной есть, `recv()` вернул `Ok(None)`, а не завис.

## FINDING 4 — SAFETY больше не противоречит файлу

Комментарий у разыменования контекста ссылался на семантику
`CancelMibChangeNotify2`, которую одиннадцатью строками выше сам файл
называет недокументированной. Переписан: контекст живёт до конца процесса,
`Drop` у `IpHelperSubscription` его сознательно не освобождает **именно
ради** этого разыменования. Обоснование теперь совпадает с тем, что код
действительно делает.

## FINDING 5 — остановка по идентификатору потока

Появился `Pump { thread_id, alive: AtomicBool }`, общий (через `Arc`) для
приёмника, контекста IP Helper и сторожа. `post_stop` сначала читает
`alive` и молча выходит, если поток уже вышел. `alive` сбрасывается на всех
путях выхода `watcher_thread` — до того, как поток отпустит канал.

`JoinHandle`, который раньше отбрасывался, теперь переезжает в задачу
сторожа и отпускается последним: пока дескриптор потока открыт, ядро не
переиспользует его идентификатор. Флаг и удержанный дескриптор закрывают
окно друг друга.

Оба механизма опираются на `oneshot`, который поток шлёт в самом конце, —
то самое общее завершение, из-за которого findings 3 и 5 делались вместе.

## Поправка к первому отчёту

Утверждение «ни аллокаций … внутри COM-обратного вызова» было неточным:
`try_send` может дойти до `block.grow()` и выделить память. Аллокация
амортизированная, достижимой паники нет, поведение кода не меняется — но
формулировка была сильнее правды. Верное утверждение: обратный вызов не
блокируется, не логирует и не содержит достижимой паники; аллокация
возможна и амортизирована.

## Повторная ручная проверка

Тот же сценарий: `netsh wlan disconnect`, через 8 секунд
`netsh wlan connect name=KZTK-38455_5G`.

```
running 1 test
2026-08-30T01:10:05.044652Z  INFO proxypilot_winnet::events::tests: ждём смены сети 45 секунд: выключите и включите Wi-Fi
2026-08-30T01:10:05.044616Z DEBUG proxypilot_winnet::events: подписка на события сети поднята thread=9544 source="nlm"
2026-08-30T01:10:09.414483Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T01:10:09.414649Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=Connectivity номер=1
2026-08-30T01:10:17.638390Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T01:10:17.638853Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=Connectivity номер=2
2026-08-30T01:10:17.647687Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T01:10:17.649820Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T01:10:18.322079Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T01:10:19.653021Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=Connectivity номер=3
2026-08-30T01:10:50.057745Z  INFO proxypilot_winnet::events::tests: окно наблюдения закрыто всего=3
test events::tests::watch_a_real_network_change ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 21 filtered out; finished in 45.02s
```

Читается так:

- отключение (`09.414`) — одно сырое событие, в окне после него ничего не
  пришло, поэтому заднего фронта нет и наружу ушла одна строка
  (`номер=1`). Схлопывание не выдумывает второй строки на пустом месте;
- подключение (`17.638`) — четыре сырых события (`.638`, `.647`, `.649`,
  `18.322`). Передний фронт ушёл сразу (`номер=2`), **задний — в `19.653`**
  (`номер=3`), по закрытии окна, отсчитанного от переднего фронта
  (`17.638 + 2.000 = 19.638`). Он несёт событие `18.322` — то самое,
  которое старый код выбрасывал;
- окно не продлевается событиями внутри себя: между передним и задним
  фронтом ровно 2,015 с, а не «пока идут события».

## CI после правок

```
$ cargo test --all
test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 21 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo clippy --all-targets -- -D warnings
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.64s
exit=0

$ cargo fmt --all --check
exit=0
```

Развёрнутый прогон `proxypilot-winnet` (21 тест, +2 к прошлому разу):

```
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test events::tests::a_burst_collapses_to_its_first_and_last_event ... ok
test events::tests::the_trailing_event_is_the_last_one_of_the_burst ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test events::tests::dropping_the_debounced_receiver_releases_the_source ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test networks::tests::category_maps_every_documented_value ... ok
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok
test events::tests::events_further_apart_than_the_window_both_pass ... ok

test result: ok. 21 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s
```

Итого 119 тестов (было 117), `#[allow]` по-прежнему ни одного,
предупреждений нет.

## Отложено (по решению ревью, не в этом круге)

- `watch_network_changes` блокирует воркер tokio на 2–3 мс рукопожатия;
- `debounce` паникует вне рантайма tokio, и это не написано в
  доккомментарии.
