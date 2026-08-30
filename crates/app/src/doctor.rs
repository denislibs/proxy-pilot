//! Диагностика: сводит уже известные и уже прочитанные факты в список
//! проверок, которые отвечают на реальные жалобы пользователей.
//!
//! [`run_checks`] — чистая функция. Она не читает реестр, не спрашивает NLM
//! и не стучится в свой же порт: всё это обязан сделать вызывающий и передать
//! сюда уже готовым результатом (в том числе неудачным — `Result`, а не
//! панику или молчаливую подмену значения). Ради этого разделения весь модуль
//! и затевался: диагностика, которая сама лезет в систему, непроверяема, а
//! непроверяемой диагностике никто не будет доверять при разборе жалобы.
//!
//! Сейчас у `run_checks` один вызывающий — старт процесса (`main.rs`), и он
//! передаёт ДВА РАЗНЫХ факта под этим именем, а не один общий на обе
//! проверки, которым они пользовались раньше:
//!
//! - `bridge_listening_now` — слушает ли мост СЕЙЧАС, в момент вызова. На
//!   старте это всегда `true`: `bind` уже отработал и `serve` уже запущен
//!   (см. `main.rs`), так что сокет действительно принимает соединения.
//! - `port_was_free_before_bind` — был ли порт свободен ДО нашего `bind`.
//!   Тоже логически выведено, а не измерено новым сетевым вызовом: раз мы
//!   вообще дошли до этой строки, `bind` не отказал кодом «адрес занят», а
//!   значит непосредственно перед ним порт был свободен.
//!
//! Смешивать их — в точности та ошибка, которую этот комментарий
//! существует, чтобы не дать повторить: единственное на двоих значение
//! приводило к тому, что «мост слушает свой порт» на каждом старте кричала
//! `Fail`, хотя мост только что поднялся, — самая частая жалоба таблицы
//! брифа превращалась в гарантированный ложный тревожный сигнал. Проверка
//! «в реестре наш адрес, но моста нет», наоборот, обязана смотреть в
//! прошлое: `sysproxy` — значение, которое `take_over` (или, при выключенном
//! управлении, `warn_if_stale_pointer_left_behind`) уже прочитали ДО того,
//! как (не) тронули реестр. Только так эта проверка вообще может увидеть
//! мёртвый указатель: второе, независимое чтение после починки застало бы
//! уже наш собственный, только что записанный адрес.
//!
//! Второй вызывающий — кнопка «Диагностика» на странице настроек
//! (`settings_page::live_checks`), и он ЖИВОЙ: по нажатию читает систему
//! заново — подключением к своему порту и свежим `sysproxy::read()` — и
//! передаёт это сюда тем же способом. Это единственный путь, где проверки
//! видят по-настоящему текущее состояние, а не срез момента запуска.
//! Там же `port_was_free_before_bind` получает своё живое значение: вопрос
//! «не отвечал ли там никто» в живом пути звучит как «не отвечает ли там
//! никто сейчас», и честный ответ на него — отрицание `bridge_listening_now`.

use proxypilot_bridge::supervisor::AppState;
use proxypilot_core::config::Config;
use proxypilot_core::mode::{Mode, Reachability};
use proxypilot_winnet::sysproxy::SysProxy;
use tracing::{debug, error, info, warn};

use crate::proxy::is_stale_pointer;

/// Насколько всё плохо. Порядок вариантов — от лучшего к худшему, но
/// сравнение по нему нигде не строится: сортировка проверок в выдаче не
/// нужна, таблица брифа уже задаёт порядок построчно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

/// Одна строка диагностики. `detail` обязан быть не просто диагнозом, а
/// подсказкой, что с этим делать — читает его не разработчик, а человек,
/// у которого «не работает».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub title: String,
    pub status: CheckStatus,
    pub detail: String,
}

fn ok(title: &str, detail: impl Into<String>) -> Check {
    Check {
        title: title.to_string(),
        status: CheckStatus::Ok,
        detail: detail.into(),
    }
}

