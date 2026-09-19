use serde::Serialize;

#[derive(Serialize)]
struct SystemInfo {
    platform: String,
    arch: String,
}

#[tauri::command]
fn system_info() -> SystemInfo {
    SystemInfo {
        platform: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![system_info])
        .run(tauri::generate_context!())
        .expect("run OpenForge desktop");
}
