# Итоговая волна правок по обзору ветки `feat/windows-rust`

Дата: 2026-08-30
База: `2342c53`, вершина после правок: `756d11b`
Тулчейн: rustc 1.98.0 (88d9e12ae 2026-08-18)

Тестов было 68, стало 74.

## Коммиты

| SHA | Тема |
| --- | --- |
| `467e197` | fix(win): не терять байты, склеенные с ответом апстрим-прокси |
| `115e88e` | docs(win): снимок маршрута берётся не «в момент приёма» |
| `c47e47f` | fix(win): сообщать о понижении маршрута, а не молчать |
| `9affc04` | test(win): сквозная передача байтов через socks5 и http коннекторы |
| `756d11b` | fix(win): бюджет accept-ошибок, TCP_NODELAY, CR/LF в заголовках, строка ответа от клиента |

---

## FIX 1 — коннектор молча выбрасывал байты апстрима

**Что сделано.** В `connector.rs` введён `Upstream { stream, pending }` с
доккомментарием из задания. `connect_via` и `connect_inner` теперь возвращают
`Result<Upstream, ConnectError>`; `Direct` и `Socks5` отдают
`pending: Vec::new()`, ветка `Http` — `pending: head.leftover`. Добавлен
`#[derive(Debug)]`: без него не компилируется существующий
`dial_timeout_is_honoured`, который печатает `{r:?}`.

В `serve.rs` и `handle_connect`, и `handle_plain` пишут `pending` **клиенту**
до старта `copy_bidirectional`. Запись `head.leftover` в апстрим оставлена как
была — это независимое направление, и оба должны отработать до перекачки.

**Тесты.**

- `connector::tests::direct_connects_to_origin` и
  `http_upstream_sends_connect_and_accepts_200` — обновлены под новый тип
  возврата, обе дополнительно утверждают `pending.is_empty()`.
- `connector::tests::http_upstream_keeps_bytes_glued_to_the_reply` — новый:
  фальшивый прокси одним `write_all` отдаёт `200` + `SSH-2.0-OpenSSH_9.6\r\n`,
  проверяется `up.pending`.
- `serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost` — новый
  регрессионный тест уровня моста: прокси одним `write_all` пишет
  `HTTP/1.1 200 Connection established\r\n\r\nBANNER`, проверяется, что
  `BANNER` дошёл **до клиента**.

### Доказательство, что регрессионный тест падает без правки

Тест был написан и запущен **до** изменения `connect_via`. Вывод дословно:

```
running 1 test
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... FAILED

failures:

---- serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost stdout ----

thread 'serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost' (13392) panicked at crates\bridge\src\serve.rs:454:18:
приветствие апстрима не дошло до клиента: Elapsed(())
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 37 filtered out; finished in 3.01s

error: test failed, to rerun pass `-p proxypilot-bridge --lib`
```

Симптом ровно тот, что описан в обзоре: не ошибка, а зависание — клиент ждёт
приветствия, которого уже нет.

---

## FIX 2 — `decision.demoted` вычислялся и выбрасывался

В `main.rs` перед созданием `Shared` добавлено одно `if decision.demoted`,
печатающее в stderr запрошенный режим и фактический маршрут. Комментарий
объясняет, почему молчать нельзя (спец 4.2) и что сохранённое предпочтение при
этом не меняется.

Проверено вручную:

```
$ cargo run -q --bin proxypilot-bridge -- --mode socks --port 39131
proxypilot-bridge: Socks недоступен, работаем через Direct; режим сохранён
мост слушает http://127.0.0.1:39131, маршрут: Direct
```

