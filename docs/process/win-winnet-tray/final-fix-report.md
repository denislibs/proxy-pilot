# Финальная волна правок по обзору ветки `feat/windows-rust`

Дата: 2026-08-30. База: `abbb349`. Итог: `2ba4a93`.

Коммиты в порядке появления:

```
1bb6cbb fix(win): второй канал событий сети поднимается всегда, а не про запас
d5da468 feat(win): имя сети доезжает от NLM до состояния приложения
689dd0b fix(win): отказ старта показывается окном, а не только в лог
44e171d feat(win): пересчёт решения по таймеру, раз в минуту
9a7103a fix(win): прежняя оконная процедура запоминается до подмены, а не после
68a3486 docs(win): почему иконок четыре, а в спеке пять состояний
2ba4a93 fix(win): отказ старта успевает попасть в лог, а не только в окно
```

Тестов: было 147 + 1 `#[ignore]`, стало **156 + 1 `#[ignore]`**. Все три
проверки CI зелёные — вывод в конце файла.

---

## FIX 1 — второй канал событий сети поднимается всегда

Коммит `1bb6cbb`, файл `win/crates/winnet/src/events.rs`.

**Что было.** `subscribe_ip_helper` вызывалась только на ветке отказа
`subscribe_nlm`. На здоровой машине работал один NLM, а его единственное
событие `ConnectivityChanged` — машинный агрегат связности: док-станция при
живом Wi-Fi, смена категории Public↔Domain и появление второй сети его не
двигают. Варианты `NetworkChange::NetworkAdded` и `NetworkPropertyChanged`
были недостижимы.

**Что стало.** В `watcher_thread` оба канала поднимаются независимо:

- отказ NLM больше не ведёт к отказу подписки целиком, а только к `warn!`
  «подписка на события NLM не поднялась: смена сети замечается только по
  `NotifyIpInterfaceChange`»;
- отказ IP Helper — к `warn!` «`NotifyIpInterfaceChange` не поднялся: смена
  сети замечается только по агрегату связности NLM»;
- отказ обоих — прежнее поведение: `warn!`, `pump.alive = false`,
  `ready.send(Err(...))` с текстом обеих ошибок, выход из потока.

Разбор подписки по смыслу не изменился: `teardown(nlm, iphelper)` снимает и
`Unadvise`, и `CancelMibChangeNotify2`, и обе ветки выхода (не дождались
`ready`; обычный конец цикла сообщений) зовут её с обоими значениями.

**Как оператор отличает случаи.** Строка «подписка на события сети поднята»
поднята с `debug!` до `info!` — уровень лога по умолчанию именно info (см.
`bridge::log`), а строка пишется один раз за жизнь процесса, — и несёт поле
`source`: `nlm+iphelper` / `nlm` / `iphelper`. Подпись вынесена в чистую
функцию `source_label` ради теста.

Комментарии обновлены: модульный, `watcher_thread`, `subscribe_ip_helper`
(больше не «запасной канал», а второй равноправный) и доккомментарий
ручного теста.

**Тесты.** Добавлен `the_log_line_names_every_combination_of_armed_channels`
— три комбинации дают три разные строки. Тесты `debounce` не менялись:
схлопывание и есть тот механизм, который гасит дубли от двух источников, и
его контракт прежний.

Живая проверка — отдельным разделом ниже.

## FIX 2 — пересчёт по таймеру

Коммит `44e171d`, файл `win/crates/app/src/main.rs`.

Добавлены константа `REEVALUATE_PERIOD = 60 с` и функция
`spawn_periodic_reevaluate`, шлющая `Cmd::Reevaluate` в **тот же** канал
`Cmd`, что и события сети и клики в меню. Второго пути в супервизор не
заведено.

- `interval_at(now + period, period)` — первый тик не немедленный: пересчёт
  на старте уже сделан, до создания слушателя.
- `MissedTickBehavior::Delay` — ноутбук, вернувшийся из сна, получает один
  пересчёт, а не пачку пропущенных тиков в очередь на 16 мест.
