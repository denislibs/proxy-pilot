# Task 3 report: крейт winnet и опознание сетей через NLM

## Что реализовано

Новый крейт `proxypilot-winnet` в `win/crates/winnet`:

- `win/crates/winnet/Cargo.toml` — как в брифе: `windows` только под
  `cfg(windows)`, плюс `thiserror`/`tracing`.
- `win/crates/winnet/src/com.rs` — `ComGuard`: RAII-страж COM-апартамента
  (`CoInitializeEx` в `new()`, `CoUninitialize` в `Drop`).
- `win/crates/winnet/src/networks.rs` — `NetworkCategory`, `NetworkSnapshot`,
  `format_guid`, `list_connected()` через `INetworkListManager`/`IEnumNetworks`.
- `win/crates/winnet/src/lib.rs` — `WinNetError`, объявление модулей.
- `win/Cargo.toml` — добавлен `"crates/winnet"` в members и
  `windows = "0.58"` в workspace.dependencies.

Produces из интерфейса задачи присутствуют без изменения имён:
`NetworkSnapshot { id, name, connected, category, internet }`,
`NetworkCategory { Public, Private, Domain, Unknown }`,
`list_connected() -> Result<Vec<NetworkSnapshot>, WinNetError>`, `ComGuard`.

## Отклонения от кода в брифе (реальный API `windows` 0.58.0)

1. **`IEnumNetworks::Next`** — брифовский вызов `enumerator.Next(&mut item, &mut fetched)?`
   не компилируется. Реальная сигнатура в windows-rs 0.58.0:
   ```rust
   pub unsafe fn Next(&self, rgelt: &mut [Option<INetwork>], pceltfetched: Option<*mut u32>) -> windows_core::Result<()>
   ```
   Второй параметр — `Option<*mut u32>`, а не `&mut u32`. Исправлено на
   `enumerator.Next(&mut item, Some(&mut fetched))?`. Поведение (запрос по
   одному элементу, выход из цикла при `fetched == 0`) не изменилось.

2. **`IEnumNetworks` не является итератором** — я сначала попробовал
   `for net in enumerator` (по инерции от современных версий `windows-rs`,
   где для части энумераторов сгенерирован `Iterator`), но 0.58.0 такого не
   генерирует (`error[E0277]: IEnumNetworks is not an iterator`). Оставил
   ручной `loop` с `Next`, как и было в брифе по духу, только с
   поправленной сигнатурой.

3. **`CoInitializeEx`** — в этой версии сигнатура
   `pub unsafe fn CoInitializeEx(pvreserved: Option<*const c_void>, dwcoinit: COINIT) -> windows_core::HRESULT`,
   то есть возвращает `HRESULT` напрямую (не `windows_core::Result`), но у
   `HRESULT` есть метод `.ok() -> Result<()>` — брифовский `.ok()?` оказался
   рабочим без изменений. `CoUninitialize()` не возвращает ничего — тоже
   совпало с брифом.

4. **`GetCategory`/`GetNetworkId`/`GetName`/`IsConnected`/`IsConnectedToInternet`** —
   сигнатуры совпали с брифом дословно (`NLM_NETWORK_CATEGORY(pub i32)` с
   `.0`, `windows_core::GUID`, `windows_core::BSTR` с `.to_string()`,
   `VARIANT_BOOL` с `.as_bool()`). Изменений не потребовалось.

5. **`ComGuard` — добавлено сверх брифа.** Брифовский `pub struct ComGuard;`
   (unit-структура без полей) по умолчанию `Send + Sync`, потому что не
   содержит несендовых полей. Это делает стража небезопасным: значение
   можно создать в одном потоке, переслать в `std::thread::spawn` на другой
   и уронить (`CoUninitialize`) там — апартамент COM привязан к потоку
   создания, и это было бы нарушением инварианта, который сам же бриф
   формулирует («CoUninitialize обязан вызваться на том же потоке»). Я
   добавил `PhantomData<*mut ()>` полем, чтобы тип стал `!Send + !Sync` и
   компилятор физически не позволил передвинуть `ComGuard` между потоками.
   Имя типа и его публичный конструктор `ComGuard::new()` не изменились —
   только внутреннее поле. (В следующем раунде правок это поле осталось,
   но у структуры появилось второе поле `uninit` — см. ниже.)

