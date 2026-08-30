//! Текущий маршрут моста.
//!
//! Смена маршрута обязана быть атомарной и НЕ трогать установленные
//! соединения: каждое соединение читает маршрут один раз, в момент приёма,
//! и дальше живёт с этим значением. Именно поэтому в macOS-версии был нужен
//! трёхуровневый антифлаппинг — там смена режима перезапускала gost и рвала
//! всё живое. Здесь рвать нечего.

use std::sync::Arc;

use arc_swap::ArcSwap;
use proxypilot_core::mode::Route;

#[derive(Debug)]
pub struct Router {
    current: ArcSwap<Route>,
}

impl Router {
    pub fn new(route: Route) -> Self {
        Self {
            current: ArcSwap::from_pointee(route),
        }
    }

    /// Снимок маршрута. Держатель снимка не заметит последующих `set`.
    pub fn get(&self) -> Arc<Route> {
        self.current.load_full()
    }

    pub fn set(&self, route: Route) {
        self.current.store(Arc::new(route));
    }

    /// Публикует маршрут, только если он отличается от уже опубликованного.
    /// Возвращает `true`, если публикация произошла.
    ///
    /// Существует ради супервизора: лишний `set` безвреден для установленных
    /// соединений, но маскирует ошибки в логике решения и засоряет лог.
    ///
    /// Собрано через `rcu`, а не через отдельные `load` и `store`: это
    /// честный compare-and-swap, а не «прочитали — решили — записали» с
    /// окном между чтением и записью. Сегодня у маршрута ровно один писатель
    /// (супервизор), и окно не выстрелит при всём желании, но метод — `pub`
    /// на структуре, и следующий план заводит второго писателя (окно
    /// настроек). Без настоящего CAS два писателя, решившие одно и то же
    /// одновременно, оба прошли бы проверку «отличается» и оба записали бы —
    /// вместо одной строки в логе получили бы две, и сама гарантия метода
    /// перестала бы что-либо гарантировать. `rcu` при столкновении просто
    /// перечитывает свежее значение и повторяет сравнение сам, поэтому
    /// сравнение и запись здесь неразделимы независимо от числа писателей.
    ///
    /// Через приватный `current` напрямую, а не через `get()`: `get()` обязан
    /// остаться с ровно одним вызовом на пути обработки соединений, и заводить
    /// второй ради супервизора незачем.
    pub fn set_if_changed(&self, route: Route) -> bool {
        let new = Arc::new(route);
        let mut changed = false;
        self.current.rcu(|current| {
            changed = **current != *new;
            if changed {
                Arc::clone(&new)
            } else {
                Arc::clone(current)
            }
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn returns_the_route_it_was_built_with() {
        let r = Router::new(Route::Direct);
        assert_eq!(*r.get(), Route::Direct);
    }

    #[test]
    fn set_replaces_the_route_for_later_readers() {
        let r = Router::new(Route::Direct);
        r.set(Route::Socks("10.0.0.2:9999".into()));
        assert_eq!(*r.get(), Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn a_handle_taken_before_set_keeps_the_old_route() {
        // Это и есть свойство, ради которого писался свой мост: соединение
        // взяло маршрут в момент установки и доживает по нему, что бы ни
        // переключили потом.
        let r = Router::new(Route::Socks("old:1080".into()));
        let held = r.get();
        r.set(Route::Direct);
        assert_eq!(*held, Route::Socks("old:1080".into()));
        assert_eq!(*r.get(), Route::Direct);
    }

    #[test]
    fn set_if_changed_skips_a_matching_value() {
        let r = Router::new(Route::Direct);
        let before = r.get();
        assert!(!r.set_if_changed(Route::Direct));
        assert!(
            Arc::ptr_eq(&before, &r.get()),
            "значение то же — публиковать нечего"
        );
    }

    #[test]
    fn set_if_changed_publishes_a_different_value() {
        let r = Router::new(Route::Direct);
        assert!(r.set_if_changed(Route::Socks("10.0.0.2:9999".into())));
        assert_eq!(*r.get(), Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn set_if_changed_reports_exactly_one_winner_under_concurrent_writers() {
        // Раньше это была проверка-потом-действие (load_full + store): два
        // писателя, решившие одно и то же одновременно, оба прошли бы
        // проверку «отличается» и оба записали бы — вместо одной строки в
        // логе получили бы две. rcu делает сравнение и запись неразделимыми,
        // поэтому ровно один поток обязан увидеть true.
        let r = Arc::new(Router::new(Route::Direct));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let r = Arc::clone(&r);
                std::thread::spawn(move || r.set_if_changed(Route::Socks("10.0.0.2:9999".into())))
            })
            .collect();
        let winners = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|&changed| changed)
            .count();
        assert_eq!(winners, 1, "ровно один писатель обязан увидеть смену");
        assert_eq!(*r.get(), Route::Socks("10.0.0.2:9999".into()));
    }

    #[test]
    fn is_shareable_across_threads() {
        let r = Arc::new(Router::new(Route::Direct));
        let r2 = Arc::clone(&r);
        let t = std::thread::spawn(move || {
            r2.set(Route::Http("p:3128".into()));
        });
        t.join().unwrap();
        assert_eq!(*r.get(), Route::Http("p:3128".into()));
    }
}
