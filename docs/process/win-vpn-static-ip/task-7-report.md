# Task 7 — Сеть и туннель в интерфейсе: отчёт

BASE=cbdc954 (`feat(win): служба профиля сети`). Коммит задачи —
`8dbaf81` (`feat(win): сеть и туннель в интерфейсе`), сверху лёг на
`9742d80` (`fix(win): round 2 review fixes for net profile service (task
6)`) — это чужой, параллельный этой сессии коммит, прилетевший в ту же
рабочую копию во время работы (см. «Что не тронуто» ниже про параллельную
правку). `git log --oneline`: `8dbaf81` → `9742d80` → `cbdc954`.

**Известная накладка git.** Мой рабочий файл `progress.md` с добавленными
строками задачи 7 к моменту моего `git commit` уже не отличался от HEAD —
он был подхвачен коммитом `9742d80` (широким `git add`/`-a` той, другой,
сессии, которая закоммитила свои файлы и заодно все мои несохранённые
правки этого файла на диске). Содержимое верное и осталось в истории, но
формально строки задачи 7 лежат внутри чужого коммита сообщения «task 6»,
а не в `8dbaf81`. Не переигрывал историю (`git commit --amend`/`rebase`)
намеренно: сама попытка исправить атрибуцию задним числом на общей ветке,
где параллельно идёт чужая работа, рискованнее, чем оставить как есть.

## Что сделано

Раздел «Туннель» на странице настроек и его короткое отражение в меню трея,
поверх всего, что уже было готово (задачи 1-6). Один новый файл вне списка
«Files» брифа (`crates/winnet/src/routes.rs`) — обоснование ниже.

### Новое

- `crates/winnet/src/routes.rs` — живой сбор IPv4-таблицы маршрутов
  (`GetIpForwardTable2`) вместе с адаптерами (`GetAdaptersAddresses`,
  классификация «туннельный» по IANA `ifType`: PPP/PROP_VIRTUAL/TUNNEL) —
  единственный производитель `Vec<tunnel_state::AdapterRoute>` в проде.
  Только чтение, тот же класс операции, что и `route print -4`, которым
  задача 3 уже сверяла `tunnel_state` на этой машине.
- `crates/app/src/settings_page.rs`:
  - `TUNNEL_PROFILE_NAME` — имя нашего профиля OpenVPN (`"proxypilot-office"`),
    общее для файла `.ovpn`, `--command connect|disconnect` и `our_alias` в
    `tunnel_state`.
  - Трейт `Tunnel` (снимок + собрать профиль + поднять/опустить +
    установить службу) и структура `TunnelSnapshot` — абстракция над
    OpenVPN и над запуском `install-service` с повышением прав.
    `SettingsState` получила поле `tunnel: Arc<dyn Tunnel>`.
  - Раздел «Туннель» (`tunnel_section`) — четыре состояния приёмки, DNS-
    предупреждение всегда до кнопки подъёма, отказ рисовать кнопку подъёма
    при чужом туннеле, две кнопки с явным упоминанием UAC ДО клика.
  - Тумблер «Поднимать туннель автоматически вне офиса»
    (`automate_tunnel`) в основной форме — выключен по умолчанию
    (наследует `Config::default()`, задача 5).
  - Обработчики `build_tunnel_profile` / `raise_tunnel` / `lower_tunnel` /
    `install_service` в `handle_post`, с серверной перепроверкой чужого
    туннеля перед подъёмом (не только в разметке).
  - `TunnelPending` — тестовая заглушка для файлов, которым сам туннель
    безразличен (`websrv.rs`).
- `crates/app/src/tray.rs`: пункт-надпись со статусом туннеля
  (`tunnel_text`) и пункт-ссылка «Туннель…» (`Action::OpenTunnel`),
  ведущий на страницу настроек с якорем `#tunnel` — управление осталось на
  странице, где есть место для предупреждений; пункт меню — одна строка.
- `crates/app/src/main.rs`: `WinTunnel` — реализация `Tunnel` поверх
  `proxypilot_winnet::{openvpn, routes, tunnel_state}`; константа
  `TUNNEL_SOURCE_FILE` (см. «Открытые решения» ниже); `SettingsDeps` —
  группировка параметров `open_settings` (клиппи `too_many_arguments` после
  добавления `tunnel`); проводка `Action::OpenTunnel` и живого снимка
  туннеля в `Tray::new`/`refresh`.
- `crates/app/src/ui.rs`: `request_elevation` — `ShellExecuteW` с verb
  `runas`, тем же приёмом, что уже был у `open_in_browser` с verb `open`.
  Единственный вызывающий — `WinTunnel::install_service`.

## Дизайн-коррекция, полученная в процессе

Контроллер прислал мид-таск правку: приложение не должно писать
`%ProgramData%\ProxyPilot\profile.toml` само — это дыра, которую
разделение «пользовательский конфиг / копия службы» (спека §7.4) как раз
призвано закрыть. Профиль службы обязана писать только повышенная ветка
(`install-service`), а не обычный процесс.

