use crate::{api, model::*};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use url::Url;

pub const REDIRECT: &str = "https://vk-backup.whitesquirrel.digital/callback";
pub const LOOPBACK_REDIRECT: &str = "http://127.0.0.1:47851/callback";

fn authorize_url(client: &str, legacy: bool, scopes: &str, nonce: &str, verifier: &str) -> Url {
    let mut url = Url::parse(if legacy {
        "https://oauth.vk.com/authorize"
    } else {
        "https://id.vk.com/authorize"
    })
    .unwrap();
    url.query_pairs_mut().extend_pairs([
        ("client_id", client),
        (
            "redirect_uri",
            if legacy { LOOPBACK_REDIRECT } else { REDIRECT },
        ),
        ("state", nonce),
        ("response_type", if legacy { "token" } else { "code" }),
        ("scope", scopes),
    ]);
    if legacy {
        url.query_pairs_mut().append_pair("display", "page");
    } else {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        url.query_pairs_mut().extend_pairs([
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "s256"),
        ]);
    }
    url
}

pub fn parse_token(input: &str) -> Result<String, String> {
    let value = input.trim();
    let token = if value.starts_with("https://") || value.starts_with("http://") {
        let url = Url::parse(value).map_err(|_| "Некорректная ссылка авторизации.")?;
        let params: Vec<_> =
            url::form_urlencoded::parse(url.fragment().unwrap_or("").as_bytes()).collect();
        params
            .iter()
            .find(|(k, _)| k == "access_token")
            .map(|(_, v)| v.to_string())
            .or_else(|| {
                url.query_pairs()
                    .find(|(k, _)| k == "access_token")
                    .map(|(_, v)| v.to_string())
            })
            .ok_or("Ссылка не содержит access_token.")?
    } else {
        value.to_string()
    };
    if token.len() < 20 || token.len() > 8192 || token.chars().any(char::is_whitespace) {
        return Err(
            "Введите пользовательский токен VK API или полную ссылку с access_token.".into(),
        );
    }
    Ok(token)
}

async fn establish(
    state: &AppState,
    token: String,
    refresh: Option<Refresh>,
) -> Result<User, String> {
    establish_for(state, token, refresh, None, None).await
}

pub(crate) async fn establish_for(
    state: &AppState,
    token: String,
    refresh: Option<Refresh>,
    expected_user: Option<i64>,
    canceled: Option<&AtomicBool>,
) -> Result<User, String> {
    let profile = api::call_token(
        state,
        &token,
        "users.get",
        &[("fields", "photo_200".into())],
    )
    .await?;
    let p = profile
        .as_array()
        .and_then(|p| p.first())
        .ok_or("Нужен пользовательский токен, а не токен сообщества или сервисный ключ.")?;
    let id = api::number(p, "id");
    if id <= 0 {
        return Err("ВК не вернул ваш профиль. Проверьте тип токена.".into());
    }
    if expected_user.is_some_and(|expected| expected != id) {
        return Err(
            "Токен относится к другому аккаунту ВК. Повторите подключение нужного аккаунта.".into(),
        );
    }
    let user = User {
        id,
        name: format!(
            "{} {}",
            api::text(p, "first_name"),
            api::text(p, "last_name")
        ),
        avatar: p["photo_200"].as_str().map(str::to_string),
    };
    let mut session = state.session.write().await;
    if canceled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err("Вход отменён.".into());
    }
    *session = Some(Session {
        token,
        user: user.clone(),
        refresh,
    });
    drop(session);
    state.sources.write().await.clear();
    state.collections.write().await.clear();
    Ok(user)
}

#[tauri::command]
pub async fn connect_token(state: State<'_, AppState>, token: String) -> Result<User, String> {
    if state.busy().await {
        return Err("Аккаунт нельзя менять во время бэкапа.".into());
    }
    if state.login.lock().await.is_some() {
        return Err("Сначала отмените вход через браузер.".into());
    }
    establish(&state, parse_token(&token)?, None).await
}

