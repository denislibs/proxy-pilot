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
