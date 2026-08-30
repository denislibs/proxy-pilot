# Task 9: Трей и приложение — отчёт

Ветка `feat/windows-rust`, база `c4468cb`.

---

## 1. Что сделано

Новый крейт `win/crates/app` (двоичный файл `proxypilot`), связывающий уже
готовые части в работающую программу:

| Файл | Содержимое |
|---|---|
| `win/crates/app/Cargo.toml` | манифест, `[[bin]] name = "proxypilot"` |
| `win/crates/app/src/main.rs` | порядок старта, tokio-рантайм, `NlmSource`, цикл сообщений, обработчик закрытия консоли |
| `win/crates/app/src/tray.rs` | иконка и меню, `header_text`, `mode_label`, буфер обмена |
| `win/crates/app/src/icons.rs` | `IconKind`, `icon_for`, программная отрисовка RGBA |
| `win/crates/app/src/proxy.rs` | жизненный цикл системного прокси, `is_stale_pointer`, `RestoreOnDrop` |

Изменены:

| Файл | Изменение | Почему |
|---|---|---|
| `win/Cargo.toml` | `members += "crates/app"`, `tray-icon = "0.24"` | существующие записи не тронуты |
| `win/crates/core/src/config.rs` | `manage_system_proxy: bool` (по умолчанию `true`), `saved_sysproxy: Option<SavedSysProxy>`, `sync_all` в `save_to` | требования задания: выключатель управления системным прокси и **долговечное** сохранение исходных настроек до записи в реестр |

`SavedSysProxy` — три скаляра, платформенных зависимостей в `proxypilot-core`
не прибавилось; перевод в `winnet::sysproxy::SysProxy` делает приложение.

Версия `tray-icon` на момент работы — **0.24.2** (проверено `cargo search`),
взято `"0.24"`.

**Отступление от списка файлов брифа:** добавлен пятый модуль `proxy.rs`.
Логика жизненного цикла системного прокси — самая опасная часть задачи, и
держать её в `main.rs` рядом с разбором аргументов значило бы спрятать её.
`is_stale_pointer` из брифа живёт там же, вместе со своими тестами.

### Мост не изменён ни на строку

`proxypilot-bridge` и `proxypilot-winnet` не правились. Это прямое следствие
того, что маршрут уже живёт в `ArcSwap`: трей вызывает `Router::set_if_changed`
(через супервизор) из фоновой задачи, мост видит новое значение на следующем
соединении.

### Ответы на обязательные вопросы

**Порядок старта — выбран вариант «не принимать соединения до первого
пересчёта», в самой сильной форме: слушатель вообще не создаётся, пока
первый `reevaluate` не отработал.**

```rust
let router = Arc::new(Router::new(Route::Direct));
let mut supervisor = new_supervisor(&router, &cfg);
let initial = runtime.block_on(supervisor.reevaluate());   // ← сначала решение
let listener = runtime.block_on(TcpListener::bind(&addr))?; // ← только потом сокет
runtime.spawn(async move { serve(listener, shared).await });
```

Почему так, а не «сконструировать `Router` безопасным значением»: безопасного
значения не существует. `Route::Direct` безопасно по последствиям (соединение
просто не пойдёт через прокси), но оно **врёт про режим** — пользователь видит
галочку на SOCKS5, а первое соединение уходит мимо. Спека 4.2 запрещает ровно
это. Окно, в котором можно было бы принять соединение на маршруте из
конструктора, здесь физически отсутствует: сокета ещё нет, `connect` на порт
получает отказ, а не тихий обход. Значение в конструкторе `Router` осталось
заглушкой и никогда не обслуживается.

В логе это видно на каждом запуске — «маршрут изменён» всегда предшествует
«мост слушает».

**Системный прокси.** Порядок в `proxy::take_over`:

1. `sysproxy::read()` — что стоит сейчас;
2. распознавание нашего же следа от убитого процесса (`is_stale_pointer`);
3. `cfg.saved_sysproxy = …; cfg.save()` — на диск, с `sync_all` во временный
   файл до переименования;
4. запись в глобальную ячейку `ORIGINAL`;
5. `sysproxy::apply(ours)` — реестр.

Вызывается `take_over` **после** `TcpListener::bind` и `spawn(serve)`: иначе
между записью в реестр и первым `accept` система уже слала бы трафик туда,
где никто не слушает.

Восстановление — `proxy::restore()`, идемпотентное (значение забирается через
`Option::take`). Пути выхода:

| Путь | Механизм | Проверено |
|---|---|---|
| «Выход» в меню | `message_loop` возвращается → `Drop for RestoreOnDrop` | да, п. 5 ниже |
| `WM_QUIT` (завершение сеанса) | тот же `return` из `message_loop` | тот же код |
| паника на главном потоке | раскрутка стека → `Drop for RestoreOnDrop` | по построению (unwind, а не abort) |
| закрытие окна консоли, Ctrl+C | `SetConsoleCtrlHandler` → `restore()` | да, отдельная проверка ниже |
| `taskkill /F` | не восстанавливается — на это и рассчитан `is_stale_pointer` | да, проверено |

**Устаревший указатель.** Если в реестре наш адрес, а порт свободен (мы его
только что заняли — значит моста не было), исходным считается **не то, что
в реестре**, а `saved_sysproxy` из конфига. Иначе адрес мёртвого слушателя
закрепился бы как «настройки пользователя» навсегда. Если конфига нет —
`error!` и выключенный прокси: вернуть настройки уже нечем, но указывать
в пустоту машина не будет.

**Выключатель.** `manage_system_proxy = false` — реестр не трогается вовсе,
`RestoreOnDrop` не создаётся, обработчик консоли не ставится.

**COM.** `ComGuard` создаётся на главном потоке (`tray-icon`/`muda` COM сами
не поднимают, но область уведомлений — часть оболочки) и **отдельно на каждый
вызов `NlmSource::connected_ids`**: NLM зовётся с рабочего потока tokio, не с
того, где трей, и не обязательно с одного и того же от вызова к вызову.
COM-объекты `list_connected` разрушаются раньше стража (порядок объявления).

**`log::init` — ровно один вызов**, в `run()` до всего остального, с каталогом
`%APPDATA%\ProxyPilot\logs`. Страж живёт до конца `run`.

**Меню** (`Мост 127.0.0.1:3129 · …` заголовком):
заголовок (выключенный) · разделитель · Авто / SOCKS5 · доступен / HTTP ·
доступен / Напрямую (`CheckMenuItem` с галочкой на текущем) · разделитель ·
«Копировать адрес моста» · разделитель · «Выход».
При понижении заголовок читается `SOCKS5 недоступен → работаем напрямую`.
Секций сети и туннеля нет — они в плане 3.
Индикатор различает «недоступен» (чинится сетью) и «не задан» (чинится
настройкой): одинаковая подпись отправила бы человека чинить не то.

