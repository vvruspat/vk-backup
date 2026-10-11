use crate::{api, model::*};
use fs2::FileExt;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use url::Url;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    title: String,
    collection: String,
    path: Option<String>,
    status: String,
    bytes: u64,
    sha256: Option<String>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    account_id: i64,
    updated_at: u64,
    status: String,
    entries: BTreeMap<String, Entry>,
    #[serde(default)]
    warnings: Vec<String>,
}

struct Task {
    collection: Collection,
    media: Option<Media>,
    relative: PathBuf,
    key: String,
}

pub fn safe_remote(url: &Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    host != "localhost"
        && !host.ends_with(".localhost")
        && !host.ends_with(".local")
        && !host.ends_with(".internal")
        && host.contains('.')
        && host.parse::<std::net::IpAddr>().is_err()
}

pub fn filename(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars() {
        if result.len() + ch.len_utf8() > 100 {
            break;
        }
        result.push(if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
            '_'
        } else {
            ch
        });
    }
    let trimmed = result.trim().trim_matches('.').trim().to_string();
    let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    if trimmed.is_empty() {
        "Без названия".into()
    } else if [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ]
    .contains(&stem.as_str())
    {
        format!("_{trimmed}")
    } else {
        trimmed
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn io_error(_: std::io::Error) -> String {
    "Не удалось записать файлы. Проверьте права на папку и свободное место.".into()
}

// Every generated path is relative to the user-selected canonical root. Refuse symlinks
// within the archive, including files left by earlier runs or a hand-edited manifest.
async fn checked_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err("Недопустимый путь в архиве.".into());
        };
        current.push(part);
        match fs::symlink_metadata(&current).await {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("Архив содержит символическую ссылку. Выберите другую папку.".into());
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(current)
}

async fn mkdir(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let path = checked_path(root, relative).await?;
    fs::create_dir_all(&path).await.map_err(io_error)?;
    Ok(path)
}

