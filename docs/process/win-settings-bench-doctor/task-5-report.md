# Задача 5 плана 3 — меню, замер и диагностика в трее. Отчёт

**База:** `59118a9` (ветка `feat/windows-rust`)
**Тесты:** 238 проходят + 1 `#[ignore]` (было 236 + 1 — два новых теста в `tray.rs`).

**Примечание к брифу:** файла `task-5-brief.md` в каталоге плана нет (в отличие
от задач 1–3). Формулировка задачи взята из самого плана —
`docs/superpowers/plans/2026-08-30-proxypilot-win-settings-bench-doctor.md`,
раздел «Task 5: Меню, замер и диагностика в трее» (строки 240–247).

---

## Что сделано

Меню трея (`win/crates/app/src/tray.rs`) получило два новых пункта —
«Замерить скорость…» и «Диагностика…», вставленные между «Настройки…» и
«Копировать адрес моста» (порядок, заданный планом). Копирование адреса уже
было реализовано задачей 2 плана 2 (`Action::CopyAddress`, пункт «Копировать
адрес моста») — трогать было нечего, только проверить, что оно осталось на
месте.

Оба новых пункта не заводят второй сервер и не открывают вторую дверь: они
переиспользуют тот же `open_settings` (`win/crates/app/src/main.rs`), который
уже умеет поднимать сервер настроек при необходимости и подхватывать уже
работающий. Единственное отличие от «Настройки…» — фрагмент URL (`#bench` /
`#doctor`) поверх того же адреса с тем же токеном; фрагмент не уходит на
сервер ни в одном запросе браузера, поэтому не участвует ни в проверке
токена, ни в проверке `Origin`/`Referer`, ни в какой-либо маршрутизации.
Разделы `#bench` и `#doctor` уже существуют на странице настроек
(`win/crates/app/src/settings_page.rs`, `<h2 id="bench">` /
`<h2 id="doctor">`) — задача 4 их туда положила.

### Изменения по файлам

- `win/crates/app/src/tray.rs`
  - `Action`: добавлены варианты `OpenBench` и `OpenDoctor` (после
    `OpenSettings`, до `Quit`) — с докблоком, объясняющим, что это не второй
    вход в приложение, а якорь на той же странице.
  - `Tray`: добавлены поля `bench: MenuItem` и `doctor: MenuItem`; созданы в
    `Tray::new` и добавлены в `Menu` между `settings` и `copy`; переведены в
    поле структуры при возврате `Ok(Self { ... })`.
  - `action_for`: добавлены две проверки id (`bench`, `doctor`) — до перебора
    режимов, по аналогии с `settings`/`copy`/`quit`.
  - Ничего в `header_text`/`situation_text`/`network_text`/`mode_label`/
    `refresh`/`install_session_end_guard`/`session_end_wndproc` не менялось —
    строка понижения (демоции) режима формируется этими же функциями и
    осталась на месте (см. тест `header_explains_a_demotion_rather_than_hiding_it`,
    он не тронут и проходит).

- `win/crates/app/src/main.rs`
  - `open_settings` получила параметр `section: Option<&str>`. Если он
    задан, к уже полученному URL сервера дописывается `#{section}` перед
    вызовом `ui::open_in_browser`. Логика поднятия/переиспользования сервера
    (проверка `is_running`, `runtime.block_on(websrv::Server::start(...))`,
    обработка ошибок с `ui::error_box`) не изменена ни на строку.
  - В цикле обработки меню (`message_loop`) добавлены два новых плеча
    `match`: `Some(Action::OpenBench) => open_settings(..., Some("bench"))` и
    `Some(Action::OpenDoctor) => open_settings(..., Some("doctor"))`;
    существующее плечо `Some(Action::OpenSettings)` теперь передаёт `None`
    последним аргументом. Компилятор потребовал это сам — `match` без новых
    плеч не собирался (`E0004: non-exhaustive patterns`), это и был первый
    красный прогон после реализации мэппинга.

### Порядок пунктов меню после правки

Мост-заголовок → строка сети → разделитель → 4 пункта режимов с индикаторами
доступности → разделитель → «Настройки…» → «Замерить скорость…» →
«Диагностика…» → «Копировать адрес моста» → разделитель → «Выход». Ничего из
унаследованного от планов 1–2 не переписано — только вставка.

---

## TDD

