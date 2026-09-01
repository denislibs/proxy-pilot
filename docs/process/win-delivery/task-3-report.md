# Task 3 report — Проверка и установка обновлений

## Источник

`https://github.com/denislibs/proxy-pilot` — жёстко зашито в
`crates/app/src/update/source.rs` (`OWNER`/`REPO`), не читается из конфига и
не выведено полем на странице настроек. Единственный тумблер — «проверять
или нет», а не «где проверять».

## Устройство

- `crates/app/src/update/version.rs` — чистое сравнение версий
  (`parse_tag`, `decide`). Без сети и без диска.
- `crates/app/src/update/verify.rs` — проверка подписи Authenticode через
  `WinVerifyTrust` (системный API, есть на любой Windows, в отличие от
  `signtool.exe`, которого на машине получателя нет).
- `crates/app/src/update/json.rs` — минимальный собственный разбор JSON.
- `crates/app/src/update/source.rs` — трейт `UpdateSource` + разбор ответа
  GitHub Releases API (`parse_release_response`) + реальная реализация
  `GithubSource` поверх `WinHTTP`.
- `crates/app/src/update/check.rs` — оркестрация: сеть (с таймаутом и в
  `spawn_blocking`) → сравнение версий → скачивание → подпись → откладывание.
- `crates/app/src/update/install.rs` — применение отложенного обновления при
  следующем запуске: переименование текущего exe и файла на его место.
- `crates/core/src/config.rs` — новое поле `check_for_updates` (по умолчанию
  включено).
- `crates/app/src/settings_page.rs` — раздел «Обновления»: тумблер +
  честный текст последнего результата.
- `crates/app/src/main.rs` — `apply_pending_update` в самом начале `run()`
  (до `Config::load`, до COM, до привязки слушателя); `spawn_update_check`
  — фоновая задача с начальной задержкой 5 минут и периодом 24 часа.

## Почему `WinHTTP` и собственный JSON, а не `reqwest`/`serde_json`

Сначала были добавлены `serde_json` и (транзитивно потребовавшийся бы)
TLS-стек. Оказалось, что эта песочница сборки не может обратиться к
`crates.io` за НОВОЙ зависимостью:

```
error: failed to get `directories` as a dependency of package `proxypilot-core v0.1.0 ...`
Caused by: ... SSL connect error (schannel: ... CRYPT_E_REVOCATION_OFFLINE ...)
```

Онлайн-попытка `cargo test` виснет на проверке отзыва сертификата, а
`--offline` честно отвечает `no matching package named serde_json`. Убрал
`serde_json` из `Cargo.toml`, написал минимальный собственный разбор JSON
(`update/json.rs`, 13 тестов, включая суррогатные пары и «поле, текстуально
похожее на другой ключ») и HTTPS-клиент поверх системного `WinHTTP`
(`windows::Win32::Networking::WinHttp`) — тот же принцип, каким `bench.rs`
уже объясняет отсутствие HTTP-библиотеки ради одного статического `GET`.
`Cargo.lock` в итоге не тронут вовсе (`git status` подтверждает) — все новые
возможности `windows` были уже в закешированном крейте 0.58.0, только
включены фичи `Win32_Security_WinTrust` и `Win32_Networking_WinHttp`.

## TDD: реальный порядок работы

Строгий red-first (стаб → падение теста → реализация → зелень) сделан для
`version.rs` и для двух обязательных команд CI (clippy, fmt). Для
остальных пяти файлов (`verify.rs`, `json.rs`, `source.rs`, `check.rs`,
`install.rs`) я писал тесты вместе с реализацией и прогонял их сразу после
первой попытки компиляции — то есть красный/зелёный цикл шёл через
**реальные ошибки компилятора** (для `verify.rs`/`source.rs`, где сигнатуры
`WinVerifyTrust`/`WinHTTP` пришлось нащупывать вживую) и через реальные
прогоны тестов, но не через нарочно упрощённую заглушку теста ради самого
факта красного прогона. Говорю это прямо, а не скрываю: контраст с
`version.rs` ниже виден по содержимому обеих RED-секций.

