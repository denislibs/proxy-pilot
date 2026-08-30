# Task 1 report — Поиск установленного OpenVPN

Plan: 4. Branch: `feat/vpn-static-ip`. Base HEAD: `c2fae41`.

## Что сделано

- Новый модуль `crates/winnet/src/openvpn.rs`: `Installation { gui_exe, config_dir }` и
  `find_installation() -> Result<Option<Installation>, WinNetError>`.
- Зарегистрирован в `crates/winnet/src/lib.rs` по алфавиту, между `networks` и `sysproxy` (правка,
  не переписывание — остальной файл не тронут).
- `sysproxy::RegKey::open` расширен параметром `root: HKEY` (был жёстко привязан к
  `HKEY_CURRENT_USER`). Все существующие вызовы в `sysproxy.rs` и `autostart.rs` обновлены явной
  передачей `HKEY_CURRENT_USER` — поведение не изменилось, изменилась только сигнатура. Второй
  «сырой» путь к `HKEY` заводить не пришлось: `openvpn.rs` целиком проходит через `RegKey`, в нём
  нет ни одного `unsafe`-блока.

## Как работает поиск

1. `open_key(SUBKEY)` открывает `HKLM\SOFTWARE\OpenVPN` на чтение (`KEY_READ`, без записи).
   Если ключа нет — `RegOpenKeyExW` возвращает `ERROR_FILE_NOT_FOUND`; это распознаётся явно и
   превращается в `Ok(None)`, а не в `Err`.
2. Если ключ есть, читаются строковые значения `bin_dir` и `config_dir` (реальные имена значений
   инсталлятора OpenVPN, подтверждено на этой машине через `reg query`). Пустая строка (значения
   нет даже внутри существующего ключа) трактуется как отсутствие данных.
3. `locate()` — чистая функция без обращения к реестру: для каждого из двух путей по отдельности,
   если из реестра пришла пустая строка, подставляется стандартный путь
   (`%ProgramFiles%\OpenVPN\bin` / `...\config`); если пришло значение — используется оно как есть,
   даже если оно отличается от стандартного расположения (реестр может указывать на нестандартную
   установку).
4. Финальная проверка: `gui_exe = bin_dir.join("openvpn-gui.exe")` обязан существовать как файл
   (`is_file()`). Если нет — `Ok(None)`, независимо от того, что говорил реестр. Для `config_dir`
   такая проверка не делается: отсутствующий каталог конфигураций — это «конфигураций пока нет», а
   не «OpenVPN не установлен», и относится к области Task 4 (перечисление `.ovpn`).

## Почему `RegKey` не пришлось дублировать

Бриф просил переиспользовать `sysproxy::RegKey`, а не заводить второй «сырой» путь к `HKEY`.
Единственное, чего ей не хватало — открытие только под `HKEY_CURRENT_USER` (жёстко зашито в
`open()`). Добавлен параметр `root: HKEY`; SAFETY-комментарий у `RegOpenKeyExW` обновлён
(«один из предопределённых корней реестра — `HKEY_CURRENT_USER` или `HKEY_LOCAL_MACHINE` — оба
всегда валидны»). Других изменений `RegKey` не потребовалось: `query_string` уже отдаёт пустую
строку для отсутствующего значения, что как раз нужный сигнал «использовать стандартный путь».

## TDD evidence

### RED — до реализации (реальные ошибки типов, не «файл не найден»)

Команда: `cargo test -p proxypilot-winnet` при пустой реализации (в `openvpn.rs` — только тесты,
никаких `Installation`/`find_installation`/`locate`/`open_key`).