## Версия `windows` и включённые фичи

- `windows = "0.58"` (workspace.dependencies), фактически зарезолвилось в
  `windows v0.58.0` — как и просил бриф (более новая `0.62.2` в индексе
  присутствовала, но требование `^0.58` её не допускает).
- Фичи у `proxypilot-winnet`: `Win32_Foundation`, `Win32_System_Com`,
  `Win32_System_Variant`, `Win32_Networking_NetworkListManager` — ровно как
  в брифе, других не добавлял.

## Что тестировалось и результат

- `cargo test -p proxypilot-winnet` — 3/3 теста прошли (см. GREEN ниже),
  включая смоук-тест на живой машине.
- `cargo test --all` (весь workspace `win/`) — 43 + 38 + 3 + 5 (doc-tests) =
  89 тестов, все `ok` (было 86 до этой задачи, +3 новых).
- `cargo clippy --all-targets -- -D warnings` — чисто, без предупреждений,
  без `#[allow]`.
- `cargo fmt --all --check` — чисто, без диффа.

(Числа в этом разделе — из первого раунда, до фикс-раунда ниже. Итоговые,
верные числа после фикс-раунда — 93 теста, см. секцию «Fix round» в конце
файла; расхождение 89 vs 43+38+3+5 было моей ошибкой подсчёта в
черновике первого прохода, признано и исправлено ниже вместе с полным
verbatim-выводом.)

## TDD Evidence

### RED

Тестовый модуль (`networks.rs`) был написан первым — с тестами, но без
`format_guid`/`NetworkCategory`/`list_connected`, и без `com.rs`/`lib.rs`
реализации (только `pub mod com; pub mod networks;` в `lib.rs`, файла
`com.rs` ещё не существовало). Команда и полный вывод:

```
$ cd win && cargo test -p proxypilot-winnet
    Updating crates.io index
     Locking 15 packages to latest compatible versions
      Adding windows v0.58.0 (available: v0.62.2)
      Adding windows-core v0.58.0
      Adding windows-implement v0.58.0
      Adding windows-interface v0.58.0
      Adding windows-result v0.2.0
      Adding windows-strings v0.1.0
      Adding windows-targets v0.52.6
      Adding windows_aarch64_gnullvm v0.52.6
      Adding windows_aarch64_msvc v0.52.6
      Adding windows_i686_gnu v0.52.6
      Adding windows_i686_gnullvm v0.52.6
      Adding windows_i686_msvc v0.52.6
      Adding windows_x86_64_gnu v0.52.6
      Adding windows_x86_64_gnullvm v0.52.6
      Adding windows_x86_64_msvc v0.52.6
 Downloading crates ...
  Downloaded windows-implement v0.58.0
  Downloaded windows-targets v0.52.6
  Downloaded windows-strings v0.1.0
  Downloaded windows-interface v0.58.0
  Downloaded windows-result v0.2.0
  Downloaded windows-core v0.58.0
  Downloaded windows_x86_64_msvc v0.52.6
  Downloaded windows v0.58.0
   Compiling windows_x86_64_msvc v0.52.6
   Compiling tracing-core v0.1.36
   Compiling syn v2.0.119
   Compiling windows-targets v0.52.6
   Compiling windows-result v0.2.0
   Compiling windows-strings v0.1.0
   Compiling windows-interface v0.58.0
   Compiling windows-implement v0.58.0
   Compiling tracing-attributes v0.1.31
   Compiling windows-core v0.58.0
   Compiling tracing v0.1.44
   Compiling windows v0.58.0
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
error[E0583]: file not found for module `com`
 --> crates\winnet\src\lib.rs:7:1
  |
7 | pub mod com;
  | ^^^^^^^^^^^^
  |
  = help: to create the module `com`, create file "crates\winnet\src\com.rs" or "crates\winnet\src\com\mod.rs"
  = note: if there is a `mod com` elsewhere in the crate already, import it with `use crate::...` instead

For more information about this error, try `rustc --explain E0583`.
error: could not compile `proxypilot-winnet` (lib) due to 1 previous error
warning: build failed, waiting for other jobs to finish...
```

Это подлинный вывод команды на этой машине (крейт и правда не существовал —
`com.rs` ещё не был создан, реализация в `networks.rs` отсутствовала). Не
реконструкция.