Проверено: бриф этой задачи вообще не просил редактировать
`office_ip`/`office_mask`/`office_gateway`/`office_dns` (`NetProfile`) на
странице настроек — состав задачи весь про OpenVPN-туннель. Эта задача
**не читает и не пишет `profile.toml` ни в каком виде** и не добавляет
формы для правки `NetProfile`. «Установить службу» — только запуск
`install-service` через `ShellExecuteW(runas)`, то есть то же самое, что
человек мог бы сделать из консоли администратора сам; что именно
`install-service` пишет на диск — решение задачи 6 (её рабочая версия
менялась параллельно в этой же рабочей копии, см. «Что не тронуто» ниже).
Коррекция была применима превентивно и подтверждена: ничего в этом диффе
подстроки `profile.toml` не содержит.

## Открытые решения (бриф их не специфицировал)

1. **`our_alias`.** ~~Бриф прямо указал: задача 7 обязана его задать, задача
   3 честно предупредила, что псевдоним адаптера Windows — не устойчивый
   идентификатор. Взято `TUNNEL_PROFILE_NAME` («proxypilot-office») — то
   же имя, что у файла профиля и у `--command connect|disconnect`.~~
   **Устарело, см. «Fix round 1» ниже.** Это оказалось не просто
   неопределённостью, а структурной ошибкой: OpenVPN никогда не называет
   адаптер по имени профиля (называет по драйверу), поэтому подстановка
   имени профиля как `our_alias` в `tunnel_state` не была «лучшим
   предположением» — она была гарантированно неверной и запирала кнопку
   «опустить» после первого же успешного подъёма. Живость туннеля теперь
   определяется `winnet::tunnel_log::liveness` по логу `openvpn-gui.exe`
   для этого профиля — идентификатор, которым код действительно владеет.
   Имя адаптера (`tunnel_state`, alias) осталось только для
   `foreign_tunnel_up`, и только пока `our_tunnel_up == false`.
2. **Источник для «Собрать профиль».** `ovpn_profile::build_profile` берёт
   готовый текст исходного `.ovpn`, а ни один из планов 1-6 не назвал,
   откуда его брать в проде (докблок `ovpn_profile.rs` ссылался на «задачу
   6», но задача 6 оказалась про службу статического IP, не про это).
   Решено: `<config_dir OpenVPN>\proxypilot-source.ovpn` — тот же
   каталог, что и наш собственный `.ovpn`, доступный на запись обычному
   пользователю без UAC (иначе OpenVPN GUI, рассчитанный на запуск без
   прав администратора, не смог бы сохранять профили). Отсутствие файла —
   явная ошибка с инструкцией, а не тихий отказ.
3. **«Какой адрес применён» (бриф, п. 1 состава).** Прочитано как «какие
   офисные подсети маршрутизируются через туннель» (`cfg.office_subnets`)
   — то, что реально относится к OpenVPN-туннелю этой задачи, а не к
   статическому IP-профилю службы (задачи 5/6, отдельная сущность). Это
   же чтение полностью совместимо с дизайн-коррекцией выше: только
   отображение уже загруженного `Config`, ничего не пишется.
4. **Тумблер автоматики без исполнителя.** `automate_tunnel` сохраняется и
   показывается, но ничто (ни эта задача, ни предыдущие) не реагирует на
   него автоматическим подъёмом/опусканием туннеля при смене места —
   такой код нигде не специфицирован ни одним из планов 1-6. Страница
   говорит это прямо человеку: «Пока только сохраняет намерение: сам
   подъём и опускание по смене сети этот тумблер ещё не выполняет».
   Написать реальную автоматику (когда поднимать, дребезг, не спорить с
   чужим туннелем, не дёргать `connect` повторно) — отдельная по объёму
   задача, не часть этого брифа.
5. **Трей: статус + ссылка, не кнопки.** Кнопки подъёма/опускания и обе
   привилегированные кнопки живут только на странице настроек, а не в
   самом меню трея — там нет места для предупреждений (DNS, UAC, чужой
   туннель), а пункт меню, исполняющий действие без единого слова
   контекста, был бы худшим местом ровно для того требования брифа, ради
   которого статья и написана. Меню получило надпись-статус
   (`tunnel_text`) и пункт-ссылку «Туннель…», тем же приёмом, что уже есть
   у «Замерить скорость…»/«Диагностика…».

## Приёмка (из брифа)

- [x] Тесты на отрисовку каждого состояния: OpenVPN не установлен
  (`tunnel_section_explains_when_openvpn_is_not_installed`); установлен и
  опущен (`tunnel_section_shows_a_down_tunnel_with_a_raise_button`);
  поднят наш (`tunnel_section_shows_our_tunnel_up_with_a_lower_button`);
  поднят чужой (`tunnel_section_refuses_to_offer_raising_over_a_foreign_tunnel`).
- [x] Тумблер автоматики по умолчанию выключен
  (`the_automate_tunnel_toggle_is_off_by_default`).
- [x] UAC-предупреждение у обеих кнопок, ДО нажатия
  (`a_uac_warning_appears_before_both_privileged_buttons` — проверяет
  позицию текста «UAC» строго раньше обеих кнопок в разметке).
