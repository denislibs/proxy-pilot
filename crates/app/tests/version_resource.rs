//! Версия и метаданные, вшитые `build.rs` в ресурсы `proxypilot.exe`,
//! обязаны совпадать с `workspace.package.version` — версия задаётся в
//! одном месте (`CLAUDE.md`), и это единственный тест, который ловит
//! расхождение механически, а не на глаз в свойствах файла.
//!
//! Тест читает реальный `VS_FIXEDFILEINFO` из уже собранного бинаря через
//! те же API (`version.dll`), которыми пользуется Проводник — не
//! пересчитывает ожидаемое значение из тех же исходных данных, что и
//! `build.rs`, а проверяет то, что действительно легло в exe.

use std::ffi::c_void;
use std::path::Path;

use windows::core::{w, HSTRING};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};

/// Читает блок `VS_FIXEDFILEINFO` из ресурсов файла `path`.
fn read_fixed_file_info(path: &Path) -> VS_FIXEDFILEINFO {
    let wide = HSTRING::from(path.as_os_str());

    // SAFETY: `GetFileVersionInfoSizeW` только сообщает размер блока версии
    // (без записи через указатель — `lpdwhandle: None`); буфер `buf` под
    // `GetFileVersionInfoW` выделен ровно этим размером, так что запись не
    // выходит за его границы; `VerQueryValueW` с подблоком `\` по контракту
    // WinAPI отдаёт указатель ВНУТРЬ уже заполненного `buf`, а не в новую
    // память, поэтому разыменование `info_ptr` ниже читает память, которую
    // `buf` держит живой до конца функции.
    unsafe {
        let size = GetFileVersionInfoSizeW(&wide, None);
        assert!(
            size > 0,
            "у {path:?} нет блока версии в ресурсах — build.rs не вшил VERSIONINFO"
        );

        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(&wide, 0, size, buf.as_mut_ptr() as *mut c_void)
            .expect("GetFileVersionInfoW не прочитала блок версии");

        let mut info_ptr: *mut c_void = std::ptr::null_mut();
        let mut info_len: u32 = 0;
        let ok = VerQueryValueW(
            buf.as_ptr() as *const c_void,
            w!("\\"),
            &mut info_ptr,
            &mut info_len,
        );
        assert!(
            ok.as_bool(),
            "VerQueryValueW(\"\\\\\") не нашёл VS_FIXEDFILEINFO в блоке версии"
        );
        assert!(
            info_len as usize >= std::mem::size_of::<VS_FIXEDFILEINFO>(),
            "блок короче VS_FIXEDFILEINFO"
        );

        *(info_ptr as *const VS_FIXEDFILEINFO)
    }
}

#[test]
fn embedded_version_matches_crate_version() {
    // `CARGO_BIN_EXE_<name>` собирает `proxypilot.exe` перед тестом и
    // указывает на реальный файл — то же самое дерево ресурсов, что получит
    // человек, которому прислали exe.
    let exe = std::env::var("CARGO_BIN_EXE_proxypilot")
        .expect("cargo не выставил CARGO_BIN_EXE_proxypilot для интеграционного теста");
    let info = read_fixed_file_info(Path::new(&exe));

    let major: u16 = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap();
    let minor: u16 = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap();
    let patch: u16 = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap();
    let expected = (major, minor, patch);

    let file_version = (
        (info.dwFileVersionMS >> 16) as u16,
        (info.dwFileVersionMS & 0xFFFF) as u16,
        (info.dwFileVersionLS >> 16) as u16,
    );
    assert_eq!(
        file_version, expected,
        "FileVersion в ресурсах разошёлся с package.version — build.rs собрал не ту версию"
    );

    let product_version = (
        (info.dwProductVersionMS >> 16) as u16,
        (info.dwProductVersionMS & 0xFFFF) as u16,
        (info.dwProductVersionLS >> 16) as u16,
    );
    assert_eq!(
        product_version, expected,
        "ProductVersion в ресурсах разошёлся с package.version"
    );
}
