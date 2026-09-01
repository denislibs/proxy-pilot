//! Источник данных для проверки обновлений: GitHub Releases API плюс
//! скачивание файла ассета.
//!
//! За трейтом [`UpdateSource`] — ровно затем, чтобы [`super::check`]
//! проверялся без единого обращения к сети (прямое ограничение задачи).
//! Реальная реализация ([`GithubSource`]) не участвует ни в одном тесте
//! этого файла: сеть в тестах запрещена, а «проверить компиляцию» — не то
//! же самое, что «проверить поведение». Честно об этом — в докблоке
//! `GithubSource` и в отчёте задачи.
//!
//! Источник — только `denislibs/proxy-pilot`
//! (`docs/process/win-delivery/progress.md`, «БЛОКЕР СНЯТ»): репозиторий
//! партнёра, опубликовавшего эту работу. Имя репозитория не берётся из
//! конфига и не выведено в UI как поле — единственный тумблер обновлений
//! это «проверять или нет», а не «где проверять» (`docs/process/win-delivery/task-3-brief.md`).

use std::path::Path;

use super::json::{self, Json};

pub const OWNER: &str = "denislibs";
pub const REPO: &str = "proxy-pilot";
/// Имя ассета, который ищем среди файлов релиза — то самое имя, под которым
/// задача 4 прикладывает бинарь ядра к релизу (`gh release create <тег>
/// proxypilot.exe proxypilot-bridge.exe`, `.github/workflows/release.yml`).
/// Мост (`proxypilot-bridge.exe`) не обновляется этим механизмом отдельно —
/// он собирается и подписывается тем же конвейером, что и `proxypilot.exe`,
/// и меняется вместе с ним; различать их версии как два независимых
/// обновления было бы дифференциальным обновлением, а не полной заменой,
/// которую задача прямо запрещает усложнять.
pub const ASSET_NAME: &str = "proxypilot.exe";

/// Что нашлось в последнем опубликованном релизе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// `tag_name` как есть, с ведущим `v` — то, что понимает
    /// [`super::version::parse_tag`].
    pub tag: String,
    /// Прямая ссылка на файл ассета (`browser_download_url`) — GitHub сам
    /// отдаёт её уже указывающей на итоговый CDN, редиректы (если будут)
    /// проходит сам HTTP-клиент.
    pub asset_url: String,
}

/// Сетевая сторона проверки обновлений, за трейтом ради тестируемости
/// [`super::check`] без сети.
pub trait UpdateSource: Send + Sync {
    /// Последний опубликованный релиз. Отсутствие релизов вовсе (GitHub
    /// отвечает `404` на `/releases/latest` для пустого репозитория —
    /// проверено вживую, read-only, на `denislibs/proxy-pilot`) — тоже
    /// `Err`, не панике и не пустому результату: вызывающий обязан отличать
    /// «сеть недоступна»/«релизов ещё нет» от «есть релиз, но он не новее».
    fn latest_release(&self) -> Result<ReleaseInfo, String>;
    /// Скачивает файл по ссылке из [`ReleaseInfo::asset_url`] в `dest`.
    /// Пишет во ВРЕМЕННЫЙ путь и не подменяет уже существующий файл — эта
    /// обязанность лежит на вызывающем ([`super::check`]), а не здесь: у
    /// функции, качающей файл, нет и не должно быть мнения о том, что с
    /// этим файлом делать дальше.
    fn download(&self, url: &str, dest: &Path) -> Result<(), String>;
}

