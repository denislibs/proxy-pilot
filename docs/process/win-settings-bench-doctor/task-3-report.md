# Task 3 report — Сервер настроек на loopback

**Status:** DONE_WITH_CONCERNS (одна осознанная правка за пределами списка файлов брифа — см. «Отклонения»). Круг правок по ревью — в конце файла.

**Commit:** `11db1b4` — feat(win): сервер настроек на loopback — транспорт
**Base:** `0b761c6`, ветка `feat/windows-rust`

## Файлы

- Создан: `win/crates/app/src/websrv.rs` — транспорт целиком (~590 строк кода + 21 тест)
- Изменён: `win/crates/app/src/main.rs` — `mod websrv;`, `open_settings`, владение сервером внутри цикла сообщений
- Изменён: `win/crates/app/src/tray.rs` — `Action::OpenSettings` и пункт меню «Настройки…»
- Изменён: `win/crates/app/src/ui.rs` — `open_in_browser` (`ShellExecuteW`)
- Изменён: `win/crates/app/Cargo.toml` — две фичи крейта `windows`: `Win32_Security_Cryptography`, `Win32_UI_Shell`. **Новых зависимостей нет.**

## Интерфейс

```rust
pub struct SettingsUrl { pub url: String }
pub struct SettingsState { pub app: Arc<ArcSwap<AppState>> }

impl Server {
    pub async fn start(state: Arc<SettingsState>) -> Result<Server, SettingsError>;
    pub async fn start_with_idle(state: Arc<SettingsState>, idle: Duration) -> Result<Server, SettingsError>;
    pub fn url(&self) -> &SettingsUrl;
    pub fn is_running(&self) -> bool;
    pub fn stop(&self);
}
impl Drop for Server { /* stop() */ }
```

Бриф записывал это как `Server::start(shared_state) -> Result<SettingsUrl, Error>`. Такая сигнатура не даёт вызывающему ничего, на чём можно позвать `stop()`, поэтому `start` возвращает сам сервер, а адрес отдаётся через `url()`. `is_running` добавлен потому, что сервер гаснет сам: без него `main` открывал бы браузер по адресу уже закрытой двери.

`start_with_idle` существует, чтобы таймаут бездействия проверялся тестом за 200 мс, а не за четверть часа; `start` — обёртка над ним с боевой константой.

## Как выполнены требования безопасности

| Требование | Как сделано | Тест |
|---|---|---|
| Строго `127.0.0.1`, порт 0 | `SocketAddr::from((Ipv4Addr::LOCALHOST, 0))` | `the_listener_is_on_loopback` |
| Токен из системного ГСЧ | `BCryptGenRandom` + `BCRYPT_USE_SYSTEM_PREFERRED_RNG`, 32 байта → 64 hex | `every_session_gets_its_own_token` |
| Сравнение в постоянное время | `constant_time_eq` — фолд XOR по всей длине + `std::hint::black_box` | `the_token_comparison_is_length_and_content_sensitive` |
| Без токена — `404`, не `403` | `not_found` на всех путях до `touch()`, тело без единого упоминания продукта | `a_request_without_the_token_is_not_found`, `a_wrong_token_is_not_found`, `a_truncated_token_is_not_found`, `an_unknown_path_under_a_valid_token_is_not_found` |
| Живёт, пока открыто окно | владение `Option<Server>` внутри `message_loop`; `stop()` и `Drop` | `stopping_closes_the_door`, `dropping_the_handle_closes_the_door` |
| Таймаут бездействия 15 мин | `IDLE_TIMEOUT`, `sleep_until(last_seen + idle)` в цикле приёма | `the_server_stops_after_the_idle_timeout`, `activity_postpones_the_idle_timeout`, `a_request_without_a_token_does_not_postpone_the_timeout` |
| `Origin`/`Referer` у запросов, меняющих состояние | `origin_is_ours`, отказ `403` | `a_state_changing_request_from_a_foreign_origin_is_rejected`, `a_state_changing_request_without_any_origin_is_rejected`, `our_own_page_may_post`, `a_referer_from_our_own_page_is_accepted_when_origin_is_missing` |
| Никаких файлов с диска | страница — `format!` над константой в бинаре, ни одного `std::fs` | — |

### Токен: чем генерируется и как сравнивается

Генерируется `BCryptGenRandom(BCRYPT_ALG_HANDLE::default(), &mut [u8; 32], BCRYPT_USE_SYSTEM_PREFERRED_RNG)` — документированный способ взять системный ГСЧ Windows; отказ ГСЧ означает отказ запуска сервера, потому что дверь без токена хуже отсутствующей двери. Хекс-кодирование ручное, по таблице.

Сравнивается так:

```rust
fn constant_time_eq(expected: &[u8], given: &[u8]) -> bool {
    if expected.len() != given.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(given.iter()) {
        diff |= a ^ b;
    }
    std::hint::black_box(diff) == 0
}
```

