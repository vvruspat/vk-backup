use crate::{auth, model::*};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tauri::{Emitter, State};

pub async fn call(
    state: &AppState,
    method: &str,
    params: &[(&str, String)],
) -> Result<Value, String> {
    let token = auth::access_token(state).await?;
    call_token(state, &token, method, params).await
}

pub async fn call_token(
    state: &AppState,
    token: &str,
    method: &str,
    params: &[(&str, String)],
) -> Result<Value, String> {
    for attempt in 0..4 {
        {
            let mut last = state.limiter.lock().await;
            tokio::time::sleep(Duration::from_millis(360).saturating_sub(last.elapsed())).await;
            *last = Instant::now();
        }
        let mut form = params.to_vec();
        form.extend([("access_token", token.to_string()), ("v", "5.199".into())]);
        #[cfg(test)]
        let endpoint = format!("{}/{method}", state.api_base);
        #[cfg(not(test))]
        let endpoint = format!("https://api.vk.com/method/{method}");
        let response = state
            .client
            .post(endpoint)
            .timeout(Duration::from_secs(45))
            .form(&form)
            .send()
            .await
            .map_err(|_| "Не удалось связаться с ВК. Проверьте соединение.".to_string())?;
        if response.status().is_server_error() || response.status().as_u16() == 429 {
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            continue;
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| "ВК вернул некорректный ответ.".to_string())?;
        if let Some(error) = body.get("error") {
            let code = error["error_code"].as_u64().unwrap_or(0);
            if code == 6 || code == 10 {
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                continue;
            }
            let explanation = match code {
                5 | 27 | 28 => "Токен недействителен или истёк. Подключите аккаунт заново.",
                7 | 15 | 20 | 200 | 201 | 203 => {
                    "ВК не предоставил доступ. Проверьте права токена и приложения."
                }
                14 => "ВК требует капчу. Откройте ВК в браузере, затем повторите попытку.",
                17 => "ВК требует подтверждения аккаунта в браузере.",
                3 => "Этот метод недоступен для вашего приложения ВК.",
                _ => "ВК отклонил запрос.",
            };
            // Raw VK errors include request_params containing the access token.
            return Err(format!("{method}: {explanation} (код {code})"));
        }
        return body
            .get("response")
            .cloned()
            .ok_or_else(|| "ВК не вернул данные.".into());
    }
    Err(format!(
        "{method}: ВК временно ограничил запросы. Повторите позже."
    ))
}

pub fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").to_string()
}
pub fn number(value: &Value, key: &str) -> i64 {
    value[key].as_i64().unwrap_or(0)
}

pub async fn paged(
    state: &AppState,
    method: &str,
    params: &[(&str, String)],
    size: u64,
) -> Result<Vec<Value>, String> {
    let mut result = Vec::new();
    let mut offset = 0;
    loop {
        let mut args = params.to_vec();
        args.extend([("offset", offset.to_string()), ("count", size.to_string())]);
        let response = call(state, method, &args).await?;
        let items = response["items"]
            .as_array()
            .ok_or("ВК вернул некорректный список.")?;
        if items.is_empty() {
            break;
        }
        offset += items.len() as u64;
        result.extend(items.iter().cloned());
        if offset >= response["count"].as_u64().unwrap_or(offset) {
            break;
        }
    }
    Ok(result)
}

#[derive(serde::Serialize)]
pub struct Sources {
    pub items: Vec<Source>,
    pub warning: Option<String>,
}

