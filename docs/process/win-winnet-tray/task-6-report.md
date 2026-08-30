# Task 6 — Системный прокси в реестре — отчёт

**Статус:** DONE

## Что сделано

- `win/crates/winnet/Cargo.toml` — в features `windows` добавлены `"Win32_Networking_WinInet"` и `"Win32_System_Registry"`.
- `win/crates/winnet/src/lib.rs` — добавлена одна строка `pub mod sysproxy;` (по алфавиту после `networks`). Файл больше ничем не тронут.
- `win/crates/winnet/src/sysproxy.rs` — новый модуль: `SysProxy`, `to_bypass_string`, `read`, `apply`.

### Ключевые решения реализации

**`RegKey` — RAII-обёртка над `HKEY`.** `read`/`apply` выходят через `?` из середины, и ручной `RegCloseKey` в конце функции пропустил бы каждый ошибочный путь. `Drop` закрывает ключ на любом выходе, включая панику. `RegKey` не `Copy`/`Clone`, так что закрытие ровно одно.

**Права ровно те, что нужны:** `read` открывает с `KEY_READ`, `apply` — с `KEY_READ | KEY_WRITE`. Оба — в `HKCU`, никакого UAC.

**Отсутствующее значение — не ошибка.** `RegQueryValueExW` возвращает `ERROR_FILE_NOT_FOUND` — `query_raw` отдаёт `Ok(None)`, `query_string` превращает это в пустую строку, `query_dword` — в 0. Проверено обеими ветками (первый и второй вызов).

**Двойной вызов за размером.** Первый `RegQueryValueExW` — без буфера, только `lptype`/`lpcbdata`. Второй — с буфером ровно на полученный размер. Если значение выросло между вызовами, второй вернёт `ERROR_MORE_DATA`, и мы отдаём ошибку, а не обрезанные данные. Вырожденный случай `needed == 0` обрабатывается до выделения буфера, чтобы не передавать в API dangling-указатель пустого `Vec`.

**Строки — UTF-16LE с завершающим нулём.** `encode_utf16_sz` строит `Vec<u8>` напрямую (`unsafe` не нужен), длина среза, которую `windows-rs` передаёт как `cbdata`, уже включает нуль. Обратно `decode_utf16_sz` режет строку по первому нулевому юниту — иначе `"\0"` уехал бы в конфиг и вернулся оттуда внутрь значения. На это есть три отдельных теста.

**Оба `InternetSetOptionW`.** `INTERNET_OPTION_SETTINGS_CHANGED`, затем `INTERNET_OPTION_REFRESH`, после закрытия ключа. Без обоих уже запущенные приложения продолжают ходить по старым настройкам до перезапуска — снаружи это выглядит как «функция не работает».

**Значение неожиданного типа** (не `REG_SZ`/`REG_EXPAND_SZ` для строк, не `REG_DWORD` для `ProxyEnable`) не роняет запуск, но и не проглатывается молча: пишется `tracing::warn!` с типом значения, и мы считаем настройку пустой/выключенной.

**Каждый `unsafe` снабжён `// SAFETY:`** — их шесть: `RegOpenKeyExW`, два `RegQueryValueExW`, `RegSetValueExW` (dword и строка), `RegCloseKey` в `Drop`, и два `InternetSetOptionW`.

**Границы честно названы в doc-комментарии модуля:** WinHTTP (`netsh winhttp` — контекст служб, нужен администратор), Firefox (свои настройки мимо WinINET), приложения, читающие `HTTP_PROXY` из окружения.

## TDD: RED

Сначала добавлены features и `pub mod sysproxy;`, файл `sysproxy.rs` создан только с блоком тестов (пустой модуль, чтобы дойти до ошибок типов, а не остановиться на «file not found for module»).

```
$ cd win && cargo test -p proxypilot-winnet sysproxy
   Compiling windows v0.58.0
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
error[E0425]: cannot find function `read` in this scope
  --> crates\winnet\src\sysproxy.rs:33:17
   |
33 |         let s = read().expect("HKCU Internet Settings обязан читаться");
   |                 ^^^^ not found in this scope
   |
help: consider importing one of these functions
   |
 3 +     use std::fs::read;
   |
 3 +     use std::ptr::read;
   |
 3 +     use core::ptr::read;
   |

warning: unused import: `super::*`
 --> crates\winnet\src\sysproxy.rs:3:9
  |
3 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0425]: cannot find function `to_bypass_string` in this scope
 --> crates\winnet\src\sysproxy.rs:9:17
  |
9 |         let s = to_bypass_string("localhost,127.0.0.1,.local,192.168.0.0/16");
  |                 ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `to_bypass_string` in this scope
  --> crates\winnet\src\sysproxy.rs:18:17
   |
18 |         let s = to_bypass_string(".local");
   |                 ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `to_bypass_string` in this scope
  --> crates\winnet\src\sysproxy.rs:24:17
   |
24 |         let s = to_bypass_string("localhost,,  ,127.0.0.1");
   |                 ^^^^^^^^^^^^^^^^ not found in this scope

For more information about this error, try `rustc --explain E0425`.
warning: `proxypilot-winnet` (lib test) generated 1 warning
error: could not compile `proxypilot-winnet` (lib test) due to 4 previous errors; 1 warning emitted
```