**Почему обычного `==` мало.** Сравнение срезов и строк в Rust сводится к `memcmp`, а тот возвращается на ПЕРВОМ различающемся байте: время ответа растёт вместе с длиной совпавшей приставки. Это превращает перебор из «16^64 вариантов» в «64 позиции по 16 вариантов» — по байту за раз, замеряя ответы. По петле сигнал слабый, но измеримый (там как раз почти нет сетевого шума), а стоит его отсутствие дюжины строк.

Ранний выход по длине оставлен сознательно: длина токена фиксирована и одинакова всегда, секрета в ней нет. Секрет — содержимое, и оно проходится целиком. `black_box` не даёт оптимизатору свернуть цикл обратно в `memcmp` с ранним возвратом — без него весь этот код мог бы ничего не значить.

### Сверх брифа: проверка `Host` и заголовки ответа

Добавлены две вещи, которых бриф не называл, но которые защищают ровно то, что он просил защитить:

- **`Host` обязан быть `127.0.0.1:<порт>`.** Перепривязка DNS — чужое имя резолвится в `127.0.0.1`, и политика одного источника перестаёт мешать чужой странице читать наши ответы. Токена она по-прежнему не знает, так что это второй рубеж, а не первый; стоит одну строку. Тест `a_foreign_host_header_is_not_found`.
- **`Referrer-Policy: no-referrer`, `Content-Security-Policy: … form-action 'self'`, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`.** Токен лежит В АДРЕСЕ, поэтому первая же внешняя ссылка или форма на странице (задача 4 будет выводить туда пользовательские значения) унесла бы его в `Referer` на чужой сервер. Тест `the_right_token_serves_the_page` проверяет наличие `Referrer-Policy`.

### Что модульный комментарий говорит честно

В `websrv.rs` есть раздел «От чего это защищает, а от чего нет». Смысл: слушатель на loopback доступен любому процессу пользователя, и токен этого не меняет и не может изменить — такой процесс и так читает `config.toml`, и так подменяет сам бинарь, а при желании читает наш адрес из таблицы соединений и подсматривает токен в нашей же памяти. Токен защищает от захода по угаданному адресу, от страницы, открытой у пользователя в браузере, и (вместе с проверкой `Host`) от перепривязки DNS.

Там же оговорено, что «одноразовый» здесь значит «один на сеанс окна», а не «сгорает на первом запросе»: одна страница — это уже несколько запросов (сама страница, отправка формы, перезагрузка), и токен, сгорающий на первом, сломал бы то, ради чего заведён. Тест `a_token_from_a_previous_session_is_not_found` показывает, что после остановки старый токен не подходит нигде.

### Таймер бездействия сбрасывается только верным токеном

`Inner::touch()` вызывается ПОСЛЕ проверки токена и до маршрутизации. Иначе любой процесс, токена не знающий, держал бы дверь в настройки открытой сколько угодно, просто стуча в неё. Это проверено тестом `a_request_without_a_token_does_not_postpone_the_timeout`: 900 мс стука с шагом 100 мс при таймауте 300 мс — сервер всё равно гаснет.

Гонка «обращение пришло, пока цикл спал на старом сроке» разрешена перепроверкой `last_seen().elapsed() < idle` в ветке таймера: срок пересчитывается, а не срабатывает.

## Отклонения от списка файлов брифа

Бриф называл `websrv.rs` и `main.rs`. Пришлось тронуть ещё `tray.rs` (одна строка меню плюс один вариант `Action`) и `ui.rs` (`open_in_browser`).

**Почему.** Без вызывающего весь модуль — мёртвый код, а CI гоняет `clippy -D warnings`: сборка не проходит, и `#[allow]` запрещён глобальными ограничениями. Что важнее, задачу нельзя проверить руками: сервер, который некому открыть, не проверяется вовсе. Правки строго ДОПИСЫВАЮЩИЕ — задача 5 добавит «Замерить скорость…» и «Диагностика…» рядом, ничего не переписывая, и переиспользует `ui::open_in_browser`.

Порядок старта, пути выхода, `RestoreOnDrop`, `BRIDGE_STOPPED`, оконная процедура завершения сеанса — не тронуты. Слушатель моста не тронут: сервер настроек — отдельный сокет со своей жизнью. `Router::get()` по-прежнему имеет одну не-тестовую точку вызова. UAC не требуется: loopback и ничего в `HKLM`.

## Что осталось задаче 4

`placeholder_page` и `placeholder_form_reply` — заглушки, обе читают `SettingsState` (порт моста), чтобы было видно, что состояние доходит. Маршрутизация уже разводит `GET` (страница) и `POST` (форма, с прочитанным телом и проверенным источником); задаче 4 остаётся заменить два тела функций и расширить `SettingsState`.

## TDD: RED

Модуль создан сразу с тестами и без реализации, `mod websrv;` дописан в `main.rs`, чтобы падение дошло до ошибок типов, а не остановилось на «file not found for module». Полный вывод `cargo test --all` до реализации:

