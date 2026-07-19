use unimail_core::ApplicationInfo;

#[tauri::command]
fn application_info() -> ApplicationInfo {
    ApplicationInfo::current()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the Tauri desktop process and installs the approved IPC commands.
///
/// # Panics
///
/// Panics when Tauri cannot initialize or run the application event loop.
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![application_info])
        .run(tauri::generate_context!())
        .expect("error while running Unimail");
}