**Иконка** — `Icon::from_rgba`, 32×32, рисуется программно, без файлов
ресурсов. Четыре состояния: `Socks` (сплошной зелёный диск), `Http` (синее
кольцо), `Direct` (серое тонкое кольцо), `Unconfigured` (янтарное кольцо с
диагональной прорезью). Различаются и цветом, и формой.

Из пяти состояний брифа реализованы четыре: «мост не запущен» в этой
архитектуре недостижимо, **потому что в этом состоянии приложение выходит**.
До трея дело не доходит, если не удался `bind`; а если мост перестал
принимать соединения уже после старта, `spawn_bridge_watch` просит главный
поток выйти (см. правку по FINDING 2). Первая редакция этого отчёта
объясняла недостижимость тем, что «процесс уже не сможет ничего показать» —
это было неверно: процесс и трей остаются вполне работоспособны, и ровно
поэтому потребовался явный выход. Заводить вариант перечисления, который
невозможно выбрать, — мёртвый код.

**Инварианты соблюдены:** слушатель на `127.0.0.1`, привязывается один раз;
`Router::get()` по-прежнему имеет ровно один нетестовый вызов
(`serve.rs::pick_route`) — трей читает `AppState`, который супервизор
складывает в `ArcSwap`; UAC не запрашивается (всё в HKCU).

---

## 2. TDD: падающий прогон до реализации

Модули созданы с телами `unimplemented!()`, чтобы тесты компилировались и
падали в рантайме, а не в компиляторе. Полный вывод `cargo test -p
proxypilot-app`:

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src\main.rs (target\debug\deps\proxypilot-037a7a6129284e09.exe)
error: test failed, to rerun pass `-p proxypilot-app --bin proxypilot`

running 13 tests
test icons::tests::every_icon_is_a_full_rgba_buffer ... FAILED
test icons::tests::a_deliberate_direct_mode_is_not_unconfigured ... FAILED
test icons::tests::nothing_configured_gets_its_own_icon ... FAILED
test icons::tests::icons_differ_from_each_other ... FAILED
test proxy::tests::a_disabled_pointer_at_our_address_is_not_stale ... FAILED
test proxy::tests::localhost_by_name_is_ours_as_well ... FAILED
test proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected ... FAILED
test proxy::tests::our_address_on_another_port_is_not_ours ... FAILED
test proxy::tests::the_per_protocol_form_is_recognised_too ... FAILED
test tray::tests::header_explains_a_demotion_rather_than_hiding_it ... FAILED
test tray::tests::header_names_the_bridge_and_the_route ... FAILED
test tray::tests::header_names_the_upstream_it_actually_uses ... FAILED
test icons::tests::icon_reflects_the_active_route ... FAILED

failures:

---- icons::tests::every_icon_is_a_full_rgba_buffer stdout ----

thread 'icons::tests::every_icon_is_a_full_rgba_buffer' (31412) panicked at crates\app\src\icons.rs:21:5:
not implemented

---- icons::tests::a_deliberate_direct_mode_is_not_unconfigured stdout ----

thread 'icons::tests::a_deliberate_direct_mode_is_not_unconfigured' (32420) panicked at crates\app\src\icons.rs:17:5:
not implemented
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- icons::tests::nothing_configured_gets_its_own_icon stdout ----

thread 'icons::tests::nothing_configured_gets_its_own_icon' (24664) panicked at crates\app\src\icons.rs:17:5:
not implemented

---- icons::tests::icons_differ_from_each_other stdout ----

thread 'icons::tests::icons_differ_from_each_other' (19460) panicked at crates\app\src\icons.rs:21:5:
not implemented

---- proxy::tests::a_disabled_pointer_at_our_address_is_not_stale stdout ----

thread 'proxy::tests::a_disabled_pointer_at_our_address_is_not_stale' (36560) panicked at crates\app\src\proxy.rs:6:5:
not implemented

---- proxy::tests::localhost_by_name_is_ours_as_well stdout ----

thread 'proxy::tests::localhost_by_name_is_ours_as_well' (24852) panicked at crates\app\src\proxy.rs:6:5:
not implemented

---- proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected stdout ----

thread 'proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected' (24620) panicked at crates\app\src\proxy.rs:6:5:
not implemented

---- proxy::tests::our_address_on_another_port_is_not_ours stdout ----

thread 'proxy::tests::our_address_on_another_port_is_not_ours' (13704) panicked at crates\app\src\proxy.rs:6:5:
not implemented

---- proxy::tests::the_per_protocol_form_is_recognised_too stdout ----

thread 'proxy::tests::the_per_protocol_form_is_recognised_too' (30040) panicked at crates\app\src\proxy.rs:6:5:
not implemented

---- tray::tests::header_explains_a_demotion_rather_than_hiding_it stdout ----

thread 'tray::tests::header_explains_a_demotion_rather_than_hiding_it' (7640) panicked at crates\app\src\tray.rs:6:5:
not implemented

---- tray::tests::header_names_the_bridge_and_the_route stdout ----

thread 'tray::tests::header_names_the_bridge_and_the_route' (34800) panicked at crates\app\src\tray.rs:6:5:
not implemented

---- tray::tests::header_names_the_upstream_it_actually_uses stdout ----

thread 'tray::tests::header_names_the_upstream_it_actually_uses' (32084) panicked at crates\app\src\tray.rs:6:5:
not implemented

---- icons::tests::icon_reflects_the_active_route stdout ----

thread 'icons::tests::icon_reflects_the_active_route' (4500) panicked at crates\app\src\icons.rs:17:5:
not implemented


failures:
    icons::tests::a_deliberate_direct_mode_is_not_unconfigured
    icons::tests::every_icon_is_a_full_rgba_buffer
    icons::tests::icon_reflects_the_active_route
    icons::tests::icons_differ_from_each_other
    icons::tests::nothing_configured_gets_its_own_icon
    proxy::tests::a_disabled_pointer_at_our_address_is_not_stale
    proxy::tests::localhost_by_name_is_ours_as_well
    proxy::tests::our_address_on_another_port_is_not_ours
    proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected
    proxy::tests::the_per_protocol_form_is_recognised_too
    tray::tests::header_explains_a_demotion_rather_than_hiding_it
    tray::tests::header_names_the_bridge_and_the_route
    tray::tests::header_names_the_upstream_it_actually_uses