```
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
error[E0425]: cannot find type `SettingsState` in this scope
  --> crates\app\src\websrv.rs:16:23
   |
16 |     fn state() -> Arc<SettingsState> {
   |                       ^^^^^^^^^^^^^ not found in this scope
   |
help: you might be missing a type parameter
   |
16 |     fn state<SettingsState>() -> Arc<SettingsState> {
   |             +++++++++++++++

error[E0422]: cannot find struct, variant or union type `SettingsState` in this scope
  --> crates\app\src\websrv.rs:17:18
   |
17 |         Arc::new(SettingsState {
   |                  ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find type `Server` in this scope
  --> crates\app\src\websrv.rs:43:39
   |
43 |     async fn open(idle: Duration) -> (Server, String, String) {
   |                                       ^^^^^^ not found in this scope

warning: unused import: `super::*`
 --> crates\app\src\websrv.rs:3:9
  |
3 |     use super::*;
  |         ^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

error[E0433]: cannot find type `Server` in this scope
  --> crates\app\src\websrv.rs:44:22
   |
44 |         let server = Server::start_with_idle(state(), idle).await.unwrap();
   |                      ^^^^^^ use of undeclared type `Server`

error[E0277]: the size for values of type `str` cannot be known at compilation time
  --> crates\app\src\websrv.rs:99:23
   |
99 |         let (_server, authority, _token) = open(LONG_IDLE).await;
   |                       ^^^^^^^^^ doesn't have a size known at compile-time
   |
   = help: the trait `Sized` is not implemented for `str`
   = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:112:23
    |
112 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:124:23
    |
124 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:131:23
    |
131 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:145:23
    |
145 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:152:23
    |
152 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:159:23
    |
159 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:171:23
    |
171 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:178:23
    |
178 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:179:36
    |
179 |         let ours = format!("http://{authority}");
    |                                    ^^^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:188:23
    |
188 |         let (_server, authority, token) = open(LONG_IDLE).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:192:81
    |
192 | ...   "POST /{token} HTTP/1.1\r\nHost: {authority}\r\nReferer: http://{authority}/{token}\r\nContent-Length: 0\r\nConnection: close...
    |                                                                       ^^^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:239:22
    |
239 |         let (server, authority, token) = open(LONG_IDLE).await;
    |                      ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:243:21
    |
243 |         let (_next, next_authority, next_token) = open(LONG_IDLE).await;
    |                     ^^^^^^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:251:22
    |
251 |         let (server, authority, _token) = open(LONG_IDLE).await;
    |                      ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:264:22
    |
264 |         let (server, authority, _token) = open(LONG_IDLE).await;
    |                      ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:274:22
    |
274 |         let (server, authority, _token) = open(Duration::from_millis(200)).await;
    |                      ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:287:23
    |
287 |         let (_server, authority, token) = open(idle).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0277]: the size for values of type `str` cannot be known at compilation time
   --> crates\app\src\websrv.rs:306:23
    |
306 |         let (_server, authority, _token) = open(idle).await;
    |                       ^^^^^^^^^ doesn't have a size known at compile-time
    |
    = help: the trait `Sized` is not implemented for `str`
    = note: all local variables must have a statically known size

error[E0425]: cannot find function `constant_time_eq` in this scope
   --> crates\app\src\websrv.rs:320:17
    |
320 |         assert!(constant_time_eq(b"abc", b"abc"));
    |                 ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `constant_time_eq` in this scope
   --> crates\app\src\websrv.rs:321:18
    |
321 |         assert!(!constant_time_eq(b"abc", b"abd"));
    |                  ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `constant_time_eq` in this scope
   --> crates\app\src\websrv.rs:322:18
    |
322 |         assert!(!constant_time_eq(b"abc", b"ab"));
    |                  ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `constant_time_eq` in this scope
   --> crates\app\src\websrv.rs:323:18
    |
323 |         assert!(!constant_time_eq(b"abc", b"abcd"));
    |                  ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `constant_time_eq` in this scope
   --> crates\app\src\websrv.rs:324:18
    |
324 |         assert!(!constant_time_eq(b"abc", b""));
    |                  ^^^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find function `constant_time_eq` in this scope
   --> crates\app\src\websrv.rs:325:17
    |
325 |         assert!(constant_time_eq(b"", b""));
    |                 ^^^^^^^^^^^^^^^^ not found in this scope

Some errors have detailed explanations: E0277, E0422, E0425, E0433.
For more information about an error, try `rustc --explain E0277`.
warning: `proxypilot-app` (bin "proxypilot" test) generated 1 warning
error: could not compile `proxypilot-app` (bin "proxypilot" test) due to 29 previous errors; 1 warning emitted
```

## GREEN: `cargo test --all`