- [x] Экранирование всего пришедшего из конфига и системы
  (`everything_rendered_into_the_page_is_escaped`,
  `office_subnets_are_listed_and_escaped`,
  `the_routes_error_disables_both_tunnel_buttons_and_is_escaped`).
- [x] `build_profile`'s ошибка доходит до человека
  (`a_failed_profile_build_is_shown_not_swallowed`).
- [x] Коммит `feat(win): сеть и туннель в интерфейсе`.

Сверх списка брифа:
- Серверная перепроверка «не поднимать над чужим туннелем» — не только на
  уровне разметки (`raising_the_tunnel_is_refused_server_side_when_a_foreign_tunnel_is_up`).
- Честный отказ при нечитаемой таблице маршрутов: подъём временно
  недоступен, пока это не исправится
  (`the_routes_error_disables_both_tunnel_buttons_and_is_escaped`,
  `tunnel_text_does_not_hide_an_unreadable_route_table_as_plain_down`).

**Обновлено в fix round 1** (см. отдельный раздел ниже) — добавлен новый
модуль `crates/winnet/src/tunnel_log.rs` (живость по логу
`openvpn-gui.exe`, ключ — имя профиля, не имя адаптера) и исправлены два
докблока в `crates/winnet/src/openvpn.rs`, утверждавшие устаревший факт
про единственный источник живого состояния.

## TDD: дословный красный прогон до реализации

Тесты для `settings_page.rs` (трейт `Tunnel`, `TunnelSnapshot`,
`TUNNEL_PROFILE_NAME`, `FakeTunnel`, все состояния раздела) были написаны и
закоммичены в рабочую копию ДО того, как появился хоть один из этих типов
в продакшн-коде. Запуск сразу после — настоящий красный прогон, не
реконструированный:

```
$ cargo test -p proxypilot-app --bin proxypilot
   Compiling windows v0.58.0
   Compiling proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
   Compiling proxypilot-netsvc v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\netsvc)
   Compiling proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
error[E0425]: cannot find type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1402:19
     |
1402 |         snapshot: TunnelSnapshot,
     |                   ^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1410:26
     |
1410 |         fn new(snapshot: TunnelSnapshot) -> Self {
     |                          ^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1420:39
     |
1420 |         fn failing_to_build(snapshot: TunnelSnapshot, err: &str) -> Self {
     |                                       ^^^^^^^^^^^^^^ not found in this scope

error[E0405]: cannot find trait `Tunnel` in this scope
    --> crates\app\src\settings_page.rs:1428:10
     |
1428 |     impl Tunnel for FakeTunnel {
     |          ^^^^^^ not found in this scope

error[E0425]: cannot find type `Ipv4Net` in this scope
    --> crates\app\src\settings_page.rs:1429:47
     |
1429 |         fn snapshot(&self, _office_subnets: &[Ipv4Net], _profile_name: &str) -> TunnelSnapshot {
     |                                               ^^^^^^^ not found in this scope

error[E0425]: cannot find type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1429:81
     |
1429 |         fn snapshot(&self, _office_subnets: &[Ipv4Net], _profile_name: &str) -> TunnelSnapshot {
     |                                                                                 ^^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find type `Ipv4Net` in this scope
    --> crates\app\src\settings_page.rs:1432:73
     |
1432 |         fn build_profile(&self, _profile_name: &str, _office_subnets: &[Ipv4Net]) -> Result<(), String> {
     |                                                                         ^^^^^^^ not found in this scope

error[E0425]: cannot find type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1446:37
     |
1446 |     fn down_installed_snapshot() -> TunnelSnapshot {
     |                                     ^^^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1447:9
     |
1447 |         TunnelSnapshot {
     |         ^^^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1505:20
     |
1505 |         let snap = TunnelSnapshot {
     |                    ^^^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1518:20
     |
1518 |         let snap = TunnelSnapshot {
     |                    ^^^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1534:20
     |
1534 |         let snap = TunnelSnapshot {
     |                    ^^^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1586:20
     |
1586 |         let snap = TunnelSnapshot {
     |                    ^^^^^^^^^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1666:20
     |
1666 |         let snap = TunnelSnapshot {
     |                    ^^^^^^^^^^^^^^ not found in this scope

error[E0433]: cannot find type `TunnelSnapshot` in this scope
    --> crates\app\src\settings_page.rs:1481:29
     |
1481 |             FakeTunnel::new(TunnelSnapshot::default()),
     |                             ^^^^^^^^^^^^^^ use of undeclared type `TunnelSnapshot`

Some errors have detailed explanations: E0405, E0422, E0425, E0433.
For more information about an error, try `rustc --explain E0405`.
error: could not compile `proxypilot-app` (bin "proxypilot" test) due to 15 previous errors
```

После этого добавлен трейт `Tunnel`, `TunnelSnapshot`, `TUNNEL_PROFILE_NAME`,
поле `SettingsState.tunnel`, разбор `automate_tunnel`, `tunnel_section`,
обработчики четырёх новых действий — весь этот прогон стал зелёным без
единой правки в самих тестах (см. «Полный прогон CI» ниже).