#[tauri::command]
pub async fn sources(state: State<'_, AppState>) -> Result<Sources, String> {
    if state.busy().await {
        return Err("Дождитесь завершения бэкапа.".into());
    }
    let user = state
        .session
        .read()
        .await
        .as_ref()
        .ok_or("Сначала подключите аккаунт.")?
        .user
        .clone();
    let mut items = vec![Source {
        id: user.id,
        name: user.name,
        avatar: user.avatar,
        community: false,
    }];
    let mut warning = None;
    match paged(
        &state,
        "groups.get",
        &[
            ("filter", "admin".into()),
            ("extended", "1".into()),
            ("fields", "photo_200".into()),
        ],
        100,
    )
    .await
    {
        Ok(groups) => {
            for g in groups {
                let id = number(&g, "id");
                if id > 0 {
                    items.push(Source {
                        id: -id,
                        name: text(&g, "name"),
                        avatar: g["photo_200"].as_str().map(str::to_string),
                        community: true,
                    });
                }
            }
        }
        Err(e) => {
            warning = Some(format!(
                "Не удалось получить администрируемые сообщества. {e}"
            ))
        }
    }
    *state.sources.write().await = items.iter().map(|s| (s.id, s.clone())).collect();
    Ok(Sources { items, warning })
}

#[allow(clippy::too_many_arguments)]
fn collection(
    owner: i64,
    id: &str,
    api_id: String,
    kind: Kind,
    title: String,
    count: u64,
    cover: Option<String>,
    description: String,
) -> Collection {
    Collection {
        id: format!("{owner}:{id}"),
        owner_id: owner,
        api_id,
        kind,
        title,
        count,
        cover,
        description,
        available: true,
        error: None,
    }
}

fn denied(owner: i64, id: &str, kind: Kind, title: &str, error: String) -> Collection {
    Collection {
        id: format!("{owner}:{id}"),
        owner_id: owner,
        api_id: String::new(),
        kind,
        title: title.into(),
        count: 0,
        cover: None,
        description: "Доступ ограничен ВК".into(),
        available: false,
        error: Some(error),
    }
}