```
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.88s
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 68 tests
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::an_ordinary_relaunch_trips_neither_bridge_check ... ok
test doctor::tests::a_sysproxy_read_failure_is_reported_once_not_as_two_failures ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::no_recognised_network_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::no_stale_pointer_when_the_registry_points_elsewhere ... ok
test doctor::tests::seven_rows_come_back_every_time ... ok
test doctor::tests::sysproxy_check_is_skipped_gracefully_when_management_is_off ... ok
test doctor::tests::sysproxy_pointing_at_us_is_ok ... ok
test doctor::tests::sysproxy_pointing_elsewhere_is_a_warning_when_we_manage_it ... ok
test doctor::tests::the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine ... ok
test doctor::tests::the_office_networks_check_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::upstreams_check_is_ok_when_nothing_is_configured ... ok
test icons::tests::a_deliberate_direct_mode_is_not_unconfigured ... ok
test icons::tests::icon_reflects_the_active_route ... ok
test icons::tests::nothing_configured_gets_its_own_icon ... ok
test proxy::tests::a_disabled_pointer_at_our_address_is_not_stale ... ok
test proxy::tests::a_pointer_at_us_is_recognised_even_with_the_switch_off ... ok
test proxy::tests::localhost_by_name_is_ours_as_well ... ok
test proxy::tests::our_address_on_another_port_is_not_ours ... ok
test icons::tests::every_icon_is_a_full_rgba_buffer ... ok
test proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected ... ok
test icons::tests::icons_differ_from_each_other ... ok
test proxy::tests::the_per_protocol_form_is_recognised_too ... ok
test proxy::tests::the_real_corporate_setting_of_this_machine_is_left_alone ... ok
test tests::the_periodic_reevaluation_is_slower_than_the_probe_cache ... ok
test tests::the_window_messages_do_not_collide ... ok
test tray::tests::a_mode_that_is_merely_unconfigured_says_so ... ok
test tray::tests::a_nameless_network_falls_back_to_its_guid ... ok
test tray::tests::a_network_outside_the_office_is_not_marked_as_one ... ok
test tray::tests::header_explains_a_demotion_rather_than_hiding_it ... ok
test tray::tests::header_names_the_bridge_and_the_route ... ok
test tray::tests::header_names_the_upstream_it_actually_uses ... ok
test tray::tests::the_bridge_address_is_always_loopback ... ok
test tray::tests::the_network_line_shows_the_name_and_marks_the_office ... ok
test tray::tests::without_any_network_the_line_says_so ... ok
test tray::tests::wm_endsession_only_means_the_session_is_ending_when_wparam_is_true ... ok
test websrv::tests::the_listener_is_on_loopback ... ok
test websrv::tests::every_session_gets_its_own_token ... ok
test websrv::tests::an_unknown_path_under_a_valid_token_is_not_found ... ok
test websrv::tests::our_own_page_may_post ... ok
test websrv::tests::a_state_changing_request_from_a_foreign_origin_is_rejected ... ok
test websrv::tests::a_wrong_token_is_not_found ... ok
test websrv::tests::the_query_string_does_not_hide_the_token ... ok
test websrv::tests::a_state_changing_request_without_any_origin_is_rejected ... ok
test websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing ... ok
test websrv::tests::a_truncated_token_is_not_found ... ok
test websrv::tests::a_foreign_host_header_is_not_found ... ok
test websrv::tests::the_token_comparison_is_length_and_content_sensitive ... ok
test websrv::tests::a_request_without_the_token_is_not_found ... ok
test websrv::tests::the_right_token_serves_the_page ... ok
test websrv::tests::stopping_closes_the_door ... ok
test websrv::tests::dropping_the_handle_closes_the_door ... ok
test websrv::tests::a_token_from_a_previous_session_is_not_found ... ok
test websrv::tests::the_server_stops_after_the_idle_timeout ... ok
test websrv::tests::activity_postpones_the_idle_timeout ... ok
test websrv::tests::a_request_without_a_token_does_not_postpone_the_timeout ... ok

test result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 15.12s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 69 tests
test bench::tests::a_failed_measurement_has_no_speed ... ok
test bench::tests::a_zero_duration_does_not_divide_by_zero ... ok
test bench::tests::fastest_ignores_failures ... ok
test bench::tests::fastest_of_nothing_is_nothing ... ok
test bench::tests::speed_is_bytes_over_seconds ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::parses_connect ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok
test http::tests::truncated_input_is_an_error ... ok
test log::tests::log_file_name_is_stable ... ok
test probe::tests::an_unconfigured_upstream_is_unknown_not_down ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::is_shareable_across_threads ... ok
test router::tests::set_if_changed_publishes_a_different_value ... ok
test router::tests::set_if_changed_skips_a_matching_value ... ok
test router::tests::set_if_changed_reports_exactly_one_winner_under_concurrent_writers ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test bench::tests::reported_bytes_are_the_body_not_the_headers ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test socks5::tests::surfaces_refusal_code ... ok
test supervisor::tests::in_the_office_with_a_live_socks_the_route_becomes_socks ... ok
test supervisor::tests::outside_the_office_the_route_is_direct_even_with_a_live_upstream ... ok
test probe::tests::a_silent_address_is_down_within_the_timeout ... ok
test supervisor::tests::a_dead_pinned_upstream_is_reported_as_demoted ... ok
test supervisor::tests::the_network_name_reaches_the_app_state ... ok
test bench::tests::an_unconfigured_upstream_is_not_measured ... ok
test probe::tests::a_changed_address_is_not_answered_from_the_old_cache ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test bench::tests::every_configured_route_is_measured_and_labelled ... ok
test probe::tests::the_result_is_cached_within_the_ttl ... ok
test bench::tests::a_dead_upstream_yields_an_error_not_a_hang ... ok
test supervisor::tests::run_reevaluates_on_start_and_on_each_event_then_exits_when_the_channel_closes ... ok
test probe::tests::a_live_listener_is_up_and_a_closed_port_is_down ... ok
test supervisor::tests::an_unchanged_decision_does_not_touch_the_router ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)

running 48 tests
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test config::tests::load_from_a_missing_file_yields_defaults ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test config::tests::defaults_match_the_spec ... ok
test config::tests::no_network_at_all_is_not_office ... ok
test config::tests::managing_the_system_proxy_is_on_by_default_and_switchable ... ok
test config::tests::matching_is_case_insensitive ... ok
test config::tests::place_is_not_office_for_an_unknown_network ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test config::tests::place_is_office_when_a_connected_network_matches ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::several_connected_networks_office_wins ... ok
test config::tests::the_name_never_decides_anything ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::validate_accepts_the_defaults ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::the_saved_system_proxy_survives_a_roundtrip ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test config::tests::validate_rejects_a_port_below_the_privileged_range ... ok
test config::tests::validate_rejects_a_zero_connection_limit ... ok
test config::tests::validate_rejects_an_absurd_connection_limit ... ok
test config::tests::without_configured_offices_nothing_is_office ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test config::tests::validate_rejects_an_office_network_with_empty_id ... ok
test config::tests::validate_rejects_a_malformed_upstream ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test config::tests::config_path_matches_what_the_spec_promises ... ok
test config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ... ok
test config::tests::save_then_load_roundtrips_through_a_real_file ... ok

test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-b921d6d1fd7e845d.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test networks::tests::category_maps_every_documented_value ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test events::tests::the_log_line_names_every_combination_of_armed_channels ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test events::tests::dropping_the_debounced_receiver_releases_the_source ... ok
test events::tests::a_burst_collapses_to_its_first_and_last_event ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test events::tests::the_trailing_event_is_the_last_one_of_the_burst ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok
test events::tests::events_further_apart_than_the_window_both_pass ... ok

test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s

   Doc-tests proxypilot_bridge

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_winnet

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## `cargo clippy --all-targets -- -D warnings` и `cargo fmt --all --check`

```
$ cargo clippy --all-targets -- -D warnings
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.06s

