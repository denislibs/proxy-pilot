# Задача 6 плана 3 — автозапуск. Отчёт

**База:** `8660eed` (ветка `feat/windows-rust`)
**Тесты:** 246 проходят + 1 `#[ignore]` (было 238 + 1 — восемь новых тестов в `winnet::autostart`).
**Коммит:** `cfc16bb` — `feat(win): автозапуск через HKCU\...\Run`

---

## Что сделано

### `win/crates/winnet/src/autostart.rs` (новый файл)

Три функции по интерфейсу брифа:

```rust
pub fn is_enabled() -> Result<bool, WinNetError>
pub fn enable(exe: &Path) -> Result<(), WinNetError>
pub fn disable() -> Result<(), WinNetError>
```

Ключ `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, значение `ProxyPilot`.
`HKCU`, не `HKLM` — второй требует администратора, а весь план обходится без
UAC.

- `enable` пишет путь `exe` **в кавычках** (`quote()`): без них Windows
  разбирает `C:\Program Files\...` по первому пробелу.
- `is_enabled` не просто проверяет наличие значения — она сравнивает его
  (через приватную `points_at`) с `env::current_exe()` в том же
  представлении, в каком мы его пишем. Если exe перенесли/переустановили,
  значение в реестре указывает в никуда, и `is_enabled` честно вернёт
  `false`, а не соврёт «включено».
- `disable` удаляет значение; отсутствие значения — не ошибка (идемпотентно).

**Причина не переиспользовать `sysproxy::RegKey` (см. глобальное
ограничение «reuse if it fits»):** тип `RegKey` в `sysproxy.rs` приватен
своему модулю (не `pub`, не `pub(crate)`) и его `open()` жёстко привязан к
одному захардкоженному подключу (`Internet Settings`) — сигнатура не
принимает subkey. Чтобы использовать его отсюда, пришлось бы менять
видимость и сигнатуру уже проверенного, покрытого 9 тестами файла ради
единственного второго потребителя, который к тому же не в списке файлов
брифа. Вместо этого `autostart.rs` заводит свою `RegKey` — тот же приём
(`Drop` закрывает хендл на любом выходе, `RegQueryValueExW` вызывается
дважды: за размером, потом за данными; та же пара `decode_utf16_sz`/
`encode_utf16_sz`), но другой, обособленный тип.

Каждый `unsafe`-блок несёт `// SAFETY:` — по образцу `sysproxy.rs`.

### `win/crates/winnet/src/lib.rs`

- `pub mod autostart;` добавлен в алфавитном порядке перед `com` (уже было:
  `com, events, networks, sysproxy` → стало `autostart, com, events,
  networks, sysproxy`).
- Новый вариант ошибки:
  ```rust
  #[error("не удалось определить путь к своему исполняемому файлу: {0}")]
  CurrentExe(#[from] std::io::Error),
  ```
  Нужен, потому что `is_enabled()` сама зовёт `env::current_exe()` (интерфейс
  не даёт ей параметра, в отличие от `enable`), а это может вернуть
  `io::Error`.

### `win/crates/app/src/main.rs`

Добавлен адаптер `WinAutostart`, реализующий `settings_page::Autostart`
поверх `winnet::autostart` (тот же приём, что и у `NlmSource` чуть выше по
файлу — реализация трейта живёт в приложении, а не в `winnet`, потому что
`winnet` не знает о странице настроек):

```rust
struct WinAutostart;

impl settings_page::Autostart for WinAutostart {
    fn is_enabled(&self) -> Result<bool, String> {
        autostart::is_enabled().map_err(|e| e.to_string())
    }

    fn set(&self, on: bool) -> Result<(), String> {
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            autostart::enable(&exe).map_err(|e| e.to_string())
        } else {
            autostart::disable().map_err(|e| e.to_string())
        }
    }
}
```

В `open_settings` заглушка `Arc::new(settings_page::AutostartPending)`
заменена на `Arc::new(WinAutostart)`. Разметка и разбор формы в
`settings_page.rs` не тронуты — ровно как и обещал докблок трейта.

### `win/crates/app/src/settings_page.rs` (правка сверх списка файлов брифа — см. ниже, почему)

`AutostartPending` и его `impl Autostart` спрятаны за `#[cfg(test)]`.
Причина: после того как `main.rs` стал использовать `WinAutostart`,
`AutostartPending` в рабочей сборке (`proxypilot-app` — это только `[[bin]]`,
без `lib`-цели, так что `pub` наружу ничего не экспортирует) стал мёртвым
кодом — `cargo build` тут же дал `warning: struct AutostartPending is never
constructed`, что `cargo clippy -D warnings` превратило бы в отказ сборки.
Решение — гейт `#[cfg(test)]`, а не удаление: структура по-прежнему нужна
как тестовая заглушка для `state_with()` в тестах страницы (в частности,
для теста `the_autostart_toggle_says_it_is_not_wired_yet_instead_of_pretending`,
который проверяет, что ошибка трейта показывается честно, а не молчаливо
скрывается). Заодно поправлены докблок трейта `Autostart` и комментарий
внутри этого теста — они ссылались на «задачу 6», которая уже выполнена.
Разметка `<h2>`, разбор формы, `checkbox()`, `apply_autostart()` — не
тронуты.

---

## TDD-свидетельство

### RED (до реализации)

`lib.rs` уже содержал `pub mod autostart;`, а `autostart.rs` содержал
только модульный докблок и `#[cfg(test)] mod tests` со всеми целевыми
тестами (`quote`, `points_at`, `encode_utf16_sz`/`decode_utf16_sz`,
`enable`/`is_enabled`/`disable`) — без единой реализации, как и предписано
брифом («если бы RED остановился на "файл не найден" — завести пустой
модуль, чтобы дойти до ошибок типов»).

Команда: `cargo test -p proxypilot-winnet autostart`

Полный, дословный вывод:

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
error[E0425]: cannot find function `encode_utf16_sz` in this scope
   --> crates\winnet\src\autostart.rs:48:17
    |
 48 |         let b = encode_utf16_sz("ab");
    |                 ^^^^^^^^^^^^^^^ not found in this scope
    |
note: function `crate::sysproxy::encode_utf16_sz` exists but is inaccessible
   --> crates\winnet\src\sysproxy.rs:231:1
    |