### RED: `version.rs` — тест до реализации (стаб `todo!()`, дословный вывод)

```
running 18 tests
test update::version::tests::a_tag_that_is_not_a_version_at_all_is_unrecognized ... FAILED
test update::version::tests::a_prerelease_tag_is_never_offered_even_when_numerically_newer ... FAILED
test update::version::tests::ordering_treats_a_full_release_as_newer_than_its_own_prerelease ... FAILED
test update::version::tests::a_current_version_newer_than_published_is_reported_as_such ... FAILED
test update::version::tests::parses_a_prerelease_suffix ... FAILED
test update::version::tests::rejects_extra_numeric_segments ... FAILED
test update::version::tests::rejects_a_dangling_hyphen ... FAILED
test update::version::tests::rejects_a_tag_that_is_not_a_version_at_all ... FAILED
test update::version::tests::a_broken_current_version_is_unrecognized_rather_than_panicking ... FAILED
test update::version::tests::parses_a_plain_tag ... FAILED
test update::version::tests::a_malformed_tag_is_unrecognized_not_treated_as_no_update ... FAILED
test update::version::tests::ordering_breaks_ties_between_prereleases_lexicographically ... FAILED
test update::version::tests::an_equal_published_version_is_up_to_date ... FAILED
test update::version::tests::a_newer_published_version_is_available ... FAILED
test update::version::tests::a_prerelease_tag_numerically_older_is_still_reported_as_prerelease ... FAILED
test update::version::tests::parses_a_tag_without_the_leading_v ... FAILED
test update::version::tests::rejects_non_numeric_segments ... FAILED
test update::version::tests::rejects_too_few_numeric_segments ... FAILED

---- update::version::tests::a_tag_that_is_not_a_version_at_all_is_unrecognized stdout ----

thread 'update::version::tests::a_tag_that_is_not_a_version_at_all_is_unrecognized' (21828) panicked at crates\app\src\update\version.rs:92:5:
not yet implemented: реализация появится следующим шагом TDD

test result: FAILED. 0 passed; 18 failed; 0 ignored; 0 measured; 146 filtered out; finished in 0.00s
```

(Остальные 17 панических блоков стдаут — та же строка `not yet implemented`,
опущены для читаемости; недостающие строки идентичны и легко
воспроизводятся повторным прогоном той же команды.)

### GREEN: `version.rs` после реализации

```
running 18 tests
test update::version::tests::a_prerelease_tag_is_never_offered_even_when_numerically_newer ... ok
test update::version::tests::a_newer_published_version_is_available ... ok
...
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 146 filtered out; finished in 0.00s
```

### RED, реально пойманная компилятором (`verify.rs`, первая попытка компиляции)

```
error[E0308]: mismatched types
  --> crates\app\src\update\verify.rs:94:22
   |
94 |         dwUIContext: 0,
   |                      ^ expected `WINTRUST_DATA_UICONTEXT`, found integer
```

Исправлено (`WINTRUST_DATA_UICONTEXT(0)`), следующая компиляция — чисто, и
все 4 теста зелёные с первого прогона:

```
running 4 tests
test update::verify::tests::a_missing_file_is_refused_not_panicking ... ok
test update::verify::tests::a_file_without_any_signature_is_refused ... ok
test update::verify::tests::a_genuinely_signed_system_file_is_accepted ... ok
test update::verify::tests::a_tampered_signature_is_refused ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 164 filtered out; finished in 0.33s
```

Это не синтетика: `a_genuinely_signed_system_file_is_accepted` копирует
настоящий `C:\Windows\System32\kernel32.dll` (подписан Microsoft на любой
Windows-машине) и проверяет копию как есть — `Ok`.
`a_tampered_signature_is_refused` копирует тот же файл и портит один байт на
смещении 4096 — `Err`. `a_file_without_any_signature_is_refused` проверяет
собственный тестовый бинарь этой же сборки (неподписанный, потому что шаг
подписи задачи 2 требует секрета, которого на этой машине нет) — `Err`.
Ничего из этого не подстроено под ожидаемый ответ: реальный `WinVerifyTrust`
реально различил все три случая с первой попытки.

