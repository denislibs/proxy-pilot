### Task 10: Сборка и CI

**Files:**
- Modify: `win/crates/app/src/main.rs`, `.github/workflows/win.yml`

- [ ] **Step 1:** Добавить `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` — в релизе консольного окна быть не должно, в отладочной сборке оно нужно для логов.
- [ ] **Step 2:** В CI добавить `cargo build --release -p proxypilot-app` и выгрузку `proxypilot.exe` артефактом рядом с существующим `proxypilot-bridge.exe`.
- [ ] **Step 3:** Прогнать все три проверки локально, закоммитить: `ci(win): сборка приложения`.

Автозапуск, подпись и обновления — план 3.

---

### Task 10: Сборка приложения и CI

- Приложение собирается как `windows_subsystem = "windows"` (без консольного окна), CLI-бинарь моста остаётся для отладки.
- CI: добавить сборку `proxypilot-app`, оставить существующие три проверки.
- Автозапуск и обновления — план 3.

---

## Что этот план НЕ делает

Уходит в план 3: окно настроек, `bench` и `doctor`, VPN через OpenVPN GUI, служба статического IP, автозапуск, подпись Authenticode, упаковка и обновления.