/// Разбирает ответ `GET /repos/{owner}/{repo}/releases/latest` и находит в
/// нём ассет с именем `asset_name`.
///
/// Чистая функция — весь смысл вынести её из [`GithubSource::latest_release`]
/// в том, чтобы разбор реального (по форме) ответа API проверялся без сети:
/// фикстуры ниже списаны с формы, подтверждённой read-only обращением к
/// живому API (см. отчёт задачи).
pub fn parse_release_response(body: &str, asset_name: &str) -> Result<ReleaseInfo, String> {
    let root = json::parse(body).map_err(|e| format!("ответ GitHub не разобрался: {e}"))?;
    let tag = root
        .get("tag_name")
        .and_then(Json::as_str)
        .ok_or("в ответе нет поля tag_name")?
        .to_string();
    let assets = root
        .get("assets")
        .and_then(Json::as_array)
        .ok_or("в ответе нет списка assets")?;
    let asset = assets
        .iter()
        .find(|a| a.get("name").and_then(Json::as_str) == Some(asset_name))
        .ok_or_else(|| format!("в релизе {tag} нет файла {asset_name}"))?;
    let asset_url = asset
        .get("browser_download_url")
        .and_then(Json::as_str)
        .ok_or("у найденного ассета нет browser_download_url")?
        .to_string();
    Ok(ReleaseInfo { tag, asset_url })
}

/// Настоящий источник — GitHub Releases API плюс скачивание файла, оба через
/// `WinHTTP` (`windows::Win32::Networking::WinHttp`).
///
/// **Почему `WinHTTP`, а не `reqwest`/`hyper`+`rustls`.** TLS для HTTPS
/// нужен обязательно (GitHub API и CDN ассетов — только `https://`), а
/// `WinHTTP` — системный HTTP-клиент Windows: TLS, доверенные корневые
/// сертификаты и поведение по умолчанию (следование редиректам) достаются
/// от системы бесплатно, вместо нового звена в дереве зависимостей, которое
/// рано или поздно придётся подписывать вместе с самим `proxypilot.exe`
/// (тот же довод, каким `bench.rs` объясняет отсутствие HTTP-библиотеки для
/// одного статического `GET`).
///
/// **Почему без системного прокси.** `WINHTTP_ACCESS_TYPE_NO_PROXY`, а не
/// автоматическое определение: при включённом `manage_system_proxy`
/// системный прокси указывает на НАС ЖЕ (`proxy::take_over`), и заводить
/// проверку обновлений через собственный мост — лишняя зависимость проверки
/// от состояния моста, которого этот модуль обязан не иметь вовсе
/// («проверка не блокирует и не зависит от работы моста»).
///
/// **Не проверено ни одним тестом.** Прямое ограничение задачи — сеть в
/// тестах запрещена; проверить эмпирически можно было бы только настоящим
/// запросом к `api.github.com`, чего этот файл не делает. То же самое
/// честное ограничение, каким задачи 2 и 4 отчитывались о `signtool
/// sign`/`gh release create`: написано по документации `WinHTTP`, не
/// подтверждено запуском.
pub struct GithubSource {
    pub owner: String,
    pub repo: String,
}

impl Default for GithubSource {
    fn default() -> Self {
        Self {
            owner: OWNER.to_string(),
            repo: REPO.to_string(),
        }
    }
}

impl UpdateSource for GithubSource {
    fn latest_release(&self) -> Result<ReleaseInfo, String> {
        let path = format!("/repos/{}/{}/releases/latest", self.owner, self.repo);
        let body = winhttp::get_https("api.github.com", &path)?;
        parse_release_response(&body, ASSET_NAME)
    }

    fn download(&self, url: &str, dest: &Path) -> Result<(), String> {
        let (host, path) = winhttp::split_https_url(url)?;
        winhttp::download_https(&host, &path, dest)
    }
}

/// Обёртка над `WinHTTP` — синхронные Win32-вызовы, поэтому вызывающий
/// (`super::check`) обязан звать их из `tokio::task::spawn_blocking`, а не
/// из асинхронного контекста напрямую: синхронный сетевой вызов на
/// executor-потоке застопорил бы весь рантайм, включая мост.
mod winhttp {
    use std::path::Path;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Networking::WinHttp::{
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
        WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT,
        WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_STATUS_CODE,
    };

    /// `HINTERNET` из `WinHTTP` — непрозрачный дескриптор, у сгенерированных
    /// биндингов это обычный `*mut c_void` без собственного имени типа в
    /// этой версии крейта; локальный алиас — чтобы не писать полный путь в
    /// каждой сигнатуре ниже.
    type HInternet = *mut core::ffi::c_void;

