//! Иконки трея: какую из четырёх фигур `proxypilot-icon` показать при
//! данном `AppState`.
//!
//! Сама растеризация (цвет, форма, RGBA-буфер) — в крейте `proxypilot-icon`,
//! который делят трей (здесь, во время выполнения) и `build.rs` (генерация
//! `.ico` для ресурсов exe, во время сборки). Здесь остаётся только то, что
//! специфично приложению: связь состояния моста с видом иконки.

use proxypilot_bridge::supervisor::AppState;
use proxypilot_core::mode::{Mode, Reachability, Route};

pub use proxypilot_icon::{rgba, IconKind, ICON_SIDE};

/// Какую иконку показывать при таком состоянии.
///
/// Нет случая «мост не запущен»: приложение выходит, как только мост
/// перестаёт принимать соединения (`BRIDGE_STOPPED` в `main.rs`) — иконки,
/// которая показывала бы это состояние, некому быть на экране. Кто
/// соберётся вернуть пятое состояние, обязан сначала вернуть условие, при
/// котором оно достижимо: приложение, продолжающее жить с мёртвым мостом.
///
/// «Не настроено» распознаётся по здоровью апстримов: `Reachability::Unknown`
/// выставляется пробой ровно тогда, когда адреса нет (см. `probe::Prober`) —
/// пробовать нечего. Явно выбранный режим «Напрямую» под это не подпадает:
/// там ничего настраивать и не требовалось.
pub fn icon_for(state: &AppState) -> IconKind {
    match &state.route {
        Route::Socks(_) => IconKind::Socks,
        Route::Http(_) => IconKind::Http,
        Route::Direct
            if state.mode != Mode::Direct
                && state.health.socks == Reachability::Unknown
                && state.health.http == Reachability::Unknown =>
        {
            IconKind::Unconfigured
        }
        Route::Direct => IconKind::Direct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::mode::{Health, Mode, Place, Reachability, Route};

    fn state(route: Route, demoted: bool) -> AppState {
        AppState {
            mode: Mode::Auto,
            route,
            demoted,
            place: Place {
                in_office: true,
                network: None,
                network_name: None,
            },
            health: Health {
                socks: Reachability::Up,
                http: Reachability::Up,
            },
            port: 3129,
        }
    }

    #[test]
    fn icon_reflects_the_active_route() {
        assert_eq!(
            icon_for(&state(Route::Socks("x:1".into()), false)),
            IconKind::Socks
        );
        assert_eq!(
            icon_for(&state(Route::Http("x:1".into()), false)),
            IconKind::Http
        );
        assert_eq!(icon_for(&state(Route::Direct, false)), IconKind::Direct);
    }

    #[test]
    fn nothing_configured_gets_its_own_icon() {
        // «Не настроено» и «пользователь выбрал напрямую» — разные вещи:
        // первое требует действия, второе нет. Апстрим без адреса даёт
        // Reachability::Unknown — «не пробовали», потому что пробовать нечего.
        let mut s = state(Route::Direct, false);
        s.health = Health {
            socks: Reachability::Unknown,
            http: Reachability::Unknown,
        };
        assert_eq!(icon_for(&s), IconKind::Unconfigured);
    }

    #[test]
    fn a_deliberate_direct_mode_is_not_unconfigured() {
        let mut s = state(Route::Direct, false);
        s.mode = Mode::Direct;
        s.health = Health {
            socks: Reachability::Unknown,
            http: Reachability::Unknown,
        };
        assert_eq!(icon_for(&s), IconKind::Direct);
    }
}