test result: FAILED. 0 passed; 13 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Красный прогон — 13 тестов; в итоговой сборке их 16 (а после правок по
ревью — 17). Разница появилась по ходу реализации: `is_stale_pointer` оброс
случаями (`the_real_corporate_setting_of_this_machine_is_left_alone`), а
`mode_label` и `bridge_address` получили свои тесты, когда были написаны.

**Два** теста в `core` (`the_saved_system_proxy_survives_a_roundtrip` и
`managing_the_system_proxy_is_on_by_default_and_switchable`) добавлены вместе
с полями конфига — это не «чистая часть» из брифа, а сопровождение правки.

---

## 3. CI-команды

```
$ cargo test --all
     Running unittests src\main.rs (target\debug\deps\proxypilot-f9c0433b09311d11.exe)
running 16 tests
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)
running 59 tests
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s
     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)
running 47 tests
test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-382daa61fec08b04.exe)
running 22 tests
test result: ok. 21 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.12s
   Doc-tests proxypilot_bridge
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests proxypilot_core
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests proxypilot_winnet
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.23s

$ cargo fmt --all --check
(вывода нет — расхождений нет)
```

Итого 145 тестов, 1 `#[ignore]`. `#[allow]` не добавлялось.

---

## 4. Ручная проверка

### Исходное состояние машины (перед началом)

```
$ reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings" /v ProxyEnable
    ProxyEnable    REG_DWORD    0x0
$ ... /v ProxyServer
    ProxyServer    REG_SZ    203.0.113.10:3128
$ ... /v ProxyOverride
    ProxyOverride  REG_SZ    198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
$ ... /v AutoConfigURL
ERROR: The system was unable to find the specified registry key or value.
```

Совпадает с `proxy-settings-before-task6.txt`. Ключ также выгружен в
`reg export` перед началом работы.

### Стенд

Корпоративный прокси `203.0.113.10:3128` снаружи офиса недоступен
(`curl --connect-timeout 3 -x http://203.0.113.10:3128 …` → `000`), поэтому
апстримом для проверки поднят второй экземпляр моста:

```
$ ./target/release/proxypilot-bridge.exe --port 3130 --mode direct
$ curl -s -o /dev/null -w "%{http_code}\n" -x http://127.0.0.1:3130 https://example.com/
200
```

`%APPDATA%\ProxyPilot\config.toml`: `bridge_port = 3129`, `mode = "http"`,
`http_upstream = "127.0.0.1:3130"`, `manage_system_proxy = true`.

### Ограничение среды и как оно обойдено

Сеанс Windows во время проверки **заблокирован** (`OpenInputDesktop` →
`Default`, но переднее окно — «Экран блокировки Windows по умолчанию»).
Следствия:

* снимок экрана даёт чёрный кадр — иконка проверена не скриншотом, а через
  UI Automation, где она видна вместе со своей подсказкой (это даже точнее:
  видно и текст);
* `TrackPopupMenu` не может получить передний план, поэтому всплывающее меню
  физически не показывается — щёлкнуть по пункту мышью невозможно.

Пункты меню поэтому активированы **тем же сообщением, которое порождает
настоящий щелчок**: `tray-icon` вызывает `TrackPopupMenu` **без**
`TPM_RETURNCMD` (`platform_impl/windows/mod.rs:546`), то есть выбор пункта
приходит в окно трея как `WM_COMMAND` с идентификатором пункта, а обрабатывает
его подкласс `muda` (`menu_subclass_proc`, `WM_COMMAND` → `menu_selected` →
`MenuEvent::send`). Мы посылаем ровно этот `WM_COMMAND` извне: весь дальнейший
путь — `MenuEvent` → `Tray::action_for` → `Cmd::SetMode` → супервизор →
`Router` → перерисовка иконки — исполняется настоящий, наш.
Не воспроизведён только показ самого всплывающего меню (код `tray-icon`).

*(Побочное наблюдение: процесс, запущенный из песочницы Bash-инструмента, не
получает `PostMessage`/`PostThreadMessage` извне вообще — ни `WM_COMMAND`, ни
`WM_QUIT`. Проверки ниже сделаны на экземпляре, запущенном из PowerShell.)*

---

### 1. Приложение запускается, иконка появляется в трее

```
$ ./target/release/proxypilot.exe   (фоном)
$ cat %APPDATA%\ProxyPilot\logs\proxypilot.2026-08-30
2026-08-30T02:01:38.330690Z  INFO proxypilot: proxypilot запускается port=3129 mode=Http manage_system_proxy=true config=C:\Users\User\AppData\Roaming\ProxyPilot\config.toml
2026-08-30T02:01:38.347056Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Http("127.0.0.1:3130") place=Place { in_office: false, network: Some("{75C7A91B-EED3-4D6A-8669-E0449B108463}") } demoted=false
2026-08-30T02:01:38.347228Z  INFO proxypilot_bridge::serve: мост слушает addr=127.0.0.1:3129
2026-08-30T02:01:38.365669Z  INFO proxypilot::proxy: системный прокси направлен на мост server=127.0.0.1:3129

$ tasklist /FI "IMAGENAME eq proxypilot.exe"
proxypilot.exe               26908 Console                    2    24 104 КБ
```

Обратите внимание на порядок строк: **«маршрут изменён» → «мост слушает» →
«системный прокси направлен на мост»**. Ровно тот порядок старта, который
требовался.

Иконка в области уведомлений (перечисление через UI Automation, окно
переполнения `TopLevelWindowForOverflowXamlIsland`):

```
overflow descendants: 45
  [ControlType.Pane] '' class=Windows.UI.Input.InputSite.WindowClass
  [ControlType.Button] 'Мост 127.0.0.1:3129 · HTTP → 127.0.0.1:3130' class=SystemTray.NormalButton
  [ControlType.Image] '' class=Image
  [ControlType.Button] 'Spotify' class=SystemTray.NormalButton
  ...
```

Наша кнопка — первая в списке, с подсказкой-заголовком, где и адрес моста, и
фактический маршрут.

### 2. `curl -x http://127.0.0.1:3129 https://example.com/` → 200

```
$ curl -s -o /dev/null -w "%{http_code}\n" --connect-timeout 8 -x http://127.0.0.1:3129 https://example.com/
200
```

### 3. Переключение режима из меню меняет иконку, трафик продолжает идти

`WM_COMMAND` с идентификатором пункта «Напрямую» (муда-счётчик: 1000 —
заголовок, 1001 Авто, 1002 SOCKS5, 1003 HTTP, 1004 Напрямую, 1005 копировать,
1006 выход):