#[tauri::command]
pub async fn catalog(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    owner_id: i64,
) -> Result<Vec<Collection>, String> {
    if state.busy().await {
        return Err("Дождитесь завершения бэкапа перед обновлением списка.".into());
    }
    let source = state
        .sources
        .read()
        .await
        .get(&owner_id)
        .cloned()
        .ok_or("Этот аккаунт или сообщество недоступны. Обновите список сообществ.")?;
    let owner = owner_id.to_string();
    let mut result = Vec::new();
    let _ = app.emit("scan-progress", "Читаем фотоальбомы…");
    match paged(
        &state,
        "photos.getAlbums",
        &[
            ("owner_id", owner.clone()),
            ("need_system", "1".into()),
            ("need_covers", "1".into()),
            ("photo_sizes", "1".into()),
        ],
        100,
    )
    .await
    {
        Ok(items) => {
            let mut seen = HashSet::new();
            for a in items {
                let id = number(&a, "id").to_string();
                if !seen.insert(id.clone()) {
                    continue;
                }
                let api_id = match id.as_str() {
                    "-6" => "profile",
                    "-7" => "wall",
                    "-15" => "saved",
                    _ => &id,
                }
                .to_string();
                let cover = a["thumb_src"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| largest(&a["sizes"]));
                result.push(collection(
                    owner_id,
                    &format!("photos:{id}"),
                    api_id,
                    Kind::Photos,
                    text(&a, "title"),
                    a["size"].as_u64().unwrap_or(0),
                    cover,
                    text(&a, "description"),
                ));
            }
        }
        Err(e) => result.push(denied(owner_id, "photos", Kind::Photos, "Фотографии", e)),
    }
    let _ = app.emit("scan-progress", "Проверяем видео…");
    match call(
        &state,
        "video.get",
        &[("owner_id", owner.clone()), ("count", "1".into())],
    )
    .await
    {
        Ok(v) => {
            result.push(collection(
                owner_id,
                "videos:all",
                String::new(),
                Kind::Videos,
                "Все видеозаписи".into(),
                v["count"].as_u64().unwrap_or(0),
                largest(&v["items"][0]["image"]),
                "Лучшее доступное качество".into(),
            ));
            match paged(
                &state,
                "video.getAlbums",
                &[("owner_id", owner.clone()), ("extended", "1".into())],
                100,
            )
            .await
            {
                Ok(albums) => {
                    for a in albums {
                        let id = number(&a, "id").to_string();
                        result.push(collection(
                            owner_id,
                            &format!("videos:{id}"),
                            id,
                            Kind::Videos,
                            text(&a, "title"),
                            a["count"].as_u64().unwrap_or(0),
                            largest(&a["image"]),
                            "Видеоальбом".into(),
                        ));
                    }
                }
                Err(e) => result.push(denied(
                    owner_id,
                    "videos:albums",
                    Kind::Videos,
                    "Видеоальбомы",
                    e,
                )),
            }
        }
        Err(e) => result.push(denied(
            owner_id,
            "videos:all",
            Kind::Videos,
            "Видеозаписи",
            e,
        )),
    }
    let _ = app.emit("scan-progress", "Проверяем документы и данные…");
    let mut extra = vec![
        (
            "docs.get",
            "documents",
            Kind::Documents,
            "Документы",
            "Оригиналы вложенных файлов",
        ),
        (
            "wall.get",
            "wall",
            Kind::Wall,
            "Записи на стене",
            "Текст и данные вложений в JSON",
        ),
    ];
    if owner_id > 0 {
        extra.extend([
            (
                "friends.get",
                "friends",
                Kind::Friends,
                "Друзья",
                "Список друзей в JSON",
            ),
            (
                "messages.getConversations",
                "messages",
                Kind::Messages,
                "Переписка",
                "Диалоги и история сообщений в JSON; доступ зависит от приложения",
            ),
        ]);
    }
    for (method, id, kind, title, description) in extra {
        let param = if kind == Kind::Friends {
            "user_id"
        } else {
            "owner_id"
        };
        match call(
            &state,
            method,
            &[(param, owner.clone()), ("count", "1".into())],
        )
        .await
        {
            Ok(v) => result.push(collection(
                owner_id,
                id,
                String::new(),
                kind,
                title.into(),
                v["count"].as_u64().unwrap_or(0),
                None,
                description.into(),
            )),
            Err(e) => result.push(denied(owner_id, id, kind, title, e)),
        }
    }
    result.push(collection(
        owner_id,
        "profile",
        String::new(),
        Kind::Profile,
        if owner_id < 0 {
            "Информация о сообществе"
        } else {
            "Профиль"
        }
        .into(),
        1,
        source.avatar,
        "Основные сведения в JSON".into(),
    ));
    // Preserve catalogs for other sources so a single backup can include several communities.
    let mut cache = state.collections.write().await;
    cache.retain(|_, c| c.owner_id != owner_id);
    cache.extend(result.iter().map(|c| (c.id.clone(), c.clone())));
    Ok(result)
}

pub fn largest(sizes: &Value) -> Option<String> {
    sizes
        .as_array()?
        .iter()
        .filter(|s| s["url"].as_str().is_some())
        .max_by_key(|s| s["width"].as_u64().unwrap_or(0) * s["height"].as_u64().unwrap_or(0))
        .and_then(|s| s["url"].as_str().map(str::to_string))
}

pub fn video_url(v: &Value) -> Option<String> {
    v["files"]
        .as_object()?
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("mp4_")?
                .parse::<u64>()
                .ok()
                .map(|quality| (quality, v))
        })
        .max_by_key(|(q, _)| *q)
        .and_then(|(_, v)| v.as_str().map(str::to_string))
}

