# Task 3 — Чужой туннель — отчёт

Branch: `feat/vpn-static-ip`, base HEAD: `31ef54c`.

## Что сделано

`crates/winnet/src/tunnel_state.rs` (новый модуль, зарегистрирован
алфавитно в `crates/winnet/src/lib.rs` — файл только дополнен строкой
`pub mod tunnel_state;` после `pub mod sysproxy;`, ничего в нём не
переписано):

```rust
pub struct AdapterRoute { pub dest: Ipv4Net, pub interface_alias: String, pub is_tunnel: bool }
pub fn our_tunnel_up(adapters: &[AdapterRoute], our_alias: &str) -> bool
pub fn foreign_tunnel_up(routes: &[Ipv4Net], adapters: &[AdapterRoute], our_alias: &str) -> bool
```

`Ipv4Net`/`mask_of` берутся из `proxypilot_core::net` (task 2), вторая
копия не заводилась. Обе функции чистые: никакого ввода-вывода, таблицу
маршрутов и список адаптеров собирает вызывающий.

### Логика

- `our_tunnel_up` — среди адаптеров есть туннельный (`is_tunnel`) с нашим
  псевдонимом интерфейса. Маршруты не смотрит вовсе — здесь важен только
  факт существования нашего адаптера.
- `foreign_tunnel_up` — среди адаптеров есть туннельный, чей псевдоним
  **не** совпадает с нашим, и чей `dest` пересекается хотя бы с одной
  подсетью из `routes` (наши офисные сети).
- Пересечение (`overlaps`) считается по границам диапазона адресов
  (`[start, end]`, вычисленным через `mask_of` из `core`, а не
  дублирующей арифметикой): диапазоны пересекаются в любую сторону —
  один шире другого, один целиком внутри другого, или оба совпадают.
  **Стейтмент правила** (см. doc-comment над `overlaps` в коде): для двух
  настоящих (выровненных) CIDR-блоков третьего варианта — частичного,
  не вложенного пересечения — не существует; блоки либо вложены один в
  другой, либо не пересекаются вовсе. Формула по границам корректна и
  для этого случая, и для не выровненных значений, собранных мимо
  `Ipv4Net::from_str` (поля `Ipv4Net` публичны) — специального случая не
  потребовалось. Это и есть выбранное прочтение пункта приёмки про
  «шире/уже»: `/8`, покрывающий наш `/16`, считается несущим её; `/24`
  внутри нашего `/16` — тоже.

### Отклонение от сигнатуры плана

Бриф прямо предупреждал, что сигнатуры раскрыты не полностью, и просил
сообщить, если код покажет другую потребность в полях. У
`foreign_tunnel_up` в плане нет параметра `our_alias`. Без него функция
физически не может отличить наш собственный (уже поднятый) туннель от
чужого — а «наш туннель не считается чужим» прямое требование приёмки,
причём проверяемое как тест именно на этой функции (не на связке с
вызывающим кодом, который бы фильтровал список адаптеров сам). Добавлен
третий параметр `our_alias: &str`. Функция остаётся чистой; вызывающий
(задачи 4 и 7) и так знает `our_alias` — тот же аргумент, что уже идёт в
`our_tunnel_up`.

`AdapterRoute` использован ровно как задан в брифе, без изменений полей.

## TDD evidence

### RED (файл только с тестами против ещё не объявленного API)

Команда: `cargo test -p proxypilot-winnet tunnel_state`

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
  --> crates\winnet\src\tunnel_state.rs:19:25
   |
