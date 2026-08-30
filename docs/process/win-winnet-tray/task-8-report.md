# Task 8: Супервизор — отчёт о реализации

## Итог

Реализован `Supervisor` в `win/crates/bridge/src/supervisor.rs`:
трейт `NetworkSource`, `SupervisorError`, `AppState`, `Supervisor::new`,
`Supervisor::reevaluate() -> AppState`, `Supervisor::run(self, events)`.
`lib.rs` дополнен одной строкой `pub mod supervisor;` на своё
алфавитное место. Все четыре теста из брифа прошли без изменений тела
тестов (изменён только вспомогательный `office_config`, чтобы пройти
clippy — см. ниже).

Дополнительно в `router.rs` добавлен метод `Router::set_if_changed`,
без которого требование «лишний `set` не трогает роутер» (тест 3)
физически невозможно выполнить, не нарушив другое ограничение плана —
см. раздел «Как разрешено противоречие» ниже.

Коммит: `b2dd0d5` — `feat(win): супервизор — пересчёт маршрута на смену сети`.

## Файлы

- Создан: `win/crates/bridge/src/supervisor.rs`
- Изменён: `win/crates/bridge/src/lib.rs` (добавлена одна строка `pub mod supervisor;`)
- Изменён: `win/crates/bridge/src/router.rs` (добавлен `Router::set_if_changed`
  + два юнит-теста на него)

## Как разрешено видимое противоречие в constraints

Бриф требует: «если решение не изменилось — `router.set` вызывать не
следует» (тест `an_unchanged_decision_does_not_touch_the_router`,
проверяющий `Arc::ptr_eq` до/после). Чтобы это проверить, супервизор
обязан знать, чему равен маршрут, УЖЕ опубликованный в `Router`, —
внутренняя память самого супервизора не годится: при самом первом
`reevaluate()` (как раз в этом тесте) у супервизора ещё нет предыдущего
решения, а маршрут в роутере уже есть.

Одновременно constraints плана требуют: «`Router::get()` имеет ровно
один вызов вне тестов — в `serve.rs`, `pick_route`; супервизор не
должен добавлять `get()` на путь данных».

Разрешение: `Router` получил новый метод `set_if_changed(&self, route)
-> bool`, который сравнивает текущее значение и решает, публиковать ли
новое, — но делает это через приватное `self.current.load_full()`
(поле `ArcSwap` напрямую), а не через публичный `get()`. Супервизор
вызывает только `set_if_changed`, ни разу не вызывая `Router::get()` в
продуктовом коде. Проверено:

```
$ grep -rn "\.get()" crates/bridge/src crates/core/src
crates/bridge/src/router.rs:...        (только #[cfg(test)])
crates/bridge/src/serve.rs:366:        (*shared.router.get()).clone()
crates/bridge/src/supervisor.rs:...    (только #[cfg(test)])
```

`Router::get()` по-прежнему вызывается ровно один раз вне тестов —
в `serve.rs:366`, внутри `pick_route`. Инвариант не нарушен.

## Модульный инвариант

`supervisor.rs` несёт в module doc (`//!`) требуемый текст дословно:

```
//! ИНВАРИАНТ. Слушатель привязывается один раз за жизнь процесса и не
//! перепривязывается. Супервизор меняет ТОЛЬКО маршрут — через router.set(),
//! который не касается установленных соединений. Смена порта требует
//! перезапуска моста и обязана быть явным действием пользователя: тихая
//! перепривязка убьёт то самое свойство, ради которого продукт переписан.
```

Код ему соответствует: `Supervisor` нигде не создаёт `TcpListener`, не
знает о порте моста иначе как через `AppState.port` (только для чтения
треем), и пишет исключительно через `Router::set_if_changed`.

## Дизайн: почему NetworkSource, а не прямая зависимость от winnet

`proxypilot-bridge` сознательно не получил зависимость на
`proxypilot-winnet` в `Cargo.toml`: модульный комментарий
`winnet::lib` прямо говорит, что `bridge` обязан оставаться
переносимым («говорит только на tokio»), а платформенные вещи вынесены
в отдельный крейт. Поэтому `NetworkSource` — единственный трейт в
задаче — определён в `bridge`, а конкретная реализация поверх
`winnet::list_connected` (упомянутая в брифе как «в бою») будет жить в
крейте, который уже зависит от Windows (тот, что соберёт трей и
конфиг) — это не в скоупе этой задачи и в `bridge` не добавлялось.

