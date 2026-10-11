use crate::{api, auth, model::*};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, OsRng, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use p256::{PublicKey, SecretKey, ecdh::diffie_hellman, elliptic_curve::sec1::ToEncodedPoint};
use serde::{Deserialize, Serialize};
use serde_json::json;
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

const BASE: &str = "https://vk-backup.whitesquirrel.digital/bridge";
const CONTEXT: &[u8] = b"vk-backup-bridge-v1";
const INVALID: &str = "Не удалось проверить зашифрованный ответ. Повторите подключение.";
const ALLOWED: &[&str] = &["photos", "video", "groups", "docs", "wall", "friends"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStart {
    pair_code: String,
    expires_at: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pair {
    pair_id: String,
    expires_at: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    version: u8,
    public_key: String,
    nonce: String,
    ciphertext: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Credentials {
    access_token: String,
    app_id: u64,
    user_id: i64,
    scopes: Vec<String>,
}
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn random_id() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn requested_scopes(scopes: &str) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    for scope in scopes.split([' ', ',']).filter(|s| !s.is_empty()) {
        if !ALLOWED.contains(&scope) {
            return Err(
                "Для Mini App доступны запросы photos, video, groups, docs, wall и friends.".into(),
            );
        }
        if !result.iter().any(|s| s == scope) {
            result.push(scope.to_string());
        }
    }
    if result.is_empty() {
        return Err("Укажите хотя бы одно разрешение для Mini App.".into());
    }
    Ok(result)
}
fn decrypt(secret: &SecretKey, pair: &str, envelope: Envelope) -> Result<Credentials, String> {
    if envelope.version != 1 || envelope.ciphertext.len() > 22_000 {
        return Err(INVALID.into());
    }
    let remote_bytes = URL_SAFE_NO_PAD
        .decode(envelope.public_key)
        .map_err(|_| INVALID)?;
    if remote_bytes.len() != 65 || remote_bytes[0] != 4 {
        return Err(INVALID.into());
    }
    let remote = PublicKey::from_sec1_bytes(&remote_bytes).map_err(|_| INVALID)?;
    let shared = diffie_hellman(secret.to_nonzero_scalar(), remote.as_affine());
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(pair.as_bytes()), shared.raw_secret_bytes())
        .expand(CONTEXT, &mut key)
        .map_err(|_| INVALID)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| INVALID)?;
    key.fill(0);
    let nonce = URL_SAFE_NO_PAD
        .decode(envelope.nonce)
        .map_err(|_| INVALID)?;
    if nonce.len() != 12 {
        return Err(INVALID.into());
    }
    let ciphertext = URL_SAFE_NO_PAD
        .decode(envelope.ciphertext)
        .map_err(|_| INVALID)?;
    let mut plaintext = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: pair.as_bytes(),
            },
        )
        .map_err(|_| INVALID)?;
    let result = serde_json::from_slice(&plaintext).map_err(|_| INVALID.to_string());
    plaintext.fill(0);
    result
}
async fn cancel_signal(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
async fn clear_login(state: &AppState, flag: &Arc<AtomicBool>) {
    let mut active = state.login.lock().await;
    if active
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, flag))
    {
        *active = None;
    }
}

#[tauri::command]
pub async fn begin_miniapp_login(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    app_id: String,
    scopes: String,
) -> Result<BridgeStart, String> {
    if state.busy().await {
        return Err("Дождитесь завершения бэкапа.".into());
    }
    let client: u64 = app_id
        .parse()
        .ok()
        .filter(|id| *id > 0 && *id <= 99_999_999_999)
        .ok_or("Введите числовой ID отдельного VK Mini App.")?;
    if client == 54816594 {
        return Err(
            "54816594 — приложение VK ID. Для моста нужен ID отдельно созданного VK Mini App."
                .into(),
        );
    }
    let requested = requested_scopes(&scopes)?;
    let mut active = state.login.lock().await;
    if active.is_some() {
        return Err("Вход уже открыт. Завершите или отмените его.".into());
    }
    let flag = Arc::new(AtomicBool::new(false));
    *active = Some(flag.clone());
    drop(active);
    let secret = SecretKey::random(&mut OsRng);
    let public = URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
    let poll_secret = random_id();
    let expected = state.session.read().await.as_ref().map(|s| s.user.id);
    let request = state.client.post(format!("{BASE}/sessions"))
        .timeout(Duration::from_secs(15))
        .json(&json!({ "receiverPublicKey": public, "pollSecretHash": hash(&poll_secret), "appId": client, "scopes": requested, "expectedUserId": expected })).send();
    let response = tokio::select! { result = request => result.map_err(|_| "Сервер подключения недоступен. Проверьте интернет.".to_string()), _ = cancel_signal(&flag) => Err("Вход отменён.".into()) };
    let setup = async {
        let response = response?;
        if !response.status().is_success() {
            return Err("Сервер не смог открыть подключение. Попробуйте позже.".to_string());
        }
        let pair: Pair = response
            .json()
            .await
            .map_err(|_| "Сервер вернул неверный ответ.".to_string())?;
        if !valid_id(&pair.pair_id) {
            return Err("Сервер вернул неверный код подключения.".into());
        }
        Ok(pair)
    }
    .await;
    let pair = match setup {
        Ok(pair) => pair,
        Err(error) => {
            clear_login(&state, &flag).await;
            return Err(error);
        }
    };
    if flag.load(Ordering::Relaxed)
        || app
            .opener()
            .open_url(
                format!("https://vk.com/app{client}#pair={}", pair.pair_id),
                None::<&str>,
            )
            .is_err()
    {
        let _ = state
            .client
            .delete(format!("{BASE}/sessions/{}", pair.pair_id))
            .bearer_auth(&poll_secret)
            .timeout(Duration::from_secs(3))
            .send()
            .await;
        clear_login(&state, &flag).await;
        return Err("Не удалось открыть Mini App. Повторите подключение.".into());
    }
    let start = BridgeStart {
        pair_code: pair.pair_id.clone(),
        expires_at: pair.expires_at,
    };
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = receive(
            &state,
            &pair.pair_id,
            &poll_secret,
            &secret,
            &flag,
            client,
            expected,
            &requested,
        )
        .await;
        let _ = state
            .client
            .delete(format!("{BASE}/sessions/{}", pair.pair_id))
            .bearer_auth(&poll_secret)
            .timeout(Duration::from_secs(3))
            .send()
            .await;
        clear_login(&state, &flag).await;
        let payload = match result {
            Ok(user) => json!({"user": user}),
            Err(error) => json!({"error": error}),
        };
        let _ = app.emit("auth-result", payload);
    });
    Ok(start)
}