- Комментарий у константы объясняет, почему период **больше** TTL проб
  (30 с): на более частом тике `Prober` отдавал бы кэш, и половина
  пересчётов не проверяла бы ничего, зато вызовы NLM тратились бы вдвое
  чаще.

**Тесты.** `the_periodic_reevaluation_is_slower_than_the_probe_cache`
закрепляет неравенство `REEVALUATE_PERIOD > PROBE_TTL`, чтобы одну из
констант нельзя было тронуть, не взглянув на вторую. Заодно добавлен
`the_window_messages_do_not_collide` на номера оконных сообщений.

## FIX 3 — отказ старта виден человеку

Коммиты `689dd0b` и `2ba4a93`; новый файл `win/crates/app/src/ui.rs`,
правки в `win/crates/app/src/main.rs`.

Новый модуль `ui` с `error_box(title, text)` — `MessageBoxW` с
`MB_OK | MB_ICONERROR | MB_SETFOREGROUND`. Отдельным модулем, потому что
окно настроек этапа 3 попросит тот же примитив. `MB_SETFOREGROUND` не
косметика: у процесса без окон нет активности, и без флага окно может
открыться под уже работающими — то есть снова остаться незамеченным.

В `main` появилась `report_failure`: `eprintln!` в отладочной сборке,
`ui::error_box` — в релизной. Ветки разделены `cfg!`, а не `#[cfg]`, чтобы
обе компилировались в обеих сборках: при `#[cfg]` в отладке `error_box`
осталась бы никем не вызванной, то есть мёртвым кодом, и предупреждение
пришлось бы глушить атрибутом.

**Попутно найдено и исправлено (`2ba4a93`).** Обзор исходил из того, что
«единственный след — файл лога». На живой машине следа не было и там:
`error!` стояла в `main`, ПОСЛЕ возврата из `run`, а страж
`tracing-appender` останавливает пишущий поток в своём `Drop` — то есть на
выходе из `run`. Строка уходила в остановленный писатель и пропадала молча
(проверено: в файле её не было). Тело `run` вынесено в `run_logged`, а
запись отказа осталась в `run`, где страж ещё жив. Без этого окно с ошибкой
было бы вообще единственным каналом.

**Живая проверка на релизной сборке.** Порт 3129 занят подставным
слушателем (`System.Net.Sockets.TcpListener`), запущен
`win/target/release/proxypilot.exe`:

```
окно: 'ProxyPilot'          класс окна #32770 — стандартный диалог Windows
код возврата: 1
лог: 2026-08-30T04:01:01.085131Z ERROR proxypilot: запуск не удался
     error=не занять 127.0.0.1:3129: Обычно разрешается только одно
     использование адреса сокета (протокол/сетевой адрес/порт).
     (os error 10048); возможно, proxypilot уже запущен
```

Окно закрыто штатным `WM_CLOSE`, процесс вышел сам. Системный прокси не
тронут: `bind` падает раньше `take_over`. `ProxyEnable=0`,
`ProxyServer=203.0.113.10:3128` до и после — без изменений.

## FIX 4 — имя сети доезжает до состояния приложения

Коммит `d5da468`. Файлы: `core/src/mode.rs`, `core/src/config.rs`,
`bridge/src/supervisor.rs`, `app/src/main.rs`, `app/src/tray.rs`.

- В `core::mode` заведён `ConnectedNetwork { id, name }` — простые данные,
  без единой зависимости от `winnet`. Граница переносимости осталась там же,
  где была: `NetworkSnapshot` → `ConnectedNetwork` перекладывается в
  `NlmSource` в приложении; категория и признак интернета туда не едут,
  потому что решение принимается по GUID (спека 2.3), а лишнее поле в модели
  — приглашение начать решать по нему.
