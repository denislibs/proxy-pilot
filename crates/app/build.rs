//! Вшивает в `proxypilot.exe` ресурсы Windows: версию, метаданные файла и
//! иконку приложения.
//!
//! Версия задаётся в одном месте — `workspace.package.version` — и приходит
//! сюда через `CARGO_PKG_VERSION*`, которые cargo сам выставляет из
//! `Cargo.toml` этого же крейта (`version.workspace = true`). Второго места
//! для версии здесь нет: `winres::WindowsResource::new()` уже заполняет
//! `FileVersion`/`ProductVersion` из этих переменных, так что расхождение
//! исключено конструктивно, а не проверкой постфактум. Проверяет это
//! `tests/version_resource.rs` — читает `VS_FIXEDFILEINFO` уже собранного
//! exe и роняет `cargo test`, если версия в ресурсах не совпадает с
//! `package.version`.
//!
//! Иконки в проекте нет файлом: трей рисует их программно
//! (`proxypilot-icon`, см. комментарий в её `lib.rs`). Ресурсам exe нужен
//! именно файл `.ico`, поэтому он порождается здесь же, при сборке, той же
//! функцией `proxypilot_icon::rgba`, что рисует иконку трея, — а не второй,
//! нарисованной вручную картинкой, которая рано или поздно разойдётся с
//! тем, что видно в трее.

use std::env;
use std::path::PathBuf;

use proxypilot_icon::{rgba, IconKind, ICON_SIDE};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let icon_path = write_icon();

    let mut res = winres::WindowsResource::new();
    // `FileVersion`/`ProductVersion` winres уже взял из CARGO_PKG_VERSION —
    // трогать не нужно, это и есть «одно место».
    res.set_icon(
        icon_path
            .to_str()
            .expect("путь к .ico вне OUT_DIR содержит не-UTF8 символы"),
    );
    res.set("CompanyName", "ProxyPilot");
    res.set("ProductName", "ProxyPilot");
    // Латиницей: `winres` пишет .rc с `#pragma code_page(65001)`, но у
    // `rc.exe` из Windows Kits 10.0.22000.0 это не спасает кириллицу — она
    // приходит в собранный exe испорченной (проверено: `(Get-Item
    // .\proxypilot.exe).VersionInfo.FileDescription` показывает мусор вместо
    // букв). Поле читают Проводник и эвристики антивируса на любой машине,
    // а не только с этим SDK, так что риск не стоит того, чтобы держать
    // здесь кириллицу.
    res.set("FileDescription", "ProxyPilot HTTP-CONNECT proxy bridge");
    // Имя, под которым бинарь известен диспетчеру задач и автозапуску
    // (`[[bin]] name` в Cargo.toml) — фиксируем его в ресурсах явно, а не
    // полагаемся на то, что оно совпадёт с именем пакета `proxypilot-app`.
    res.set("OriginalFilename", "proxypilot.exe");

    res.compile()
        .expect("не вшить ресурсы Windows (версия/метаданные/иконка) в exe");
}

/// Растеризует иконку трея в `.ico` и кладёт его в `OUT_DIR`.
///
/// Выбрана `IconKind::Direct` — нейтральное серое кольцо, «сквозной проход»
/// без апстрима: у приложения нет отдельного логотипа, а из четырёх
/// состояний трея это единственное, что не несёт значения предупреждения
/// (`Unconfigured`, оранжевая) и не привязано к конкретному протоколу
/// (`Socks`/`Http`, зелёная/синяя) — то есть меньше остальных похоже на
/// случайный выбор одного из состояний трея в качестве лица программы.
fn write_icon() -> PathBuf {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo не выставил OUT_DIR"));
    let path = out_dir.join("app.ico");

    let pixels = rgba(IconKind::Direct);
    let image = ico::IconImage::from_rgba_data(ICON_SIDE, ICON_SIDE, pixels);
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    dir.add_entry(ico::IconDirEntry::encode(&image).expect("закодировать растр иконки в .ico"));

    let file = std::fs::File::create(&path)
        .unwrap_or_else(|e| panic!("создать {path:?} для сгенерированной иконки: {e}"));
    dir.write(file)
        .unwrap_or_else(|e| panic!("записать сгенерированную иконку в {path:?}: {e}"));

    path
}
