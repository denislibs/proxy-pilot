//! Супервизор: пересчёт маршрута при смене обстановки.
//!
//! ИНВАРИАНТ. Слушатель привязывается один раз за жизнь процесса и не
//! перепривязывается. Супервизор меняет ТОЛЬКО маршрут — через
//! `Router::set_if_changed()`, который не касается установленных
//! соединений. Смена порта требует перезапуска моста и обязана быть явным
//! действием пользователя: тихая перепривязка убьёт то самое свойство,
//! ради которого продукт переписан.
//!
//! На старте и на каждое схлопнутое событие смены сети (см.
//! `proxypilot-winnet::events::debounce`) супервизор спрашивает, какие сети
//! подключены, определяет место (`Config::place_for`), проверяет живость
//! апстримов (`Prober`) и пересчитывает решение (`core::decide`) — но
//! публикует его через `Router::set_if_changed`, только если оно
//! действительно изменилось. Лишний `set` безвреден для соединений — в этом
//! весь смысл конструкции, — но он маскирует ошибки в логике решения и
//! засоряет лог, поэтому супервизор сравнивает решение прежде, чем писать.

use std::sync::Arc;

use proxypilot_core::config::Config;
use proxypilot_core::mode::{decide, ConnectedNetwork, Health, Mode, Place, Route};
use tracing::{debug, info, warn};

use crate::probe::Prober;
use crate::router::Router;

#[cfg(test)]
use proxypilot_core::config::OfficeNetwork;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("не удалось получить список подключённых сетей: {0}")]
    Network(String),
}

/// Источник списка подключённых сетей.
///
/// Единственный трейт, который здесь нужен: NLM синхронен, и оборачивать его
/// в async не дало бы ничего, кроме лишнего слоя. В бою реализация
/// оборачивает `winnet::list_connected` — но не в этом крейте: мост обязан
/// оставаться переносимым (см. модульный комментарий `winnet::lib`), поэтому
/// платформенная реализация трейта живёт там, где уже есть зависимость от
/// Windows. Здесь, в тестах, — подставная.
pub trait NetworkSource: Send + Sync {
    /// Снимки целиком, а не одни идентификаторы: имя сети нужно UI (спека
    /// 6.1 — кнопка «эта сеть — офис» в окне настроек), а платформенный
    /// источник его и так уже прочитал. Сузить здесь до `Vec<String>`
    /// значило бы выбросить то, за чем потом придётся возвращаться, трогая
    /// заодно супервизор, `AppState` и трей.
    fn connected(&self) -> Result<Vec<ConnectedNetwork>, SupervisorError>;
}

/// Состояние, которое читает трей, чтобы нарисовать иконку и меню, — целиком,
/// не дублируя логику принятия решения.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Сохранённое предпочтение пользователя. Не путать с `route`: закреплённый
    /// режим может быть временно недоступен — см. `demoted`.
    pub mode: Mode,
    /// Фактически выбранный и опубликованный маршрут.
    pub route: Route,
    /// Закреплённый режим оказался недоступен, работаем иначе. Сохранённое
    /// предпочтение при этом не меняется — молчать об этом факте в UI нельзя.
    pub demoted: bool,
    /// Где мы, судя по подключённым сетям на момент пересчёта.
    pub place: Place,
    /// Живость обоих апстримов на момент пересчёта.
    pub health: Health,
    /// Порт, на котором слушает мост — трею больше неоткуда его узнать.
    pub port: u16,
}

/// Пересчитывает маршрут на старте и на каждую смену сети.
///
/// Не трогает мост: слушатель живёт своей жизнью (см. модульный инвариант),
/// супервизор только читает обстановку и пишет решение в `Router`.
pub struct Supervisor {
    router: Arc<Router>,
    prober: Prober,
    config: Config,
    source: Box<dyn NetworkSource>,
}

impl Supervisor {
    pub fn new(
        router: Arc<Router>,
        prober: Prober,
        config: Config,
        source: Box<dyn NetworkSource>,
    ) -> Self {
        Self {
            router,
            prober,
            config,
            source,
        }
    }

    /// Пересчитать решение и опубликовать маршрут, если он изменился.
    ///
    /// Ошибку источника сетей не роняем: трактуем её как «не знаем, где мы» —
    /// пустой список сетей и так означает «не офис» (см.
    /// `Config::place_for`), так что решение принимается на этом основании, а
    /// не падает. Молчать при этом нельзя — иначе деградация до этой ветки
    /// осталась бы незамеченной.
    pub async fn reevaluate(&self) -> AppState {
        let connected = match self.source.connected() {
            Ok(nets) => nets,
            Err(e) => {
                warn!(
                    error = %e,
                    "не удалось опросить список сетей, считаем себя вне офиса"
                );
                Vec::new()
            }
        };

        let place = self.config.place_for(&connected);
        let upstreams = self.config.upstreams();
        let health = self.prober.health(&upstreams).await;
        let decision = decide(self.config.mode, &upstreams, place.clone(), health);

        if self.router.set_if_changed(decision.route.clone()) {
            // Редкое событие — ровно то, что нужно увидеть при разборе,
            // почему трафик вдруг пошёл иначе.
            info!(route = ?decision.route, place = ?place, demoted = decision.demoted, "маршрут изменён");
        } else {
            debug!(route = ?decision.route, "решение не изменилось, маршрут не трогаем");
        }

        AppState {
            mode: self.config.mode,
            route: decision.route,
            demoted: decision.demoted,
            place,
            health,
            port: self.config.bridge_port,
        }
    }

