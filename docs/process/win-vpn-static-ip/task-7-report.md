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

1. **`our_alias`.** Бриф прямо указал: задача 7 обязана его задать, задача
   3 честно предупредила, что псевдоним адаптера Windows — не устойчивый
   идентификатор. Взято `TUNNEL_PROFILE_NAME` («proxypilot-office») — то
   же имя, что у файла профиля и у `--command connect|disconnect`. Это
   ЛУЧШЕЕ ПРЕДПОЛОЖЕНИЕ, не гарантия: OpenVPN не обязан назвать сетевой
   адаптер точно как файл профиля (зависит от настроек OpenVPN GUI, и
   пользователь мог переименовать адаптер вручную). Страница говорит это
   прямо: у строки статуса профиля есть примечание «статус ниже определён
   по этому предположению — Windows не гарантирует, что адаптер называется
   именно так», а не только докблок в коде.
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
- Честный отказ при нечитаемой таблице маршрутов: не «опущен» по
  умолчанию, а «состояние неизвестно», кнопки скрыты
  (`tunnel_text_reports_an_unreadable_route_table_honestly`,
  `the_routes_error_disables_both_tunnel_buttons_and_is_escaped`).

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
- Реальная проверка `our_alias`: после подъёма посмотреть в Панели
  управления/`ipconfig`, как Windows на самом деле назвала адаптер, и
  сверить с текстом на странице («статус ниже определён по этому
  предположению»). Если имя разошлось — это ожидаемо документированное
  ограничение (задача 3), не дефект этой задачи, и его придётся починить
  либо через `dev-node` в исходном `.ovpn`, либо переименованием адаптера.
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
- `our_alias` — предположение, не факт (см. «Открытые решения», п. 1) —
  задача 3 предупредила заранее, задача 7 унаследовала предупреждение в UI.
- `TUNNEL_SOURCE_FILE`/«Собрать профиль» — решение задачи 7, не
  специфицированное ни одним предыдущим планом; если у продукта уже есть
  другой канал доставки исходного `.ovpn` (например, инсталлятор кладёт
  его заранее), это стоит свести к единому месту отдельной правкой.
- Уадаление службы (`uninstall-service`) кнопкой не покрыто — бриф просил
  только «установить», CLI-путь (`proxypilot.exe uninstall-service`,
  задача 6) остаётся рабочим и никуда не делся.