```
$ PostMessageW(hwnd_tray, WM_COMMAND, 1004, 0)
--- log ---
2026-08-30T02:14:06.115632Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Direct place=Place { in_office: false, network: Some("{75C7A91B-EED3-4D6A-8669-E0449B108463}") } demoted=false
2026-08-30T02:14:06.903831Z  INFO proxypilot::tray: иконка трея сменилась kind=Direct route=Direct
--- config ---
mode = "direct"
--- трафик ---
curl -x 127.0.0.1:3129 -> 200
--- подсказка иконки ---
TRAY: 'Мост 127.0.0.1:3129 · напрямую'
```

Понижение показывается, а не скрывается — выбираем SOCKS5, которого нет:

```
$ PostMessageW(hwnd_tray, WM_COMMAND, 1002, 0)
mode = "socks"
TRAY: 'Мост 127.0.0.1:3129 · SOCKS5 недоступен → работаем напрямую'
curl -x 127.0.0.1:3129 -> 200
```

Обратно в HTTP — иконка возвращается:

```
$ PostMessageW(hwnd_tray, WM_COMMAND, 1003, 0)
2026-08-30T02:14:51.661239Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Http("127.0.0.1:3130") place=Place { in_office: false, network: Some("{75C7A91B-EED3-4D6A-8669-E0449B108463}") } demoted=false
2026-08-30T02:14:52.110152Z  INFO proxypilot::tray: иконка трея сменилась kind=Http route=Http("127.0.0.1:3130")
curl -x 127.0.0.1:3129 -> 200
```

Пункт «Копировать адрес моста» отработал, но буфер обмена в заблокированном
сеансе недоступен **всему сеансу** — `Set-Clipboard` самого PowerShell падает
так же. Приложение при этом деградировало правильно:

```
2026-08-30T02:14:54.839293Z  WARN proxypilot::tray: не скопировать адрес в буфер обмена error=Отказано в доступе. (0x80070005)
```

### 4. Системный прокси указывает на нас, пока приложение работает

```
$ reg query ... /v ProxyEnable
    ProxyEnable    REG_DWORD    0x1
$ reg query ... /v ProxyServer
    ProxyServer    REG_SZ    127.0.0.1:3129
$ reg query ... /v ProxyOverride
    ProxyOverride  REG_SZ    localhost;127.0.0.1;::1;*.local;169.254.0.0/16;192.168.0.0/16;10.0.0.0/8;172.16.0.0/12;<local>
```

Исходные настройки при этом уже лежат на диске (записаны ДО реестра):

```
$ cat %APPDATA%\ProxyPilot\config.toml
...
[saved_sysproxy]
enabled = false
server = "203.0.113.10:3128"
bypass = "198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>"
```

### 5. «Выход» из меню возвращает системный прокси в исходное состояние

```
$ PostMessageW(hwnd_tray, WM_COMMAND, 1006, 0)      # пункт «Выход»
процессов proxypilot.exe: 0
--- log ---
2026-08-30T02:15:23.423027Z  INFO proxypilot: выход по команде пользователя
2026-08-30T02:15:23.427180Z  INFO proxypilot::proxy: системный прокси восстановлен enabled=false server=203.0.113.10:3128

$ reg query ... /v ProxyEnable
    ProxyEnable    REG_DWORD    0x0
$ reg query ... /v ProxyServer
    ProxyServer    REG_SZ    203.0.113.10:3128
$ reg query ... /v ProxyOverride
    ProxyOverride  REG_SZ    198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
```

### 6. После выхода машина не осталась без сети

```
$ curl -s -o NUL -w "%{http_code}\n" https://example.com/
200
$ Invoke-WebRequest https://example.com/       # .NET/WinINET — читает системные настройки
200
```

Второй вызов важнее первого: `curl` системные настройки прокси **не читает**
вовсе, поэтому сам по себе он ничего про реестр не доказывает. `Invoke-WebRequest`
их читает — и именно он падал, пока в реестре стоял мёртвый указатель (см.
ниже).

### Сверка с эталоном

```
ProxyEnable   : 0
ProxyServer   : 203.0.113.10:3128
ProxyOverride : 198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
AutoConfigURL :

  ProxyEnable   = 0                    : True
  ProxyServer   = 203.0.113.10:3128     : True
  ProxyOverride = корпоративный список : True
  AutoConfigURL отсутствует            : True
```

---

## 5. Дополнительные проверки

### Убитый процесс и распознавание своего следа

`taskkill /F` (восстановление невозможно по построению):

```
--- реестр после жёсткого убийства ---
    ProxyEnable    REG_DWORD    0x1
    ProxyServer    REG_SZ    127.0.0.1:3129

--- что видит клиент, читающий системные настройки ---
Invoke-WebRequest FAILED: Невозможно соединиться с удаленным сервером
curl direct: 200            # curl системные настройки не читает
```

Следующий запуск распознаёт свой след и НЕ принимает его за настройки
пользователя:

```
2026-08-30T02:13:40.194869Z  WARN proxypilot::proxy: в реестре остался наш адрес от прошлого запуска, исходные настройки берём из конфига server=127.0.0.1:3129
```

`saved_sysproxy` в конфиге после этого по-прежнему корпоративный
`203.0.113.10:3128`, а не наш адрес — именно это и требовалось.

### `manage_system_proxy = false`

```
2026-08-30T02:15:59.643979Z  INFO proxypilot: proxypilot запускается port=3129 mode=Http manage_system_proxy=false ...
2026-08-30T02:15:59.659689Z  INFO proxypilot: manage_system_proxy = false: системные настройки не трогаем
--- реестр ---
ProxyEnable=0  ProxyServer=203.0.113.10:3128     (нетронут)
мост всё равно работает: 200
--- после Stop-Process -Force ---
ProxyEnable=0  ProxyServer=203.0.113.10:3128     (по-прежнему нетронут)
```

### Закрытие окна консоли (путь, где `Drop` не вызывается)

```
$ PostMessageW(hwnd_console, WM_CLOSE, 0, 0)
процессов proxypilot: 0
2026-08-30T02:16:36.940922Z  INFO proxypilot::proxy: системный прокси восстановлен enabled=false server=203.0.113.10:3128
после закрытия консоли: ProxyEnable=0 ProxyServer=203.0.113.10:3128
```

### Уборка

Вспомогательный мост на 3130 остановлен, `%APPDATA%\ProxyPilot` удалён целиком
(до задачи его не существовало), полный лог прогона сохранён отдельно и
процитирован выше. Процессов `proxypilot*` не осталось. Реестр — как в эталоне.

---

## 6. Что осталось непроверенным и почему