fn warn_check(title: &str, detail: impl Into<String>) -> Check {
    Check {
        title: title.to_string(),
        status: CheckStatus::Warn,
        detail: detail.into(),
    }
}

fn fail(title: &str, detail: impl Into<String>) -> Check {
    Check {
        title: title.to_string(),
        status: CheckStatus::Fail,
        detail: detail.into(),
    }
}

const TITLE_BRIDGE_LISTENING: &str = "Мост слушает свой порт";
const TITLE_SYSPROXY_POINTS_AT_US: &str = "Системный прокси указывает на нас";
const TITLE_STALE_POINTER: &str = "В реестре наш адрес, но моста нет";
const TITLE_UPSTREAMS: &str = "Апстримы отвечают";
const TITLE_NETWORK_RECOGNISED: &str = "Текущая сеть опознана";
const TITLE_OFFICE_NETWORKS: &str = "Настроены офисные сети";
const TITLE_COVERAGE_GAP: &str = "Что не входит в наше управление";

/// Сводит уже собранные факты в список проверок. Порядок строк — как в
/// таблице брифа: от самой частой жалобы («не работает») к самой тихой
/// (границы управления).
///
/// # Параметры
/// - `bridge_listening_now` — слушает ли мост СЕЙЧАС, `127.0.0.1:{state.port}`.
///   Не выводится из `AppState`: супервизор знает только про выбранный
///   маршрут, а не про то, жив ли сам слушатель (см. инвариант
///   `supervisor.rs`). Используется ТОЛЬКО проверкой «мост слушает свой
///   порт» — не путать со следующим параметром, который смотрит в прошлое,
///   а не в настоящее (см. модульный комментарий про их разделение).
/// - `port_was_free_before_bind` — был ли порт свободен непосредственно
///   ДО того, как мы его заняли. Используется ТОЛЬКО проверкой «в реестре
///   наш адрес, но моста нет»: живой мост сейчас (`bridge_listening_now`)
///   ничего не говорит о том, был ли порт занят кем-то ДРУГИМ в момент,
///   когда реестр в последний раз читали, — а эта проверка обязана судить
///   именно о том, прошлом моменте.
/// - `sysproxy` — результат `sysproxy::read()`. Важно, ЧТО именно прочитано:
///   при старте это обязано быть значение ДО `take_over`, а не после —
///   иначе проверка «в реестре наш адрес, но моста нет» никогда не сможет
///   увидеть мёртвый указатель, потому что мы сами успели его переписать
///   первым делом.
pub fn run_checks(
    cfg: &Config,
    state: &AppState,
    bridge_listening_now: bool,
    port_was_free_before_bind: bool,
    sysproxy: &Result<SysProxy, String>,
) -> Vec<Check> {
    vec![
        check_bridge_listening(state, bridge_listening_now),
        check_sysproxy_points_at_us(cfg, state, sysproxy),
        check_stale_pointer_without_bridge(state, port_was_free_before_bind, sysproxy),
        check_upstreams(cfg, state),
        check_network_recognised(cfg, state),
        check_office_networks_configured(cfg),
        coverage_gap_warning(),
    ]
}

/// Самая частая жалоба — «не работает», а слушателя нет вовсе.
///
/// `bridge_listening_now`, а не `port_was_free_before_bind` из соседней
/// проверки: эта строка отвечает на вопрос «работает ли мост СЕЙЧАС», и
/// подсовывать сюда факт из прошлого — ровно та путаница, из-за которой эта
/// проверка однажды кричала `Fail` на каждом здоровом старте.
fn check_bridge_listening(state: &AppState, bridge_listening_now: bool) -> Check {
    if bridge_listening_now {
        ok(
            TITLE_BRIDGE_LISTENING,
            format!("мост принимает соединения на 127.0.0.1:{}", state.port),
        )
    } else {
        fail(
            TITLE_BRIDGE_LISTENING,
            format!(
                "не удалось подключиться к 127.0.0.1:{}: мост не отвечает. \
                 Перезапустите ProxyPilot — до перезапуска ни одно приложение, \
                 использующее этот прокси, не выйдет в сеть.",
                state.port
            ),
        )
    }
}