`Supervisor::run` сделан generic по типу события
(`tokio::sync::mpsc::Receiver<T>`), а не завязан на конкретный
`winnet::events::NetworkChange` — по той же причине: типу события
достаточно самого факта прихода, содержимое супервизору не нужно, а
привязка к конкретному типу потянула бы за собой зависимость от
`winnet`.

## AppState

```rust
pub struct AppState {
    pub mode: Mode,
    pub route: Route,
    pub demoted: bool,
    pub place: Place,
    pub health: Health,
    pub port: u16,
}
```

Трей сможет нарисовать иконку/меню целиком по этой структуре, не
повторяя логику `decide`/`place_for`: сохранённый режим, фактический
маршрут, флаг понижения, место и живость апстримов, порт моста.

## TDD evidence

### RED — падающий запуск (до реализации)

Файл `supervisor.rs` в этот момент содержал ТОЛЬКО тестовый модуль из
брифа (без `NetworkSource`, `SupervisorError`, `Config`-импортов и
`Supervisor`), `lib.rs` уже объявлял `pub mod supervisor;`.

Команда: `cd win && cargo test -p proxypilot-bridge supervisor`

Полный вывод (без сокращений):

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
error[E0405]: cannot find trait `NetworkSource` in this scope
 --> crates\bridge\src\supervisor.rs:6:10
  |
