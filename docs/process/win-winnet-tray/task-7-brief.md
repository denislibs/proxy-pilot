### Task 7: События смены сети

**Files:**
- Create: `win/crates/winnet/src/events.rs`
- Modify: `win/crates/winnet/src/lib.rs`
- Modify: `win/crates/winnet/Cargo.toml`

**Interfaces:**
- Consumes: `ComGuard` (задача 3), `WinNetError`.
- Produces: `NetworkChange` (перечисление), `watch_network_changes() -> Result<tokio::sync::mpsc::Receiver<NetworkChange>, WinNetError>`, `debounce(rx, window) -> Receiver<NetworkChange>`.

**Устройство.** NLM отдаёт события по классическому паттерну точек подключения: создать `NetworkListManager`, запросить у него `IConnectionPointContainer`, найти точку по IID `INetworkListManagerEvents`, вызвать `Advise` с объектом-приёмником. Приёмник реализуется макросом `#[implement]` из windows-rs. Готового примера на Rust в открытом виде нет — писать аккуратно, сверяясь с документацией интерфейсов.

Поток, на котором сделан `Advise`, обязан быть в апартаменте и **крутить цикл сообщений**: COM доставляет события апартаментного объекта через оконные сообщения, и без цикла приёмник просто не вызовется. Поэтому подписка живёт на своём выделенном потоке, а наружу отдаёт `tokio::sync::mpsc`.

**Запасной канал.** Если подписка не поднялась (нет прав, сломан COM, экзотическая сборка Windows) — `NotifyIpInterfaceChange` из IP Helper. Он грубее (реагирует на изменения адресов, а не на смену профиля сети), но лучше, чем ничего. Опрос по таймеру не используется: на macOS он был вынужденным, здесь есть настоящие события. Отказ подписки логируется как `warn` — молча деградировать нельзя.

**Дребезг.** Одно физическое переключение Wi-Fi порождает пачку событий. Схлопываем окном в 2 секунды: супервизор пересчитывает решение целиком, и десять пересчётов подряд ничего не добавляют, зато десять записей в логе мешают читать.

- [ ] **Step 1: Написать падающий тест на схлопывание**

`win/crates/winnet/src/events.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_burst_collapses_to_one_event() {
        // Одно переключение Wi-Fi даёт пачку событий; наружу должно уйти одно.
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut out = debounce(rx, Duration::from_millis(100));

        for _ in 0..5 {
            tx.send(NetworkChange::Connectivity).await.unwrap();
        }
        drop(tx);

        assert!(out.recv().await.is_some(), "первое событие обязано пройти");
        assert!(out.recv().await.is_none(), "остальные — схлопнуться");
    }

    #[tokio::test]
    async fn events_further_apart_than_the_window_both_pass() {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut out = debounce(rx, Duration::from_millis(50));

        tx.send(NetworkChange::Connectivity).await.unwrap();
        assert!(out.recv().await.is_some());

        tokio::time::sleep(Duration::from_millis(120)).await;
        tx.send(NetworkChange::NetworkPropertyChanged).await.unwrap();
        drop(tx);
        assert!(out.recv().await.is_some(), "после окна событие обязано пройти");
    }

    #[tokio::test]
    async fn closing_the_source_closes_the_output() {
        let (tx, rx) = tokio::sync::mpsc::channel::<NetworkChange>(1);
        let mut out = debounce(rx, Duration::from_millis(10));
        drop(tx);
        assert!(out.recv().await.is_none());
    }
}
```

- [ ] **Step 2: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-winnet events`
Expected: FAIL — `NetworkChange` и `debounce` не определены.

- [ ] **Step 3: Реализовать схлопывание**

```rust
//! События смены сети.
//!
//! NLM отдаёт их через точку подключения: создать NetworkListManager,
//! запросить IConnectionPointContainer, найти точку по IID
//! INetworkListManagerEvents, вызвать Advise с приёмником.
//!
//! Поток, сделавший Advise, обязан крутить цикл сообщений: COM доставляет
//! события апартаментного объекта оконными сообщениями, и без цикла приёмник
//! просто не вызовется. Отсюда выделенный поток и канал наружу.

use std::time::Duration;

use tokio::sync::mpsc::{channel, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkChange {
    Connectivity,
    NetworkAdded,
    NetworkPropertyChanged,
}

/// Схлопывает пачку событий в одно.
///
/// Первое событие проходит сразу — реагировать надо быстро; всё, что пришло
/// в течение окна после него, отбрасывается.
pub fn debounce(mut rx: Receiver<NetworkChange>, window: Duration) -> Receiver<NetworkChange> {
    let (tx, out) = channel(8);
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if tx.send(ev).await.is_err() {
                return;
            }
            // Дожёвываем хвост пачки, ничего не пересылая.
            let deadline = tokio::time::Instant::now() + window;
            loop {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some(_)) => continue,
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
        }
    });
    out
}
```

- [ ] **Step 4: Реализовать подписку**

Приёмник через `#[implement(INetworkListManagerEvents)]`, метод `ConnectivityChanged` шлёт `NetworkChange::Connectivity` в канал (`try_send`, чтобы не блокировать COM-поток; переполнение канала означает, что супервизор ещё не разгрёб прошлое событие, и терять новое безопасно — решение всё равно пересчитывается целиком).

Порядок: `ComGuard` → `CoCreateInstance(&NetworkListManager)` → `.cast::<IConnectionPointContainer>()` → `FindConnectionPoint(&INetworkListManagerEvents::IID)` → `Advise(&sink)`. Полученный `cookie` хранится и отдаётся в `Unadvise` при завершении. Затем цикл `GetMessage`/`DispatchMessage` до сигнала остановки.

При любой ошибке на этом пути: `warn!` с текстом и переход на `NotifyIpInterfaceChange`.

- [ ] **Step 5: Ручная проверка**

Запустить, физически переключить Wi-Fi (или включить/выключить адаптер), увидеть в логе ровно одну строку о смене сети, а не пачку. Приложить вывод лога в отчёт.

- [ ] **Step 6: Коммит**

```bash
git add win/crates/winnet
git commit -m "feat(win): события смены сети через NLM со схлопыванием пачки"
```

---