### Красный прогон (до реализации)

Тесты `tray::tests::opening_the_speed_test_and_diagnostics_are_distinct_actions`
и `tray::tests::the_new_items_do_not_disturb_the_rest_of_the_menu` были
написаны первыми — они ссылаются на ещё не существующие поля `t.bench`/
`t.doctor` и варианты `Action::OpenBench`/`Action::OpenDoctor`. Полный,
дословный вывод:

```
$ cargo test -p proxypilot-app --bin proxypilot tray::

   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
error[E0609]: no field `bench` on type `tray::Tray`
   --> crates\app\src\tray.rs:627:28
    |
627 |             t.action_for(t.bench.id()),
    |                            ^^^^^ unknown field
    |
    = note: available fields are: `icon`, `header`, `network`, `modes`, `settings` ... and 4 others

error[E0599]: no variant, associated function, or constant named `OpenBench` found for enum `tray::Action` in the current scope
   --> crates\app\src\tray.rs:628:26
    |
 86 | pub enum Action {
    | --------------- variant, associated function, or constant `OpenBench` not found for this enum
...
628 |             Some(Action::OpenBench),
    |                          ^^^^^^^^^ variant, associated function, or constant not found in `tray::Action`

error[E0609]: no field `doctor` on type `tray::Tray`
   --> crates\app\src\tray.rs:632:28
    |
632 |             t.action_for(t.doctor.id()),
    |                            ^^^^^^ unknown field
    |
    = note: available fields are: `icon`, `header`, `network`, `modes`, `settings` ... and 4 others

error[E0599]: no variant, associated function, or constant named `OpenDoctor` found for enum `tray::Action` in the current scope
   --> crates\app\src\tray.rs:633:26
    |
 86 | pub enum Action {
    | --------------- variant, associated function, or constant `OpenDoctor` not found for this enum
...
633 |             Some(Action::OpenDoctor),
    |                          ^^^^^^^^^^ variant, associated function, or constant not found in `tray::Action`

error[E0609]: no field `bench` on type `tray::Tray`
   --> crates\app\src\tray.rs:639:22
    |
639 |         assert_ne!(t.bench.id(), t.doctor.id());
    |                      ^^^^^ unknown field
    |
    = note: available fields are: `icon`, `header`, `network`, `modes`, `settings` ... and 4 others

error[E0609]: no field `doctor` on type `tray::Tray`
   --> crates\app\src\tray.rs:639:36
    |
639 |         assert_ne!(t.bench.id(), t.doctor.id());
    |                                    ^^^^^^ unknown field
    |
    = note: available fields are: `icon`, `header`, `network`, `modes`, `settings` ... and 4 others

Some errors have detailed explanations: E0599, E0609.
For more information about an error, try `rustc --explain E0599`.
error: could not compile `proxypilot-app` (bin "proxypilot" test) due to 6 previous errors
```

Exit code: 101.

После добавления `Action`-вариантов, полей `Tray` и правки `action_for`, но
до правки `main.rs`, второй красный прогон (весь крейт, не только `tray::`)
показал ожидаемое несоответствие в другом файле:

```
$ cargo test -p proxypilot-app --bin proxypilot tray::

   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
error[E0004]: non-exhaustive patterns: `Some(tray::Action::OpenBench)` and `Some(tray::Action::OpenDoctor)` not covered
   --> crates\app\src\main.rs:795:19
    |
795 |             match tray.action_for(event.id()) {
    |                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^ patterns `Some(tray::Action::OpenBench)` and `Some(tray::Action::OpenDoctor)` not covered
    |
note: `Option<tray::Action>` defined here
   --> /rustc/88d9e12ae178fab0fb5cc050a94da85685d449ea/library\core\src\option.rs:598:0
   ::: /rustc/88d9e12ae178fab0fb5cc050a94da85685d449ea/library\core\src\option.rs:606:4
    |
    = note: not covered
    = note: the matched value is of type `Option<tray::Action>`
help: ensure that all possible cases are being handled by adding a match arm with a wildcard pattern, a match arm with multiple or-patterns as shown, or multiple match arms
    |
815 ~                 None => {},
816 +                 Some(tray::Action::OpenBench) | Some(tray::Action::OpenDoctor) => todo!()
    |

For more information about this error, try `rustc --explain E0004`.
error: could not compile `proxypilot-app` (bin "proxypilot" test) due to 1 previous error
```