    /// Крутится, пока не закроется канал событий: пересчитывает решение на
    /// старте и на каждое пришедшее событие. Тип события супервизору
    /// неважен — ему нужен только сам факт «сеть могла измениться»; конкретный
    /// тип (`winnet::events::NetworkChange`) остаётся в крейте, который уже
    /// знает про Windows, — этот обязан оставаться переносимым.
    pub async fn run<T>(self, mut events: tokio::sync::mpsc::Receiver<T>) {
        self.reevaluate().await;
        while events.recv().await.is_some() {
            self.reevaluate().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeNet(std::sync::Mutex<Vec<ConnectedNetwork>>);
    impl NetworkSource for FakeNet {
        fn connected(&self) -> Result<Vec<ConnectedNetwork>, SupervisorError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn nets(pairs: &[(&str, &str)]) -> Mutex<Vec<ConnectedNetwork>> {
        Mutex::new(
            pairs
                .iter()
                .map(|(id, name)| ConnectedNetwork {
                    id: (*id).into(),
                    name: (*name).into(),
                })
                .collect(),
        )
    }

    fn office_config(socks: &str) -> Config {
        Config {
            socks_upstream: Some(socks.to_string()),
            mode: Mode::Auto,
            office_networks: vec![OfficeNetwork {
                id: "{OFFICE}".into(),
                name: "Офис".into(),
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn in_the_office_with_a_live_socks_the_route_becomes_socks() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();

        let router = Arc::new(Router::new(Route::Direct));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config(&addr),
            Box::new(FakeNet(nets(&[("{OFFICE}", "OFFICE-WIFI")]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Socks(addr.clone()));
        assert_eq!(*router.get(), Route::Socks(addr));
        assert!(state.place.in_office);
    }

    #[tokio::test]
    async fn outside_the_office_the_route_is_direct_even_with_a_live_upstream() {
        // Правило спеки 4.2 дословно: снаружи офисный прокси тоже отвечает
        // (через туннель), но маршрут через него был бы кругом через офис.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();

        let router = Arc::new(Router::new(Route::Socks(addr.clone())));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config(&addr),
            Box::new(FakeNet(nets(&[("{HOME}", "Домашний Wi-Fi")]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Direct);
        assert_eq!(*router.get(), Route::Direct);
        assert!(!state.place.in_office);
    }

    #[tokio::test]
    async fn the_network_name_reaches_the_app_state() {
        // Трею и будущему окну настроек нужен не голый GUID, а то, что
        // человек видит в списке сетей Windows. Имя обязано доехать от
        // источника до `AppState` целиком — иначе подписать кнопку «эта
        // сеть — офис» (спека 6.1) будет нечем.
        let router = Arc::new(Router::new(Route::Direct));
        let sup = Supervisor::new(
            router,
            Prober::new(Duration::from_secs(30), Duration::from_millis(200)),
            office_config("127.0.0.1:1"),
            Box::new(FakeNet(nets(&[("{OFFICE}", "OFFICE-WIFI")]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.place.network.as_deref(), Some("{OFFICE}"));
        assert_eq!(state.place.network_name.as_deref(), Some("OFFICE-WIFI"));
    }

    #[tokio::test]
    async fn an_unchanged_decision_does_not_touch_the_router() {
        // Лишний set безвреден для соединений, но маскирует ошибки в логике
        // и засоряет лог. Решение не изменилось — значит и трогать нечего.
        let router = Arc::new(Router::new(Route::Direct));
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config("127.0.0.1:1"),
            Box::new(FakeNet(nets(&[("{HOME}", "Домашний Wi-Fi")]))),
        );

        let before = router.get();
        sup.reevaluate().await;
        let after = router.get();
        assert!(
            Arc::ptr_eq(&before, &after),
            "router.set вызывать не следовало"
        );
    }

    #[tokio::test]
    async fn a_dead_pinned_upstream_is_reported_as_demoted() {
        let router = Arc::new(Router::new(Route::Direct));
        let mut cfg = office_config("127.0.0.1:1");
        cfg.mode = Mode::Socks;
        let sup = Supervisor::new(
            Arc::clone(&router),
            Prober::new(Duration::from_secs(30), Duration::from_millis(200)),
            cfg,
            Box::new(FakeNet(nets(&[("{OFFICE}", "OFFICE-WIFI")]))),
        );

        let state = sup.reevaluate().await;
        assert_eq!(state.route, Route::Direct);
        assert!(state.demoted, "понижение обязано быть видно в состоянии");
    }

    #[tokio::test]
    async fn run_reevaluates_on_start_and_on_each_event_then_exits_when_the_channel_closes() {
        // `run` — единственное место, которое реально связывает события смены
        // сети с пересчётом; все прочие тесты дёргают `reevaluate` напрямую и
        // эту связку не проверяют. Считаем вызовы источника сетей — по одному
        // на каждый пересчёт — и убеждаемся, что их ровно N+1 (старт плюс N
        // событий), а сам `run` не зависает, когда канал закрылся.
        struct CountingNet(Arc<std::sync::atomic::AtomicUsize>);
        impl NetworkSource for CountingNet {
            fn connected(&self) -> Result<Vec<ConnectedNetwork>, SupervisorError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::new())
            }
        }

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let router = Arc::new(Router::new(Route::Direct));
        let sup = Supervisor::new(
            router,
            Prober::new(Duration::from_secs(30), Duration::from_secs(1)),
            office_config("127.0.0.1:1"),
            Box::new(CountingNet(Arc::clone(&calls))),
        );

        let (tx, rx) = tokio::sync::mpsc::channel::<()>(4);
        for _ in 0..3 {
            tx.send(()).await.unwrap();
        }
        drop(tx);

        tokio::time::timeout(Duration::from_secs(5), sup.run(rx))
            .await
            .expect("run обязан выйти, когда канал закрылся, а не зависнуть");

        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            4,
            "один пересчёт на старте плюс один на каждое из 3 событий"
        );
    }
}
