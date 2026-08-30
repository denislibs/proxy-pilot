# Task 2 — Сборка split-tunnel профиля — отчёт

Branch: `feat/vpn-static-ip`, base HEAD: `41cf379`.

## Что сделано

- `crates/core/src/net.rs` (новый модуль, зарегистрирован в `crates/core/src/lib.rs`):
  - `Ipv4Net { addr: Ipv4Addr, prefix: u8 }`;
  - `mask_of(bits: u8) -> Ipv4Addr` — маска считается арифметикой из длины
    префикса (`u32::MAX << (32 - bits)`, `/0` отдельной веткой ради паники
    сдвига на 32 в debug-сборке — тот же приём, что и в `bypass.rs`).
    `bits` сверх 32 насыщается до 32, а не паникует — валидность входа уже
    держит `Ipv4Net::from_str`;
  - `impl FromStr for Ipv4Net` — `"10.0.0.0/8"`, с отдельными вариантами
    ошибки `Ipv4NetParseError` на отсутствие `/`, битый адрес, нечисловой
    или превышающий 32 префикс;
  - `impl Display for Ipv4Net` — точный round-trip с `FromStr` (важно для
    задачи 5, где этим же текстом подсети хранятся в TOML).
- `crates/winnet/src/ovpn_profile.rs` (новый модуль, зарегистрирован
  алфавитно в `crates/winnet/src/lib.rs` между `openvpn` и `sysproxy`):
  - `pub fn build_profile(source: &str, routes: &[Ipv4Net]) -> String` —
    чистая функция без файлового I/O.
  - `crates/winnet/Cargo.toml`: добавлена зависимость
    `proxypilot-core = { path = "../core" }` (по образцу `bridge`/`app`).

### Как собран профиль

1. Из исходных строк вычищается `setenv opt block-outside-dns` (артефакт
   другой Windows-сборки клиента, наш ругается на неё при каждом старте) —
   в любом месте исходника, не только внутри своего блока.
2. Если в исходнике уже есть блок между маркерами
   `# --- ProxyPilot: начало добавленного блока, не редактировать руками ---`
   и `# --- ProxyPilot: конец добавленного блока ---` (результат прошлой
   сборки), он вычищается целиком.
3. Хвостовые пустые строки после вычистки убираются — иначе каждая
   пересборка копила бы ещё одну пустую строку.
4. В конец дописывается свежий блок: `pull-filter ignore
   "redirect-gateway"`, `route <net> <mask>` на каждый элемент `routes`
   (маска — через `mask_of` из `core::net`), и три explaining-комментария
   по-русски: зачем `redirect-gateway`-фильтр, зачем явные маршруты, и
   почему пушенный DNS осознанно НЕ фильтруется (спека 8.2, задача 7
   покажет это в UI).

Идемпотентность обеспечена блоком-с-маркерами, а не построчной проверкой
«такая директива уже есть»: при следующей сборке (после того как задача 5
изменит список офисных подсетей в конфиге) весь прежний блок вычищается и
пишется заново — старый `route` за уже убранную из конфига подсеть не
остаётся сиротой навсегда. Тест `building_twice_does_not_duplicate_directives`
гоняет `build_profile` над собственным выводом и считает вхождения.

`pull-filter ignore "dhcp-option DNS"` **не добавляется** — тест
`pushed_dns_is_not_filtered` проверяет отсутствие строки `dhcp-option DNS`
в результате.

`build_profile` не читает конфиг и не видит `OfficeNetwork` (GUID сети
NLM) — рулинг «нет `default_routes(office_networks)`» соблюдён: маршруты
приходят параметром, откуда их взять — решает вызывающий код в задаче 5/6.

## TDD evidence

### RED: `crates/core/src/net.rs` (пустой файл, только тесты — до реализации)

Команда: `cargo test -p proxypilot-core net::`

```
   Compiling syn v3.0.4
   Compiling serde_derive v1.0.229
   Compiling thiserror-impl v2.0.20
   Compiling thiserror v2.0.20
   Compiling serde v1.0.229
   Compiling serde_spanned v0.6.9
   Compiling toml_datetime v0.6.3
   Compiling toml_edit v0.20.2
   Compiling toml v0.8.2
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\core)
warning: unused import: `super::*`
 --> crates\core\src\net.rs:3:9
  |
3 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

warning: unused import: `std::str::FromStr`
 --> crates\core\src\net.rs:5:9
  |
5 |     use std::str::FromStr;
  |         ^^^^^^^^^^^^^^^^^

error[E0425]: cannot find function `mask_of` in this scope
 --> crates\core\src\net.rs:9:20
  |
9 |         assert_eq!(mask_of(0), Ipv4Addr::new(0, 0, 0, 0));
  |                    ^^^^^^^ not found in this scope

error[E0425]: cannot find function `mask_of` in this scope
  --> crates\core\src\net.rs:14:20
   |
14 |         assert_eq!(mask_of(1), Ipv4Addr::new(128, 0, 0, 0));
   |                    ^^^^^^^ not found in this scope

error[E0425]: cannot find function `mask_of` in this scope
  --> crates\core\src\net.rs:19:20
   |
19 |         assert_eq!(mask_of(8), Ipv4Addr::new(255, 0, 0, 0));
   |                    ^^^^^^^ not found in this scope

error[E0425]: cannot find function `mask_of` in this scope
  --> crates\core\src\net.rs:26:20
   |
26 |         assert_eq!(mask_of(14), Ipv4Addr::new(255, 252, 0, 0));
   |                    ^^^^^^^ not found in this scope

error[E0425]: cannot find function `mask_of` in this scope
  --> crates\core\src\net.rs:31:20
   |
31 |         assert_eq!(mask_of(24), Ipv4Addr::new(255, 255, 255, 0));
   |                    ^^^^^^^ not found in this scope

error[E0425]: cannot find function `mask_of` in this scope
  --> crates\core\src\net.rs:36:20
   |
36 |         assert_eq!(mask_of(31), Ipv4Addr::new(255, 255, 255, 254));
   |                    ^^^^^^^ not found in this scope

error[E0425]: cannot find function `mask_of` in this scope
  --> crates\core\src\net.rs:41:20
   |
41 |         assert_eq!(mask_of(32), Ipv4Addr::new(255, 255, 255, 255));
   |                    ^^^^^^^ not found in this scope

error[E0433]: cannot find type `Ipv4Net` in this scope
  --> crates\core\src\net.rs:46:19
   |
46 |         let net = Ipv4Net::from_str("10.0.0.0/8").expect("должен разобраться");
   |                   ^^^^^^^ use of undeclared type `Ipv4Net`

error[E0433]: cannot find type `Ipv4Net` in this scope
  --> crates\core\src\net.rs:54:17
   |
54 |         assert!(Ipv4Net::from_str("203.0.113.0/33").is_err());
   |                 ^^^^^^^ use of undeclared type `Ipv4Net`

error[E0433]: cannot find type `Ipv4Net` in this scope
  --> crates\core\src\net.rs:59:17
   |
59 |         assert!(Ipv4Net::from_str("203.0.113.0").is_err());
   |                 ^^^^^^^ use of undeclared type `Ipv4Net`

error[E0433]: cannot find type `Ipv4Net` in this scope
  --> crates\core\src\net.rs:64:17
   |
64 |         assert!(Ipv4Net::from_str("203.0.113.0/abc").is_err());
   |                 ^^^^^^^ use of undeclared type `Ipv4Net`

error[E0433]: cannot find type `Ipv4Net` in this scope
  --> crates\core\src\net.rs:69:17
   |
69 |         assert!(Ipv4Net::from_str("not-an-address/8").is_err());
   |                 ^^^^^^^ use of undeclared type `Ipv4Net`

Some errors have detailed explanations: E0425, E0433.
For more information about an error, try `rustc --explain E0425`.
warning: `proxypilot-core` (lib test) generated 2 warnings
error: could not compile `proxypilot-core` (lib test) due to 12 previous errors; 2 warnings emitted
```