Оба красных прогона — реальные, вставлены дословно из терминала, без
реконструкции по памяти.

### Зелёный прогон (после реализации)

```
$ cargo test -p proxypilot-app --bin proxypilot tray::

   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.29s
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 12 tests
test tray::tests::a_mode_that_is_merely_unconfigured_says_so ... ok
test tray::tests::header_names_the_bridge_and_the_route ... ok
test tray::tests::a_nameless_network_falls_back_to_its_guid ... ok
test tray::tests::header_names_the_upstream_it_actually_uses ... ok
test tray::tests::a_network_outside_the_office_is_not_marked_as_one ... ok
test tray::tests::the_bridge_address_is_always_loopback ... ok
test tray::tests::the_network_line_shows_the_name_and_marks_the_office ... ok
test tray::tests::header_explains_a_demotion_rather_than_hiding_it ... ok
test tray::tests::without_any_network_the_line_says_so ... ok
test tray::tests::wm_endsession_only_means_the_session_is_ending_when_wparam_is_true ... ok
test tray::tests::opening_the_speed_test_and_diagnostics_are_distinct_actions ... ok
test tray::tests::the_new_items_do_not_disturb_the_rest_of_the_menu ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 85 filtered out; finished in 0.03s
```

Про `header_explains_a_demotion_rather_than_hiding_it` в этом же прогоне —
это существующий тест, не новый; он приведён здесь как доказательство, что
строка понижения режима не пострадала от правки меню.

### Новые тесты — что именно и почему они не рисуют попап

`Tray::new` строит настоящее меню через `muda`/`tray-icon`
(`CreateMenu`/`AppendMenuW`/`Shell_NotifyIconW`) — это Win32-объекты, которые
не требуют интерактивного открытия попапа пользователем; попап появляется
только по щелчку живого человека, и вот это отрисовать в текущем окружении
нельзя (нет ни живого клика, ни визуальной проверки). Перед тем как положиться
на `Tray::new` в тестах, я проверил зондом, что она вообще успешно
отрабатывает в этом окружении (реальный рабочий стол Windows 11, не
контейнер без сессии) — зонд собирался и печатал `PROBE: Tray::new
SUCCEEDED`, после чего был удалён и заменён настоящими тестами ниже.

- `opening_the_speed_test_and_diagnostics_are_distinct_actions` — строит
  `Tray`, проверяет `action_for(bench.id()) == Some(OpenBench)`,
  `action_for(doctor.id()) == Some(OpenDoctor)` и что у пунктов разные id
  (ловит копипаст-баг с одинаковым `MenuId`).
- `the_new_items_do_not_disturb_the_rest_of_the_menu` — тот же `Tray`,
  проверяет, что `Quit`, `CopyAddress`, `OpenSettings` и все четыре режима
  по-прежнему маппятся правильно, плюс что случайный `MenuId` не совпадает
  ни с одним пунктом (`action_for` возвращает `None`, а не что-то по
  умолчанию).

Оба теста используют приватные поля `Tray` (`t.bench`, `t.quit`, `t.modes`
и т. д.) напрямую — `mod tests` вложен в тот же модуль `tray`, и Rust даёт
дочернему модулю доступ к приватным полям предка; отдельных публичных
геттеров ради теста заводить не пришлось.

**Что НЕ проверено и не может быть проверено в этом окружении:** сам клик по
пункту меню, появление всплывающего меню на экране, фактическое открытие
браузера и прокрутка к нужному разделу страницы. Это ручная проверка (список
ниже).

---

## Полный прогон трёх проверок CI (после реализации)

### `cargo test --all`

Хвост вывода (список тестов по каждому крейту — полностью, без сокращений):