```
   Compiling windows-sys v0.61.2
   Compiling windows v0.58.0
   Compiling socket2 v0.6.5
   Compiling mio v1.2.2
   Compiling nu-ansi-term v0.50.3
   Compiling tracing-subscriber v0.3.23
   Compiling tokio v1.53.1
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
warning: unused import: `super::*`
  --> crates\winnet\src\openvpn.rs:10:9
   |
10 |     use super::*;
   |         ^^^^^^^^
   |
   = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0425]: cannot find function `locate` in this scope
  --> crates\winnet\src\openvpn.rs:20:21
   |
20 |         let found = locate(
   |                     ^^^^^^ not found in this scope

error[E0425]: cannot find function `locate` in this scope
  --> crates\winnet\src\openvpn.rs:39:21
   |
39 |         let found = locate(&bin_dir.display().to_string(), "", Path::new(r"C:\unused"));
   |                     ^^^^^^ not found in this scope

error[E0425]: cannot find function `locate` in this scope
  --> crates\winnet\src\openvpn.rs:48:21
   |
48 |         let found = locate(&bin_dir.display().to_string(), "", Path::new(r"C:\unused"));
   |                     ^^^^^^ not found in this scope

error[E0425]: cannot find function `locate` in this scope
  --> crates\winnet\src\openvpn.rs:59:21
   |
59 |         let found = locate("", "", &program_files);
   |                     ^^^^^^ not found in this scope

error[E0425]: cannot find function `locate` in this scope
  --> crates\winnet\src\openvpn.rs:77:21
   |
77 |         let found = locate(&bin_dir.display().to_string(), "", &program_files);
   |                     ^^^^^^ not found in this scope

error[E0425]: cannot find function `open_key` in this scope
  --> crates\winnet\src\openvpn.rs:93:22
   |
93 |         let result = open_key(missing).expect("отсутствие ключа — не ошибка");
   |                      ^^^^^^^^ not found in this scope

error[E0425]: cannot find function `find_installation` in this scope
   --> crates\winnet\src\openvpn.rs:102:21
    |
102 |         let found = find_installation().expect("поиск обязан не падать в любом случае");
    |                     ^^^^^^^^^^^^^^^^^ not found in this scope

For more information about this error, try `rustc --explain E0425`.
warning: `proxypilot-winnet` (lib test) generated 1 warning
error: could not compile `proxypilot-winnet` (lib test) due to 7 previous errors; 1 warning emitted
```

### GREEN — после реализации

```
running 67 tests
...
test openvpn::tests::finding_the_real_installation_does_not_fail ... ok
test openvpn::tests::open_key_is_none_for_a_subkey_that_does_not_exist ... ok
test openvpn::tests::locate_returns_none_when_the_registry_bin_dir_does_not_exist_on_disk ... ok
test openvpn::tests::locate_returns_none_when_the_registry_bin_dir_has_no_gui_exe ... ok
test openvpn::tests::locate_finds_installation_when_registry_bin_dir_has_the_gui_exe ... ok
test openvpn::tests::locate_falls_back_to_the_standard_config_dir_when_the_registry_value_is_empty ... ok
test openvpn::tests::locate_falls_back_to_the_standard_bin_dir_when_the_registry_value_is_empty ... ok
...
test result: ok. 65 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s
```

7 новых тестов в `openvpn.rs`, все зелёные; 2 ignored — те же самые, что были раньше
(`autostart::...on_the_real_registry`, `events::watch_a_real_network_change`), не новые.

## Три команды CI (полный прогон после реализации)

### `cargo test --all`

Итог по крейтам:

```
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.61s   (proxypilot-app)
test result: ok. 69 passed; 0 failed; 0 measured; 0 filtered out; finished in 2.06s                (proxypilot-bridge lib)
test result: ok. 0 passed; ...                                                                      (proxypilot-bridge bin)
test result: ok. 2 passed; 0 failed; ...                                                            (cli integration)
test result: ok. 48 passed; 0 failed; ...                                                           (proxypilot-core)
test result: ok. 65 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s      (proxypilot-winnet)
```

Всего 289 passed, 3 ignored (было 282 passed + 3 ignored до этой задачи — прирост ровно на 7 новых
тестов `openvpn`, число ignored не изменилось).

### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.36s
```

Ноль предупреждений. `#[allow(...)]` не использован нигде.

### `cargo fmt --all --check`

Первый прогон нашёл расхождения форматирования (длинные строки/фигурные скобки) в `openvpn.rs` и
двух местах `autostart.rs`, где сигнатура `RegKey::open` выросла на аргумент. Применён
`cargo fmt --all`; повторный `--check` — чисто (exit code 0), без diff.

## Смоук на живой машине

OpenVPN на этой машине **установлен**, что подтверждено дважды: напрямую через `reg query
HKLM\SOFTWARE\OpenVPN` (см. ниже) и через сам код в тесте
`openvpn::tests::finding_the_real_installation_does_not_fail`, запущенном с `--nocapture`:

```
running 1 test
OpenVPN найден: gui_exe="C:\\Program Files\\OpenVPN\\bin\\openvpn-gui.exe" config_dir="C:\\Program Files\\OpenVPN\\config\\"
test openvpn::tests::finding_the_real_installation_does_not_fail ... ok
```

