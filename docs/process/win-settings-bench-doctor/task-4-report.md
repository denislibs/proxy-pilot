# Задача 4 плана 3 — страница настроек. Отчёт

**Коммит:** `cdeb45a` — feat(win): страница настроек — форма, валидация и вывод замера с диагностикой
**База:** `0ea1e1c` (ветка `feat/windows-rust`)
**Тесты:** 233 проходят + 1 `#[ignore]` (было 211 + 1). Прогон тестового двоичного файла приложения — **2,59 с**.

---

## Что сделано

Заглушка страницы из задачи 3 заменена настоящей. Новый файл
`win/crates/app/src/settings_page.rs` (разметка, разбор формы, применение
изменений); `win/crates/app/src/websrv.rs` остался транспортом и зовёт его
двумя строками в `serve_one`. Ни одна проверка транспорта не ослаблена:
токен, `Origin`/`Referer`, `Host`, потолки заголовка, тела, времени и числа
соединений, таймаут бездействия — все прежние тесты на месте, не менялись и
проходят.

### На странице

- оба апстрима с индикаторами доступности (`доступен` / `недоступен` /
  `ещё не проверен` / `не задан` — тем же словарём, что и трей);
- порт моста;
- список офисных сетей (GUID + имя), пустая строка для добавления и кнопка
  «Эта сеть — офис: ИМЯ»;
- bypass-список;
- тумблер управления системным прокси;
- тумблер автозапуска — шов до задачи 6;
- кнопки «Замерить» и «Проверить» с выводом там же, каждая в своей форме
  (чтобы не тащить с собой поля, которые человек ещё правит).

### Правило порта — как оно выполняется

`settings_page::live_config(saved, bound_port)` возвращает тот же конфиг, но
с портом, на котором слушатель УЖЕ привязан. Задача, обслуживающая канал
`Cmd` в `main.rs`, держит два конфига: `saved` уходит на диск таким, каким
его задал человек, а супервизор получает `live_config(&saved, port)`. То
есть смена порта в форме не доходит до `AppState.port` вообще — а значит и
до заголовка меню, «скопировать адрес» и диагностики. Перепривязки нет
нигде: во всём крейте приложения ровно два не-тестовых `bind` — мост
(`main.rs`, один раз за жизнь процесса) и собственный loopback-порт сервера
настроек (`websrv.rs`).

Тест `websrv::tests::changing_only_the_port_does_not_rebind_the_listener`
поднимает настоящий слушатель, шлёт POST, меняющий ТОЛЬКО порт, и проверяет
четыре вещи: старый слушатель по-прежнему принимает соединения; запрошенный
порт свободен настолько, что тест сам его занимает (перепривязка отдала бы
«адрес уже используется»); на диск ушло введённое значение, а `live_config`
от него по-прежнему даёт привязанный порт; страница показывает привязанный
порт и говорит про перезапуск.

### Единственный путь в супервизор

Новый вариант `Cmd::ApplyConfig { config, done }`. `done` — обратный
`oneshot`: страница обязана показать, что именно случилось (в том числе
отказ записи на диск), а не бодрое «сохранено» вслепую. Ответ отправляется в
самом конце витка, после `reevaluate` и `state.store`, чтобы страница
перерисовалась по уже применённому состоянию. `Router` из `settings_page` и
`websrv` не виден вовсе (`grep` по обоим файлам даёт только строку
комментария); `Router::get()` по-прежнему имеет ровно один не-тестовый вызов
(`serve.rs:366`).

Конфиг, который читает страница, живёт в `Arc<ArcSwap<Config>>`; пишет в него
только та же задача канала `Cmd`. Это читатель, а не второй писатель.

### Валидация

`config_from_form` только разбирает текст в типы; осмысленность значений
устанавливает `Config::validate`, и страница печатает её текст дословно.
Единственная проверка вне `validate` — разбор `bridge_port` в `u16`: в
`Config` поле уже типизировано, и «abc» до `validate` не доживает.

Форма владеет не всеми полями конфига: `mode` (трей), тайминги,
`max_connections` и — что важнее всего — `saved_sysproxy` берутся из
текущего конфига. Стереть `saved_sysproxy` при сохранении формы значило бы
потерять единственный след системных настроек пользователя до нас. Покрыто
тестом `the_form_does_not_touch_the_fields_it_does_not_own`.

### Кнопка «эта сеть — офис»

Обычная кнопка отправки формы (`name="action" value="office"`), а не скрипт:
GUID текущей сети сервер и так знает из `AppState.place`, и страница
остаётся вовсе без JavaScript. Благодаря этому `Content-Security-Policy`
транспорта не пришлось ослаблять до `script-src 'unsafe-inline'` — то есть
до той дырки, через которую любое пропущенное экранирование стало бы
выполнением чужого кода на странице, которая правит настройки прокси.
Кнопка не рисуется, когда текущая сеть не определена.

### Экранирование

Всё, что попадает в разметку, проходит через `escape_html` (`& < > " '`,
амперсанд первым). Источники недоверенные и названы в комментарии модуля:
имя сети приходит из системы (его задал тот, кто поднял точку доступа),
адреса апстримов, bypass-список и GUID — из файла, который правят руками,
тексты ошибок замера — из сети. Два теста (`settings_page` и `websrv`)
подают `<script>`, `<img src=x onerror=...>`, `"` и `&` и требуют, чтобы в
разметке их не оказалось.

### Диагностика по нажатию

`live_checks` — второй вызывающий `doctor::run_checks` и первый ЖИВОЙ:
подключение к собственному порту и свежий `sysproxy::read()`. Параметр
`port_was_free_before_bind` получает `!bridge_listening_now`: в живом пути
вопрос «не отвечал ли там никто» звучит как «не отвечает ли там никто
сейчас». Подставить туда `listening` значило бы, что проверка «в реестре наш
адрес, но моста нет» кричала бы отказ ровно тогда, когда мост как раз жив.
Модульный комментарий `doctor.rs` обновлён под появившегося вызывающего.

### Замер

`BENCH_URL = http://cachefly.cachefly.net/1mb.test`, лимит 1 МиБ, таймаут 3 с
на маршрут. Арифметика не случайна: маршрутов максимум три, они меряются по
очереди, и 9 с обязаны уместиться в `websrv::REQUEST_TIMEOUT` (15 с) вместе с
отрисовкой. Мёртвый маршрут показывается как мёртвый, а не пропускается.

---

## Ручной проверки в браузере НЕ БЫЛО

**В этом окружении живой проверки в браузере сделать было нельзя** — нет
интерактивного сеанса Windows, из которого можно открыть страницу и нажать
кнопку. Всё проверено тестами по HTTP (настоящий сокет, настоящий разбор
запроса, настоящий разбор формы), но человеческого прохода по интерфейсу не
выполнялось.

### Что человек должен прокликать

1. Трей → «Настройки…» — страница открывается в браузере. Открыть консоль
   разработчика и убедиться, что нарушений CSP нет (скриптов на странице нет,
   стиль один и инлайновый).
2. Заголовок вверху совпадает с заголовком меню трея (адрес моста, маршрут,
   сеть).
3. Сменить адрес SOCKS5 на живой → «Сохранить» → зелёная плашка «Сохранено и
   применено». **Проверить, что режим применился без перезапуска:** в трее
   маршрут и индикатор доступности изменились, `curl -x http://127.0.0.1:ПОРТ`
   идёт новым путём.
4. Вписать в SOCKS5 значение без порта → «Сохранить» → красная плашка с
   текстом ровно из `Config::validate` («нужен формат host:port»); настройки
   НЕ применены.
5. **Сменить порт моста на другой** → «Сохранить» → появилась строка
   «Сохранено N, но мост слушает M — перезапустите ProxyPilot». Убедиться,
   что `netstat -ano | findstr N` ничего не показывает, а `findstr M` —
   по-прежнему наш процесс; открытое через мост длинное соединение
   (например `curl` на большой файл) не оборвалось.
6. Перезапустить ProxyPilot — мост слушает новый порт, строки про
   расхождение больше нет.
7. «Эта сеть — офис: ИМЯ» → сеть с её GUID появилась в таблице; та же запись
   в `%APPDATA%\ProxyPilot\config.toml`; в режиме «Авто» трей показал
   «офис» и маршрут ушёл на апстрим.
8. Нажать ту же кнопку второй раз → красная плашка «эта сеть уже в списке
   офисных».
9. Очистить GUID строки → «Сохранить» → сеть исчезла из списка и из конфига.
10. «Замерить» → таблица со строкой на каждый настроенный маршрут; мёртвый
    апстрим показан строкой с ошибкой, а не пропущен.
11. «Проверить» → семь строк диагностики, «Мост слушает свой порт» — ок.
12. Тумблер автозапуска заблокирован и подписан «автозапуск ещё не подключён
    в этой сборке» (до задачи 6).
13. Закрыть вкладку, подождать 15 минут → повторное открытие того же адреса
    из истории даёт «страница недоступна», а пункт меню «Настройки…»
    открывает новую с новым токеном.