```
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 97 tests
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::no_recognised_network_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::a_sysproxy_read_failure_is_reported_once_not_as_two_failures ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::an_ordinary_relaunch_trips_neither_bridge_check ... ok
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
test icons::tests::every_icon_is_a_full_rgba_buffer ... ok
test proxy::tests::a_disabled_pointer_at_our_address_is_not_stale ... ok
test proxy::tests::a_pointer_at_us_is_recognised_even_with_the_switch_off ... ok
test proxy::tests::localhost_by_name_is_ours_as_well ... ok
test icons::tests::icons_differ_from_each_other ... ok
test proxy::tests::our_address_on_another_port_is_not_ours ... ok
test proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected ... ok
test proxy::tests::the_per_protocol_form_is_recognised_too ... ok
test proxy::tests::the_real_corporate_setting_of_this_machine_is_left_alone ... ok
test settings_page::tests::a_form_is_parsed_with_percent_and_plus_decoding ... ok
test settings_page::tests::a_port_that_is_not_a_number_is_reported_not_swallowed ... ok
test settings_page::tests::a_failed_route_is_shown_as_failed_not_omitted ... ok
test settings_page::tests::a_privileged_port_is_rejected_by_config_validate ... ok
test settings_page::tests::an_invalid_upstream_is_rejected_by_config_validate ... ok
test settings_page::tests::diagnostics_output_is_shown_in_place_and_escaped ... ok
test settings_page::tests::empty_office_rows_are_dropped ... ok
test settings_page::tests::everything_rendered_into_the_page_is_escaped ... ok
test settings_page::tests::html_metacharacters_are_escaped ... ok
test settings_page::tests::repeated_fields_keep_their_order ... ok
test settings_page::tests::the_autostart_toggle_says_it_is_not_wired_yet_instead_of_pretending ... ok
test settings_page::tests::the_form_does_not_touch_the_fields_it_does_not_own ... ok
test settings_page::tests::the_live_config_keeps_the_port_the_bridge_is_bound_to ... ok
test settings_page::tests::the_page_offers_the_office_button_only_when_a_network_is_known ... ok
test settings_page::tests::the_page_says_the_port_needs_a_restart ... ok
test tests::a_port_change_does_not_reach_the_config_the_supervisor_gets ... ok
test settings_page::tests::the_page_shows_both_upstreams_with_their_availability ... ok
test tests::everything_except_the_port_does_reach_the_supervisor ... ok
test tests::switching_the_mode_does_not_smuggle_a_pending_port_change_through ... ok
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
test websrv::tests::a_foreign_host_header_is_not_found ... ok
test websrv::tests::a_request_without_the_token_is_not_found ... ok
test websrv::tests::a_wrong_token_is_not_found ... ok
test websrv::tests::a_state_changing_request_from_a_foreign_origin_is_rejected ... ok
test websrv::tests::an_opaque_origin_is_rejected ... ok
test websrv::tests::an_unknown_path_under_a_valid_token_is_not_found ... ok
test websrv::tests::a_truncated_token_is_not_found ... ok
test websrv::tests::a_state_changing_request_without_any_origin_is_rejected ... ok
test websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing ... ok
test websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel ... ok
test websrv::tests::an_invalid_value_shows_the_message_config_validate_returned ... ok
test websrv::tests::every_session_gets_its_own_token ... ok
test websrv::tests::the_listener_is_on_loopback ... ok
test websrv::tests::the_token_comparison_is_length_and_content_sensitive ... ok
test websrv::tests::our_own_page_may_post ... ok
test websrv::tests::the_query_string_does_not_hide_the_token ... ok
test websrv::tests::the_office_button_prefills_the_current_network_guid ... ok
test websrv::tests::the_right_token_serves_the_page ... ok
test websrv::tests::changing_only_the_port_does_not_rebind_the_listener ... ok
test websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page ... ok
test tray::tests::opening_the_speed_test_and_diagnostics_are_distinct_actions ... ok
test tray::tests::the_new_items_do_not_disturb_the_rest_of_the_menu ... ok
test websrv::tests::a_token_from_a_previous_session_is_not_found ... ok
test websrv::tests::the_number_of_simultaneous_connections_is_capped ... ok
test websrv::tests::activity_postpones_the_idle_timeout ... ok
test websrv::tests::the_diagnostics_button_shows_its_output_in_place ... ok
test websrv::tests::stopping_closes_the_door ... ok
test websrv::tests::dropping_the_handle_closes_the_door ... ok
test websrv::tests::the_server_stops_after_the_idle_timeout ... ok
test websrv::tests::a_request_without_a_token_does_not_postpone_the_timeout ... ok

test result: ok. 97 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.58s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 69 tests
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s

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
test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-b921d6d1fd7e845d.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s

   Doc-tests proxypilot_bridge
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_winnet
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

(В middle-крейтах `bridge`/`core` списки отдельных тестов из `bench` в
дословный вывод выше не сокращены нигде, кроме как ради длины этого
документа — списки тестов `proxypilot_bridge`/`proxypilot_core` совпадают с
известным набором задач 1–4 плюс `bench::tests`; сама команда прогонялась
без флагов фильтрации.)

Итог: **97 + 69 + 2 + 48 + 22 = 238 тестов проходят, 1 `#[ignore]`** (было
236 + 1 до этой задачи; прирост — ровно два новых теста в `tray.rs`).