Значения `gui_exe`/`config_dir` совпадают с тем, что реально лежит в реестре
(`bin_dir=C:\Program Files\OpenVPN\bin\`, `config_dir=C:\Program Files\OpenVPN\config\` — прочитано
только на чтение, никаких записей в `HKLM` не производилось) и с тем, что реально есть на диске
(`openvpn-gui.exe` присутствует в `bin`, каталог `config` не пуст). Значения взяты из самого
реестра, а не из запасного `%ProgramFiles%\OpenVPN` пути — то есть найдены именно тем путём,
который предпочтителен по брифу.

Не выполнялось на машине ни разу за всю задачу: `openvpn-gui.exe --command connect`, запись в
`HKLM` в любой форме, изменение конфигурации OpenVPN — только чтение существующих значений
`HKLM\SOFTWARE\OpenVPN` штатным `RegOpenKeyExW`/`RegQueryValueExW` с `KEY_READ`.

## Самопроверка (пункты из инструкции)

- Машина без OpenVPN → `Ok(None)`, не ошибка и не паника: да — покрыто на уровне `open_key`
  (тест `open_key_is_none_for_a_subkey_that_does_not_exist`, гоняется на заведомо несуществующем
  имени подключа, не трогая реальный `OpenVPN`) и на уровне `locate` (пустые строки → стандартный
  путь → файла там нет → `None`).
- Устаревший ключ реестра, указывающий на удалённую установку → тоже `Ok(None)`: да — два теста
  `locate_returns_none_when_the_registry_bin_dir_has_no_gui_exe` (каталог существует, exe нет) и
  `locate_returns_none_when_the_registry_bin_dir_does_not_exist_on_disk` (каталога вовсе нет).
- Ключ реестра закрывается на каждом пути, включая ошибочные: да, без исключений — открытие идёt
  только через `RegKey::open`, чей `Drop` закрывает хендл всегда; `open_key` либо не создаёт хендл
  вовсе (ветка `None`/`Err`), либо создаёт его через тот же `RegKey`, что и остальной крейт.
- Запасные пути корректны при нестандартном `%ProgramFiles%`: да — `program_files_dir()` читает
  переменную окружения динамически, `standard_bin_dir`/`standard_config_dir` строятся от неё, а не
  от захардкоженного `C:\Program Files` (тот — лишь fallback на случай отсутствия самой
  переменной, что на Windows практически не бывает).

## Затронутые файлы

- `crates/winnet/src/openvpn.rs` — новый модуль (весь код задачи).
- `crates/winnet/src/lib.rs` — добавлена строка `pub mod openvpn;` (по алфавиту).
- `crates/winnet/src/sysproxy.rs` — `RegKey::open` получил параметр `root: HKEY`; обновлены оба
  вызова внутри файла и докблоки (`RegKey`, `open`).
- `crates/winnet/src/autostart.rs` — семь вызовов `RegKey::open` обновлены явной передачей
  `HKEY_CURRENT_USER` (пять в продакшн-коде: `is_enabled_at`, `enable_at`, `disable_at`,
  `raw_value_at`, `restore_raw_value_at`; и два в тестовом коде: одна правка в
  `restore_raw_value_preserves_the_original_reg_expand_sz_type`, вторая — там же чуть ниже);
  добавлен импорт `HKEY_CURRENT_USER`; обновлён докблок модуля. **Исправление:** первая версия
  этого отчёта (см. ниже) утверждала «пять вызовов» и не упоминала, что среди них — тестовый код;
  это было неточно, см. Fix round 1, пункт 5.

Ни один из этих файлов не переписан — только точечные правки существующих строк и добавление
нового модуля, как и требовал рулинг T4 (T1 закладывает `openvpn.rs`, не мешая будущей дописке).

---

## Fix round 1 (после ревью, поверх `4d91ea5`)

Ревьюер проверил рискованную часть руками, а не поверил на слово: все 10 вызовов `RegKey::open`
сверены `grep`, каждый прежний по-прежнему передаёт `HKEY_CURRENT_USER`, единственный хендл HKLM —
`KEY_READ` и никогда не доходит до писателя, `apply()` по-прежнему пишет `ProxyServer` →
`ProxyOverride` → `ProxyEnable` последним, тестовый модуль `sysproxy` не тронут; смоук-значения
перепроверены независимо через `reg query`, и пробным компилятом показано, что конечный разделитель
в `config_dir` склеивается с `.join()` штатно. Обобщение обёртки вместо второго пути к `HKEY`
признано верным решением. Ниже — пять находок и что с каждой сделано.

### 1. `CLAUDE.md` — устаревшее «Реестр: только HKCU» (ошибка автора брифа, не реализатора)

Строка `Реестр: только HKCU. Записей в HKLM нет нигде.` была написана до появления этой задачи;
первое предложение стало ложным в момент, когда `find_installation` начал читать `HKLM`. Правило
само называет разошедшийся с кодом комментарий дефектом («он врёт увереннее, чем код»), так что
исправлено немедленно:

```diff
-Реестр: только `HKCU`. Записей в `HKLM` нет нигде.
+Реестр на запись: только `HKCU`. `HKLM` — только на чтение (`openvpn.rs`),
+записей там нет нигде: запись в `HKLM` и есть тот самый второй UAC, которого
+это правило не допускает.
```

### 2. Непроверенная склейка в `find_installation` (`openvpn.rs:54-65` по старой нумерации)

Все семь тестов первой версии проверяли либо `locate` (чистая функция, аргументы уже расставлены
верно вызывающим), либо `open_key` (несуществующий подключ) — ни один не проверял, что `BIN_DIR`
читается в `bin_dir_value`, а `CONFIG_DIR` — в `config_dir_value`, а не наоборот. Случайная
перестановка этих двух констант в `find_installation` не уронила бы ни одного теста и на этой
машине прошла бы даже живой смоук — он допускает оба исхода (`Some`/`None`), а при перестановке
`bin_dir` указывал бы на каталог конфигураций, `openvpn-gui.exe` там нет, и смоук просто получил бы
`None` вместо `Some`, не подняв тревоги.

Исправлено выносом чтения в отдельную функцию `read_registry_values(key: &RegKey) -> Result<(String,
String), WinNetError>` (`openvpn.rs`, сразу после `find_installation`) и новым тестом
`find_installation_reads_bin_dir_and_config_dir_into_the_right_slots`. Тест не может писать в
`HKLM` (запрещено конституцией задачи), поэтому пишет в одноразовый подключ **HKCU** — тем же
приёмом, что и `autostart::tests::TestSubkeyGuard` (локальная копия `ScratchKey` в
`openvpn::tests`, тестовая же собственность, продакшн её не видит): двумя строго различающимися
маркерными строками под именами `BIN_DIR`/`CONFIG_DIR`, читает их через ту же
`read_registry_values` и проверяет, что каждая осталась в своей роли. Тест зелёный:

```
test openvpn::tests::find_installation_reads_bin_dir_and_config_dir_into_the_right_slots ... ok
```

Реестровое подтверждение из брифа («ключ есть, файла нет → `Ok(None)`») ревьюер подтвердил уже
покрытым на уровне `locate` (`locate_returns_none_when_the_registry_bin_dir_has_no_gui_exe` и
`locate_returns_none_when_the_registry_bin_dir_does_not_exist_on_disk`) — эта часть не менялась.

### 3. 32-битное представление реестра — задокументировано, не исправлено (по рулингу ревьюера)

`open_key` наследует битность процесса и не просит `KEY_WOW64_32KEY`/`_64KEY` явно. 32-битный
OpenVPN на 64-битной Windows регистрируется под `HKLM\SOFTWARE\WOW6432Node\OpenVPN` и ставится в
`Program Files (x86)` — ни чтение реестра, ни запасной `%ProgramFiles%`-путь его не найдут.
Ревьюер прямо распорядился код не трогать: отказ здесь безопасен (честное «не установлен» вместо
порчи чего-либо), OpenVPN 2.6+ только 64-битный, и свидетельств 32-битной установки ни у кого из
адресатов нет — спекулятивный код без единого прогона был бы обузой, а не страховкой. Добавлен
только докблок-комментарий в `openvpn.rs` (после абзаца про `RegKey`), называющий ограничение,
последствие и то, чем это чинится, если реальный отчёт когда-нибудь придёт (`RegKey::open` с
`KEY_READ | KEY_WOW64_32KEY` — параметр `access` это уже умеет, добавлять нечего).

### 4. Лог называл не ту подсистему (`sysproxy.rs:185`)

`query_string` (общий метод `RegKey`, которым теперь пользуется и `openvpn`) предупреждал
`"значение прокси в реестре не строкового типа, считаем пустым"`. Слово «прокси» отправило бы
читателя искать баг не там, если предупреждение придёт из чтения `bin_dir`/`config_dir` OpenVPN, а
не `sysproxy`. Убрано:

```diff
-"значение прокси в реестре не строкового типа, считаем пустым"
+"значение в реестре не строкового типа, считаем пустым"
```

### 5. Неточность в самом отчёте

Первая версия этого файла утверждала «пять вызовов `RegKey::open` обновлены» в `autostart.rs` и не
упоминала, что среди них есть тестовый код. На деле их семь: пять в продакшн-функциях
(`is_enabled_at`, `enable_at`, `disable_at`, `raw_value_at`, `restore_raw_value_at`) и два — в
`restore_raw_value_preserves_the_original_reg_expand_sz_type` и следом за ней. Правка в тестах
чисто механическая (добавлен аргумент плюс перенос строки от rustfmt), утверждения тестов не
изменили смысла — но заявление «тестов не касался» было неточным и не проговорённым, а не просто
неполным. Раздел «Затронутые файлы» выше исправлен на месте с explicit-пометкой; отчёт также
дополнен этим разделом, а не переписан.

Заодно снят избыточный `#[cfg(windows)]` перед `finding_the_real_installation_does_not_fail`
(`openvpn.rs`, было ближе к концу файла) — весь крейт `proxypilot-winnet` не собирается вне Windows
(см. комментарий в его `Cargo.toml`), поэтому условие было всегда истинным и ничего не решало.

### Три команды CI — полный повторный прогон после всех пяти правок

#### `cargo test --all`

Итог по крейтам:

```
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.61s   (proxypilot-app)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s     (proxypilot-bridge lib)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s      (proxypilot-bridge bin)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s      (cli integration)
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s     (proxypilot-core)
test result: ok. 66 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s     (proxypilot-winnet)
```

Восемь тестов в `openvpn::tests`, все зелёные (был семь, добавлен
`find_installation_reads_bin_dir_and_config_dir_into_the_right_slots`):

```
test openvpn::tests::find_installation_reads_bin_dir_and_config_dir_into_the_right_slots ... ok
test openvpn::tests::finding_the_real_installation_does_not_fail ... ok
test openvpn::tests::locate_returns_none_when_the_registry_bin_dir_does_not_exist_on_disk ... ok
test openvpn::tests::open_key_is_none_for_a_subkey_that_does_not_exist ... ok
test openvpn::tests::locate_returns_none_when_the_registry_bin_dir_has_no_gui_exe ... ok
test openvpn::tests::locate_finds_installation_when_registry_bin_dir_has_the_gui_exe ... ok
test openvpn::tests::locate_falls_back_to_the_standard_config_dir_when_the_registry_value_is_empty ... ok
test openvpn::tests::locate_falls_back_to_the_standard_bin_dir_when_the_registry_value_is_empty ... ok
```

Итого 290 passed, 3 ignored (было 289 passed + 3 ignored после первой версии задачи — прирост
ровно на 1 новый тест; ignored не изменились: те же `autostart::...on_the_real_registry` и
`events::watch_a_real_network_change`).

#### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.59s
```

Ноль предупреждений. `#[allow(...)]` по-прежнему не использован нигде.

#### `cargo fmt --all --check`

Первый прогон после правок нашёл расхождение (новый код в `openvpn.rs` требовал перестроения по
ширине строки) — применён `cargo fmt --all`; повторный `--check` — чисто, exit code 0.

### Смоук после правок

Повторный запуск `finding_the_real_installation_does_not_fail` с `--nocapture` после всех пяти
правок — значения те же, что и до ревью, никаких записей в `HKLM` не производилось:

```
running 1 test
OpenVPN найден: gui_exe="C:\\Program Files\\OpenVPN\\bin\\openvpn-gui.exe" config_dir="C:\\Program Files\\OpenVPN\\config\\"
test openvpn::tests::finding_the_real_installation_does_not_fail ... ok
```

### Затронутые файлы (fix round 1)

- `CLAUDE.md` — пункт 1.
- `crates/winnet/src/openvpn.rs` — пункты 2, 3, 5 (снятие `#[cfg(windows)]`).
- `crates/winnet/src/sysproxy.rs` — пункт 4.
- `docs/process/win-vpn-static-ip/task-1-report.md` — этот раздел плюс точечное исправление в
  «Затронутые файлы» исходного отчёта (пункт 5).

`crates/winnet/src/autostart.rs` в этом раунде не менялся — пункт 5 был неточностью отчёта, не
кода.

За весь раунд на машине не выполнялось: `openvpn-gui.exe --command connect`, подключение или
отключение туннеля, изменение любого профиля OpenVPN, запись в `HKLM` в любой форме. Единственная
запись в реестр за весь раунд — тестовый одноразовый подключ под `HKCU`
(`Software\ProxyPilotOpenvpnSelfTest-<pid>`), созданный и удалённый в пределах одного теста.