Промежуточно (после создания `com.rs`, но до починки сигнатуры `Next`) была
и вторая, отдельная RED-стадия — реальная ошибка типов:
```
error[E0277]: `IEnumNetworks` is not an iterator
  --> crates\winnet\src\networks.rs:75:20
   |
75 |         for net in enumerator {
   |                    ^^^^^^^^^^ `IEnumNetworks` is not an iterator
```
(зафиксирована выше в разделе «Отклонения»).

### GREEN

```
$ cd win && cargo test -p proxypilot-winnet
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.46s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-aa4c7a768a2badc3.exe)

running 3 tests
test networks::tests::category_maps_every_documented_value ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

   Doc-tests proxypilot_winnet

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Полный прогон рабочего пространства (`cargo test --all`) — итоговые числа:
`proxypilot-bridge` unit: 43 ok; `cli` integration: 2 ok; `proxypilot-core`:
38 ok; `proxypilot-winnet`: 3 ok; doc-tests: 0/0/0 — всё `ok`, ни одного
провала.

`cargo clippy --all-targets -- -D warnings`:
```
    Checking windows_x86_64_msvc v0.52.6
    Checking tracing v0.1.44
    Checking proxypilot-core v0.1.0 (...)
    Checking windows-targets v0.52.6
    Checking windows-result v0.2.0
    Checking tracing-subscriber v0.3.23
    Checking windows-strings v0.1.0
    Checking windows-core v0.58.0
    Checking windows v0.58.0
    Checking tracing-appender v0.2.5
    Checking proxypilot-bridge v0.1.0 (...)
    Checking proxypilot-winnet v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.89s
```
(без единого предупреждения).

`cargo fmt --all --check` — пустой вывод, код 0.

## Ручная проверка (Step 6)

Написал временный `examples/list_networks.rs` (вызывает `ComGuard::new()` и
`list_connected()`, печатает `{:?}` по каждой сети), запустил
`cargo run -p proxypilot-winnet --example list_networks`, получил реальный
вывод с этой машины:

```
NetworkSnapshot { id: "{75C7A91B-EED3-4D6A-8669-E0449B108463}", name: "KZTK-38455_5G 2", connected: true, category: Public, internet: true }
```

Единственная подключённая сейчас сеть на этой машине:
- **GUID**: `{75C7A91B-EED3-4D6A-8669-E0449B108463}`
- Имя: `KZTK-38455_5G 2`
- Категория: `Public` (это домашняя/провайдерская Wi-Fi сеть по имени, не
  офисная — на реальной офисной машине пользователь увидит свою сеть,
  скорее всего с категорией `Private`/`Domain`, и её GUID нужно будет
  записать в конфиг как «офис»)
- `internet: true`

После снятия показаний временный `examples/list_networks.rs` удалён — он
не входит в Produces и не упомянут в шаге 7 (коммит) брифа.

## Файлы

Изменены/созданы (все пути абсолютные):
- `C:\Users\User\Desktop\proxypilot\repo\win\Cargo.toml` (members + workspace.dependencies)
- `C:\Users\User\Desktop\proxypilot\repo\win\Cargo.lock` (лок-файл, зафиксирован в коммите)
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet\Cargo.toml` (новый)
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet\src\lib.rs` (новый)
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet\src\com.rs` (новый)
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet\src\networks.rs` (новый)

Коммит: `3e92968` — `feat(win): крейт winnet и опознание сетей через NLM`.

## Самопроверка

- Все пункты брифа реализованы; имена из Produces не менялись.
- Каждый `unsafe`-блок (`CoInitializeEx`, `CoUninitialize`,
  `CoCreateInstance`+перечисление сетей) несёт `// SAFETY:`, объясняющий
  инвариант (тот же поток, парность вызова, кто освобождает интерфейсы).
- `ComGuard` **не** является `Send`/`Sync` (за счёт `PhantomData<*mut ()>`),
  значит `CoUninitialize` физически не может быть вызван на другом потоке —
  это было отклонением от буквального текста брифа (unit-структура), но
  необходимым для реальной корректности инварианта, который сам бриф
  описывает словами. Без этого поля `ComGuard` компилировался бы и работал
  бы правильно в однопоточном сценарии, но был бы небезопасен при
  случайной передаче между потоками — компилятор такую ошибку не поймал
  бы, что противоречит требованию задачи «is ComGuard sound».