pub async fn checkpoint(job: &Job) -> Result<(), String> {
    loop {
        if job.canceled.load(Ordering::Relaxed) {
            return Err("Бэкап отменён.".into());
        }
        if !job.paused.load(Ordering::Relaxed) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn emit(app: &tauri::AppHandle, job: &Job) {
    let p = job.progress.lock().await.clone();
    let _ = app.emit("backup-progress", p);
}

fn relative_collection(c: &Collection) -> PathBuf {
    let source = if c.owner_id < 0 {
        format!("Сообщества/club{}", -c.owner_id)
    } else {
        format!("Мой аккаунт/id{}", c.owner_id)
    };
    let kind = match c.kind {
        Kind::Photos => "Фото",
        Kind::Videos => "Видео",
        Kind::Documents => "Документы",
        _ => "Данные",
    };
    let folder = format!("{} [{}]", filename(&c.title), filename(&c.id));
    PathBuf::from(source).join(kind).join(folder)
}

#[tauri::command]
pub async fn start_backup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    folder: String,
    selections: Vec<Selection>,
    use_ytdlp: bool,
) -> Result<Progress, String> {
    let mut guard = state.job.lock().await;
    if let Some(job) = guard.as_ref()
        && matches!(
            job.progress.lock().await.status.as_str(),
            "running" | "paused" | "canceling"
        )
    {
        return Err("Бэкап уже выполняется.".into());
    }
    let account_id = state
        .session
        .read()
        .await
        .as_ref()
        .ok_or("Сначала подключите аккаунт.")?
        .user
        .id;
    if selections.is_empty() {
        return Err("Выберите хотя бы один альбом или раздел.".into());
    }
    let cache = state.collections.read().await;
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for selection in selections {
        if !seen.insert(selection.collection_id.clone()) {
            continue;
        }
        let c = cache
            .get(&selection.collection_id)
            .cloned()
            .ok_or("Выбранный альбом не найден. Обновите список.")?;
        if !state.sources.read().await.contains_key(&c.owner_id) {
            return Err(
                "Один из выбранных источников больше недоступен. Обновите сообщества и выбор."
                    .into(),
            );
        }
        if !c.available {
            return Err(format!("Раздел «{}» недоступен.", c.title));
        }
        if selection.item_ids.as_ref().is_some_and(Vec::is_empty) {
            continue;
        }
        selected.push((c, selection.item_ids, selection.excluded_ids));
    }
    drop(cache);
    if selected.is_empty() {
        return Err("Не выбрано ни одного файла.".into());
    }
    let destination = fs::canonicalize(&folder)
        .await
        .map_err(|_| "Выбранная папка не существует или недоступна.")?;
    if !destination.is_dir() {
        return Err("Выберите папку для бэкапа.".into());
    }
    let root = mkdir(
        &destination,
        Path::new(&format!("VK Backup - id{account_id}")),
    )
    .await?;
    let lock_path = checked_path(&root, Path::new(".backup.lock")).await?;
    let archive_lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(io_error)?;
    archive_lock.try_lock_exclusive().map_err(|_| "Эту папку уже использует другой экземпляр VK Backup. Дождитесь завершения или выберите другую папку.")?;
    let progress = Progress {
        id: uuid::Uuid::new_v4().to_string(),
        status: "running".into(),
        phase: "scanning".into(),
        total: 0,
        completed: 0,
        skipped: 0,
        failed: 0,
        unavailable: 0,
        bytes: 0,
        current: "Собираем список файлов…".into(),
        current_bytes: 0,
        current_total: None,
        speed: 0.0,
        folder: root.to_string_lossy().to_string(),
        errors: vec![],
        started_at: now(),
    };
    let job = Arc::new(Job {
        progress: tokio::sync::Mutex::new(progress.clone()),
        paused: AtomicBool::new(false),
        canceled: AtomicBool::new(false),
    });
    *guard = Some(job.clone());
    drop(guard);
    tauri::async_runtime::spawn(async move {
        let _archive_lock = archive_lock;
        let state = app.state::<AppState>();
        let result = run(&app, &state, &job, &root, account_id, selected, use_ytdlp).await;
        let mut p = job.progress.lock().await;
        if let Err(e) = result {
            p.status = if job.canceled.load(Ordering::Relaxed) {
                "canceled"
            } else {
                "failed"
            }
            .into();
            if p.status == "failed" {
                p.errors.push(e);
            }
        } else {
            p.status = if p.failed > 0 || p.unavailable > 0 {
                "partial"
            } else {
                "completed"
            }
            .into();
        }
        p.phase = "finished".into();
        p.speed = 0.0;
        let final_status = p.status.clone();
        let warnings = p.errors.clone();
        drop(p);
        if let Err(e) = finalize(&root, account_id, &final_status, warnings).await {
            let mut p = job.progress.lock().await;
            p.errors.push(e);
            if p.status == "completed" {
                p.status = "failed".into();
            }
        }
        emit(&app, &job).await;
    });
    Ok(progress)
}

async fn run(
    app: &tauri::AppHandle,
    state: &AppState,
    job: &Job,
    root: &Path,
    account_id: i64,
    selected: Vec<(Collection, Option<Vec<String>>, Vec<String>)>,
    use_ytdlp: bool,
) -> Result<(), String> {
    let manifest_path = checked_path(root, Path::new("manifest.json")).await?;
    let mut manifest = match fs::read(&manifest_path).await {
        Ok(bytes) => serde_json::from_slice::<Manifest>(&bytes).map_err(|_| "Существующий manifest.json повреждён. Выберите новую папку, чтобы сохранить старый архив.")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Manifest { version: 1, account_id, updated_at: now(), status: "running".into(), entries: BTreeMap::new(), warnings: vec![] },
        Err(e) => return Err(io_error(e)),
    };
    if manifest.account_id != account_id || manifest.version != 1 {
        return Err(
            "Существующий архив принадлежит другому аккаунту или имеет неподдерживаемую версию."
                .into(),
        );
    }
    manifest.status = "running".into();
    save_manifest(root, &mut manifest).await?;
    let mut tasks = Vec::new();
    for (c, selected_ids, excluded_ids) in selected {
        checkpoint(job).await?;
        job.progress.lock().await.current = format!("Читаем: {}", c.title);
        emit(app, job).await;
        let relative = relative_collection(&c);
        mkdir(root, &relative).await?;
        let mut metadata = serde_json::to_value(&c).map_err(|_| "Ошибка данных альбома.")?;
        metadata["source"] = json!(state.sources.read().await.get(&c.owner_id));
        write_json(root, &relative.join("album.json"), &metadata).await?;
        if matches!(c.kind, Kind::Photos | Kind::Videos | Kind::Documents) {
            let mut offset = 0;
            let filter = selected_ids.map(|ids| ids.into_iter().collect::<HashSet<_>>());
            let excluded: HashSet<_> = excluded_ids.into_iter().collect();
            let mut seen = HashSet::new();
            let mut found = HashSet::new();
            loop {
                checkpoint(job).await?;
                let page = match api::items(state, &c, offset).await {
                    Ok(p) => p,
                    Err(e) => {
                        record_error(job, format!("{}: {e}", c.title)).await;
                        break;
                    }
                };
                for media in page.items {
                    if !seen.insert(media.id.clone()) {
                        continue;
                    }
                    if filter.as_ref().is_some_and(|f| !f.contains(&media.id)) {
                        continue;
                    }
                    if excluded.contains(&media.id) {
                        continue;
                    }
                    found.insert(media.id.clone());
                    let ext = filename(&media.extension)
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .take(10)
                        .collect::<String>();
                    let ext = if ext.is_empty() { "bin".into() } else { ext };
                    let path = relative.join(format!(
                        "{} [{}].{}",
                        filename(&media.title),
                        filename(&media.id),
                        ext
                    ));
                    tasks.push(Task {
                        key: format!("{}/{}", c.id, media.id),
                        collection: c.clone(),
                        media: Some(media),
                        relative: path,
                    });
                }
                {
                    let mut p = job.progress.lock().await;
                    p.total = tasks.len() + p.failed;
                }
                emit(app, job).await;
                if let Some(next) = page.next_offset {
                    offset = next;
                } else {
                    break;
                }
            }
            if let Some(ids) = filter {
                for missing in ids.difference(&found) {
                    record_error(
                        job,
                        format!(
                            "{}: выбранный файл {missing} больше не доступен в ВК.",
                            c.title
                        ),
                    )
                    .await;
                }
            }
        } else {
            tasks.push(Task {
                key: c.id.clone(),
                relative: relative.join("data.json"),
                collection: c,
                media: None,
            });
        }
    }
    {
        let mut p = job.progress.lock().await;
        p.phase = "downloading".into();
        p.total = tasks.len() + p.failed;
    }
    emit(app, job).await;
    let outcome: Result<(), String> = async {
        for mut task in tasks {
            checkpoint(job).await?;
            let title = task.media.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| task.collection.title.clone());
            { let mut p = job.progress.lock().await; p.current = title.clone(); p.current_bytes = 0; p.current_total = None; p.speed = 0.0; }
            emit(app, job).await;
            let old = manifest.entries.get(&task.key).cloned();
            let is_data = task.media.is_none();
            if !is_data
                && let Some(old) = old.as_ref().filter(|e| e.status == "completed")
                    && let (Some(path), Some(hash)) = (&old.path, &old.sha256) {
                        let target = checked_path(root, Path::new(path)).await?;
                        if valid_file(&target, old.bytes, hash).await {
                            job.progress.lock().await.skipped += 1;
                            emit(app, job).await;
                            continue;
                        }
                    }
            let output = checked_path(root, &task.relative).await?;
            let relative_path = task.relative.to_string_lossy().replace('\\', "/");
            // Never replace an unrelated user's file which just happens to share a name.
            if fs::try_exists(&output).await.map_err(io_error)? && old.as_ref().and_then(|e| e.path.as_deref()) != Some(relative_path.as_str()) {
                return Err(format!("Файл «{}» уже существует вне manifest.json. Выберите другую папку.", output.file_name().unwrap_or_default().to_string_lossy()));
            }
            // Reserve ownership before writing a file. A crash between rename and the
            // completion receipt is recoverable on the next run without name collisions.
            manifest.entries.insert(task.key.clone(), Entry { title: title.clone(), collection: task.collection.title.clone(), path: Some(relative_path.clone()), status: "pending".into(), bytes: 0, sha256: None, error: None });
            save_manifest(root, &mut manifest).await?;
            let result = if let Some(media) = task.media.as_mut() {
                let mut raw = media.raw.clone();
                scrub(&mut raw);
                write_json(root, &task.relative.with_extension("metadata.json"), &raw).await?;
                if task.collection.kind == Kind::Videos && media.url.is_none() {
                    let access = api::text(&media.raw, "access_key");
                    let id = format!("{}{}", media.id, if access.is_empty() { String::new() } else { format!("_{access}") });
                    if let Ok(v) = api::call(state, "video.get", &[("videos", id)]).await
                        && let Some(v) = v["items"].as_array().and_then(|a| a.first()) { media.url = api::video_url(v); media.player = v["player"].as_str().map(str::to_string).or(media.player.clone()); }
                }
                if let Some(url) = &media.url {
                    let mut result = Err("Не удалось скачать файл.".into());
                    for attempt in 0..3 {
                        checkpoint(job).await?;
                        result = download(state, Some(app), job, root, &task.relative, url).await;
                        if result.is_ok() || job.canceled.load(Ordering::Relaxed) { break; }
                        if attempt < 2 { tokio::time::sleep(Duration::from_secs(1 << attempt)).await; }
                    }
                    result
                } else if task.collection.kind == Kind::Videos && use_ytdlp {
                    let mut result = Err("Не удалось получить видео.".into());
                    // The API's signed embed player can reject extraction while the
                    // normal page for the same video works. Prefer the normal page.
                    for url in video_pages(media) {
                        checkpoint(job).await?;
                        result = ytdlp(app, job, root, &task.relative, &url).await;
                        if result.is_ok() || job.canceled.load(Ordering::Relaxed) { break; }
                    }
                    result
                } else {
                    let error = "ВК не предоставил файл. Для видео можно включить yt-dlp и повторить бэкап.".to_string();
                    job.progress.lock().await.unavailable += 1;
                    manifest.entries.insert(task.key.clone(), Entry { title: title.clone(), collection: task.collection.title.clone(), path: Some(relative_path.clone()), status: "unavailable".into(), bytes: 0, sha256: None, error: Some(error.clone()) });
                    job.progress.lock().await.errors.push(format!("{title}: {error}"));
                    save_manifest(root, &mut manifest).await?;
                    emit(app, job).await;
                    continue;
                }
            } else {
                match api::export_data(state, &task.collection, job).await {
                    Ok(mut value) => { scrub(&mut value); write_json(root, &task.relative, &value).await },
                    Err(e) => Err(e),
                }
            };
            if job.canceled.load(Ordering::Relaxed) { return Err("Бэкап отменён.".into()); }
            match result {
                Ok(()) => {
                    let (bytes, hash) = hash_file(&output).await?;
                    let mut p = job.progress.lock().await;
                    p.completed += 1; p.bytes += bytes;
                    manifest.entries.insert(task.key, Entry { title, collection: task.collection.title, path: Some(relative_path.clone()), status: "completed".into(), bytes, sha256: Some(hash), error: None });
                },
                Err(e) => {
                    record_error(job, format!("{title}: {e}")).await;
                    manifest.entries.insert(task.key, Entry { title, collection: task.collection.title, path: Some(relative_path), status: "failed".into(), bytes: 0, sha256: None, error: Some(e) });
                },
            }
            save_manifest(root, &mut manifest).await?;
            emit(app, job).await;
        }
        Ok(())
    }.await;
    let p = job.progress.lock().await;
    manifest.status = if job.canceled.load(Ordering::Relaxed) {
        "canceled"
    } else if outcome.is_err() {
        "failed"
    } else if p.failed > 0 || p.unavailable > 0 {
        "partial"
    } else {
        "completed"
    }
    .into();
    drop(p);
    save_manifest(root, &mut manifest).await?;
    outcome
}