$ cargo fmt --all --check
(вывод пуст)
```

## Самопроверка

- **Токен из настоящего источника случайности?** Да — `BCryptGenRandom` с системным ГСЧ, 32 байта. Не `SystemTime`, не счётчик, не адрес объекта.
- **Сравнение действительно в постоянное время?** Да — фолд по всей длине без ранних выходов, `black_box` против сворачивания обратно в `memcmp`. Ранний выход только по длине, которая не секрет.
- **Может ли запрос без токена узнать, что сервер существует?** Он получает `404` с телом `Not Found` и без упоминания продукта — ровно то, что отдал бы любой чужой сервер. Отдельный тест это стережёт. Сам факт открытого TCP-порта, разумеется, виден — это свойство любого слушателя, а не этой реализации.
- **Таймер сбрасывается на активности и сервер действительно останавливается?** Да, три теста: продление, гашение, и что стук без токена не продлевает.
- **Лишние зависимости?** Ни одной новой в дереве. Две фичи уже присутствующего крейта `windows`. HTTP разбирается `proxypilot_bridge::http::read_head`, как и требовалось.
- **Модульный комментарий честен?** Да, отдельным разделом, и он прямо говорит, что токен НЕ защищает от локального процесса с правами пользователя.

## Оговорки

1. **`tray.rs`/`ui.rs` за пределами списка файлов** — обоснование выше. Если ревью сочтёт это вторжением в задачу 5, откат сводится к трём правкам, но тогда задача 3 не проходит CI без вызывающего.
2. **Уже принятое соединение доигрывается после остановки.** Слушатель закрыт, новых соединений нет, но задача, которая уже пишет ответ, его допишет. Рвать ответ на полуслове ради миллисекунды закрытия смысла нет; в докблоке `Server` это сказано.
3. **Ручной проверки в браузере не проводилось** — окружение без интерактивного сеанса. Тесты бьют по настоящему слушателю настоящими HTTP-запросами (включая `Origin`, `Referer`, `Host`, строку запроса и тело формы), но «браузер открылся по нажатию пункта меню» проверено не было. Это стоит сделать вместе с ручной проверкой задачи 4, которую план и так требует.

---

# Fix round 1 — по итогам ревью

**Commit:** `0ea1e1c` — fix(win): страница настроек не отвергала собственную форму (правки только в `win/crates/app/src/websrv.rs`)

Все четыре Important приняты и исправлены; принята и вся мелочь.

## Finding 1 — `Referrer-Policy: no-referrer` ломал собственную форму страницы

Принято, и это была настоящая ошибка: заголовок, поставленный ради защиты токена, отменял бы отправку формы. По Fetch («append a request Origin header») запрос не-GET/HEAD при политике `no-referrer` уходит с `Origin: null`, а `Referer` при той же политике не уходит вообще. То есть `<form method="post" action="">` нашей же страницы приходил бы к нам без единого признака происхождения, `origin_is_ours` сравнивал бы `"null"` с `http://127.0.0.1:{port}` и честно отдавал бы `403` на нажатие кнопки «Сохранить».