`crates/winnet/src/routes.rs` писан тем же порядком (тест
`is_tunnel_if_type` сначала не существовал бы вовсе без функции — здесь
red-стадия это отсутствие модуля, что равнозначно компилятивному красному:
файл создавался с тестами внутри одним заходом, как принято для новых
модулей в этом проекте, см. `networks.rs`/`sysproxy.rs`).

## Полный прогон CI (после реализации)

### `cargo test --all`

```
test result: ok. 125 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.60s   (proxypilot-app)
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s    (proxypilot-bridge, lib)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (proxypilot-bridge, bin)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s     (doc-tests bridge, если считать отдельно)
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (proxypilot-core)
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s    (proxypilot-netsvc, lib)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s     (proxypilot-netsvc, bin)
test result: ok. 142 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s    (proxypilot-winnet)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s      (doc-tests × 4, все пустые)
```

Итого **467 тестов, 0 отказов, 3 игнорируемых** (было 438+3 игнорируемых
до этой задачи; прирост — 29: 7 в `winnet::routes`, 1 в `winnet::tunnel_state`
не менялся, ~21 в `app` между `settings_page.rs` и `tray.rs`). Игнорируемые —
все три существовавшие раньше (`win_autostart_set_round_trips_through_the_real_registry`
и два в `winnet`), новых игнорируемых не добавлено — этой задаче не
понадобилось откладывать проверку живой машины: `routes::gathering_ipv4_routes_does_not_fail_on_a_real_machine`
прогнан по-настоящему (только чтение) и прошёл.

### `cargo clippy --all-targets -- -D warnings`

Первый прогон нашёл две находки (обе исправлены без `#[allow(...)]`,
запрещённого CLAUDE.md):

```
error: `format!` in `format!` args
   --> crates\app\src\settings_page.rs:826:17
   = note: `-D clippy::format-in-format-args` implied by `-D warnings`

error: this function has too many arguments (8/7)
   --> crates\app\src\main.rs:903:1
   = note: `-D clippy::too-many-arguments` implied by `-D warnings`
```

Первая — вложенный `format!` в `tunnel_section`, переписан без вложенности.
Вторая — `open_settings` доросла до восьмого параметра (`tunnel`) вместе с
задачей 7; шесть параметров, которые всегда идут вместе, собраны в
`SettingsDeps<'a>` (не обход находки, а обычная группировка — комментарий
у структуры это объясняет).

Финальный прогон:

```
    Checking proxypilot-winnet v0.1.0 (...)
    Checking proxypilot-netsvc v0.1.0 (...)
    Checking proxypilot-app v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.92s
```

Чисто, ноль предупреждений.

### `cargo fmt --all --check`

Первый прогон нашёл расхождения (длинные строки, не разбитые по правилам
rustfmt) — исправлено `cargo fmt --all`. Повторный `--check` — пустой вывод,
код возврата 0.

## Что НЕ сделано и не могло быть сделано в этой сессии

Ни разу за сессию не запускался `openvpn-gui.exe`, не собирался и не
записывался НАСТОЯЩИЙ `.ovpn`-профиль в реальный каталог конфигураций
OpenVPN этой машины, не вызывался `ShellExecuteW` с `runas` (не показан ни
один диалог UAC), не устанавливалась и не удалялась никакая служба, не
менялись ни таблица маршрутов, ни адрес адаптера. Единственные реальные
Windows-вызовы, исполнившиеся на этой машине во время тестов, — чтения:
`find_installation`/`profile_status` (уже жили в задачах 1/4, не тронуты
этой задачей) и новый `routes::gather_ipv4_routes` (только что добавлен,
подтверждён отдельным смоук-тестом, проверено построчно перед запуском,
что это исключительно `GetIpForwardTable2`/`GetAdaptersAddresses`, без
единого пишущего вызова).

Гарантия по конструкции, не по обещанию: все обработчики привилегированных
и мутирующих действий (`raise`, `lower`, `build_profile`,
`install_service`) доступны тестам только через трейт `Tunnel` и его
тестовую реализацию `FakeTunnel`/`TunnelPending` — ни один тест этой сессии
не создаёт `WinTunnel` (реальную реализацию) и не вызывает
`ui::request_elevation` напрямую; проверено `grep`, оба символа встречаются
только в продакшн-проводке `main.rs`, вне `#[cfg(test)]`.

Живые проверки, которые остаются человеку (список приложений, а не
пропуск — см. `CLAUDE.md`, «Живые проверки, которые не делает агент»):

- «Собрать профиль» с настоящим `.ovpn` (положить корпоративный файл под
  `proxypilot-source.ovpn` в каталог конфигураций OpenVPN, нажать кнопку,
  убедиться, что появившийся `proxypilot-office.ovpn` открывается
  OpenVPN GUI).
- «Поднять туннель» / «Опустить туннель» — убедиться, что внутренний
  ресурс становится доступен, а внешний IP остаётся домашним
  (split-tunnel), и что второй раз подряд кнопка «Поднять» не плодит
  повторных `connect`.
