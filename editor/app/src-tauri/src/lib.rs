mod children;
mod files;
// `gen` is a reserved keyword in Rust 2024, so the module spells out what it drives.
mod generation;
mod project;
mod pty;
mod stage;
mod wire;

use std::time::Duration;

use tauri::Manager as _;

/// How long app exit waits for the terminals' pseudoconsoles to close. Bounded: a hang on the way
/// out would be worse than the leak it prevents.
const PTY_EXIT_GRACE: Duration = Duration::from_secs(5);

/// How long app exit waits for the one-shot pipeline children's process trees to be killed.
const CHILD_EXIT_GRACE: Duration = Duration::from_secs(5);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            app.manage(pty::registry(app.handle().clone()));
            app.manage(generation::GenTickets::default());
            app.manage(stage::stages(app.handle().clone(), pty::repo_root()?));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            pty::pty_list,
            pty::write_editor_context,
            pty::read_editor_file,
            pty::stat_editor_file,
            pty::write_editor_file,
            pty::editor_paths,
            project::project_info,
            project::add_stage_scenes,
            files::list_editor_dir,
            stage::stage_start,
            stage::stage_status,
            stage::stage_restart,
            stage::stage_stop,
            stage::run_check_scene,
            generation::gen_invoke,
            generation::gen_cancel,
            generation::read_editor_image,
            generation::editor_file_size,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        // The stage sessions, the terminals and the one-shot pipeline children (`gen`,
        // `check-scene`, `sync`) are owned OS processes; app exit must take them down. `Exit` fires
        // after the last window closes, and all three run inline (the stages stop concurrently and
        // return once every child is reaped; the terminals and the children are bounded by their
        // own grace) because the process is on its way out — a detached thread would be raced by
        // it. The pty half matters as much as the stages': Windows neither kills a child
        // with its parent nor closes a pseudoconsole for a process that forgot to, so exiting
        // without this strands every open terminal's `claude` plus a core-spinning `conhost.exe`.
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let stages = app.state::<stage::AppStages>();
                stages.stop_all();
                stages.children.kill_all(CHILD_EXIT_GRACE);
                app.state::<pty::AppPtyRegistry>().shutdown(PTY_EXIT_GRACE);
            }
        });
}