## TDD: GREEN

```
$ cd win && cargo test -p proxypilot-winnet sysproxy
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.97s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-165a8654af058a73.exe)

running 7 tests
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

К четырём тестам из брифа добавлены три собственных, закрывающих именно ту ошибку, которую труднее всего заметить глазами: `reg_sz_bytes_end_with_a_utf16_nul`, `reg_sz_bytes_of_an_empty_string_are_just_the_nul`, `decoding_drops_the_terminating_nul`.

## Ручная проверка на живой машине

Перед началом контроллером был сохранён снимок в `proxy-settings-before-task6.txt`; дополнительно я снял точную копию значений в JSON (scratchpad `proxy-before.json`) для побайтового сравнения при восстановлении.

### Шаг 1 — наш собственный `read()` («до»)

```
$ cargo run -q -p proxypilot-winnet --example sysproxy_probe -- read
enabled = false
server  = 203.0.113.10:3128
bypass  = 198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
```

Совпадает со снимком контроллера символ в символ (`ProxyEnable` = 0 → `enabled = false`).

### Шаг 2 — применили тестовое значение

```
$ cargo run -q -p proxypilot-winnet --example sysproxy_probe -- apply 1 "127.0.0.1:3129" "127.0.0.1;localhost;*.local;<local>"
applied: SysProxy { enabled: true, server: "127.0.0.1:3129", bypass: "127.0.0.1;localhost;*.local;<local>" }
```

### Шаг 3 — независимая проверка («после»), PowerShell, не наш `read()`

```
PS> $p = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
ProxyEnable   : 1 (Int32)
ProxyServer   : 127.0.0.1:3129 (len=14)
ProxyOverride : 127.0.0.1;localhost;*.local;<local> (len=35)
AutoConfigURL :

HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Internet Settings
    ProxyServer    REG_SZ    127.0.0.1:3129
```

Длина `ProxyServer` = 14 (PowerShell отбрасывает завершающий нуль, но лишнего символа внутри строки нет), тип — `REG_SZ`, `ProxyEnable` — `Int32`/DWord. `AutoConfigURL` не тронут.

### Шаг 4 — восстановление исходных значений

```
$ cargo run -q -p proxypilot-winnet --example sysproxy_probe -- apply 0 "203.0.113.10:3128" "198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>"
applied: SysProxy { enabled: false, server: "203.0.113.10:3128", bypass: "198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>" }
```

### Шаг 5 — независимая проверка восстановления («вернули»)

Сравнение с сохранённым до начала работы JSON, оператором `-ceq` (регистрозависимо):

```
ProxyEnable   : 0
ProxyServer   : 203.0.113.10:3128
ProxyOverride : 198.51.100.221;198.51.100.8;198.51.100.248;198.51.100.222:8080;203.0.113.154;192.168.*;intranet-app.example.internal;lo.example.internal;<local>
AutoConfigURL :

MATCH ProxyEnable   : True
MATCH ProxyServer   : True
MATCH ProxyOverride : True
MATCH AutoConfigURL : True
TYPE ProxyEnable    : DWord
TYPE ProxyServer    : String
TYPE ProxyOverride  : String
```

**Исходные настройки машины восстановлены точно, включая `ProxyOverride` символ в символ. `AutoConfigURL` не трогали ни на одном шаге.**

Вспомогательный `examples/sysproxy_probe.rs`, которым делалась эта проверка, после неё удалён — в коммит вошли только три файла из брифа.

## CI-команды

```
$ cd win && cargo fmt --all --check
fmt: OK   (вывод пуст — нарушений нет)
```

```
$ cd win && cargo test --all
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-f9bfedea04baa417.exe)
running 51 tests
test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s
     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-837393c89186d591.exe)
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests\cli.rs (target\debug\deps\cli-eb7488564f5ac25b.exe)
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-ce820a0b07ec9f56.exe)
running 45 tests
test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-4f04aa23a4b32fba.exe)
running 14 tests
test networks::tests::category_maps_every_documented_value ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

   Doc-tests proxypilot_bridge / proxypilot_core / proxypilot_winnet
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Итого 112 тестов (было 105, добавлено 7).

```
$ cd win && cargo clippy --all-targets -- -D warnings
    Checking windows v0.58.0
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.72s
```

Ни одного `#[allow]` не добавлено.

## Самопроверка