- COM-интерфейсы (`INetworkListManager`, `IEnumNetworks`, `INetwork`)
  освобождаются автоматически через `Drop`, сгенерированный `windows-rs`
  (`Interface` реализует `Drop`, вызывающий `Release`) — вручную `Release`
  нигде не нужен, утечки нет; после `list_connected()` все интерфейсы
  выходят из области видимости и освобождаются до возврата функции (кроме
  того, что уже скопировано наружу как plain-данные в `NetworkSnapshot`).
- Форматирование GUID проверено на значении с ведущими нулями по
  фактическому тесту (`0x1234_5678_90ab_cdef_1234_5678_90ab_cdef` даёт
  ровно 38 символов, регистр верхний, скобки на месте) — формат
  `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` реализован через `{:08X}`/`{:04X}`/`{:02X}`
  спецификаторы, которые сами добьют ведущие нули, так что для GUID вида
  `00000000-...` вывод останется корректным (проверено логически по формату
  `format!`, не отдельным юнит-тестом на нулевой GUID — можно было бы
  добавить, но бриф такого теста не требовал). *(В фикс-раунде такой тест
  добавлен — ревью справедливо указало, что исходный тест не мог поймать
  свою же регрессию.)*
- Смоук-тест `listing_connected_networks_does_not_fail_on_a_real_machine`
  честен: если бы сетей не было, `for n in &nets` просто не выполнил бы
  тело цикла, и тест прошёл бы вакуумно — это ровно то поведение, которое
  описано в комментарии брифа («список может быть пустым — это не ошибка»).
  На этой машине сеть есть, так что assert'ы внутри цикла реально
  выполнились (`connected == true`, `id` не пуст) — тест не вакуумный на
  практике здесь, но остался бы валидным и без сети.
- Вывод тестов чист (никаких лишних `println!`/`dbg!`/`eprintln!` в самом
  тестовом коде — вывод, который печатал я, шёл только из временного
  примера, удалённого перед коммитом).
- `git diff --stat` перед коммитом содержал только ожидаемые файлы
  (`win/Cargo.toml`, `win/Cargo.lock`, `win/crates/winnet/**`) — никаких
  случайных файлов из временного `examples/` не попало в коммит.

## Проблемы и замечания (первый раунд)

- **Небольшая архитектурная нестыковка, унаследованная из брифа (не
  правил, так как не влияет на реальную платформу):** `lib.rs` объявляет
  `pub mod com; pub mod networks;` безусловно, хотя зависимость `windows`
  подключена только под `[target.'cfg(windows)'.dependencies]`. На
  не-Windows машине (`cargo check` без `--target`) сборка `proxypilot-winnet`
  не пройдёт, потому что `use windows::...` не найдёт крейт. Это не
  проблема для CI: `.github/workflows/win.yml` гоняет весь `win/`
  workspace только на `runs-on: windows-latest`, а других workflow для
  `win/` нет. **Ревью подтвердило это как реальный дефект (Finding 2) и
  потребовало убрать гейт — исправлено в фикс-раунде ниже.**
- UAC/повышение прав не потребовалось — чтение NLM работает от обычного
  пользователя, что и проверено на этой машине (тесты и пример запускались
  без elevation).
- Категория текущей сети на машине разработчика — `Public`, что ожидаемо
  для домашней сети; для реальной настройки офиса пользователю нужно будет
  подставить GUID своей корпоративной сети, увидев его через будущий CLI
  или `Get-NetConnectionProfile`.

---

## Fix round (post-review): ComGuard/MTA, честный Cargo.toml, крепче тесты GUID

Ревью прочитало вендоренные исходники `windows-0.58.0`, скомпилировало
отдельные проверки и прошло по форматированию GUID на нулевых полях.
Оно подтвердило оба API-адаптации первого раунда как верные, признало
`unsafe` корректным, а `PhantomData`-хардненинг — настоящим фиксом
безопасности. Затем оно указало на два Important-дефекта самого плана (не
реализации) и два дешёвых Minor. Все четыре исправлены здесь, коммит
`de9594e`.

