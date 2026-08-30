//! Иконки трея, нарисованные программно.
//!
//! Из сырых RGBA через `Icon::from_rgba`, без файлов ресурсов: иконка здесь
//! — четыре сплошные фигуры, и таскать ради них .ico в сборку значит завести
//! артефакт, который придётся синхронизировать руками с этим перечислением.
//!
//! Цвет один не годится: в трее иконка 16×16 и рядом с чужими значками, а
//! часть людей цвета не различает. Поэтому состояния отличаются ещё и
//! формой — диск, кольцо, кольцо с чертой.

use proxypilot_bridge::supervisor::AppState;
use proxypilot_core::mode::{Mode, Reachability, Route};

/// Сторона иконки в пикселях.
pub const ICON_SIDE: u32 = 32;

/// Состояний четыре, а спека 11.1 перечисляет пять.
///
/// Нет «мост не запущен»: приложение выходит, как только мост перестаёт
/// принимать соединения (`BRIDGE_STOPPED` в `main.rs`) — иконки, которая
/// показывала бы это состояние, некому быть на экране. Кто соберётся
/// вернуть пятое состояние, обязан сначала вернуть условие, при котором оно
/// достижимо: приложение, продолжающее жить с мёртвым мостом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Socks,
    Http,
    Direct,
    /// Апстримы не заданы вообще — состояние «требует действия», а не
    /// «пользователь так выбрал».
    Unconfigured,
}

/// Какую иконку показывать при таком состоянии.
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

/// Цвет заливки (RGB) для каждого состояния.
fn colour(kind: IconKind) -> [u8; 3] {
    match kind {
        IconKind::Socks => [46, 160, 67],
        IconKind::Http => [31, 111, 235],
        IconKind::Direct => [139, 148, 158],
        IconKind::Unconfigured => [219, 109, 40],
    }
}

/// Внутренний радиус кольца в долях внешнего; 0 — сплошной диск.
fn inner_ratio(kind: IconKind) -> f32 {
    match kind {
        // Сплошной диск: трафик идёт через апстрим.
        IconKind::Socks => 0.0,
        // Кольцо потолще — HTTP отличается от SOCKS5 и формой тоже, а не
        // только цветом.
        IconKind::Http => 0.45,
        // Тонкое кольцо: «сквозной» проход, ничего внутри.
        IconKind::Direct => 0.62,
        IconKind::Unconfigured => 0.62,
    }
}

/// RGBA-буфер иконки: `ICON_SIDE * ICON_SIDE * 4` байта, premultiplied не
/// требуется — `Icon::from_rgba` ждёт обычную straight-alpha картинку.
///
/// Сглаживание — по покрытию: альфа считается из расстояния до края фигуры,
/// иначе на 16×16 круг превращается в лесенку.
pub fn rgba(kind: IconKind) -> Vec<u8> {
    let side = ICON_SIDE as f32;
    let [r, g, b] = colour(kind);
    let centre = side / 2.0;
    let outer = side / 2.0 - 1.5;
    let inner = outer * inner_ratio(kind);
    // Диагональная черта у «не настроено»: половина ширины полосы.
    let slash = if kind == IconKind::Unconfigured {
        Some(side * 0.09)
    } else {
        None
    };

    let mut px = Vec::with_capacity((ICON_SIDE * ICON_SIDE * 4) as usize);
    for y in 0..ICON_SIDE {
        for x in 0..ICON_SIDE {
            // Центр пикселя, а не его угол: иначе фигура уезжает на полпикселя.
            let dx = x as f32 + 0.5 - centre;
            let dy = y as f32 + 0.5 - centre;
            let d = (dx * dx + dy * dy).sqrt();

            let mut a = coverage(outer - d);
            if inner > 0.0 {
                a = a.min(coverage(d - inner));
            }
            if let Some(half) = slash {
                // Расстояние до прямой y = x (нормаль (1,-1)/√2).
                let to_line = (dx - dy).abs() / std::f32::consts::SQRT_2;
                // Внутри полосы — прозрачно: черта «прорезает» кольцо, а не
                // рисуется поверх. Так она видна на любом фоне трея.
                a = a.min(coverage(to_line - half));
            }

            let alpha = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            px.extend_from_slice(&[r, g, b, alpha]);
        }
    }
    px
}

/// Покрытие пикселя по знаковому расстоянию до границы: примерно один
/// пиксель перехода.
fn coverage(signed_distance: f32) -> f32 {
    (signed_distance + 0.5).clamp(0.0, 1.0)
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

    #[test]
    fn every_icon_is_a_full_rgba_buffer() {
        for kind in [
            IconKind::Socks,
            IconKind::Http,
            IconKind::Direct,
            IconKind::Unconfigured,
        ] {
            let px = rgba(kind);
            assert_eq!(
                px.len(),
                (ICON_SIDE * ICON_SIDE * 4) as usize,
                "{kind:?}: Icon::from_rgba требует ровно side*side*4 байта"
            );
            assert!(
                px.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "{kind:?}: полностью прозрачная иконка невидима в трее"
            );
        }
    }

    #[test]
    fn icons_differ_from_each_other() {
        // Иконка существует, чтобы отличать состояния взглядом; две
        // одинаковых картинки — это отсутствие индикации.
        let all = [
            rgba(IconKind::Socks),
            rgba(IconKind::Http),
            rgba(IconKind::Direct),
            rgba(IconKind::Unconfigured),
        ];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "иконки {i} и {j} совпадают");
            }
        }
    }
}