| Что | Почему |
|---|---|
| Показ самого всплывающего меню и щелчок мышью по пункту | Сеанс Windows заблокирован; `TrackPopupMenu` не может получить передний план. Разблокировать сеанс нельзя — это требует пароля пользователя. Проверено всё, что после `WM_COMMAND`, тем же сообщением, которое порождает щелчок. |
| Скриншот иконки | Снимок экрана заблокированного сеанса — чёрный кадр. Заменён перечислением через UI Automation, где видна и кнопка, и её подсказка. |
| Буфер обмена | Недоступен всему сеансу при блокировке (падает и `Set-Clipboard` PowerShell). Код отработал и корректно записал предупреждение вместо падения. |
| Восстановление при панике | Опирается на `Drop for RestoreOnDrop` и раскрутку стека; искусственную панику в главном потоке не устраивал. |
| Реальный офисный SOCKS5/HTTP апстрим | `203.0.113.10:3128` снаружи офиса недоступен; апстримом служил второй экземпляр моста. |

---
---

# Правки по ревью (второй заход)

Две Critical, две Important и четыре мелких. Всё исправлено, ручная проверка
повторена целиком — обе Critical меняли пути выхода, поэтому прежний прогон
их больше не покрывал.

## FINDING 1 (Critical) — отказ `take_over` уносил из `run` мимо стража

Было: `?` возвращал из `run()` **до** создания `RestoreOnDrop`, а `main`
следом звал `std::process::exit(1)`. `sysproxy::apply` умеет отказать уже
после записи трёх значений — реестр изменён, `ORIGINAL` заполнен, а код,
который бы им воспользовался, не достигался никогда.

Стало (`main.rs`): страж и обработчик консоли ставятся **до** `take_over`.
Пока `ORIGINAL` пуст, `restore()` — no-op, так что ранний страж безвреден.

```rust
let _restore = cfg.manage_system_proxy.then(|| {
    install_console_handler();
    proxy::RestoreOnDrop
});

if cfg.manage_system_proxy {
    proxy::take_over(&mut cfg, port).map_err(|e| e.to_string())?;
} else {
    info!("manage_system_proxy = false: системные настройки не трогаем");
    warn_if_stale_pointer_left_behind(port);
}
```

### Проверка намеренным отказом

Во `winnet::sysproxy::apply` временно вставлена инъекция: отказ **после**
записи в реестр, ровно один раз (чтобы восстановление, идущее следом,
отработало по-настоящему). Инъекция снята до сборки коммита —
`git diff crates/winnet` пуст.

```
--- реестр ДО ---
ProxyEnable=0  ProxyServer=203.0.113.10:3128

$ PROXYPILOT_FAIL_AFTER_WRITE=1 proxypilot.exe
код возврата: 1

--- лог ---
2026-08-30T02:34:55.883605Z  INFO proxypilot: proxypilot запускается port=3129 mode=Http manage_system_proxy=true config=C:\Users\User\AppData\Roaming\ProxyPilot\config.toml
2026-08-30T02:34:57.918533Z  INFO proxypilot_bridge::serve: мост слушает addr=127.0.0.1:3129
2026-08-30T02:34:57.932990Z  INFO proxypilot::proxy: системный прокси восстановлен enabled=false server=203.0.113.10:3128

--- реестр ПОСЛЕ ---
ProxyEnable   = 0
ProxyServer   = 203.0.113.10:3128
ProxyOverride = 198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
```

`take_over` вернул `Err`, `run` вышел по `?` с кодом 1 — и системный прокси
всё равно восстановлен. До правки эта строка лога не появилась бы вовсе.

*(Первый прогон инъекции был не одноразовым, и тогда отказывал в том числе
`apply` внутри `restore` — в логе была строка «НЕ УДАЛОСЬ восстановить …»,
а значения в реестре всё равно оказались правильными, потому что инъекция
срабатывает уже после записи. Инъекцию сделали одноразовой, чтобы
доказательство читалось однозначно.)*

## FINDING 2 (Critical) — мост мог умереть, а приложение продолжало работать

Было: `JoinHandle` от `serve` выбрасывался. `serve` сдаётся сам после
`MAX_CONSECUTIVE_ACCEPT_ERRORS`, слушатель разрушается, порт закрывается — а
приложение оставалось с исправной иконкой и системным прокси, направленным в
пустоту. Паника в цикле приёма давала то же самое даже без строки в логе.

Стало: `JoinHandle` удерживается, `spawn_bridge_watch` его ждёт (`JoinHandle`,
а не результат `serve` — он ловит и панику) и посылает главному потоку
`WM_BRIDGE_STOPPED`. Цикл сообщений возвращает `Exit::BridgeStopped`, `run`
отдаёт `Err` — и выход идёт обычным путём, через `_restore`, а не через
`process::exit` из рантайма.

`message_loop` теперь возвращает причину выхода (`Exit::{User, SessionEnd,
BridgeStopped, MessageLoopFailed}`) вместо `()`.

### Проверка намеренной паникой в мосте

Временная инъекция в `main.rs`: задача моста роняет слушатель и паникует
через 8 секунд. Снята до сборки коммита.

```
--- реестр ДО ---
ProxyEnable=0 ProxyServer=203.0.113.10:3128

$ PROXYPILOT_KILL_BRIDGE=1 proxypilot.exe
через 4 с: процесс жив = True, прокси в реестре = 127.0.0.1:3129
после падения моста: процесс жив = False, код возврата = 1

--- лог ---
2026-08-30T02:35:24.082763Z  INFO proxypilot: proxypilot запускается port=3129 mode=Http manage_system_proxy=true config=C:\Users\User\AppData\Roaming\ProxyPilot\config.toml
2026-08-30T02:35:26.144552Z  INFO proxypilot::proxy: системный прокси направлен на мост server=127.0.0.1:3129
2026-08-30T02:35:34.130629Z ERROR proxypilot: мост упал error=task 17 panicked with message "инъекция: паника в цикле приёма"
2026-08-30T02:35:34.134626Z  INFO proxypilot::proxy: системный прокси восстановлен enabled=false server=203.0.113.10:3128

--- реестр ПОСЛЕ ---
ProxyEnable   = 0
ProxyServer   = 203.0.113.10:3128
ProxyOverride = 198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
```

## FINDING 3 (Important) — `manage_system_proxy = false` не замечал чужой след

Стало: в ветке «не трогаем» вызывается `warn_if_stale_pointer_left_behind` —
`sysproxy::read()` + `is_stale_pointer`, обе без побочных эффектов, и `error!`
с именем значения. Ничего не пишется.