### Finding 1 (Important) — `ComGuard` превращал рабочий MTA-поток в жёсткий отказ

`ComGuard::new()` безусловно вызывал `.ok()?` на результате
`CoInitializeEx`. Если вызывающий поток уже был переведён в MTA каким-то
другим компонентом — GUI-тулкитом, рантаймом-хостом, то есть именно тем,
чем и является интеграция с треем, — `CoInitializeEx(COINIT_APARTMENTTHREADED)`
возвращает `RPC_E_CHANGED_MODE`, а `.ok()` трактует это как ошибку. Это
неверно: NLM прекрасно вызывается и из MTA — модель апартамента — это
требование именно этого стража (из-за выбранного флага
`COINIT_APARTMENTTHREADED`), а не требование самого API NLM. Старый код
превращал не связанное с этим крейтом архитектурное решение хоста в
молчаливый, постоянный «ошибка Windows: …» для единственной вещи, ради
которой этот крейт существует.

**Фикс** (`win/crates/winnet/src/com.rs`): `ComGuard` теперь несёт поле
`uninit: bool`, которое запоминает, сам ли он выполнил инициализацию:

- `hr == RPC_E_CHANGED_MODE` → `Ok(Self { uninit: false, .. })` — поток
  пригоден, апартамент чужой, снимать нечего.
- `hr.is_ok()` (`S_OK`/`S_FALSE`) → `Ok(Self { uninit: true, .. })` — наша
  инициализация, `Drop` обязан вызвать `CoUninitialize`.
- Любой другой неуспех → `Err(WinNetError::Windows(..))` через `hr.ok()?`.
- `Drop` вызывает `CoUninitialize` только `if self.uninit`.

`PhantomData<*mut ()>`-отметка `!Send`/`!Sync` из прошлого раунда оставлена
без изменений — привязка к потоку важна независимо от того, кому
принадлежит апартамент. Комментарии `SAFETY` на `new()` и `Drop`
переписаны так, чтобы точно описывать инвариант по каждой ветке, а не
прежнее общее (и теперь неточное) утверждение.

Добавлены три теста в `com.rs` (все проходят на этой машине):
- `a_guard_created_on_a_bare_thread_owns_its_uninit` — обычный случай,
  `uninit == true`.
- `a_second_guard_on_the_same_thread_still_owns_its_uninit` — повторный
  вход даёт `S_FALSE`, это тоже «наша» инициализация, и `Drop` обязан её
  снять.
- `a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit` — ровно
  сценарий регрессии: тест поднимает отдельный OS-поток (чтобы не задеть
  состояние COM других тестов, которые может исполнять тот же пул потоков
  test-harness), сам вызывает `CoInitializeEx(COINIT_MULTITHREADED)`,
  имитируя хост, который вошёл в MTA раньше нас, затем создаёт
  `ComGuard::new()` и проверяет `!g.uninit` и то, что вернулся `Ok`
  (а не `Err`) — именно этот тест провалился бы на коде до фикса (тот
  вернул бы `Err`, а не `Ok(uninit:false)`).

### Finding 2 (Important) — гейт `cfg(windows)` на зависимость был мёртвым и вводил в заблуждение

`Cargo.toml` держал `windows` под `[target.'cfg(windows)'.dependencies]`,
намекая на то, что крейт умеет мягко деградировать вне Windows. На деле
нет: `lib.rs` безусловно объявляет `pub mod com; pub mod networks;`, а сам
`lib.rs` ссылается на `windows::core::Error` в `WinNetError` — сборка вне
Windows падает на `E0433: unresolved crate 'windows'` ещё до того, как
дело доходит до `com` или `networks`. Единственная задача, трогающая
`win/` (`.github/workflows/win.yml`), гоняется только на `windows-latest`,
так что историю про «работает и без Windows», которую обещал гейт, никто
никогда не проверял.

**Фикс**: `windows` перенесён в обычную таблицу `[dependencies]` рядом с
`thiserror`/`tracing`, с комментарием, прямо говорящим, что крейт требует
Windows безусловно. По явному указанию ревью НЕ пошёл другим путём
(`#![cfg(windows)]` на корне крейта + гейтирование варианта ошибки) —
в этом плане нигде не нужен крейт, который на не-Windows компилируется в
пустое место.

### Finding 3 (Minor) — тест канонического GUID не мог поймать свою же регрессию