14. Переименовать сеть в Windows во что-нибудь с кавычками и угловыми
    скобками → имя на странице отображается как текст, разметка цела.

---

## Осознанные ограничения (не баги, но сказать обязан)

**1. Bypass-список и тумблер управления системным прокси применяются при
запуске, а не на лету.** На странице у обоих стоит явная строка «Применяется
при запуске: после изменения перезапустите ProxyPilot».

Почему не сделано живым:

- Bypass живёт в `serve::Shared.bypass: Arc<BypassList>` — поле создаётся
  один раз и уезжает в `serve`. Сделать его живым — это `ArcSwap` в структуре
  крейта `bridge` и правка `serve.rs`; бриф этой задачи ограничил файлы
  страницей и транспортом, и лезть в мост ради этого я не стал.
- `manage_system_proxy` живым сделать нельзя, не тронув путь восстановления:
  страж `RestoreOnDrop` и обработчик консоли создаются в `run_logged`
  условно (`cfg.manage_system_proxy.then(...)`). Включить управление уже
  после старта значило бы записать в реестр указатель на себя, не имея
  стража, который его вернёт; а сделать стража безусловным — ровно то, что
  глобальные ограничения запрещают трогать.

Живыми стали те правки, которые в сегодняшней архитектуре живыми быть могут
и которые определяют маршрут: апстримы и список офисных сетей. Они идут через
супервизор и пересчитывают решение немедленно.

**2. Мелкое следствие пункта 1.** Если снять тумблер «управлять системным
прокси» и сразу нажать «Проверить», строка «Системный прокси указывает на
нас» скажет «проверка не применяется» — хотя до перезапуска мы всё ещё
управляем реестром. Живёт ровно до перезапуска, о котором страница тут же и
просит.

**3. URL и объём для замера — константы, а не поля конфига.** Спека 10
перечисляет их в составе конфигурации; бриф задачи 4 в списке полей страницы
их не называет, и я не стал расширять `Config` за пределы задания. Кандидат
в отдельную маленькую задачу.

**4. Автозапуск — трейт `Autostart` с заглушкой `AutostartPending`.** Шов, а
не мёртвый код: заглушка используется, тумблер рисуется заблокированным и
честно говорит, что не подключён. Задаче 6 останется подставить реализацию в
одном месте `main.rs` — ни разметку, ни разбор формы трогать не придётся.

---

## Самопроверка по списку из брифа

| Вопрос | Ответ |
|---|---|
| Может ли смена порта дойти до слушателя хоть каким путём? | Нет. Единственные не-тестовые `bind` — мост (один раз) и порт самого сервера настроек. Конфиг в супервизор идёт только через `live_config`, который прибивает порт к привязанному. |
| Есть ли второй путь в супервизор помимо канала `Cmd`? | Нет. `grep Router` по `settings_page.rs` и `websrv.rs` даёт только строку комментария. `Router::get()` — один не-тестовый вызов, `serve.rs:366`. |
| Экранируется ли всё, что рисуется, включая пришедшее из сети? | Да. Числа (`u16`/`u64`) подставляются как числа, всё остальное — через `escape_html`. Два теста бьют по разметке метасимволами. |
| Показывает ли негодный ввод причину сервера или падает молча? | Показывает дословный текст `ConfigError`, и в супервизор такой конфиг не уходит (проверено `applied.all().is_empty()`). |
| Ослаблена ли хоть одна проверка транспорта? | Нет. Все прежние тесты `websrv` на месте и не менялись, CSP не тронута. |

---

## Список тестов, добавленных этой задачей

`settings_page`:

- `html_metacharacters_are_escaped`
- `a_form_is_parsed_with_percent_and_plus_decoding`
- `repeated_fields_keep_their_order`
- `the_form_does_not_touch_the_fields_it_does_not_own`
- `an_invalid_upstream_is_rejected_by_config_validate`
- `a_privileged_port_is_rejected_by_config_validate`
- `a_port_that_is_not_a_number_is_reported_not_swallowed`
- `the_live_config_keeps_the_port_the_bridge_is_bound_to`
- `empty_office_rows_are_dropped`
- `the_page_shows_both_upstreams_with_their_availability`
- `everything_rendered_into_the_page_is_escaped`
- `the_page_says_the_port_needs_a_restart`
- `the_page_offers_the_office_button_only_when_a_network_is_known`
- `a_failed_route_is_shown_as_failed_not_omitted`
- `diagnostics_output_is_shown_in_place_and_escaped`
- `the_autostart_toggle_says_it_is_not_wired_yet_instead_of_pretending`

`websrv` (через настоящий HTTP на настоящем сокете):

- `changing_only_the_port_does_not_rebind_the_listener` — то самое правило
- `a_valid_change_reaches_the_supervisor_through_the_command_channel`
- `an_invalid_value_shows_the_message_config_validate_returned`
- `values_with_html_metacharacters_are_escaped_in_the_page`
- `the_office_button_prefills_the_current_network_guid`
- `the_diagnostics_button_shows_its_output_in_place`

---

## TDD: падающий прогон ДО реализации

Тесты и каркас модуля были написаны первыми; тела ключевых функций
(`live_config`, `escape_html`, `Form::parse`, `config_from_form`, `render`,
`handle_post`) стояли `unimplemented!()`, чтобы каркас компилировался и
падение приходило от тестов, а не от компилятора. Полный вывод
`cargo test --all` того прогона — ниже, без сокращений. 25 падений из 89.

```text
warning: unused imports: `bench_all` and `fastest`
  --> crates\app\src\settings_page.rs:45:32
   |
45 | use proxypilot_bridge::bench::{bench_all, fastest, BenchResult};
   |                                ^^^^^^^^^  ^^^^^^^
   |
   = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

warning: unused import: `oneshot`
  --> crates\app\src\settings_page.rs:49:25
   |
49 | use tokio::sync::{mpsc, oneshot};
   |                         ^^^^^^^

warning: unused import: `tracing::warn`
  --> crates\app\src\settings_page.rs:50:5
   |
50 | use tracing::warn;
   |     ^^^^^^^^^^^^^

warning: unused imports: `CheckStatus` and `self`
  --> crates\app\src\settings_page.rs:52:21
   |
52 | use crate::doctor::{self, Check, CheckStatus};
   |                     ^^^^         ^^^^^^^^^^^

warning: value captured by `live` is never read
   --> crates\app\src\main.rs:428:21
    |
428 |                     live = settings_page::live_config(&saved, port);
    |                     ^^^^
    |
    = help: did you mean to capture by reference instead?
    = note: `#[warn(unused_assignments)]` (part of `#[warn(unused)]`) on by default

warning: constant `BENCH_URL` is never used
  --> crates\app\src\settings_page.rs:65:7
   |
65 | const BENCH_URL: &str = "http://cachefly.cachefly.net/1mb.test";
   |       ^^^^^^^^^
   |
   = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: constant `BENCH_LIMIT` is never used
  --> crates\app\src\settings_page.rs:66:7
   |
66 | const BENCH_LIMIT: u64 = 1024 * 1024;
   |       ^^^^^^^^^^^

warning: constant `BENCH_TIMEOUT` is never used
  --> crates\app\src\settings_page.rs:67:7
   |
67 | const BENCH_TIMEOUT: Duration = Duration::from_secs(3);
   |       ^^^^^^^^^^^^^

warning: constant `APPLY_TIMEOUT` is never used
  --> crates\app\src\settings_page.rs:74:7
   |
74 | const APPLY_TIMEOUT: Duration = Duration::from_secs(10);
   |       ^^^^^^^^^^^^^

warning: constant `PROBE_TIMEOUT` is never used
  --> crates\app\src\settings_page.rs:77:7
   |
77 | const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
   |       ^^^^^^^^^^^^^

warning: methods `is_enabled` and `set` are never used
  --> crates\app\src\settings_page.rs:87:8
   |