fn media(kind: &Kind, raw: Value) -> Media {
    let owner = number(&raw, "owner_id");
    let id = format!("{owner}_{}", number(&raw, "id"));
    let (title, url, thumbnail, extension, player) = match kind {
        Kind::Photos => (
            if text(&raw, "text").is_empty() {
                format!("Фотография {}", number(&raw, "id"))
            } else {
                text(&raw, "text")
            },
            raw["orig_photo"]["url"]
                .as_str()
                .map(str::to_string)
                .or_else(|| largest(&raw["sizes"])),
            largest(&raw["sizes"]),
            "jpg".into(),
            None,
        ),
        Kind::Videos => (
            text(&raw, "title"),
            video_url(&raw),
            largest(&raw["image"]),
            "mp4".into(),
            raw["player"].as_str().map(str::to_string),
        ),
        _ => (
            text(&raw, "title"),
            raw["url"].as_str().map(str::to_string),
            None,
            text(&raw, "ext"),
            None,
        ),
    };
    let downloadable = url.is_some();
    let reason = if downloadable {
        None
    } else if *kind == Kind::Videos {
        Some("Прямой файл не предоставлен ВК; попробуем получить его при бэкапе или использовать yt-dlp.".into())
    } else {
        Some("ВК не предоставил прямой файл для скачивания.".into())
    };
    Media {
        id,
        title,
        url,
        thumbnail,
        extension,
        player,
        raw: raw.clone(),
        duration: raw["duration"].as_u64(),
        date: raw["date"].as_i64(),
        downloadable,
        reason,
    }
}

pub async fn items(state: &AppState, c: &Collection, offset: u64) -> Result<Page, String> {
    let mut params = vec![
        ("owner_id", c.owner_id.to_string()),
        ("offset", offset.to_string()),
        ("count", "100".into()),
    ];
    let method = match c.kind {
        Kind::Photos => {
            params.extend([("album_id", c.api_id.clone()), ("photo_sizes", "1".into())]);
            "photos.get"
        }
        Kind::Videos => {
            if !c.api_id.is_empty() {
                params.push(("album_id", c.api_id.clone()));
            }
            "video.get"
        }
        Kind::Documents => "docs.get",
        _ => return Err("Этот раздел сохраняется целиком в JSON.".into()),
    };
    let response = call(state, method, &params).await?;
    let raw = response["items"]
        .as_array()
        .ok_or("ВК вернул некорректный список файлов.")?;
    let total = response["count"].as_u64().unwrap_or(raw.len() as u64);
    let next = offset + raw.len() as u64;
    Ok(Page {
        items: raw.iter().cloned().map(|v| media(&c.kind, v)).collect(),
        total,
        next_offset: if next < total && !raw.is_empty() {
            Some(next)
        } else {
            None
        },
    })
}

#[tauri::command]
pub async fn collection_items(
    state: State<'_, AppState>,
    collection_id: String,
    offset: u64,
) -> Result<Page, String> {
    let c = state
        .collections
        .read()
        .await
        .get(&collection_id)
        .cloned()
        .ok_or("Альбом не найден. Обновите список.")?;
    items(&state, &c, offset).await
}

