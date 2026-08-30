### Task 6: Системный прокси в реестре

**Files:**
- Create: `win/crates/winnet/src/sysproxy.rs`
- Modify: `win/crates/winnet/src/lib.rs`
- Modify: `win/crates/winnet/Cargo.toml`

**Interfaces:**
- Consumes: `WinNetError`.
- Produces: `SysProxy { enabled: bool, server: String, bypass: String }`, `read() -> Result<SysProxy, WinNetError>`, `apply(&SysProxy) -> Result<(), WinNetError>`, `to_bypass_string(no_proxy: &str) -> String`.

**Почему мы этим управляем сами.** На macOS пользователю предписано выставить прокси руками один раз. Здесь прав администратора не нужно — это `HKCU`, — поэтому приложение делает это само. Взамен появляется обязанность: **при падении процесса в реестре останется указатель на мёртвый слушатель, и пользователь останется без сети вообще** — отказ хуже того, который мы лечим. Поэтому прежнее значение сохраняется в конфиг ДО записи в реестр, и восстанавливается при следующем старте (спека 6.3).

Что этим не покрывается и должно быть честно сказано в UI: **WinHTTP** (`netsh winhttp`, контекст служб, нужен администратор), **Firefox** (свои настройки мимо WinINET), и приложения, читающие `HTTP_PROXY` из окружения.

- [ ] **Step 1: Добавить фичи windows-rs**

В `win/crates/winnet/Cargo.toml`, в список features: `"Win32_System_Registry"`, `"Win32_Networking_WinInet"`.

- [ ] **Step 2: Написать падающий тест**

`win/crates/winnet/src/sysproxy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_string_uses_semicolons_and_keeps_local_token() {
        // WinINET разделяет точкой с запятой, а не запятой, и понимает
        // особый токен <local> для адресов без точки.
        let s = to_bypass_string("localhost,127.0.0.1,.local,192.168.0.0/16");
        assert!(s.contains(';'), "получили: {s}");
        assert!(!s.contains(','), "запятых остаться не должно: {s}");
        assert!(s.contains("<local>"), "локальные имена без точки: {s}");
    }

    #[test]
    fn bypass_string_converts_dot_suffix_to_wildcard() {
        // «.local» в нашем формате — суффикс; WinINET ждёт «*.local».
        let s = to_bypass_string(".local");
        assert!(s.contains("*.local"), "получили: {s}");
    }

    #[test]
    fn bypass_string_skips_empty_entries() {
        let s = to_bypass_string("localhost,,  ,127.0.0.1");
        assert!(!s.contains(";;"), "получили: {s}");
    }

    #[cfg(windows)]
    #[test]
    fn reading_current_settings_does_not_fail() {
        // Смоук на живой машине: ключ существует всегда, даже когда прокси
        // выключен. Ничего не меняем — только читаем.
        let s = read().expect("HKCU Internet Settings обязан читаться");
        // enabled может быть любым; проверяем лишь, что структура заполнена
        let _ = (s.enabled, s.server.len(), s.bypass.len());
    }
}
```

- [ ] **Step 3: Проверить, что тест падает**

Run: `cd win && cargo test -p proxypilot-winnet sysproxy`
Expected: FAIL — модуля нет.

- [ ] **Step 4: Написать реализацию**

```rust
//! Системные настройки прокси (WinINET).
//!
//! Живут в HKCU, поэтому прав администратора не нужно и приложение
//! управляет ими само — в отличие от macOS-версии, где это делал человек.
//!
//! Плата за это — обязанность прибраться. Если процесс упадёт, в реестре
//! останется указатель на мёртвый слушатель, и пользователь окажется без
//! сети вообще: отказ хуже того, который мы лечим. Поэтому прежнее значение
//! сохраняется в конфиг ДО записи сюда и восстанавливается при старте.
//!
//! Что этим НЕ покрывается: WinHTTP (контекст служб, нужен администратор),
//! Firefox (свои настройки), и приложения, читающие HTTP_PROXY из окружения.

use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinInet::{
    InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ,
};

use crate::WinNetError;

const SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SysProxy {
    pub enabled: bool,
    /// `127.0.0.1:3129` либо пусто
    pub server: String,
    /// список исключений в формате WinINET (через `;`)
    pub bypass: String,
}

/// Наш список исключений → формат WinINET.
///
/// Отличия от нашего: разделитель `;`, суффикс пишется как `*.local`,
/// и есть особый токен `<local>` — адреса без точки в имени.
pub fn to_bypass_string(no_proxy: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for raw in no_proxy.split(',') {
        let e = raw.trim();
        if e.is_empty() {
            continue;
        }
        if let Some(sfx) = e.strip_prefix('.') {
            parts.push(format!("*.{sfx}"));
        } else {
            parts.push(e.to_string());
        }
    }
    parts.push("<local>".to_string());
    parts.join(";")
}

pub fn read() -> Result<SysProxy, WinNetError> { /* RegOpenKeyExW(KEY_READ) + RegQueryValueExW */ }

pub fn apply(p: &SysProxy) -> Result<(), WinNetError> {
    /* RegSetValueExW ProxyEnable/ProxyServer/ProxyOverride, затем:
       InternetSetOptionW(None, INTERNET_OPTION_SETTINGS_CHANGED, None, 0)
       InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0)
       Без этих двух вызовов уже запущенные приложения продолжат ходить
       по старым настройкам до перезапуска. */
}
```

Реализацию `read`/`apply` пиши целиком — оставленные заглушки в этом плане только чтобы не диктовать построчно обвязку `RegQueryValueExW` с её двойным вызовом за размером буфера. Требования: `ProxyEnable` — `REG_DWORD` 0/1, `ProxyServer` и `ProxyOverride` — `REG_SZ` в UTF-16 с завершающим нулём; ключ открывается с `KEY_READ`/`KEY_WRITE` и закрывается через `RegCloseKey` в любом случае, включая ошибочный путь.

- [ ] **Step 5: Ручная проверка — и обязательно вернуть как было**

Прочитай текущие настройки, применить свои, убедиться в System Settings → Network → Proxy, что значение изменилось, **вернуть исходное**. Приложи в отчёт вывод «до», «после» и «вернули».

- [ ] **Step 6: Прогнать тесты и линтеры, закоммитить**

```bash
git add win/crates/winnet
git commit -m "feat(win): системный прокси через реестр и InternetSetOption"
```

---

