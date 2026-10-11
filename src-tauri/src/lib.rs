mod api;
mod auth;
mod backup;
mod model;
#[cfg(test)]
mod test_support;

use model::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            auth::connect_token,
            auth::begin_login,
            auth::cancel_login,
            auth::logout,
            api::sources,
            api::catalog,
            api::collection_items,
            backup::start_backup,
            backup::backup_status,
            backup::control_backup,
            backup::open_backup_folder,
            backup::tools_status
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start VK Backup");
}