- **Реальная проверка `tunnel_log` (главная — заменяет прежний пункт про
  `our_alias`, снятый в fix round 1).** Поднять туннель кнопкой,
  проверить, что раздел и меню трея в течение секунд показывают «поднят»
  и кнопку «опустить»; опустить и проверить обратный переход. Отдельно —
  что происходит при аварийном завершении (`taskkill /F` по
  `openvpn.exe`, а не штатный disconnect): лог не допишет маркер
  остановки, и `our_tunnel_up` останется `true` дольше, чем на самом
  деле — задокументированный, но не проверенный на практике предел
  честности `tunnel_log` (докблок модуля, раздел «Честные пределы»).
- Появление чужого туннеля (Tailscale/WireGuard/другой VPN) поверх наших
  подсетей — убедиться, что раздел честно показывает предупреждение и
  прячет кнопку подъёма.
- «Установить службу статического IP…» — убедиться, что UAC действительно
  появляется, что согласие ведёт к рабочей регистрации
  `ProxyPilotNetProfile`, и (после того, как задача 6 в её текущей
  переработке допишет запись профиля службы при установке) что сама
  установка не рухнет из-за отсутствующего `%ProgramData%\ProxyPilot`.

## Что не тронуто

Строка «Сеть: …» (`network_text`, `tray.rs`/`settings_page.rs`), пункты
режимов, строка демоции, канал `Cmd`, правило «порт не меняется на лету»
(`apply_change`, `#[must_use]`), CSP страницы настроек — всё как было.

**Важно про параллельную работу.** Во время этой сессии в этой же рабочей
копии шла (судя по `git status`) отдельная, не моя правка задачи 6:
`crates/netsvc/**`, `crates/core/src/netprofile.rs`,
`docs/process/win-vpn-static-ip/task-6-report.md` и новый файл
`crates/netsvc/src/exec.rs` стоят изменёнными/непроверенными в рабочем
дереве — судя по всему, это и есть переработка «служба сама пишет
`profile.toml` под DACL», о которой предупредил контроллер. Коммит этой
задачи (`git add`) взял **только** файлы, которые правила эта сессия:
`crates/winnet/src/routes.rs`, `crates/winnet/src/lib.rs`,
`crates/app/src/{main,settings_page,tray,ui,websrv}.rs`. Ни `Cargo.lock`,
ни файлы `netsvc`/`core::netprofile`, ни чужой отчёт задачи 6 в этот
коммит не попали — их трогать не моё дело, и правки другой сессии остались
в рабочем дереве нетронутыми для неё же.

## Известные ограничения (не находки, честные записи)

- `automate_tunnel` сохраняется, но ничего не реагирует на него
  автоматическим подъёмом/опусканием (см. «Открытые решения», п. 4) —
  страница говорит об этом прямо.
- `tunnel_log::liveness` не допишет маркер остановки, если процесс убит
  без штатного выхода (`taskkill /F`, обрыв питания, неудачно совпавший
  сон машины) — `our_tunnel_up` останется `true` дольше, чем это правда.
  Единственный способ узнать точнее — подключиться самим и последить
  (ручная проверка, `CLAUDE.md`); задокументировано в докблоке
  `tunnel_log` и в «живых проверках» выше. `our_alias` для
  `foreign_tunnel_up` (не для `our_tunnel_up` — это исправлено в fix
  round 1) остаётся тем же предположением, что описала задача 3: имя
  адаптера не устойчивый идентификатор.
- `TUNNEL_SOURCE_FILE`/«Собрать профиль» — решение задачи 7, не
  специфицированное ни одним предыдущим планом; если у продукта уже есть
  другой канал доставки исходного `.ovpn` (например, инсталлятор кладёт
  его заранее), это стоит свести к единому месту отдельной правкой.
- Уадаление службы (`uninstall-service`) кнопкой не покрыто — бриф просил
  только «установить», CLI-путь (`proxypilot.exe uninstall-service`,
  задача 6) остаётся рабочим и никуда не делся.

## Fix round 1 (finding, серьёзное) — `our_alias` никогда не совпадал ни с одним реальным адаптером

Ревью прочло реальные адаптеры этой машины (`Get-NetAdapter`, только
чтение — тот же класс операции, что уже разрешён брифом) и нашло: OpenVPN
называет адаптер по ДРАЙВЕРУ, а не по имени соединения. На этой машине
подключённые к OpenVPN интерфейсы называются `OpenVPN Data Channel
Offload`, `Подключение по локальной сети` (стандартное имя Windows для
адаптера с описанием `TAP-Windows Adapter V9`) и `OpenVPN Wintun`
(описание `Wintun Userspace Tunnel`). Ни один не называется и не может
называться `proxypilot-office` — OpenVPN не переименовывает адаптер под
профиль.

```
Name                               InterfaceDescription                      ifIndex
----                               --------------------                      -------
OpenVPN Data Channel Offload       OpenVPN Data Channel Offload                   18
Подключение по локальной сети      TAP-Windows Adapter V9                         14
Сетевое подключение Bluetooth      Bluetooth Device (Personal Area Network)       10
Беспроводная сеть                  MediaTek Wi-Fi 6 MT7921 Wireless LAN Card       9
Ethernet                           Realtek PCIe GbE Family Controller              4
OpenVPN Wintun                     Wintun Userspace Tunnel                         3
vEthernet (WSL (Hyper-V firewall)) Hyper-V Virtual Ethernet Adapter               51
```