#[allow(clippy::too_many_arguments)]
async fn receive(
    state: &AppState,
    pair: &str,
    poll_secret: &str,
    secret: &SecretKey,
    flag: &AtomicBool,
    app_id: u64,
    expected: Option<i64>,
    requested: &[String],
) -> Result<User, String> {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if flag.load(Ordering::Relaxed) {
            return Err("Вход отменён.".into());
        }
        if Instant::now() >= deadline {
            return Err("Время подключения истекло. Откройте Mini App заново.".into());
        }
        let request = state
            .client
            .post(format!("{BASE}/sessions/{pair}/poll"))
            .bearer_auth(poll_secret)
            .timeout(Duration::from_secs(10))
            .send();
        let response = tokio::select! { result = request => result, _ = cancel_signal(flag) => return Err("Вход отменён.".into()) };
        if let Ok(response) = response {
            if response.status() == 410 {
                return Err("Код подключения истёк. Откройте Mini App заново.".into());
            }
            if response.status() == 200 {
                let envelope = response.json::<Envelope>().await.map_err(|_| INVALID)?;
                let credentials = decrypt(secret, pair, envelope)?;
                return verify_and_connect(state, credentials, app_id, expected, requested, flag)
                    .await;
            }
            if response.status() != 202 && !response.status().is_server_error() {
                return Err("Сервер отклонил подключение. Откройте Mini App заново.".into());
            }
        }
        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(2)) => {}, _ = cancel_signal(flag) => return Err("Вход отменён.".into()) };
    }
}