Первая формулировка использовала перенос строки `\` внутри литерала; rustfmt
схлопнул его в пачку пробелов прямо в сообщении. Текст сокращён до одной
строки — это видно в выводе выше.

---

## FIX 3 — доккомментарий `serve.rs` врал про головное свойство

Было: снимок берётся «в момент приёма». Стало: снимок берётся один раз на
соединение, до набора апстрима, и на пути данных к роутеру больше никто не
обращается. Отдельный коммит, чтобы правка формулировки не потерялась внутри
функциональной.

Инвариант проверен: `Router::get()` по-прежнему имеет **ровно один** вызов вне
тестов — `serve.rs:267` в `pick_route`. Ничего, рвущего живые соединения, не
добавлено.

---

## FIX 4 — не было сквозной передачи байтов через socks5 и http

Добавлены две фальшивки в тестовый модуль `serve.rs`, которые не только жмут
руку, но и **релеят** до существующего `origin()`:

- `fake_socks5_relay()` — серверная сторона SOCKS5; дополнительно утверждает
  `CMD=CONNECT` и `ATYP=0x03` (то есть мост действительно шлёт имя, семантика
  `socks5h`);
- `fake_http_relay()` — принимает CONNECT, отвечает `200`, релеит.

Тесты: `connect_through_socks5_upstream_tunnels_bytes` и
`connect_through_http_upstream_tunnels_bytes` — `ping` уходит, `pong`
возвращается через мост.

**Первая версия этих тестов была слабой, и я это поймал.** Проверка была
`reply.starts_with("HTTP/1.1 200")`. Контрольная мутация (убрать в `socks5.rs`
вычитывание адреса привязки из ответа — классическая порча потока) тесты
**не уронила**: шесть мусорных байт склеивались с ответом моста в одном чтении
и молча попадали в `reply`. Сверка сделана точной:

```rust
assert_eq!(
    reply, "HTTP/1.1 200 Connection established\r\n\r\n",
    "в ответ клиенту просочились байты апстрима"
);
```

После этого та же мутация даёт:

```
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... FAILED
  left: "HTTP/1.1 200 Connection established\r\n\r\n\0\0\0\0\0\0"
 right: "HTTP/1.1 200 Connection established\r\n\r\n"
test result: FAILED. 40 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Падает только socks5-тест, http-тест мутацией не задет — то есть тесты
различают коннекторы, а не срабатывают заодно. Мутация откачена.

---

## A — бюджет accept-ошибок

`MAX_CONSECUTIVE_ACCEPT_ERRORS` поднят с 64 до 640 (~32 с вместо 3,2 с).
`ConnectionAborted`, `ConnectionReset` и `Interrupted` вынесены в отдельную
guard-ветку `match`: они всегда `continue` и бюджет не тратят — это отказ
одного соединения, а не слушателя. Сон 50 мс сохранён в считаемой ветке, чтобы
цикл не крутился вхолостую.

Теста нет намеренно: чтобы `accept` вернул нужный `ErrorKind`, надо исчерпать
дескрипторы процесса или подменить слушатель — первое ломает весь тестовый
прогон, второе требует трейта поверх `TcpListener`, то есть архитектурного шва,
который заданием прямо отнесён к следующему плану. Слабый тест здесь был бы
хуже честного его отсутствия.

## B — `set_nodelay(true)`

Ставится на оба сокета в `handle_connect` непосредственно перед
`copy_bidirectional`, ошибка игнорируется. Теста нет: наблюдаемый эффект —
задержка на реальной сети, на loopback Nagle не проявляется, а проверять факт
вызова через сокет-опцию значило бы тестировать `tokio`, а не нас.

## C — CR и LF внутри заголовка

В `http.rs::parse` после `split_once(':')`:

```rust
if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
    return Err(HeadError::Malformed);
}
```

**Отступление от буквы задания:** проверяется не только значение, но и имя.
Имя уязвимо ровно так же — строка `X\nInjected: yes` даёт имя `X\nInjected`,
и `handle_plain` переизлучил бы его дословно. Это то же самое условие и та же
пара строк, так что закрывается весь класс, а не половина.

Тест `http::tests::header_value_with_a_bare_cr_or_lf_is_rejected` — три случая:
`\n` в значении, `\r` в значении, `\n` в имени.

## D — строка ответа, пришедшая как запрос клиента

Двусторонность `parse` сохранена (коннектор ею пользуется). В `handle` перед
развилкой connect/plain добавлен отказ `400`, если `head.method` начинается с
`HTTP/`.

