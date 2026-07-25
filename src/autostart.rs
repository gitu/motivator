//! Start-on-login entry: XDG autostart on Linux, login item on macOS, HKCU
//! Run key on Windows. The system entry is the source of truth — nothing is
//! stored in the config, so the checkbox can never drift from reality.

use auto_launch::{AutoLaunch, AutoLaunchBuilder};

fn launcher() -> Option<AutoLaunch> {
    let exe = std::env::current_exe().ok()?;
    AutoLaunchBuilder::new()
        .set_app_name("motivator")
        .set_app_path(&exe.to_string_lossy())
        .build()
        .ok()
}

/// does an autostart entry currently exist on this system?
pub fn is_enabled() -> bool {
    launcher()
        .and_then(|l| l.is_enabled().ok())
        .unwrap_or(false)
}

/// create or remove the entry; Err carries a short user-facing message
pub fn set_enabled(on: bool) -> Result<(), String> {
    let launcher = launcher().ok_or("can't locate the motivator binary")?;
    let result = if on {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| format!("autostart change failed: {e}"))
}