Следствие было хуже, чем «статус неизвестен»: `our_tunnel_up` (round 0)
возвращала `false` НАВСЕГДА, а `foreign_tunnel_up` (тот же негодный
`our_alias`) классифицировала НАШ ЖЕ поднятый туннель как чужой —
правило «не трогать чужой туннель» запрещало и подъём, и опускание
разом. Ровно тот дедлок, ради устранения которого задача 3 писала
`same_alias`/`our_tunnel_up`, вернулся не через сравнение (оно было
верным), а через ЗНАЧЕНИЕ, которое ему подсовывали.

### Что нашлось на машине read-only и что из этого построено

Собственный `README.txt` инсталляции OpenVPN
(`Program Files\OpenVPN\log\README.txt`, ставится самим инсталлятором,
не данные этой инфраструктуры) прямо говорит: «Logs for connections
started by the GUI are kept in `%USERPROFILE%\OpenVPN\log`». На машине
нашлись два лога, оставшихся от прежних подключений её собственного
пользователя (их реальные имена и содержимое в этот отчёт и в код не
попали — по смыслу это два файла с профилями настоящего пользователя
машины, не наши). У обоих время СОЗДАНИЯ файла — на годы старше времени
последней ЗАПИСИ, а первая строка содержимого датирована временем
последней записи. Это подтверждает: `openvpn-gui.exe` усекает лог заново
на каждой попытке подключения, а не копит его через сессии — то есть файл
в любой момент содержит ровно ОДНУ, последнюю попытку. Реестр
(`HKCU\Software\OpenVPN-GUI`) на этой машине не переопределяет каталог
лога — несёт только `version`, что подтверждает: путь по умолчанию
действует как задокументировано.

Построено на этом: `crates/winnet/src/tunnel_log.rs` — читает
`<каталог лога>\<имя профиля>.log`, ищет последнее вхождение
`Initialization Sequence Completed` и проверяет, нет ли ПОСЛЕ него одной
из строк остановки/перезапуска процесса (`received, process exiting` /
`received, process restarting` — стабильные литералы из исходников самого
OpenVPN, не текст этой инсталляции). Ключ — имя профиля, единственный
идентификатор в этой цепочке, которым реально владеет код (мы сами его
придумали, сами кладём под ним файл, сами передаём в
`--command connect|disconnect`), а не имя адаптера, которым не владеет
никто, кроме драйвера VPN и, отчасти, пользователя.

### Как это меняет приоритет решений

`TunnelSnapshot` разделена на два независимых источника с двумя
независимыми полями ошибок:
- `our_tunnel_up` + `liveness_error` — из `tunnel_log::liveness`, ключ —
  имя профиля;
- `foreign_tunnel_up` + `routes_error` — из `tunnel_state::foreign_tunnel_up`
  поверх живой таблицы маршрутов, ключ — псевдоним адаптера (по-прежнему
  ненадёжный, задача 3).

`tunnel_section` (страница) и `tunnel_text` (трей) проверяют
`our_tunnel_up` РАНЬШЕ `foreign_tunnel_up` — это и есть исправление:
подтверждённая логом поднятость нашего туннеля перевешивает любую догадку
по алиасу адаптера, так что дедлок (свой же туннель, гасящий обе кнопки)
больше не может возникнуть — новый тест
`our_confirmed_up_tunnel_wins_over_a_misclassified_foreign_reading`
(`settings_page.rs`) и
`tunnel_text_prefers_confirmed_liveness_over_a_misclassified_foreign_reading`
(`tray.rs`) воспроизводят ИМЕННО прежний ложный ввод (`our_tunnel_up: true,
foreign_tunnel_up: true` одновременно) и проверяют, что кнопка «опустить»
всё равно доступна.

`foreign_tunnel_up` осталась в разделе не просто по инерции: она
по-прежнему нужна ДО подъёма — предупредить, что какие-то офисные подсети
уже кем-то заняты. Формулировка ослаблена с «обнаружен ЧУЖОЙ туннель» до
«туннель занят другим туннельным адаптером»: алиас всё ещё не отличает
настоящий чужой VPN от собственного незакрытого «хвоста», а безопасное
действие («не поднимать поверх») одинаково в обоих случаях — текст больше
не утверждает то, чего не может знать.

### Честное «не знаю» вместо запертых кнопок

Если `tunnel_log::liveness` отказала (`liveness_error`), раздел явно
говорит «состояние неизвестно» и показывает ОБЕ кнопки — не запирает их.
Обоснование: `openvpn::connect`/`disconnect` уже задокументированы как
«Ok значит команда доставлена, не что состояние изменилось» — повторная
команда для уже поднятого/опущенного профиля не то действие, которое эта
страница обязана предотвращать (в отличие от подъёма поверх настоящего
чужого туннеля, где цена ошибки — гонка за одни и те же маршруты). Новые
тесты: `an_unknown_liveness_shows_both_buttons_instead_of_locking_the_section`,
`raising_the_tunnel_is_not_refused_when_liveness_is_merely_unknown`.