async fn finalize(
    root: &Path,
    account_id: i64,
    status: &str,
    warnings: Vec<String>,
) -> Result<(), String> {
    let path = checked_path(root, Path::new("manifest.json")).await?;
    let bytes = fs::read(&path).await.map_err(io_error)?;
    let mut manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| "Не удалось записать итоговый отчёт архива.")?;
    if manifest.account_id != account_id {
        return Err("Архив принадлежит другому аккаунту.".into());
    }
    manifest.status = status.into();
    manifest.warnings = warnings;
    save_manifest(root, &mut manifest).await?;
    write_gallery(root, &manifest).await
}

async fn record_error(job: &Job, error: String) {
    let mut p = job.progress.lock().await;
    p.failed += 1;
    p.errors.push(error);
}

async fn download(
    state: &AppState,
    app: Option<&tauri::AppHandle>,
    job: &Job,
    root: &Path,
    relative: &Path,
    url: &str,
) -> Result<(), String> {
    let url = Url::parse(url).map_err(|_| "ВК вернул некорректный адрес файла.")?;
    #[allow(unused_mut)]
    let mut allowed = safe_remote(&url);
    #[cfg(test)]
    {
        allowed |= url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    }
    if !allowed {
        return Err("Адрес файла не является безопасным HTTPS-адресом.".into());
    }
    let target = checked_path(root, relative).await?;
    let partial = checked_path(
        root,
        &relative.with_extension(format!(
            "{}.part",
            relative.extension().unwrap_or_default().to_string_lossy()
        )),
    )
    .await?;
    let result: Result<(), String> = async {
        let response = state
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| "Не удалось начать скачивание.")?;
        if !response.status().is_success() {
            return Err(format!(
                "Сервер файла вернул HTTP {}.",
                response.status().as_u16()
            ));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if content_type.contains("text/html") || content_type.contains("mpegurl") {
            return Err("Вместо медиа сервер вернул страницу или потоковый плейлист.".into());
        }
        let total = response.content_length();
        job.progress.lock().await.current_total = total;
        let mut file = fs::File::create(&partial).await.map_err(io_error)?;
        let mut stream = response.bytes_stream();
        let start = Instant::now();
        let mut last = Instant::now();
        let mut bytes = 0;
        loop {
            checkpoint(job).await?;
            let chunk = tokio::select! {
                c = stream.next() => c,
                _ = tokio::time::sleep(Duration::from_millis(200)) => { continue; }
            };
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk.map_err(|_| "Соединение прервано при скачивании.")?;
            file.write_all(&chunk).await.map_err(io_error)?;
            bytes += chunk.len() as u64;
            if last.elapsed() > Duration::from_millis(150) {
                let mut p = job.progress.lock().await;
                p.current_bytes = bytes;
                p.speed = bytes as f64 / start.elapsed().as_secs_f64().max(0.1);
                drop(p);
                if let Some(app) = app {
                    emit(app, job).await;
                }
                last = Instant::now();
            }
        }
        if bytes == 0 || total.is_some_and(|total| total != bytes) {
            return Err("Файл скачан не полностью.".into());
        }
        file.flush().await.map_err(io_error)?;
        file.sync_all().await.map_err(io_error)?;
        drop(file);
        replace(&partial, &target).await
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&partial).await;
    }
    result
}