- `Place` получил `network_name: Option<String>`. `Config::place_for` теперь
  принимает `&[ConnectedNetwork]` и проносит имя выбранной сети — той самой,
  по которой принято решение, а не первой попавшейся. Сравнение по-прежнему
  только по `id`.
- `NetworkSource::connected_ids` → `NetworkSource::connected`, отдаёт
  снимки.
- В меню трея — отдельный неактивный пункт «Сеть: …» (спека 11.1, секция
  сети), а не приписка к заголовку: заголовок дублируется во всплывающую
  подсказку иконки, а там длина ограничена, и адрес моста с маршрутом и
  именем сети обрезались бы посередине. Пустое имя (сеть без профиля)
  откатывается на GUID.

**Тесты.** В `core`: новый `the_name_never_decides_anything` (сеть,
названная «Офис», но с чужим GUID, офисом не считается — иначе достаточно
назвать свою точку доступа «Офис», чтобы увести на неё корпоративный
маршрут), плюс проверки имени в существующих `place_is_office_…`,
`place_is_not_office_…`, `several_connected_networks_office_wins`,
`no_network_at_all_is_not_office`. В `bridge`: новый
`the_network_name_reaches_the_app_state`. В `app`: четыре новых теста на
`network_text` — офис, не офис, безымянная сеть, сети нет.

## FIX 5 — `PREV_WNDPROC` пишется до подмены

Коммит `9a7103a`, файл `win/crates/app/src/tray.rs`.

`install_session_end_guard` теперь читает текущую процедуру
`GetWindowLongPtrW(hwnd, GWLP_WNDPROC)`, кладёт её в `PREV_WNDPROC` и лишь
затем зовёт `SetWindowLongPtrW`. Если возврат подмены разошёлся с
прочитанным (кто-то встал в цепочку между двумя вызовами), авторитетен
возврат подмены — он и записывается, с `warn!`: иначе мы выкинули бы его
звено и сломали бы то, что оно обслуживало.

## FIX 6 — почему иконок четыре

Коммит `68a3486`, файл `win/crates/app/src/icons.rs`. Комментарий над
`IconKind`: пятого состояния «мост не запущен» нет, потому что приложение
выходит, как только мост перестаёт принимать соединения (`BRIDGE_STOPPED` в
`main.rs`); кто соберётся вернуть состояние, обязан сначала вернуть условие,
при котором оно достижимо.

---

## Инварианты — не тронуты

- `Router::get()` — по-прежнему один нетестовый вызов (`serve.rs`,
  `pick_route`) и один нетестовый писатель (`set_if_changed` в супервизоре).
  `spawn_periodic_reevaluate` пишет не в роутер, а в существующий канал
  `Cmd`.
- Слушатель привязывается один раз; ни одна правка не трогает `bind`.
- `RestoreOnDrop`, `BRIDGE_STOPPED`, `Exit`, оконная процедура завершения
  сеанса по смыслу не менялись. FIX 5 меняет только порядок двух инструкций
  при установке процедуры; FIX 3 показывает окно уже ПОСЛЕ возврата из
  `run`, то есть после того, как страж восстановления отработал.
- `proxypilot-core` остался без платформенных зависимостей: `Cargo.toml`
  крейта не менялся, `ConnectedNetwork` — два `String`.
- UAC не появился: ни одна правка не трогает `HKLM`, службы и `netsh`.

## Живая проверка FIX 1

Машина: Wi-Fi `KZTK-38455_5G` — единственное подключение с интернетом,
Ethernet отключён. Ручной `#[ignore]`-тест `watch_a_real_network_change`
запущен фоном; через 10 с — `netsh wlan disconnect`, ещё через 10 с —
`netsh wlan connect name="KZTK-38455_5G"`. Прав администратора не
требовалось, машина не перезагружалась, сеанс не завершался.