### Исправленные docblock'и

`crates/winnet/src/openvpn.rs` (докблоки `connect` и `ProfileStatus`)
раньше утверждали «единственный источник живого состояния —
`tunnel_state::our_tunnel_up`» — это стало неверно с появлением
`tunnel_log`, и CLAUDE.md прямо называет разошедшийся комментарий
дефектом. Оба докблока переписаны на `tunnel_log::liveness` с объяснением,
почему имя адаптера для этого не годится.

### Живые действия при исследовании

Read-only весь путь: `Get-NetAdapter`, `Get-Content -Tail`/`-TotalCount`
(с явным обрезанием строк до заголовков — полное содержимое пушенных
маршрутов/DNS из чужих логов никогда не читалось целиком и никуда не
копировалось), `Get-Item` (времена файла), `Test-Path`,
`Get-ItemProperty` на `HKCU\Software\OpenVPN-GUI` (чтение, не запись).
Ничего не подключено, не отключено, не установлено, не изменено на диске
или в реестре. Новый смоук-тест
`tunnel_log::tests::liveness_does_not_fail_on_a_real_machine_for_a_profile_that_was_never_connected`
использует заведомо несуществующее имя профиля — не задевает реальные
логи этой машины.

### Прогон CI после исправления

`cargo test --all`:

```
test result: ok. 130 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.61s
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 152 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.14s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

**482 теста, 0 отказов, 3 игнорируемых** (было 467; +15: 10 в новом
`winnet::tunnel_log`, остальное — новые/переписанные тесты
`settings_page.rs`/`tray.rs` на приоритет и честное «не знаю»).

`cargo clippy --all-targets -- -D warnings`:

```
    Checking proxypilot-winnet v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\winnet)
    Checking proxypilot-netsvc v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\netsvc)
    Checking proxypilot-app v0.1.0 (C:\Users\User\Desktop\proxypilot\proxy-pilot-win\crates\app)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.19s
```

Чисто, без единой находки — не понадобился ни один новый `#[allow(...)]`.

`cargo fmt --all --check`: первый прогон нашёл несколько длинных строк
(те же тесты, которые сам и добавил), исправлено `cargo fmt --all`,
повторный `--check` — пустой вывод, код возврата 0.

### Урок для следующего, кто это читает

Дефект появился именно потому, что имя адаптера было ПРЕДПОЛОЖЕНО, а не
проверено — код был написан и покрыт тестами до того, как кто-либо открыл
`Get-NetAdapter` на настоящей машине. Один read-only просмотр settled это
за минуту. Прежде чем передавать в `tunnel_state`/`tunnel_log` что-то
похожее на идентификатор ОС — адаптер, службу, процесс, файл, который
пишет сторонняя программа, — стоит сначала посмотреть, что реально
существует на диске/в реестре/в списке адаптеров этой машины, а не
выводить это из того, что кажется удобным по документации или по памяти.

## Fix round 2 — alias убран из `tunnel_state` целиком

Round 1 закрыл дедлок (лог проверяется раньше алиас-based
`foreign_tunnel_up`), но оставил alias работать в половине логики: пока
`our_tunnel_up == false`, `foreign_tunnel_up` всё ещё строилась по
`tunnel_state::foreign_tunnel_up(routes, adapters, TUNNEL_PROFILE_NAME)`.
Ревью указало на остаточный случай: если лог нечитаем
(`liveness_error`), `our_tunnel_up` падает в `false`, и код round 1 (по
недосмотру приоритета) мог бы дойти до alias-based проверки и назвать
НАШ ЖЕ туннель чужим — тот самый признак, уже доказанный round 1
структурно неверным, оставался нагружен работой в этой ветке.

### Новая формула (без alias вовсе)

`tunnel_state::any_tunnel_carries(routes, adapters) -> bool` — несёт ли
ХОТЬ ОДИН туннельный адаптер маршрут, пересекающийся с `routes`, без
единой попытки решить, чей это адаптер. Функции `our_tunnel_up` и
`foreign_tunnel_up(..., our_alias)` (обе принимали alias) и приватный
`same_alias` удалены из `crates/winnet/src/tunnel_state.rs` целиком —
они стали недостижимы (никто их больше не вызывает) и, что важнее,
опирались на признак, для которого round 1 нашёл прямое опровержение на
реальной машине.

Вызывающий (`WinTunnel::snapshot`, `crates/app/src/main.rs`) собирает
`our_tunnel_up`/`foreign_tunnel_up` из связки двух независимых
`Result`ов — «что говорит лог» (`tunnel_log::liveness`) и «несёт ли
что-то наши подсети» (`any_tunnel_carries`) — чистой функцией
`combine_tunnel_facts`, полностью покрытой тестами (10 случаев,
все девять комбинаций `Result<bool>×Result<bool>` плюс регрессия):