86 | pub trait Autostart: Send + Sync {
   |           --------- methods in this trait
87 |     fn is_enabled(&self) -> Result<bool, String>;
   |        ^^^^^^^^^^
88 |     fn set(&self, on: bool) -> Result<(), String>;
   |        ^^^

warning: fields `app`, `config`, `commands`, `bound_port`, and `autostart` are never read
   --> crates\app\src\settings_page.rs:110:9
    |
106 | pub struct SettingsState {
    |            ------------- fields in this struct
...
110 |     pub app: Arc<ArcSwap<AppState>>,
    |         ^^^
...
118 |     pub config: Arc<ArcSwap<Config>>,
    |         ^^^^^^
119 |     /// Единственный путь применить изменение.
120 |     pub commands: mpsc::Sender<Cmd>,
    |         ^^^^^^^^
121 |     /// Порт, на котором мост слушает СЕЙЧАС и до конца жизни процесса.
122 |     pub bound_port: u16,
    |         ^^^^^^^^^^
123 |     pub autostart: Arc<dyn Autostart>,
    |         ^^^^^^^^^

warning: fields `notes`, `bench`, and `doctor` are never read
   --> crates\app\src\settings_page.rs:129:9
    |
128 | pub struct Outcome {
    |            ------- fields in this struct
129 |     pub notes: Vec<Note>,
    |         ^^^^^
130 |     pub bench: Option<Vec<BenchResult>>,
    |         ^^^^^
131 |     pub doctor: Option<Vec<Check>>,
    |         ^^^^^^
    |
    = note: `Outcome` has a derived impl for the trait `Debug`, but this is intentionally ignored during dead code analysis

warning: fields `bad` and `text` are never read
   --> crates\app\src\settings_page.rs:136:9
    |
135 | pub struct Note {
    |            ---- fields in this struct
136 |     pub bad: bool,
    |         ^^^
137 |     pub text: String,
    |         ^^^^
    |
    = note: `Note` has a derived impl for the trait `Debug`, but this is intentionally ignored during dead code analysis

warning: associated functions `ok` and `bad` are never used
   --> crates\app\src\settings_page.rs:141:8
    |
140 | impl Outcome {
    | ------------ associated functions in this implementation
141 |     fn ok(text: impl Into<String>) -> Self {
    |        ^^
...
151 |     fn bad(text: impl Into<String>) -> Self {
    |        ^^^

warning: `proxypilot-app` (bin "proxypilot" test) generated 15 warnings (run `cargo fix --bin "proxypilot" -p proxypilot-app --tests` to apply 4 suggestions)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.18s
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 89 tests
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free ... ok
test doctor::tests::an_ordinary_relaunch_trips_neither_bridge_check ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::a_sysproxy_read_failure_is_reported_once_not_as_two_failures ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
test doctor::tests::no_recognised_network_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::seven_rows_come_back_every_time ... ok
test doctor::tests::no_stale_pointer_when_the_registry_points_elsewhere ... ok
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
test icons::tests::every_icon_is_a_full_rgba_buffer ... ok
test proxy::tests::localhost_by_name_is_ours_as_well ... ok
test icons::tests::icons_differ_from_each_other ... ok
test proxy::tests::our_address_on_another_port_is_not_ours ... ok
test proxy::tests::stale_registry_pointing_at_us_without_a_bridge_is_detected ... ok
test proxy::tests::the_per_protocol_form_is_recognised_too ... ok
test proxy::tests::the_real_corporate_setting_of_this_machine_is_left_alone ... ok
test settings_page::tests::a_form_is_parsed_with_percent_and_plus_decoding ... FAILED
test settings_page::tests::a_port_that_is_not_a_number_is_reported_not_swallowed ... FAILED
test settings_page::tests::a_privileged_port_is_rejected_by_config_validate ... FAILED
test settings_page::tests::an_invalid_upstream_is_rejected_by_config_validate ... FAILED
test settings_page::tests::empty_office_rows_are_dropped ... FAILED
test settings_page::tests::html_metacharacters_are_escaped ... FAILED
test settings_page::tests::everything_rendered_into_the_page_is_escaped ... FAILED
test settings_page::tests::repeated_fields_keep_their_order ... FAILED
test settings_page::tests::the_form_does_not_touch_the_fields_it_does_not_own ... FAILED
test settings_page::tests::the_live_config_keeps_the_port_the_bridge_is_bound_to ... FAILED
test settings_page::tests::the_page_offers_the_office_button_only_when_a_network_is_known ... FAILED
test settings_page::tests::the_page_says_the_port_needs_a_restart ... FAILED
test settings_page::tests::the_page_shows_both_upstreams_with_their_availability ... FAILED
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
test websrv::tests::a_state_changing_request_without_any_origin_is_rejected ... ok
test websrv::tests::an_unknown_path_under_a_valid_token_is_not_found ... ok
test websrv::tests::a_wrong_token_is_not_found ... ok
test websrv::tests::a_state_changing_request_from_a_foreign_origin_is_rejected ... ok
test websrv::tests::a_foreign_host_header_is_not_found ... ok
test websrv::tests::a_truncated_token_is_not_found ... ok
test websrv::tests::a_request_without_the_token_is_not_found ... ok
test websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing ... FAILED
test websrv::tests::an_opaque_origin_is_rejected ... ok
test websrv::tests::every_session_gets_its_own_token ... ok
test websrv::tests::the_listener_is_on_loopback ... ok
test websrv::tests::the_token_comparison_is_length_and_content_sensitive ... ok
test websrv::tests::our_own_page_may_post ... FAILED
test websrv::tests::the_right_token_serves_the_page ... FAILED
test websrv::tests::the_query_string_does_not_hide_the_token ... FAILED
test websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page ... FAILED
test websrv::tests::a_token_from_a_previous_session_is_not_found ... ok
test websrv::tests::activity_postpones_the_idle_timeout ... FAILED
test websrv::tests::the_number_of_simultaneous_connections_is_capped ... FAILED
test websrv::tests::stopping_closes_the_door ... ok
test websrv::tests::dropping_the_handle_closes_the_door ... ok
test websrv::tests::the_server_stops_after_the_idle_timeout ... ok
test websrv::tests::a_request_without_a_token_does_not_postpone_the_timeout ... ok
test websrv::tests::the_diagnostics_button_shows_its_output_in_place ... FAILED
test websrv::tests::an_invalid_value_shows_the_message_config_validate_returned ... FAILED
test websrv::tests::changing_only_the_port_does_not_rebind_the_listener ... FAILED
test websrv::tests::the_office_button_prefills_the_current_network_guid ... FAILED
test websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel ... FAILED

failures:

---- settings_page::tests::a_form_is_parsed_with_percent_and_plus_decoding stdout ----

thread 'settings_page::tests::a_form_is_parsed_with_percent_and_plus_decoding' (34772) panicked at crates\app\src\settings_page.rs:195:9:
not implemented: разбор формы — задача этой реализации
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- settings_page::tests::a_port_that_is_not_a_number_is_reported_not_swallowed stdout ----

thread 'settings_page::tests::a_port_that_is_not_a_number_is_reported_not_swallowed' (25588) panicked at crates\app\src\settings_page.rs:195:9:
not implemented: разбор формы — задача этой реализации

---- settings_page::tests::a_privileged_port_is_rejected_by_config_validate stdout ----

thread 'settings_page::tests::a_privileged_port_is_rejected_by_config_validate' (9180) panicked at crates\app\src\settings_page.rs:195:9:
not implemented: разбор формы — задача этой реализации

---- settings_page::tests::an_invalid_upstream_is_rejected_by_config_validate stdout ----

thread 'settings_page::tests::an_invalid_upstream_is_rejected_by_config_validate' (13604) panicked at crates\app\src\settings_page.rs:195:9:
not implemented: разбор формы — задача этой реализации

---- settings_page::tests::empty_office_rows_are_dropped stdout ----

thread 'settings_page::tests::empty_office_rows_are_dropped' (32040) panicked at crates\app\src\settings_page.rs:195:9:
not implemented: разбор формы — задача этой реализации

---- settings_page::tests::html_metacharacters_are_escaped stdout ----

thread 'settings_page::tests::html_metacharacters_are_escaped' (23028) panicked at crates\app\src\settings_page.rs:181:5:
not implemented: экранирование — задача этой реализации

---- settings_page::tests::everything_rendered_into_the_page_is_escaped stdout ----

thread 'settings_page::tests::everything_rendered_into_the_page_is_escaped' (11948) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

---- settings_page::tests::repeated_fields_keep_their_order stdout ----

thread 'settings_page::tests::repeated_fields_keep_their_order' (37356) panicked at crates\app\src\settings_page.rs:195:9:
not implemented: разбор формы — задача этой реализации

---- settings_page::tests::the_form_does_not_touch_the_fields_it_does_not_own stdout ----

thread 'settings_page::tests::the_form_does_not_touch_the_fields_it_does_not_own' (24440) panicked at crates\app\src\settings_page.rs:195:9:
not implemented: разбор формы — задача этой реализации

---- settings_page::tests::the_live_config_keeps_the_port_the_bridge_is_bound_to stdout ----

thread 'settings_page::tests::the_live_config_keeps_the_port_the_bridge_is_bound_to' (33828) panicked at crates\app\src\settings_page.rs:171:5:
not implemented: правило порта — задача этой реализации

---- settings_page::tests::the_page_offers_the_office_button_only_when_a_network_is_known stdout ----

thread 'settings_page::tests::the_page_offers_the_office_button_only_when_a_network_is_known' (25904) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

---- settings_page::tests::the_page_says_the_port_needs_a_restart stdout ----

thread 'settings_page::tests::the_page_says_the_port_needs_a_restart' (33492) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

---- settings_page::tests::the_page_shows_both_upstreams_with_their_availability stdout ----

thread 'settings_page::tests::the_page_shows_both_upstreams_with_their_availability' (31024) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

---- websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing stdout ----

thread 'websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing' (2864) panicked at crates\app\src\settings_page.rs:247:5:
not implemented: обработка формы — задача этой реализации

thread 'websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing' (2864) panicked at crates\app\src\websrv.rs:991:9:
получили: 

---- websrv::tests::our_own_page_may_post stdout ----

thread 'websrv::tests::our_own_page_may_post' (25492) panicked at crates\app\src\settings_page.rs:247:5:
not implemented: обработка формы — задача этой реализации

thread 'websrv::tests::our_own_page_may_post' (25492) panicked at crates\app\src\websrv.rs:963:9:
получили: 

---- websrv::tests::the_right_token_serves_the_page stdout ----

thread 'websrv::tests::the_right_token_serves_the_page' (21624) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

thread 'websrv::tests::the_right_token_serves_the_page' (21624) panicked at crates\app\src\websrv.rs:867:9:
получили: 

---- websrv::tests::the_query_string_does_not_hide_the_token stdout ----

thread 'websrv::tests::the_query_string_does_not_hide_the_token' (29832) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

thread 'websrv::tests::the_query_string_does_not_hide_the_token' (29832) panicked at crates\app\src\websrv.rs:923:9:
получили: 

---- websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page stdout ----

thread 'websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page' (30892) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

thread 'websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page' (30892) panicked at crates\app\src\websrv.rs:1279:9:
получили: 

---- websrv::tests::activity_postpones_the_idle_timeout stdout ----

thread 'websrv::tests::activity_postpones_the_idle_timeout' (28948) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

thread 'websrv::tests::activity_postpones_the_idle_timeout' (28948) panicked at crates\app\src\websrv.rs:1091:13:
получили: 

---- websrv::tests::the_number_of_simultaneous_connections_is_capped stdout ----

thread 'websrv::tests::the_number_of_simultaneous_connections_is_capped' (35032) panicked at crates\app\src\settings_page.rs:241:5:
not implemented: разметка — задача этой реализации

thread 'websrv::tests::the_number_of_simultaneous_connections_is_capped' (35032) panicked at crates\app\src\websrv.rs:916:9:
получили: 

---- websrv::tests::the_diagnostics_button_shows_its_output_in_place stdout ----

thread 'websrv::tests::the_diagnostics_button_shows_its_output_in_place' (32468) panicked at crates\app\src\websrv.rs:1326:9:
получили: 

---- websrv::tests::an_invalid_value_shows_the_message_config_validate_returned stdout ----

thread 'websrv::tests::an_invalid_value_shows_the_message_config_validate_returned' (15848) panicked at crates\app\src\websrv.rs:1246:9:
получили: 

---- websrv::tests::changing_only_the_port_does_not_rebind_the_listener stdout ----

thread 'websrv::tests::changing_only_the_port_does_not_rebind_the_listener' (33772) panicked at crates\app\src\websrv.rs:1175:9:
получили: 

---- websrv::tests::the_office_button_prefills_the_current_network_guid stdout ----

thread 'websrv::tests::the_office_button_prefills_the_current_network_guid' (17532) panicked at crates\app\src\websrv.rs:1311:9:
получили: 

---- websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel stdout ----

thread 'websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel' (9164) panicked at crates\app\src\websrv.rs:1225:9:
получили: 


failures:
    settings_page::tests::a_form_is_parsed_with_percent_and_plus_decoding
    settings_page::tests::a_port_that_is_not_a_number_is_reported_not_swallowed
    settings_page::tests::a_privileged_port_is_rejected_by_config_validate
    settings_page::tests::an_invalid_upstream_is_rejected_by_config_validate
    settings_page::tests::empty_office_rows_are_dropped
    settings_page::tests::everything_rendered_into_the_page_is_escaped
    settings_page::tests::html_metacharacters_are_escaped
    settings_page::tests::repeated_fields_keep_their_order
    settings_page::tests::the_form_does_not_touch_the_fields_it_does_not_own
    settings_page::tests::the_live_config_keeps_the_port_the_bridge_is_bound_to
    settings_page::tests::the_page_offers_the_office_button_only_when_a_network_is_known
    settings_page::tests::the_page_says_the_port_needs_a_restart
    settings_page::tests::the_page_shows_both_upstreams_with_their_availability
    websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing
    websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel
    websrv::tests::activity_postpones_the_idle_timeout
    websrv::tests::an_invalid_value_shows_the_message_config_validate_returned
    websrv::tests::changing_only_the_port_does_not_rebind_the_listener
    websrv::tests::our_own_page_may_post
    websrv::tests::the_diagnostics_button_shows_its_output_in_place
    websrv::tests::the_number_of_simultaneous_connections_is_capped
    websrv::tests::the_office_button_prefills_the_current_network_guid
    websrv::tests::the_query_string_does_not_hide_the_token
    websrv::tests::the_right_token_serves_the_page
    websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page

test result: FAILED. 64 passed; 25 failed; 0 ignored; 0 measured; 0 filtered out; finished in 15.02s

error: test failed, to rerun pass `-p proxypilot-app --bin proxypilot`
```

---

## Зелёный прогон и три команды CI

### `cargo test --all`

```text
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.28s
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 92 tests
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free ... ok
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::no_recognised_network_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::an_ordinary_relaunch_trips_neither_bridge_check ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::a_sysproxy_read_failure_is_reported_once_not_as_two_failures ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
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
test icons::tests::every_icon_is_a_full_rgba_buffer ... ok
test proxy::tests::our_address_on_another_port_is_not_ours ... ok
test icons::tests::icons_differ_from_each_other ... ok
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
test settings_page::tests::html_metacharacters_are_escaped ... ok
test settings_page::tests::everything_rendered_into_the_page_is_escaped ... ok
test settings_page::tests::repeated_fields_keep_their_order ... ok
test settings_page::tests::the_autostart_toggle_says_it_is_not_wired_yet_instead_of_pretending ... ok
test settings_page::tests::the_form_does_not_touch_the_fields_it_does_not_own ... ok
test settings_page::tests::the_live_config_keeps_the_port_the_bridge_is_bound_to ... ok
test settings_page::tests::the_page_offers_the_office_button_only_when_a_network_is_known ... ok
test settings_page::tests::the_page_says_the_port_needs_a_restart ... ok
test settings_page::tests::the_page_shows_both_upstreams_with_their_availability ... ok
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
test websrv::tests::an_unknown_path_under_a_valid_token_is_not_found ... ok
test websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel ... ok
test websrv::tests::a_wrong_token_is_not_found ... ok
test websrv::tests::a_truncated_token_is_not_found ... ok
test websrv::tests::a_request_without_the_token_is_not_found ... ok
test websrv::tests::a_state_changing_request_from_a_foreign_origin_is_rejected ... ok
test websrv::tests::a_state_changing_request_without_any_origin_is_rejected ... ok
test websrv::tests::an_opaque_origin_is_rejected ... ok
test websrv::tests::an_invalid_value_shows_the_message_config_validate_returned ... ok
test websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing ... ok
test websrv::tests::every_session_gets_its_own_token ... ok
test websrv::tests::the_listener_is_on_loopback ... ok
test websrv::tests::the_token_comparison_is_length_and_content_sensitive ... ok
test websrv::tests::our_own_page_may_post ... ok
test websrv::tests::changing_only_the_port_does_not_rebind_the_listener ... ok
test websrv::tests::the_office_button_prefills_the_current_network_guid ... ok
test websrv::tests::the_right_token_serves_the_page ... ok
test websrv::tests::the_query_string_does_not_hide_the_token ... ok
test websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page ... ok
test websrv::tests::a_token_from_a_previous_session_is_not_found ... ok
test websrv::tests::the_number_of_simultaneous_connections_is_capped ... ok
test websrv::tests::activity_postpones_the_idle_timeout ... ok
test websrv::tests::the_diagnostics_button_shows_its_output_in_place ... ok
test websrv::tests::dropping_the_handle_closes_the_door ... ok
test websrv::tests::stopping_closes_the_door ... ok
test websrv::tests::the_server_stops_after_the_idle_timeout ... ok
test websrv::tests::a_request_without_a_token_does_not_postpone_the_timeout ... ok

test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.56s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 69 tests
test bench::tests::a_failed_measurement_has_no_speed ... ok
test bench::tests::fastest_ignores_failures ... ok
test bench::tests::a_zero_duration_does_not_divide_by_zero ... ok
test bench::tests::fastest_of_nothing_is_nothing ... ok
test bench::tests::speed_is_bytes_over_seconds ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::parses_connect ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::splits_host_and_port ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok
test log::tests::log_file_name_is_stable ... ok
test http::tests::truncated_input_is_an_error ... ok
test probe::tests::an_unconfigured_upstream_is_unknown_not_down ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::is_shareable_across_threads ... ok
test router::tests::set_if_changed_publishes_a_different_value ... ok
test router::tests::set_if_changed_skips_a_matching_value ... ok
test router::tests::set_if_changed_reports_exactly_one_winner_under_concurrent_writers ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test bench::tests::reported_bytes_are_the_body_not_the_headers ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
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
test connector::tests::dial_timeout_is_honoured ... ok
test probe::tests::a_changed_address_is_not_answered_from_the_old_cache ... ok
test bench::tests::every_configured_route_is_measured_and_labelled ... ok
test supervisor::tests::an_unchanged_decision_does_not_touch_the_router ... ok
test probe::tests::the_result_is_cached_within_the_ttl ... ok
test bench::tests::a_dead_upstream_yields_an_error_not_a_hang ... ok
test supervisor::tests::run_reevaluates_on_start_and_on_each_event_then_exits_when_the_channel_closes ... ok
test probe::tests::a_live_listener_is_up_and_a_closed_port_is_down ... ok
test connector::tests::refused_upstream_reports_error ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok

test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s

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
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test config::tests::defaults_match_the_spec ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test config::tests::matching_is_case_insensitive ... ok
test config::tests::managing_the_system_proxy_is_on_by_default_and_switchable ... ok
test config::tests::no_network_at_all_is_not_office ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::the_name_never_decides_anything ... ok
test config::tests::load_from_a_missing_file_yields_defaults ... ok
test config::tests::place_is_office_when_a_connected_network_matches ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test config::tests::the_saved_system_proxy_survives_a_roundtrip ... ok
test config::tests::place_is_not_office_for_an_unknown_network ... ok
test config::tests::several_connected_networks_office_wins ... ok
test config::tests::validate_accepts_the_defaults ... ok
test config::tests::validate_rejects_a_malformed_upstream ... ok
test config::tests::validate_rejects_a_port_below_the_privileged_range ... ok
test config::tests::validate_rejects_an_absurd_connection_limit ... ok
test config::tests::validate_rejects_an_office_network_with_empty_id ... ok
test config::tests::without_configured_offices_nothing_is_office ... ok
test config::tests::validate_rejects_a_zero_connection_limit ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test config::tests::config_path_matches_what_the_spec_promises ... ok
test config::tests::save_then_load_roundtrips_through_a_real_file ... ok

test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-b921d6d1fd7e845d.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test events::tests::the_log_line_names_every_combination_of_armed_channels ... ok
test networks::tests::category_maps_every_documented_value ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test events::tests::a_burst_collapses_to_its_first_and_last_event ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test events::tests::the_trailing_event_is_the_last_one_of_the_burst ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test events::tests::dropping_the_debounced_receiver_releases_the_source ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
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

```

Прогон тестового двоичного файла приложения
(`proxypilot-1e1afdb6b3b21ba1.exe`) — **2,59 с**; в нём 92 теста (было 89).
Всего по репозиторию 233 проходят и 1 `#[ignore]`.

### `cargo clippy --all-targets -- -D warnings`

Крейты проекта предварительно вычищены (`cargo clean -p proxypilot-app -p
proxypilot-bridge -p proxypilot-core -p proxypilot-winnet`), чтобы вывод был
честным, а не «Finished» по кэшу. Ни одного `#[allow]` не добавлено:
единственная находка (`is_none_or` стабилен с 1.82, а MSRV проекта — 1.75)
исправлена заменой на `map_or`.

```text
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.52s
```

### `cargo fmt --all --check`

Пустой вывод, код возврата 0.

```text
```


---
---

# Круг правок 1 — по ревью

**Коммит:** `93bee1d` — fix(win): правило порта проверялось слоем ниже того, где применяется
**Тесты после правки:** 236 проходят + 1 `#[ignore]` (было 233 + 1). Прогон тестового двоичного файла приложения — **2,65 с**.

## FINDING 1 (Important) — исправлено

Замечание верное, и оно бьёт в самое важное место. Оба теста на порт
проверяли `live_config` как чистую функцию:
`the_live_config_keeps_the_port_the_bridge_is_bound_to` звал её напрямую, а
websrv-тест шёл через `spawn_supervisor_stub`, который её не вызывает вовсе
(его третье утверждение само переспрашивало `live_config` у захваченного
конфига, а четвёртое читало неподвижный `app.port`). То есть вычеркнув
`settings_page::live_config(&saved, port)` из `main.rs`, можно было оставить
все 233 теста зелёными — и получить `AppState.port`, называющий порт, на
котором никто не слушает, а с ним соврали бы заголовок меню, «скопировать
адрес» и проба порта в диагностике.

Тело витка вынесено в функцию, которой правило и принадлежит:

```rust
enum Change {
    Mode(Mode),
    Whole(Config),
}

fn apply_change(saved: &mut Config, change: Change, bound_port: u16) -> Config {
    match change {
        Change::Mode(mode) => saved.mode = mode,
        Change::Whole(next) => *saved = next,
    }
    settings_page::live_config(saved, bound_port)
}
```

Цикл теперь зовёт её и отдаёт супервизору её результат:

```rust
if let Some(change) = change {
    let live = apply_change(&mut saved, change, port);
    outcome = saved.save().map_err(|e| e.to_string());
    ...
    supervisor = new_supervisor(&router, &live);
    saved_config.store(Arc::new(saved.clone()));
}
```

Три новых теста в `main.rs`:

- `a_port_change_does_not_reach_the_config_the_supervisor_gets` — на диск
  ложится введённое значение, супервизор остаётся на привязанном порту;
- `everything_except_the_port_does_reach_the_supervisor` — обратная половина
  правила, без которой его выполнила бы и функция, не пропускающая ничего;
- `switching_the_mode_does_not_smuggle_a_pending_port_change_through` —
  человек сменил порт и не перезапустился; переключение режима в трее не
  протаскивает отложенную смену «заодно».

**Поправка к этому абзацу (внесена в круге правок 2).** Я подал третий тест
как найденную дыру. Дырой он не был: и ДО правки цикл звал
`live_config(&saved, port)` в единственной ветке `config_changed` — общей
для `Cmd::SetMode` и `Cmd::ApplyConfig`, — а `live_config` переписывает
`bridge_port` безусловно. Отложенная смена порта не могла проехать на
переключении режима и раньше: правило применялось единообразно. Тест
остаётся и полезен, но он РЕГРЕССИОННЫЙ — закрепляет уже безопасный путь,
чтобы разделение этих веток в будущем не открыло его молча.

### Проверено мутацией, а не на слово

**Мутация 1** — ровно то, что описало ревью: убрать правило из точки
применения (`settings_page::live_config(saved, bound_port)` → `saved.clone()`).
Падают все три новых теста; вывод `cargo test -p proxypilot-app` дословно:

```text
warning: unused variable: `bound_port`
   --> crates\app\src\main.rs:540:53
    |
540 | fn apply_change(saved: &mut Config, change: Change, bound_port: u16) -> Config {
    |                                                     ^^^^^^^^^^ help: if this is intentional, prefix it with an underscore: `_bound_port`
    |
    = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `proxypilot-app` (bin "proxypilot" test) generated 1 warning (run `cargo fix --bin "proxypilot" -p proxypilot-app --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.15s
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 95 tests
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free ... ok
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::an_ordinary_relaunch_trips_neither_bridge_check ... ok
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::a_sysproxy_read_failure_is_reported_once_not_as_two_failures ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
test doctor::tests::no_stale_pointer_when_the_registry_points_elsewhere ... ok
test doctor::tests::no_recognised_network_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::seven_rows_come_back_every_time ... ok
test doctor::tests::sysproxy_check_is_skipped_gracefully_when_management_is_off ... ok
test doctor::tests::sysproxy_pointing_at_us_is_ok ... ok
test doctor::tests::sysproxy_pointing_elsewhere_is_a_warning_when_we_manage_it ... ok
test doctor::tests::the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine ... ok
test doctor::tests::upstreams_check_is_ok_when_nothing_is_configured ... ok
test doctor::tests::the_office_networks_check_does_not_apply_to_a_pinned_mode ... ok
test icons::tests::a_deliberate_direct_mode_is_not_unconfigured ... ok
test icons::tests::icon_reflects_the_active_route ... ok
test icons::tests::nothing_configured_gets_its_own_icon ... ok
test proxy::tests::a_disabled_pointer_at_our_address_is_not_stale ... ok
test icons::tests::every_icon_is_a_full_rgba_buffer ... ok
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
test settings_page::tests::html_metacharacters_are_escaped ... ok
test settings_page::tests::everything_rendered_into_the_page_is_escaped ... ok
test settings_page::tests::repeated_fields_keep_their_order ... ok
test settings_page::tests::the_autostart_toggle_says_it_is_not_wired_yet_instead_of_pretending ... ok
test settings_page::tests::the_form_does_not_touch_the_fields_it_does_not_own ... ok
test settings_page::tests::the_live_config_keeps_the_port_the_bridge_is_bound_to ... ok
test settings_page::tests::the_page_offers_the_office_button_only_when_a_network_is_known ... ok
test settings_page::tests::the_page_says_the_port_needs_a_restart ... ok
test settings_page::tests::the_page_shows_both_upstreams_with_their_availability ... ok
test tests::a_port_change_does_not_reach_the_config_the_supervisor_gets ... FAILED
test tests::everything_except_the_port_does_reach_the_supervisor ... FAILED
test tests::switching_the_mode_does_not_smuggle_a_pending_port_change_through ... FAILED
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
test websrv::tests::a_request_without_the_token_is_not_found ... ok
test websrv::tests::an_opaque_origin_is_rejected ... ok
test websrv::tests::an_unknown_path_under_a_valid_token_is_not_found ... ok
test websrv::tests::a_foreign_host_header_is_not_found ... ok
test websrv::tests::a_state_changing_request_from_a_foreign_origin_is_rejected ... ok
test websrv::tests::a_truncated_token_is_not_found ... ok
test websrv::tests::a_state_changing_request_without_any_origin_is_rejected ... ok
test websrv::tests::a_wrong_token_is_not_found ... ok
test websrv::tests::an_invalid_value_shows_the_message_config_validate_returned ... ok
test websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing ... ok
test websrv::tests::every_session_gets_its_own_token ... ok
test websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel ... ok
test websrv::tests::the_listener_is_on_loopback ... ok
test websrv::tests::changing_only_the_port_does_not_rebind_the_listener ... ok
test websrv::tests::the_token_comparison_is_length_and_content_sensitive ... ok
test websrv::tests::our_own_page_may_post ... ok
test websrv::tests::the_right_token_serves_the_page ... ok
test websrv::tests::the_query_string_does_not_hide_the_token ... ok
test websrv::tests::the_office_button_prefills_the_current_network_guid ... ok
test websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page ... ok
test websrv::tests::a_token_from_a_previous_session_is_not_found ... ok
test websrv::tests::the_number_of_simultaneous_connections_is_capped ... ok
test websrv::tests::activity_postpones_the_idle_timeout ... ok
test websrv::tests::the_diagnostics_button_shows_its_output_in_place ... ok
test websrv::tests::stopping_closes_the_door ... ok
test websrv::tests::dropping_the_handle_closes_the_door ... ok
test websrv::tests::the_server_stops_after_the_idle_timeout ... ok
test websrv::tests::a_request_without_a_token_does_not_postpone_the_timeout ... ok

failures:

---- tests::a_port_change_does_not_reach_the_config_the_supervisor_gets stdout ----

thread 'tests::a_port_change_does_not_reach_the_config_the_supervisor_gets' (14908) panicked at crates\app\src\main.rs:889:9:
assertion `left == right` failed: супервизор обязан остаться на привязанном порту
  left: 3999
 right: 3129
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- tests::everything_except_the_port_does_reach_the_supervisor stdout ----

thread 'tests::everything_except_the_port_does_reach_the_supervisor' (27360) panicked at crates\app\src\main.rs:918:9:
assertion `left == right` failed
  left: 3999
 right: 3129

---- tests::switching_the_mode_does_not_smuggle_a_pending_port_change_through stdout ----

thread 'tests::switching_the_mode_does_not_smuggle_a_pending_port_change_through' (23696) panicked at crates\app\src\main.rs:933:9:
assertion `left == right` failed: а порт — нет
  left: 3999
 right: 3129


failures:
    tests::a_port_change_does_not_reach_the_config_the_supervisor_gets
    tests::everything_except_the_port_does_reach_the_supervisor
    tests::switching_the_mode_does_not_smuggle_a_pending_port_change_through

test result: FAILED. 92 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.60s

error: test failed, to rerun pass `-p proxypilot-app --bin proxypilot`
```

**Мутация 2** — обойти функцию, отдав супервизору `saved` напрямую
(`new_supervisor(&router, &live)` → `new_supervisor(&router, &saved)`). Тесты
до этого дела не доходят: у `apply_change` ровно один вызывающий, и обход
делает `live` неиспользуемой переменной, а `-D warnings` в CI этого не
терпит. Вывод `cargo clippy --all-targets -- -D warnings` дословно:

```text
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
error: unused variable: `live`
   --> crates\app\src\main.rs:416:25
    |
416 |                     let live = apply_change(&mut saved, change, port);
    |                         ^^^^ help: if this is intentional, prefix it with an underscore: `_live`
    |
    = note: `-D unused-variables` implied by `-D warnings`
    = help: to override `-D warnings` add `#[allow(unused_variables)]`

error: could not compile `proxypilot-app` (bin "proxypilot") due to 1 previous error
warning: build failed, waiting for other jobs to finish...
error: could not compile `proxypilot-app` (bin "proxypilot" test) due to 1 previous error
```

Обе лазейки закрыты: одна тестами, вторая — тем, что у правила ровно один
вызывающий и обойти его молча нельзя.

## Записано, а не исправлено

**Отклонение по объёму: «всё остальное применяется немедленно» выполнено для
четырёх полей формы из шести.** Немедленно применяются оба апстрима и список
офисных сетей (те, что определяют маршрут) — через канал `Cmd` в супервизор.
`no_proxy` и `manage_system_proxy` сохраняются, но вступают в силу при
перезапуске, и страница говорит об этом словами, по которым видно, что
делать: «Применяется при запуске: после изменения перезапустите ProxyPilot».
Это отклонение объявленное, а не молчаливое.

Причины (подробнее — в разделе «Осознанные ограничения» выше): bypass живёт в
`serve::Shared.bypass: Arc<BypassList>`, и оживить его — это `ArcSwap` в
структуре крейта моста, то есть за пределами файлов задачи;
`manage_system_proxy` живым сделать нельзя, не сделав `RestoreOnDrop`
безусловным, а его глобальные ограничения запрещают трогать. Передано
владельцу плана: bypass-список — кандидат на отдельную маленькую задачу,
`serve::Shared` может держать его в `ArcSwap` ровно так же, как уже держит
`Router`.

**Пробел в красном прогоне.** Три теста из списка добавленных появились
ПОСЛЕ красного прогона, поэтому в нём их нет, а в зелёном есть:

- `a_failed_route_is_shown_as_failed_not_omitted`
- `diagnostics_output_is_shown_in_place_and_escaped`
- `the_autostart_toggle_says_it_is_not_wired_yet_instead_of_pretending`

Все три написаны после того, как `render` заработал: они проверяют вывод
замера и диагностики «на месте» и честность тумблера автозапуска — то, что я
захотел закрепить, уже увидев отрисованную страницу. Остальные 19 тестов
задачи (включая оба про порт) были в красном прогоне и падали в нём. Три
теста этого круга правок (`a_port_change_...`,
`everything_except_the_port_...`, `switching_the_mode_...`) красного прогона
тоже не имеют — вместо него у них мутационная проверка выше, которая строже:
она показывает не «тест падал, пока кода не было», а «тест падает, если
убрать именно то, что он охраняет».

## Minor — исправлено

Комментарий у `saved_config` в `main.rs` и у `SettingsState.config` в
`settings_page.rs` называл ячейку «конфиг, каким он лежит НА ДИСКЕ», хотя
`store` происходит и после отказа `saved.save()`. Поведение верное (правка
применена и живёт до перезапуска, а про отказ записи страница говорит
отдельной строкой) — переписан только комментарий: «каким его задал человек»,
с оговоркой про отказ записи.

## Три команды CI после правки

### `cargo test --all`

```text
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 8.19s
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 95 tests
test doctor::tests::an_ordinary_relaunch_trips_neither_bridge_check ... ok
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::a_sysproxy_read_failure_is_reported_once_not_as_two_failures ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::no_recognised_network_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
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
test icons::tests::icons_differ_from_each_other ... ok
test proxy::tests::localhost_by_name_is_ours_as_well ... ok
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
test settings_page::tests::the_page_says_the_port_needs_a_restart ... ok
test settings_page::tests::the_page_offers_the_office_button_only_when_a_network_is_known ... ok
test settings_page::tests::the_page_shows_both_upstreams_with_their_availability ... ok
test tests::a_port_change_does_not_reach_the_config_the_supervisor_gets ... ok
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
test websrv::tests::an_unknown_path_under_a_valid_token_is_not_found ... ok
test websrv::tests::a_state_changing_request_from_a_foreign_origin_is_rejected ... ok
test websrv::tests::an_opaque_origin_is_rejected ... ok
test websrv::tests::a_request_without_the_token_is_not_found ... ok
test websrv::tests::a_wrong_token_is_not_found ... ok
test websrv::tests::a_foreign_host_header_is_not_found ... ok
test websrv::tests::a_truncated_token_is_not_found ... ok
test websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel ... ok
test websrv::tests::a_state_changing_request_without_any_origin_is_rejected ... ok
test websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing ... ok
test websrv::tests::an_invalid_value_shows_the_message_config_validate_returned ... ok
test websrv::tests::every_session_gets_its_own_token ... ok
test websrv::tests::the_listener_is_on_loopback ... ok
test websrv::tests::changing_only_the_port_does_not_rebind_the_listener ... ok
test websrv::tests::the_token_comparison_is_length_and_content_sensitive ... ok
test websrv::tests::our_own_page_may_post ... ok
test websrv::tests::the_query_string_does_not_hide_the_token ... ok
test websrv::tests::the_right_token_serves_the_page ... ok
test websrv::tests::the_office_button_prefills_the_current_network_guid ... ok
test websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page ... ok
test websrv::tests::a_token_from_a_previous_session_is_not_found ... ok
test websrv::tests::the_number_of_simultaneous_connections_is_capped ... ok
test websrv::tests::activity_postpones_the_idle_timeout ... ok
test websrv::tests::the_diagnostics_button_shows_its_output_in_place ... ok
test websrv::tests::stopping_closes_the_door ... ok
test websrv::tests::dropping_the_handle_closes_the_door ... ok
test websrv::tests::the_server_stops_after_the_idle_timeout ... ok
test websrv::tests::a_request_without_a_token_does_not_postpone_the_timeout ... ok

test result: ok. 95 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.65s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 69 tests
test bench::tests::a_failed_measurement_has_no_speed ... ok
test bench::tests::fastest_of_nothing_is_nothing ... ok
test bench::tests::fastest_ignores_failures ... ok
test bench::tests::a_zero_duration_does_not_divide_by_zero ... ok
test bench::tests::speed_is_bytes_over_seconds ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::parses_connect ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::splits_host_and_port ... ok
test log::tests::filter_defaults_to_info_and_honours_the_env_var ... ok
test http::tests::truncated_input_is_an_error ... ok
test log::tests::log_file_name_is_stable ... ok
test probe::tests::an_unconfigured_upstream_is_unknown_not_down ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::is_shareable_across_threads ... ok
test router::tests::set_if_changed_publishes_a_different_value ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test bench::tests::reported_bytes_are_the_body_not_the_headers ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test router::tests::set_if_changed_skips_a_matching_value ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::set_if_changed_reports_exactly_one_winner_under_concurrent_writers ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
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
test supervisor::tests::the_network_name_reaches_the_app_state ... ok
test supervisor::tests::a_dead_pinned_upstream_is_reported_as_demoted ... ok
test probe::tests::a_silent_address_is_down_within_the_timeout ... ok
test bench::tests::an_unconfigured_upstream_is_not_measured ... ok
test probe::tests::a_changed_address_is_not_answered_from_the_old_cache ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test bench::tests::every_configured_route_is_measured_and_labelled ... ok
test probe::tests::the_result_is_cached_within_the_ttl ... ok
test probe::tests::a_live_listener_is_up_and_a_closed_port_is_down ... ok
test bench::tests::a_dead_upstream_yields_an_error_not_a_hang ... ok
test supervisor::tests::an_unchanged_decision_does_not_touch_the_router ... ok
test supervisor::tests::run_reevaluates_on_start_and_on_each_event_then_exits_when_the_channel_closes ... ok
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
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test config::tests::defaults_match_the_spec ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test config::tests::load_from_a_missing_file_yields_defaults ... ok
test config::tests::matching_is_case_insensitive ... ok
test config::tests::managing_the_system_proxy_is_on_by_default_and_switchable ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::no_network_at_all_is_not_office ... ok
test config::tests::place_is_not_office_for_an_unknown_network ... ok
test config::tests::place_is_office_when_a_connected_network_matches ... ok
test config::tests::several_connected_networks_office_wins ... ok
test config::tests::the_name_never_decides_anything ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::validate_accepts_the_defaults ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test config::tests::validate_rejects_a_malformed_upstream ... ok
test config::tests::the_saved_system_proxy_survives_a_roundtrip ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test config::tests::validate_rejects_a_port_below_the_privileged_range ... ok
test config::tests::validate_rejects_a_zero_connection_limit ... ok
test config::tests::validate_rejects_an_office_network_with_empty_id ... ok
test config::tests::without_configured_offices_nothing_is_office ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test config::tests::validate_rejects_an_absurd_connection_limit ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test config::tests::config_path_matches_what_the_spec_promises ... ok
test config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ... ok
test config::tests::save_then_load_roundtrips_through_a_real_file ... ok

test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-b921d6d1fd7e845d.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test networks::tests::category_maps_every_documented_value ... ok
test events::tests::dropping_the_debounced_receiver_releases_the_source ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test events::tests::a_burst_collapses_to_its_first_and_last_event ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test events::tests::the_trailing_event_is_the_last_one_of_the_burst ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test events::tests::the_log_line_names_every_combination_of_armed_channels ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
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

```

### `cargo clippy --all-targets -- -D warnings`

Крейты проекта предварительно вычищены (`cargo clean -p ...`).

```text
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.89s
```

### `cargo fmt --all --check`

Пустой вывод, код возврата 0.

```text
```


---
---

# Круг правок 2 — по ревью

**Коммит:** `59118a9` — fix(win): гарантия обхода apply_change держалась только на одной форме обхода
**Тесты:** 236 проходят + 1 `#[ignore]` (без изменений — правка не про тесты, а про то, что ловит компилятор). Прогон тестового двоичного файла приложения — **2,60 с**.

Оба замечания верны, и второе из них — про мой собственный текст, а не про код.

## FINDING 2 (fix-introduced, accuracy) — исправлено, а не смягчено

Я написал в докблоке `apply_change`, что обойти функцию «молча не выйдет».
Это было верно ровно для той мутации, которую я и проверил: заменить `&live`
на `&saved` в вызове супервизора. Ревью право: обойти её можно естественнее —
взять побочный эффект и выбросить результат.

```rust
apply_change(&mut saved, change, port);   // побочный эффект взят, результат выброшен
...
supervisor = new_supervisor(&router, &saved);
```

`Config` не помечен `#[must_use]`, поэтому такая форма компилировалась без
единого замечания. Причём это ровно тот способ, каким «убирают лишнюю
абстракцию» на самом деле, — то есть дыра открылась бы молча, под
комментарием, обещающим обратное.

**Проверено, что премисса ревью верна.** Та же мутация при снятом атрибуте:
`cargo clippy --all-targets -- -D warnings`, код возврата **0**, вывод
дословно:

```text
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.75s
```

**Исправление.** На `apply_change` поставлен `#[must_use]` с причиной прямо в
атрибуте — не «переменная не используется», а что именно стоит на кону:

```rust
#[must_use = "это ЖИВОЙ конфиг; выбросив его и отдав супервизору сохранённый, переедешь мост на новый порт на лету"]
fn apply_change(saved: &mut Config, change: Change, bound_port: u16) -> Config {
```

Докблок переписан так, чтобы обещание стало правдой: он теперь называет ОБЕ
формы обхода и говорит, что первую ловит `unused_variables`, а вторую —
`unused_must_use`, и без атрибута вторая не ловилась бы вовсе.

**Проверено мутацией.** Та же мутация с атрибутом: `cargo clippy
--all-targets -- -D warnings`, код возврата **101**, вывод дословно:

```text
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
error: unused return value of `apply_change` that must be used
   --> crates\app\src\main.rs:416:21
    |
416 |                     apply_change(&mut saved, change, port);
    |                     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    |
    = note: это ЖИВОЙ конфиг; выбросив его и отдав супервизору сохранённый, переедешь мост на новый порт на лету
    = note: `-D unused-must-use` implied by `-D warnings`
    = help: to override `-D warnings` add `#[allow(unused_must_use)]`
help: use `let _ = ...` to ignore the resulting value
    |
416 |                     let _ = apply_change(&mut saved, change, port);
    |                     +++++++

error: could not compile `proxypilot-app` (bin "proxypilot") due to 1 previous error
warning: build failed, waiting for other jobs to finish...
error: could not compile `proxypilot-app` (bin "proxypilot" test) due to 1 previous error
```

Заметьте note в выводе: `-D warnings` не просто валит сборку, а называет
цену — переезд моста на новый порт на лету. Тому, кто это увидит, не придётся
идти читать докблок.

Итого правило порта охраняют три независимые вещи:

| Форма обхода | Что ловит |
|---|---|
| убрать `live_config` из тела `apply_change` | три теста в `main.rs` (круг правок 1) |
| отдать супервизору `saved` вместо `live` | `unused_variables` + `-D warnings` |
| выбросить результат `apply_change` | `unused_must_use` + `-D warnings` (эта правка) |

## FINDING 3 (accuracy, report only) — поправлено в отчёте

Ревью право, и это моя ошибка в тексте, а не в коде. Я подал
`switching_the_mode_does_not_smuggle_a_pending_port_change_through` как
третью найденную дыру. Дырой она не была: и ДО правки цикл звал
`live_config(&saved, port)` в единственной ветке `config_changed` — общей для
`Cmd::SetMode` и `Cmd::ApplyConfig`, — а `live_config` переписывает
`bridge_port` безусловно. Значит, отложенная смена порта не могла проехать на
переключении режима в трее и раньше: правило применялось единообразно.

Тест остаётся — он полезен, — но описан теперь верно: это РЕГРЕССИОННЫЙ тест,
закрепляющий уже безопасный путь, чтобы разделение этих двух веток в будущем
не открыло его молча. Абзац в разделе «Круг правок 1» выше исправлен на
месте, с пометкой, что это поправка.

Приписывать себе найденную дыру, которой не было, — ровно тот сорт неточности,
из-за которого перестают верить остальному отчёту. Замечание принято целиком.

## Три команды CI после правки

### `cargo test --all`

```text
   Compiling proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.90s
     Running unittests src\main.rs (target\debug\deps\proxypilot-1e1afdb6b3b21ba1.exe)

running 95 tests
test doctor::tests::a_sysproxy_read_failure_is_reported_once_not_as_two_failures ... ok
test doctor::tests::a_live_configured_upstream_is_ok ... ok
test doctor::tests::a_dead_configured_upstream_fails_the_check ... ok
test doctor::tests::a_sysproxy_read_failure_fails_that_check ... ok
test doctor::tests::a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line ... ok
test doctor::tests::a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free ... ok
test doctor::tests::no_office_networks_configured_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::an_unprobed_upstream_is_only_a_warning ... ok
test doctor::tests::an_unrecognised_network_in_auto_mode_is_a_warning ... ok
test doctor::tests::at_least_one_office_network_makes_that_check_pass ... ok
test doctor::tests::bridge_listening_is_ok_when_the_port_answers ... ok
test doctor::tests::network_recognition_does_not_apply_to_a_pinned_mode ... ok
test doctor::tests::an_office_network_in_auto_mode_is_ok ... ok
test doctor::tests::an_ordinary_relaunch_trips_neither_bridge_check ... ok
test doctor::tests::no_recognised_network_at_all_is_a_warning_in_auto_mode ... ok
test doctor::tests::no_listener_on_the_port_is_the_loudest_failure ... ok
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
test icons::tests::every_icon_is_a_full_rgba_buffer ... ok
test proxy::tests::our_address_on_another_port_is_not_ours ... ok
test icons::tests::icons_differ_from_each_other ... ok
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
test settings_page::tests::the_page_shows_both_upstreams_with_their_availability ... ok
test tests::a_port_change_does_not_reach_the_config_the_supervisor_gets ... ok
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
test websrv::tests::a_state_changing_request_without_any_origin_is_rejected ... ok
test websrv::tests::a_truncated_token_is_not_found ... ok
test websrv::tests::a_wrong_token_is_not_found ... ok
test websrv::tests::a_state_changing_request_from_a_foreign_origin_is_rejected ... ok
test websrv::tests::an_opaque_origin_is_rejected ... ok
test websrv::tests::an_unknown_path_under_a_valid_token_is_not_found ... ok
test websrv::tests::a_request_without_the_token_is_not_found ... ok
test websrv::tests::a_valid_change_reaches_the_supervisor_through_the_command_channel ... ok
test websrv::tests::a_referer_from_our_own_page_is_accepted_when_origin_is_missing ... ok
test websrv::tests::an_invalid_value_shows_the_message_config_validate_returned ... ok
test websrv::tests::every_session_gets_its_own_token ... ok
test websrv::tests::the_listener_is_on_loopback ... ok
test websrv::tests::the_token_comparison_is_length_and_content_sensitive ... ok
test websrv::tests::changing_only_the_port_does_not_rebind_the_listener ... ok
test websrv::tests::our_own_page_may_post ... ok
test websrv::tests::the_query_string_does_not_hide_the_token ... ok
test websrv::tests::the_right_token_serves_the_page ... ok
test websrv::tests::the_office_button_prefills_the_current_network_guid ... ok
test websrv::tests::values_with_html_metacharacters_are_escaped_in_the_page ... ok
test websrv::tests::a_token_from_a_previous_session_is_not_found ... ok
test websrv::tests::the_number_of_simultaneous_connections_is_capped ... ok
test websrv::tests::activity_postpones_the_idle_timeout ... ok
test websrv::tests::the_diagnostics_button_shows_its_output_in_place ... ok
test websrv::tests::stopping_closes_the_door ... ok
test websrv::tests::dropping_the_handle_closes_the_door ... ok
test websrv::tests::the_server_stops_after_the_idle_timeout ... ok
test websrv::tests::a_request_without_a_token_does_not_postpone_the_timeout ... ok

test result: ok. 95 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.60s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-620b032c4470b356.exe)

running 69 tests
test bench::tests::a_zero_duration_does_not_divide_by_zero ... ok
test bench::tests::a_failed_measurement_has_no_speed ... ok
test bench::tests::fastest_ignores_failures ... ok
test bench::tests::fastest_of_nothing_is_nothing ... ok
test bench::tests::speed_is_bytes_over_seconds ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
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
test router::tests::set_if_changed_reports_exactly_one_winner_under_concurrent_writers ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test bench::tests::reported_bytes_are_the_body_not_the_headers ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test serve::tests::plain_http_through_http_upstream_uses_absolute_form_not_connect ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test socks5::tests::surfaces_refusal_code ... ok
test supervisor::tests::in_the_office_with_a_live_socks_the_route_becomes_socks ... ok
test supervisor::tests::outside_the_office_the_route_is_direct_even_with_a_live_upstream ... ok
test supervisor::tests::the_network_name_reaches_the_app_state ... ok
test probe::tests::a_silent_address_is_down_within_the_timeout ... ok
test supervisor::tests::a_dead_pinned_upstream_is_reported_as_demoted ... ok
test bench::tests::an_unconfigured_upstream_is_not_measured ... ok
test probe::tests::a_changed_address_is_not_answered_from_the_old_cache ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test bench::tests::every_configured_route_is_measured_and_labelled ... ok
test probe::tests::the_result_is_cached_within_the_ttl ... ok
test probe::tests::a_live_listener_is_up_and_a_closed_port_is_down ... ok
test bench::tests::a_dead_upstream_yields_an_error_not_a_hang ... ok
test supervisor::tests::run_reevaluates_on_start_and_on_each_event_then_exits_when_the_channel_closes ... ok
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
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test config::tests::defaults_match_the_spec ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test config::tests::load_from_a_missing_file_yields_defaults ... ok
test config::tests::matching_is_case_insensitive ... ok
test config::tests::no_network_at_all_is_not_office ... ok
test config::tests::place_is_not_office_for_an_unknown_network ... ok
test config::tests::managing_the_system_proxy_is_on_by_default_and_switchable ... ok
test config::tests::place_is_office_when_a_connected_network_matches ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::several_connected_networks_office_wins ... ok
test config::tests::the_name_never_decides_anything ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test config::tests::the_saved_system_proxy_survives_a_roundtrip ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::validate_accepts_the_defaults ... ok
test config::tests::validate_rejects_a_malformed_upstream ... ok
test config::tests::validate_rejects_a_port_below_the_privileged_range ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test config::tests::without_configured_offices_nothing_is_office ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test config::tests::validate_rejects_a_zero_connection_limit ... ok
test config::tests::validate_rejects_an_absurd_connection_limit ... ok
test config::tests::validate_rejects_an_office_network_with_empty_id ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok
test config::tests::load_from_an_invalid_file_is_an_error_not_a_panic ... ok
test config::tests::config_path_matches_what_the_spec_promises ... ok
test config::tests::save_then_load_roundtrips_through_a_real_file ... ok

test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_winnet-b921d6d1fd7e845d.exe)

running 23 tests
test events::tests::watch_a_real_network_change ... ignored, нужна живая сеть: переключить Wi-Fi руками
test networks::tests::category_maps_every_documented_value ... ok
test events::tests::the_log_line_names_every_combination_of_armed_channels ... ok
test sysproxy::tests::bypass_string_converts_dot_suffix_to_wildcard ... ok
test networks::tests::guid_with_leading_zeros_keeps_fixed_field_widths ... ok
test sysproxy::tests::bypass_string_skips_empty_entries ... ok
test events::tests::the_trailing_event_is_the_last_one_of_the_burst ... ok
test events::tests::dropping_the_debounced_receiver_releases_the_source ... ok
test events::tests::a_burst_collapses_to_its_first_and_last_event ... ok
test events::tests::closing_the_source_closes_the_output ... ok
test sysproxy::tests::bypass_string_does_not_duplicate_an_existing_local_token ... ok
test networks::tests::guid_is_formatted_in_the_canonical_braced_form ... ok
test sysproxy::tests::bypass_string_skips_a_bare_dot ... ok
test sysproxy::tests::bypass_string_uses_semicolons_and_keeps_local_token ... ok
test sysproxy::tests::decoding_drops_the_terminating_nul ... ok
test sysproxy::tests::reg_sz_bytes_end_with_a_utf16_nul ... ok
test sysproxy::tests::reg_sz_bytes_of_an_empty_string_are_just_the_nul ... ok
test sysproxy::tests::reading_current_settings_does_not_fail ... ok
test com::tests::a_guard_created_on_a_bare_thread_owns_its_uninit ... ok
test com::tests::a_second_guard_on_the_same_thread_still_owns_its_uninit ... ok
test com::tests::a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit ... ok
test networks::tests::listing_connected_networks_does_not_fail_on_a_real_machine ... ok
test events::tests::events_further_apart_than_the_window_both_pass ... ok

test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.12s

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

### `cargo clippy --all-targets -- -D warnings`

Крейты проекта предварительно вычищены (`cargo clean -p ...`).

```text
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\winnet)
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.32s
```

### `cargo fmt --all --check`

Пустой вывод, код возврата 0.

```text
```