    /// Таймаут КАЖДОЙ фазы (resolve/connect/send/receive) в миллисекундах.
    /// Не общий бюджет на всю операцию: `WinHttpSetTimeouts` устроен именно
    /// так, а несколько независимых, но одинаково коротких таймаутов
    /// достаточно, чтобы «сеть недоступна» не превратилось в зависший поток
    /// исполнителя.
    const PHASE_TIMEOUT_MS: i32 = 15_000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error(context: &str) -> String {
        // SAFETY: `GetLastError` не принимает аргументов и не может отказать.
        let code = unsafe { GetLastError() };
        format!("{context}: WinHTTP код ошибки {}", code.0)
    }

    /// `GET https://{host}{path}`, тело ответа как `String` (ответ GitHub
    /// API — всегда JSON в UTF-8). Успешным считается только `200`: любой
    /// другой код (`404` — релизов ещё нет, `403` — упёрлись в лимит
    /// анонимных запросов) — `Err` с самим кодом в тексте, чтобы разница
    /// была видна в логе, а не терялась за общим «не получилось».
    pub fn get_https(host: &str, path: &str) -> Result<String, String> {
        let (status, body) = request(host, path, RequestKind::Get)?;
        if status != 200 {
            return Err(format!(
                "GitHub API ответил {status} на GET https://{host}{path}"
            ));
        }
        String::from_utf8(body).map_err(|_| "ответ GitHub не UTF-8".to_string())
    }

    /// Скачивает `https://{host}{path}` в файл `dest`. Пишет напрямую по
    /// пути, который выбрал вызывающий ([`super::super::check`] отвечает за
    /// то, что это временный, а не боевой путь).
    pub fn download_https(host: &str, path: &str, dest: &Path) -> Result<(), String> {
        let (status, body) = request(host, path, RequestKind::Get)?;
        if status != 200 {
            return Err(format!(
                "скачивание ответило {status} на GET https://{host}{path}"
            ));
        }
        std::fs::write(dest, &body).map_err(|e| format!("не записать {}: {e}", dest.display()))
    }

    enum RequestKind {
        Get,
    }