Лечить это приёмом `Origin: null` нельзя, и в коде теперь стоит комментарий, почему: `null` шлют непрозрачные источники — песочница в iframe, страница из `data:`, — ровно те, от кого проверка и заведена.

Политика теперь `Referrer-Policy: same-origin`. Своему источнику браузер шлёт и настоящий `Origin`, и настоящий `Referer` (с токеном — он и раньше так работал в тесте), при переходе на чужой источник `Referer` не шлёт вовсе. Токен по-прежнему не покидает 127.0.0.1, а форма работает.

Тесты приведены к тому, что браузер действительно пришлёт, а не к тому, что заставляет утверждение сойтись:

- `our_own_page_may_post` — комментарий объясняет, что `Origin` здесь настоящий именно потому, что новая политика его разрешает, и что взаимодействие `no-referrer`/`null` и есть причина выбора `same-origin`;
- `a_referer_from_our_own_page_is_accepted_when_origin_is_missing` — комментарий отмечает, что `same-origin` этот заголовок своему источнику шлёт, так что тест его не выдумывает;
- `the_right_token_serves_the_page` теперь проверяет `Referrer-Policy: same-origin`, а не `no-referrer`;
- **новый** `an_opaque_origin_is_rejected` — `Origin: null` получает `403`. Он стоит стражем именно против «починки», от которой ревью предостерегло.

## Finding 2 — сырой `Referer` в логе

Принято. Модульный комментарий обещает, что токен не попадает в лог, потому что лог живёт на диске дольше сеанса. `Origin` — это схема и авторитет, пути в нём нет, и его можно писать целиком. `Referer` несёт путь, то есть у нашей же страницы — токен; после правки Finding 1 такие заголовки стали реальными, и строка писала бы токен на диск.

Теперь логируется `has_referer = head.header("Referer").is_some()` — булев признак, а рядом комментарий, объясняющий разницу между двумя заголовками.

## Finding 3 — тест сброса таймера был инертен и стоил 15 секунд

Принято, диагноз точен. `knock` слал голые `\n`; `read_head` ищет `\r\n\r\n`, не находил, ждал закрытия сокета, возвращал `Truncated`, а `serve_one` на `Truncated` не отвечает — значит стук не доходил до проверки токена. `read_to_end` клиента висел до `REQUEST_TIMEOUT`, цикл `while elapsed < 900ms` проворачивался один раз, и единственное уцелевшее утверждение дублировало соседний тест.

Исправлено:

- концы строк `\r\n`, как у `raw`/`get`/`post`, с комментарием, объясняющим цену ошибки;
- чтение — одно `read` со сроком, а не `read_to_end`: сервер после ответа ещё до 100 мс дочитывает запрос, чтобы Windows послала FIN, а не RST (см. `write_all`), и ждать этого на каждом витке значило бы стучать вдвое реже;
- цикл стучит, **пока сервер жив**, и считает витки: `assert!(knocks >= 3)`. Стучать в закрытую дверь и дорого, и незачем — подключение к уже закрытому порту Windows на этой машине отвергает примерно через две секунды (измерено: 775 мкс по живому серверу против 2.05 с по мёртвому);
- перед циклом отдельно проверяется, что **один** стук по заведомо живому серверу укладывается быстрее таймаута. Это и есть страж против повторения находки: счётчик витков сам по себе устойчивым стражем не был бы, потому что витки считаются по настенным часам, а те самые две секунды их съедают;
- добавлен помощник `stopped_within(&Server, …)`, ждущий `is_running() == false` вместо стука в порт. Тесты, где проверяется ТАЙМЕР, пользуются им; что порт действительно закрывается, по-прежнему доказывают `stopping_closes_the_door` и `dropping_the_handle_closes_the_door` через `closed_within` — и `the_server_stops_after_the_idle_timeout` проверяет теперь оба факта.

**Время двоичного файла тестов приложения: 15.13 с → 2.59 с** (мост для сравнения — 2.05 с). Тестов при этом стало больше: 68 → 70.

## Finding 4 — не был ограничен только счёт одновременных соединений

