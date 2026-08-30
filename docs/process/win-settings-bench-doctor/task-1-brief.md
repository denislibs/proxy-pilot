### Task 1: Замер путей

**Files:**
- Create: `win/crates/bridge/src/bench.rs`
- Modify: `win/crates/bridge/src/lib.rs`

**Interfaces:**
- Consumes: `Route`, `Upstreams`, `connect_via`.
- Produces: `BenchResult { label: String, route: Route, bytes: u64, elapsed: Duration, error: Option<String> }`, `BenchResult::speed_bps(&self) -> Option<u64>`, `bench_all(&Upstreams, url: &str, limit: u64, timeout: Duration) -> Vec<BenchResult>`, `fastest(&[BenchResult]) -> Option<&BenchResult>`.

**Что меряем и что не меряем.** Один поток и короткий файл — **намеренно**. Цифры сравнивают пути между собой, а не измеряют скорость линии; абсолютную скорость меряют многопоточно и с разгоном. Если подать это как измерение канала, человек справедливо не поверит числам.

Мост в сравнении **не участвует как путь** — он проверка, а не маршрут: измерять его значит измерять один из тех же трёх путей плюс накладные расходы на лишний хоп.

- [ ] **Step 1: Написать падающий тест**

`win/crates/bridge/src/bench.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn res(label: &str, bytes: u64, ms: u64, err: Option<&str>) -> BenchResult {
        BenchResult {
            label: label.to_string(),
            route: Route::Direct,
            bytes,
            elapsed: Duration::from_millis(ms),
            error: err.map(|e| e.to_string()),
        }
    }

    #[test]
    fn speed_is_bytes_over_seconds() {
        assert_eq!(res("x", 1_000_000, 1000, None).speed_bps(), Some(1_000_000));
        assert_eq!(res("x", 500_000, 500, None).speed_bps(), Some(1_000_000));
    }

    #[test]
    fn a_failed_measurement_has_no_speed() {
        // Иначе «0 байт за 0 секунд» превратилось бы в деление на ноль
        // или в бесконечность, и путь выглядел бы бесконечно быстрым.
        assert_eq!(res("x", 0, 0, Some("отказ")).speed_bps(), None);
        assert_eq!(res("x", 0, 100, Some("отказ")).speed_bps(), None);
    }

    #[test]
    fn a_zero_duration_does_not_divide_by_zero() {
        assert_eq!(res("x", 1000, 0, None).speed_bps(), None);
    }

    #[test]
    fn fastest_ignores_failures() {
        let rs = vec![
            res("мёртвый", 0, 0, Some("отказ")),
            res("медленный", 100_000, 1000, None),
            res("быстрый", 900_000, 1000, None),
        ];
        assert_eq!(fastest(&rs).map(|r| r.label.as_str()), Some("быстрый"));
    }

    #[test]
    fn fastest_of_nothing_is_nothing() {
        assert!(fastest(&[]).is_none());
        assert!(fastest(&[res("x", 0, 0, Some("отказ"))]).is_none());
    }

    #[tokio::test]
    async fn a_dead_upstream_yields_an_error_not_a_hang() {
        let up = Upstreams { socks: Some("127.0.0.1:1".into()), http: None };
        let started = std::time::Instant::now();
        let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(500)).await;
        assert!(started.elapsed() < Duration::from_secs(5), "замер обязан укладываться в таймаут");
        assert!(rs.iter().any(|r| r.error.is_some()), "мёртвый путь обязан быть помечен ошибкой");
    }

    #[tokio::test]
    async fn every_configured_route_is_measured_and_labelled() {
        let up = Upstreams { socks: Some("127.0.0.1:1".into()), http: Some("127.0.0.1:2".into()) };
        let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(300)).await;
        // напрямую + socks + http
        assert_eq!(rs.len(), 3, "получили: {:?}", rs.iter().map(|r| &r.label).collect::<Vec<_>>());
        assert!(rs.iter().any(|r| matches!(r.route, Route::Direct)));
        assert!(rs.iter().any(|r| matches!(r.route, Route::Socks(_))));
        assert!(rs.iter().any(|r| matches!(r.route, Route::Http(_))));
    }

    #[tokio::test]
    async fn an_unconfigured_upstream_is_not_measured() {
        let up = Upstreams { socks: None, http: None };
        let rs = bench_all(&up, "http://127.0.0.1:1/", 1000, Duration::from_millis(300)).await;
        assert_eq!(rs.len(), 1, "только «напрямую»");
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-bridge bench`
Expected: FAIL — модуля нет. Создай пустой `bench.rs` и объяви его, чтобы прогон дошёл до ошибок типов, а не остановился на «file not found for module».

- [ ] **Step 3: Написать реализацию**

Требования к `bench_all`:

- Меряет каждый **настроенный** путь: всегда `Direct`, плюс `Socks`/`Http`, если заданы.
- Для каждого: набрать через `connect_via` с общим таймаутом, отправить простой `GET`, читать тело до `limit` байт или до конца, засечь время от начала набора до последнего байта.
- Любая ошибка — в поле `error`, а не паника и не пропуск строки: путь, который не отработал, надо показать как не отработавший.
- Каждый замер ограничен `timeout` целиком, а не по-фазно.
- Не использует HTTP-библиотек: запрос простой, а лишняя зависимость в дереве, которое придётся подписывать, не нужна.

`speed_bps` возвращает `None`, если была ошибка или прошло ноль времени.

`fastest` игнорирует строки с ошибкой и строки без скорости.

- [ ] **Step 4: Прогнать тесты и линтеры, закоммитить**

```bash
git add win/crates/bridge
git commit -m "feat(win): замер путей — сравнение маршрутов между собой"
```

---