pub async fn export_data(state: &AppState, c: &Collection, job: &Job) -> Result<Value, String> {
    let owner = c.owner_id.to_string();
    if c.kind == Kind::Profile {
        return if c.owner_id < 0 {
            call(
                state,
                "groups.getById",
                &[
                    ("group_ids", (-c.owner_id).to_string()),
                    (
                        "fields",
                        "description,contacts,links,site,status,members_count,city,country".into(),
                    ),
                ],
            )
            .await
        } else {
            call(state, "users.get", &[("user_ids", owner), ("fields", "photo_200,about,bdate,city,country,contacts,education,career,interests,activities,music,movies,books,quotes,personal,status,relation,site,screen_name".into())]).await
        };
    }
    let (method, mut params, page_size) = match c.kind {
        Kind::Wall => ("wall.get", vec![("owner_id", owner)], 100),
        Kind::Friends => (
            "friends.get",
            vec![
                ("user_id", owner),
                ("fields", "nickname,domain,city,country".into()),
            ],
            100,
        ),
        Kind::Messages => ("messages.getConversations", vec![], 100),
        _ => return Err("Неизвестный тип экспорта.".into()),
    };
    let mut offset = 0;
    let mut all = Vec::new();
    loop {
        crate::backup::checkpoint(job).await?;
        let mut args = params.clone();
        args.extend([
            ("offset", offset.to_string()),
            ("count", page_size.to_string()),
        ]);
        let value = call(state, method, &args).await?;
        let page = value["items"]
            .as_array()
            .ok_or("ВК вернул некорректный список.")?;
        if page.is_empty() {
            break;
        }
        offset += page.len() as u64;
        all.extend(page.iter().cloned());
        if offset >= value["count"].as_u64().unwrap_or(offset) {
            break;
        }
    }
    if c.kind == Kind::Messages {
        for convo in &mut all {
            crate::backup::checkpoint(job).await?;
            let peer = convo["conversation"]["peer"]["id"]
                .as_i64()
                .ok_or("ВК не вернул идентификатор диалога.")?;
            params = vec![("peer_id", peer.to_string())];
            let mut history = Vec::new();
            let mut offset = 0;
            loop {
                crate::backup::checkpoint(job).await?;
                let mut args = params.clone();
                args.extend([("count", "200".into()), ("offset", offset.to_string())]);
                let v = call(state, "messages.getHistory", &args).await?;
                let p = v["items"]
                    .as_array()
                    .ok_or("ВК не вернул историю диалога.")?;
                if p.is_empty() {
                    break;
                }
                offset += p.len() as u64;
                history.extend(p.iter().cloned());
                if offset >= v["count"].as_u64().unwrap_or(offset) {
                    break;
                }
            }
            convo["history"] = json!(history);
        }
    }
    Ok(json!({"count": all.len(), "items": all}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pagination_fetches_every_page_with_the_group_owner() {
        let (address, mut requests) = crate::test_support::json_server(vec![
            json!({"response":{"count":3,"items":[{"id":1},{"id":2}]}}),
            json!({"response":{"count":3,"items":[{"id":3}]}}),
        ])
        .await;
        let mut state = AppState::new();
        state.api_base = address;
        *state.session.write().await = Some(Session {
            token: "test-user-token".into(),
            user: User {
                id: 123,
                name: "Test".into(),
                avatar: None,
            },
            refresh: None,
        });
        let items = paged(&state, "photos.getAlbums", &[("owner_id", "-77".into())], 2)
            .await
            .unwrap();
        assert_eq!(items.len(), 3);
        let first = requests.recv().await.unwrap();
        let second = requests.recv().await.unwrap();
        assert!(first.contains("owner_id=-77") && first.contains("offset=0"));
        assert!(second.contains("owner_id=-77") && second.contains("offset=2"));
    }
    #[tokio::test]
    async fn errors_never_return_echoed_token_or_request_parameters() {
        let (address, _) = crate::test_support::json_server(vec![json!({"error":{"error_code":15,"error_msg":"secret-test-token","request_params":[{"key":"access_token","value":"secret-test-token"}]}})]).await;
        let mut state = AppState::new();
        state.api_base = address;
        let error = call_token(&state, "secret-test-token", "photos.get", &[])
            .await
            .unwrap_err();
        assert!(error.contains("код 15"));
        assert!(!error.contains("secret-test-token"));
    }
    #[test]
    fn largest_image_and_video_quality() {
        assert_eq!(
            largest(
                &json!([{"url":"small","width":100,"height":100},{"url":"large","width":2000,"height":1000}])
            ),
            Some("large".into())
        );
        assert_eq!(
            video_url(&json!({"files":{"hls":"stream","mp4_240":"low","mp4_1080":"high"}})),
            Some("high".into())
        );
    }
    #[test]
    fn group_collection_has_negative_owner_and_distinct_id() {
        let c = collection(
            -42,
            "photos:7",
            "7".into(),
            Kind::Photos,
            "Album".into(),
            2,
            None,
            String::new(),
        );
        assert_eq!(c.owner_id, -42);
        assert_eq!(c.id, "-42:photos:7");
    }
}