Принято. Семафор `MAX_CONNECTIONS = 32` вокруг `tokio::spawn`, как у моста (`serve::Limits`). Отличие от моста намеренное и прокомментировано: мост сверх предела отвечает `503`, а здесь сокет закрывается **молча** — ответ подтвердил бы существование сервера тому, кто токена не предъявлял, то есть отменял бы смысл `404`.

Новый тест `the_number_of_simultaneous_connections_is_capped`: 32 молчащих сокета занимают все места, следующий запрос с ВЕРНЫМ токеном не получает ничего, а после освобождения мест страница снова отдаётся. Чтение в нём терпимое: закрытие сокета с непрочитанным запросом Windows оформляет как RST, и это не ошибка теста, а тот самый отказ.

## Мелочи

- `frame-ancestors 'none'` добавлено в CSP; `the_right_token_serves_the_page` это проверяет.
- Раздел о честности в модульном комментарии дополнен: токен лежит в ПУТИ адреса, поэтому адрес целиком попадает в историю браузера, а оттуда, возможно, и в синхронизацию профиля с подсказками адресной строки. Сказано и то, что это смягчает: токен умирает вместе с сеансом окна, запись в истории остаётся, а ключ уже ни от чего не подходит — со ссылкой на тест, который это проверяет.
- Счёт тестов в отчёте исправлен: тестов `websrv` было **20**, а не 21; после этого круга их **22**.

## Не тронуто (по указанию ревью)

Первый-выигрывает при дублирующихся заголовках и переаллокация на каждое чтение в `read_body` — оба ограничены и недостижимы в модели угроз.

## Проверка