/// Человек мог поправить настройки руками, или их сбросила групповая
/// политика — и режим в трее продолжал бы выглядеть исправным.
fn check_sysproxy_points_at_us(
    cfg: &Config,
    state: &AppState,
    sysproxy: &Result<SysProxy, String>,
) -> Check {
    if !cfg.manage_system_proxy {
        // Выключатель — осознанный выбор пользователя (GPO или ручной прокси
        // через -x); указывать на нас реестр в этом случае и не обязан.
        return ok(
            TITLE_SYSPROXY_POINTS_AT_US,
            "управление системным прокси выключено (manage_system_proxy = false) — \
             проверка не применяется, настройки — дело пользователя",
        );
    }
    match sysproxy {
        Err(e) => fail(
            TITLE_SYSPROXY_POINTS_AT_US,
            format!("не удалось прочитать системные настройки прокси: {e}"),
        ),
        Ok(current) => {
            if is_stale_pointer(current, state.port) {
                ok(
                    TITLE_SYSPROXY_POINTS_AT_US,
                    format!("указывает на 127.0.0.1:{}, как и ожидалось", state.port),
                )
            } else {
                warn_check(
                    TITLE_SYSPROXY_POINTS_AT_US,
                    format!(
                        "сейчас в системе: включён={}, адрес=«{}»; ожидали 127.0.0.1:{}. \
                         Похоже, настройки поправили вручную или их сбросила групповая \
                         политика — переключите режим в трее, чтобы ProxyPilot \
                         восстановил свой адрес.",
                        current.enabled, current.server, state.port
                    ),
                )
            }
        }
    }
}

/// Прошлый процесс убили: в реестре остался наш адрес, а слушателя больше
/// нет. Сеть у человека при этом не работает вовсе, и он не знает почему —
/// самая ценная строка, которую диагностика может напечатать. Детектор уже
/// написан ([`is_stale_pointer`]) — здесь только собираем факты вокруг него.
///
/// `port_was_free_before_bind`, а не `bridge_listening_now` из соседней
/// проверки: эта строка судит о прошлом моменте (был ли порт свободен ДО
/// нашего собственного `bind`), а не о том, слушает ли что-то СЕЙЧАС — на
/// старте второе всегда true (наш же мост уже поднят) и не различило бы
/// здоровый запуск от восстановления после аварии.
///
/// Отказ чтения реестра сюда репортится как `Warn` «не выполнена», а не как
/// второй `Fail`: причина ровно та же, что уже отражена проверкой
/// «Системный прокси указывает на нас» выше, и превращать одну первопричину
/// в две строки с одинаковым текстом — вводить в заблуждение, а не помогать.
fn check_stale_pointer_without_bridge(
    state: &AppState,
    port_was_free_before_bind: bool,
    sysproxy: &Result<SysProxy, String>,
) -> Check {
    match sysproxy {
        Err(_) => warn_check(
            TITLE_STALE_POINTER,
            "не выполнена: не удалось прочитать системные настройки прокси — причина та \
             же, что и у проверки «Системный прокси указывает на нас» выше.",
        ),
        Ok(current) => {
            if is_stale_pointer(current, state.port) && port_was_free_before_bind {
                fail(
                    TITLE_STALE_POINTER,
                    format!(
                        "в реестре указан наш адрес (127.0.0.1:{}), но мост не отвечает — \
                         похоже, предыдущий процесс ProxyPilot был завершён аварийно и не \
                         успел вернуть настройки. Сеть, скорее всего, не работает у всех \
                         приложений, читающих системные настройки прокси. Запустите \
                         ProxyPilot заново; если он уже показывает иконку в трее — \
                         перезапустите его.",
                        state.port
                    ),
                )
            } else {
                ok(
                    TITLE_STALE_POINTER,
                    "мёртвого указателя на нас в реестре нет",
                )
            }
        }
    }
}