```
running 1 test
2026-08-30T03:56:29.422005Z  INFO proxypilot_winnet::events: подписка на события сети поднята thread=37020 source="nlm+iphelper"
2026-08-30T03:56:29.422041Z  INFO proxypilot_winnet::events::tests: ждём смены сети 45 секунд: выключите и включите Wi-Fi
2026-08-30T03:56:39.445697Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T03:56:39.460153Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=Connectivity номер=1
2026-08-30T03:56:39.474115Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:39.484445Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:39.965321Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:39.967355Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:41.461896Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=NetworkPropertyChanged номер=2
2026-08-30T03:56:49.561902Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:49.562830Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:49.563268Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=NetworkPropertyChanged номер=3
2026-08-30T03:56:49.591985Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:49.592661Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:49.596134Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T03:56:49.600762Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:49.607297Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T03:56:49.607646Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:49.607941Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:49.611427Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T03:56:49.643339Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:50.560407Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:50.562142Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=NetworkPropertyChanged
2026-08-30T03:56:50.608586Z  INFO proxypilot_winnet::events::tests: сырое событие NLM ev=Connectivity
2026-08-30T03:56:51.576089Z  INFO proxypilot_winnet::events::tests: смена сети после схлопывания ev=Connectivity номер=4
2026-08-30T03:57:14.433451Z  INFO proxypilot_winnet::events::tests: окно наблюдения закрыто всего=4
test events::tests::watch_a_real_network_change ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 22 filtered out; finished in 45.03s
```

Как это читать:

- `source="nlm+iphelper"` — оба канала подняты, как и задумано; до правки
  здесь стояло бы `nlm`, и строка была бы на уровне `debug`, то есть в
  файле лога её не было бы вовсе;
- в сырой пачке есть `NetworkPropertyChanged` — вариант, который до этой
  правки на здоровой машине был недостижим: он приходит только от IP
  Helper. Второй канал действительно работает;
- **отключение** (03:56:39–03:56:40) дало 5 сырых событий → наружу ушло
  ровно два: передний фронт (`номер=1`, немедленно) и задний (`номер=2`,
  по закрытии окна схлопывания в 2 с);
- **подключение** (03:56:49–03:56:50) дало 13 сырых событий → наружу ушло
  ровно два: `номер=3` и `номер=4`;
- то есть на одну физическую смену — одна пара «передний + задний фронт», а
  не по паре на канал. Ровно то, что обещает доккомментарий `debounce`.

После проверки: Wi-Fi подключён к тому же `KZTK-38455_5G`. Системные
настройки прокси не менялись — сверено до и после: `ProxyEnable=0`,
`ProxyServer=203.0.113.10:3128`, `ProxyOverride` оканчивается на `<local>`.
Тест `proxy::tests::the_real_corporate_setting_of_this_machine_is_left_alone`
в наборе тоже зелёный.

## Вывод трёх проверок CI