### RED, реально пойманная компилятором (`source.rs`, восемь ошибок разом)

`WinHttpConnect`/`WinHttpOpen`/`WinHttpOpenRequest` в версии `windows`
0.58.0 возвращают `*mut c_void` напрямую (не `Result`, не именованный тип
`HINTERNET`), а `WINHTTP_NO_PROXY_NAME`/`WINHTTP_NO_REFERER`/
`WINHTTP_NO_PROXY_BYPASS` в этой версии крейта не существуют вовсе —
документация (и просмотренная через `microsoft.github.io/windows-docs-rs`
более новая версия 0.62.2) с реальными сигнатурами 0.58.0 разошлась.
Восемь ошибок компиляции, все по этой причине, дословно:

```
error[E0432]: unresolved imports `windows::Win32::Networking::WinHttp::WINHTTP_NO_PROXY_BYPASS`, `...WINHTTP_NO_PROXY_NAME`, `...WINHTTP_NO_REFERER`
error[E0425]: cannot find type `HINTERNET` in module `windows::Win32::Networking::WinHttp`  (× 3)
error[E0599]: no method named `is_invalid` found for raw pointer `*mut c_void`  (× 2)
error[E0610]: `u16` is a primitive type and therefore doesn't have fields
error[E0308]: mismatched types (WinHttpOpenRequest — не `Result`, а прямой указатель)
```

Исправлено: локальный алиас `type HInternet = *mut core::ffi::c_void`,
`PCWSTR::null()` вместо несуществующих констант, `.is_null()` вместо
`.is_invalid()`, `INTERNET_DEFAULT_HTTPS_PORT` без `.0`. Следующая
компиляция — чисто; 7 тестов чистого разбора (`parse_release_response`,
`split_https_url`) зелёные с первого прогона — сетевая часть
(`GithubSource`/`winhttp::request`) тестами не покрыта вовсе (см. ниже).

### `check.rs`, `install.rs`, `config.rs`, `settings_page.rs`

Компилировались и проходили тесты с первой или второй попытки без
предварительно зафиксированного красного прогона — написаны сразу
реализацией плюс тестами, прогнаны один раз. Честно: не red-first по
методичке, а «написал — прогнал — увидел зелень» (или один раз поправил
явную ошибку компилятора). Единственные реальные RED в этой партии —
две находки clippy и один диф `cargo fmt` (обе секции ниже).

### RED: `cargo clippy --all-targets -- -D warnings`

```
error: this function has too many arguments (8/7)
    --> crates\app\src\main.rs:1182:1
     |
1182 | / fn message_loop(
...
error: field assignment outside of initializer for an instance created with Default::default()
    --> crates\app\src\settings_page.rs:2684:9
     |
2684 |         cfg.check_for_updates = false;
```

Исправлено: `message_loop` теперь принимает готовый `&SettingsDeps` (собран
у вызывающего, тем же приёмом, что уже применён к `open_settings` в задаче
7), а не восемь отдельных параметров; тестовый `Config` собирается через
`Config { check_for_updates: false, ..Config::default() }`.