pub async fn access_token(state: &AppState) -> Result<String, String> {
    // Hold the write lock across refresh so concurrent API requests cannot rotate it twice.
    let mut guard = state.session.write().await;
    let session = guard.as_mut().ok_or("Сначала подключите аккаунт.")?;
    if let Some(refresh) = &session.refresh
        && refresh.expires <= Instant::now() + Duration::from_secs(60)
    {
        let nonce = uuid::Uuid::new_v4().to_string();
        let response: Value = state
            .client
            .post("https://id.vk.com/oauth2/auth")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.token.as_str()),
                ("device_id", refresh.device.as_str()),
                ("client_id", refresh.client.as_str()),
                ("redirect_uri", REDIRECT),
                ("state", nonce.as_str()),
            ])
            .send()
            .await
            .map_err(|_| "Не удалось обновить вход. Проверьте соединение.")?
            .json()
            .await
            .map_err(|_| "VK ID вернул некорректный ответ.")?;
        if response["state"].as_str() != Some(&nonce) {
            return Err("Не удалось подтвердить ответ VK ID. Войдите заново.".into());
        }
        let token = response["access_token"]
            .as_str()
            .ok_or("Вход истёк. Подключите аккаунт заново.")?
            .to_string();
        let next = Refresh {
            token: response["refresh_token"]
                .as_str()
                .unwrap_or(&refresh.token)
                .to_string(),
            device: refresh.device.clone(),
            client: refresh.client.clone(),
            expires: Instant::now()
                + Duration::from_secs(response["expires_in"].as_u64().unwrap_or(3600)),
        };
        session.token = token;
        session.refresh = Some(next);
    }
    Ok(session.token.clone())
}

#[tauri::command]
pub async fn begin_login(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    client_id: String,
    mode: String,
    scopes: String,
) -> Result<(), String> {
    if state.busy().await {
        return Err("Дождитесь завершения бэкапа.".into());
    }
    if client_id.is_empty() || !client_id.chars().all(|c| c.is_ascii_digit()) {
        return Err("Введите числовой ID вашего приложения ВК.".into());
    }
    if !matches!(mode.as_str(), "vkid" | "legacy") {
        return Err("Неизвестный способ авторизации.".into());
    }
    if scopes.len() > 256
        || !scopes
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " .,_".contains(c))
    {
        return Err("Некорректный список прав.".into());
    }
    let mut active = state.login.lock().await;
    if active.is_some() {
        return Err("Вход уже открыт. Завершите или отмените его.".into());
    }
    let listener = TcpListener::bind("127.0.0.1:47851")
        .await
        .map_err(|_| "Порт 47851 занят. Закройте другой экземпляр приложения и повторите вход.")?;
    let canceled = Arc::new(AtomicBool::new(false));
    *active = Some(canceled.clone());
    drop(active);
    let nonce = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let verifier = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let legacy = mode == "legacy";
    let url = authorize_url(&client_id, legacy, &scopes, &nonce, &verifier);
    if app.opener().open_url(url.as_str(), None::<&str>).is_err() {
        *state.login.lock().await = None;
        return Err("Не удалось открыть браузер.".into());
    }
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = receive_login(
            &state, listener, canceled, legacy, &nonce, &verifier, &client_id,
        )
        .await;
        *state.login.lock().await = None;
        let payload = match result {
            Ok(user) => json!({"user":user}),
            Err(e) => json!({"error":e}),
        };
        let _ = app.emit("auth-result", payload);
    });
    Ok(())
}

async fn receive_login(
    state: &AppState,
    listener: TcpListener,
    canceled: Arc<AtomicBool>,
    legacy: bool,
    nonce: &str,
    verifier: &str,
    client: &str,
) -> Result<User, String> {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if canceled.load(Ordering::Relaxed) {
            return Err("Вход отменён.".into());
        }
        if Instant::now() > deadline {
            return Err("Время ожидания входа истекло. Попробуйте снова.".into());
        }
        let accepted = tokio::time::timeout(Duration::from_millis(250), listener.accept()).await;
        let Ok(Ok((mut socket, _))) = accepted else {
            continue;
        };
        let request =
            match tokio::time::timeout(Duration::from_secs(5), read_http(&mut socket)).await {
                Ok(Ok(r)) => r,
                _ => {
                    let _ = respond(&mut socket, "400 Bad Request", "Некорректный запрос").await;
                    continue;
                }
            };
        let first = request.lines().next().unwrap_or("");
        let target = first.split_whitespace().nth(1).unwrap_or("/");
        let url = Url::parse(&format!("http://127.0.0.1:47851{target}"))
            .map_err(|_| "Некорректный адрес возврата.")?;
        if url.path() == "/callback" && legacy {
            respond(&mut socket, "200 OK", LEGACY_PAGE).await?;
            continue;
        }
        let params: std::collections::HashMap<String, String> =
            if legacy && url.path() == "/token" && first.starts_with("POST ") {
                let body = request.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
                url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect()
            } else if !legacy && url.path() == "/callback" && first.starts_with("GET ") {
                url.query_pairs().into_owned().collect()
            } else {
                let _ = respond(&mut socket, "404 Not Found", "Not found").await;
                continue;
            };
        if params.get("state").map(String::as_str) != Some(nonce) {
            let _ = respond(&mut socket, "403 Forbidden", "Проверка входа не пройдена.").await;
            continue;
        }
        if params.contains_key("error") {
            respond(
                &mut socket,
                "200 OK",
                "Вход отменён в ВК. Можно закрыть вкладку.",
            )
            .await?;
            return Err(
                "ВК отклонил авторизацию. Проверьте настройки приложения и список прав.".into(),
            );
        }
        let (token, refresh) = if legacy {
            (
                parse_token(params.get("access_token").ok_or("ВК не вернул токен.")?)?,
                None,
            )
        } else {
            let code = params.get("code").ok_or("VK ID не вернул код.")?;
            let device = params
                .get("device_id")
                .ok_or("VK ID не вернул device_id.")?;
            let body: Value = state
                .client
                .post("https://id.vk.com/oauth2/auth")
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("code", code.as_str()),
                    ("device_id", device.as_str()),
                    ("client_id", client),
                    ("redirect_uri", REDIRECT),
                    ("code_verifier", verifier),
                    ("state", nonce),
                ])
                .send()
                .await
                .map_err(|_| "Не удалось завершить вход через VK ID.")?
                .json()
                .await
                .map_err(|_| "VK ID вернул некорректный ответ.")?;
            if body["state"].as_str() != Some(nonce) {
                return Err("VK ID вернул неверный state.".into());
            }
            let token = body["access_token"]
                .as_str()
                .ok_or(
                    "VK ID не предоставил токен. Проверьте настройки приложения и redirect URI.",
                )?
                .to_string();
            let refresh = body["refresh_token"].as_str().map(|t| Refresh {
                token: t.into(),
                device: device.clone(),
                client: client.into(),
                expires: Instant::now()
                    + Duration::from_secs(body["expires_in"].as_u64().unwrap_or(3600)),
            });
            (token, refresh)
        };
        let result = establish(state, token, refresh).await;
        let message = if result.is_ok() {
            "<h1>Аккаунт подключён</h1><p>Вернитесь в VK Backup. Эту вкладку можно закрыть.</p>"
        } else {
            "<h1>Не удалось подключить аккаунт</h1><p>Вернитесь в VK Backup, чтобы увидеть причину.</p>"
        };
        let _ = respond(&mut socket, "200 OK", message).await;
        return result;
    }
}

