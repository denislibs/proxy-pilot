//! Растеризация иконок трея — чистая математика, без платформенных
//! зависимостей.
//!
//! Вынесено из `proxypilot-app` в отдельный крейт, потому что этот же код
//! нужен `build.rs`, который порождает `.ico` для ресурсов exe: файла `.ico`
//! в проекте нет, иконка трея рисуется из сырых RGBA через
//! `Icon::from_rgba` (см. `proxypilot-app/src/icons.rs`), и заводить для
//! ресурсов exe вторую, нарисованную вручную картинку значит завести
//! артефакт, который разойдётся с тем, что показывает трей. Общий крейт —
//! единственный способ, чтобы `build.rs` и трей рисовали ровно одно и то же
//! ровно одной функцией.
//!
//! Иконка здесь — четыре сплошные фигуры, и цвет один не годится: в трее
//! иконка 16×16 и рядом с чужими значками, а часть людей цвета не
//! различает. Поэтому состояния отличаются ещё и формой — диск, кольцо,
//! кольцо с чертой.

/// Сторона иконки в пикселях.
pub const ICON_SIDE: u32 = 32;

/// Какую иконку трея рисовать.
///
/// Смысл каждого состояния (какому маршруту какая иконка соответствует)
/// живёт в `proxypilot-app::icons::icon_for` — этот крейт знает только про
/// форму и цвет, не про `AppState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Socks,
    Http,
    Direct,
    /// Апстримы не заданы вообще — состояние «требует действия», а не
    /// «пользователь так выбрал».
    Unconfigured,
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
