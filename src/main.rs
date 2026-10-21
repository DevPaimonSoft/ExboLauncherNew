#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod api;
mod app;
mod download;
mod ipc;
mod launch;
mod model;
mod optimization;
mod platform;
mod protocol;
mod seeding;
mod storage;
mod support;
#[cfg(test)]
mod tests;
mod updater;

slint::include_modules!();

fn main() {
    if let Err(e) = app::run() {
        eprintln!("EXBO Launcher: {e:#}");
        rfd::MessageDialog::new()
            .set_title("EXBO Launcher New")
            .set_description(format!("{e:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}