async fn replace(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).await.map_err(io_error)
}

async fn write_json(root: &Path, relative: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| "Не удалось подготовить данные архива.")?;
    write_atomic(root, relative, &bytes).await
}

async fn write_atomic(root: &Path, relative: &Path, bytes: &[u8]) -> Result<(), String> {
    let target = checked_path(root, relative).await?;
    let partial = checked_path(root, &relative.with_extension("tmp")).await?;
    let mut file = fs::File::create(&partial).await.map_err(io_error)?;
    file.write_all(bytes).await.map_err(io_error)?;
    file.sync_all().await.map_err(io_error)?;
    drop(file);
    replace(&partial, &target).await
}

async fn save_manifest(root: &Path, manifest: &mut Manifest) -> Result<(), String> {
    manifest.updated_at = now();
    write_json(root, Path::new("manifest.json"), manifest).await
}

async fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut file = fs::File::open(path).await.map_err(io_error)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 65536];
    let mut bytes = 0;
    loop {
        let n = file.read(&mut buffer).await.map_err(io_error)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        bytes += n as u64;
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}

async fn valid_file(path: &Path, bytes: u64, hash: &str) -> bool {
    if fs::metadata(path)
        .await
        .map(|m| m.len() != bytes)
        .unwrap_or(true)
    {
        return false;
    }
    hash_file(path)
        .await
        .map(|(n, h)| n == bytes && h == hash)
        .unwrap_or(false)
}