Тест `serve::tests::a_response_status_line_from_a_client_yields_400`. Наивный
вариант (`HTTP/1.1 200 OK`) был бы пустым: `split_absolute("200")` и так даёт
`400`. Поэтому во второе поле подставлен живой absolute-form адрес:
`HTTP/1.1 http://<origin>/ OK`. До правки это уезжало к origin строкой
`HTTP/1.1 / OK`, и тест ловил именно это:

```
test serve::tests::a_response_status_line_from_a_client_yields_400 ... FAILED
получили: HTTP/1.1 200 OK
```

То есть клиент получал бодрый `200` от origin на заведомую бессмыслицу.
Тест `header_value_with_a_bare_cr_or_lf_is_rejected` также был написан и
запущен до правки и падал.

---

## Проверки

### `cargo test --all`

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.13s
     Running unittests src\lib.rs (target\debug\deps\proxypilot_bridge-9c2fac5c83a9c1ce.exe)

running 43 tests
test http::tests::header_names_keep_value_spacing_trimmed ... ok
test http::tests::parses_absolute_form_request ... ok
test http::tests::garbage_request_line_is_an_error ... ok
test http::tests::header_value_with_a_bare_cr_or_lf_is_rejected ... ok
test http::tests::keeps_bytes_that_follow_the_head ... ok
test http::tests::splits_bracketed_ipv6 ... ok
test http::tests::parses_connect ... ok
test http::tests::rejects_bad_port ... ok
test http::tests::parses_a_response_status_line_too ... ok
test http::tests::splits_host_and_port ... ok
test http::tests::oversized_head_is_rejected ... ok
test http::tests::truncated_input_is_an_error ... ok
test router::tests::returns_the_route_it_was_built_with ... ok
test router::tests::set_replaces_the_route_for_later_readers ... ok
test router::tests::a_handle_taken_before_set_keeps_the_old_route ... ok
test router::tests::is_shareable_across_threads ... ok
test connector::tests::http_upstream_keeps_bytes_glued_to_the_reply ... ok
test connector::tests::http_upstream_non_200_is_an_error ... ok
test connector::tests::direct_connects_to_origin ... ok
test connector::tests::http_upstream_sends_connect_and_accepts_200 ... ok
test serve::tests::a_response_status_line_from_a_client_yields_400 ... ok
test serve::tests::banner_arriving_with_the_upstream_reply_is_not_lost ... ok
test serve::tests::malformed_request_yields_400 ... ok
test serve::tests::bypassed_host_goes_direct_even_with_upstream_set ... ok
test serve::tests::non_absolute_target_yields_400 ... ok
test serve::tests::connect_direct_tunnels_bytes ... ok
test socks5::tests::accepts_domain_bound_address_in_reply ... ok
test socks5::tests::accepts_ipv4_bound_address_in_reply ... ok
test serve::tests::payload_sent_with_the_connect_head_is_not_lost ... ok
test socks5::tests::rejects_overlong_hostname ... ok
test serve::tests::plain_http_is_forwarded_in_origin_form ... ok
test socks5::tests::rejects_non_socks5_greeting ... ok
test serve::tests::connect_through_http_upstream_tunnels_bytes ... ok
test serve::tests::exceeding_the_connection_limit_yields_503 ... ok
test socks5::tests::rejects_server_demanding_auth ... ok
test serve::tests::connect_through_socks5_upstream_tunnels_bytes ... ok
test socks5::tests::sends_hostname_not_resolved_address ... ok
test socks5::tests::surfaces_refusal_code ... ok
test connector::tests::dial_timeout_is_honoured ... ok
test serve::tests::dead_upstream_yields_502_not_a_hang ... ok
test serve::tests::plain_http_with_dead_upstream_yields_502 ... ok
test serve::tests::changing_route_does_not_disturb_an_open_tunnel ... ok
test connector::tests::refused_upstream_reports_error ... ok

test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s

     Running unittests src\main.rs (target\debug\deps\proxypilot_bridge-c3a64ea26c1c605f.exe)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests\cli.rs (target\debug\deps\cli-ab4a9a8014464208.exe)

running 2 tests
test prints_usage_on_help ... ok
test rejects_an_invalid_upstream ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src\lib.rs (target\debug\deps\proxypilot_core-6d8e89ae7fb487cd.exe)