Exit code: 101.

### GREEN: `core::net` после реализации

Команда: `cargo test -p proxypilot-core net::`

```
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\core)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.24s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-fcb0899bb01237a1.exe)

running 12 tests
test net::tests::mask_of_eight_bits_is_a_full_octet ... ok
test net::tests::mask_of_twenty_four_bits_is_three_full_octets ... ok
test net::tests::mask_of_thirty_two_bits_is_a_single_host ... ok
test net::tests::parse_rejects_a_missing_prefix ... ok
test net::tests::mask_of_thirty_one_bits_leaves_a_single_host_bit ... ok
test net::tests::mask_of_zero_is_all_zero ... ok
test net::tests::parse_rejects_a_malformed_address ... ok
test net::tests::mask_of_fourteen_bits_does_not_round_to_a_full_octet ... ok
test net::tests::parse_rejects_a_prefix_over_thirty_two ... ok
test net::tests::mask_of_one_bit ... ok
test net::tests::parse_and_display_roundtrip ... ok
test net::tests::parse_rejects_a_non_numeric_prefix ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 48 filtered out; finished in 0.00s
```

### RED: `crates/winnet/src/ovpn_profile.rs` (тесты написаны над несуществующей `build_profile`, `core::net` уже реализован)

Команда: `cargo test -p proxypilot-winnet ovpn_profile::`

```
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\core)
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
warning: unused import: `super::*`
 --> crates\winnet\src\ovpn_profile.rs:3:9
  |
3 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0425]: cannot find function `build_profile` in this scope
  --> crates\winnet\src\ovpn_profile.rs:31:19
   |
31 |         let out = build_profile(SOURCE, &routes());
   |                   ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_profile` in this scope
  --> crates\winnet\src\ovpn_profile.rs:37:19
   |
37 |         let out = build_profile(SOURCE, &routes());
   |                   ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_profile` in this scope
  --> crates\winnet\src\ovpn_profile.rs:44:19
   |
44 |         let out = build_profile(SOURCE, &routes());
   |                   ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_profile` in this scope
  --> crates\winnet\src\ovpn_profile.rs:56:19
   |
56 |         let out = build_profile(SOURCE, &routes());
   |                   ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_profile` in this scope
  --> crates\winnet\src\ovpn_profile.rs:64:19
   |
64 |         let out = build_profile(SOURCE, &routes());
   |                   ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_profile` in this scope
  --> crates\winnet\src\ovpn_profile.rs:70:20
   |
70 |         let once = build_profile(SOURCE, &routes());
   |                    ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_profile` in this scope
  --> crates\winnet\src\ovpn_profile.rs:71:21
   |
71 |         let twice = build_profile(&once, &routes());
   |                     ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `build_profile` in this scope
  --> crates\winnet\src\ovpn_profile.rs:82:19
   |
82 |         let out = build_profile(SOURCE, &[]);
   |                   ^^^^^^^^^^^^^ not found in this scope

For more information about this error, try `rustc --explain E0425`.
warning: `proxypilot-winnet` (lib test) generated 1 warning
error: could not compile `proxypilot-winnet` (lib test) due to 8 previous errors; 1 warning emitted
```

Exit code: 101.

### GREEN: `ovpn_profile` после реализации

Команда: `cargo test -p proxypilot-winnet ovpn_profile::`

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.61s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-09ac8735ffd742da.exe)