```
$ cargo test --all
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.57s
     Running unittests src\main.rs (target\debug\deps\proxypilot-f9c0433b09311d11.exe)

running 24 tests
test icons::tests::a_deliberate_direct_mode_is_not_unconfigured ... ok
test proxy::tests::a_pointer_at_us_is_recognised_even_with_the_switch_off ... ok
test proxy::tests::the_per_protocol_form_is_recognised_too ... ok
test icons::tests::nothing_configured_gets_its_own_icon ... ok
test icons::tests::icon_reflects_the_active_route ... ok
test proxy::tests::localhost_by_name_is_ours_as_well ... ok
test proxy::tests::our_address_on_another_port_is_not_ours ... ok
test icons::tests::every_icon_is_a_full_rgba_buffer ... ok
test proxy::tests::the_real_corporate_setting_of_this_machine_is_left_alone ... ok
test tests::the_periodic_reevaluation_is_slower_than_the_probe_cache ... ok
test tests::the_window_messages_do_not_collide ... ok
test proxy::tests::a_disabled_pointer_at_our_address_is_not_stale ... ok
test tray::tests::a_mode_that_is_merely_unconfigured_says_so ... ok
test tray::tests::header_explains_a_demotion_rather_than_hiding_it ... ok
test proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected ... ok
test tray::tests::header_names_the_bridge_and_the_route ... ok
test icons::tests::icons_differ_from_each_other ... ok
test tray::tests::a_network_outside_the_office_is_not_marked_as_one ... ok
test tray::tests::a_nameless_network_falls_back_to_its_guid ... ok
test tray::tests::header_names_the_upstream_it_actually_uses ... ok
test tray::tests::the_bridge_address_is_always_loopback ... ok
test tray::tests::the_network_line_shows_the_name_and_marks_the_office ... ok
test tray::tests::without_any_network_the_line_says_so ... ok
test tray::tests::wm_endsession_only_means_the_session_is_ending_when_wparam_is_true ... ok

test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 60 tests
test http::tests::parses_connect ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::truncated_input_is_an_error ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok
test log::tests::log_file_name_is_stable ... ok
test probe::tests::an_unconfigured_upstream_is_unknown_not_down ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::set_if_changed_publishes_a_different_value ... ok
test router::tests::set_if_changed_skips_a_matching_value ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::is_shareable_across_threads ... ok
test router::tests::set_if_changed_reports_exactly_one_winner_under_concurrent_writers ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... ok
test socks5::tests::surfaces_refusal_code ... ok
test supervisor::tests::in_the_office_with_a_live_socks_the_route_becomes_socks ... ok
test supervisor::tests::outside_the_office_the_route_is_direct_even_with_a_live_upstream ... ok
test probe::tests::a_silent_address_is_down_within_the_timeout ... ok
test supervisor::tests::a_dead_pinned_upstream_is_reported_as_demoted ... ok
test supervisor::tests::the_network_name_reaches_the_app_state ... ok
test probe::tests::a_changed_address_is_not_answered_from_the_old_cache ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test probe::tests::a_live_listener_is_up_and_a_closed_port_is_down ... ok
test probe::tests::the_result_is_cached_within_the_ttl ... ok
test supervisor::tests::run_reevaluates_on_start_and_on_each_event_then_exits_when_the_channel_closes ... ok
test supervisor::tests::an_unchanged_decision_does_not_touch_the_router ... ok
test connector::tests::refused_upstream_reports_error ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok

test result: ok. 60 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s

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
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test config::tests::defaults_match_the_spec ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test config::tests::load_from_a_missing_file_yields_defaults ... ok
test config::tests::matching_is_case_insensitive ... ok
test config::tests::no_network_at_all_is_not_office ... ok
test config::tests::managing_the_system_proxy_is_on_by_default_and_switchable ... ok
test config::tests::place_is_not_office_for_an_unknown_network ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::place_is_office_when_a_connected_network_matches ... ok
test config::tests::several_connected_networks_office_wins ... ok
test config::tests::validate_rejects_a_malformed_upstream ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test config::tests::validate_accepts_the_defaults ... ok
test config::tests::the_name_never_decides_anything ... ok
test config::tests::validate_rejects_a_port_below_the_privileged_range ... ok
test config::tests::the_saved_system_proxy_survives_a_roundtrip ... ok
test config::tests::validate_rejects_a_zero_connection_limit ... ok
test config::tests::validate_rejects_an_office_network_with_empty_id ... ok
test config::tests::without_configured_offices_nothing_is_office ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::direct_mode_is_direct ... ok
test config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test config::tests::validate_rejects_an_absurd_connection_limit ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test config::tests::config_path_matches_what_the_spec_promises ... ok
test config::tests::save_then_load_roundtrips_through_a_real_file ... ok

test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-382daa61fec08b04.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test networks::tests::category_maps_every_documented_value ... ok
test events::tests::the_log_line_names_every_combination_of_armed_channels ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test events::tests::a_burst_collapses_to_its_first_and_last_event ... ok
test events::tests::dropping_the_debounced_receiver_releases_the_source ... ok
test events::tests::the_trailing_event_is_the_last_one_of_the_burst ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
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
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.37s

$ cargo fmt --all --check
(вывода нет; код возврата 0)
```