pub fn scrub(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            obj.retain(|key, _| {
                ![
                    "access_token",
                    "refresh_token",
                    "id_token",
                    "access_key",
                    "player",
                ]
                .contains(&key.as_str())
            });
            for value in obj.values_mut() {
                scrub(value);
            }
        }
        Value::Array(array) => {
            for value in array {
                scrub(value);
            }
        }
        Value::String(s) if s.contains("access_token=") || s.contains("access_key=") => {
            *s = "[ссылка с ключом доступа скрыта]".into();
        }
        _ => (),
    }
}

fn html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn link(path: &str) -> String {
    path.split('/')
        .map(|p| {
            url::form_urlencoded::byte_serialize(p.as_bytes())
                .collect::<String>()
                .replace('+', "%20")
        })
        .collect::<Vec<_>>()
        .join("/")
}

async fn write_gallery(root: &Path, manifest: &Manifest) -> Result<(), String> {
    let mut rows = String::new();
    for entry in manifest.entries.values() {
        let title = html(&entry.title);
        let content =
            if let Some(path) = entry.path.as_ref().filter(|_| entry.status == "completed") {
                let _ = checked_path(root, Path::new(path)).await?;
                let href = link(path);
                let ext = Path::new(path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let preview = if ["jpg", "jpeg", "png", "webp"].contains(&ext.as_str()) {
                    format!("<img loading='lazy' src='{href}' alt='{title}'>")
                } else if ["mp4", "webm"].contains(&ext.as_str()) {
                    format!("<video controls preload='none' src='{href}'></video>")
                } else {
                    "<div class='file'>Файл</div>".into()
                };
                format!("{preview}<a href='{href}'>{title}</a>")
            } else {
                format!(
                    "<div class='file'>Не сохранено</div><strong>{title}</strong><p>{}</p>",
                    html(entry.error.as_deref().unwrap_or("Файл недоступен"))
                )
            };
        rows.push_str(&format!(
            "<article>{content}<small>{}</small></article>",
            html(&entry.collection)
        ));
    }
    let content = format!(
        r#"<!doctype html><html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src 'self' file:; media-src 'self' file:; style-src 'unsafe-inline'; base-uri 'none'"><title>Мой архив ВК</title><style>body{{font:16px system-ui;background:#f4f6fa;color:#182238;max-width:1200px;margin:60px auto;padding:0 24px}}h1{{font-size:42px}}.grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(260px,1fr));gap:20px}}article{{background:white;border-radius:14px;overflow:hidden;padding-bottom:18px}}img,video,.file{{width:100%;height:220px;object-fit:cover;background:#e7ecf4}}.file{{display:grid;place-items:center;color:#5d6a80}}a,strong,small,p{{display:block;margin:14px 18px;overflow-wrap:anywhere}}a{{color:#1768d5}}small{{color:#657187}}</style></head><body><h1>Мой архив ВК</h1><p>Аккаунт id{} · {} записей · Статус: {}. Все файлы находятся на этом компьютере.</p><div class="grid">{rows}</div></body></html>"#,
        manifest.account_id,
        manifest.entries.len(),
        html(&manifest.status)
    );
    write_atomic(root, Path::new("index.html"), content.as_bytes()).await
}

fn video_pages(media: &Media) -> Vec<String> {
    let access = api::text(&media.raw, "access_key");
    let mut page = format!("https://vk.com/video{}", media.id);
    if !access.is_empty() {
        page.push('_');
        page.push_str(&access);
    }
    let mut pages = vec![page];
    if let Some(player) = &media.player
        && !pages.contains(player)
    {
        pages.push(player.clone());
    }
    pages
}

fn ytdlp_error(message: &str) -> String {
    let message = message.to_lowercase();
    let detail = if message.contains("requested format is not available") {
        "ВК не предоставил готовый MP4 для этого видео."
    } else if message.contains("login")
        || message.contains("sign in")
        || message.contains("private")
    {
        "Видео требует отдельного входа на сайте ВК. Токен VK ID не является сессией видеоплеера."
    } else if message.contains("captcha") {
        "ВК запросил капчу для видеоплеера. Откройте видео в браузере."
    } else if message.contains("404")
        || message.contains("does not exist")
        || message.contains("removed")
    {
        "Видео удалено или недоступно по ссылке."
    } else if message.contains("403") || message.contains("access denied") {
        "ВК отказал в доступе к файлу видео."
    } else {
        "Не удалось получить видео. Обновите yt-dlp; для приватного видео может потребоваться отдельный вход."
    };
    // Extractor stderr can contain signed URLs. Only return a fixed explanation.
    format!("yt-dlp: {detail}")
}

fn ytdlp_command() -> Command {
    #[cfg(target_os = "macos")]
    let program = if Path::new("/opt/homebrew/bin/yt-dlp").is_file() {
        "/opt/homebrew/bin/yt-dlp"
    } else if Path::new("/usr/local/bin/yt-dlp").is_file() {
        "/usr/local/bin/yt-dlp"
    } else {
        "yt-dlp"
    };
    #[cfg(not(target_os = "macos"))]
    let program = "yt-dlp";
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}

async fn ytdlp(
    app: &tauri::AppHandle,
    job: &Job,
    root: &Path,
    relative: &Path,
    url: &str,
) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|_| "Некорректная ссылка видео.")?;
    let host = parsed.host_str().unwrap_or("");
    if !safe_remote(&parsed)
        || !["vk.com", "vk.ru", "vkvideo.ru"]
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return Err("yt-dlp разрешён только для видео ВК.".into());
    }
    let target = checked_path(root, relative).await?;
    let staging_relative = relative
        .parent()
        .unwrap_or(Path::new(""))
        .join(format!(".video-{}", uuid::Uuid::new_v4()));
    let staging = mkdir(root, &staging_relative).await?;
    let result: Result<(), String> = async {
        loop {
        checkpoint(job).await?;
        let mut child = ytdlp_command()
            .args(["--ignore-config", "--no-playlist", "--no-cache-dir", "--newline", "--no-warnings", "--socket-timeout", "20", "--retries", "2", "--format", "best[ext=mp4]", "--progress-template", "download:%(progress.downloaded_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s", "--output"])
            .arg(staging.join("video.mp4")).arg("--").arg(url)
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true)
            .spawn().map_err(|_| "yt-dlp не найден. Установите его и повторите бэкап.")?;
        let stderr = child.stderr.take().ok_or("Не удалось читать результат yt-dlp.")?;
        let errors = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut tail = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 8192 {
                    tail = tail.chars().rev().take(4096).collect::<String>().chars().rev().collect();
                }
            }
            tail
        });
        let mut lines = BufReader::new(child.stdout.take().ok_or("Не удалось читать прогресс yt-dlp.")?).lines();
        let mut restart = false;
        loop {
            if job.canceled.load(Ordering::Relaxed) { let _ = child.kill().await; errors.abort(); return Err("Бэкап отменён.".into()); }
            // Pausing a subprocess portably means stopping the current video and restarting
            // it after resume. yt-dlp resumes its partial download in the staging directory.
            if job.paused.load(Ordering::Relaxed) {
                let _ = child.kill().await;
                errors.abort();
                checkpoint(job).await?;
                restart = true;
                break;
            }
            let line = tokio::select! { l = lines.next_line() => l.map_err(|_| "Прерван вывод yt-dlp.")?, _ = tokio::time::sleep(Duration::from_millis(200)) => { continue; } };
            let Some(line) = line else { break; };
            let fields: Vec<_> = line.split('|').collect();
            if fields.len() == 3 {
                let mut p = job.progress.lock().await;
                p.current_bytes = fields[0].parse().unwrap_or(0); p.current_total = fields[1].parse().ok(); p.speed = fields[2].parse().unwrap_or(0.0);
                drop(p); emit(app, job).await;
            }
        }
        if restart { continue; }
        let success = child.wait().await.map_err(|_| "Не удалось дождаться yt-dlp.")?.success();
        let errors = errors.await.unwrap_or_default();
        if !success { return Err(ytdlp_error(&errors)); }
        let file = staging.join("video.mp4");
        if fs::metadata(&file).await.map_err(io_error)?.len() == 0 { return Err("yt-dlp вернул пустой файл.".into()); }
        return replace(&file, &target).await;
        }
    }.await;
    let _ = fs::remove_dir_all(&staging).await;
    result
}

