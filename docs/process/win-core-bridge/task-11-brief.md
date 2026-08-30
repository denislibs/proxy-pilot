### Task 11: CI

**Files:**
- Create: `.github/workflows/win.yml`

**Interfaces:**
- Consumes: весь воркспейс.
- Produces: проверку на каждый push и PR.

- [ ] **Step 1: Написать конфигурацию**

`.github/workflows/win.yml`:

```yaml
name: Windows build

on:
  push:
    branches: [main]
    paths: ["win/**", ".github/workflows/win.yml"]
  pull_request:
    paths: ["win/**", ".github/workflows/win.yml"]
  workflow_dispatch:

defaults:
  run:
    working-directory: win

jobs:
  check:
    name: Тесты и линтеры
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt

      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: win

      - name: Форматирование
        run: cargo fmt --check

      - name: Клиппи
        run: cargo clippy --all-targets -- -D warnings

      - name: Тесты
        run: cargo test --all

      - name: Сборка релиза
        run: cargo build --release -p proxypilot-bridge

      - uses: actions/upload-artifact@v4
        with:
          name: proxypilot-bridge
          path: win/target/release/proxypilot-bridge.exe
          retention-days: 14
```

- [ ] **Step 2: Проверить локально то же, что проверяет CI**

Run: `cd win && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all`
Expected: всё зелёное. Если `clippy` ругается — чини, а не глуши `#[allow]`.

- [ ] **Step 3: Коммит**

```bash
git add .github/workflows/win.yml
git commit -m "ci(win): тесты, клиппи, формат и сборка релиза"
```

---

## Что этот план НЕ делает

Осознанно вне объёма, приходит следующими планами:

- **План 2:** `winnet` — опознание офиса через NLM, события смены сети, системный прокси в реестре, проба живости апстримов; трей и переключение режимов. После него это приложение Windows, а не консольная утилита.
- **План 3:** окно настроек, `bench`, `doctor`, VPN, служба статического IP, подпись и поставка.

Из спеки в этот план сознательно не вошли: авторизация на апстриме (5.4), keep-alive для обычного HTTP (5.3), IPv6-апстримы (13), собственный резолвер (2.2 — это была болезнь macOS).