```
--- убиваем процесс с включённым управлением ---
после taskkill /F: ProxyEnable=1 ProxyServer=127.0.0.1:3129

--- запуск с manage_system_proxy = false ---
2026-08-30T02:38:38.839016Z  INFO proxypilot: manage_system_proxy = false: системные настройки не трогаем
2026-08-30T02:38:38.839055Z  INFO proxypilot_bridge::serve: мост слушает addr=127.0.0.1:3129
2026-08-30T02:38:38.839087Z ERROR proxypilot: в системных настройках остался наш адрес от прошлого запуска, но управление системным прокси выключено — уберите значение ProxyServer вручную, иначе приложения, читающие настройки Windows, останутся без сети server=127.0.0.1:3129

реестр (нетронут): ProxyEnable=1 ProxyServer=127.0.0.1:3129
```

## FINDING 4 (Important) — восстановление затирало чужие изменения

Стало: `ORIGINAL` хранит не только снимок, но и порт (`struct Taken`), а
`restore()` перед записью читает текущее значение. Предикат расщеплён:

* `is_stale_pointer` = включён **и** указывает на нас (для старта);
* `server_points_at_port` = указывает на нас, без оглядки на выключатель —
  для восстановления, где снятая пользователем галочка не отменяет того, что
  адрес в реестре наш.

Если адрес уже не наш — `warn!` и ничего не пишем. Если `read()` отказал —
восстанавливаем вслепую: риск отменить чужую правку меньше риска оставить
машину без сети.

```
наш адрес в реестре: 127.0.0.1:3129
--- сторонняя правка (как это сделал бы GPO или сам пользователь) ---
стало: 10.1.2.3:8080
--- «Выход» из меню ---
2026-08-30T02:39:03.718681Z  INFO proxypilot: выход по команде пользователя
2026-08-30T02:39:03.720024Z  WARN proxypilot::proxy: системные настройки прокси изменились не нами — оставляем как есть current=10.1.2.3:8080 enabled=true
реестр после выхода (чужое изменение обязано уцелеть): 10.1.2.3:8080
```

Добавлен тест `a_pointer_at_us_is_recognised_even_with_the_switch_off`.

## Мелкие

* `tray.rs` — каждому разделителю свой `PredefinedMenuItem::separator()`.
* `main.rs` — комментарий, объясняющий, почему `cfg.clone()` для задачи
  супервизора обязан оставаться **после** `take_over`: снятый раньше клон не
  содержал бы `saved_sysproxy`, и первая же смена режима стёрла бы его с
  диска вместе с возможностью восстановиться после убийства процесса.
* `proxy.rs` — комментарий о долговечности больше не утверждает
  «либо целиком на носителе, либо файл не тронут вовсе»: `sync_all` покрывает
  временный файл, но запись в каталог, которую делает `rename`, не
  сбрасывается. Остаточное окно названо явно, вместе с ценой его закрытия.
* Отчёт: §2 говорил «три теста в core» — их два; добавлено объяснение
  разницы 13 → 16 тестов между красным прогоном и итогом. Рассуждение про
  пятую иконку исправлено: состояние недостижимо потому, что приложение
  выходит, а не потому, что «не смогло бы показать».

### Ещё одна мелочь, найденная при проверке

Сообщения с переносом через обратный слэш внутри однострочного вызова
`error!` rustfmt схлопывает в одну строку, не убирая отступы, — в лог уезжали
цепочки пробелов. Такие сообщения переписаны на `concat!`.

---

## Повторная ручная проверка (после правок)

Стенд тот же: вспомогательный мост `--port 3130 --mode direct` как апстрим,
`mode = "http"`, `manage_system_proxy = true`. Бинарник — тот, что уходит в
коммит.

**1. Запуск, иконка в трее**

```
2026-08-30T02:39:29.024614Z  INFO proxypilot: proxypilot запускается port=3129 mode=Http manage_system_proxy=true config=C:\Users\User\AppData\Roaming\ProxyPilot\config.toml
2026-08-30T02:39:29.040274Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Http("127.0.0.1:3130") place=Place { in_office: false, network: Some("{75C7A91B-EED3-4D6A-8669-E0449B108463}") } demoted=false
2026-08-30T02:39:29.040516Z  INFO proxypilot_bridge::serve: мост слушает addr=127.0.0.1:3129
2026-08-30T02:39:29.058899Z  INFO proxypilot::proxy: системный прокси направлен на мост server=127.0.0.1:3129

1. иконка:
  'Мост 127.0.0.1:3129 · HTTP → 127.0.0.1:3130'  class=SystemTray.NormalButton
```

Порядок строк прежний и правильный: решение → слушатель → системный прокси.

**2. `curl -x` через мост**

```
  curl -x http://127.0.0.1:3129 -> 200
```

**3. Переключение режима из меню**

Полный набор (прогон непосредственно перед финальным):

```
=== «Напрямую» (WM_COMMAND 1004) ===
2026-08-30T02:36:33.798682Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Direct ... demoted=false
2026-08-30T02:36:33.800738Z  INFO proxypilot::tray: иконка трея сменилась kind=Direct route=Direct
  TRAY: 'Мост 127.0.0.1:3129 · напрямую'
  трафик: 200
=== «SOCKS5» (не настроен) — понижение видно ===
  TRAY: 'Мост 127.0.0.1:3129 · SOCKS5 недоступен → работаем напрямую'
  трафик: 200
=== обратно «HTTP» ===
2026-08-30T02:36:40.282086Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Http("127.0.0.1:3130") ... demoted=false
2026-08-30T02:36:40.283662Z  INFO proxypilot::tray: иконка трея сменилась kind=Http route=Http("127.0.0.1:3130")
  TRAY: 'Мост 127.0.0.1:3129 · HTTP → 127.0.0.1:3130'
  трафик: 200
```

И на финальном бинарнике:

```
3. переключение режима из меню:
2026-08-30T02:39:54.308519Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Direct ... demoted=false
2026-08-30T02:39:55.093812Z  INFO proxypilot::tray: иконка трея сменилась kind=Direct route=Direct
  трафик после переключения: 200
```

**4. Системный прокси указывает на нас**

```
  ProxyEnable=1 ProxyServer=127.0.0.1:3129
```

**5. «Выход» из меню возвращает исходное**

```
5. «Выход» из меню:
  процессов proxypilot: 0
2026-08-30T02:39:57.459366Z  INFO proxypilot: выход по команде пользователя
2026-08-30T02:39:57.463198Z  INFO proxypilot::proxy: системный прокси восстановлен enabled=false server=203.0.113.10:3128
  ProxyEnable   = 0
  ProxyServer   = 203.0.113.10:3128
  ProxyOverride = 198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
```

**6. Машина не осталась без сети**