#[tauri::command]
pub async fn tools_status() -> bool {
    tokio::time::timeout(
        Duration::from_secs(5),
        ytdlp_command()
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await
    .is_ok_and(|r| r.is_ok_and(|s| s.success()))
}

#[tauri::command]
pub async fn backup_status(state: State<'_, AppState>) -> Result<Option<Progress>, String> {
    let job = state.job.lock().await;
    if let Some(job) = job.as_ref() {
        Ok(Some(job.progress.lock().await.clone()))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn control_backup(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    action: String,
) -> Result<(), String> {
    let guard = state.job.lock().await;
    let job = guard.as_ref().ok_or("Нет активного бэкапа.")?;
    let mut p = job.progress.lock().await;
    if p.id != id {
        return Err("Этот бэкап уже не активен.".into());
    }
    if !matches!(p.status.as_str(), "running" | "paused") {
        return Err("Бэкап уже завершён или отменяется.".into());
    }
    match action.as_str() {
        "pause" => {
            job.paused.store(true, Ordering::Relaxed);
            p.status = "paused".into();
            p.speed = 0.0;
        }
        "resume" => {
            job.paused.store(false, Ordering::Relaxed);
            p.status = "running".into();
        }
        "cancel" => {
            job.canceled.store(true, Ordering::Relaxed);
            job.paused.store(false, Ordering::Relaxed);
            p.status = "canceling".into();
        }
        _ => return Err("Неизвестная команда.".into()),
    }
    drop(p);
    emit(&app, job).await;
    Ok(())
}

#[tauri::command]
pub async fn open_backup_folder(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let guard = state.job.lock().await;
    let job = guard.as_ref().ok_or("Бэкап ещё не создан.")?;
    let folder = job.progress.lock().await.folder.clone();
    app.opener()
        .open_path(folder, None::<&str>)
        .map_err(|_| "Не удалось открыть папку.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_extraction_prefers_normal_page_and_keeps_private_key() {
        let mut media: Media = serde_json::from_value(json!({
            "id":"123_456", "title":"Video", "downloadable":false
        }))
        .unwrap();
        media.player = Some("https://vk.com/video_ext.php?oid=123&id=456&hash=signed".into());
        media.raw = json!({"access_key":"private-key"});
        let pages = video_pages(&media);
        assert_eq!(pages[0], "https://vk.com/video123_456_private-key");
        assert_eq!(pages[1], media.player.clone().unwrap());
        assert!(
            !ytdlp_error("ERROR: HTTP Error 403 https://vk.com/?hash=signed").contains("signed")
        );
        assert!(ytdlp_error("ERROR: HTTP Error 403").contains("отказал"));
    }
    fn job() -> Job {
        Job {
            progress: tokio::sync::Mutex::new(Progress {
                id: "test".into(),
                status: "running".into(),
                phase: "downloading".into(),
                total: 1,
                completed: 0,
                skipped: 0,
                failed: 0,
                unavailable: 0,
                bytes: 0,
                current: String::new(),
                current_bytes: 0,
                current_total: None,
                speed: 0.0,
                folder: String::new(),
                errors: vec![],
                started_at: 0,
            }),
            paused: AtomicBool::new(false),
            canceled: AtomicBool::new(false),
        }
    }
    #[tokio::test]
    async fn download_commits_complete_files_and_removes_truncated_partial_files() {
        let root = std::env::temp_dir().join(format!("vk-download-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).await.unwrap();
        let (url, _) = crate::test_support::server(vec![
            ("image/jpeg".into(), b"complete-file".to_vec(), 13),
            ("image/jpeg".into(), b"truncated".to_vec(), 100),
            ("text/html".into(), b"login".to_vec(), 5),
        ])
        .await;
        let state = AppState::new();
        download(&state, None, &job(), &root, Path::new("good.jpg"), &url)
            .await
            .unwrap();
        assert_eq!(
            fs::read(root.join("good.jpg")).await.unwrap(),
            b"complete-file"
        );
        assert!(
            download(&state, None, &job(), &root, Path::new("bad.jpg"), &url)
                .await
                .is_err()
        );
        assert!(!root.join("bad.jpg").exists());
        assert!(!root.join("bad.jpg.part").exists());
        assert!(
            download(&state, None, &job(), &root, Path::new("login.jpg"), &url)
                .await
                .is_err()
        );
        assert!(!root.join("login.jpg").exists());
        fs::remove_dir_all(root).await.unwrap();
    }
    #[tokio::test]
    async fn canceling_a_paused_job_does_not_wait_for_resume() {
        let job = Arc::new(job());
        job.paused.store(true, Ordering::Relaxed);
        let pending = {
            let job = job.clone();
            tokio::spawn(async move { checkpoint(&job).await })
        };
        job.canceled.store(true, Ordering::Relaxed);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), pending)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
    #[test]
    fn gallery_paths_encode_spaces_without_form_encoding_plus_signs() {
        assert_eq!(link("My album/a + b.jpg"), "My%20album/a%20%2B%20b.jpg");
        assert_eq!(html("<script>\"&"), "&lt;script&gt;&quot;&amp;");
    }
    #[tokio::test]
    async fn pending_file_receipts_and_final_report_survive_a_restart() {
        let root = std::env::temp_dir().join(format!("vk-receipt-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).await.unwrap();
        let mut manifest = Manifest {
            version: 1,
            account_id: 123,
            updated_at: 0,
            status: "running".into(),
            warnings: vec![],
            entries: BTreeMap::from([(
                "123/file".into(),
                Entry {
                    title: "File".into(),
                    collection: "Album".into(),
                    path: Some("file.jpg".into()),
                    status: "pending".into(),
                    bytes: 0,
                    sha256: None,
                    error: None,
                },
            )]),
        };
        save_manifest(&root, &mut manifest).await.unwrap();
        fs::write(root.join("file.jpg"), b"received before crash")
            .await
            .unwrap();
        finalize(&root, 123, "canceled", vec!["interrupted".into()])
            .await
            .unwrap();
        let receipt: Manifest =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).await.unwrap()).unwrap();
        assert_eq!(
            receipt.entries["123/file"].path.as_deref(),
            Some("file.jpg")
        );
        assert_eq!(receipt.status, "canceled");
        assert_eq!(receipt.warnings, vec!["interrupted"]);
        let gallery = fs::read_to_string(root.join("index.html")).await.unwrap();
        assert!(!gallery.contains("src='file.jpg'"));
        fs::remove_dir_all(root).await.unwrap();
    }
    #[test]
    fn filenames_work_across_platforms() {
        assert_eq!(filename("../../CON"), "_.._CON");
        assert_eq!(filename("CON.txt"), "_CON.txt");
        assert!(!filename("a/b:c?d").contains('/'));
        assert!(filename(&"я".repeat(300)).len() <= 100);
    }
    #[test]
    fn secrets_are_removed_recursively() {
        let mut v = json!({"access_token":"secret","items":[{"access_key":"secret","player":"url","url":"https://x.vk.com?access_token=secret","text":"caption"}]});
        scrub(&mut v);
        assert!(!v.to_string().contains("secret"));
        assert_eq!(v["items"][0]["text"], "caption");
    }
    #[test]
    fn reject_local_and_non_https_downloads() {
        for u in [
            "http://vk.com/a",
            "https://127.0.0.1/a",
            "https://localhost/a",
            "https://u:p@vk.com/a",
            "https://test.local/a",
        ] {
            assert!(!safe_remote(&Url::parse(u).unwrap()));
        }
        assert!(safe_remote(
            &Url::parse("https://sun1.userapi.com/a").unwrap()
        ));
    }
    #[tokio::test]
    async fn validate_completed_file_and_detect_corruption() {
        let path = std::env::temp_dir().join(format!("vk-test-{}", uuid::Uuid::new_v4()));
        fs::write(&path, b"photo bytes").await.unwrap();
        let (bytes, hash) = hash_file(&path).await.unwrap();
        assert!(valid_file(&path, bytes, &hash).await);
        fs::write(&path, b"wrong bytes").await.unwrap();
        assert!(!valid_file(&path, bytes, &hash).await);
        fs::remove_file(path).await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn archive_refuses_symlink_and_path_traversal() {
        let root = std::env::temp_dir().join(format!("vk-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).await.unwrap();
        std::os::unix::fs::symlink(std::env::temp_dir(), root.join("linked")).unwrap();
        assert!(checked_path(&root, Path::new("linked/file")).await.is_err());
        assert!(checked_path(&root, Path::new("../outside")).await.is_err());
        fs::remove_dir_all(root).await.unwrap();
    }
}