running 7 tests
test ovpn_profile::tests::empty_routes_still_adds_the_filter ... ok
test ovpn_profile::tests::block_outside_dns_is_stripped ... ok
test ovpn_profile::tests::source_lines_survive ... ok
test ovpn_profile::tests::pushed_dns_is_not_filtered ... ok
test ovpn_profile::tests::every_route_is_present ... ok
test ovpn_profile::tests::redirect_gateway_is_filtered ... ok
test ovpn_profile::tests::building_twice_does_not_duplicate_directives ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 68 filtered out; finished in 0.00s
```

## CI, три команды

### `cargo test --all`

Итоговые строки по крейтам (полный лог — 309 тестов, весь вывод собирался
локально и опущен здесь построчно ради объёма; ниже — `test result:` каждого
рана и любые FAILED/error, которых нет):

```
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.57s   (proxypilot-app, bin unittests)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s    (proxypilot-bridge, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (proxypilot-bridge, bin unittests)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s      (proxypilot-bridge, tests/cli.rs)
test result: ok. 60 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (proxypilot-core, lib unittests)
test result: ok. 73 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s    (proxypilot-winnet, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (doc-tests x3)
```

Итого: 309 passed, 0 failed, 3 ignored (было 290 passed + 3 ignored до этой
задачи; прирост — 12 тестов `core::net` + 7 тестов `ovpn_profile` = 19,
290 + 19 = 309, сходится).

### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\bridge)
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.98s
```

Чисто, без единого предупреждения. Ни одного `#[allow(...)]` не добавлено.

### `cargo fmt --all --check`

Первый прогон нашёл 3 непрочищенных места (свои же новые файлы, ловится
сразу же — `cargo fmt --all` их поправил), второй прогон:

```
FMT_OK
```
(пустой diff, exit code 0).

## Проверка приёмки

- [x] `mask_of` на `/0, /1, /8, /14, /24, /31, /32` — 7 тестов, все зелёные.
- [x] Профиль содержит `pull-filter ignore "redirect-gateway"` и все
      переданные маршруты — `redirect_gateway_is_filtered`,
      `every_route_is_present`, `empty_routes_still_adds_the_filter`.
- [x] Исходные строки присутствуют в результате — `source_lines_survive`
      (включая кириллицу внутри сертификата-заглушки).
- [x] `block-outside-dns` вычищен — `block_outside_dns_is_stripped`.
- [x] Пушенный DNS не отфильтрован — `pushed_dns_is_not_filtered` проверяет
      отсутствие `dhcp-option DNS` в результате.
- [x] Повторная сборка не удваивает директивы —
      `building_twice_does_not_duplicate_directives`.
- [x] Round-trip `Ipv4Net`, отклонение `/33`, отсутствующего префикса,
      нечислового префикса, битого адреса — 5 тестов в `core::net`.

## Границы

- Никакой `openvpn-gui.exe` не запускался, ничего не подключалось и не
  отключалось.
- Ни один файл под `C:\Program Files\OpenVPN\config\` не читался, не
  перемещался, не переименовывался и не писался — `build_profile` работает
  только со строками в памяти, тестовый источник — константа в тесте.
  Каталог, куда класть собранный профиль на диске, эта задача не решает
  (её решает задача 6, которая тоже не пишет реальные файлы без отдельного
  разрешения).
- Запись в реестр не производилась — задача чисто над строками, реестр не
  трогает вовсе (в отличие от Task 1, использующего только `HKLM`-чтение).
- `#[allow(...)]` не использован. `unsafe`-блоков не добавлено.

## Отклонения от плана / решения

Ничего сверх решений контроллера из брифа не потребовалось. Оба рулинга
(`Ipv4Net` в `core`, отсутствие `default_routes(office_networks)`)
соблюдены как есть.

Одно решение реализации, не оговорённое явно брифом: идемпотентность
сделана через блок с маркерами (`# --- ProxyPilot: начало ... ---` /
`# --- ProxyPilot: конец ... ---`), а не через построчную проверку «эта
директива уже есть». Разница всплывёт в задаче 5/6, когда список офисных
подсетей в конфиге поменяется между пересборками: маркерный блок вычищает
и пишет заново весь набор `route`, тогда как построчная проверка оставила
бы маршрут за подсеть, которую убрали из конфига, в профиле навсегда. Если
это решение сочтут неверным — правка небольшая: заменить вычистку по
маркерам на построчную дедупликацию, тесты акцептанса это не различают
(они гоняют с одинаковым списком `routes` на обоих вызовах).

## Fix round 1

Ревьюер собрал отдельный пробный крейт вне репозитория с `#[path]`-инклудом
настоящего `ovpn_profile.rs` и независимо проверил арифметику, а не
поверил тестам: `mask_of` подтверждена на `/0 /1 /8 /14 /24 /31 /32`, `/0`
не паникует в debug, `FromStr` отклоняет `/33`, отсутствующий префикс,
нечисловой, пробелы, пять октетов, `/256`, `/8/9`. `core` не приобрёл
платформенных зависимостей. TDD-логи сверены построчно и признаны
подлинными. Marker-block решение признано верным и разрешающим напряжение
между «вычистить» и «сохранить»: `block-outside-dns` вычищается по
строкам вне блока, поэтому пересборка над уже собранным результатом —
неподвижная точка, а пользователь, вручную вернувший эту директиву в
исходник, получает её вычищенной заново.

Найдено 9 замечаний, все исправлены на коммите `91b235d`. Правки — в тех
же файлах: `crates/core/src/net.rs`, `crates/winnet/src/ovpn_profile.rs`.

### 1. Important — несбалансированный BEGIN обрезал профиль

`in_generated_block` раньше не имел «отката»: одинокий `BEGIN` без `END`
(обрезанный или вручную подправленный прошлый результат) включал вычистку
до конца файла, сертификаты включительно. Исправлено — `matched_generated_block`
(`ovpn_profile.rs`) ищет пару `BEGIN`+`END`, где `END` строго после
`BEGIN`, и вычищает диапазон только при найденной паре; без пары маркеры
не трогаются вовсе, они остаются обычными строками (для OpenVPN это
безвредный комментарий, начинающийся с `#`). Тест
`an_unbalanced_begin_marker_does_not_truncate_the_profile` гоняет источник
с одиноким `BEGIN_MARKER` перед сертификатом и проверяет, что сертификат
остался.

### 2. Important — биты хоста не маскировались

Исправлено в обоих местах, как и требовалось, — и обе правки остаются
намеренно, не как дублирование:

- `Ipv4Net::from_str` (`net.rs`) теперь маскирует адрес по вычисленной
  маске сразу при разборе — одно каноническое представление на подсеть.
  Тест `parse_masks_host_bits`: `"10.1.2.3/24"` → `addr = 10.1.2.0`,
  `to_string() == "10.1.2.0/24"`.
- `build_profile` (`ovpn_profile.rs`) маскирует адрес заново при
  формировании `route`, функцией `masked_addr` — потому что поля
  `Ipv4Net` публичны и конструктор в обход `FromStr`
  (`Ipv4Net { addr, prefix }` напрямую) эту маскировку не проходит; задачи
  3, 5, 7 будут собирать такие значения не только через `FromStr`.
  Doc-комментарий над `masked_addr` в коде явно называет причину
  сохранять обе точки защиты. Тест
  `route_host_bits_are_masked_even_when_ipv4net_is_built_directly`
  строит `Ipv4Net { addr: "203.0.113.5".parse().unwrap(), prefix: 24 }`
  напрямую (в обход `FromStr`) и проверяет, что в `route` попал
  `203.0.113.0`, а не `.5`.

### 3. Minor — вычистка `block-outside-dns` по точному совпадению строки

`is_block_outside_dns_directive` (`ovpn_profile.rs`) теперь отрезает
хвостовой комментарий (после `#` или `;`) и схлопывает повторяющиеся
пробелы перед сравнением с канонической директивой. Тесты
`block_outside_dns_with_extra_whitespace_is_stripped` (двойные пробелы) и
`block_outside_dns_with_a_trailing_comment_is_stripped` (директива с
`# заметка` после неё).

### 4. Minor — дублирование `pull-filter`, уже стоящего вне блока

`build_profile` перед формированием нового блока проверяет, есть ли
`pull-filter ignore "redirect-gateway"` уже среди уцелевших строк
источника (`redirect_gateway_present`); если да — не добавляет ни
объясняющий комментарий, ни саму директиву повторно. Тест
`an_existing_redirect_gateway_filter_outside_the_block_is_not_duplicated`.

### 5. Minor — CRLF источника превращался в LF

`detect_line_ending` (`ovpn_profile.rs`) считает, каких переносов строк в
источнике больше — `\r\n` или голых `\n` — и результат собирается тем же
переносом. Решает большинство, а не первая встреченная строка, чтобы один
случайный перенос другого стиля не решал за весь файл. Тест
`crlf_source_stays_crlf`: источник целиком на `\r\n`, проверяется, что в
результате число `"\n"` совпадает с числом `"\r\n"` (то есть голых `\n`
нет вовсе).

### 6-7. Minor — `net.rs` принимал `+8` и `/08`; пустой источник получал
### лишнюю пустую строку

`parse_prefix` (`net.rs`) требует, чтобы длина префикса состояла только
из ASCII-цифр и не начиналась с `0` при длине больше одного знака —
отклоняет `+8` (не цифра) и `08` (не канонический вид), при этом `"0"`
по-прежнему принимается. Тесты `parse_rejects_a_leading_plus_prefix`,
`parse_rejects_a_leading_zero_prefix`, `parse_accepts_a_bare_zero_prefix`.

В `build_profile` пустая строка-разделитель перед добавленным блоком
теперь пишется, только если после вычистки источника осталась хоть одна
строка (`if !out_lines.is_empty()`). Тест
`empty_source_has_no_leading_blank_line`: `build_profile("", &routes())`
начинается сразу с `BEGIN_MARKER`.

### 8. Minor — `Cargo.lock` v3→v4

Разобрано: добавление зависимости `proxypilot-core = { path = "../core" }`
в `crates/winnet/Cargo.toml` заставило cargo 1.98 перезаписать
`Cargo.lock`, и cargo при перезаписи поднял формат файла с `version = 3`
на `version = 4` — это самостоятельное решение тулчейна при любом
touch-е файла, не ручная правка. В этом раунде исправлений `Cargo.lock` не
менялся вовсе (новых зависимостей не добавлялось) — `git diff --stat
Cargo.lock` пуст. Формат `v4` совместим с MSRV 1.88 (`--locked` проходит,
см. прогон `cargo test --all` ниже) и был осознанно оставлен, а не
отменён втихую, как и просил ревью.

### 9. Minor — стиль импортов

`use proxypilot_core::net::Ipv4Net;` в `ovpn_profile.rs` заменён на
`use proxypilot_core::net::{mask_of, Ipv4Net};` наверху модуля;
`proxypilot_core::net::mask_of(...)` по месту вызова заменён на короткое
`mask_of(...)`. В тестовом модуле убран повторный
`use proxypilot_core::net::Ipv4Net;` — тип уже приходит через
`use super::*;`.

### TDD этого раунда

Правки шли не по чистому red-green с нуля (менялась уже покрытая тестами
функция), поэтому RED снят честно, а не реконструирован по памяти:
production-код коммита `91b235d` (`git show 91b235d:<путь>`) временно
подставлен обратно под уже написанные новые тесты (тесты — из
исправленной версии файла), прогнан, и только после того как каждый новый
тест падал по делу — файлы возвращены к исправленной версии. Ниже —
дословный вывод обоих прогонов.

#### RED: `crates/core/src/net.rs` (production-код `91b235d` + новые тесты этого раунда)

Команда: `cargo test -p proxypilot-core net::`

```
running 20 tests
test net::tests::mask_of_eight_bits_is_a_full_octet ... ok
test net::tests::mask_of_fourteen_bits_does_not_round_to_a_full_octet ... ok
test net::tests::mask_of_one_bit ... ok
test net::tests::mask_of_thirty_one_bits_leaves_a_single_host_bit ... ok
test net::tests::mask_of_twenty_four_bits_is_three_full_octets ... ok
test net::tests::parse_rejects_a_malformed_address ... ok
test net::tests::parse_rejects_a_prefix_over_thirty_two ... ok
test net::tests::mask_of_zero_is_all_zero ... ok
test net::tests::parse_and_display_roundtrip ... ok
test net::tests::mask_of_thirty_two_bits_is_a_single_host ... ok
test net::tests::parse_rejects_a_missing_prefix ... ok
test net::tests::parse_rejects_a_non_numeric_prefix ... ok
test net::tests::parse_rejects_a_prefix_that_does_not_fit_a_byte ... ok
test net::tests::parse_rejects_a_second_slash ... ok
test net::tests::parse_rejects_five_octets ... ok
test net::tests::parse_rejects_whitespace_around_the_prefix ... ok
test net::tests::parse_rejects_a_leading_plus_prefix ... FAILED
test net::tests::parse_accepts_a_bare_zero_prefix ... FAILED
test net::tests::parse_masks_host_bits ... FAILED
test net::tests::parse_rejects_a_leading_zero_prefix ... FAILED

failures:

---- net::tests::parse_rejects_a_leading_plus_prefix stdout ----

thread 'net::tests::parse_rejects_a_leading_plus_prefix' (9640) panicked at crates\core\src\net.rs:171:9:
assertion failed: Ipv4Net::from_str("203.0.113.0/+8").is_err()

---- net::tests::parse_accepts_a_bare_zero_prefix stdout ----

thread 'net::tests::parse_accepts_a_bare_zero_prefix' (31076) panicked at crates\core\src\net.rs:143:9:
assertion `left == right` failed
  left: 203.0.113.5
 right: 0.0.0.0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- net::tests::parse_masks_host_bits stdout ----

thread 'net::tests::parse_masks_host_bits' (34140) panicked at crates\core\src\net.rs:136:9:
assertion `left == right` failed
  left: 10.1.2.3
 right: 10.1.2.0

---- net::tests::parse_rejects_a_leading_zero_prefix stdout ----

thread 'net::tests::parse_rejects_a_leading_zero_prefix' (11328) panicked at crates\core\src\net.rs:176:9:
assertion failed: Ipv4Net::from_str("203.0.113.0/08").is_err()


failures:
    net::tests::parse_accepts_a_bare_zero_prefix
    net::tests::parse_masks_host_bits
    net::tests::parse_rejects_a_leading_plus_prefix
    net::tests::parse_rejects_a_leading_zero_prefix

test result: FAILED. 16 passed; 4 failed; 0 ignored; 0 measured; 48 filtered out; finished in 0.00s
error: test failed, to rerun pass `-p proxypilot-core --lib`
```

Четыре падения — ровно замечания 2 (`parse_masks_host_bits`,
`parse_accepts_a_bare_zero_prefix` — последний ловит маскировку именно на
границе `/0`) и 6/7 (`parse_rejects_a_leading_plus_prefix`,
`parse_rejects_a_leading_zero_prefix`). Остальные новые тесты (пробелы,
пять октетов, переполнение байта, второй `/`) уже проходили на старом
коде — они не про новые баги, а про то же поведение, что ревьюер уже
проверил своим пробником, и остаются в файле как регресс-тесты.

#### RED: `crates/winnet/src/ovpn_profile.rs` (production-код `91b235d` + новые тесты этого раунда)

Команда: `cargo test -p proxypilot-winnet ovpn_profile::`

```
running 14 tests
test ovpn_profile::tests::empty_routes_still_adds_the_filter ... ok
test ovpn_profile::tests::block_outside_dns_is_stripped ... ok
test ovpn_profile::tests::building_twice_does_not_duplicate_directives ... ok
test ovpn_profile::tests::every_route_is_present ... ok
test ovpn_profile::tests::pushed_dns_is_not_filtered ... ok
test ovpn_profile::tests::redirect_gateway_is_filtered ... ok
test ovpn_profile::tests::source_lines_survive ... ok
test ovpn_profile::tests::route_host_bits_are_masked_even_when_ipv4net_is_built_directly ... FAILED
test ovpn_profile::tests::an_unbalanced_begin_marker_does_not_truncate_the_profile ... FAILED
test ovpn_profile::tests::block_outside_dns_with_extra_whitespace_is_stripped ... FAILED
test ovpn_profile::tests::crlf_source_stays_crlf ... FAILED
test ovpn_profile::tests::an_existing_redirect_gateway_filter_outside_the_block_is_not_duplicated ... FAILED
test ovpn_profile::tests::empty_source_has_no_leading_blank_line ... FAILED
test ovpn_profile::tests::block_outside_dns_with_a_trailing_comment_is_stripped ... FAILED

failures:

---- ovpn_profile::tests::route_host_bits_are_masked_even_when_ipv4net_is_built_directly stdout ----

thread 'ovpn_profile::tests::route_host_bits_are_masked_even_when_ipv4net_is_built_directly' (9684) panicked at crates\winnet\src\ovpn_profile.rs:244:9:
assertion failed: out.contains("route 203.0.113.0 255.255.255.0")

---- ovpn_profile::tests::an_unbalanced_begin_marker_does_not_truncate_the_profile stdout ----

thread 'ovpn_profile::tests::an_unbalanced_begin_marker_does_not_truncate_the_profile' (13392) panicked at crates\winnet\src\ovpn_profile.rs:219:9:
assertion failed: out.contains("CERT")

---- ovpn_profile::tests::block_outside_dns_with_extra_whitespace_is_stripped stdout ----

thread 'ovpn_profile::tests::block_outside_dns_with_extra_whitespace_is_stripped' (33128) panicked at crates\winnet\src\ovpn_profile.rs:171:9:
assertion failed: !out.contains("block-outside-dns")
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- ovpn_profile::tests::crlf_source_stays_crlf stdout ----

thread 'ovpn_profile::tests::crlf_source_stays_crlf' (18056) panicked at crates\winnet\src\ovpn_profile.rs:252:9:
assertion failed: out.contains("client\r\ndev tun")

---- ovpn_profile::tests::an_existing_redirect_gateway_filter_outside_the_block_is_not_duplicated stdout ----

thread 'ovpn_profile::tests::an_existing_redirect_gateway_filter_outside_the_block_is_not_duplicated' (3264) panicked at crates\winnet\src\ovpn_profile.rs:228:9:
assertion `left == right` failed
  left: 2
 right: 1

---- ovpn_profile::tests::empty_source_has_no_leading_blank_line stdout ----

thread 'ovpn_profile::tests::empty_source_has_no_leading_blank_line' (16964) panicked at crates\winnet\src\ovpn_profile.rs:261:9:
assertion failed: out.starts_with(BEGIN_MARKER)

---- ovpn_profile::tests::block_outside_dns_with_a_trailing_comment_is_stripped stdout ----

thread 'ovpn_profile::tests::block_outside_dns_with_a_trailing_comment_is_stripped' (3852) panicked at crates\winnet\src\ovpn_profile.rs:181:9:
assertion failed: !out.contains("block-outside-dns")


failures:
    ovpn_profile::tests::an_existing_redirect_gateway_filter_outside_the_block_is_not_duplicated
    ovpn_profile::tests::an_unbalanced_begin_marker_does_not_truncate_the_profile
    ovpn_profile::tests::block_outside_dns_with_a_trailing_comment_is_stripped
    ovpn_profile::tests::block_outside_dns_with_extra_whitespace_is_stripped
    ovpn_profile::tests::crlf_source_stays_crlf
    ovpn_profile::tests::empty_source_has_no_leading_blank_line
    ovpn_profile::tests::route_host_bits_are_masked_even_when_ipv4net_is_built_directly

test result: FAILED. 7 passed; 7 failed; 0 ignored; 0 measured; 68 filtered out; finished in 0.00s
error: test failed, to rerun pass `-p proxypilot-winnet --lib`
```

Все 7 новых тестов упали, по одному на каждое замечание 1, 3 (два теста —
пробелы и хвостовой комментарий), 2, 4, 5, 6/7. После этого файлы
возвращены к исправленной версии и весь прогон (все 4 крейта) стал
зелёным — см. ниже.

### CI, три команды — после исправлений

#### `cargo test --all`

```
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.60s   (proxypilot-app, bin unittests)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.06s    (proxypilot-bridge, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (proxypilot-bridge, bin unittests)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s      (proxypilot-bridge, tests/cli.rs)
test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (proxypilot-core, lib unittests)
test result: ok. 80 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s    (proxypilot-winnet, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (doc-tests x3)
```

Итого: 324 passed, 0 failed, 3 ignored (было 309 после исходной задачи;
прирост — 8 новых тестов в `core::net` + 7 новых в `ovpn_profile` = 15,
309 + 15 = 324, сходится).

#### `cargo clippy --all-targets -- -D warnings`

```
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\bridge)
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.08s
```

Чисто. Ни одного `#[allow(...)]` не добавлено.

#### `cargo fmt --all --check`

Первый прогон после правок нашёл несколько мест, где `cargo fmt` иначе
разбивает многострочные `push(...)` (сам код из фикс-раунда) — `cargo fmt
--all` их поправил, второй прогон:

```
FMT_OK
```

### Границы (подтверждено повторно)

Никакой `openvpn-gui.exe` не запускался. Ни один файл под
`C:\Program Files\OpenVPN\config\` не читался и не писался — весь
фикс-раунд правил только `crates/core/src/net.rs` и
`crates/winnet/src/ovpn_profile.rs`, оба теста работают со строками в
памяти. Запись в реестр не производилась. `#[allow(...)]` не
использован, `unsafe`-блоков не добавлено.

## Fix round 2

Вердикт был "Needs rework": один Critical, найденный пробником, а не
чтением, и это находка 1 из раунда 1, вернувшаяся спустя одну сборку.

Что подтвердилось из раунда 1: ревьюер заново независимо проверил
арифметику и подтвердил, что **обе точки маскировки битов хоста целы и
реально проверяются** — `route_host_bits_are_masked_even_when_ipv4net_is_built_directly`
строит `Ipv4Net { addr, prefix }` через публичные поля и проверяет, что
адрес хоста не попадает в вывод; это было ровно то место, которое легче
всего тихо схлопнуть обратно к одинарной маскировке, и оно не схлопнулось.
Идемпотентность держится на LF-источниках, на CRLF-источниках и на
источниках, уже несущих `pull-filter ignore "redirect-gateway"`. Пункты
2, 3, 5, 6, 7, 8, 9 раунда 1 закрыты и не переоткрывались.

Ревьюер также независимо реконструировал RED-доказательства раунда 1:
подставил production-код `91b235d` под тесты раунда 1 и сверил номера
строк паники, счётчики (core 20/4, winnet 14/7) и конкретное значение
`left: 203.0.113.5 / right: 0.0.0.0`, которое старый немаскирующий код
действительно выдаёт. Итог ревьюера: "reads as a real run" — решение
выбросить придуманный лог и сделать по-честному было верным, а раскрыть
это — тем более.

### 1. Critical — усечение возвращалось на второй сборке

`matched_generated_block` (раунд 1) спаривала *первый* `BEGIN` с *любым*
последующим `END`. Непарный `BEGIN` в источнике переживал первую сборку
как обычная строка — но добавленный этой же сборкой `END` в конце файла
становился ему парой на **следующей** сборке, и всё между ними (включая
сертификаты) стиралось. Пробником ревьюера:

```
A1 keeps CERT-A: true
A2 keeps CERT-A: false   ← вторая сборка
```

Опаснее исходного раунд-1-бага по двум причинам: ошибка молчит именно на
той сборке, что её создаёт (первая выглядит правильной), и задача 5
пересобирает профиль при каждой смене офисных подсетей — то есть второй
вызов является обычным ходом дел, а не патологией.

Исправлено в `crates/winnet/src/ovpn_profile.rs`, функция переименована в
`lines_to_drop`: вместо «найти первый BEGIN + любой следующий END» —
однопроходный разбор с состоянием `pending_begin`. Каждый маркер, не
образовавший пару (одинокий `BEGIN` без `END`, одинокий `END` без
предшествующего `BEGIN`, более ранний `BEGIN`, вытесненный более поздним
до появления `END`), вычищается **сам, одной строкой**, а не как начало
диапазона до следующего чужого маркера. Тест
`an_unbalanced_begin_marker_does_not_truncate_the_profile` расширен именно
так, как просило ревью — второй вызов `build_profile` над результатом
первого, с отдельным `assert!` на CERT для каждого прохода. Добавлен и
отдельный тест на конвергенцию —
`a_source_that_is_only_a_begin_marker_is_a_fixpoint` (источник — один
одинокий `BEGIN` без ничего вокруг; `build_profile(build_profile(x)) ==
build_profile(x)`).

### 2. Important — маркеры внутри inline-блока (`<ca>`) стирали контент

Тот же корень, другой триггер: если `BEGIN`/`END` оказывались внутри
`<ca>...</ca>`, пара распознавалась и удаляла реальное содержимое
сертификата между ними, оставляя пустую оболочку из тегов.

`lines_to_drop` теперь отслеживает, находится ли текущая строка внутри
inline-блока (`is_inline_block_open`/`is_inline_block_close` — строка вида
`<tag>` / `</tag>`), и **не распознаёт маркеры вовсе**, пока флаг взведён:
содержимое между открывающим и закрывающим тегом — непрозрачные PEM-данные,
и наш собственный блок туда никогда не попадает (мы всегда дописываем его
на верхнем уровне, в конец файла). Тест
`markers_inside_an_inline_block_do_not_delete_its_content` — маркеры внутри
`<ca>...</ca>`, проверка на первом и втором проходе (по методическому
совету ревью — для каждой правки, трогающей маркеры, проверять оба).

### 3. Minor — `redirect_gateway_present` сравнивался буквально

Раунд 1 научил нормализации (схлопывание пробелов, отрезание хвостового
комментария) только вычистку `block-outside-dns`; проверка «такой
`pull-filter` уже есть» сравнивала строки через `l.trim() ==` буквально, и
источник с двойным пробелом получал вторую копию. Вынесена общая
`normalize_directive`, переиспользуемая и `is_block_outside_dns_directive`,
и проверкой `redirect_gateway_present` — одна и та же проблема не решается
дважды по-разному. Тест
`an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated`.

### 4. Minor — ничья CRLF/LF решалась в пользу LF

`detect_line_ending` требовала строгого большинства (`crlf >
lf_only`), поэтому один CRLF плюс один голый LF давали ничью 1:1,
уходившую в LF, — единственная CRLF-строка источника переписывалась.
Теперь ничья при ненулевом сигнале уходит в CRLF (профиль обычно готовят
на Windows), а источник вовсе без переносов строк (пустой или
однострочный) по-прежнему получает `\n` — обе исходные гарантии («пустой
источник даёт LF», «источник без завершающего переноса получает его»)
сохранены отдельной веткой на случай `crlf == 0 && lf_only == 0`. Тест
`a_tie_between_crlf_and_lf_favours_crlf`.

### 5. Minor — источник только из BEGIN не был неподвижной точкой

Прямое следствие Critical — тест `a_source_that_is_only_a_begin_marker_is_a_fixpoint`
(добавлен для пункта 1 выше) проверяет это напрямую и подтверждён зелёным
после исправления.

### 6. Ошибка формулировки в CLAUDE.md, не в коде

Правка не в `net.rs` (примеры `10.1.2.3/24` и `10.0.0.0/8` в тестах
оставлены как есть), а в `CLAUDE.md`, раздел «Данные: репозиторий
публичный»: буквальное прочтение прежней формулировки запрещало бы
`192.168.0.0/16` и `10.0.0.0/8` в `DEFAULT_NO_PROXY`
(`crates/core/src/config.rs`) — то есть рабочее поведение продукта.
Переформулировано: правило — про утечку данных о реальной инфраструктуре,
а не про диапазоны как таковые; серые адреса RFC 1918 явно разрешены и как
продуктовые значения по умолчанию, и как обобщённые примеры в коде и
тестах. Документационные диапазоны RFC 5737 остаются обязательными только
там, где нужен пример, изображающий конкретный (вымышленный) внешний адрес.

### Метод: RED для этого раунда — тоже настоящий прогон, не реконструкция

Тот же приём, что и в раунде 1: production-код коммита `2a444a5` (раунд 1)
подставлен обратно под пять новых тестов этого раунда, прогнан, и только
после этого файлы возвращены к исправленной версии.

Команда: `cargo test -p proxypilot-winnet ovpn_profile::` (production-код
`2a444a5` + тесты раунда 2):

```
running 18 tests
test ovpn_profile::tests::block_outside_dns_with_a_trailing_comment_is_stripped ... ok
test ovpn_profile::tests::an_existing_redirect_gateway_filter_outside_the_block_is_not_duplicated ... ok
test ovpn_profile::tests::block_outside_dns_is_stripped ... ok
test ovpn_profile::tests::block_outside_dns_with_extra_whitespace_is_stripped ... ok
test ovpn_profile::tests::crlf_source_stays_crlf ... ok
test ovpn_profile::tests::empty_source_has_no_leading_blank_line ... ok
test ovpn_profile::tests::building_twice_does_not_duplicate_directives ... ok
test ovpn_profile::tests::every_route_is_present ... ok
test ovpn_profile::tests::redirect_gateway_is_filtered ... ok
test ovpn_profile::tests::empty_routes_still_adds_the_filter ... ok
test ovpn_profile::tests::pushed_dns_is_not_filtered ... ok
test ovpn_profile::tests::route_host_bits_are_masked_even_when_ipv4net_is_built_directly ... ok
test ovpn_profile::tests::markers_inside_an_inline_block_do_not_delete_its_content ... FAILED
test ovpn_profile::tests::an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated ... FAILED
test ovpn_profile::tests::a_source_that_is_only_a_begin_marker_is_a_fixpoint ... FAILED
test ovpn_profile::tests::a_tie_between_crlf_and_lf_favours_crlf ... FAILED
test ovpn_profile::tests::an_unbalanced_begin_marker_does_not_truncate_the_profile ... FAILED
test ovpn_profile::tests::source_lines_survive ... ok

failures:

---- ovpn_profile::tests::markers_inside_an_inline_block_do_not_delete_its_content stdout ----

thread 'ovpn_profile::tests::markers_inside_an_inline_block_do_not_delete_its_content' (27580) panicked at crates\winnet\src\ovpn_profile.rs:314:9:
первая сборка потеряла CERT-D

---- ovpn_profile::tests::an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated stdout ----

thread 'ovpn_profile::tests::an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated' (37384) panicked at crates\winnet\src\ovpn_profile.rs:347:9:
assertion `left == right` failed
  left: 2
 right: 1

---- ovpn_profile::tests::a_source_that_is_only_a_begin_marker_is_a_fixpoint stdout ----

thread 'ovpn_profile::tests::a_source_that_is_only_a_begin_marker_is_a_fixpoint' (2252) panicked at crates\winnet\src\ovpn_profile.rs:302:9:
assertion `left == right` failed
  left: "# --- ProxyPilot: начало добавленного блока, не редактировать руками ---\n# Сервер обычно пушит маршрут по умолчанию и не пушит маршруты\n# в офисные подсети — без строки ниже весь трафик, включая видео,\n# уходит в туннель кругом через офис (спека 8.1).\npull-filter ignore \"redirect-gateway\"\n# Явные маршруты в офисные подсети. Подсеть, где машина стоит\n# физически, не страдает: её собственная запись в таблице\n# маршрутов точнее любой из этих.\nroute 203.0.113.0 255.255.255.0\nroute 198.51.100.0 255.255.255.0\n# Пушенный DNS осознанно НЕ фильтруется (расхождение с macOS-версией,\n# спека 8.2): туннель нужен ради внутренних имён (git, dev-серверы),\n# а без офисного DNS они не резолвятся. Плата — пока туннель поднят,\n# все DNS-запросы идут в офис; это показывается в UI (задача 7).\n# --- ProxyPilot: конец добавленного блока ---\n"
 right: "# --- ProxyPilot: начало добавленного блока, не редактировать руками ---\n\n# --- ProxyPilot: начало добавленного блока, не редактировать руками ---\n# Сервер обычно пушит маршрут по умолчанию и не пушит маршруты\n# в офисные подсети — без строки ниже весь трафик, включая видео,\n# уходит в туннель кругом через офис (спека 8.1).\npull-filter ignore \"redirect-gateway\"\n# Явные маршруты в офисные подсети. Подсеть, где машина стоит\n# физически, не страдает: её собственная запись в таблице\n# маршрутов точнее любой из этих.\nroute 203.0.113.0 255.255.255.0\nroute 198.51.100.0 255.255.255.0\n# Пушенный DNS осознанно НЕ фильтруется (расхождение с macOS-версией,\n# спека 8.2): туннель нужен ради внутренних имён (git, dev-серверы),\n# а без офисного DNS они не резолвятся. Плата — пока туннель поднят,\n# все DNS-запросы идут в офис; это показывается в UI (задача 7).\n# --- ProxyPilot: конец добавленного блока ---\n"

---- ovpn_profile::tests::a_tie_between_crlf_and_lf_favours_crlf stdout ----

thread 'ovpn_profile::tests::a_tie_between_crlf_and_lf_favours_crlf' (32036) panicked at crates\winnet\src\ovpn_profile.rs:379:9:
assertion failed: out.starts_with("client\r\ndev tun\r\n")
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- ovpn_profile::tests::an_unbalanced_begin_marker_does_not_truncate_the_profile stdout ----

thread 'ovpn_profile::tests::an_unbalanced_begin_marker_does_not_truncate_the_profile' (13648) panicked at crates\winnet\src\ovpn_profile.rs:288:9:
вторая сборка потеряла CERT


failures:
    ovpn_profile::tests::a_source_that_is_only_a_begin_marker_is_a_fixpoint
    ovpn_profile::tests::a_tie_between_crlf_and_lf_favours_crlf
    ovpn_profile::tests::an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated
    ovpn_profile::tests::an_unbalanced_begin_marker_does_not_truncate_the_profile
    ovpn_profile::tests::markers_inside_an_inline_block_do_not_delete_its_content

test result: FAILED. 13 passed; 5 failed; 0 ignored; 0 measured; 68 filtered out; finished in 0.00s
error: test failed, to rerun pass `-p proxypilot-winnet --lib`
```

Пять падений — ровно на замечания Critical (`an_unbalanced_begin_marker...`
— вторая сборка), пункт 5 (`a_source_that_is_only_a_begin_marker_is_a_fixpoint`),
Important (`markers_inside_an_inline_block...`), и Minor 3 и 4
(`an_existing_redirect_gateway_filter_with_odd_whitespace...`,
`a_tie_between_crlf_and_lf_favours_crlf`). После этого файл возвращён к
исправленной версии, добавлена вторая (post-fix) сборка в проверке
inline-блока (по тому же методическому совету), и весь прогон стал
зелёным.

Один побочный итог самого исправления: `lines_to_drop` изначально писала
диапазон циклом `for idx in begin..=i { drop[idx] = true; }`, что поймал
`cargo clippy` (`needless_range_loop`) — переписано на
`drop.iter_mut().take(i + 1).skip(begin)`, без `#[allow(...)]`.

### CI, три команды — после исправлений раунда 2

#### `cargo test --all`

```
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.58s   (proxypilot-app, bin unittests)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s    (proxypilot-bridge, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (proxypilot-bridge, bin unittests)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s      (proxypilot-bridge, tests/cli.rs)
test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (proxypilot-core, lib unittests)
test result: ok. 84 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s    (proxypilot-winnet, lib unittests)
test result: ok. 0 passed; 0 failed; 0 measured; 0 filtered out; finished in 0.00s     (doc-tests x3)
```

Итого: 328 passed, 0 failed, 3 ignored (было 324 после раунда 1; прирост —
4 новых теста в `ovpn_profile`:
`markers_inside_an_inline_block_do_not_delete_its_content`,
`an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated`,
`a_tie_between_crlf_and_lf_favours_crlf`,
`a_source_that_is_only_a_begin_marker_is_a_fixpoint`; 324 + 4 = 328,
сходится).

#### `cargo clippy --all-targets -- -D warnings`

Первый прогон после исправления Critical поймал `needless_range_loop`
(см. выше), исправлено без `#[allow(...)]`. Повторный прогон:

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.30s
```

Чисто.

#### `cargo fmt --all --check`

Первый прогон нашёл два места, где `cargo fmt` иначе переносит цепочки
`.push(...)` — `cargo fmt --all` поправил, повторный прогон:

```
FMT_OK
```

### Границы (подтверждено повторно)

Никакой `openvpn-gui.exe` не запускался. Ни один файл под
`C:\Program Files\OpenVPN\config\` не читался и не писался — правки этого
раунда затронули `crates/winnet/src/ovpn_profile.rs` и `CLAUDE.md`, оба
теста работают со строками в памяти. Запись в реестр не производилась.
`#[allow(...)]` не использован (в том числе и для находки clippy этого
раунда — она исправлена, не заглушена), `unsafe`-блоков не добавлено.

## Fix round 3

Вердикт был "Needs rework": три находки Important, две — тот же класс
тихой порчи данных, который раунд 2 должен был закрыть.

### Поправка к раунду 2

Ревью верно указало на неточность в тексте предыдущего раунда: там все
пять изменённых тестов названы «новых тестов этого раунда» одним списком.
На деле `an_unbalanced_begin_marker_does_not_truncate_the_profile`
существовал с раунда 1 и в раунде 2 был расширен (добавлена вторая
сборка), а не создан заново. Новых тестов было четыре:
`a_source_that_is_only_a_begin_marker_is_a_fixpoint`,
`markers_inside_an_inline_block_do_not_delete_its_content`,
`an_existing_redirect_gateway_filter_with_odd_whitespace_is_not_duplicated`,
`a_tie_between_crlf_and_lf_favours_crlf`. Исправляю здесь явно, текст
раунда 2 не переписываю (см. правило «дописывать»).

### Что подтвердилось

Ревью независимо пробовало классификатор на полном наборе раскладок
маркеров — ни одной, только BEGIN, только END, парная пара, два BEGIN
затем END, BEGIN/END/BEGIN, END раньше BEGIN, вложенные, с комментариями,
как подстрока, с пробелами, всё то же на CRLF, и целые уже собранные
профили, поданные заново — и каждая из них оказалась неподвижной точкой
через три сборки. Эта часть (`normalize_directive` как единственный
нормализатор для обеих директив, ничья CRLF/LF в пользу CRLF с LF для
пустого источника, `mask_of` на всех границах, двойная маскировка с обеих
сторон включая конструирование `Ipv4Net` через публичные поля) подтверждена
и не трогалась.

### Структурная причина, а не три отдельных бага

Три находки этого раунда — не независимые баги, а один и тот же корень:
до этой правки в файле было **три независимых классификатора строки**
(`lines_to_drop` вело собственный учёт inline-блоков; проверка
`redirect_gateway_present` о них не знала вовсе; `is_inline_block_open` и
`is_inline_block_close` расходились в требованиях друг с другом), и
каждое новое требование добавляло четвёртое мнение вместо того, чтобы
поправить решение в одном месте.

Сделано так, как просило ревью — не патч поверх патча, а одна
перестройка: `crates/winnet/src/ovpn_profile.rs` теперь строится вокруг
единственного прохода `classify_lines`, который относит каждую строку
источника ровно к одному из четырёх [`LineKind`]: `TopLevel`, `Inline`,
`Begin`, `End`. Всё остальное — `drop_mask` (какие строки вычищаются),
проверка `redirect_gateway_present`, вычистка `block-outside-dns` —
потребляет готовую классификацию и не передопрашивает текст. Открывающий
и закрывающий тег (`inline_tag_open`/`inline_tag_close`) теперь
симметричны, требуют совпадения имени тега и терпят хвостовой комментарий
одинаково с обеих сторон.

Незакрытый inline-блок стал явной ошибкой: `build_profile` теперь
возвращает `Result<String, ProfileError>`, и `ProfileError::UnterminatedInlineBlock
{ tag, line }` называет тег и номер строки, где он открылся. Единственный
вызывающий на сегодня — тесты этого модуля; задача 4 будет первым
настоящим потребителем, задачи 3, 5 и 7 ещё не написаны — дешевле момента
для смены сигнатуры не будет.

### A. Important — состояние inline-блока протекало за конец файла

Незакрытый `<ca>` держал `in_inline_block` взведённым до EOF, поэтому наш
же хвостовой блок (всегда в конце файла) становился невидим для
распознавания маркеров, и каждая следующая сборка дописывала ещё один —
без предела. Пробник ревью: шесть сборок незакрытого `<ca>` дали
route-count 6.

Закрыто перестройкой: `classify_lines` возвращает `Err` при незакрытом
блоке вместо того, чтобы строить что-то похожее на профиль. Тест
`an_unterminated_inline_block_is_rejected_not_silently_mangled` проверяет
и текст ошибки (тег `ca`, строка `2`), и
`repeatedly_building_from_an_unterminated_block_keeps_failing_the_same_way`
— что повторные попытки не начинают вдруг «собираться» и не ведут себя
по-разному от вызова к вызову.

### B. Important — открывающая и закрывающая проверки расходились

`is_inline_block_open` (раунд 2) требовала `ends_with('>')`, поэтому
`<ca> # сертификат` не признавался открытием блока — а `</ca>` блок всё
равно закрывал. Маркеры между ними считались нашими, и содержимое
(включая сертификат) вычищалось на первой же сборке. Пробник ревью
терял `CERT3`.

Закрыто той же перестройкой: `inline_tag_open`/`inline_tag_close`
симметричны и оба терпят хвостовой контент после имени тега. Тест
`an_open_tag_with_a_trailing_comment_still_protects_its_contents` — три
сборки подряд, `CERT3` и сам комментарий после `<ca>` целы на каждой.

### C. Important — проверка «фильтр уже стоит» не знала про inline-блоки

`pull-filter ignore "redirect-gateway"` внутри `<connection>` — валидный
OpenVPN, но не top-level. Старая проверка сравнивала со всеми уцелевшими
строками без разбора, поэтому считала фильтр уже присутствующим и не
добавляла top-level копию — то есть ровно тот отказ, который спека 8.1
называет причиной существования этой строки: весь трафик уходит в туннель
кругом через офис, и это устойчивая неподвижная точка, которая сама себя
не чинит при пересборке.

Закрыто тем же классификатором: `redirect_gateway_present` теперь
проверяется только по строкам, классифицированным как `TopLevel` — то,
что лежит внутри `<connection>`, для неё непрозрачно. Тест
`a_redirect_gateway_filter_inside_a_connection_block_does_not_suppress_the_top_level_one`
— два прохода, каждый раз ровно два вхождения фильтра (тот, что внутри
`<connection>`, и наш top-level).

### Метод: классификатор проверен напрямую, не только через build_profile

По совету ревью — `classify_lines` теперь пробуется напрямую, отдельным
подмодулем `tests::classify`, без смешивания с форматированием вывода и
списком маршрутов: пустой источник, одинокий `BEGIN`, одинокий `END`,
пара, два `BEGIN` затем `END`, `BEGIN`/`END`/`BEGIN`, `END` раньше
`BEGIN`, маркер как подстрока (не распознаётся), маркер с пробелами
(распознаётся), тег-двойник внутри уже открытого блока (не открывает
вложенный), открывающий и закрывающий тег с хвостовым комментарием,
несовпадающий закрывающий тег (блок не закрывается), незакрытый блок
(ошибка с тегом и строкой), целый уже собранный профиль, поданный заново.
14 тестов в `ovpn_profile::tests::classify`.

Там, где ошибка не бьёт по одному вызову, тесты гоняют три сборки подряд
(`build_chain`), не только первую и вторую: `an_unbalanced_begin_marker_does_not_truncate_the_profile`,
`a_source_that_is_only_a_begin_marker_is_a_fixpoint`,
`markers_inside_an_inline_block_do_not_delete_its_content`,
`an_open_tag_with_a_trailing_comment_still_protects_its_contents`,
`an_inline_block_survives_a_crlf_source_across_three_builds` (тот же
класс сценариев, но на CRLF-источнике).

### RED-доказательства: находки A/B/C воспроизведены на коде раунда 2

Тот же приём, что и в раундах 1-2, с поправкой: полная перестройка
сигнатуры (`String` → `Result<String, ProfileError>`) означает, что
новый набор тестов физически не компилируется поверх старого кода — это
такой же честный сигнал RED, что и «тип не найден» в самом первом раунде
задачи, но он не отвечает на вопрос «а действительно ли баги A/B/C —
баги». Поэтому для этого раунда написаны три отдельных минимальных
пробника на **собственном API кода раунда 2** (`build_profile(&str, &[Ipv4Net])
-> String`, без `Result`), подставленных под production-код коммита
`f448a9c`, прогнанных и затем отброшенных.

Команда: `cargo test -p proxypilot-winnet round3_probes -- --nocapture`
(production-код `f448a9c` + три пробника):

```
running 3 tests
redirect-gateway occurrences: 1

thread 'ovpn_profile::round3_probes::probe_b_open_tag_with_trailing_comment_loses_content' (28248) panicked at crates\winnet\src\ovpn_profile.rs:301:9:
ОЖИДАНИЕ ОШИБКИ: CERT3 обязан выжить, но раунд 2 его теряет
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

thread 'ovpn_profile::round3_probes::probe_c_redirect_gateway_inside_connection_suppresses_top_level_filter' (30244) panicked at crates\winnet\src\ovpn_profile.rs:321:9:
assertion `left == right` failed: ОЖИДАНИЕ ОШИБКИ: обязано быть 2 (внутри <connection> + наш top-level), раунд 2 добавляет только 0 новых
  left: 1
 right: 2
test ovpn_profile::round3_probes::probe_c_redirect_gateway_inside_connection_suppresses_top_level_filter ... FAILED
route counts across 6 builds: [1, 2, 3, 4, 5, 6]

thread 'ovpn_profile::round3_probes::probe_a_unterminated_ca_grows_route_count_across_builds' (19612) panicked at crates\winnet\src\ovpn_profile.rs:284:9:
assertion `left == right` failed: ОЖИДАНИЕ ОШИБКИ: шестая сборка должна была бы тоже дать один маршрут, но раунд 2 копит директивы
  left: 6
 right: 1
test ovpn_profile::round3_probes::probe_b_open_tag_with_trailing_comment_loses_content ... FAILED
test ovpn_profile::round3_probes::probe_a_unterminated_ca_grows_route_count_across_builds ... FAILED

failures:
    ovpn_profile::round3_probes::probe_a_unterminated_ca_grows_route_count_across_builds
    ovpn_profile::round3_probes::probe_b_open_tag_with_trailing_comment_loses_content
    ovpn_profile::round3_probes::probe_c_redirect_gateway_inside_connection_suppresses_top_level_filter

test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 68 filtered out; finished in 0.00s
error: test failed, to rerun pass `-p proxypilot-winnet --lib`
```

Числа совпадают с пробником ревью дословно: route-count `[1, 2, 3, 4, 5,
6]` через шесть сборок незакрытого `<ca>` (находка A), `CERT3` теряется
на первой же сборке (находка B), `redirect-gateway` остаётся `1`
вхождением вместо `2` (находка C). После прогона production-код и тесты
возвращены к исправленной версии; `diff` подтвердил побайтовое совпадение
с версией до подстановки.

### CI, три команды — после исправлений раунда 3

#### `cargo test --all`

```
test result: ok. 105 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.60s   (proxypilot-app, bin unittests)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s    (proxypilot-bridge, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (proxypilot-bridge, bin unittests)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s      (proxypilot-bridge, tests/cli.rs)
test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (proxypilot-core, lib unittests)
test result: ok. 104 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.13s   (proxypilot-winnet, lib unittests)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (doc-tests x3)
```

Итого: 348 passed, 0 failed, 3 ignored (было 328 после раунда 2; прирост —
20 тестов в `ovpn_profile` (18 → 38): 14 прямых проб классификатора
(`tests::classify::*`), плюс
`a_redirect_gateway_filter_inside_a_connection_block_does_not_suppress_the_top_level_one`,
`an_open_tag_with_a_trailing_comment_still_protects_its_contents`,
`an_unterminated_inline_block_is_rejected_not_silently_mangled`,
`repeatedly_building_from_an_unterminated_block_keeps_failing_the_same_way`,
`an_inline_block_survives_a_crlf_source_across_three_builds`,
`а_source_that_is_only_a_begin_marker_is_a_fixpoint` не новый — уже был в
раунде 2. 328 + 20 = 348, сходится).

#### `cargo clippy --all-targets -- -D warnings`

Первый прогон поймал `useless_format` — тестовый источник без
интерполяции был обёрнут в `format!()` по инерции от соседних строк с
`{BEGIN_MARKER}`; заменено на строковый литерал без `#[allow(...)]`.
Повторный прогон:

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.66s
```

Чисто.

#### `cargo fmt --all --check`

Первый прогон нашёл несколько мест, где `cargo fmt` иначе переносит
длинные цепочки и строковые литералы — `cargo fmt --all` поправил,
повторный прогон:

```
FMT_OK
```

### Границы (подтверждено повторно)

Никакой `openvpn-gui.exe` не запускался. Ни один файл под
`C:\Program Files\OpenVPN\config\` не читался и не писался — весь раунд
затронул только `crates/winnet/src/ovpn_profile.rs`, все тесты и пробники
работают со строками в памяти. Запись в реестр не производилась.
`#[allow(...)]` не использован (в том числе для находки clippy этого
раунда — она исправлена, не заглушена), `unsafe`-блоков не добавлено.