```
  curl без -x -> 200
  Invoke-WebRequest -> 200
сверка с эталоном: True
```

`сверка с эталоном: True` — это одновременная проверка всех четырёх значений
против `proxy-settings-before-task6.txt`: `ProxyEnable = 0`,
`ProxyServer = 203.0.113.10:3128`, полный корпоративный `ProxyOverride` и
отсутствующий `AutoConfigURL`.

### Уборка

Вспомогательный мост остановлен, `%APPDATA%\ProxyPilot` удалён,
`git diff crates/winnet crates/core` пуст (инъекции сняты), процессов
`proxypilot*` не осталось.

---

## CI после правок

```
$ cargo test --all
     Running unittests src\main.rs (target\debug\deps\proxypilot-f9c0433b09311d11.exe)
running 17 tests
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)
running 59 tests
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.06s
     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)
running 47 tests
test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-382daa61fec08b04.exe)
running 22 tests
test result: ok. 21 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s
   Doc-tests proxypilot_bridge
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests proxypilot_core
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests proxypilot_winnet
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo clippy --all-targets -- -D warnings
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.12s

$ cargo fmt --all --check
(вывода нет — расхождений нет)
```

146 тестов, 1 `#[ignore]`. `#[allow]` по-прежнему нет.

## Что по-прежнему не проверено

Список из §6 не изменился (сеанс заблокирован: сам показ всплывающего меню,
скриншот иконки, буфер обмена; настоящий офисный апстрим). Восстановление при
панике главного потока теперь косвенно подтверждено: FINDING 1 доказал, что
`Drop` стража отрабатывает на выходе из `run` по ошибке, а это тот же путь
раскрутки стека.

---
---

# Правки по ревью (третий заход)

## FINDING 6 (внесена предыдущей правкой) — уведомление о смерти моста терялось

`spawn_bridge_watch` посылал `WM_BRIDGE_STOPPED` один раз и выбрасывал
результат, а решение принималось по `msg.message`. `tray-icon` показывает
меню, вызывая `TrackPopupMenu` прямо из оконной процедуры — то есть ВНУТРИ
нашего `DispatchMessageW`; пока меню открыто, крутится вложенный цикл
сообщений, и потоковые сообщения (`hwnd == 0`, а `PostThreadMessageW` шлёт
именно такие) он извлекает и выбрасывает: диспетчеризовать их некуда. Смерть
моста ровно в этот момент теряла уведомление навсегда — то самое состояние,
ради устранения которого заводился FINDING 2.

**Стало.** Решение принимается по флагу, сообщение только будит:

```rust
static BRIDGE_STOPPED: AtomicBool = AtomicBool::new(false);

// сторож:
BRIDGE_STOPPED.store(true, Ordering::Release);   // сначала флаг,
post_to_main(main_thread, WM_BRIDGE_STOPPED);    // потом побудка
```

Цикл перечитывает флаг в трёх местах на витке:

1. до блокирующего `GetMessageW` — мост мог умереть ещё до входа в цикл;
2. сразу после `GetMessageW` — обычный, быстрый путь;
3. **сразу после `DispatchMessageW`** — здесь заканчивается вложенный цикл
   всплывающего меню, и здесь ловится смерть, случившаяся, пока меню было
   открыто, а побудку съел этот вложенный цикл.

`WM_STATE_CHANGED` оставлен как есть: его потеря безобидна — следующее
событие всё равно перерисует иконку.

### Проверка 6.1 — побудка не послана вовсе

Инъекции: мост паникует через 8 с; `post_to_main` для `WM_BRIDGE_STOPPED`
подавлен (`PROXYPILOT_DROP_WAKE`). Это точный аналог «сообщение съедено
вложенным циклом».

```
--- реестр ДО ---
ProxyEnable=0 ProxyServer=203.0.113.10:3128

через 14 с (мост уже умер, побудка НЕ послана): процесс жив = True
  реестр сейчас: 127.0.0.1:3129
2026-08-30T02:52:41.438888Z  INFO proxypilot::proxy: системный прокси направлен на мост server=127.0.0.1:3129
2026-08-30T02:52:49.437176Z ERROR proxypilot: мост упал error=task 17 panicked with message "инъекция: паника в цикле приёма"
2026-08-30T02:52:49.437219Z ERROR proxypilot: инъекция: побудка WM_BRIDGE_STOPPED НЕ послана
```

Цикл спит в `GetMessageW`, будить его нечем — ожидаемо. Будим посторонним
сообщением (`WM_NULL`) — ровно так цикл просыпается, когда вложенный цикл
меню закончился, а съеденной побудки уже нет:

```
процессов proxypilot: 0
2026-08-30T02:53:12.954392Z  INFO proxypilot::proxy: системный прокси восстановлен enabled=false server=203.0.113.10:3128
ProxyEnable   = 0
ProxyServer   = 203.0.113.10:3128
ProxyOverride = 198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
```

Решение принято по флагу — сообщения `WM_BRIDGE_STOPPED` не было вообще.

### Проверка 6.2 — настоящий вложенный цикл меню

Ревью просило проверить «с открытым меню, если получится». **Получилось —
неожиданным образом.** Заблокированный сеанс не даёт `TrackPopupMenu`
показать окно меню (`#32768` не появился ни разу за 6 с наблюдения), но
модальный цикл при этом ЗАПУСКАЕТСЯ и исправно ест потоковые сообщения. То
есть сценарий FINDING 6 воспроизвёлся по-настоящему, просто без картинки.

Прогон: мост паникует через 8 с, побудка посылается штатно; в момент
t≈3 с посылаем `WM_USER_TRAYICON` + `WM_RBUTTONUP`, чтобы `tray-icon`
вызвал `TrackPopupMenu`.

```
просим показать меню (WM_USER_TRAYICON + WM_RBUTTONUP), мост умрёт через 5 с
  всплывающее меню НЕ появилось ни разу за 6 с наблюдения
после смерти моста: процесс жив = True
2026-08-30T02:53:38.611539Z  INFO proxypilot::proxy: системный прокси направлен на мост server=127.0.0.1:3129
2026-08-30T02:53:46.596524Z ERROR proxypilot: мост упал error=task 17 panicked with message "инъекция: паника в цикле приёма"
ProxyEnable=1 ProxyServer=127.0.0.1:3129
```

Побудка послана — и съедена вложенным циклом, как и предсказано. Приложение
ждёт: выходить изнутри чужого модального цикла нельзя, и замысел именно
такой. Закрываем меню (`WM_CANCELMODE`) — вложенный цикл заканчивается,
управление возвращается из `DispatchMessageW`, флаг перечитывается:

