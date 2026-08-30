use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_proxypilot-bridge")
}

#[test]
fn rejects_an_invalid_upstream() {
    let out = Command::new(bin())
        .args(["--socks", "нет-порта"])
        .output()
        .expect("бинарь должен запускаться");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("host:port"));
}

#[test]
fn prints_usage_on_help() {
    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--port"));
    assert!(text.contains("--socks"));
}