`guid_is_formatted_in_the_canonical_braced_form` использует
`0x1234_5678_90ab_cdef_1234_5678_90ab_cdef` — в нём нет ни одного нулевого
полубайта, так что `len() == 38` и проверка верхнего регистра прошли бы,
даже удали кто-то все спецификаторы ширины (`{:08X}`/`{:04X}`/`{:02X}`) из
`format_guid`. Добавлен `guid_with_leading_zeros_keeps_fixed_field_widths`
на значении `0x0000_000B_000C_00D0_0001_0000_0000_0A00`, сверяющий точную
строку `"{0000000B-000C-00D0-0001-000000000A00}"` (вычислено вручную по
логике разбиения полей `GUID::from_u128` в `windows-core-0.58.0/src/guid.rs`,
затем подтверждено прошедшим тестом).

### Finding 4 (Minor) — смоук-тест проверял только непустоту `id`

`listing_connected_networks_does_not_fail_on_a_real_machine` теперь
проверяет `n.id.starts_with('{')` и `n.id.len() == 38` для каждой сети,
полученной от живого вызова NLM, — привязывая единственный тест, который
видит GUID, произведённый самой Windows, к канонической форме, от которой
зависят и конфиг-файл, и сверка с `Get-NetConnectionProfile`.

### Проверка — verbatim-вывод (реальная машина, эта сессия)

`cargo fmt --all --check` — код выхода 0, вывода нет.

`cargo clippy --all-targets -- -D warnings`:
```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.32s
CLIPPY_EXIT=0
```

`cargo test --all` (полный, verbatim, этот прогон):
```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.71s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-f9bfedea04baa417.exe)

running 46 tests
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::parses_connect ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::truncated_input_is_an_error ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok
test log::tests::log_file_name_is_stable ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::is_shareable_across_threads ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... ok
test socks5::tests::surfaces_refusal_code ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test connector::tests::refused_upstream_reports_error ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok

test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-837393c89186d591.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-eb7488564f5ac25b.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-ce820a0b07ec9f56.exe)

running 38 tests
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test config::tests::defaults_match_the_spec ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::validate_accepts_the_defaults ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test config::tests::load_from_a_missing_file_yields_defaults ... ok
test config::tests::validate_rejects_a_malformed_upstream ... ok
test config::tests::validate_rejects_a_port_below_the_privileged_range ... ok
test config::tests::validate_rejects_a_zero_connection_limit ... ok
test config::tests::validate_rejects_an_absurd_connection_limit ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test config::tests::upstream_format_is_validated ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ... ok
test config::tests::save_then_load_roundtrips_through_a_real_file ... ok
test config::tests::config_path_matches_what_the_spec_promises ... ok

test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-1325b8c59218fb85.exe)

running 7 tests
test networks::tests::category_maps_every_documented_value ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

   Doc-tests proxypilot_bridge

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_winnet

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

TEST_EXIT=0
```

Итоговые цифры этого прогона: `proxypilot-bridge` unit 46 + `main` 0 +
`cli` integration 2 + `proxypilot-core` 38 + `proxypilot-winnet` 7 =
**93 теста, 0 провалов**, плюс 3 набора doc-тестов по 0/0/0. (Расхождение
«89» vs «86» в первом черновике отчёта было моей реальной ошибкой подсчёта
— вот настоящее число, полученное одной командой за один прогон в одной
терминальной сессии, вставлено целиком.)

### Отложено (по явному указанию координатора, не в этом раунде)

- Сузить `unsafe`-блок в `list_connected`, чтобы он охватывал только
  FFI-вызовы, а не окружающие `Vec::new`/`push`.
- Неиспользуемая зависимость `tracing` (объявлена, но код крейта её пока
  нигде не использует).
- На будущих задачах начинать захват RED-состояния с пустой заглушки
  модуля, чтобы падение доходило до ошибок типов, а не останавливалось на
  `E0583 file not found`.

### Коммит фикс-раунда

`de9594e` — `fix(win): winnet — не ронять COM на MTA-потоке, честный
Cargo.toml, крепче тесты GUID`.

### Файлы, изменённые в этом раунде

- `C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet\Cargo.toml`
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet\src\com.rs`
- `C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet\src\networks.rs`
