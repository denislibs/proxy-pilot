### Task 6: Автозапуск

**Files:**
- Create: `win/crates/winnet/src/autostart.rs`
- Modify: `win/crates/winnet/src/lib.rs`

**Interfaces:** `is_enabled() -> Result<bool, WinNetError>`, `enable(exe: &Path) -> Result<(), WinNetError>`, `disable() -> Result<(), WinNetError>`.

Ключ `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, значение `ProxyPilot`. **`HKCU`, а не `HKLM`** — второе требует администратора, а весь план обязан обходиться без него.

Путь пишется в кавычках: без них путь с пробелами (`C:\Program Files\…`) Windows разберёт неверно. `is_enabled` сверяет не только наличие значения, но и то, что оно указывает на **этот** экземпляр: иначе перенос exe оставит запись, ведущую в никуда, а тумблер будет показывать «включено».

- [ ] Шаги по обычному циклу; ручная проверка: включить, увидеть запись в реестре, выключить, убедиться что удалена.

---