19 |         let adapters = [AdapterRoute {
   |                         ^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
  --> crates\winnet\src\tunnel_state.rs:30:25
   |
30 |         let adapters = [AdapterRoute {
   |                         ^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
  --> crates\winnet\src\tunnel_state.rs:41:25
   |
41 |         let adapters = [AdapterRoute {
   |                         ^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
  --> crates\winnet\src\tunnel_state.rs:52:25
   |
52 |         let adapters = [AdapterRoute {
   |                         ^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
  --> crates\winnet\src\tunnel_state.rs:68:25
   |
68 |         let adapters = [AdapterRoute {
   |                         ^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
  --> crates\winnet\src\tunnel_state.rs:79:25
   |
79 |         let adapters = [AdapterRoute {
   |                         ^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
  --> crates\winnet\src\tunnel_state.rs:90:25
   |
90 |         let adapters = [AdapterRoute {
   |                         ^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
   --> crates\winnet\src\tunnel_state.rs:101:25
    |
101 |         let adapters = [AdapterRoute {
    |                         ^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `AdapterRoute` in this scope
   --> crates\winnet\src\tunnel_state.rs:111:25
    |
111 |         let adapters = [AdapterRoute {
    |                         ^^^^^^^^^^^^ not found in this scope

warning: unused import: `super::*`
 --> crates\winnet\src\tunnel_state.rs:9:9
  |
9 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0425]: cannot find function `foreign_tunnel_up` in this scope
  --> crates\winnet\src\tunnel_state.rs:25:18
   |
25 |         assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
   |                  ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `foreign_tunnel_up` in this scope
  --> crates\winnet\src\tunnel_state.rs:36:17
   |
36 |         assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
   |                 ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `foreign_tunnel_up` in this scope
  --> crates\winnet\src\tunnel_state.rs:47:18
   |
47 |         assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
   |                  ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `foreign_tunnel_up` in this scope
  --> crates\winnet\src\tunnel_state.rs:58:18
   |
58 |         assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
   |                  ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `foreign_tunnel_up` in this scope
  --> crates\winnet\src\tunnel_state.rs:63:18
   |
63 |         assert!(!foreign_tunnel_up(&[], &[], "OfficeVPN"));
   |                  ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `foreign_tunnel_up` in this scope
  --> crates\winnet\src\tunnel_state.rs:74:17
   |
74 |         assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
   |                 ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `foreign_tunnel_up` in this scope
  --> crates\winnet\src\tunnel_state.rs:85:17
   |
85 |         assert!(foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
   |                 ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `foreign_tunnel_up` in this scope
  --> crates\winnet\src\tunnel_state.rs:96:18
   |
96 |         assert!(!foreign_tunnel_up(&office, &adapters, "OfficeVPN"));
   |                  ^^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `our_tunnel_up` in this scope
   --> crates\winnet\src\tunnel_state.rs:106:17
    |
106 |         assert!(our_tunnel_up(&adapters, "OfficeVPN"));
    |                 ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `our_tunnel_up` in this scope
   --> crates\winnet\src\tunnel_state.rs:116:18
    |
116 |         assert!(!our_tunnel_up(&adapters, "OfficeVPN"));
    |                  ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `our_tunnel_up` in this scope
   --> crates\winnet\src\tunnel_state.rs:121:18
    |
121 |         assert!(!our_tunnel_up(&[], "OfficeVPN"));
    |                  ^^^^^^^^^^^^^ not found in this scope

Some errors have detailed explanations: E0422, E0425.
For more information about an error, try `rustc --explain E0422`.
warning: `proxypilot-winnet` (lib test) generated 1 warning
error: could not compile `proxypilot-winnet` (lib test) due to 20 previous errors; 1 warning emitted
```

Exit code: 101. Настоящий прогон — `tunnel_state.rs` в момент этой команды
содержал только `#[cfg(test)] mod tests { ... }` с тестами против
`AdapterRoute`/`our_tunnel_up`/`foreign_tunnel_up`, но без единого
определения этих элементов (следуя указанию брифа: если RED упёрся бы в
«файл не найден», сначала создать пустой модуль — здесь пустой модуль
сразу дал настоящие ошибки типов, до этого шага дело не дошло).

### GREEN (после реализации)

Команда: `cargo test -p proxypilot-winnet tunnel_state`

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.80s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-09ac8735ffd742da.exe)

running 11 tests
test tunnel_state::tests::broader_foreign_route_covering_a_narrower_office_subnet_is_foreign ... ok
test tunnel_state::tests::narrower_foreign_route_inside_a_broader_office_subnet_is_foreign ... ok
test tunnel_state::tests::our_own_tunnel_is_not_foreign ... ok
test tunnel_state::tests::disjoint_foreign_tunnel_route_is_not_foreign ... ok
test tunnel_state::tests::empty_routing_table_is_not_foreign ... ok
test tunnel_state::tests::tunnel_carrying_office_route_is_foreign ... ok
test tunnel_state::tests::our_tunnel_up_false_when_alias_matches_but_not_a_tunnel ... ok
test tunnel_state::tests::office_route_through_non_tunnel_adapter_is_not_foreign ... ok
test tunnel_state::tests::our_tunnel_up_true_when_our_alias_is_a_tunnel ... ok
test tunnel_state::tests::permanently_up_tailscale_is_not_foreign_for_office_10_x ... ok
test tunnel_state::tests::our_tunnel_up_false_on_empty_adapters ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 106 filtered out; finished in 0.00s
```

## Проверка приёмки

- [x] Постоянно поднятый Tailscale (`100.64.0.0/10`) при офисных `10.x` —
      `permanently_up_tailscale_is_not_foreign_for_office_10_x`.
- [x] Туннель с маршрутом в офисную сеть — чужой —
      `tunnel_carrying_office_route_is_foreign`.
- [x] Наш собственный туннель — не чужой — `our_own_tunnel_is_not_foreign`.
- [x] Маршрут в офисную сеть через не-туннельный адаптер — не чужой —
      `office_route_through_non_tunnel_adapter_is_not_foreign`.
- [x] Пустая таблица — `false`, без паники —
      `empty_routing_table_is_not_foreign` (плюс `our_tunnel_up_false_on_empty_adapters`
      для второй функции).
- [x] Правило для широкого/узкого маршрута зафиксировано в
      doc-комментарии над `overlaps` и покрыто двумя тестами в обе
      стороны — `broader_foreign_route_covering_a_narrower_office_subnet_is_foreign`,
      `narrower_foreign_route_inside_a_broader_office_subnet_is_foreign`
      — плюс контрольный `disjoint_foreign_tunnel_route_is_not_foreign`
      на действительно непересекающиеся сети.

Дополнительно (не в списке приёмки, но напрашивалось для полноты второй
функции): `our_tunnel_up_true_when_our_alias_is_a_tunnel`,
`our_tunnel_up_false_when_alias_matches_but_not_a_tunnel`.

## CI, три команды

### `cargo test --all`

```
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.61s   (proxypilot-app, bin unittests)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s    (proxypilot-bridge, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (proxypilot-bridge, bin unittests)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s      (proxypilot-bridge, tests/cli.rs)
test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (proxypilot-core, lib unittests)
test result: ok. 115 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s   (proxypilot-winnet, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (doc-tests x3)
```

Итого: 359 passed, 0 failed, 3 ignored (было 348 + 3 ignored до этой
задачи; прирост — 11 новых тестов `tunnel_state`, 348 + 11 = 359,
сходится).

### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.23s
```

Чисто. Ни одного `#[allow(...)]` не добавлено.

### `cargo fmt --all --check`

Первый прогон нашёл одно место (сам новый файл — `cargo fmt` разбивает
длинную цепочку `&&` внутри `.any(|a| ...)` иначе, чем я написал вручную);
`cargo fmt --all` поправил, второй прогон — пустой diff, exit code 0.

## Границы

- Никакой `openvpn-gui.exe` не запускался, ничего не подключалось и не
  отключалось, маршруты машины не менялись.
- Ни один файл под `C:\Program Files\OpenVPN\config\` не читался и не
  писался — модуль работает только с `AdapterRoute`/`Ipv4Net` в памяти,
  тестовые таблицы — константы в тесте.
- Реестр не трогается вовсе (задача про строки и арифметику, ввода-вывода
  нет).
- Живая таблица маршрутов прочитана однократно, только на чтение, для
  ориентира по реалистичности фикстур:
  `route print -4` показал адаптеры `Wintun Userspace Tunnel`,
  `TAP-Windows Adapter V9` и `OpenVPN Data Channel Offload` как
  присутствующие в системе, но **не активные** в момент проверки — в
  активной таблице маршрутов нет ни одной записи через них (только
  собственные локальные подсети машины и мультикаст/broadcast; сами
  значения этих подсетей здесь намеренно не приводятся — правило
  CLAUDE.md про данные распространяется на `docs/process/` по имени, и
  прошлый прецедент утечки был именно такой формы, «описание с
  перечислением исходных значений»). Значит на этой машине сейчас нет ни
  поднятого нашего, ни поднятого чужого туннеля —
  фикстуры для теста Tailscale/чужого туннеля пришлось выдумать (что и
  ожидалось: бриф прямо просит покрыть тестами случаи, которых «эта
  машина сейчас не проявляет»). Команда read-only, `set address` /
  `set dnsservers` / `openvpn-gui.exe --command connect` /
  install-service не запускались.
- `#[allow(...)]` не использован. `unsafe`-блоков не добавлено — задача
  не требовала ни одного.

## Отклонения от плана / решения

Одно отклонение, изложено выше и обосновано: `foreign_tunnel_up`
получила третий параметр `our_alias: &str`, отсутствующий в сигнатуре
плана. Без него нельзя было бы реализовать и протестировать пункт
приёмки «наш собственный туннель не считается чужим» как свойство самой
функции — а не как поведение, которое сборка вокруг неё обязана
обеспечить сама, без проверки этим модулем. Функция остаётся чистой;
цена отклонения, если сочтут неверным, — вернуть фильтрацию по алиасу на
сторону вызывающего (задача 4/7), убрав параметр и один аргумент из
тестов.

`AdapterRoute` реализована как задано в брифе без изменений (в этом
раунде получила только `#[derive(Debug)]`, см. ниже — форма полей не
менялась).

## Fix round 1

Вердикт: Approved with fixes. Два Important, четыре Minor. Коммит
`5a6c4d3`. Ревьюер прогнал `overlaps` против независимой интервальной
арифметики на 200 704 парах CIDR (префиксы {0,8,10,16,24,30,32}) — ноль
расхождений — и подтвердил все четыре запрошенных случая (`0.0.0.0/0`
как чужой, чужой `/24` внутри нашей `/16`, `100.64.0.0/10` против
`10.0.0.0/8` как заведомо непересекающиеся без путаницы префиксов на
уровне строк, и адрес с незамаскированными хостовыми битами, собранный
мимо `FromStr`, всё равно matches благодаря перемаскировке в `range()`).
Отмечено также, что реализация `overlaps` сильнее заявленного в
комментарии обоснования: код не полагается на «CIDR-блоки либо вложены,
либо не пересекаются» и использует общий интервальный тест, который
остаётся верным и для невыровненных значений — это специально оставлено
как есть. Отклонение сигнатуры (`our_alias` у `foreign_tunnel_up`)
принято без изменений. RED-свидетельство проверено (не просто принято на
веру) сверкой смещений номеров строк между зафиксированным логом и
итоговым файлом — совпало, включая один тест с иным сдвигом из-за
отсутствия в нём строки `office`.

### 1. Important — байт-в-байт сравнение алиаса запирало приложение от его же туннеля

Проба ревьюера: alias `"officevpn"` против `our_alias="OfficeVPN"` (или с
хвостовым пробелом) давал `our_tunnel_up=false` **и**
`foreign_tunnel_up=true` одновременно — то есть дедлок под правилом,
ради которого вся задача существует («чужой туннель поднят — не трогаем
ни подъём, ни останов»): мы отказываемся остановить туннель, который
подняли сами, и ничего в UI это не может разрешить.

Исправлено: добавлена `same_alias` (`tunnel_state.rs:53`) — сравнение
через `.trim().to_lowercase()` с обеих сторон, используется и в
`our_tunnel_up`, и в `foreign_tunnel_up`. `to_lowercase()`, а не
`eq_ignore_ascii_case`, — псевдоним не обязан быть ASCII. Doc-комментарий
объясняет причину (расхождение регистра между `route print` и
`Get-NetRoute`) и отдельно оговаривает `our_alias = ""`: пустая строка
никогда не совпадёт с непустым алиасом, то есть незапрошенная
конфигурация читается как «наш туннель не поднят, чужой — есть» —
консервативный отказ, а не наоборот (замечание 6 закрыто тем же
комментарием, отдельного кода не потребовалось).

Более глубокое ограничение задокументировано в doc-комментарии модуля
(`tunnel_state.rs:16-25`), а не спрятано правкой сравнения: псевдоним
интерфейса Windows — это переименовываемая пользователем свободная
строка, разные инструменты (`route print`, `Get-NetRoute`, `netsh`)
показывают её не всегда одинаково, и это не устойчивый идентификатор.
Переименование адаптера, пока наш туннель поднят, снаружи неотличимо от
появления чужого туннеля с тем же маршрутом — `same_alias` эту проблему
не решает, только смягчает конкретно регистр и пробелы. Устойчивым
идентификатором был бы LUID или индекс интерфейса. Форма `AdapterRoute`
задана брифом этой задачи — я не менял её сам, но по указанию ревью
называю ограничение явно, чтобы задачи 4 и 7 унаследовали знание, а не
неожиданность на живой машине.

Тест `alias_comparison_is_case_insensitive_and_trims_whitespace`
(`tunnel_state.rs:276`) — алиас `" officevpn "` против `"OfficeVPN"`,
проверяет обе функции разом.

### 2. Important — отчёт называл реальные подсети этой машины

Ранняя версия этого отчёта переносила в раздел «Границы» конкретные
адреса, показанные `route print` на рабочей машине, — подсеть Wi-Fi и
подсеть локального виртуального адаптера. CLAUDE.md распространяет
правило про данные на `docs/process/` по имени, а прецедент, который оно
называет, — утечка ровно такой формы: запись, *описывающая* факт,
перечисляя исходные значения.

Исправлено: остались только выводы — туннельные адаптеры на машине
присутствуют, активных маршрутов через них нет, поэтому фикстуры тестов
собраны вручную, а не сняты с живой таблицы.

Смотреть живую таблицу было верно и полезно; переносить её содержимое
дословно — нет. Это различие и есть всё правило целиком.

Коммит с этой правкой не выложен в общую историю: контроллер
объединит задачу в один коммит перед пушем именно по этой причине —
чтобы утёкший текст не попал в публичную историю вовсе, а не только был
исправлен последующим коммитом. Историю не переписывал сам, как и
просили — только исправил файл.

### 3. Minor — не были покрыты тестами случаи, на которых держится критерий

Добавлены три теста (`tunnel_state.rs:231-273`):
`a_full_tunnel_commercial_vpn_is_foreign` (`0.0.0.0/0` — самый частый
случай чужого туннеля на практике, коммерческий full-tunnel VPN),
`a_single_host_route_inside_the_office_subnet_is_foreign` (`/32` внутри
офисной `/16`), `a_host_bits_set_destination_built_past_from_str_still_matches`
(`Ipv4Net { addr, prefix }` собран напрямую с ненулевыми хостовыми
битами — проверяет ту самую перемаскировку в `range()`, которая раньше
была защищена только комментарием). Все три уже проходили на коде до
этого раунда — ревьюер их проверил заранее пробником; тесты фиксируют
это поведение явно, а не оставляют его держаться на одном комментарии.

### 4. Minor — `AdapterRoute` не имел `#[derive(...)]`

Добавлен `#[derive(Debug)]` (`tunnel_state.rs:32`) — понадобится задачам
4 и 7 для логирования и для сообщений `assert_eq!`. Только `Debug`, как
явно просило ревью; `PartialEq`/`Clone` не добавлялись — не запрошены и
не используются нигде в этой задаче.

### 5. Minor — повторный импорт `Ipv4Net` в тестовом модуле

`use proxypilot_core::net::Ipv4Net;` в `mod tests` убран
(`tunnel_state.rs:116-118`) — тип уже приходит через `use super::*;`.

### 6. Minor — `our_alias = ""` совпадал бы с безымянным адаптером

Закрыто тем же комментарием, что и находка 1 (см. выше): одно
предложение в doc-comment над `same_alias`, кода это не потребовало.

### TDD этого раунда

Первая попытка описать RED для находки 1 ушла в сторону: я начал было
писать правдоподобный, но не снятый заново лог («иллюстративно», с
номером строки из головы и текстом паники, который `assert!` без
сообщения не печатает). Это была ровно та ошибка, о которой предупреждал
бриф, — и её тоже стоило признать, а не оставить как есть. Вместо этого
снят настоящий RED: `our_tunnel_up`/`foreign_tunnel_up` временно
возвращены к байт-в-байт `a.interface_alias == our_alias` /
`!= our_alias` (код коммита `5a6c4d3`, без `same_alias`), новый тест
`alias_comparison_is_case_insensitive_and_trims_whitespace` уже стоял в
файле, прогнан, затем файл возвращён к исправленной версии.

Команда: `cargo test -p proxypilot-winnet tunnel_state::tests::alias_comparison_is_case_insensitive_and_trims_whitespace -- --exact`
(production-код временно откачен к байт-в-байт сравнению алиаса):

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
warning: function `same_alias` is never used
  --> crates\winnet\src\tunnel_state.rs:53:4
   |
53 | fn same_alias(a: &str, b: &str) -> bool {
   |    ^^^^^^^^^^
   |
   = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: `proxypilot-winnet` (lib) generated 1 warning
warning: `proxypilot-winnet` (lib test) generated 1 warning (1 duplicate)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.38s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-09ac8735ffd742da.exe)

running 1 test
test tunnel_state::tests::alias_comparison_is_case_insensitive_and_trims_whitespace ... FAILED

failures:

---- tunnel_state::tests::alias_comparison_is_case_insensitive_and_trims_whitespace stdout ----

thread 'tunnel_state::tests::alias_comparison_is_case_insensitive_and_trims_whitespace' (34404) panicked at crates\winnet\src\tunnel_state.rs:286:9:
assertion failed: our_tunnel_up(&adapters, "OfficeVPN")
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    tunnel_state::tests::alias_comparison_is_case_insensitive_and_trims_whitespace

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p proxypilot-winnet --lib`
```

Exit code: 101. `same_alias` осталась в файле неиспользуемой (отсюда
предупреждение) — временный откат трогал только тела `our_tunnel_up` и
`foreign_tunnel_up`, саму функцию не удалял, чтобы не переписывать файл
дважды. После этого файл возвращён к исправленной версии, и прогон всего
модуля стал зелёным по всем 15 тестам (лог — ниже, раздел CI).

### GREEN: `tunnel_state` после возврата к исправленной версии

Команда: `cargo test -p proxypilot-winnet tunnel_state`

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.21s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-09ac8735ffd742da.exe)

running 15 tests
test tunnel_state::tests::alias_comparison_is_case_insensitive_and_trims_whitespace ... ok
test tunnel_state::tests::tunnel_carrying_office_route_is_foreign ... ok
test tunnel_state::tests::a_single_host_route_inside_the_office_subnet_is_foreign ... ok
test tunnel_state::tests::broader_foreign_route_covering_a_narrower_office_subnet_is_foreign ... ok
test tunnel_state::tests::a_host_bits_set_destination_built_past_from_str_still_matches ... ok
test tunnel_state::tests::a_full_tunnel_commercial_vpn_is_foreign ... ok
test tunnel_state::tests::empty_routing_table_is_not_foreign ... ok
test tunnel_state::tests::office_route_through_non_tunnel_adapter_is_not_foreign ... ok
test tunnel_state::tests::our_own_tunnel_is_not_foreign ... ok
test tunnel_state::tests::our_tunnel_up_false_on_empty_adapters ... ok
test tunnel_state::tests::our_tunnel_up_false_when_alias_matches_but_not_a_tunnel ... ok
test tunnel_state::tests::our_tunnel_up_true_when_our_alias_is_a_tunnel ... ok
test tunnel_state::tests::permanently_up_tailscale_is_not_foreign_for_office_10_x ... ok
test tunnel_state::tests::disjoint_foreign_tunnel_route_is_not_foreign ... ok
test tunnel_state::tests::narrower_foreign_route_inside_a_broader_office_subnet_is_foreign ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 106 filtered out; finished in 0.00s
```

15 = 11 из первого круга + 4 новых (`a_full_tunnel_commercial_vpn_is_foreign`,
`a_single_host_route_inside_the_office_subnet_is_foreign`,
`a_host_bits_set_destination_built_past_from_str_still_matches`,
`alias_comparison_is_case_insensitive_and_trims_whitespace`).

### CI, три команды — после исправлений раунда 1

#### `cargo test --all`

```
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.61s   (proxypilot-app, bin unittests)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s    (proxypilot-bridge, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (proxypilot-bridge, bin unittests)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s      (proxypilot-bridge, tests/cli.rs)
test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (proxypilot-core, lib unittests)
test result: ok. 119 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s   (proxypilot-winnet, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (doc-tests x3)
```

Итого: 363 passed, 0 failed, 3 ignored (было 359 + 3 ignored после
исходной сдачи задачи; прирост — 4 новых теста в `tunnel_state`,
359 + 4 = 363, сходится).

#### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.10s
```

Чисто. Ни одного `#[allow(...)]` не добавлено.

#### `cargo fmt --all --check`

Пустой diff уже на первом прогоне после правок, exit code 0.

## Границы (подтверждено повторно для раунда 1)

Никакой `openvpn-gui.exe` не запускался, ничего не подключалось и не
отключалось, маршруты машины не менялись. Ни один файл под
`C:\Program Files\OpenVPN\config\` не читался и не писался. Реестр не
трогался. `#[allow(...)]` не использован, `unsafe`-блоков не добавлено.
История git не переписывалась — контроллер выполнит squash сам.