/// «Медленно» и «не грузится» чаще всего про это.
fn check_upstreams(cfg: &Config, state: &AppState) -> Check {
    if cfg.socks_upstream.is_none() && cfg.http_upstream.is_none() {
        return ok(
            TITLE_UPSTREAMS,
            "апстримы не настроены — проверка не применяется",
        );
    }

    let mut dead = Vec::new();
    let mut unchecked = Vec::new();
    for (label, addr, health) in [
        ("SOCKS", &cfg.socks_upstream, state.health.socks),
        ("HTTP", &cfg.http_upstream, state.health.http),
    ] {
        let Some(addr) = addr else { continue };
        match health {
            Reachability::Up => {}
            Reachability::Down => dead.push(format!("{label} {addr} не отвечает")),
            Reachability::Unknown => unchecked.push(format!("{label} {addr} ещё не проверен")),
        }
    }

    if !dead.is_empty() {
        return fail(
            TITLE_UPSTREAMS,
            format!(
                "{}. Проверьте сеть до апстрима и не блокирует ли его файрвол — \
                 жалобы «медленно» и «не грузится» почти всегда об этом.",
                dead.join("; ")
            ),
        );
    }
    if !unchecked.is_empty() {
        return warn_check(
            TITLE_UPSTREAMS,
            format!("{} — подождите ближайшего пересчёта.", unchecked.join("; ")),
        );
    }
    ok(TITLE_UPSTREAMS, "все настроенные апстримы отвечают")
}

/// Если сеть не в списке офисных, `auto` уходит напрямую — это должно быть
/// видно, а не выясняться методом «а почему не работает прокси».
///
/// В закреплённом режиме место не влияет на маршрут (спека: «закреплённый
/// режим — воля пользователя»), поэтому там проверка не тревожит зря.
///
/// Источник факта о сети — `AppState.place`, а не отдельный список от
/// `list_connected`: супервизор уже опросил NLM при пересчёте маршрута и
/// положил результат (id и имя сети) в `Place`, а второй запрос к NLM здесь
/// был бы синхронным сетевым/COM-вызовом ровно ради данных, которые уже
/// есть — на старте это лишняя задержка перед тем, как цикл сообщений
/// начнёт качать события. Плата за это: если у супервизора список сетей не
/// прочитался, `Place` этого не помнит отдельно от «сетей не подключено»
/// (см. `supervisor.rs`: обе ветки схлопнуты в пустой список нарочно) — эта
/// проверка наследует то же огрубление и не пытается отличить одно от
/// другого.
fn check_network_recognised(cfg: &Config, state: &AppState) -> Check {
    if cfg.mode != Mode::Auto {
        return ok(
            TITLE_NETWORK_RECOGNISED,
            format!(
                "режим закреплён вручную ({:?}) — место сети на маршрут не влияет",
                cfg.mode
            ),
        );
    }
    match (&state.place.network, state.place.in_office) {
        (None, _) => warn_check(
            TITLE_NETWORK_RECOGNISED,
            "сейчас не опознано ни одной подключённой сети (либо сеть не подключена, \
             либо список получить не удалось — подробности в логе выше); режим auto в \
             этом случае считает, что мы не в офисе, и уходит напрямую.",
        ),
        (Some(_), true) => ok(
            TITLE_NETWORK_RECOGNISED,
            format!(
                "опознана как офисная: {}",
                state.place.network_name.as_deref().unwrap_or("?")
            ),
        ),
        (Some(_), false) => warn_check(
            TITLE_NETWORK_RECOGNISED,
            format!(
                "текущая сеть «{}» не входит в список офисных — режим auto уходит \
                 напрямую, минуя прокси. Если это офисная сеть, добавьте её в \
                 настройках.",
                state.place.network_name.as_deref().unwrap_or("?")
            ),
        ),
    }
}