async fn read_http(socket: &mut TcpStream) -> Result<String, String> {
    let mut all = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = socket
            .read(&mut buf)
            .await
            .map_err(|_| "Ошибка локального соединения.")?;
        if n == 0 {
            return Err("Соединение закрыто.".into());
        }
        all.extend_from_slice(&buf[..n]);
        if all.len() > 16384 {
            return Err("Запрос слишком большой.".into());
        }
        let text = String::from_utf8_lossy(&all);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if body.len() >= length {
                return String::from_utf8(all).map_err(|_| "Некорректный запрос.".into());
            }
        }
    }
}

async fn respond(socket: &mut TcpStream, status: &str, body: &str) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket
        .write_all(response.as_bytes())
        .await
        .map_err(|_| "Не удалось ответить браузеру.".to_string())
}

const LEGACY_PAGE: &str = r#"<!doctype html><html lang="ru"><meta charset="utf-8"><title>VK Backup</title><body><h1>Завершаем вход…</h1><script>const p=location.hash.slice(1);history.replaceState(null,'','/callback');fetch('/token',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:p}).then(r=>r.text()).then(t=>{document.body.innerHTML=t}).catch(()=>{document.body.textContent='Вернитесь в VK Backup и повторите вход.'});</script></body></html>"#;

#[tauri::command]
pub async fn cancel_login(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(flag) = state.login.lock().await.as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    if state.busy().await {
        return Err(str::to_string("Сначала завершите или отмените бэкап."));
    }
    if state.login.lock().await.is_some() {
        return Err("Сначала отмените вход через браузер.".into());
    }
    *state.session.write().await = None;
    state.sources.write().await.clear();
    state.collections.write().await.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vkid_authorization_uses_public_redirect_and_pkce() {
        let verifier = "verifier";
        let url = authorize_url("123", false, "photos video groups", "nonce", verifier);
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["redirect_uri"], REDIRECT);
        assert_eq!(params["response_type"], "code");
        assert_eq!(params["scope"], "photos video groups");
        assert_eq!(params["state"], "nonce");
        assert_eq!(
            params["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
        );
        let legacy = authorize_url("123", true, "photos,video", "nonce", verifier);
        let params: std::collections::HashMap<_, _> = legacy.query_pairs().into_owned().collect();
        assert_eq!(params["redirect_uri"], LOOPBACK_REDIRECT);
        assert!(!params.contains_key("code_challenge"));
    }
    #[test]
    fn token_can_be_pasted_from_redirect_without_retaining_url() {
        assert_eq!(parse_token("https://oauth.vk.com/blank.html#access_token=abcdefghijklmnopqrstuvwxyz&expires_in=1").unwrap(), "abcdefghijklmnopqrstuvwxyz");
        assert!(parse_token("https://example.com/#code=123").is_err());
        assert!(parse_token("short").is_err());
    }
}