- Ключ закрывается на любом пути, включая ошибочный — через `Drop` у `RegKey`. Да.
- Строки пишутся UTF-16 с завершающим нулём, длина в байтах включает нуль. Да, покрыто тестом `reg_sz_bytes_end_with_a_utf16_nul`.
- Отсутствующее значение — пусто, а не ошибка. Да, `ERROR_FILE_NOT_FOUND → Ok(None)` на обоих вызовах.
- Оба `InternetSetOptionW` на месте. Да.
- Исходные настройки машины вернулись точно. Да, подтверждено `-ceq`-сравнением со снимком.
- UAC не появляется: всё в `HKCU`, `InternetSetOptionW` без дескриптора сессии прав не требует.

## Оговорки

- Восстановление исходных значений на шаге 4 делалось нашим же `apply()` — это одновременно и проверка кода, и восстановление. Независимая сверка (шаг 5) сделана PowerShell'ом и подтверждает точное совпадение, так что общей ошибки с кодом у неё нет.
- `read()`/`apply()` пока никем не вызываются, кроме тестов: сохранение прежнего значения ДО записи и восстановление при старте — предмет следующей задачи (спека 6.3). До тех пор гарантии «процесс упал — сеть вернулась» ещё нет.

---

# Раунд правок по ревью

## FINDING 1 (Important) — порядок записи в `apply`

Было: `ProxyEnable`, потом `ProxyServer`, потом `ProxyOverride`. Обрыв на второй записи оставлял прокси **включённым** и направленным по старому (или отсутствующему) адресу, причём оба `InternetSetOptionW` пропускались — ровно та потеря сети, ради предотвращения которой модуль и написан, достижимая обычным путём ошибки, а не падением.

Стало: сначала `ProxyServer`, затем `ProxyOverride`, `ProxyEnable` — последним. Частичный отказ теперь оставляет выключатель в положении «выкл» — безопасная сторона. Порядок закреплён комментарием, объясняющим, что он не случаен.

## FINDING 2 (Minor) — уведомления и различимость «не записано» / «записано, но не разослано»

`SETTINGS_CHANGED` и `REFRESH` теперь вызываются оба, независимо от исхода первого; наружу отдаётся первая по порядку ошибка. В doc-комментарии `apply` явно сказано: `Err`, вернувшийся после записи, означает, что реестр **изменён**, и вызывающий обязан отличать это от «ничего не записано» — следующая задача будет решать по этому, откатывать ли сохранённый оригинал.

## FINDING 3 (Minor) — дублирование `<local>`

`<local>` дописывается только если его ещё нет в списке. Реальный `ProxyOverride` этой машины им заканчивается, так что после появления restore-обвязки «…;`<local>`;`<local>`» был бы одним неосторожным вызовом. Тест: `bypass_string_does_not_duplicate_an_existing_local_token`.

## FINDING 4 (Minor) — запись `"."` давала `"*."`

Запись, у которой после снятия точки остаётся пустой суффикс, пропускается. Тест: `bypass_string_skips_a_bare_dot` (сверяет строку целиком: `"localhost;<local>"`).

## FINDING 5 (Minor) — лишние права

`apply` открывает ключ с `KEY_WRITE`, без `KEY_READ`: читать ему нечего. `read` по-прежнему с `KEY_READ`.

## Отложенное (зафиксировано, не чинится)

`read` принимает `REG_EXPAND_SZ`, `apply` всегда пишет `REG_SZ`, поэтому восстановление значения, изначально бывшего `REG_EXPAND_SZ`, молча сменит его тип. Пронос типа расплескался бы на форму, которую потребляет UI, а вероятность для этих двух значений исчезающе мала. Doc-комментарий `apply` теперь честно это называет и нигде не обещает побайтового восстановления.

## Проверки после правок

Живой реестр повторно не трогался: правки — перестановка записей и чистая функция, машина уже проверена и восстановлена в первом раунде.

```
$ cd win && cargo fmt --all --check
(вывод пуст — нарушений нет)
```

```
$ cd win && cargo test --all
running 51 tests
test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
running 45 tests
test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
running 16 tests
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Полный вывод по крейту winnet (16 тестов, было 14):

```
     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-4f04aa23a4b32fba.exe)

running 16 tests
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test networks::tests::category_maps_every_documented_value ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

```
$ cd win && cargo clippy --all-targets -- -D warnings
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.45s
```

Итого 114 тестов (105 до задачи, 112 после первого коммита, 114 после правок). Ни одного `#[allow]`.


---

> **Примечание при публикации.** Файл `proxy-settings-before-task6.txt`, на который
> ссылается этот отчёт, был снимком реальных настроек прокси рабочей машины и в
> публичный репозиторий не попал. Внутренние адреса и имена хостов по всему
> репозиторию заменены на документационные (RFC 5737: `203.0.113.0/24`,
> `198.51.100.0/24`) и `example.internal`.