async fn verify_and_connect(
    state: &AppState,
    credentials: Credentials,
    app_id: u64,
    expected: Option<i64>,
    requested: &[String],
    flag: &AtomicBool,
) -> Result<User, String> {
    if credentials.app_id != app_id
        || credentials.user_id <= 0
        || expected.is_some_and(|id| id != credentials.user_id)
    {
        return Err("Mini App подключил другой аккаунт или приложение. Повторите вход.".into());
    }
    if requested.iter().any(|s| !credentials.scopes.contains(s)) {
        return Err("ВК не предоставил запрошенные разрешения.".into());
    }
    let token = auth::parse_token(&credentials.access_token)?;
    let owner = credentials.user_id.to_string();
    for scope in requested {
        let method = match scope.as_str() {
            "photos" => "photos.getAlbums",
            "video" => "video.get",
            "groups" => "groups.get",
            "docs" => "docs.get",
            "wall" => "wall.get",
            "friends" => "friends.get",
            _ => continue,
        };
        let params = if scope == "groups" {
            vec![("filter", "admin".into()), ("count", "1".into())]
        } else if scope == "friends" {
            vec![("user_id", owner.clone()), ("count", "1".into())]
        } else {
            vec![("owner_id", owner.clone()), ("count", "1".into())]
        };
        if flag.load(Ordering::Relaxed) {
            return Err("Вход отменён.".into());
        }
        api::call_token(state, &token, method, &params)
            .await
            .map_err(|error| format!("Mini App выдал токен, но раздел недоступен. {error}"))?;
    }
    if flag.load(Ordering::Relaxed) {
        return Err("Вход отменён.".into());
    }
    auth::establish_for(state, token, None, Some(credentials.user_id), Some(flag)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn credentials() -> Credentials {
        Credentials {
            access_token: "test-only-not-a-real-token-123456789".into(),
            app_id: 12345,
            user_id: 123,
            scopes: vec!["photos".into(), "video".into(), "groups".into()],
        }
    }
    async fn existing_session(state: &AppState) {
        *state.session.write().await = Some(Session {
            token: "previous-session".into(),
            user: User {
                id: 123,
                name: "Previous account".into(),
                avatar: None,
            },
            refresh: None,
        });
    }
    #[tokio::test]
    async fn denied_photo_access_does_not_replace_existing_session() {
        let (base, mut requests) = crate::test_support::json_server(vec![json!({"error":{"error_code":1051,"error_msg":"Method is not available for this profile type"}})]).await;
        let mut state = AppState::new();
        state.api_base = base;
        existing_session(&state).await;
        let error = verify_and_connect(
            &state,
            credentials(),
            12345,
            Some(123),
            &["photos".into()],
            &AtomicBool::new(false),
        )
        .await
        .err()
        .unwrap();
        assert!(error.contains("1051"));
        assert!(!error.contains("test-only"));
        assert_eq!(
            state.session.read().await.as_ref().unwrap().token,
            "previous-session"
        );
        assert!(
            requests
                .recv()
                .await
                .unwrap()
                .starts_with("POST /photos.getAlbums")
        );
    }
    #[tokio::test]
    async fn rejects_wrong_app_account_missing_scopes_and_cancel_before_any_api_call() {
        let state = AppState::new();
        existing_session(&state).await;
        assert!(
            verify_and_connect(
                &state,
                credentials(),
                222,
                Some(123),
                &["photos".into()],
                &AtomicBool::new(false)
            )
            .await
            .is_err()
        );
        assert!(
            verify_and_connect(
                &state,
                credentials(),
                12345,
                Some(456),
                &["photos".into()],
                &AtomicBool::new(false)
            )
            .await
            .is_err()
        );
        assert!(
            verify_and_connect(
                &state,
                credentials(),
                12345,
                Some(123),
                &["docs".into()],
                &AtomicBool::new(false)
            )
            .await
            .is_err()
        );
        assert!(
            verify_and_connect(
                &state,
                credentials(),
                12345,
                Some(123),
                &["photos".into()],
                &AtomicBool::new(true)
            )
            .await
            .is_err()
        );
        assert_eq!(
            state.session.read().await.as_ref().unwrap().token,
            "previous-session"
        );
    }
    #[tokio::test]
    async fn accepts_verified_media_access_and_validates_the_actual_token_account() {
        for actual_id in [123, 456] {
            let (base, _) = crate::test_support::json_server(vec![
                json!({"response":{"count":0,"items":[]}}),
                json!({"response":{"count":0,"items":[]}}),
                json!({"response":{"count":0,"items":[]}}),
                json!({"response":[{"id":actual_id,"first_name":"Test","last_name":"Account"}]}),
            ])
            .await;
            let mut state = AppState::new();
            state.api_base = base;
            existing_session(&state).await;
            let result = verify_and_connect(
                &state,
                credentials(),
                12345,
                Some(123),
                &["photos".into(), "video".into(), "groups".into()],
                &AtomicBool::new(false),
            )
            .await;
            assert_eq!(result.is_ok(), actual_id == 123);
            let session = state.session.read().await;
            assert_eq!(session.as_ref().unwrap().user.id, 123);
            assert_eq!(
                session.as_ref().unwrap().token,
                if actual_id == 123 {
                    "test-only-not-a-real-token-123456789"
                } else {
                    "previous-session"
                }
            );
        }
    }
    #[test]
    fn accepts_only_supported_scopes_and_fixed_pair_ids() {
        assert_eq!(
            requested_scopes("photos,video groups photos").unwrap(),
            ["photos", "video", "groups"]
        );
        assert!(requested_scopes("messages").is_err());
        assert!(requested_scopes("").is_err());
        assert!(valid_id(&random_id()));
        assert!(!valid_id(&"../".repeat(22)));
    }
    #[test]
    fn decrypts_browser_fixture_and_rejects_pair_substitution_and_tampering() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../test-fixtures/bridge.json")).unwrap();
        let secret = SecretKey::from_slice(
            &URL_SAFE_NO_PAD
                .decode(fixture["privateKey"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let pair = fixture["pairId"].as_str().unwrap();
        let envelope = || serde_json::from_value::<Envelope>(fixture["envelope"].clone()).unwrap();
        let credentials = decrypt(&secret, pair, envelope()).unwrap();
        assert_eq!(
            credentials.access_token,
            "test-only-not-a-real-token-123456789"
        );
        assert_eq!(credentials.user_id, 123);
        assert!(decrypt(&secret, &random_id(), envelope()).is_err());
        let mut modified = envelope();
        modified.ciphertext.replace_range(0..1, "_");
        assert!(decrypt(&secret, pair, modified).is_err());
    }
}