### GREEN: `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.63s
```

### RED → GREEN: `cargo fmt --all --check` → `cargo fmt --all`

Диф был реальным (форматирование WinHTTP-блока, разбивка длинных строк
JSON-парсера и т. п.) — приведён построчно в терминале при работе, не
воспроизвожу здесь весь диф ради места; после `cargo fmt --all` повторный
`--check` вышел с кодом 0 (`FMT_CLEAN`).

### GREEN: `cargo test --all` (финальный прогон)

```
proxypilot-app       (bin unittests):        205 passed; 0 failed; 1 ignored
proxypilot-app       (tests/version_resource): 1 passed; 0 failed
proxypilot-bridge    (lib unittests):          69 passed; 0 failed
proxypilot-bridge    (tests/cli):               2 passed; 0 failed
proxypilot-core      (lib unittests):          88 passed; 0 failed
proxypilot-icon      (lib unittests):           2 passed; 0 failed
proxypilot-netsvc    (lib unittests):          43 passed; 0 failed
proxypilot-winnet    (lib unittests):         158 passed; 0 failed; 2 ignored
```

Итого **568 passed, 0 failed, 3 ignored** (было 566 передано до этой задачи
по последнему известному прогону — прибавилось 55 тестов задачи 3: 18 в
`version.rs`, 4 в `verify.rs`, 13 в `json.rs`, 7 в `source.rs`, 8 в
`check.rs`, 5 в `install.rs`, плюс 2 новых в `config.rs` и несколько
новых/расширенных в `settings_page.rs`/`websrv.rs`, покрытых существующими
именами тестов, изменёнными под новое поле `SettingsState`).

### MSRV: `cargo +1.88.0 check --all --all-targets --locked`

Чисто, `Cargo.lock` не трогался этой задачей вовсе (`git status` это
подтверждает — единственные тронутые файлы перечислены ниже).

## Как выполнены пять обязательных свойств

1. **Не блокирует старт.** `apply_pending_update` в начале `run()` — это
   только локальные файловые операции (переименование), без сети; сетевая
   фоновая проверка заводится `runtime.spawn` (не `block_on`) с задержкой
   первого тика 5 минут, ПОСЛЕ привязки слушателя, трея и самопроверки.
   Тест `a_hanging_source_does_not_block_the_caller_beyond_the_timeout`
   (`check.rs`) доказывает: источник «висит» секунду реального времени,
   вызывающий получает ответ меньше чем за 500 мс при таймауте 50 мс.
2. **Проверяется подпись, отказ = не установлено.** `verify.rs` —
   `WinVerifyTrust` без единого пути, возвращающего успех при отсутствующей
   или неверной подписи; `check.rs::stage` удаляет скачанный файл и НЕ
   переименовывает его в отложенный при любом отказе `verify`;
   `install.rs::apply_pending_update` перепроверяет подпись ЕЩЁ РАЗ перед
   самим переименованием exe. Ни один код в `Config`/UI не может выключить
   именно эту проверку — выключатель существует только для сетевого опроса.
   **Сегодня сертификата продукта нет, поэтому `verify_authenticode` вернёт
   отказ для любого настоящего релиза `denislibs/proxy-pilot`, и обновление
   не установится никогда** — ожидаемо, не дефект; страница настроек говорит
   об этом честно текстом отказа с причиной, а не молчит и не пишет «всё в
   порядке».
3. **Замена при следующем запуске.** `install.rs::apply_pending_update`
   вызывается в самом начале `run()`, до `Config::load`, COM, привязки
   слушателя и `proxy::take_over` — переименование происходит ДО того, как
   этот запуск взял на себя хоть что-то. При успехе — `relaunch` (новый
   процесс по тому же пути) и немедленный выход текущего без единого
   обращения к реестру. Тест `a_signed_pending_update_swaps_the_file_...`
   доказывает механику переименования на фиктивных путях в этом же реальном
   NTFS этой машины (не на живом `proxypilot.exe`).
4. **Тумблер выключает только проверку.** `Config::check_for_updates`
   читается в начале `update::check::run`; `disabled_check_never_touches_the_source`
   доказывает, что источник не вызывается вовсе. У `verify_authenticode`
   такого параметра нет физически.
5. **Без дифференциальных обновлений.** Один файл (`proxypilot.exe`)
   целиком; `proxypilot-bridge.exe` собирается и подписывается тем же
   конвейером и версией задачи 4, отдельно не отслеживается.

## Что проверено ТОЛЬКО рассуждением и НЕ может быть проверено без сети/сертификата

Хард-лимит задачи — никаких сетевых обращений в тестах, и я его не обошёл:

- `update::source::GithubSource` (реальные вызовы `WinHTTP`:
  `WinHttpOpen`/`Connect`/`OpenRequest`/`SendRequest`/`ReceiveResponse`/
  `ReadData`) — не вызвана ни разу ни одним тестом. Компилируется и линкуется
  чисто (доказано `cargo check`/`clippy`/`test` целиком), но её сетевое
  поведение подтверждено только чтением документации Microsoft, не запуском
  — та же оговорка, какой задачи 2 и 4 честно отчитывались о `signtool
  sign`/`gh release create`.
- Форма ответа GitHub API проверена READ-ONLY через `WebFetch` (не тест, не
  код, не сохранено в репозитории) на живом `api.github.com`:
  `GET /repos/denislibs/proxy-pilot/releases` вернул `[]` (релизов
  действительно нет — соответствует ограничению «не создавать тег»),
  `GET /repos/denislibs/proxy-pilot/releases/latest` вернул `404`, и форма
  объекта релиза (`tag_name`, `assets[].name`,
  `assets[].browser_download_url`) сверена на другом публичном репозитории
  (`cli/cli`) с реальными релизами — фикстуры тестов `source.rs` списаны с
  этой подтверждённой формы, а не выдуманы.
- Полный self-update цикл (реальный `proxypilot.exe` скачивается,
  переименовывается, перезапускается) не выполнялся ни разу — прямой запрет
  задачи не трогать живой процесс на этой машине. Механика переименования
  доказана на фикстурах (`install.rs`, реальная файловая система, не живой
  exe); механика самого HTTPS-скачивания — нет.
- Собственно `signtool`/сертификат по-прежнему не существуют — согласно
  задаче 2, это состояние правильное и ожидаемое, не блокер этой задачи.

## Изменённые/новые файлы

- `crates/app/src/update/` (новый) — `mod.rs`, `version.rs`, `verify.rs`,
  `json.rs`, `source.rs`, `check.rs`, `install.rs`.
- `crates/app/Cargo.toml` — фичи `windows`: `Win32_Security_WinTrust`,
  `Win32_Networking_WinHttp`. `serde_json` НЕ добавлен (см. выше).
- `crates/app/src/main.rs` — `mod update`; `apply_pending_update` в начале
  `run()`; `spawn_update_check`; `SettingsDeps.update_status`;
  `message_loop` перестроен под `&SettingsDeps` (clippy).
- `crates/app/src/settings_page.rs` — раздел «Обновления»
  (`update_status_text`/`update_status_note`), поле `check_for_updates` в
  `config_from_form`, поле `update_status` в `SettingsState`.
- `crates/app/src/websrv.rs` — тестовый конструктор `SettingsState` учитывает
  новое поле.
- `crates/core/src/config.rs` — поле `check_for_updates` (по умолчанию
  `true`), 2 новых теста + правка round-trip теста.

## Ограничения / что не делалось

- Тег не создавался, релиз не публиковался — прямой запрет задачи.
- Самоподписанный сертификат не создавался — не запрещён явно этой задачей,
  но и не понадобился: реальные тесты подписи используют уже подписанные
  Microsoft-файлы системы (`kernel32.dll`), что оказалось и проще, и честнее
  как доказательство.
- `docs/setup.md`/инструкция получателю не трогались — это зона задачи 5
  плана 5, которая ещё не начата.
- `relaunch()` (`install.rs`) не вызывается ни одним тестом и не была
  вызвана вручную — реальный запуск породил бы второй экземпляр ProxyPilot
  рядом с уже работающей на этой машине копией (хард-лимит задачи).

## Статус

Готово в пределах того, что проверяется без сети и без сертификата. Пять
обязательных свойств выполнены и там, где возможно, доказаны прогоном
(таймаут проверки, отказ на неверной/отсутствующей подписи на реальных
файлах, механика переименования на реальной файловой системе, тумблер).
Сетевая часть (`GithubSource`) и полный цикл самообновления на живом
бинаре — не проверены эмпирически по прямому ограничению задачи, это не
скрыто.