```
закрываем меню (WM_CANCELMODE) — вложенный цикл заканчивается:
процессов proxypilot: 0
2026-08-30T02:54:46.538834Z  INFO proxypilot::proxy: системный прокси восстановлен enabled=false server=203.0.113.10:3128
ProxyEnable   = 0
ProxyServer   = 203.0.113.10:3128
```

### Проверка 6.3 — контроль на коде ДО правки

Тот же сценарий на бинарнике, собранном из `394524c` (правка FINDING 2 есть,
правки FINDING 6 нет; `grep -c 'BRIDGE_STOPPED.load'` = 0):

```
=== контроль: код ДО правки FINDING 6 ===
открываем меню (вложенный цикл), мост умрёт через 5 с
мост уже мёртв. процесс жив = True, реестр = 127.0.0.1:3129
закрываем меню (WM_CANCELMODE):
после закрытия меню: процесс жив = True
реестр: 127.0.0.1:3129
2026-08-30T02:56:02.891310Z  INFO proxypilot::proxy: системный прокси направлен на мост server=127.0.0.1:3129
2026-08-30T02:56:10.879847Z ERROR proxypilot: мост упал error=task 17 panicked with message "инъекция: паника в цикле приёма"
```

До правки приложение продолжало работать с закрытым портом и захваченным
реестром **и после закрытия меню** — навсегда. Находка была настоящей.
(Контрольный экземпляр снят `Stop-Process -Force`, реестр после него
восстановлен вручную командой `Set-ItemProperty`; это делалось до финального
прогона, см. ниже.)

## FINDING 6b — формулировка сообщения при пропуске восстановления

`restore()` пропускает запись не только когда значение поменял кто-то другой,
но и когда НАША запись не легла вовсе (первый же `set_string` в `apply`
отказал, и в реестре по-прежнему значение пользователя). Поведение одно и то
же и верное — ничего не писать, — но текст утверждал причину, которой знать
не может.

```diff
-"системные настройки прокси изменились не нами — оставляем как есть"
+"системные настройки прокси не указывают на нас — оставляем как есть"
```

Рядом добавлен комментарий, называющий обе ситуации.

## Отложено (записано, не чинится в этом заходе)

Обработчик закрытия консоли теперь ставится ДО `take_over`, поэтому Ctrl+C,
пришедший ровно внутри `take_over`, может переплести `restore()` с `apply()`:
обработчик исполняется на отдельном потоке, а `ORIGINAL` к тому моменту уже
заполнен. Окно — микросекунды между заполнением `ORIGINAL` и возвратом из
`apply`; худший исход — две записи в реестр в неопределённом порядке, причём
обе из нашего же кода и обе с осмысленными значениями. До правки этот
промежуток не обрабатывался вообще (обработчик ещё не стоял, и Ctrl+C уносил
процесс, оставляя реестр захваченным), так что регрессии нет — но и
закрытым вопрос считать нельзя. Правильное лечение — мьютекс, сериализующий
`take_over` и `restore`; отложено, чтобы не тащить в этот заход ещё одну
правку в самый опасный модуль.

---

## Повторная ручная проверка (после правки FINDING 6)

Стенд прежний, бинарник — тот, что уходит в коммит; в дереве не осталось ни
одной инъекции (`grep -c ИНЪЕКЦИЯ` = 0 в `main.rs` и `sysproxy.rs`,
`git diff crates/winnet crates/core` пуст).

```
апстрим 3130: 200
=== 1. запуск ===
2026-08-30T02:57:19.827041Z  INFO proxypilot: proxypilot запускается port=3129 mode=Http manage_system_proxy=true config=C:\Users\User\AppData\Roaming\ProxyPilot\config.toml
2026-08-30T02:57:19.842944Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Http("127.0.0.1:3130") place=Place { in_office: false, network: Some("{75C7A91B-EED3-4D6A-8669-E0449B108463}") } demoted=false
2026-08-30T02:57:19.843121Z  INFO proxypilot_bridge::serve: мост слушает addr=127.0.0.1:3129
2026-08-30T02:57:19.861127Z  INFO proxypilot::proxy: системный прокси направлен на мост server=127.0.0.1:3129
  иконка: 'Мост 127.0.0.1:3129 · HTTP → 127.0.0.1:3130'
=== 2. трафик ===
  curl -x http://127.0.0.1:3129 -> 200
=== 3. режим из меню ===
2026-08-30T02:57:24.146526Z  INFO proxypilot_bridge::supervisor: маршрут изменён route=Direct place=Place { in_office: false, network: Some("{75C7A91B-EED3-4D6A-8669-E0449B108463}") } demoted=false
2026-08-30T02:57:24.148419Z  INFO proxypilot::tray: иконка трея сменилась kind=Direct route=Direct
  трафик после переключения: 200
=== 4. системный прокси во время работы ===
  ProxyEnable=1 ProxyServer=127.0.0.1:3129
=== 5. Выход из меню ===
  процессов: 0
2026-08-30T02:57:27.294300Z  INFO proxypilot: выход по команде пользователя
2026-08-30T02:57:27.298573Z  INFO proxypilot::proxy: системный прокси восстановлен enabled=false server=203.0.113.10:3128
  ProxyEnable   = 0
  ProxyServer   = 203.0.113.10:3128
  ProxyOverride = 198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
=== 6. сеть после выхода ===
  curl без -x -> 200
  Invoke-WebRequest -> 200
сверка с эталоном: True
```

Уборка: вспомогательный мост остановлен, `%APPDATA%\ProxyPilot` удалён,
процессов `proxypilot*` не осталось.

---

## CI после правки FINDING 6

```
$ cargo test --all
     Running unittests src\main.rs (target\debug\deps\proxypilot-f9c0433b09311d11.exe)
running 17 tests
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)
running 59 tests
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s
     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)
running 47 tests
test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-382daa61fec08b04.exe)
running 22 tests
test result: ok. 21 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s
   Doc-tests proxypilot_bridge
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests proxypilot_core
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests proxypilot_winnet
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo clippy --all-targets -- -D warnings
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.90s

$ cargo fmt --all --check
(вывода нет — расхождений нет)
```

146 тестов, 1 `#[ignore]`, `#[allow]` нет.


---

> **Примечание при публикации.** Файл `proxy-settings-before-task6.txt`, на который
> ссылается этот отчёт, был снимком реальных настроек прокси рабочей машины и в
> публичный репозиторий не попал. Внутренние адреса и имена хостов по всему
> репозиторию заменены на документационные (RFC 5737: `203.0.113.0/24`,
> `198.51.100.0/24`) и `example.internal`.
