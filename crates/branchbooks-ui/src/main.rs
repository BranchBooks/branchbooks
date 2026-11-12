// A release build on Windows opens no console window beside the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

#[cfg(feature = "desktop")]
fn main() {
    use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
    branchbooks_ui::keep_crash_log();
    branchbooks_ui::store::move_out_of_roaming();
    let window = WindowBuilder::new()
        .with_title("BranchBooks")
        .with_inner_size(LogicalSize::new(1200.0, 800.0));
    dioxus::LaunchBuilder::new()
        .with_cfg(Config::new().with_window(window).with_menu(None))
        .launch(branchbooks_ui::App);
}

#[cfg(not(feature = "desktop"))]
fn main() {
    dioxus::launch(branchbooks_ui::App);
}