/// `auto` тогда всегда «не офис» — частая причина «почему прокси не
/// включается». Тревожить этим пользователя в закреплённом режиме незачем:
/// список офисных сетей там ни на что не влияет (см. `check_network_recognised`
/// и `mode.rs::pinned_mode_ignores_place`), и вечный `Warn` был бы просто
/// шумом, который никогда не станет актуальным без смены режима.
fn check_office_networks_configured(cfg: &Config) -> Check {
    if cfg.mode != Mode::Auto {
        return ok(
            TITLE_OFFICE_NETWORKS,
            format!(
                "режим закреплён вручную ({:?}) — список офисных сетей сейчас не используется",
                cfg.mode
            ),
        );
    }
    if cfg.office_networks.is_empty() {
        warn_check(
            TITLE_OFFICE_NETWORKS,
            "офисных сетей не настроено ни одной — режим auto всегда будет считать, \
             что мы вне офиса, и трафик пойдёт напрямую, минуя прокси. Добавьте хотя \
             бы одну сеть в настройках.",
        )
    } else {
        ok(
            TITLE_OFFICE_NETWORKS,
            format!("настроено офисных сетей: {}", cfg.office_networks.len()),
        )
    }
}

/// Не проверка, а честное предупреждение о границах — печатается всегда,
/// а не только когда что-то выглядит сломанным. Диагностика, умалчивающая о
/// своих границах, отправляет человека искать ошибку не там: WinHTTP
/// (нужен `netsh winhttp` от администратора), Firefox (свои настройки, мимо
/// WinINET) и приложения, читающие `HTTP_PROXY`/`HTTPS_PROXY` из окружения —
/// не наше, и никогда не станет нашим без UAC.
fn coverage_gap_warning() -> Check {
    warn_check(
        TITLE_COVERAGE_GAP,
        "ProxyPilot управляет только системными настройками WinINET (Панель \
         управления → Свойства обозревателя). Он НЕ управляет: WinHTTP \
         (используют службы и часть приложений — правится только `netsh winhttp` \
         от администратора), Firefox (свои настройки прокси, WinINET не читает) и \
         приложениями, которые сами берут адрес из переменных окружения \
         HTTP_PROXY/HTTPS_PROXY. Если что-то из перечисленного ведёт себя не так, \
         как ожидалось, — дело не в ProxyPilot, проверяйте настройки именно этой \
         программы.",
    )
}