231 | fn encode_utf16_sz(s: &str) -> Vec<u8> {
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ not accessible

error[E0425]: cannot find function `decode_utf16_sz` in this scope
   --> crates\winnet\src\autostart.rs:55:13
    |
 55 |             decode_utf16_sz(&encode_utf16_sz(r#""C:\ProxyPilot\proxypilot.exe""#)),
    |             ^^^^^^^^^^^^^^^ not found in this scope
    |
note: function `crate::sysproxy::decode_utf16_sz` exists but is inaccessible
   --> crates\winnet\src\sysproxy.rs:217:1
    |
217 | fn decode_utf16_sz(bytes: &[u8]) -> String {
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ not accessible

error[E0425]: cannot find function `encode_utf16_sz` in this scope
   --> crates\winnet\src\autostart.rs:55:30
    |
 55 |             decode_utf16_sz(&encode_utf16_sz(r#""C:\ProxyPilot\proxypilot.exe""#)),
    |                              ^^^^^^^^^^^^^^^ not found in this scope
    |
note: function `crate::sysproxy::encode_utf16_sz` exists but is inaccessible
   --> crates\winnet\src\sysproxy.rs:231:1
    |
231 | fn encode_utf16_sz(s: &str) -> Vec<u8> {
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ not accessible

error[E0425]: cannot find function `decode_utf16_sz` in this scope
  --> crates\winnet\src\autostart.rs:58:20
   |
 58 |         assert_eq!(decode_utf16_sz(&[]), "");
   |                    ^^^^^^^^^^^^^^^ not found in this scope
   |
note: function `crate::sysproxy::decode_utf16_sz` exists but is inaccessible
   --> crates\winnet\src\sysproxy.rs:217:1
   |
217 | fn decode_utf16_sz(bytes: &[u8]) -> String {
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ not accessible

error[E0425]: cannot find function `quote` in this scope
  --> crates\winnet\src\autostart.rs:15:17
   |
15 |         let q = quote(Path::new(r"C:\Program Files\ProxyPilot\proxypilot.exe"));
   |                 ^^^^^ not found in this scope

error[E0425]: cannot find function `quote` in this scope
  --> crates\winnet\src\autostart.rs:22:23
   |
22 |         let written = quote(exe);
   |                       ^^^^^ not found in this scope

error[E0425]: cannot find function `points_at` in this scope
  --> crates\winnet\src\autostart.rs:23:17
   |
23 |         assert!(points_at(&Some(written), exe));
   |                 ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `points_at` in this scope
  --> crates\winnet\src\autostart.rs:29:18
   |
29 |         assert!(!points_at(&None, exe));
   |                  ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `quote` in this scope
  --> crates\winnet\src\autostart.rs:34:19
   |
34 |         let old = quote(Path::new(r"C:\ProxyPilot\old\proxypilot.exe"));
   |                   ^^^^^ not found in this scope

error[E0425]: cannot find function `points_at` in this scope
  --> crates\winnet\src\autostart.rs:36:18
   |
36 |         assert!(!points_at(&Some(old), new_exe));
   |                  ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `points_at` in this scope
  --> crates\winnet\src\autostart.rs:43:18
   |
43 |         assert!(!points_at(&Some(unquoted), exe));
   |                  ^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `enable` in this scope
  --> crates\winnet\src\autostart.rs:65:9
   |
65 |         enable(&exe).expect("enable обязан пройти без прав администратора");
   |         ^^^^^^ not found in this scope

error[E0425]: cannot find function `is_enabled` in this scope
  --> crates\winnet\src\autostart.rs:66:17
   |
66 |         assert!(is_enabled().expect("is_enabled обязан читаться"));
   |                 ^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `disable` in this scope
  --> crates\winnet\src\autostart.rs:67:9
   |
67 |         disable().expect("disable обязан пройти");
   |         ^^^^^^^ not found in this scope

error[E0425]: cannot find function `is_enabled` in this scope
  --> crates\winnet\src\autostart.rs:68:18
   |
68 |         assert!(!is_enabled().expect("is_enabled обязан читаться"));
   |                  ^^^^^^^^^^ not found in this scope

For more information about this error, try `rustc --explain E0425`.
error: could not compile `proxypilot-winnet` (lib test) due to 15 previous errors
```

### GREEN (после реализации)

Команда: `cargo test -p proxypilot-winnet autostart`

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.96s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-d431b601035c531c.exe)

running 8 tests
test autostart::tests::points_at_rejects_an_unquoted_value_pointing_at_the_same_exe ... ok
test autostart::tests::points_at_is_false_for_a_different_executable ... ok
test autostart::tests::decoding_drops_the_terminating_nul ... ok
test autostart::tests::points_at_is_false_when_registry_is_empty ... ok
test autostart::tests::quote_wraps_the_path_in_double_quotes ... ok
test autostart::tests::quoted_path_with_spaces_round_trips_through_points_at ... ok
test autostart::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test autostart::tests::enable_then_disable_round_trip_on_the_real_registry ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 23 filtered out; finished in 0.00s
```

Восьмой тест, `enable_then_disable_round_trip_on_the_real_registry`, —
не мок: он вправду пишет и читает `HKCU\...\Run` на этой машине, но
безопасно — сохраняет прежнее значение `ProxyPilot` (на момент запуска
теста его не было ни разу) и восстанавливает его в `Drop`-страже
независимо от исхода теста, так что параллельный прогон тестов или
падение теста не оставляют реестр в чужом состоянии.

---

## Прогон трёх команд CI (после реализации, полный workspace)

### `cargo test --all`

Итог по крейтам (полный вывод — 302 строки, здесь — сводка; каждый прогон
воспроизведён студентом лично, без сокращений видел выход целиком):

```
running 97 tests   (proxypilot-app)
test result: ok. 97 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.60s

running 69 tests   (proxypilot-bridge, lib)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

running 0 tests    (proxypilot-bridge, bin)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 2 tests    (proxypilot-bridge, tests/cli.rs)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

running 48 tests   (proxypilot-core)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 31 tests   (proxypilot-winnet)
test result: ok. 30 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s
  ↳ 1 ignored: events::tests::watch_a_real_network_change ("нужна живая сеть: переключить Wi-Fi руками") — существовал до этой задачи

Doc-tests: 0/0/0 по трём крейтам, всё ok
```

Итого: 97 + 69 + 0 + 2 + 48 + 30 = **246 passed, 0 failed, 1 ignored** (было
238 + 1 до задачи; прирост — ровно 8 новых тестов `autostart`).

Строка, где новые тесты видны внутри полного `--all`-прогона (тот же
файл, что выше):

```
test autostart::tests::points_at_is_false_for_a_different_executable ... ok
test autostart::tests::points_at_is_false_when_registry_is_empty ... ok
test autostart::tests::decoding_drops_the_terminating_nul ... ok
test autostart::tests::quoted_path_with_spaces_round_trips_through_points_at ... ok
test autostart::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test autostart::tests::enable_then_disable_round_trip_on_the_real_registry ... ok
test autostart::tests::points_at_rejects_an_unquoted_value_pointing_at_the_same_exe ... ok
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test autostart::tests::quote_wraps_the_path_in_double_quotes ... ok
```

### `cargo clippy --all-targets -- -D warnings`

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.23s
```

Пустой (кроме строки `Finished`), код выхода 0 — предупреждений нет.
(Первый прогон, до правки видимости `AutostartPending`, показал
`warning: struct AutostartPending is never constructed` при обычном
`cargo build --all`; после гейта `#[cfg(test)]` это исчезло, и полноценный
`clippy --all-targets -D warnings`, который компилирует тесты и раньше не
запускался, тоже чист.)

### `cargo fmt --all --check`

Первый прогон нашёл одно расхождение (перенос строк внутри
`RegQueryValueExW(...)` в `query_string`) — `cargo fmt --all` его
поправил. Повторный `cargo fmt --all --check`:

Пустой вывод, код выхода 0.

---

## Ручная проверка на реальном реестре

Автоматический тест `enable_then_disable_round_trip_on_the_real_registry`
уже гоняет живой реестр при каждом `cargo test`, но по чек-листу брифа
сделана отдельная, видимая проверка через временный `cargo example`
(`win/crates/winnet/examples/manual_autostart.rs`, удалён сразу после
проверки, в финальном дереве отсутствует).

### 1. Состояние `Run` ДО (сняли и записали дословно)

```
OpenVPN-GUI                                              : C:\Program Files\OpenVPN\bin\openvpn-gui.exe
Download Master                                          : C:\Program Files (x86)\Download Master\dmaster.exe -autorun
YandexDisk2                                              : C:\Users\User\AppData\Roaming\Yandex\YandexDisk2\3.2.50.5196\YandexDisk2.exe -autostart
Figma Agent                                              : "C:\Users\User\AppData\Local\FigmaAgent\figma_agent.exe"
Steam                                                    : "C:\Program Files (x86)\Steam\steam.exe" -silent
MicrosoftEdgeAutoLaunch_C46CFC0629905CC775E70B50EA8A519C : "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe" --win-session-start
YandexBrowserAutoLaunch_B64B7D5D07784CD66F00CA43360BB68B : "C:\Program Files\Yandex\YandexBrowser\Application\browser.exe" --shutdown-if-not-closed-by-system-restart
GoogleChromeAutoLaunch_BCEA24321E5E4F1401136BBEDFB545FE  : "C:\Program Files\Google\Chrome\Application\chrome.exe" --no-startup-window /prefetch:5
Docker Desktop                                           : C:\Program Files\Docker\Docker\Docker Desktop.exe
electron.app.Electron                                    : C:\Users\User\Downloads\proxy-switch\proxy-switch\node_modules\electron\dist\electron.exe --hidden
```

`ProxyPilot` — отсутствует. Базовый `is_enabled()` (через `example ...
check`): `Ok(false)`.

### 2. `enable(свой exe)`

```
$ ./target/debug/examples/manual_autostart.exe enable
enable(свой exe = C:\Users\User\Desktop\proxypilot\repo\win\target\debug\examples\manual_autostart.exe)
is_enabled() = Ok(true)
```

Считано значение реестра сразу после:

```
PS> Get-ItemPropertyValue ...\Run -Name "ProxyPilot"
"C:\Users\User\Desktop\proxypilot\repo\win\target\debug\examples\manual_autostart.exe"
PS> (Get-Item ...\Run).GetValueKind("ProxyPilot")
String
```

Путь в кавычках, тип `REG_SZ` (`String`) — как и требовалось.

### 3. `enable(чужой путь с пробелами)` — проверка честности `is_enabled`

```
$ ./target/debug/examples/manual_autostart.exe enable-foreign
enable(чужой exe = C:\Program Files\ProxyPilot\proxypilot.exe)
is_enabled() = Ok(false)
```

Реестр при этом:

```
PS> Get-ItemPropertyValue ...\Run -Name "ProxyPilot"
"C:\Program Files\ProxyPilot\proxypilot.exe"
```

Значение с пробелом внутри в кавычках сохранилось и прочиталось дословно
(кавычки не потерялись, не задвоились). `is_enabled()` вернула `false`,
потому что запись указывает не на реально запущенный `manual_autostart.exe` —
ровно тот сценарий «перенесли/переустановили exe», ради которого сверка
введена.

### 4. `disable()`

```
$ ./target/debug/examples/manual_autostart.exe disable
disable()
is_enabled() = Ok(false)
```

### 5. Состояние `Run` ПОСЛЕ

```
PS> Get-ItemPropertyValue ...\Run -Name "ProxyPilot" -ErrorAction Stop
<ошибка: значение не найдено, как и ожидалось>
```

Полный ключ `Run` после — посимвольно совпадает со снимком «ДО» (те же 10
записей, тот же порядок, те же значения); `ProxyPilot` отсутствует, как и
до начала проверки.

**Итог ручной проверки:** ключ `Run` вернулся к исходному состоянию,
никакая чужая запись не задета.

---

## Самопроверка по чек-листу задания

- **Путь в кавычках, и путь с пробелами round-trip'ится через enable →
  is_enabled?** Да — юнит-тест
  `quoted_path_with_spaces_round_trips_through_points_at` и ручная проверка
  шага 2 (путь `target\debug\examples\...` не содержит пробела, поэтому
  главное доказательство даёт шаг 3, где путь `C:\Program Files\...`
  записался и прочитался с сохранёнными кавычками).
- **`is_enabled` возвращает `false`, когда значение указывает на другой
  exe?** Да — юнит-тест `points_at_is_false_for_a_different_executable` и
  ручная проверка шага 3 (`enable-foreign` → `is_enabled() = Ok(false)`).
- **Ключ реестра закрывается на каждом пути, включая ошибки?** Да — `Drop`
  для `RegKey` закрывает хендл всегда; все три функции (`is_enabled`,
  `enable`, `disable`) открывают `RegKey` через `?`-раннюю ошибку, но объект
  `RegKey` создаётся успешно до того, как в него можно записать/прочитать,
  и его `Drop` отработает при любом выходе из содержащей его области
  видимости, панике включительно.
- **Отключение не задело другие записи `Run`?** Да — автоматический тест
  восстанавливает прежнее значение в `Drop`-страже, а ручная проверка
  (раздел выше) показывает побайтово идентичные снимки ключа до и после.
- **Тумблер на странице настроек теперь что-то делает и сообщает об
  ошибке, а не молчит?** Да — `main.rs` подключает `WinAutostart` вместо
  `AutostartPending` в единственной продакшн-точке создания
  `SettingsState` (`open_settings`); `apply_autostart` в
  `settings_page.rs` (не тронут этой задачей) уже пробрасывает ошибку
  `Autostart::set`/`is_enabled` в `Note { bad: true, ... }`, которую видит
  человек на странице.

---

## Отклонения от списка файлов брифа (и почему)

Бриф перечислял только `autostart.rs` (создать) и `lib.rs` (изменить), но
контекст задачи прямо требовал «Wire it» — подключить тумблер, который
`settings_page.rs` уже рендерит как нерабочую заглушку. Без правки
`main.rs` автозапуск остался бы нерабочим кодом без единого вызывающего.
Также пришлось точечно поправить `settings_page.rs` (`#[cfg(test)]` на
`AutostartPending`) — без этого `cargo clippy --all-targets -- -D
warnings` отказывал бы из-за мёртвого кода, появившегося как побочный
эффект подключения `WinAutostart`. Оба изменения не трогают ни разметку
страницы, ни разбор формы, ни правила `Config::validate` — то есть не
выходят за границу, которую бриф провёл для задачи 6.

---

## Что не проверено (честно)

CI по ветке ни разу не запускался (унаследованная пометка ещё из передачи
задачи 5) — все три команды прогнаны локально на этой машине. Тест
`enable_then_disable_round_trip_on_the_real_registry` предполагает, что
`HKCU\...\Run` открывается на запись без дополнительных ограничений
(групповая политика, антивирус) — на машинах с более жёсткой политикой
профиля это предположение не проверялось.

---

## Fix round 1 (после ревью коммита `cfc16bb`)

Ревьюер прогнал все три команды CI (246 тестов, clippy и fmt чисты), сам
перечитал ключ `Run` и подтвердил моё «до/после» побайтово, проверил, что
правка не касается инвариантов моста — и дал шесть находок. Разбираю по
одной; итоговый коммит — `e1bfa75`, поверх `cfc16bb`, ветка та же.

### 1. Critical — `points_at` был строже, чем «указывает на этот exe», и это переворачивало ту самую проблему, ради которой писался бриф

Согласен полностью, это была реальная критическая ошибка. `points_at`
(`autostart.rs`) сравнивал сырое значение реестра побайтово с
`quote(current_exe())`. Живая, рабочая запись `Run`, оставленная
инсталлятором или человеком руками (типично — без кавычек, возможно, в
другом регистре диска), читалась как «выключено». Хуже того:
`apply_autostart` в этом случае видит `Ok(false)` от `is_enabled()`, а
человек снял галочку (`wanted = false`) — ветка `Ok(current) if current ==
wanted => None` считает, что менять нечего, и НЕ пишет в реестр ничего.
Тумблер лжёт «выключено» при работающем автозапуске, а выключить его через
интерфейс нельзя вовсе — ровно та находка, которую подтвердил
руками ревьюер.

**Исправление** (`points_at`, `autostart.rs`): снимает одну пару
обрамляющих кавычек, если она есть, и сравнивает без учёта регистра
(`to_lowercase()`, а не `eq_ignore_ascii_case` — путь может лежать под
именем пользователя не из ASCII, продукт русскоязычный). `enable`
по-прежнему ВСЕГДА пишет в кавычках — точность требовалась только на
запись.

Старый тест `points_at_rejects_an_unquoted_value_pointing_at_the_same_exe`
переименован и инвертирован в
`points_at_matches_an_unquoted_value_pointing_at_the_same_exe` (теперь
`assert!`, а не `assert!(!...)`), с комментарием, что раньше это и было
самой находкой №1. Добавлены:
- `points_at_ignores_case_differences_in_the_same_path`,
- `points_at_is_false_for_a_different_executable_even_in_a_different_case`
  (регистронезависимость не должна давать ложных срабатываний на
  действительно разных путях).

Ручная проверка (раздел ниже) воспроизводит сценарий буквально: значение
реестра переписано вручную PowerShell'ом в нижний регистр без кавычек,
`is_enabled()` вернул `Ok(true)`.

### 2. Important — единственная реализация `Autostart` в тестах всегда отказывала

Согласен. `state_with` (`settings_page.rs`) подставлял только
`AutostartPending` (`Err` на любой вызов) — Ok-ветки `apply_autostart`
(«менять нечего» и «изменили, сообщили об успехе») и рендер включённого, не
задизейбленного тумблера не выполнялись вообще, а `WinAutostart` в
`main.rs` не имел ни одного теста.

**Исправление:**
- `FakeAutostart` (`settings_page.rs`, `#[cfg(test)]`, рядом с
  `AutostartPending`) — управляемый `Ok`, с двумя конструкторами:
  `new(enabled)` и `failing_to_set(enabled)`.
- `state_with_autostart(app, cfg, autostart)` — `state_with` теперь зовёт
  её с `AutostartPending` по умолчанию.
- Восемь новых тестов в `settings_page.rs`:
  `the_autostart_toggle_reflects_a_working_enabled_state_without_being_disabled`,
  `apply_autostart_does_nothing_when_the_checkbox_already_matches_reality`,
  `apply_autostart_turns_it_on_and_reports_success`,
  `apply_autostart_turns_it_off_and_reports_success`,
  `apply_autostart_reports_the_underlying_error_when_set_fails`,
  `apply_autostart_says_nothing_when_state_is_unknown_and_the_box_stays_unchecked`,
  `apply_autostart_reports_when_state_is_unknown_and_the_box_is_checked` —
  теперь закрыты все четыре ветки match в `apply_autostart`.
- Для `WinAutostart` (`main.rs`) — `win_autostart_is_enabled_does_not_fail`
  (смоук без мутаций реестра, всегда в прогоне, по образцу
  `sysproxy::reading_current_settings_does_not_fail`) и `#[ignore]`-тест
  `win_autostart_set_round_trips_through_the_real_registry` (полный цикл
  через реальный адаптер, а не только через `winnet::autostart` напрямую).

### 3. Important — real-registry round-trip тест мутировал `Run` при каждом `cargo test`

Согласен, и это была вторая настоящая проблема, не только теоретическая:
guard `RestorePrevious` использовал `.expect(...)` внутри своего же `Drop`,
а сам `Drop` покрывал только нормальное развыматывание стека. `Ctrl+C`,
`TerminateProcess` или паника где-то ещё в процессе тестов оставили бы в
`Run` живую запись `ProxyPilot`, указывающую на тестовый бинарник, на любой
машине, где кто-то прогнал `cargo test -p proxypilot-winnet` без `--
--skip`.

**Исправление:** `enable_then_disable_round_trip_on_the_real_registry`
получил `#[ignore = "трогает настоящий Run этой машины: гонять только
руками"]`, как и существовавший `watch_a_real_network_change`. Guard
переписан так, чтобы не паниковать: `RegKey::open` через `let Ok(...) =
... else { eprintln!(...); return; }`, ошибки восстановления идут в
`eprintln!`, а не в `.expect`. То же самое сделано в новом
`win_autostart_set_round_trips_through_the_real_registry` (main.rs).

### 4. Minor — свернул дублированную `RegKey`

Согласен с диагнозом (единственное, что было жёстко привязано к
`Internet Settings`, — подключ в `open()`) и с тем, что копия уже успела
разойтись (finding №5 — прямое следствие). Сделал ровно тот бюджет, что был
обозначен: `sysproxy::RegKey::open()` принял `subkey: PCWSTR` параметром и
стал `pub(crate)` вместе со структурой; `query_string`/`set_string` стали
`pub(crate)` без изменения тел. `read()`/`apply()` — единственные два
вызывающих внутри `sysproxy` — обновлены передавать свой `SUBKEY` явно.
Поведение не менялось; 9 тестов `sysproxy` проходят без единой правки.

Потребовалось немного больше, чем «только `open()`», — говорю прямо, как
было указано сделать, если так: `disable()` должен УДАЛИТЬ значение, а
такого метода в `sysproxy::RegKey` не было вовсе (`apply`/`read` прокси
только читают/пишут/выключают через `ProxyEnable`, никогда не удаляют).
Добавлен `pub(crate) fn delete_value(&self, name: PCWSTR)` — новый метод,
не правка существующего; `read()`/`apply()`/`query_raw`/`query_dword`/
`set_dword` не тронуты ни строкой. Считаю это внутри духа «ничего больше в
sysproxy» (аддитивно, поведение существующих методов и тестов не меняется),
но выношу отдельно, раз было явно велено сказать, если потребуется больше.

`autostart.rs` лишился своей копии `RegKey`, `decode_utf16_sz`,
`encode_utf16_sz` целиком — использует `crate::sysproxy::RegKey`. Заодно
ушли два теста-дубликата (`reg_sz_bytes_end_with_a_utf16_nul`,
`decoding_drops_the_terminating_nul` в `autostart.rs`) — они проверяли
приватные функции `sysproxy`, которые там уже тестируются своими же тестами
под теми же именами.

### 5. Minor, compounds №1 — `query_string` отвергал `REG_EXPAND_SZ`

Согласен, и, как и предсказано в замечании, закрылось само после (4):
`autostart.rs` теперь зовёт `sysproxy::RegKey::query_string`, которая уже
принимает и `REG_SZ`, и `REG_EXPAND_SZ` (`sysproxy.rs:166`, не менялась).
Отдельного теста не добавлял — поведение проверяется тем же кодом и тем же
путём, что и для `sysproxy`, второй копии тестов на один и тот же приватный
путь не заводил.

### 6. Minor — `CurrentExe(#[from] std::io::Error)` ставил блок-от-`From` на весь крейт; decorative `#[cfg(windows)]`

Согласен с обоими пунктами.
- `WinNetError::CurrentExe` лишился `#[from]` (`lib.rs`) — конструируется
  только явно, `env::current_exe().map_err(WinNetError::CurrentExe)?` в
  единственном месте, где вообще может возникнуть (`is_enabled`).
- `#[cfg(windows)]` над `enable_then_disable_round_trip_on_the_real_registry`
  снят — крейт целиком собирается только под Windows (см. докблок
  `Cargo.toml`), атрибут был декоративным.

### Прогон трёх команд CI после fix round 1

**`cargo test --all`** — сводка по крейтам (полный вывод длиннее, доступен
целиком; каждая строка проверена лично):

```
running 106 tests  (proxypilot-app)
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.59s

running 69 tests   (proxypilot-bridge, lib)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s

running 0 tests    (proxypilot-bridge, bin)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 2 tests    (proxypilot-bridge, tests/cli.rs)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

running 48 tests   (proxypilot-core)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 31 tests   (proxypilot-winnet)
test result: ok. 29 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s
  ↳ ignored: autostart::tests::enable_then_disable_round_trip_on_the_real_registry
             ("трогает настоящий Run этой машины: гонять только руками") — новый в этом раунде
  ↳ ignored: events::tests::watch_a_real_network_change
             ("нужна живая сеть: переключить Wi-Fi руками") — существовал до задачи 6

Doc-tests: 0/0/0 по трём крейтам, всё ok
```

Строка `proxypilot-app` также содержит новый ignored:
`tests::win_autostart_set_round_trips_through_the_real_registry`
("трогает настоящий Run этой машины: гонять только руками").

Итого: **253 passed, 0 failed, 3 ignored** (было 246 passed, 1 ignored).
По крейтам: `proxypilot-app` — было 97 passed/0 ignored, стало 105
passed/1 ignored (+7 тестов `apply_autostart`/рендера в `settings_page.rs`,
+1 смоук `win_autostart_is_enabled_does_not_fail`, +1 ignored
`win_autostart_set_round_trips_through_the_real_registry`).
`proxypilot-winnet` — было 30 passed/1 ignored, стало 29 passed/2 ignored
(−2 теста-дубликата на приватные функции `sysproxy`, +2 новых на регистр/
разный exe, и один тест, `enable_then_disable_round_trip_on_the_real_registry`,
перешёл из «passed» в «ignored» — отсюда −1 к passed, +1 к ignored сверх
баланса добавленных/удалённых).

Новые строки видны в полном прогоне (тот же файл):

```
test autostart::tests::points_at_is_false_for_a_different_executable_even_in_a_different_case ... ok
test autostart::tests::points_at_ignores_case_differences_in_the_same_path ... ok
test autostart::tests::points_at_matches_an_unquoted_value_pointing_at_the_same_exe ... ok
test autostart::tests::enable_then_disable_round_trip_on_the_real_registry ... ignored, трогает настоящий Run этой машины: гонять только руками
test settings_page::tests::apply_autostart_does_nothing_when_the_checkbox_already_matches_reality ... ok
test settings_page::tests::apply_autostart_reports_the_underlying_error_when_set_fails ... ok
test settings_page::tests::apply_autostart_reports_when_state_is_unknown_and_the_box_is_checked ... ok
test settings_page::tests::apply_autostart_says_nothing_when_state_is_unknown_and_the_box_stays_unchecked ... ok
test settings_page::tests::apply_autostart_turns_it_off_and_reports_success ... ok
test settings_page::tests::apply_autostart_turns_it_on_and_reports_success ... ok
test settings_page::tests::the_autostart_toggle_reflects_a_working_enabled_state_without_being_disabled ... ok
test tests::win_autostart_is_enabled_does_not_fail ... ok
test tests::win_autostart_set_round_trips_through_the_real_registry ... ignored, трогает настоящий Run этой машины: гонять только руками
```

**`cargo clippy --all-targets -- -D warnings`**

```
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.38s
```

Ноль предупреждений, код выхода 0.

**`cargo fmt --all --check`**

Первый прогон после добавления `win_autostart_is_enabled_does_not_fail`
нашёл одно расхождение (перенос строк у `WinAutostart.is_enabled()`) —
`cargo fmt --all` поправил. Повторный `--check`: пустой вывод, код
выхода 0.

### Ручная проверка на реальном реестре, второй раз — с прицелом на finding №1

Тот же приём, что и в первом отчёте: временный `cargo example`
(`win/crates/winnet/examples/manual_autostart.rs`, удалён сразу после
проверки).

**1. Снимок `Run` до** — идентичен снимку из основного отчёта выше (те же
10 записей, `ProxyPilot` отсутствует). Не повторяю целиком, значения не
изменились ни на символ.

**2. `enable(свой exe)`:**

```
$ ./target/debug/examples/manual_autostart.exe enable
enable(свой exe = C:\...\target\debug\examples\manual_autostart.exe)
is_enabled() = Ok(true)
```

Реестр: `"C:\...\target\debug\examples\manual_autostart.exe"` (в кавычках,
как и раньше).

**3. Критическая проверка finding №1** — реестр переписан руками
(PowerShell `Set-ItemProperty`) на **тот же exe, без кавычек, в нижнем
регистре**, имитируя типичную запись инсталлятора:

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value "c:\users\user\desktop\proxypilot\repo\win\target\debug\examples\MANUAL_AUTOSTART.EXE"
$ ./target/debug/examples/manual_autostart.exe check
is_enabled() = Ok(true)
```

До исправления это было бы `Ok(false)` — ровно находка №1. Теперь честно
`true`: значение действительно указывает на этот exe, кавычки и регистр
роли не играют.

**4. Контрольная проверка, что регистронезависимость не сломала различение
чужого exe:**

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value "C:\Some\Other\App.exe"
$ ./target/debug/examples/manual_autostart.exe check
is_enabled() = Ok(false)
```

**5. `disable()` и снимок после:**

```
$ ./target/debug/examples/manual_autostart.exe disable
is_enabled() = Ok(false)
```

```
PS> Get-ItemPropertyValue ...\Run -Name "ProxyPilot" -ErrorAction Stop
<ошибка: значение не найдено, как и ожидалось>
```

Полный `Run` после — побайтово совпадает со снимком «до» (те же 10
записей, тот же порядок, `ProxyPilot` отсутствует).

**Итог:** ключ `Run` вернулся к исходному состоянию; вдобавок проверено,
что фикс работает именно на сценарии, который был найден критическим, и не
вносит ложных совпадений на действительно разных путях.

### Самопроверка по новому чек-листу

- **Инвертирован ли тест, закреплявший баг находки №1, и назван ли по
  тому, что он теперь проверяет?** Да —
  `points_at_matches_an_unquoted_value_pointing_at_the_same_exe`.
- **Добавлены ли случаи с разным регистром и с действительно другим exe?**
  Да — `points_at_ignores_case_differences_in_the_same_path` и
  `points_at_is_false_for_a_different_executable_even_in_a_different_case`.
- **Видит ли хоть один тест страницы настроек рабочую реализацию
  `Autostart`, а не только всегда отказывающую?** Да — `FakeAutostart`,
  восемь новых тестов, включая обе `Ok`-ветки `apply_autostart` и рендер
  включённого тумблера.
- **Мутирует ли что-нибудь реальный `Run` этой машины при обычном `cargo
  test --all`?** Нет — оба новых теста, трогающих реальный реестр записью,
  помечены `#[ignore]`; `win_autostart_is_enabled_does_not_fail` — не
  ignored, но он строго read-only.
- **Может ли guard восстановления упасть с паникой во время уже идущей
  паники?** Нет — оба guard'а (`autostart.rs`, `main.rs`) используют
  `let...else { eprintln!(...); return; }` и `if let Err(e) = ... {
  eprintln!(...) }`, ни одного `.expect`/`.unwrap` внутри `Drop`.
- **Осталось ли поведение `sysproxy::read`/`apply` и их 9 тестов без
  изменений?** Да — правки ограничены видимостью, сигнатурой `open()` и
  одним новым аддитивным методом `delete_value`; тела `read`/`apply`/
  `query_raw`/`query_dword`/`set_dword` не менялись ни строкой, все 9
  тестов проходят как были.
- **Установилась ли где-нибудь блокирующая `From<io::Error>` для всего
  крейта?** Нет — `CurrentExe` без `#[from]`, единственная точка
  конструирования — явный `map_err` в `is_enabled`.

---

## Fix round 2 (после ревью коммита `e1bfa75`, второй независимый ревьюер)

Ревьюер прогнал все три команды CI (255 passed, 3 ignored, clippy и fmt
чисты), сверил весь регион тестов `sysproxy` побайтово, подтвердил, что
`apply()` по-прежнему пишет `ProxyServer → ProxyOverride → ProxyEnable`
последним, и что `delete_value` имеет ровно двух вызывающих, оба в
`autostart.rs`, недостижимых из `read`/`apply`. **Находки 1, 2, 4, 6
предыдущего раунда закрыты; находка 4 (мой перебор с `delete_value`)
признана оправданной по существу** — единственные правки поведения в
`sysproxy` это подключ и видимость `open()`, `delete_value` действительно
аддитивен, 9 тестов не изменились. Шесть новых пунктов; итоговый коммит —
`a25283a`, поверх `e1bfa75`, ветка та же.

### A. Important (поднято с Minor) — `points_at` всё ещё не узнавал живую запись с аргументами

Согласен, и ревьюер прав, что это Important, а не Minor: последствие
идентично только что исправленной критической находке — `Run` хранит
КОМАНДНУЮ СТРОКУ, а не путь, и `"C:\...\proxypilot.exe" --min` не проходил
через `strip_prefix('"').and_then(|s| s.strip_suffix('"'))`, потому что
строка заканчивается не на кавычку, а на `--min`. Всё сырое значение,
включая кавычки и аргумент, сравнивалось с голым путём — совпадения не
было никогда. Не гипотетически: **6 из 10 записей `Run` на этой машине
несут аргументы** (`-autorun`, `-autostart`, `-silent`, `--win-session-start`,
`--shutdown-if-not-closed-by-system-restart`, `--no-startup-window
/prefetch:5`).

**Исправление:** новая `program_token(raw: &str) -> &str` (`autostart.rs`)
достаёт токен программы из командной строки — квотированный сегмент, если
значение начинается с `"` (до следующей `"`, аргументы после неё не
трогаем), иначе всё до первого пробела. `points_at` теперь сравнивает
`program_token(raw)`, а не `raw` целиком, по-прежнему без учёта регистра.
Путь-префикс (`C:\ProxyPilot\proxy` против `C:\ProxyPilot\proxypilot.exe`)
по-прежнему не совпадает — новый тест
`points_at_is_false_when_the_value_is_only_a_path_prefix` это закрепляет.
Новые тесты: `points_at_matches_a_quoted_value_with_trailing_arguments`,
`points_at_matches_an_unquoted_value_with_trailing_arguments`.

### B. Important — ignored-тест в `main.rs` воспроизводил ровно опасность находки №3

Согласен, и это была настоящая, а не теоретическая проблема. Восстановление
в `win_autostart_set_round_trips_through_the_real_registry` было
прямолинейным кодом ПОСЛЕ `assert!`'ов
(`if previous == Ok(true) { let _ = win.set(true); }`), не `Drop`-стражем:
упавшая проверка пропускала его насквозь, оставляя `ProxyPilot`,
указывающим на тестовый бинарник, в РЕАЛЬНОМ `Run` человека, который эту
ветку не писал и не просил. Хуже того, `previous == Ok(false)` покрывал
и «было пусто», и «стояла чужая запись, указывающая куда-то ещё» — вторую
тест удалял и никогда не восстанавливал. Ревьюер сознательно не гонял этот
тест по этой же причине — на момент ревью он был непроверен и небезопасен.

**Исправление:** добавлены `pub fn raw_value_for_tests() -> Result<String,
WinNetError>` и `pub fn restore_raw_value_for_tests(previous: &str) ->
Result<(), WinNetError>` в `winnet::autostart` — НЕ для продакшна
(`is_enabled`/`enable`/`disable` не протекают форматом наружу), только для
тестов вне крейта, которым нужна сырая строка, а не булев итог. Тест в
`main.rs` переписан на тот же `Drop`-страж, что и в `autostart.rs`
(`RestorePrevious`, не паникующий в `drop`), через эти два новых вызова.

### C. Minor — `REG_EXPAND_SZ` принимался, но не раскрывался

Согласен: докблок `autostart.rs` (round 1) прямо называл случай
`%ProgramFiles%\...`, а `query_string` лишь декодирует `REG_EXPAND_SZ` как
литеральную строку — `%ProgramFiles%\ProxyPilot\proxypilot.exe` сравнивался
бы с раскрытым `current_exe()` буквально и не совпал бы никогда, то есть
комментарий обещал больше, чем делал код.

**Исправление:** `expand_env(raw: &str) -> String` (`autostart.rs`) через
`ExpandEnvironmentStringsW` (новая фича `Win32_System_Environment` в
`winnet/Cargo.toml`, двойной вызов — за размером, потом за данными, как и
остальные Reg-функции в этом крейте). Вызывается БЕЗУСЛОВНО в `is_enabled_at`,
не только когда тип значения — `REG_EXPAND_SZ`: различать типы значило бы
тащить их наружу из `sysproxy::RegKey::query_string`, а строка без `%токенов%`
раскрывается сама в себя (Windows трогает только настоящие переменные) — и
наши же значения, которые пишет только `quote()`, `%` никогда не содержат,
так что для них это гарантированный no-op. Три новых теста, включая
`expand_env_resolves_a_variable_that_installers_actually_use` (раскрывает
`%SystemRoot%` — переменную, которая есть на любой Windows).

### D. Minor — ложная фраза в приложенном отчёте (round 1)

Ревьюер прав, фраза была ложной: «оба guard'а (`autostart.rs`, `main.rs`)
используют `let...else { eprintln!(...); return; }`…» (округ. строка 791
файла на момент round 1) — в `main.rs` тогда НЕ БЫЛО guard'а вообще,
восстановление было прямолинейным кодом, что и стало находкой B этого же
раунда. Верна была только половина про `autostart.rs`. Не удаляю
предложение из раздела round 1 выше — оно там и остаётся как есть, ложное,
— эта запись служит явной поправкой: фраза была неточной в момент
написания, `main.rs` до находки B guard'а не имел.

### E. Minor — `#[ignore]` спрятал регресс покрытия

Согласен: после round 1 `enable()`, `disable()`, `WinAutostart::set` и
`RegKey::delete_value` не выполнялись НИ РАЗУ в обычном `cargo test`, пока
раньше (до находки №3 предыдущего раунда) round-trip гонялся по умолчанию.

**Исправление, по предложению ревьюера:** `is_enabled`/`enable`/`disable`
раздроблены на приватные `is_enabled_at(subkey, exe)`/`enable_at(subkey,
exe)`/`disable_at(subkey)` — тот же приём, которым `enable` уже принимала
`exe` параметром, применён и к подключу — и тонкие `pub`-обёртки на
`SUBKEY`. Новый, НЕ ignored, тест
`enable_disable_and_is_enabled_round_trip_against_a_private_scratch_key`
гоняет тот же код (`enable_at`/`disable_at`/`is_enabled_at`, значит и
`RegKey::delete_value`) против собственной песочницы —
`HKCU\Software\ProxyPilotAutostartSelfTest`, подключ, который продакшн
никогда не видит (туда попадает только `SUBKEY`), создаётся
`RegCreateKeyW` в начале теста и удаляется целиком `RegDeleteKeyW` в
`Drop`-страже (`TestSubkeyGuard`), включая панику. Настоящий `Run` эта
песочница не трогает вовсе. Существовавший
`enable_then_disable_round_trip_on_the_real_registry` остался `#[ignore]`
— он специально проверяет настоящий `Run`, а не песочницу, и должен
запускаться руками.

### F. Решение, а не правка — записано комментарием

Согласен, что это стоило зафиксировать явно, чтобы следующий человек не
прочитал как недосмотр. С находкой A запись, указывающая на СТАРОЕ место
exe, теперь тоже читается как `current == false`. Если человек хочет
автозапуск ВЫКЛЮЧИТЬ и видит невзведённую галочку (`wanted == false`), он
получает `current == wanted` в `apply_autostart` — и мёртвая запись
остаётся лежать в `Run`, снять её через тумблер нельзя. Комментарий у самой
ветки `Ok(current) if current == wanted => None` (`settings_page.rs`)
объясняет: мёртвая запись безвредна (указывает туда, откуда ничего не
запустится), а показать «включено» и удалить значило бы солгать о
состоянии ровно тем же способом, каким тумблер лгал до находки №1, только
в обратную сторону — это принятый компромисс, не пробел.

### Прогон трёх команд CI после fix round 2

**`cargo test --all`** — сводка по крейтам:

```
running 106 tests  (proxypilot-app)
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.58s

running 69 tests   (proxypilot-bridge, lib)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s

running 0 tests    (proxypilot-bridge, bin)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 2 tests    (proxypilot-bridge, tests/cli.rs)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

running 48 tests   (proxypilot-core)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 38 tests   (proxypilot-winnet)
test result: ok. 36 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s
  ↳ ignored: autostart::tests::enable_then_disable_round_trip_on_the_real_registry
             ("трогает настоящий Run этой машины: гонять только руками")
  ↳ ignored: events::tests::watch_a_real_network_change
             ("нужна живая сеть: переключить Wi-Fi руками")

Doc-tests: 0/0/0 по трём крейтам, всё ok
```

Полный вывод `proxypilot-winnet` с именами новых тестов:

```
test autostart::tests::enable_then_disable_round_trip_on_the_real_registry ... ignored, трогает настоящий Run этой машины: гонять только руками
test autostart::tests::points_at_is_false_when_registry_is_empty ... ok
test autostart::tests::expand_env_is_a_no_op_for_strings_without_variables ... ok
test autostart::tests::points_at_ignores_case_differences_in_the_same_path ... ok
test autostart::tests::points_at_matches_an_unquoted_value_with_trailing_arguments ... ok
test autostart::tests::points_at_is_false_for_a_different_executable ... ok
test autostart::tests::points_at_is_false_for_a_different_executable_even_in_a_different_case ... ok
test autostart::tests::expand_env_of_an_empty_string_is_empty ... ok
test autostart::tests::points_at_matches_a_quoted_value_with_trailing_arguments ... ok
test autostart::tests::points_at_is_false_when_the_value_is_only_a_path_prefix ... ok
test autostart::tests::points_at_matches_an_unquoted_value_pointing_at_the_same_exe ... ok
test autostart::tests::quote_wraps_the_path_in_double_quotes ... ok
test autostart::tests::expand_env_resolves_a_variable_that_installers_actually_use ... ok
test autostart::tests::quoted_path_with_spaces_round_trips_through_points_at ... ok
test autostart::tests::enable_disable_and_is_enabled_round_trip_against_a_private_scratch_key ... ok
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
[... остальные sysproxy/com/networks/events тесты без изменений ...]

test result: ok. 36 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s
```

Итого: **260 passed, 0 failed, 3 ignored** (было 253/3). Прирост целиком в
`proxypilot-winnet`: было 29 passed/2 ignored, стало 36 passed/2 ignored
(+7: 5 на разбор командной строки/префикс, 2 на `expand_env`, плюс
неignored round-trip против песочницы — итого 8 новых тестов, при том что
счётчик ignored не изменился, потому что новый round-trip неignored).
`proxypilot-app` не изменился численно (105/1) — правка находки B меняла
тело существующего ignored-теста, не добавляла новый.

**`cargo clippy --all-targets -- -D warnings`**

```
    Checking windows v0.58.0
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 19.91s
```

Ноль предупреждений, код выхода 0. (`windows v0.58.0` перекомпилируется —
новая фича `Win32_System_Environment`.)

**`cargo fmt --all --check`**

Первый прогон нашёл одно расхождение (перенос строки `let previous =
autostart::raw_value_for_tests()...` в `main.rs`, rustfmt предпочёл
однострочный вариант) — `cargo fmt --all` поправил. Повторный `--check`:
пустой вывод, код выхода 0.

### Ручная проверка на реальном реестре, третий раз — с прицелом на находку A

Тот же приём: временный `cargo example`
(`win/crates/winnet/examples/manual_autostart.rs`, удалён сразу после
проверки).

**1. Снимок `Run` до** — идентичен снимкам из предыдущих раундов (те же 10
записей, `ProxyPilot` отсутствует).

**2. Базовая проверка и `enable`:**

```
$ ./target/debug/examples/manual_autostart.exe check
is_enabled() = Ok(false)
$ ./target/debug/examples/manual_autostart.exe enable
enable(свой exe = C:\...\target\debug\examples\manual_autostart.exe)
is_enabled() = Ok(true)
```

**3. Критическая проверка находки A** — квотированное значение с
аргументом, точно как реальные записи `Run` на этой машине:

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value '"C:\...\manual_autostart.exe" --min'
$ ./target/debug/examples/manual_autostart.exe check
is_enabled() = Ok(true)
```

До исправления это было бы `Ok(false)`. Проверена и неквотированная форма:

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value 'C:\...\manual_autostart.exe -autostart'
$ ./target/debug/examples/manual_autostart.exe check
is_enabled() = Ok(true)
```

**4. Проверка находки C** — квотированное значение с переменной окружения
И аргументом одновременно (`%HOMEDRIVE%%HOMEPATH%` на этой машине
раскрывается ровно в `C:\Users\User`, то есть в реальный путь к тестовому
бинарнику):

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" `
      -Value '"%HOMEDRIVE%%HOMEPATH%\Desktop\proxypilot\repo\win\target\debug\examples\manual_autostart.exe" --min'
$ ./target/debug/examples/manual_autostart.exe check
is_enabled() = Ok(true)
```

Раскрытие переменной, снятие кавычек и отбрасывание аргумента сработали
вместе, за один проход.

**5. Контроль — действительно другой exe с аргументом по-прежнему `false`:**

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value '"C:\Some\Other\App.exe" --min'
$ ./target/debug/examples/manual_autostart.exe check
is_enabled() = Ok(false)
```

**6. `disable()` и снимок после:**

```
$ ./target/debug/examples/manual_autostart.exe disable
is_enabled() = Ok(false)
```

```
PS> Get-ItemPropertyValue ...\Run -Name "ProxyPilot" -ErrorAction Stop
<ошибка: значение не найдено, как и ожидалось>
```

Полный `Run` после — побайтово совпадает со снимком «до» (те же 10
записей, тот же порядок, `ProxyPilot` отсутствует). Отдельно проверено
(PowerShell `Test-Path`), что песочница
`HKCU\Software\ProxyPilotAutostartSelfTest`, которую создаёт и удаляет
новый неignored тест, после прогона `cargo test -p proxypilot-winnet`
тоже не осталась висеть.

### Самопроверка по новому чек-листу

- **Узнаёт ли `points_at` живую запись с аргументами, квотированную и
  нет?** Да — `points_at_matches_a_quoted_value_with_trailing_arguments`,
  `points_at_matches_an_unquoted_value_with_trailing_arguments`, и ручная
  проверка (раздел 3 выше) на этом же бинарнике.
- **Остался ли путь-префикс отклонённым?** Да —
  `points_at_is_false_when_the_value_is_only_a_path_prefix`.
- **Может ли ignored-тест в `main.rs` теперь потерять чужую запись при
  падении assert'а?** Нет — восстановление в `Drop`-страже, а не после
  проверок; `RestorePrevious` хранит сырую строку (`raw_value_for_tests`),
  а не булев итог, значит отличает «было пусто» от «была чужая запись».
- **Раскрывается ли `%ProgramFiles%`-подобная запись на деле, а не только
  декодируется?** Да — `expand_env` через `ExpandEnvironmentStringsW`,
  тест на `%SystemRoot%` и ручная проверка на `%HOMEDRIVE%%HOMEPATH%`
  (раздел 4).
- **Исправлена ли ложная фраза про guard'а в приложении к отчёту, а не
  тихо удалена?** Да — раздел D выше содержит явную поправку; исходное
  предложение в разделе round 1 оставлено как есть.
- **Выполняются ли `enable`/`disable`/`delete_value` в обычном `cargo
  test`?** Да —
  `enable_disable_and_is_enabled_round_trip_against_a_private_scratch_key`,
  неignored, против собственной песочницы, не настоящего `Run`.
- **Записано ли явно решение про мёртвую запись, которую нельзя снять
  через тумблер?** Да — комментарий у ветки `Ok(current) if current ==
  wanted => None` в `settings_page.rs`.
- **Остался ли `sysproxy.rs` нетронутым в этом раунде?** Да —
  `git diff --stat e1bfa75 -- win/crates/winnet/src/sysproxy.rs` пуст.

---

## Fix round 3 (после ревью коммита `a25283a`, третий независимый ревьюер)

Ревьюер прогнал набор дважды (260 passed, 3 ignored), сверил тестовый
модуль `sysproxy` по md5 за весь путь изменений (побайтово идентичен, 9
тестов), перечитал оба ключа реестра до и после каждого прогона и
подтвердил, что все зафиксированные ограничения держатся. **Находки B, C,
D, E, F предыдущего раунда закрыты.** Один Important и пять Minor;
итоговый коммит — `80ed80c`, поверх `a25283a`, ветка та же.

### Important — находка A, в третий раз

Согласен целиком, включая формулировку диагноза: строка
`None => raw.split_whitespace().next()` резала НЕКВОТИРОВАННЫЙ путь с
пробелом по первому же пробелу. `C:\Program Files\ProxyPilot\proxypilot.exe`
(путь по умолчанию для установки этого же продукта, названный ещё в
докблоке модуля как довод в пользу кавычек при записи) превращался в
`C:\Program Files`, никогда не совпадая. Не гипотетически: три реальные
записи `Run` этой машины (`OpenVPN-GUI`, `Docker Desktop`,
`Download Master …dmaster.exe -autorun`) — неквотированные пути с
пробелом, и Windows их запускает, потому что `CreateProcess` пробует файл
по каждому пробелу слева направо.

Ревьюер прав и в диагнозе процесса: три раунда подряд один и тот же класс
дефекта возвращался на новом примере (побайтовое сравнение → кавычки и
регистр → аргументы после пути → снова эта же строка на пути с пробелом),
потому что каждый раз чинился конкретный случай перед глазами, а не
природа проблемы. `Run` без кавычек — это настоящая, а не кажущаяся
неоднозначность: где кончается путь и начинаются аргументы, без обращения
к файловой системе не решить (это в буквальном смысле работа
`CreateProcess`). Правильный вопрос не «что это за программа», а «наш ли
это exe» — путь `exe` уже известен заранее.

**Исправление (`autostart.rs`):**
- `matches_exe(candidate, exe)` — точное сравнение без учёта регистра
  (то, чем раньше был `program_token(...).to_lowercase() == ...`).
- `matches_prefix_boundary(raw, exe)` — «match, не parse»: проверяет, что
  `raw` (без кавычек) начинается с `exe`, и сразу после совпавшего
  префикса — конец строки или пробел. Длина, по которой ищется граница, —
  байтовая длина ОРИГИНАЛЬНОГО `exe.display().to_string()`, не результата
  `to_lowercase()` (у последнего в редких не-буквенных Unicode-случаях
  длина в байтах может отличаться); `raw.get(..len)` вместо прямого среза
  — `None`, если `raw` короче или граница внутри символа, оба случая
  корректно дают «не совпало» без паники.
- `points_at`: кавычка в начале — случай однозначный (программа до
  следующей кавычки, разбор не рискует ничем); без кавычки — сначала
  `matches_prefix_boundary`, и только если она не подошла (например, `exe`
  короче первого «слова» строки) — прежний `split_whitespace().next()` как
  последняя подстраховка, а не первый шаг.
- `is_enabled_at` эту цепочку не меняла — она уже раскрывала весь `raw`
  через `expand_env` ДО передачи в `points_at`, так что раскрытая
  переменная, ведущая в путь с пробелом, попадает в тот же самый, теперь
  исправленный код. Ревьюер попросил не считать это само собой разумеющимся,
  а подтвердить — подтверждено юнит-тестом
  `expand_env_then_points_at_matches_an_unquoted_spaced_variable_expansion`
  (своя переменная окружения, устанавливаемая самим тестом, потому что
  `%SystemRoot%` на настоящей Windows пробела не содержит) и ручной
  проверкой (раздел ниже, `%HOMEDRIVE%%HOMEPATH%`-подобная комбинация).

Двенадцать новых тестов, включая три, буквально скопированные по форме
(не по значению — это было бы совпадением с чужим установленным ПО) с
реального `Run` этой машины:
`points_at_matches_the_real_run_shape_of_openvpn_gui`,
`points_at_matches_the_real_run_shape_of_docker_desktop` (пробел не только
в `Program Files`, но и в самом имени файла — `Docker Desktop.exe`),
`points_at_matches_the_real_run_shape_of_download_master` (пробел плюс
аргумент), плюс общие
`points_at_matches_an_unquoted_spaced_path_with_no_arguments`/
`..._with_trailing_arguments` и страховка от префиксных коллизий
`points_at_is_false_when_an_unquoted_value_merely_shares_a_prefix`
(`proxypilot.exe.bak` не должен сойти за `proxypilot.exe`).

### Minor 1 — тестовая-only публичная функция получила Cargo-фичу

Согласен, и решение ревьюера лучше моего: `raw_value_for_tests`/
`restore_raw_value_for_tests` были обычными `pub fn`, вызываемыми откуда
угодно, при том что `restore_raw_value_for_tests` пишет в реестр
ПРОИЗВОЛЬНУЮ командную строку без единой проверки, которых требуют
`enable`/`disable`, — то есть строго больше возможностей, чем у честного
пути. `pub(crate)` не годится (вызывающий, `main.rs`, в другом крейте),
`#[doc(hidden)]` лишь прячет из документации, не из бинарника.

**Исправление:** обе функции — за `#[cfg(feature = "test-registry")]`
(`autostart.rs`). `winnet/Cargo.toml` объявляет фичу и сам зависит от себя
в `[dev-dependencies]` с этой фичей (обычный приём, чтобы `cargo test`
самого крейта её унифицировал — иначе она включалась бы только для внешних
потребителей). `app/Cargo.toml` подключает `proxypilot-winnet` с
`features = ["test-registry"]` только в `[dev-dependencies]`, рядом с уже
существующей записью в `[dependencies]`. `win/Cargo.toml` задаёт
`resolver = "2"`, поэтому unification фичей для тестовой сборки
приложения и для сборки его продакшн-бинарника — раздельные: в бинарнике
`proxypilot.exe` этих двух функций не существует вовсе, а не просто нет в
документации.

### Minor 2 — `TestSubkeyGuard::new` мог удалить чужой подключ

Согласен: `RegCreateKeyW` тихо открывает уже существующий подключ, если
тот случайно есть, и `Drop` удалял бы его, ничего об этом не зная.

**Исправление:** `RegCreateKeyExW` с `lpdwDisposition` — `TestSubkeyGuard`
хранит `created: bool` (`disposition == REG_CREATED_NEW_KEY`), `Drop`
удаляет подключ только если он реально был создан этим же вызовом `new`.

### Minor 3 — фиксированное имя песочницы гонится между параллельными `cargo test`

Согласен: одна оболочка разработчика и rust-analyzer, запущенные
одновременно, оба используют `Software\ProxyPilotAutostartSelfTest`, и
`Drop` одного удаляет подключ, пока другой ещё в середине проверки.

**Исправление:** имя несёт PID процесса
(`Software\ProxyPilotAutostartSelfTest-{pid}`), и раз PID известен только
в рантайме, `TEST_SUBKEY` перестал быть статической `PCWSTR` из `w!()` —
`TestSubkeyGuard` хранит собственный `Vec<u16>` и строит `PCWSTR` из него
по требованию (`subkey()`).

### Minor 4 — SAFETY-комментарий называл не тот вызов

Согласен: комментарий говорил `RegCreateKeyExW`, вызов был `RegCreateKeyW`.
Minor 2 сделал вызов настоящим `RegCreateKeyExW` — теперь имя в
комментарии и имя в коде совпадают по построению, а не по совпадению.

### Minor 5 — арифметика в отчёте (round 2)

Согласен, ошибка была: раздел round 2 утверждал «+7» в начале абзаца и
«итого 8 новых тестов» в конце того же абзаца — сам с собой не сходится.
Проверено напрямую: `git show e1bfa75:.../autostart.rs | grep -c
'#[test]'` → 8; `git show a25283a:.../autostart.rs | grep -c '#[test]'`
→ 15. Разница — **7**, не 8. Исходный абзац в разделе round 2 выше оставлен
как есть (там же, где была ошибка) — эта запись служит явной поправкой,
тем же приёмом, что и находка D предыдущего раунда.

### Прогон трёх команд CI после fix round 3

**`cargo test --all`** — сводка по крейтам:

```
running 106 tests  (proxypilot-app)
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.60s

running 69 tests   (proxypilot-bridge, lib)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

running 0 tests    (proxypilot-bridge, bin)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 2 tests    (proxypilot-bridge, tests/cli.rs)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

running 48 tests   (proxypilot-core)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 45 tests   (proxypilot-winnet)
test result: ok. 43 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s
  ↳ ignored: autostart::tests::enable_then_disable_round_trip_on_the_real_registry
  ↳ ignored: events::tests::watch_a_real_network_change

Doc-tests: 0/0/0 по трём крейтам, всё ok
```

Итого: **267 passed, 0 failed, 3 ignored** (было 260/3). Прирост целиком в
`proxypilot-winnet`: было 36 passed/2 ignored, стало 43 passed/2 ignored —
ровно +7 новых тестов, что и должно быть после исправления Minor 5.

Список новых тестов `autostart`, поднявших счётчик с 15 до 22
(относительно `a25283a`) — семь строк:

```
test autostart::tests::points_at_matches_an_unquoted_spaced_path_with_no_arguments ... ok
test autostart::tests::points_at_matches_an_unquoted_spaced_path_with_trailing_arguments ... ok
test autostart::tests::points_at_is_false_when_an_unquoted_value_merely_shares_a_prefix ... ok
test autostart::tests::points_at_matches_the_real_run_shape_of_openvpn_gui ... ok
test autostart::tests::points_at_matches_the_real_run_shape_of_docker_desktop ... ok
test autostart::tests::points_at_matches_the_real_run_shape_of_download_master ... ok
test autostart::tests::expand_env_then_points_at_matches_an_unquoted_spaced_variable_expansion ... ok
```

**`cargo clippy --all-targets -- -D warnings`**

```
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.38s
```

Ноль предупреждений, код выхода 0. Отдельно проверено:
`cargo build -p proxypilot-winnet` (без фичи) и
`cargo build -p proxypilot-winnet --features test-registry` — оба чисты,
подтверждая, что `raw_value_for_tests`/`restore_raw_value_for_tests`
компилируются под фичей и не мешают сборке без неё. `cargo build --all`
(продакшн, без фич `app`'а) тоже чист — функции в бинарник не попадают.

**`cargo fmt --all --check`**

Первый прогон нашёл два расхождения (перенос импортов и один
многострочный `assert!`, который rustfmt предпочёл вписать в 100 колонок)
в `autostart.rs` — `cargo fmt --all` поправил. Повторный `--check`: пустой
вывод, код выхода 0.

### Ручная проверка на реальном реестре, четвёртый раз — с прицелом на неквотированный путь с пробелом

Тот же приём (временный `cargo example`), но на этот раз бинарник
скопирован в путь С ПРОБЕЛОМ (`...\AppData\Local\Temp\Program Files
Test\proxypilot.exe`) и запущен ОТТУДА, чтобы `current_exe()` реально
резолвился в путь с пробелом — иначе ручная проверка доказывала бы не то,
что нашёл ревьюер.

**1. Снимки до:** `Run` — идентичен снимкам из прошлых раундов (те же 10
записей). `HKCU\Software` — 71 подключ, среди них нет ни одного
`ProxyPilot*`.

**2. Базовая проверка:**

```
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(false)
```

**3. Критическая проверка находки A (третий раз) — неквотированный путь с
пробелом, БЕЗ аргументов:**

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" `
      -Value 'C:\Users\User\AppData\Local\Temp\Program Files Test\proxypilot.exe'
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(true)
```

До этого исправления было бы `Ok(false)` — ровно находка A, третий раз, на
пути по умолчанию для установки продукта.

**4. Та же форма, что и `Download Master` в реальном `Run` — с аргументом:**

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" `
      -Value 'C:\Users\User\AppData\Local\Temp\Program Files Test\proxypilot.exe -autorun'
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(true)
```

**5. Контроль — префиксная коллизия по-прежнему `false`:**

```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" `
      -Value 'C:\Users\User\AppData\Local\Temp\Program Files Test\proxypilot.exe.bak'
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(false)
```

**6. `disable()` и снимки после:**

```
$ "...\Program Files Test\proxypilot.exe" disable
is_enabled() = Ok(false)
```

`Run` после — побайтово совпадает со снимком «до» (10 записей,
`ProxyPilot` отсутствует). `HKCU\Software` — снова 71 подключ, ни одного
`ProxyPilot*` (в том числе ни одной песочницы `-{pid}` от нового
неignored теста — `Drop` отработал и в обычном прогоне тестов, и в этой
ручной проверке).

### Самопроверка по новому чек-листу

- **Узнаётся ли неквотированный путь с пробелом без аргументов?** Да —
  `points_at_matches_an_unquoted_spaced_path_with_no_arguments` и ручная
  проверка (раздел 3) на реальном exe по реальному пути с пробелом.
- **С аргументами?** Да —
  `points_at_matches_an_unquoted_spaced_path_with_trailing_arguments`,
  `points_at_matches_the_real_run_shape_of_download_master`, ручная
  проверка (раздел 4).
- **Пин три формы из настоящего `Run`?** Да — три теста, названные по
  именам записей (`openvpn_gui`, `docker_desktop`, `download_master`), со
  значениями той же ФОРМЫ (не тех же значений — иначе тест совпадал бы с
  чужим установленным ПО).
- **Проверено ли, что `expand_env` перед `points_at` не рассыпает
  исправление на раскрытых значениях, а не предположено?** Да —
  `expand_env_then_points_at_matches_an_unquoted_spaced_variable_expansion`
  (своя переменная окружения) и ручная проверка через
  `%HOMEDRIVE%%HOMEPATH%` в round 2 (комбинация не менялась в этом раунде,
  но логика, через которую она проходит, — да, и тест её теперь фиксирует
  явно).
- **Физически ли отсутствуют `raw_value_for_tests`/
  `restore_raw_value_for_tests` в продакшн-бинарнике?** Да —
  `cargo build --all` (без фичи `app`'а) собирается чисто, а сами функции
  за `#[cfg(feature = "test-registry")]`, включённой только в
  `[dev-dependencies]`.
- **Может ли `TestSubkeyGuard` удалить чужой подключ?** Нет —
  `RegCreateKeyExW` + проверка `REG_CREATED_NEW_KEY` перед удалением.
- **Гонится ли имя песочницы между параллельными прогонами?** Нет — имя
  несёт PID процесса.
- **Совпадает ли SAFETY-комментарий с тем, что реально вызывается?** Да —
  оба (создание и то, что стало создание+проверка) называют
  `RegCreateKeyExW`, вызов — тот же.
- **Исправлена ли арифметика в отчёте явной поправкой, а не тихо?** Да —
  раздел Minor 5 выше; абзац round 2 с ошибкой не тронут.
- **Остался ли `sysproxy.rs` нетронутым?** Да —
  `git diff --stat a25283a -- win/crates/winnet/src/sysproxy.rs` пуст.

---

## Fix round 4 (после ревью коммита `80ed80c`, четвёртый независимый ревьюер — заявлен как последний)

Ревьюер извлёк `matches_exe`, `matches_prefix_boundary` и `points_at` дословно в отдельный тестовый бинарник и прогнал через них всё пространство входов; подтвердил, что фича `test-registry` физически отсутствует в продакшн-бинарнике тремя независимыми способами (`cargo tree -e features,no-dev`, отдельный крейт без фичи не собирается с явной ошибкой "configured out", отсутствие следов в собранном `.rlib`/`.exe`) — отметив, что моё обоснование в предыдущем отчёте («`cargo build --all` собирается чисто») доказательством не было, хотя утверждение оказалось верным. `TestSubkeyGuard`, побайтовая идентичность `sysproxy` и все зафиксированные ограничения — в порядке. **Находки B/C/D/E/F предыдущего раунда закрыты.** Один Important и четыре Minor; итоговый коммит — `fbf6fc6`, поверх `80ed80c`, ветка та же.

### Important — находка A, четвёртый раз, но другого класса

Согласен с диагнозом полностью. Три предыдущих раунда чинили СРАВНЕНИЕ НАПИСАНИЙ (побайтовое → кавычки/регистр → аргументы → пробел в пути) — и оно в итоге стало корректным для любых написаний одной и той же ЛИТЕРАЛЬНОЙ строки. Но остался класс, который сравнение написаний не может увидеть по построению: альтернативные написания одного и того же ФАЙЛА — прямые слэши (`C:/Program Files/...`), сегменты `.`/`..`, короткое 8.3-имя (`PROGRA~1`).

Самое важное в находке — не сами эти формы (маловероятные для `Run` конкретно), а то, что напрямую производит их НАША СОБСТВЕННАЯ запись: `env::current_exe()` возвращает путь в ТОЙ ФОРМЕ, в которой был запущен процесс. Человек включает автозапуск, запустив программу из ярлыка или консоли с коротким путём — `enable` пишет в `Run` именно короткую форму. Следующий обычный запуск (из проводника, из своего же автозапуска в следующий раз) резолвится в длинную форму — и наша ЖЕ запись перестаёт себя узнавать. Ни инсталлятор, ни правка руками для этого не нужны — это баг в собственном пути записи.

**Исправление (`autostart.rs`)** — тот же ход, что и round 3, на шаг ниже: перестать сравнивать написания и спросить файловую систему, как это делает `CreateProcess`.

- `matches_exe_by_identity(candidate, exe_canonical)` — резолвит `candidate` через `fs::canonicalize` и сравнивает с уже резолвленным `exe_canonical`. Оба канонизированы ДО сравнения (на Windows `canonicalize` даёт `\\?\`-verbatim путь — сравнивать канонизированный с сырым нельзя, они не совпадут никогда, даже будучи одним файлом). `false`, если `candidate` не резолвится вовсе (устаревшая запись на удалённый файл — это `false`, а не ошибка) и `false`, если резолвится в ДРУГОЙ, существующий файл (коллизия по написанию с чем-то реальным).
- `whitespace_boundary_prefixes(raw)` — перебирает все точки, где неквотированная командная строка могла бы оборваться в путь (перед каждым пробельным разделителем, плюс строка целиком) — тот же перебор, что делает сам `CreateProcess`.
- `points_at`: канонизирует `exe` один раз; для квотированного сегмента и для каждого кандидата из `whitespace_boundary_prefixes` сначала пробует identity, и только если ни один не резолвился — откатывается на прежнее сравнение написаний (`matches_exe`/`matches_prefix_boundary`), которое остаётся ПОДСТРАХОВКОЙ ровно на случай синтетических/удалённых путей.

Двенадцать новых тестов на identity-совпадение, включая forward slashes и `..`-сегменты (оба — против **реального** текущего тестового бинарника: `fs::canonicalize` нечего резолвить для синтетического пути) и коллизию по префиксу, где файл-коллизия ДЕЙСТВИТЕЛЬНО существует на диске (обязана остаться `false` — иначе идентичность стала бы новым источником ложных срабатываний).

**8.3 короткое имя намеренно не покрыто автоматическим тестом.** Оно зависит от того, включена ли генерация коротких имён на конкретном томе (NTFS `8dot3name`) — параметр, который на многих системах давно отключён из соображений производительности/безопасности. Захардкодить предположение вида `PROGRA~1` было бы хрупким тестом, зависящим от окружения, а не от кода. Вместо этого проверено вручную на реальном `Run` этой машины (раздел ниже) — на этой машине генерация коротких имён включена, короткое имя реально существует, и `is_enabled()` его корректно распознаёт.

### Minor 1 — рудимент брошенного разбора командной строки

Согласен: `matches_exe(raw.split_whitespace().next()...)` был живым только для значений с ведущим пробелом (probe ревьюера это показал) и не был протестирован. Выбрал первый вариант: `points_at` теперь делает `raw.trim()` перед всем остальным — это заодно чинит и настоящий, отдельный случай: значение вида ` "C:\...\proxypilot.exe"` (пробел ПЕРЕД открывающей кавычкой) раньше не опозналось бы как квотированное вовсе, потому что `strip_prefix('"')` требует, чтобы кавычка шла первым байтом. После `trim()` фолбэк `split_whitespace().next()` стал доказуемо избыточным — `matches_prefix_boundary` уже покрывает всё, что он раньше давал уникально, — и удалён целиком.

### Minor 2 — обработанные, но непроверенные случаи

Восемь новых тестов пиннят: конечные пробелы (квотированное и неквотированное значение — оба уже работали корректно и без изменений в логике, `matches_prefix_boundary`'s граница по пробелу это уже покрывала), таб как разделитель (`char::is_whitespace()` уже считает `\t` пробельным), ведущий пробел (закрыт Minor 1 выше), значение только из пробелов (после `trim()` — пустая строка, `false`), значение только из аргументов (`--min` без пути — `false`), незакрытая кавычка с содержимым, похожим на аргумент (вся строка после кавычки становится одним «путём» целиком и не совпадает — то же самое сделал бы и сам `CreateProcess`, не сумев найти такой файл), и две дополнительные префиксные коллизии по именам, названным в ревью — `proxy.exe2` и `pp.exe_old\`.

### Minor 3 — не-ASCII свёртка регистра

Согласен: докблок `matches_exe` давно заявляет `to_lowercase()` вместо `eq_ignore_ascii_case()` ради путей с не-ASCII символами (продукт русскоязычный), но до этого раунда ни один тест этого не проверял. Добавлен `points_at_matches_a_cyrillic_path_regardless_of_case` (кириллический путь, регистр отличается, квотированный и неквотированный вариант) и `points_at_does_not_conflate_sharp_s_with_double_s` — закрепляет, что `ß`/`SS` осознанно НЕ считаются эквивалентными (`to_lowercase()` не превращает "SS" в "ß"), то есть текущее поведение не пытается тянуть Unicode-эквивалентность регистра дальше простого `to_lowercase()`.

### Minor 4 — тип значения при восстановлении

Согласен: `restore_raw_value_for_tests` писала всегда через `set_string` (`REG_SZ`) безусловно — реальная `REG_EXPAND_SZ`-запись (обычный тип для инсталляторских `%ProgramFiles%\...`) откатилась бы в `REG_SZ`, и `%VAR%` в восстановленном значении перестал бы раскрываться при следующем реальном чтении Windows.

**Исправление:** в `sysproxy::RegKey` добавлены два аддитивных метода — `query_string_with_type` (как `query_string`, но не отбрасывает тип) и `set_string_as` (как `set_string`, но тип — параметр; тело намеренно НЕ переиспользует `set_string` через делегирование, чтобы последняя, а с ней и её единственный вызывающий `apply`, осталась нетронутой ни строкой). Оба — за той же фичей `test-registry`, что и вызывающая их тестовая инфраструктура `autostart`, иначе в сборке без фичи у них оказалось бы ноль вызывающих (`dead_code`). `raw_value_for_tests`/`restore_raw_value_for_tests` теперь несут `(String, u32)` — тип как простое число, а не `windows::...::REG_VALUE_TYPE`: вызывающий крейт (`main.rs`) не должен заводить зависимость от `windows` ради типа, который он всё равно только проносит туда и обратно, не заглядывая внутрь. Оба вызывающих места (`autostart.rs`'s собственный ignored-тест и `main.rs`'s) обновлены под новую сигнатуру. Новый неignored тест `restore_raw_value_preserves_the_original_reg_expand_sz_type` — против собственной песочницы, не настоящего `Run` — пишет `REG_EXPAND_SZ`, гоняет через `raw_value_at`/`restore_raw_value_at`, проверяет, что тип и значение вернулись такими же.

### Прогон трёх команд CI после fix round 4

**`cargo test --all`** — сводка по крейтам:

```
running 106 tests  (proxypilot-app)
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.62s

running 69 tests   (proxypilot-bridge, lib)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.06s

running 0 tests    (proxypilot-bridge, bin)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 2 tests    (proxypilot-bridge, tests/cli.rs)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

running 48 tests   (proxypilot-core)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 60 tests   (proxypilot-winnet)
test result: ok. 58 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s
  ↳ ignored: autostart::tests::enable_then_disable_round_trip_on_the_real_registry
  ↳ ignored: events::tests::watch_a_real_network_change

Doc-tests: 0/0/0 по трём крейтам, всё ok
```

Итого: **282 passed, 0 failed, 3 ignored** (было 267/3). Прирост целиком в `proxypilot-winnet`: 43 → 58, ровно +15 новых тестов (12 на identity-сравнение и разбор из чек-листа Minor 1/2, 2 на не-ASCII регистр из Minor 3, 1 на сохранение типа из Minor 4).

Отдельно проверено, что фича `test-registry` реально отсутствует в продакшн-конфигурации:
```
$ cargo build -p proxypilot-winnet                      # без фичи — чисто, 0 предупреждений
$ cargo build -p proxypilot-winnet --features test-registry  # с фичей — чисто, 0 предупреждений
$ cargo build --all                                      # весь workspace, без фич app'а — чисто
```
Оба гейта (`raw_value_for_tests`/`restore_raw_value_for_tests` в `autostart.rs` и `query_string_with_type`/`set_string_as` в `sysproxy.rs`, плюс приватные `raw_value_at`/`restore_raw_value_at`) синхронизированы: без фичи ни один из них не компилируется, и убрать фичу с одного конца цепочки, оставив её на другом, тут же дало бы `dead_code`-предупреждение — сборка `-D warnings` поймала бы рассинхронизацию сама.

**`cargo clippy --all-targets -- -D warnings`**

```
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.46s
```

Ноль предупреждений, код выхода 0. Отдельно: `cargo clippy -p proxypilot-winnet --all-targets --features test-registry -- -D warnings` — тоже чисто.

**`cargo fmt --all --check`**

Первый прогон нашёл два расхождения в `autostart.rs` (порядок `#[cfg(feature)]`-гейтнутого импорта относительно обычного, и один многострочный `assert_ne!`) — `cargo fmt --all` поправил. Повторный `--check`: пустой вывод, код выхода 0.

### Ручная проверка на реальном реестре, пятый раз — с прицелом на альтернативные написания

Тот же приём (временный `cargo example`, бинарник скопирован в путь с пробелом и запущен оттуда).

**1. Снимки до:** `Run` — идентичен снимкам из всех прошлых раундов (те же 10 записей). `HKCU\Software` — 71 подключ, ни одного `ProxyPilot*`.

**2. Базовая проверка:**
```
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(false)
```

**3. Прямые слэши:**
```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value 'C:/Users/User/AppData/Local/Temp/Program Files Test/proxypilot.exe'
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(true)
```

**4. `..`-сегмент:**
```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" `
      -Value 'C:\Users\User\AppData\Local\Temp\Program Files Test\..\Program Files Test\proxypilot.exe'
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(true)
```

**5. 8.3 короткое имя — сначала получено настоящее короткое имя реального файла:**
```
PS> (New-Object -ComObject Scripting.FileSystemObject).GetFile('...\Program Files Test\proxypilot.exe').ShortPath
C:\Users\User\AppData\Local\Temp\PROGRA~1\PROXYP~1.EXE
```
Генерация коротких имён на этой машине включена. Записано в `Run` и без кавычек, и в кавычках с аргументом:
```
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value 'C:\Users\User\AppData\Local\Temp\PROGRA~1\PROXYP~1.EXE'
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(true)

PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value '"C:\Users\User\AppData\Local\Temp\PROGRA~1\PROXYP~1.EXE" -min'
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(true)
```

**6. Контроль — коллизия по префиксу с ДЕЙСТВИТЕЛЬНО существующим файлом:**
```
$ copy proxypilot.exe proxypilot.exe.bak2
PS> Set-ItemProperty ...\Run -Name "ProxyPilot" -Value '...\proxypilot.exe.bak2'
$ "...\Program Files Test\proxypilot.exe" check
is_enabled() = Ok(false)
```

**7. `disable()` и снимки после:**
```
$ "...\Program Files Test\proxypilot.exe" disable
is_enabled() = Ok(false)
```
`Run` после — побайтово совпадает со снимком «до» (10 записей, `ProxyPilot` отсутствует). `HKCU\Software` — снова 71 подключ, ни одного `ProxyPilot*`.

### Самопроверка по новому чек-листу

- **Резолвятся ли прямые слэши, `..`-сегменты и 8.3 короткое имя в идентичный файл?** Да — автотесты на первые два (против реального тестового бинарника) и ручная проверка на всех трёх, включая 8.3, на реальном `Run` (разделы 3–5).
- **Сравниваются ли канонизированные пути только с канонизированными, никогда с сырыми?** Да — `exe_canonical` считается один раз в `points_at`, обе стороны в `matches_exe_by_identity` — результат `fs::canonicalize`.
- **Остаётся ли коллизия по префиксу с ДЕЙСТВИТЕЛЬНО существующим файлом ложной?** Да — `points_at_is_false_when_the_prefix_collision_file_genuinely_exists` (автотест) и раздел 6 ручной проверки (реальный `Run`).
- **Остаётся ли устаревшая запись на удалённый файл `false`, а не ошибкой?** Да — `matches_exe_by_identity` возвращает `false` из ветки `Err(_)` `fs::canonicalize`, без паники и без `?`.
- **Убран ли рудимент разбора командной строки, или запинен явно?** Убран — `raw.trim()` в начале `points_at` сделал его доказуемо избыточным.
- **Пиннуты ли все перечисленные в ревью обработанные случаи?** Да — восемь тестов, по одному на каждый (конечные пробелы×2, таб, ведущий пробел, только пробелы, только аргументы, незакрытая кавычка, две доп. коллизии).
- **Проверена ли не-ASCII свёртка регистра, и что она не переусердствует (`ß`/`SS`)?** Да — оба теста добавлены.
- **Сохраняет ли восстановление тип `REG_EXPAND_SZ`?** Да — `restore_raw_value_preserves_the_original_reg_expand_sz_type`, неignored, против песочницы.
- **Синхронизированы ли гейты `test-registry` на обоих концах цепочки (иначе `dead_code`)?** Да — `cargo build` и в фиче, и без неё чист с обеих сторон.
- **Остался ли `sysproxy.rs`'s регион `mod tests` побайтово идентичным?** Да — `diff` пуст (см. выше).

---

## Fix round 5 (после ревью коммита `fbf6fc6`, четвёртый ревьюер — cleanup перед публикацией, заявлен как последний)

Ревьюер вернул **Approved with fixes: без Critical, без Important** — вытащил `matches_exe`, `matches_prefix_boundary` и `points_at` дословно в отдельный тестовый крейт и прогнал через них `PROGRA~1`, `.\`, `..`, прямые слэши, конечные точки и конечные пробелы; подтвердил, что обе стороны сравнения канонизируются и никогда не сравниваются канонизированная с сырой, что ложное срабатывание недостижимо, и что откат на сравнение написаний реально срабатывает, когда наш собственный exe не резолвится. Класс дефекта для написаний путей закрыт. Дальше — код публикуется в открытый репозиторий сегодня, поэтому косметика важнее обычного. Один пункт по существу (комментарий, который лжёт), три по гигиене тестов, один pre-existing докблок в другом крейте, поправка к отчёту, докблок-оговорка о границах фикса, и главное — MSRV, заявленный неверно, плюс то, что он открывает в clippy. Итоговый коммит — `e31525d`, поверх `fbf6fc6`, ветка та же.

### 1. Комментарий, который лжёт — sysproxy.rs

Согласен: докблок `set_string_as` называл `apply` «единственным вызывающим» `set_string`, но `autostart::enable_at` тоже его зовёт (с тех пор, как `autostart.rs` стал переиспользовать `sysproxy::RegKey` в round 1). Исправлено — названы оба вызывающих; обоснование «без делегирования, чтобы не тронуть ни `set_string`, ни её вызывающих» не изменилось по существу.

### 2. Гонка данных на `std::env::set_var` — autostart.rs

Согласен, и диагноз важный: `set_var`/`remove_var` мутируют окружение ПРОЦЕССА целиком, а тесты этого файла выполняются в общем пуле потоков одного прогона — параллельно с этим же тестом могли выполняться другие, читающие переменные окружения (не в этом файле, но `cargo test` не гарантирует изоляцию по умолчанию). Это ровно тот класс гонки, из-за которого в редакции 2024 обе функции помечены `unsafe` — этот крейт просто на 2021 и потому не ловит это на этапе компиляции.

**Исправление:** `expand_env_then_points_at_matches_an_unquoted_spaced_variable_expansion` больше не пишет в окружение вовсе. Вместо своей переменной — стандартная `%ProgramFiles%`, которую тест только ЧИТАЕТ (`std::env::var`, не `set_var`): она есть на любой Windows, и её значение почти всегда содержит пробел («C:\Program Files») независимо от языка системы, потому что физическое имя папки Microsoft не локализует.

### 3. Уборка не через `Drop` и непроверенная достижимость ветки — autostart.rs

Согласен с обоими пунктами. `points_at_is_false_when_the_prefix_collision_file_genuinely_exists` убирал временный файл голым `let _ = fs::remove_file(...)` ПОСЛЕ вызова `points_at` — паника внутри самой проверки (или где-то раньше в теле теста) оставила бы файл висеть рядом с тестовым бинарником, тогда как вся остальная уборка в этом файле (`TestSubkeyGuard`, `RestorePrevious`) идёт через `Drop`. Отдельно: тест не проверял, что `fs::canonicalize` для файла-коллизии вообще успевает выполниться, — он остался бы зелёным даже в мире, где ветка identity-сравнения для несуществующих/ошибочных путей всегда возвращает `false` без реальной проверки коллизии.

**Исправление:** локальный `RemoveOnDrop`, plus явный `assert!(fs::canonicalize(&collision).is_ok(), ...)` перед основной проверкой — тест теперь доказывает, что путь идентичности действительно пройден, а не просто получил `false` по умолчанию.

### 4. Докблок `supervisor.rs` — pre-existing, но становится публичным сегодня

Согласен с фактом (не моя правка изначально): `Router::set` имеет ноль непроверочных вызывающих, реальный путь — `set_if_changed` (сам файл ниже в этом же докблоке уже верно об этом говорит). Строка 4 исправлена, чтобы не противоречить остальному тексту того же комментария.

### 5. Ошибка в отчёте (round 4)

Согласен: раздел round 4 «Important» утверждает «Двенадцать новых тестов на identity-совпадение» — но из пятнадцати новых тестов того раунда только три (`points_at_matches_forward_slashes_via_filesystem_identity`, `points_at_matches_a_path_with_dot_dot_segments_via_filesystem_identity`, `points_at_is_false_when_the_prefix_collision_file_genuinely_exists`) реально заходят в `matches_exe_by_identity`; остальные девять — пины на прежнее сравнение написаний (аргументы, регистр, пробелы и т. д.), которые под identity-веткой не проходят вовсе (синтетические, несуществующие пути). Собственная сводная строка того же отчёта чуть ниже («12 новых тестов на identity-совпадение… против реального бинарника») по факту тоже смешивает identity-тесты с пинами написаний в одну фразу — реально к идентичности файла относятся только упомянутые три. Как и с находкой D в round 1: исходный абзац round 4 оставлен как есть, эта запись — явная поправка.

### 6. Докблок `autostart.rs` преувеличивал границы фикса

Согласен: до этой правки докблок читался так, будто identity-сравнение закрывает класс «alternate spellings» безусловно. Ревьюер нашёл два остаточных отверстия — `fs::canonicalize(r"C:\Windows\explorer")` (без `.exe`) не резолвится, хотя `CreateProcess` дописал бы расширение и запустил; то же для обёртки `cmd /c start "" "...\proxypilot.exe"`. Оба требуют, чтобы кто-то ТРЕТИЙ сам вписал такую форму под именем `ProxyPilot` в `Run` — `enable` никогда не пишет ни ту, ни другую. Ревьюер прямо попросил не гнаться за этим (риск низкий, не нулевой), только не оставлять докблок читающимся как «класс закрыт безусловно». Добавлено одно уточняющее замечание в докблок модуля: механизм закрывает разные НАПИСАНИЯ пути, называющего файл, — не любую команду, которая его в итоге запускает.

### 7. Заявленный MSRV был ложью

Согласен, и проверил сам на всех трёх установленных тулчейнах:

```
$ cargo +1.75.0 check --all --all-targets --locked
error: failed to download `time-macros v0.2.32`
  feature `edition2024` is required, ... не стабилизирована в этой версии Cargo (1.75.0)

$ cargo +1.85.0 check --all --all-targets --locked
error: rustc 1.85.0 is not supported by the following packages:
  time@0.3.55 requires rustc 1.88.0
  time-core@0.1.9 requires rustc 1.88.0

$ cargo +1.88.0 check --all --all-targets --locked
    Checking proxypilot-core v0.1.0 (...)
    Checking proxypilot-bridge v0.1.0 (...)
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 27.23s
```

`win/Cargo.toml`: `rust-version = "1.75"` → `"1.88"`, с комментарием, называющим обе причины отказа предыдущих версий.

**Последствие, о котором предупредил ревьюер, подтвердилось:** после подъёма MSRV `cargo clippy --all-targets -- -D warnings` открыл четыре MSRV-зависимые подсказки — API, которых не было в 1.75, но которые есть в 1.88. Все четыре механические:

- `sysproxy.rs::decode_utf16_sz` — `bytes.chunks_exact(2)` → `bytes.as_chunks::<2>().0.iter()`. Поведение не изменилось: оба варианта молча отбрасывают лишний байт при нечётной длине. Это `sysproxy` — 9 тестов прогнаны отдельно и остались зелёными; регион `mod tests` файла побайтово идентичен `fbf6fc6` (см. прогон ниже), правка целиком внутри тела `decode_utf16_sz`.
- `icons.rs` — `px.chunks_exact(4).any(|p| p[3] > 0)` → `px.as_chunks::<4>().0.iter().any(|p| p[3] > 0)`.
- `http.rs` — `std::iter::repeat(b'x').take(9000)` → `std::iter::repeat_n(b'x', 9000)`.
- `settings_page.rs::health_text` — `addr.map_or(true, str::is_empty)` → `addr.is_none_or(str::is_empty)`; заодно убран соседний комментарий, объяснявший, почему `is_none_or` раньше не годился («стабильно только с 1.82, а MSRV — 1.75») — он был единственным местом в кодовой базе, ссылавшимся на старый MSRV (`grep -rn "1\.75\|MSRV"` после правки — пусто).

Ни одного `#[allow(...)]` — все четыре исправлены по существу.

### 8. Тестовые фикстуры с реальным адресом офиса

Согласен: `core/src/bypass.rs` (`cidr_matches_addresses_inside`, `full_prefix_cidr_matches_single_address`) и `core/src/config.rs` (`default_no_proxy_covers_local_ranges`) использовали `203.0.113.246`/`203.0.113.247`/`203.0.113.1` — специфичный, похожий на настоящий адрес офиса, при том что все три теста проверяют лишь принадлежность `192.168.0.0/16` (содержимое `DEFAULT_NO_PROXY`), и конкретный хост внутри диапазона к делу не относится. Заменено на `192.168.1.246`/`192.168.1.247`/`192.168.1.1` — тот же диапазон, другой, безобидный хост. (Заодно поправлена и вторая находка того же адреса в `bypass.rs`, `full_prefix_cidr_matches_single_address`, — она не была явно названа в ревью, но несёт то же самое число тремя тестами ниже в том же файле, и оставлять его нетронутым означало бы не выполнить исходную задачу до конца.)

### Прогон трёх команд CI плюс явный MSRV-чек после fix round 5

**`cargo test --all`** — сводка по крейтам:

```
running 106 tests  (proxypilot-app)
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.58s

running 69 tests   (proxypilot-bridge, lib)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

running 0 tests    (proxypilot-bridge, bin)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 2 tests    (proxypilot-bridge, tests/cli.rs)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

running 48 tests   (proxypilot-core)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 60 tests   (proxypilot-winnet)
test result: ok. 58 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s
  ↳ ignored: autostart::tests::enable_then_disable_round_trip_on_the_real_registry
  ↳ ignored: events::tests::watch_a_real_network_change

Doc-tests: 0/0/0 по трём крейтам, всё ok
```

Итого: **282 passed, 0 failed, 3 ignored** — счётчик не изменился относительно round 4 (этот раунд не добавлял тестов, только чинил существующие: комментарии, `Drop`-уборка, гонка окружения, четыре clippy-фикса, две IP-фикстуры).

Отдельная проверка sysproxy: `git diff fbf6fc6 -- win/crates/winnet/src/sysproxy.rs` меняет только тело `decode_utf16_sz` и докблок `set_string_as`; регион `mod tests` побайтово идентичен:
```
$ diff <(git show fbf6fc6:win/crates/winnet/src/sysproxy.rs | sed -n '/^mod tests/,$p') \
       <(sed -n '/^mod tests/,$p' win/crates/winnet/src/sysproxy.rs)
$ echo $?
0
```

**`cargo clippy --all-targets -- -D warnings`**

```
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-bridge v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.63s
```

Ноль предупреждений, код выхода 0. Отдельно: `cargo clippy -p proxypilot-winnet --all-targets --features test-registry -- -D warnings` — тоже чисто.

**`cargo fmt --all --check`** — пустой вывод, код выхода 0 (форматирование не потребовалось после точечных правок).

**`cargo +1.88.0 check --all --all-targets --locked`** (явно запрошено ревьюером, после подъёма MSRV):

```
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-bridge v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.17s
```

Ноль ошибок, код выхода 0 — подтверждает, что новый MSRV `1.88` не просто задекларирован, а действительно достаточен: `as_chunks`/`is_none_or`, добавленные в этом же раунде, реально доступны на этой версии, не только на текущем `rustc 1.98.0`.

### Ручная проверка на реальном реестре, шестой раз

Этот раунд не менял логику сравнения `points_at`/`matches_exe_by_identity` — только гигиену тестов, докблоки и MSRV/clippy/IP-фикстуры, ни одна из которых не трогает реальный реестр напрямую. Проверка — что автотест, трогающий песочницу, по-прежнему ничего не задевает в настоящем `Run`/`Software`.

**До:**
```
Run: те же 10 записей, что и во всех предыдущих раундах.
HKCU\Software: 71 подключ, ни одного ProxyPilot*.
```

**Прогон `cargo test -p proxypilot-winnet autostart::`:** 36 passed, 1 ignored (включая `enable_disable_and_is_enabled_round_trip_against_a_private_scratch_key` и новый по форме, но не по коду, `restore_raw_value_preserves_the_original_reg_expand_sz_type` — оба против собственной песочницы `-{pid}`, не настоящего `Run`).

**После:**
```
ProxyPilot value: <absent, as expected>
Run: те же 10 записей, побайтово.
HKCU\Software: 71 подключ, ни одного ProxyPilot*.
```

Идентично снимку «до».

### Самопроверка по новому чек-листу

- **Названы ли оба вызывающих `set_string` в докблоке `set_string_as`?** Да — `apply` и `autostart::enable_at`.
- **Мутирует ли что-нибудь общее окружение процесса в тестах этого файла?** Нет — `std::env::set_var`/`remove_var` убраны полностью, единственное обращение к окружению — `std::env::var("ProgramFiles")` (чтение).
- **Убирает ли `points_at_is_false_when_the_prefix_collision_file_genuinely_exists` за собой файл при панике?** Да — `RemoveOnDrop`. Доказана ли достижимость ветки идентичности? Да — явный `assert!(fs::canonicalize(&collision).is_ok())` перед основной проверкой.
- **Согласован ли докблок `supervisor.rs` с реальным вызовом (`set_if_changed`, не `set`)?** Да.
- **Исправлена ли ошибка отчёта явной поправкой, а не тихо?** Да — раздел 5 выше; исходный абзац round 4 не тронут.
- **Оговорены ли границы identity-фикса в докблоке `autostart.rs` (написания vs произвольные команды-обёртки)?** Да — новый абзац, без попытки закрыть сами эти два случая.
- **Верен ли теперь заявленный MSRV?** Да — проверено на реальных тулчейнах 1.75.0 (отказ), 1.85.0 (отказ), 1.88.0 (проходит), включая явный `cargo +1.88.0 check --all --all-targets --locked` после всех правок этого раунда.
- **Есть ли хоть один `#[allow(...)]` среди новых clippy-фиксов?** Нет — все четыре исправлены по существу.
- **Остаются ли тесты `bypass`/`config` внутри `192.168.0.0/16` после замены адреса?** Да — `192.168.1.246`, `192.168.1.247`, `192.168.1.1` все внутри диапазона.
- **Остался ли `sysproxy.rs`'s регион `mod tests` побайтово идентичным?** Да — `diff` пуст (см. выше); 9 тестов зелёные.

## Fix round 6 (найдено CI, не ревью — `HKCU\...\Run` не гарантированно существует)

Не ревью, а красный CI на `windows-latest`: 9 из 16 прогонов подряд падали
`process completed with exit code 1` без единого имени теста — GitHub Actions
логи требуют авторизации даже у публичного репозитория. Первым шагом workflow
(`.github/workflows/ci.yml`, шаг «Тесты») был переделан так, чтобы упавшие
тесты назывались через `::error::` — это попадает в аннотации прогона, а они
читаются через API без авторизации. Следующий же красный прогон назвал тест:

```
test tests::win_autostart_is_enabled_does_not_fail ... FAILED
panicked at crates\app\src\main.rs:1454
```

### Диагноз

`autostart::is_enabled_at` (`crates/winnet/src/autostart.rs`) открывала
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` через `RegKey::open`,
которая пробрасывает ошибку Windows, если подключа нет. Но этот подключ **не
гарантирован**: он существует только после того, как что-то в системе хоть
раз написало в него автозапуск. Свежий профиль пользователя (ровно то, чем
отличаются образы раннеров GitHub между прогонами) может его не иметь вовсе —
это и было источником «интермиттентности», а не флакующим тестом.

Находка касалась трёх функций, не одной:

1. **`is_enabled_at`** — отсутствие ключа пробрасывалось как `Err`, хотя это
   такой же честный ответ «выключено», как и уже обрабатываемое отсутствие
   значения внутри существующего ключа.
2. **`enable_at`** (самая заметная находка — её не поймал ни один
   существовавший тест) — открывала ключ только на запись (`RegKey::open`,
   `KEY_WRITE`), которая тоже отказывает, если ключа нет. На свежем профиле
   включить автозапуск было нельзя **никаким способом через интерфейс** —
   ни разу, ни один пользователь такого профиля.
3. **`disable_at`** — тот же отказ там, где собственный докблок `disable`
   обещает идемпотентность («повторный вызов, как и вызов при уже выключенном
   автозапуске, не ошибка») — отсутствие ключа целиком это то же самое
   состояние, что докблок уже обещал покрывать.

### Исправление

Новый код не трогает ничего в существующих `RegKey::open`/`apply`/`read` —
`sysproxy.rs`'s регион `mod tests` остался побайтово идентичным (`diff` пуст,
9 тестов не изменились ни строкой), `apply()` по-прежнему пишет
ProxyServer → ProxyOverride → ProxyEnable последним. Добавлены два новых
метода `RegKey` в `crates/winnet/src/sysproxy.rs`:

- `open_if_exists(root, subkey, access) -> Result<Option<Self>, WinNetError>`
  — как `open`, но `Ok(None)`, если подключа нет; любая другая ошибка (нет
  прав, битый куст) пробрасывается как есть, не маскируется под «нет ключа».
  Использует тот же приём, что уже был в `openvpn::open_key` (сравнение кода
  ошибки с `ERROR_FILE_NOT_FOUND` через `HRESULT::from_win32`), просто
  вынесенный в саму обёртку ради переиспользования сразу двумя вызывающими.
- `open_or_create(root, subkey, access) -> Result<Self, WinNetError>` — как
  `open`, но через `RegCreateKeyExW` вместо `RegOpenKeyExW`: создаёт подключ,
  если его нет, вместо отказа.

`crates/winnet/src/autostart.rs`:

- `is_enabled_at` теперь зовёт `open_if_exists(..., KEY_READ)`; `None` →
  `Ok(false)`.
- `enable_at` теперь зовёт `open_or_create(..., KEY_WRITE)` вместо `open`.
- `disable_at` теперь зовёт `open_if_exists(..., KEY_WRITE)`; `None` →
  `Ok(())`. Докблок `disable` уточнён: идемпотентность явно распространена
  и на случай отсутствующего `Run` целиком.

### Тесты

Четыре новых теста в `crates/winnet/src/autostart.rs`, против одноразового
подключа-песочницы с суффиксом PID (`AbsentSubkeyGuard` — тот же приём, что
и у существующего `TestSubkeyGuard`, но без создания ключа при входе; `Drop`
всё равно пытается удалить, на случай если тестируемый код его создал):

- `is_enabled_at_is_ok_false_when_the_registry_key_itself_is_missing` —
  заведомо отсутствующий подключ → `Ok(false)`, не ошибка.
- `enable_at_creates_the_missing_registry_key_and_writes_the_value` —
  `enable_at` на отсутствующем подключе создаёт его и пишет значение;
  проверено round-trip через `is_enabled_at`.
- `disable_at_is_ok_when_the_registry_key_itself_is_missing` — заведомо
  отсутствующий подключ → `Ok(())`.
- `is_enabled_at_surfaces_a_non_missing_key_error_instead_of_treating_it_as_false`
  — подключ с именем длиннее 255 символов (предел одного компонента пути
  реестра) даёт Windows-ошибку, заведомо отличную от `ERROR_FILE_NOT_FOUND`
  (проверено явным `assert_ne!` на коде ошибки внутри теста); эта ошибка
  обязана остаться `Err`, а не тихо стать `Ok(false)` — без этого теста
  правка выше могла бы случайно превратиться в «любая ошибка → выключено» и
  спрятать реальный отказ (нет прав, битый куст) под честным на вид
  ответом.

Настоящий `HKCU\...\Run` этой машины не тронут ни одним из новых тестов —
все работают против подключей `Software\ProxyPilotAutostartSelfTest-<label>-
<pid>`, которых до и после прогона в реестре нет. Проверено count-ом значений
в `Run` через `Get-Item ... | ValueCount` до и после: **10 и 10**, число не
изменилось.

### Прогон трёх команд CI

Все три — на этой же ветке, с `CARGO_TARGET_DIR`, указывающим в отдельный
каталог (не `target/`, где уже собран и запущен другой экземпляр
`proxypilot.exe`, чтобы не задеть линковку работающего процесса).

**`cargo fmt --all --check`** — пустой вывод, код выхода 0.

**`cargo clippy --all-targets -- -D warnings`**

```
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-netsvc v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.74s
```

Ноль предупреждений, код выхода 0.

**`cargo test --all`** — хвост:

```
test result: ok. 145 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.60s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.06s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 158 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s
```

`proxypilot-winnet` — 158 passed (было 154 до этого раунда, +4 новых теста),
0 failed, 2 ignored (оба — `#[ignore]`-тесты против настоящего `Run`,
намеренно не гоняются автоматически). `tests::win_autostart_is_enabled_does_not_fail`
(`crates/app/src/main.rs`) — тот самый тест, названный CI в аннотации, — в
зелёной группе (`ok`).

До правки, для контраста: тот же `cargo test --all` на этой же машине сейчас
**тоже проходит** (154 passed в `proxypilot-winnet`, 0 failed) — потому что
на этой машине `HKCU\...\Run` существует (10 записей). Это ожидаемо и
согласуется с диагнозом: баг не флакует сам по себе, он детерминирован
относительно состояния профиля, а не относительно чего-то во времени —
воспроизвести его на машине с уже существующим `Run` нельзя было в принципе,
только на профиле без него (какими и оказались часть образов
`windows-latest`), поэтому воспроизведение здесь идёт через отдельный
подключ-песочницу, который целенаправленно не создаётся, а не через попытку
снести настоящий `Run`.

### `openvpn`'s HKLM-чтение — проверено, тот же баг отсутствует

`openvpn::open_key` (`crates/winnet/src/openvpn.rs`), открывающая
`HKLM\SOFTWARE\OpenVPN`, уже отличает отсутствие подключа от прочих ошибок:

```rust
fn open_key(subkey: PCWSTR) -> Result<Option<RegKey>, WinNetError> {
    match RegKey::open(HKEY_LOCAL_MACHINE, subkey, KEY_READ) {
        Ok(key) => Ok(Some(key)),
        Err(WinNetError::Windows(e)) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
```

Это ровно тот же приём, что теперь вынесен в `RegKey::open_if_exists`, и он
уже был покрыт тестом (`open_key_is_none_for_a_subkey_that_does_not_exist`,
проходит и в этом прогоне). Латентного бага здесь нет — `find_installation`
и так трактует отсутствие ключа `OpenVPN` как «не установлен», что и есть
корректный исход (докблок модуля: «Отсутствие OpenVPN — это `Ok(None)`, а не
ошибка»). Правка `openvpn.rs` не потребовалась и не вносилась.

### Коммит

`fix(win): не падать, если HKCU\...\Run не существует` — три места
(`is_enabled_at`, `enable_at`, `disable_at`), два новых метода `RegKey`
(`open_if_exists`, `open_or_create`), четыре новых теста.
