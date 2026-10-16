//! Embed John's Playground badge as the Windows exe's file icon.
//! Best-effort: if the resource compiler isn't available the build
//! still succeeds, the exe just keeps the default file icon (the
//! window/taskbar icon comes from the app itself either way).
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=app icon embedding failed: {}", e);
        }
    }
}
