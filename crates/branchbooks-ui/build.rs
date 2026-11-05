//! The Windows app's icon: `icons/icon.ico` goes into the exe as resource 32512, the id Explorer and
//! the taskbar show and Dioxus loads for the window, so the app needs no icon code.
fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var_os("CARGO_FEATURE_DESKTOP").is_some()
        && let Err(e) = winresource::WindowsResource::new()
            .set_icon_with_id("icons/icon.ico", "32512")
            .compile()
    {
        panic!("embedding icons/icon.ico: {e}");
    }
}