```
(лог=Up,   несёт=true)  -> our_tunnel_up=true                (подтверждено обоими)
(лог=Up,   несёт=false) -> rising=true                        (переходное окно)
(лог=Down, несёт=true)  -> foreign_tunnel_up=true              (занято, не мы)
(лог=Down, несёт=false) -> всё false                           (опущен)
(лог=Down, routes=Err)  -> routes_error=Some(..)                (лог уверен — маршруты только подтверждали бы «свободно»)
(лог=Up,   routes=Err)  -> liveness_error=Some(..)               (логу одному больше не верим)
(лог=Err,  несёт=Any)   -> liveness_error=Some(..)               (неизвестность лога ПОБЕЖДАЕТ, не даёт «чужой»)
(лог=Err,  routes=Err)  -> liveness_error=Some(объединённое сообщение)
```

### Закрывает hard-kill дыру, которую `tunnel_log` в одиночку не мог

Если `openvpn.exe` убит без штатного выхода, лог продолжает врать
«поднято» (докблок `tunnel_log`, «Честные пределы»), но маршруты уходят
вместе с процессом почти сразу — `any_tunnel_carries` в этот момент
честно становится `false`, и `combine_tunnel_facts` гасит `our_tunnel_up`
раньше, чем об этом узнал бы человек. Один лог этого не видит (не смотрит
таблицу маршрутов); одни маршруты не отличают наш адаптер от чужого.
Вместе — отличают. Тест
`tests::a_hard_killed_process_stops_claiming_up_once_routes_disappear`
(`main.rs`) фиксирует это явно.

### `rising` — честное «поднимается», не «опущен» и не «поднят»

Побочный эффект конъюнкции, названный ревью заранее: сразу после
«Поднять туннель» лог может подтвердить успех раньше, чем встанут
маршруты профиля (`route ...` из собранного `.ovpn`). `our_tunnel_up`
корректно читает `false` в этот момент — конъюнкция не соврала, — но
показывать это как «опущен» означало бы приглашение нажать «Поднять»
ещё раз. Новое поле `TunnelSnapshot.rising` различает это состояние:
`tunnel_section` показывает «Туннель поднимается…» и кнопку
«Отменить/опустить» (та же команда `disconnect`, что прерывает
незавершённое подключение), без кнопки «Поднять». `raise_tunnel`
на сервере тоже не блокирует повторный POST в этом состоянии — он
избыточен, но не опасен, тем же рассуждением, что round 1 уже применил к
`our_tunnel_up == true`.

### Регрессионный тест, который мотивировал раунд

`tests::an_unreadable_log_does_not_call_our_own_tunnel_someone_elses`
(`main.rs`) — лог нечитаем, `any_tunnel_carries` при этом `true` (то есть
именно тот случай, где round 1 мог бы по недосмотру дойти до
alias-сравнения и ошибочно назвать это «чужим»): проверяет, что
`foreign_tunnel_up` остаётся `false`, а `liveness_error` — `Some`. Плюс
рендер-тесты `settings_page.rs`/`tray.rs`
(`an_unknown_liveness_shows_both_buttons_instead_of_locking_the_section`
уже существовал с round 1 и продолжает проходить с новой формулой,
подтверждая, что неизвестность по-прежнему первый приоритет).

### Сохранённое покрытие задачи 3

Тесты `tunnel_state.rs`, содержательно не завязанные на alias
(«постоянно поднятый Tailscale не в счёт для несвязанной офисной
подсети», широкий/узкий/точный/несвязанный/`0.0.0.0/0`/`/32`/хостовые
биты мимо `FromStr`), перенесены на новую сигнатуру `any_tunnel_carries`
без потери смысла — переименованы под то, что теперь проверяют
(«carries», не «foreign»), но проверяют то же самое пересечение
диапазонов. Тесты, чей смысл был ЦЕЛИКОМ про alias (`our_own_tunnel_is_not_foreign`,
`our_tunnel_up_*`, `alias_comparison_is_case_insensitive_and_trims_whitespace`)
удалены — поведение, которое они пином, для новой функции не существует
по построению (у неё нет параметра, за который можно было бы зацепиться).

### Прогон CI после round 2

```
cargo test --all:    493 теста, 0 отказов, 3 игнорируемых (было 482; +11:
                      1 в winnet::tunnel_state (несколько адаптеров, только
                      один несёт), 10 в app между combine_tunnel_facts,
                      rising-рендером и regression-тестами)
cargo clippy --all-targets -- -D warnings:  чисто, без единой находки
cargo fmt --all --check:  чисто после cargo fmt --all (несколько длинных
                      строк в новых тестах)
```

### Живые действия при этом раунде

Ни одного — весь раунд опирался на факты, уже установленные round 1
(README инсталляции, поведение усечения лога, реальные имена адаптеров),
без повторного обращения к живой машине. Изменения — чистый рефакторинг
существующих чистых функций и их вызывающего кода; ничего нового не
читалось и не менялось на диске/в реестре/в сети этой машины.

### Что не тронуто

`winnet::tunnel_log` (модуль целиком, только докблок «Честные пределы»
уточнён — сама логика классификации лога не менялась), `winnet::routes`
(логика сбора таблицы маршрутов и классификации `is_tunnel` не менялась,
только докблок), UAC-путь, DNS-предупреждение, приёмка исходного брифа —
всё как в round 1.