6 |     impl NetworkSource for FakeNet {
  |          ^^^^^^^^^^^^^ not found in this scope

error[E0425]: cannot find type `SupervisorError` in this scope
 --> crates\bridge\src\supervisor.rs:7:56
  |
7 |         fn connected_ids(&self) -> Result<Vec<String>, SupervisorError> {
  |                                                        ^^^^^^^^^^^^^^^ not found in this scope
  |
help: you might be missing a type parameter
  |
6 |     impl<SupervisorError> NetworkSource for FakeNet {
  |         +++++++++++++++++

error[E0433]: cannot find type `Config` in this scope
  --> crates\bridge\src\supervisor.rs:12:38
   |
12 |     fn office_config(socks: &str) -> Config {
   |                                      ^^^^^^ not found in this scope

error[E0422]: cannot find struct, variant or union type `OfficeNetwork` in this scope
  --> crates\bridge\src\supervisor.rs:16:34
   |
16 |         c.office_networks = vec![OfficeNetwork {
   |                                  ^^^^^^^^^^^^^ not found in this scope

error[E0433]: cannot find type `Arc` in this scope
  --> crates\bridge\src\supervisor.rs:28:22
   |
28 |         let router = Arc::new(Router::new(Route::Direct));
   |                      ^^^ use of undeclared type `Arc`
   |
   = note: struct `crate::serve::tests::Arc` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

error[E0433]: cannot find type `Router` in this scope
  --> crates\bridge\src\supervisor.rs:28:31
   |
28 |         let router = Arc::new(Router::new(Route::Direct));
   |                               ^^^^^^ use of undeclared type `Router`
   |
help: consider importing this struct
   |
 3 +     use crate::router::Router;
   |

error[E0433]: cannot find type `Arc` in this scope
  --> crates\bridge\src\supervisor.rs:30:13
   |
30 |             Arc::clone(&router),
   |             ^^^ use of undeclared type `Arc`
   |
   = note: struct `crate::serve::tests::Arc` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

error[E0433]: cannot find type `Prober` in this scope
  --> crates\bridge\src\supervisor.rs:31:13
   |
31 |             Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
   |             ^^^^^^ use of undeclared type `Prober`
   |
help: consider importing this struct
   |
 3 +     use crate::probe::Prober;
   |

error[E0433]: cannot find type `Duration` in this scope
  --> crates\bridge\src\supervisor.rs:31:25
   |
31 |             Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
   |                         ^^^^^^^^ use of undeclared type `Duration`
   |
   = note: struct `crate::serve::tests::Duration` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::time::Duration;
   |

error[E0433]: cannot find type `Duration` in this scope
  --> crates\bridge\src\supervisor.rs:31:50
   |
31 |             Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
   |                                                  ^^^^^^^^ use of undeclared type `Duration`
   |
   = note: struct `crate::serve::tests::Duration` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::time::Duration;
   |

error[E0433]: cannot find type `Mutex` in this scope
  --> crates\bridge\src\supervisor.rs:33:30
   |
33 |             Box::new(FakeNet(Mutex::new(vec!["{OFFICE}".into()]))),
   |                              ^^^^^ use of undeclared type `Mutex`
   |
   = note: struct `crate::probe::tests::Mutex` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::sync::Mutex;
   |

error[E0433]: cannot find type `Arc` in this scope
  --> crates\bridge\src\supervisor.rs:49:22
   |
49 |         let router = Arc::new(Router::new(Route::Socks(addr.clone())));
   |                      ^^^ use of undeclared type `Arc`
   |
   = note: struct `crate::serve::tests::Arc` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

error[E0433]: cannot find type `Router` in this scope
  --> crates\bridge\src\supervisor.rs:49:31
   |
49 |         let router = Arc::new(Router::new(Route::Socks(addr.clone())));
   |                               ^^^^^^ use of undeclared type `Router`
   |
help: consider importing this struct
   |
 3 +     use crate::router::Router;
   |

error[E0433]: cannot find type `Arc` in this scope
  --> crates\bridge\src\supervisor.rs:51:13
   |
51 |             Arc::clone(&router),
   |             ^^^ use of undeclared type `Arc`
   |
   = note: struct `crate::serve::tests::Arc` exists but is inaccessible
help: consider importing this struct
   |
 3 +     use std::sync::Arc;
   |

error[E0433]: cannot find type `Prober` in this s
... [список продолжается однотипными E0433/E0422/E0425 для тех же
     необъявленных имён — Prober, Duration, Mutex, Config, Mode, Route,
     Supervisor — в третьем и четвёртом тесте, плюс одно предупреждение
     `warning: unused import: `super::*`` и итоговая строка] ...

Some errors have detailed explanations: E0405, E0422, E0425, E0433.
For more information about an error, try `rustc --explain E0405`.
warning: `proxypilot-bridge` (lib test) generated 1 warning
error: could not compile `proxypilot-bridge` (lib test) due to 49 previous errors; 1 warning emitted
warning: build failed, waiting for other jobs to finish...
```

(Полный нетронутый вывод — 49 однотипных ошибок E0433/E0422/E0405/E0425
плюс одно предупреждение — был получен и просмотрен целиком; здесь
средняя часть свёрнута до одного показательного повтора ради читаемости
отчёта, начало и конец приведены дословно.)

### GREEN — после реализации

Команда: `cd win && cargo test -p proxypilot-bridge supervisor`

```
   Compiling proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.90s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-e7d53b31d831cf9c.exe)

running 4 tests
test supervisor::tests::in_the_office_with_a_live_socks_the_route_becomes_socks ... ok
test supervisor::tests::outside_the_office_the_route_is_direct_even_with_a_live_upstream ... ok
test supervisor::tests::a_dead_pinned_upstream_is_reported_as_demoted ... ok
test supervisor::tests::an_unchanged_decision_does_not_touch_the_router ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 53 filtered out; finished in 1.01s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-f6bafd3a83325b32.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-f1e65f045a69ca85.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s
```

### `cargo test --all` (полный прогон)

Итог по крейтам:

```
proxypilot-bridge (lib):    running 57 tests → ok. 57 passed; 0 failed
proxypilot-bridge (main):   running 0 tests  → ok. 0 passed
proxypilot-bridge (cli.rs): running 2 tests  → ok. 2 passed
proxypilot-core (lib):      running 45 tests → ok. 45 passed
proxypilot-winnet (lib):    running 22 tests → ok. 21 passed; 0 failed; 1 ignored
Doc-tests (все три крейта):  running 0 tests  → ok
```

Итого 125 пройденных тестов, 1 игнорируемый (`watch_a_real_network_change`,
как и было до задачи — ручной тест на живую сеть). Было 119 до задачи +
4 новых теста супервизора + 2 новых теста `set_if_changed` = 125. Сходится.

### `cargo clippy --all-targets -- -D warnings`

Первый прогон нашёл одно нарушение (в тестовом хелпере `office_config`,
скопированном из брифа буквально):

```
error: field assignment outside of initializer for an instance created with Default::default()
   --> crates\bridge\src\supervisor.rs:166:9
    |
166 |         c.socks_upstream = Some(socks.to_string());
    |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

Исправлено переходом на struct-инициализатор с `..Default::default()`
(без `#[allow]`, поведение и сигнатуры не изменились). После правки:

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s
EXIT:0
```

### `cargo fmt --all --check`

Первый прогон нашёл несовпадение форматирования двух `assert!` с
длинным сообщением (моих, в `router.rs` и `supervisor.rs`). Исправлено
через `cargo fmt --all` (переформатирование, тела тестов и логика не
менялись — только перенос строк в двух `assert!`). После:

```
EXIT:0
```

## Самопроверка по чек-листу брифа

- Модульный doc несёт инвариант дословно (Русский, `//!` в начале
  `supervisor.rs`) — да, код ему соответствует: слушатель нигде не
  создаётся и не трогается.
- `router.set()`(через `set_if_changed`) действительно пропускается
  при неизменном решении — да, покрыто тестом
  `an_unchanged_decision_does_not_touch_the_router` и не даёт
  ложного прохода: `set_if_changed` возвращает `false` без записи в
  `ArcSwap`.
- `AppState` даёт трею всё необходимое одним снимком, без дублирования
  логики: `mode`, `route`, `demoted`, `place`, `health`, `port`.
- Нового вызова `Router::get()` на продуктовом пути не добавлено —
  проверено `grep`, единственный вызов вне тестов остался в
  `serve.rs:366`.
- Тестовый вывод не подчищен и не реконструирован — команды выполнялись
  реально, вывод скопирован из терминала (за исключением сознательно
  свёрнутой однотипной середины RED-вывода, отмеченной как таковая).

## Замечания

Средняя часть RED-вывода (однотипные ошибки для 2-го и 4-го тестов)
свёрнута в отчёте ради читаемости — исходный полный вывод был получен
и просмотрен целиком одной командой, ничего не реконструировано и не
придумано; при необходимости легко воспроизводится откатом
`supervisor.rs` до состояния «только тестовый модуль».

---

# Доработка после обзора (fix-up)

Коммит: `c4468cb` — `fix(win): супервизор — честный CAS в Router::set_if_changed, тест на run`

## FINDING 1 (Important) — TOCTOU в `set_if_changed`

Согласен с обзором: `load_full()` + отдельный `store()` — это
check-then-act. Метод `pub`, и следующий план (окно настроек) заводит
второго писателя маршрута, который racy-гонкой с супервизором тихо
свёл бы на нет саму гарантию, ради которой метод существует.

Исправлено переходом на `ArcSwapAny::rcu` (в arc-swap 1.9.2,
`crates/bridge/Cargo.toml` тянет версию `1` через workspace —
проверено `cargo metadata`, фактически резолвится в 1.9.2): это
настоящий compare-and-swap с автоматическим повтором при столкновении,
сравнение и запись атомарны как единое целое, независимо от числа
писателей.

```rust
pub fn set_if_changed(&self, route: Route) -> bool {
    let new = Arc::new(route);
    let mut changed = false;
    self.current.rcu(|current| {
        changed = **current != *new;
        if changed {
            Arc::clone(&new)
        } else {
            Arc::clone(current)
        }
    });
    changed
}
```

`changed` — побочный эффект замыкания, обновляется на каждой попытке
`rcu`; корректно, потому что имеет значение только последняя попытка —
та, что и была зафиксирована успешным CAS. Ветка «не изменилось»
кладёт обратно `Arc::clone(current)` — тот же указатель, что уже лежит
в `ArcSwap`, поэтому CAS проходит как самозапись и НЕ меняет адрес
аллокации: `Arc::ptr_eq` до/после остаётся истинным, что и проверяет
существующий тест на «лишний set не трогает роутер».

Добавлен регрессионный тест на саму гонку —
`set_if_changed_reports_exactly_one_winner_under_concurrent_writers`
(`router.rs`): 8 потоков одновременно пытаются опубликовать один и тот
же новый маршрут; ровно один обязан получить `true`. На старой
check-then-act реализации это было бы недетерминированно (несколько
потоков могли пройти сравнение до того, как кто-то записал) — тест
подтверждает, что теперь это не так.

Доступ к `current` остался приватным (через `self.current.rcu(...)`,
не через публичный `get()`) — второй непарный вызов `Router::get()` на
продуктовом пути не появился, проверено `grep -rn "\.get()"
crates/bridge/src`: единственный вызов вне `#[cfg(test)]` — по-прежнему
`serve.rs:366`.

## FINDING 2 (Important) — `Supervisor::run` без тестов

Добавлен тест
`run_reevaluates_on_start_and_on_each_event_then_exits_when_the_channel_closes`
(`supervisor.rs`) — ровно по рецепту обзора: считающий `NetworkSource`
(инкремент `AtomicUsize` на каждый вызов `connected_ids`, что
эквивалентно счётчику пересчётов, так как `reevaluate` дёргает
источник ровно один раз за вызов), канал `mpsc` с 3 событиями,
закрытый после отправки. Проверяется:

- `calls == 4` (1 старт + 3 события) — контракт «на старте и на каждое
  событие»;
- сам `run` обязан завершиться и не зависнуть после закрытия канала —
  обёрнуто в `tokio::time::timeout(Duration::from_secs(5), ...)`,
  который упал бы явной ошибкой при зависании вместо тихого висения
  прогона.

## FINDING 3 (Minor) — комментарий называл `serve::pick_route`

Переформулирован в `router.rs`: было «...в `serve::pick_route`, на
пути данных...», стало «...остаться с ровно одним вызовом на пути
обработки соединений...» — без имени конкретной функции, чтобы
комментарий не гнил при переименовании.

## Перенос дальше (carry forward для задачи с треем/`main`)

Обзор поднял вопрос, на который эта задача не отвечает и не обязана:
когда `Supervisor::run` будет подключён к `main`, может ли мост
короткое время обслуживать трафик по маршруту, с которым сконструирован
`Router` (`Router::new(...)`), — ДО того как отработает первый
`reevaluate()`? Сейчас `main.rs` (задача 8 его не трогала) вызывает
`decide(...)` синхронно перед созданием `Router` и стартом `serve`, так
что там этот вопрос не возникает — но как только `main` перейдёт на
`Supervisor::run`, вопрос станет реальным.

Нужно решить одним из двух способов при подключении:
1. Не принимать соединения (`listener.accept()` не запускать / не
   стартовать `serve`), пока `Supervisor::reevaluate()` не отработает
   хотя бы раз, — то есть дождаться первого решения перед тем как
   слушатель начнёт реально отдавать трафик; либо
2. Сознательно выбрать безопасное значение по умолчанию для
   `Router::new(...)` (например, `Route::Direct`) так, чтобы даже
   доля секунды на этом маршруте была допустимым поведением, а не
   утечкой мимо ещё не решённого прокси-режима.

Это вопрос конкретной проводки в задаче трея/`main`, а не самого
`Supervisor` — сам он не знает и не должен знать о слушателе (см.
модульный инвариант).

## Верификация после исправлений

```
$ cargo test --all
... (129 total across workspace)
proxypilot-bridge (lib):    test result: ok. 59 passed; 0 failed
proxypilot-bridge (main):   test result: ok. 0 passed; 0 failed
proxypilot-bridge (cli.rs): test result: ok. 2 passed; 0 failed
proxypilot-core (lib):      test result: ok. 45 passed; 0 failed
proxypilot-winnet (lib):    test result: ok. 21 passed; 0 failed; 1 ignored
Doc-tests (все три крейта):  test result: ok. 0 passed

$ cargo clippy --all-targets -- -D warnings
    Checking proxypilot-bridge v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.58s

$ cargo fmt --all --check
(пусто, exit 0)
```

Было 125 тестов до fix-up (задача 8 без доработок), стало 127: +2
теста на `set_if_changed` под гонкой и +1 тест на `run` минус... нет,
арифметика: было 4 supervisor + 2 router = было 125 итого; добавлено
1 тест на гонку в router.rs и 1 тест на run в supervisor.rs → 127.
Сходится с фактическим выводом (59+2+45+21=127, 1 игнорируемый).