    /// Один запрос от открытия сессии до закрытия всех дескрипторов.
    /// Дескрипторы Win32 не переживают эту функцию сознательно: проверка
    /// обновлений происходит редко (раз в сутки, см. `main.rs`), и держать
    /// открытую HTTP-сессию между вызовами ради миллисекунд — сложность без
    /// потребителя.
    fn request(host: &str, path: &str, _kind: RequestKind) -> Result<(u16, Vec<u8>), String> {
        let user_agent = wide("proxypilot-updater");
        // SAFETY: все указатели — либо `PCWSTR::null()`, либо ведут в
        // локальные `Vec<u16>`, которые не выходят из области видимости
        // раньше самого вызова (все они живут до конца функции). Флаги
        // синхронного режима (по умолчанию, без `WINHTTP_FLAG_ASYNC`) —
        // вызовы ниже блокируют текущий поток, что и требуется от функции,
        // рассчитанной на вызов из `spawn_blocking`.
        let session: HInternet = unsafe {
            WinHttpOpen(
                PCWSTR(user_agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        };
        if session.is_null() {
            return Err(last_error("WinHttpOpen"));
        }
        // SAFETY: `session` — валидный дескриптор, только что полученный
        // выше; закрывается сторожем ниже независимо от исхода — RAII
        // руками, потому что дескриптор Win32, а не `Box`.
        let _session_guard = CloseOnDrop(session);

        // Таймауты — до первого сетевого вызова: без этого `WinHTTP`
        // использует собственные умолчания, а «сеть недоступна» обязана
        // проявиться быстро, а не только когда TCP сам решит сдаться.
        // SAFETY: `session` валиден (проверено выше).
        unsafe {
            let _ = WinHttpSetTimeouts(
                session,
                PHASE_TIMEOUT_MS,
                PHASE_TIMEOUT_MS,
                PHASE_TIMEOUT_MS,
                PHASE_TIMEOUT_MS,
            );
        }

        let host_wide = wide(host);
        // SAFETY: `session` валиден; `host_wide` живёт до конца функции.
        let connect: HInternet = unsafe {
            WinHttpConnect(
                session,
                PCWSTR(host_wide.as_ptr()),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            )
        };
        if connect.is_null() {
            return Err(last_error("WinHttpConnect"));
        }
        let _connect_guard = CloseOnDrop(connect);

        let verb = wide("GET");
        let path_wide = wide(path);
        // SAFETY: `connect` валиден; `verb`/`path_wide` живут до конца
        // функции; остальные параметры — документированные «нет значения»
        // константы `WinHTTP` (версия HTTP по умолчанию, без Referer, без
        // дополнительных типов Accept).
        let request: HInternet = unsafe {
            WinHttpOpenRequest(
                connect,
                PCWSTR(verb.as_ptr()),
                PCWSTR(path_wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        };
        if request.is_null() {
            return Err(last_error("WinHttpOpenRequest"));
        }
        let _request_guard = CloseOnDrop(request);

        // SAFETY: `request` валиден; без дополнительных заголовков и тела
        // запроса — обычный `GET`.
        let sent = unsafe { WinHttpSendRequest(request, None, None, 0, 0, 0) };
        if sent.is_err() {
            return Err(last_error("WinHttpSendRequest"));
        }

        // SAFETY: `request` валиден и запрос уже отправлен — ровно то, что
        // требует `WinHttpReceiveResponse`.
        let received = unsafe { WinHttpReceiveResponse(request, std::ptr::null_mut()) };
        if received.is_err() {
            return Err(last_error("WinHttpReceiveResponse"));
        }

        let status = query_status_code(request)?;
        let body = read_body(request)?;
        Ok((status, body))
    }

    fn query_status_code(request: HInternet) -> Result<u16, String> {
        let mut buf = [0u16; 16];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `request` валиден (запрошен вызывающим сразу после
        // успешного `WinHttpReceiveResponse`); `buf`/`size` описывают один и
        // тот же буфер и его точный размер в байтах, как требует API.
        let ok = unsafe {
            WinHttpQueryHeaders(
                request,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(buf.as_mut_ptr() as *mut _),
                &mut size,
                std::ptr::null_mut(),
            )
        };
        if ok.is_err() {
            return Err(last_error("WinHttpQueryHeaders(status)"));
        }
        // SAFETY: `WINHTTP_QUERY_FLAG_NUMBER` документированно кладёт в
        // буфер ровно один `u32` — тот же приём, каким `WinHTTP`-примеры
        // Microsoft читают код статуса.
        let code = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const u32) };
        Ok(code as u16)
    }

    fn read_body(request: HInternet) -> Result<Vec<u8>, String> {
        let mut body = Vec::new();
        loop {
            let mut available: u32 = 0;
            // SAFETY: `request` валиден; `available` — локальная переменная,
            // адрес которой живёт до конца этой итерации.
            let ok = unsafe { WinHttpQueryDataAvailable(request, &mut available) };
            if ok.is_err() {
                return Err(last_error("WinHttpQueryDataAvailable"));
            }
            if available == 0 {
                break;
            }
            let mut chunk = vec![0u8; available as usize];
            let mut read: u32 = 0;
            // SAFETY: `chunk` — буфер длиной ровно `available` байт, тот
            // же размер передан вызову; `read` — локальная переменная под
            // фактическое число прочитанных байт.
            let ok = unsafe {
                WinHttpReadData(request, chunk.as_mut_ptr() as *mut _, available, &mut read)
            };
            if ok.is_err() {
                return Err(last_error("WinHttpReadData"));
            }
            chunk.truncate(read as usize);
            if chunk.is_empty() {
                break;
            }
            body.extend_from_slice(&chunk);
            // Потолок на тело ответа — 64 МиБ с большим запасом:
            // `proxypilot.exe` — единственное, что здесь качается, весит
            // считаные мегабайты, а без потолка испорченный или чужой
            // ответ мог бы исчерпать память фонового процесса.
            if body.len() > 64 * 1024 * 1024 {
                return Err("ответ длиннее ожидаемого — прервано".to_string());
            }
        }
        Ok(body)
    }

    /// `https://host[:port]/path` → `(host, path)`. Минимальный разбор — то
    /// же решение, каким `bridge::bench::parse_url` разбирает `http://`:
    /// полноценный разбор URL здесь не нужен, `browser_download_url`
    /// GitHub — всегда `https://<host>/<path>` без пользовательской части
    /// и без порта.
    pub fn split_https_url(url: &str) -> Result<(String, String), String> {
        let rest = url
            .strip_prefix("https://")
            .ok_or_else(|| format!("ожидался https://: {url}"))?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if host.is_empty() {
            return Err(format!("пустой хост в ссылке: {url}"));
        }
        Ok((host.to_string(), path.to_string()))
    }

    /// Закрывает дескриптор `WinHTTP` в `Drop` — тот же приём, что у
    /// `Server` в `websrv.rs`: дверь закрывается вместе с владельцем
    /// независимо от того, каким путём функция вышла (успех, ранний
    /// `return Err`, паника).
    struct CloseOnDrop(HInternet);

    impl Drop for CloseOnDrop {
        fn drop(&mut self) {
            // SAFETY: дескриптор либо валиден (получен успешным вызовом
            // выше), либо это последний `Drop` уже закрытого — `WinHTTP`
            // документированно переживает повторное закрытие своего же
            // дескриптора без падения.
            let _ = unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, url: &str) -> String {
        format!(r#"{{"name": "{name}", "browser_download_url": "{url}"}}"#)
    }

    fn release_body(tag: &str, assets: &[String]) -> String {
        format!(
            r#"{{"tag_name": "{tag}", "prerelease": false, "assets": [{}]}}"#,
            assets.join(",")
        )
    }

    #[test]
    fn finds_our_asset_among_several_like_the_real_release_pipeline_produces() {
        // Форма фикстуры — с настоящей формой ответа API (read-only
        // проверено на живом `api.github.com`, см. отчёт задачи), с двумя
        // ассетами — тем же набором, что кладёт задача 4 (`gh release
        // create <тег> proxypilot.exe proxypilot-bridge.exe`).
        let body = release_body(
            "v0.2.0",
            &[
                asset(
                    "proxypilot-bridge.exe",
                    "https://example.internal/bridge.exe",
                ),
                asset("proxypilot.exe", "https://example.internal/app.exe"),
            ],
        );
        let info = parse_release_response(&body, "proxypilot.exe").expect("должен разобраться");
        assert_eq!(info.tag, "v0.2.0");
        assert_eq!(info.asset_url, "https://example.internal/app.exe");
    }

    #[test]
    fn a_release_without_our_asset_is_an_error() {
        let body = release_body(
            "v0.2.0",
            &[asset(
                "proxypilot-bridge.exe",
                "https://example.internal/bridge.exe",
            )],
        );
        let err = parse_release_response(&body, "proxypilot.exe").unwrap_err();
        assert!(err.contains("proxypilot.exe"), "получили: {err}");
    }

    #[test]
    fn an_empty_releases_list_is_an_error_not_a_panic() {
        // Форма реального ответа `/releases` для репозитория без релизов —
        // подтверждена read-only обращением к живому API (отчёт задачи).
        // `/releases/latest` для того же случая отвечает `404`, что этот
        // парсер вообще не видит — `get_https` обязан вернуть `Err` раньше,
        // до вызова разбора; здесь проверяется just сам парсер на форме,
        // которая тоже может прийти.
        let err = parse_release_response("[]", "proxypilot.exe").unwrap_err();
        assert!(err.contains("tag_name"), "получили: {err}");
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        let err = parse_release_response("не json вовсе", "proxypilot.exe").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn truncated_json_is_an_error() {
        let err = parse_release_response(r#"{"tag_name": "v1.0"#, "proxypilot.exe").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn splits_a_release_download_url() {
        let (host, path) = winhttp::split_https_url(
            "https://github.com/denislibs/proxy-pilot/releases/download/v0.2.0/proxypilot.exe",
        )
        .expect("должен разобраться");
        assert_eq!(host, "github.com");
        assert_eq!(
            path,
            "/denislibs/proxy-pilot/releases/download/v0.2.0/proxypilot.exe"
        );
    }

    #[test]
    fn rejects_a_non_https_url() {
        assert!(winhttp::split_https_url("http://example.internal/x").is_err());
    }
}