running 29 tests
test bypass::tests::cidr_matches_addresses_inside ... ok
test bypass::tests::dot_suffix_matches_subdomains_only ... ok
test bypass::tests::bracketed_ipv6_host_is_unwrapped ... ok
test bypass::tests::cidr_never_matches_a_hostname ... ok
test bypass::tests::cidr_does_not_match_outside ... ok
test config::tests::default_no_proxy_covers_local_ranges ... ok
test bypass::tests::exact_hostname_is_case_insensitive ... ok
test bypass::tests::full_prefix_cidr_matches_single_address ... ok
test bypass::tests::ip_literal_matches ... ok
test bypass::tests::empty_and_blank_entries_are_ignored ... ok
test config::tests::defaults_match_the_spec ... ok
test config::tests::broken_toml_is_an_error_not_a_panic ... ok
test bypass::tests::exact_hostname_matches ... ok
test bypass::tests::zero_prefix_cidr_matches_every_ipv4 ... ok
test bypass::tests::empty_list_matches_nothing ... ok
test config::tests::upstream_format_is_validated ... ok
test config::tests::missing_fields_fall_back_to_defaults ... ok
test config::tests::upstreams_view_is_built_from_config ... ok
test mode::tests::auto_in_office_falls_back_to_http ... ok
test mode::tests::auto_in_office_prefers_socks ... ok
test config::tests::roundtrip_through_toml_preserves_everything ... ok
test mode::tests::auto_in_office_with_everything_dead_is_direct ... ok
test mode::tests::auto_outside_office_is_always_direct ... ok
test mode::tests::direct_mode_is_direct ... ok
test mode::tests::pinned_http_demotes_to_direct_when_dead ... ok
test mode::tests::pinned_mode_ignores_place ... ok
test mode::tests::pinned_socks_demotes_to_direct_when_dead ... ok
test mode::tests::unconfigured_upstream_is_never_chosen ... ok
test mode::tests::unknown_reachability_counts_as_unusable ... ok

test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_bridge

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests proxypilot_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Код возврата 0.

### `cargo clippy --all-targets -- -D warnings`

Запущено после `touch` всех исходников, чтобы вывод не был кэшированным:

```
    Checking proxypilot-core v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\core)
    Checking proxypilot-bridge v0.1.0 (C:\Users\User\Desktop\proxypilot\repo\win\crates\bridge)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s
```

Код возврата 0. Ни одного `#[allow]` не добавлено.

### `cargo fmt --all --check`

Вывод пустой, код возврата 0.

---

## Чего я не делал и почему

**Вне области, по прямому указанию задания.** Не добавлял путь остановки для
`serve`, не переносил семафор в `Shared`, не делал `no_proxy` и `limits`
сменяемыми на ходу. Это швы для трея и окна настроек, и проектироваться они
должны вместе с кодом, который их потребляет. Также не тронуты: недостижимый
`ConfigError::Serialize`, приём `--port 0`, документирование алиаса `-h`,
точность диагностики `Socks5Error`, плавающая ссылка на тулчейн в CI и
отсутствие логирования.

**Тесты для A и B не написаны** — обоснование выше, в разделах A и B. Задание
разрешает сказать это прямо вместо выдумывания слабого теста.

**Отступление от буквы задания одно** — в пункте C проверяется и имя
заголовка, не только значение; обосновано выше.

## Замечания на будущее (не правил)

`handle_plain` при `Route::Http` набирает апстрим через `connect_via`, то есть
через CONNECT-туннель, и затем шлёт origin-form запрос. Спец 5.3 говорит, что
при http-апстриме запрос «проксируется почти как есть», то есть ожидался бы
absolute-form напрямую вышестоящему прокси, без CONNECT. Поведение рабочее
(корпоративные прокси обычно принимают CONNECT на 80-й порт), поведение
существовало до этой волны, и в список правок обзора оно не входило — поэтому
не трогал. Запись `pending` клиенту, добавленная в FIX 1, корректна в обеих
трактовках. Стоит решить отдельно.
