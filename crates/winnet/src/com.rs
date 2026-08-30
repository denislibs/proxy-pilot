//! Инициализация COM.
//!
//! NLM — COM-объект, и до первого обращения поток обязан войти в апартамент.
//! Держим это стражем, а не свободной функцией: `CoUninitialize` обязан
//! вызваться на том же потоке и ровно столько же раз, сколько `CoInitialize`
//! реально что-то инициализировал.

use std::marker::PhantomData;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};

use crate::WinNetError;

/// Пока жив — поток пригоден для вызовов COM/NLM. Сбрасывать вручную не
/// нужно.
///
/// `PhantomData<*mut ()>` не несёт данных, а лишь запрещает компилятору
/// сделать тип `Send`/`Sync`: апартамент (свой ли, чужой ли — см. `uninit`)
/// привязан к потоку вызова `new()`, и `CoUninitialize`, если он вообще
/// нужен, обязан вызваться там же. Без этой отметки значение можно было бы
/// уронить в `Drop` на чужом потоке из другого `std::thread::spawn`, и COM
/// оказался бы деинициализирован не там, где был инициализирован.
pub struct ComGuard {
    /// Инициализировали ли апартамент именно мы. Если поток уже был в COM —
    /// в частности, в MTA, куда NLM прекрасно можно звать напрямую, —
    /// `CoInitializeEx` вернёт `RPC_E_CHANGED_MODE`: это не ошибка потока,
    /// апартамент просто чужой, и `CoUninitialize` за собой снимать не
    /// наше — это сняло бы чужой счётчик ссылок.
    uninit: bool,
    _not_send: PhantomData<*mut ()>,
}

impl ComGuard {
    pub fn new() -> Result<Self, WinNetError> {
        // SAFETY: вызывается на текущем потоке до какого-либо использования
        // COM/NLM с него. Возврат разбирается ниже: RPC_E_CHANGED_MODE —
        // не наша инициализация, остальной неуспех — ошибка, S_OK/S_FALSE —
        // наша, и тогда парный CoUninitialize при Drop обязателен.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };

        if hr == RPC_E_CHANGED_MODE {
            // Поток уже сидит в другой модели апартамента — например, GUI-
            // тулкит или рантайм-хост инициализировал его как MTA раньше
            // нас. NLM зовётся из MTA так же исправно, как из STA: апартамент
            // — требование именно этого стража, а не самого API. Считаем
            // поток пригодным и ничего не трогаем при разрушении.
            return Ok(Self {
                uninit: false,
                _not_send: PhantomData,
            });
        }
        hr.ok()?;
        Ok(Self {
            uninit: true,
            _not_send: PhantomData,
        })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: парный вызов к CoInitializeEx из new() на том же
            // потоке, и только когда это МЫ подняли счётчик (uninit ==
            // true) — иначе апартамент чужой, и звать CoUninitialize нельзя.
            // Тип не Send, переехать между потоками ему нечем.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guard_created_on_a_bare_thread_owns_its_uninit() {
        // На свежем потоке, где COM ещё никто не поднимал, страж обязан
        // взять деинициализацию на себя.
        let g = ComGuard::new().expect("COM должен подняться");
        assert!(g.uninit, "страж должен считать апартамент своим");
    }

    #[test]
    fn a_second_guard_on_the_same_thread_still_owns_its_uninit() {
        // CoInitializeEx с той же моделью на том же потоке возвращает
        // S_FALSE (счётчик увеличился) — это тоже наша инициализация,
        // и наш Drop обязан её снять.
        let _outer = ComGuard::new().expect("COM должен подняться");
        let inner = ComGuard::new().expect("повторный вход должен быть Ok (S_FALSE)");
        assert!(inner.uninit, "S_FALSE — тоже наша инициализация");
    }

    #[test]
    fn a_guard_on_a_thread_already_in_mta_does_not_own_its_uninit() {
        // Это ровно сценарий бага из ревью: хост (GUI-тулкит, рантайм)
        // успел ввести поток в MTA раньше нас. CoInitializeEx для нашей
        // STA-модели тогда вернёт RPC_E_CHANGED_MODE — не ошибку, а знак,
        // что апартамент чужой. Страж обязан завестись (Ok), но не имеет
        // права звать CoUninitialize за хозяином. Гоняем на отдельном
        // потоке, чтобы не задеть состояние COM других тестов, которые
        // может исполнять тот же пул потоков test-harness.
        std::thread::spawn(|| {
            // SAFETY: поток свежий (только что создан), никакой другой код
            // с ним ещё не работал; парный CoUninitialize — двумя строками
            // ниже, на этом же потоке.
            unsafe {
                CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED)
                    .ok()
                    .expect("MTA должен подняться");
            }

            let g = ComGuard::new().expect("RPC_E_CHANGED_MODE — не ошибка стража");
            assert!(!g.uninit, "апартамент чужой: снимать его не наше дело");
            // `uninit == false` гарантирует (через if в Drop), что вот этот
            // drop ничего не вызовет — счётчик MTA-апартамента останется
            // ровно таким, каким его оставил CoInitializeEx выше.
            drop(g);

            // SAFETY: единственный парный вызов нашему CoInitializeEx выше,
            // на том же потоке — ровно один раз, счётчик закрывается сам,
            // без участия стража (у него нечего снимать).
            unsafe { CoUninitialize() };
        })
        .join()
        .expect("поток с MTA-сценарием не должен паниковать");
    }
}