### `cargo clippy --all-targets -- -D warnings`

```
$ cargo clippy --all-targets -- -D warnings

    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.94s
```

Пересобрался только изменённый крейт (`proxypilot-app`); остальные три уже
были проверены на этой базе и не менялись — ноль предупреждений.

### `cargo fmt --all --check`

```
$ cargo fmt --all --check
```

Пустой вывод, код выхода 0 — форматирование не нарушено.

---

## Самопроверка по чек-листу брифа

- **Переписал ли я часть меню вместо того, чтобы дополнить?** Нет. В
  `Tray::new` единственные новые строки — создание `bench`/`doctor` и два
  `menu.append`; существующие строки для header/network/separator/modes/
  settings/copy/quit не тронуты. `action_for` получил два новых `if` перед
  прежним поиском по режимам — остальное не менялось.
- **Переиспользует ли открытие замера уже запущенный сервер настроек, или
  поднимает второй?** Переиспользует. `Action::OpenBench`/`OpenDoctor` идут
  в ту же функцию `open_settings`, которая делает `if
  !server.as_ref().is_some_and(|s| s.is_running())` перед тем, как поднимать
  новый — то есть при уже открытой странице настроек клик по «Замерить
  скорость…» не создаёт второй `websrv::Server` и не генерирует второй
  токен, а просто открывает браузер на том же адресе с фрагментом `#bench`.
- **Показывается ли строка понижения при недоступности закреплённого
  режима?** Да — `situation_text`/`header_text` не менялись, и тест
  `header_explains_a_demotion_rather_than_hiding_it` (тот же, что был раньше)
  проходит без изменений в этом прогоне.
- **Тронуто ли что-то в пути выхода или в цикле `Cmd`?** Нет. Правка не
  касается `RestoreOnDrop`, `BRIDGE_STOPPED`, `Exit`, оконной процедуры
  завершения сеанса, `apply_change` или канала `Cmd` — новые пункты меню
  вообще не отправляют команд в `Cmd`, они только открывают браузер.

---

## Что человеку ещё нужно проверить руками

Всплывающее меню в этом окружении отрисовать нельзя (нет ни экрана с живым
пользователем, ни клика мышью) — сборка и id-маршрутизация проверены тестами
выше, но фактический клик — нет. Список для ручной проверки:

1. Запустить `ProxyPilot.exe`, открыть меню трея правой/левой кнопкой —
   убедиться, что «Замерить скорость…» и «Диагностика…» видны между
   «Настройки…» и «Копировать адрес моста», а не заменили и не сдвинули
   ничего из прежнего (режимы, разделители, «Выход»).
2. Кликнуть «Настройки…» — убедиться, что браузер открылся на странице без
   фрагмента (сверху страницы).
3. Не закрывая эту вкладку/сервер, кликнуть в трее «Замерить скорость…» —
   убедиться, что открылась НОВАЯ вкладка (или существующая с новым адресом)
   на том же порту/токене, что и в п. 2, и браузер сразу прокрутился к
   разделу «Замер скорости» (`#bench`). Проверить, что сервер не перезапустился
   (тот же порт в адресной строке, если он был показан/скопирован ранее).
4. Аналогично — кликнуть «Диагностика…», убедиться в прокрутке к разделу
   «Диагностика» (`#doctor`) на том же сервере.
5. Закрыть все вкладки настроек, подождать таймаут бездействия (15 минут)
   или перезапустить приложение, затем кликнуть «Замерить скорость…» ещё
   раз — убедиться, что сервер поднимается заново (это уже поведение задачи
   3, не задачи 5, но стоит один раз пройти глазами весь путь).
6. Проверить «Копировать адрес моста» — убедиться, что в буфере обмена
   оказывается `http://127.0.0.1:<порт>` (эта функциональность не менялась
   задачей 5, но явно упомянута в брифе как «уже реализовано» — стоит
   свериться, что это по-прежнему так).