/// Пишет уже готовый список проверок в лог. Не сама их собирает: вызывающий
/// решает, откуда брать факты — `main.rs` строит их вручную из того, что уже
/// знает после старта, а будущая кнопка «Диагностика» соберёт их живым
/// чтением (см. модульный комментарий) и передаст сюда тем же способом.
/// Уровень записи зависит от статуса: заводить шум на `info` ради
/// обычно-исправных строк незачем, а вот настоящую проблему нужно увидеть
/// сразу.
pub fn log_diagnostics(checks: &[Check]) {
    let warnings = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Warn)
        .count();
    let failures = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    info!(total = checks.len(), warnings, failures, "самодиагностика");
    for check in checks {
        match check.status {
            CheckStatus::Ok => {
                debug!(check = %check.title, detail = %check.detail, "диагностика: ок")
            }
            CheckStatus::Warn => {
                warn!(check = %check.title, detail = %check.detail, "диагностика: предупреждение")
            }
            CheckStatus::Fail => {
                error!(check = %check.title, detail = %check.detail, "диагностика: отказ")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxypilot_core::config::OfficeNetwork;
    use proxypilot_core::mode::{Health, Place, Route};

    fn base_config() -> Config {
        Config::default()
    }

    fn base_state(port: u16) -> AppState {
        AppState {
            mode: Mode::Auto,
            route: Route::Direct,
            demoted: false,
            place: Place {
                in_office: false,
                network: None,
                network_name: None,
            },
            health: Health {
                socks: Reachability::Unknown,
                http: Reachability::Unknown,
            },
            port,
        }
    }

    fn sys(enabled: bool, server: &str) -> SysProxy {
        SysProxy {
            enabled,
            server: server.into(),
            bypass: String::new(),
        }
    }

    fn find<'a>(checks: &'a [Check], title_part: &str) -> &'a Check {
        checks
            .iter()
            .find(|c| c.title.contains(title_part))
            .unwrap_or_else(|| panic!("нет проверки с заголовком «{title_part}»: {checks:?}"))
    }

    #[test]
    fn seven_rows_come_back_every_time() {
        // Таблица брифа перечисляет ровно семь строк, включая честное
        // предупреждение о границах — оно тоже строка, а не довесок.
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(false, "")),
        );
        assert_eq!(checks.len(), 7, "получили: {checks:?}");
    }

    #[test]
    fn an_ordinary_relaunch_trips_neither_bridge_check() {
        // Это ровно та комбинация фактов, которую производит КАЖДЫЙ обычный
        // (не аварийный) старт: `bind` только что удался (мост слушает
        // СЕЙЧАС, и порт был свободен ДО него), а прочитанный ДО `take_over`
        // реестр показывает настоящие настройки пользователя, а не наш
        // собственный адрес, — если бы он показывал наш адрес на этом самом
        // шаге, это значило бы, что предыдущий процесс не прибрался за собой
        // (см. `a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line`
        // ниже, где нарочно взят другой `sysproxy`, а не эта комбинация).
        //
        // Ради этого теста и заведён Finding 5: раньше `bridge_listening_now`
        // и `port_was_free_before_bind` были одним общим булем, и на каждом
        // таком обычном старте «мост слушает свой порт» кричала `Fail`,
        // хотя мост только что поднялся, — самая частая жалоба таблицы
        // брифа превращалась в гарантированный ложный тревожный сигнал.
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(false, "")),
        );
        assert_eq!(find(&checks, "слушает").status, CheckStatus::Ok);
        assert_eq!(find(&checks, "но моста нет").status, CheckStatus::Ok);
    }

    #[test]
    fn bridge_listening_is_ok_when_the_port_answers() {
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(false, "")),
        );
        assert_eq!(find(&checks, "слушает").status, CheckStatus::Ok);
    }

    #[test]
    fn no_listener_on_the_port_is_the_loudest_failure() {
        // Самая частая жалоба: «не работает», а слушателя нет. Это про
        // `bridge_listening_now`, а не про `port_was_free_before_bind` —
        // последний здесь `true` нарочно, чтобы показать, что от него
        // результат этой проверки не зависит вовсе.
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            false,
            true,
            &Ok(sys(false, "")),
        );
        let c = find(&checks, "слушает");
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.detail.contains("3129"));
    }

    #[test]
    fn sysproxy_pointing_at_us_is_ok() {
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(true, "127.0.0.1:3129")),
        );
        assert_eq!(find(&checks, "Системный прокси").status, CheckStatus::Ok);
    }

    #[test]
    fn sysproxy_pointing_elsewhere_is_a_warning_when_we_manage_it() {
        // Человек мог поправить настройки руками, или их сбросила политика.
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(true, "10.0.0.2:3128")),
        );
        assert_eq!(find(&checks, "Системный прокси").status, CheckStatus::Warn);
    }

    #[test]
    fn sysproxy_check_is_skipped_gracefully_when_management_is_off() {
        let cfg = Config {
            manage_system_proxy: false,
            ..base_config()
        };
        let checks = run_checks(
            &cfg,
            &base_state(3129),
            true,
            true,
            &Ok(sys(false, "10.0.0.2:3128")),
        );
        assert_eq!(find(&checks, "Системный прокси").status, CheckStatus::Ok);
    }

    #[test]
    fn a_sysproxy_read_failure_fails_that_check() {
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Err("отказ реестра".into()),
        );
        assert_eq!(find(&checks, "Системный прокси").status, CheckStatus::Fail);
    }

    #[test]
    fn a_sysproxy_read_failure_is_reported_once_not_as_two_failures() {
        // Одна первопричина — одна проблема. Проверка №3 не имеет своих
        // данных для суждения (та же ошибка чтения реестра), поэтому
        // репортит «не выполнена» (Warn), а не дублирует Fail проверки №2
        // тем же текстом.
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Err("отказ реестра".into()),
        );
        assert_eq!(find(&checks, "Системный прокси").status, CheckStatus::Fail);
        let c = find(&checks, "но моста нет");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.detail.contains("не выполнена"));
    }

    #[test]
    fn a_stale_pointer_without_a_running_bridge_is_the_most_valuable_line() {
        // Прошлый процесс убили: сеть у человека не работала, пока не
        // запустился этот, новый процесс. `bridge_listening_now = true`,
        // потому что к моменту, когда это в принципе можно проверить, НАШ
        // собственный `bind` уже отработал (см. модульный комментарий) —
        // единственный факт, который отличает этот старт от обычного,
        // это то, что реестр, прочитанный ДО `take_over`, уже показывал
        // наш адрес: значит, СТАРЫЙ процесс не прибрался за собой.
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(true, "127.0.0.1:3129")),
        );
        assert_eq!(
            find(&checks, "слушает").status,
            CheckStatus::Ok,
            "мост, поднятый НАМИ только что, обязан отчитаться как рабочий"
        );
        let c = find(&checks, "но моста нет");
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.detail.contains("3129"));
    }

    #[test]
    fn a_stale_looking_pointer_is_fine_when_the_port_was_not_actually_free() {
        // `port_was_free_before_bind = false` — единственный способ отличить
        // этот случай от предыдущего теста в чистой функции: снаружи это
        // соответствует ситуации, когда порт непосредственно до `bind` был
        // ещё занят (проверка не должна была бы сюда попасть в реальном
        // вызове, но обязана вести себя предсказуемо на любых входах).
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            false,
            &Ok(sys(true, "127.0.0.1:3129")),
        );
        assert_eq!(find(&checks, "но моста нет").status, CheckStatus::Ok);
    }

    #[test]
    fn no_stale_pointer_when_the_registry_points_elsewhere() {
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            false,
            true,
            &Ok(sys(true, "10.0.0.2:3128")),
        );
        assert_eq!(find(&checks, "но моста нет").status, CheckStatus::Ok);
    }

    #[test]
    fn upstreams_check_is_ok_when_nothing_is_configured() {
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(false, "")),
        );
        assert_eq!(find(&checks, "Апстрим").status, CheckStatus::Ok);
    }

    #[test]
    fn a_dead_configured_upstream_fails_the_check() {
        let cfg = Config {
            socks_upstream: Some("10.0.0.2:9999".into()),
            ..base_config()
        };
        let mut st = base_state(3129);
        st.health = Health {
            socks: Reachability::Down,
            http: Reachability::Unknown,
        };
        let checks = run_checks(&cfg, &st, true, true, &Ok(sys(false, "")));
        let c = find(&checks, "Апстрим");
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.detail.contains("10.0.0.2:9999"));
    }

    #[test]
    fn an_unprobed_upstream_is_only_a_warning() {
        // Unknown значит «ещё не пробовали» — это не то же самое, что мёртв.
        let cfg = Config {
            http_upstream: Some("10.0.0.2:3128".into()),
            ..base_config()
        };
        let checks = run_checks(&cfg, &base_state(3129), true, true, &Ok(sys(false, "")));
        assert_eq!(find(&checks, "Апстрим").status, CheckStatus::Warn);
    }

    #[test]
    fn a_live_configured_upstream_is_ok() {
        let cfg = Config {
            socks_upstream: Some("10.0.0.2:9999".into()),
            ..base_config()
        };
        let mut st = base_state(3129);
        st.health = Health {
            socks: Reachability::Up,
            http: Reachability::Unknown,
        };
        let checks = run_checks(&cfg, &st, true, true, &Ok(sys(false, "")));
        assert_eq!(find(&checks, "Апстрим").status, CheckStatus::Ok);
    }

    #[test]
    fn network_recognition_does_not_apply_to_a_pinned_mode() {
        // Закреплённый режим — воля пользователя, место значения не имеет
        // (mode.rs: pinned_mode_ignores_place).
        let cfg = Config {
            mode: Mode::Socks,
            ..base_config()
        };
        let mut st = base_state(3129);
        st.mode = Mode::Socks;
        let checks = run_checks(&cfg, &st, true, true, &Ok(sys(false, "")));
        assert_eq!(find(&checks, "сеть опознана").status, CheckStatus::Ok);
    }

    #[test]
    fn an_unrecognised_network_in_auto_mode_is_a_warning() {
        let mut st = base_state(3129);
        st.place = Place {
            in_office: false,
            network: Some("{HOME}".into()),
            network_name: Some("Домашний Wi-Fi".into()),
        };
        let checks = run_checks(&base_config(), &st, true, true, &Ok(sys(false, "")));
        let c = find(&checks, "сеть опознана");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.detail.contains("Домашний Wi-Fi"));
    }

    #[test]
    fn an_office_network_in_auto_mode_is_ok() {
        let mut st = base_state(3129);
        st.place = Place {
            in_office: true,
            network: Some("{OFFICE}".into()),
            network_name: Some("OFFICE-WIFI".into()),
        };
        let checks = run_checks(&base_config(), &st, true, true, &Ok(sys(false, "")));
        assert_eq!(find(&checks, "сеть опознана").status, CheckStatus::Ok);
    }

    #[test]
    fn no_recognised_network_at_all_is_a_warning_in_auto_mode() {
        // Место брифа с NLM-факом (список сетей не получить) и с "сетей
        // нет вовсе" — одно и то же в `AppState.place` (супервизор их уже
        // схлопнул, см. `supervisor.rs`), поэтому у этой ветки один тест,
        // а не два: `place.network == None` покрывает обе причины разом.
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(false, "")),
        );
        assert_eq!(find(&checks, "сеть опознана").status, CheckStatus::Warn);
    }

    #[test]
    fn no_office_networks_configured_at_all_is_a_warning_in_auto_mode() {
        let checks = run_checks(
            &base_config(),
            &base_state(3129),
            true,
            true,
            &Ok(sys(false, "")),
        );
        assert_eq!(find(&checks, "офисные сети").status, CheckStatus::Warn);
    }

    #[test]
    fn at_least_one_office_network_makes_that_check_pass() {
        let cfg = Config {
            office_networks: vec![OfficeNetwork {
                id: "{OFFICE}".into(),
                name: "Офис".into(),
            }],
            ..base_config()
        };
        let checks = run_checks(&cfg, &base_state(3129), true, true, &Ok(sys(false, "")));
        assert_eq!(find(&checks, "офисные сети").status, CheckStatus::Ok);
    }

    #[test]
    fn the_office_networks_check_does_not_apply_to_a_pinned_mode() {
        // Список офисных сетей ни на что не влияет вне auto — вечный Warn
        // здесь был бы просто шумом (то же рассуждение, что и у
        // check_network_recognised).
        let cfg = Config {
            mode: Mode::Socks,
            office_networks: vec![],
            ..base_config()
        };
        let mut st = base_state(3129);
        st.mode = Mode::Socks;
        let checks = run_checks(&cfg, &st, true, true, &Ok(sys(false, "")));
        assert_eq!(find(&checks, "офисные сети").status, CheckStatus::Ok);
    }

    #[test]
    fn the_coverage_gap_warning_is_always_present_even_when_everything_else_is_fine() {
        // Самое важное свойство этой строки: она печатается ВСЕГДА, а не
        // только когда что-то выглядит сломанным. Молчание о границах
        // управления отправило бы человека искать ошибку не там.
        let cfg = Config {
            office_networks: vec![OfficeNetwork {
                id: "{OFFICE}".into(),
                name: "Офис".into(),
            }],
            ..base_config()
        };
        let mut st = base_state(3129);
        st.place = Place {
            in_office: true,
            network: Some("{OFFICE}".into()),
            network_name: Some("Офис".into()),
        };
        let checks = run_checks(&cfg, &st, true, false, &Ok(sys(true, "127.0.0.1:3129")));
        let c = checks
            .iter()
            .find(|c| c.detail.contains("WinHTTP"))
            .expect("предупреждение о границах обязано присутствовать всегда");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.detail.contains("Firefox"));
        assert!(c.detail.contains("HTTP_PROXY"));
    }
}