```
$ cargo test --all
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.99s
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 70 tests
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::a_sysproxy_read_failure_is_reported_once_not_as_two_failures ... ok
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free ... ok
test doctor::tests::an_ordinary_relaunch_trips_neither_bridge_check ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::no_recognised_network_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::no_stale_pointer_when_the_registry_points_elsewhere ... ok
test doctor::tests::seven_rows_come_back_every_time ... ok
test doctor::tests::sysproxy_check_is_skipped_gracefully_when_management_is_off ... ok
test doctor::tests::sysproxy_pointing_at_us_is_ok ... ok
test doctor::tests::sysproxy_pointing_elsewhere_is_a_warning_when_we_manage_it ... ok
test doctor::tests::the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine ... ok
test doctor::tests::the_office_networks_check_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::upstreams_check_is_ok_when_nothing_is_configured ... ok
test icons::tests::a_deliberate_direct_mode_is_not_unconfigured ... ok
test icons::tests::icon_reflects_the_active_route ... ok
test icons::tests::nothing_configured_gets_its_own_icon ... ok
test proxy::tests::a_disabled_pointer_at_our_address_is_not_stale ... ok
test icons::tests::every_icon_is_a_full_rgba_buffer ... ok
test proxy::tests::a_pointer_at_us_is_recognised_even_with_the_switch_off ... ok
test icons::tests::icons_differ_from_each_other ... ok
test proxy::tests::localhost_by_name_is_ours_as_well ... ok
test proxy::tests::our_address_on_another_port_is_not_ours ... ok
test proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected ... ok
test proxy::tests::the_per_protocol_form_is_recognised_too ... ok
test proxy::tests::the_real_corporate_setting_of_this_machine_is_left_alone ... ok
test tests::the_periodic_reevaluation_is_slower_than_the_probe_cache ... ok
test tests::the_window_messages_do_not_collide ... ok
test tray::tests::a_mode_that_is_merely_unconfigured_says_so ... ok
test tray::tests::a_nameless_network_falls_back_to_its_guid ... ok
test tray::tests::a_network_outside_the_office_is_not_marked_as_one ... ok
test tray::tests::header_explains_a_demotion_rather_than_hiding_it ... ok
test tray::tests::header_names_the_bridge_and_the_route ... ok
test tray::tests::header_names_the_upstream_it_actually_uses ... ok
test tray::tests::the_bridge_address_is_always_loopback ... ok
test tray::tests::the_network_line_shows_the_name_and_marks_the_office ... ok
test tray::tests::without_any_network_the_line_says_so ... ok
test tray::tests::wm_endsession_only_means_the_session_is_ending_when_wparam_is_true ... ok
test websrv::tests::a_wrong_token_is_not_found ... ok
test websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing ... ok
test websrv::tests::our_own_page_may_post ... ok
test websrv::tests::every_session_gets_its_own_token ... ok
test websrv::tests::an_unknown_path_under_a_valid_token_is_not_found ... ok
test websrv::tests::an_opaque_origin_is_rejected ... ok
test websrv::tests::a_state_changing_request_from_a_foreign_origin_is_rejected ... ok
test websrv::tests::a_foreign_host_header_is_not_found ... ok
test websrv::tests::a_state_changing_request_without_any_origin_is_rejected ... ok
test websrv::tests::a_truncated_token_is_not_found ... ok
test websrv::tests::a_request_without_the_token_is_not_found ... ok
test websrv::tests::the_listener_is_on_loopback ... ok
test websrv::tests::the_token_comparison_is_length_and_content_sensitive ... ok
test websrv::tests::the_query_string_does_not_hide_the_token ... ok
test websrv::tests::the_right_token_serves_the_page ... ok
test websrv::tests::a_token_from_a_previous_session_is_not_found ... ok
test websrv::tests::the_number_of_simultaneous_connections_is_capped ... ok
test websrv::tests::activity_postpones_the_idle_timeout ... ok
test websrv::tests::dropping_the_handle_closes_the_door ... ok
test websrv::tests::stopping_closes_the_door ... ok
test websrv::tests::the_server_stops_after_the_idle_timeout ... ok
test websrv::tests::a_request_without_a_token_does_not_postpone_the_timeout ... ok

test result: ok. 70 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.59s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 69 tests
test bench::tests::a_failed_measurement_has_no_speed ... ok
test bench::tests::fastest_ignores_failures ... ok
test bench::tests::speed_is_bytes_over_seconds ... ok
test bench::tests::fastest_of_nothing_is_nothing ... ok
test bench::tests::a_zero_duration_does_not_divide_by_zero ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::parses_connect ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::truncated_input_is_an_error ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok
test log::tests::log_file_name_is_stable ... ok
test probe::tests::an_unconfigured_upstream_is_unknown_not_down ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::is_shareable_across_threads ... ok
test router::tests::set_if_changed_publishes_a_different_value ... ok
test router::tests::set_if_changed_skips_a_matching_value ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test bench::tests::reported_bytes_are_the_body_not_the_headers ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::set_if_changed_reports_exactly_one_winner_under_concurrent_writers ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test socks5::tests::surfaces_refusal_code ... ok
test supervisor::tests::in_the_office_with_a_live_socks_the_route_becomes_socks ... ok
test supervisor::tests::outside_the_office_the_route_is_direct_even_with_a_live_upstream ... ok
test supervisor::tests::a_dead_pinned_upstream_is_reported_as_demoted ... ok
test supervisor::tests::the_network_name_reaches_the_app_state ... ok
test probe::tests::a_silent_address_is_down_within_the_timeout ... ok
test bench::tests::an_unconfigured_upstream_is_not_measured ... ok
test probe::tests::a_changed_address_is_not_answered_from_the_old_cache ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test bench::tests::every_configured_route_is_measured_and_labelled ... ok
test probe::tests::the_result_is_cached_within_the_ttl ... ok
test probe::tests::a_live_listener_is_up_and_a_closed_port_is_down ... ok
test supervisor::tests::an_unchanged_decision_does_not_touch_the_router ... ok
test bench::tests::a_dead_upstream_yields_an_error_not_a_hang ... ok
test supervisor::tests::run_reevaluates_on_start_and_on_each_event_then_exits_when_the_channel_closes ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-67e9de3f4fdbb341.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-f96b6e93f92cfebe.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-e8c89a5d89aa7499.exe)

running 48 tests
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::cidr_matches_addresses_inside ... ok
test config::tests::load_from_a_missing_file_yields_defaults ... ok
test config::tests::defaults_match_the_spec ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test config::tests::matching_is_case_insensitive ... ok
test config::tests::managing_the_system_proxy_is_on_by_default_and_switchable ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::no_network_at_all_is_not_office ... ok
test config::tests::place_is_not_office_for_an_unknown_network ... ok
test config::tests::place_is_office_when_a_connected_network_matches ... ok
test config::tests::validate_rejects_a_port_below_the_privileged_range ... ok
test config::tests::several_connected_networks_office_wins ... ok
test config::tests::the_name_never_decides_anything ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::validate_accepts_the_defaults ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test config::tests::the_saved_system_proxy_survives_a_roundtrip ... ok
test config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ... ok
test config::tests::validate_rejects_a_malformed_upstream ... ok
test config::tests::validate_rejects_a_zero_connection_limit ... ok
test config::tests::validate_rejects_an_absurd_connection_limit ... ok
test config::tests::validate_rejects_an_office_network_with_empty_id ... ok
test config::tests::without_configured_offices_nothing_is_office ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test config::tests::config_path_matches_what_the_spec_promises ... ok
test config::tests::save_then_load_roundtrips_through_a_real_file ... ok

test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-b921d6d1fd7e845d.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test events::tests::a_burst_collapses_to_its_first_and_last_event ... ok
test networks::tests::category_maps_every_documented_value ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test events::tests::the_trailing_event_is_the_last_one_of_the_burst ... ok
test events::tests::the_log_line_names_every_combination_of_armed_channels ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test events::tests::dropping_the_debounced_receiver_releases_the_source ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok
test events::tests::events_further_apart_than_the_window_both_pass ... ok

test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.13s

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
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.14s

$ cargo fmt --all --check
(вывод пуст)
```
