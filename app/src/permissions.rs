//! The two macOS permission checks (S4/RT-2) and the Settings-pane opener.
//! Moved verbatim: `AXIsProcessTrusted`/`AXIsProcessTrustedWithOptions` are direct
//! macOS framework FFI (ApplicationServices/CoreFoundation), not Tauri — same for
//! `open <url>` below. None of this needs an `AppHandle`.

use serde::Serialize;
use ubilink_core::{OsaScriptRunner, PermissionState};

/// The two macOS permissions ubilink needs, and their current state — so the
/// onboarding screen can tell the user exactly what's left to do (S4/RT-2/U3).
#[derive(Serialize, Clone)]
pub struct PermissionReport {
    /// Accessibility (AXIsProcessTrusted) — lets Ubilink script another app's
    /// menus (AX-based adapters, e.g. Bear's "Copy Link"). Defect 5: this is
    /// NOT required for the global hotkey, which registers via Carbon
    /// `RegisterEventHotKey` (`desktop-gpui/src/shell/hotkeys.rs`, via global-hotkey)
    /// and needs no TCC grant at all — nor for capture from browsers/Finder,
    /// which goes through osascript (Automation) instead.
    pub accessibility: bool,
    /// Automation (System Events, via osascript): "granted" | "denied" | "not_determined".
    pub automation: &'static str,
}

/// Whether this process is trusted for the macOS Accessibility API — needed for
/// AX-based menu-scripting adapters (e.g. Bear), NOT the global hotkey (Defect
/// 5: Carbon `RegisterEventHotKey` needs no TCC grant). Direct FFI — no crate,
/// keeping the dep tree honest.
#[cfg(target_os = "macos")]
pub fn accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }
    unsafe { AXIsProcessTrusted() != 0 }
}
#[cfg(not(target_os = "macos"))]
pub fn accessibility_trusted() -> bool {
    false
}

/// Trigger the macOS "grant Accessibility" system dialog
/// (`AXIsProcessTrustedWithOptions` with the prompt option) and return the current
/// trust state. Needed for UI-scripting app menus (e.g. Bear's "Copy Link"). Builds
/// the options CFDictionary by hand — dep-free CoreFoundation FFI.
#[cfg(target_os = "macos")]
pub fn prompt_accessibility() -> bool {
    use std::ffi::c_void;
    use std::ptr;
    type CFRef = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFRef;
        static kCFTypeDictionaryKeyCallBacks: [u8; 0];
        static kCFTypeDictionaryValueCallBacks: [u8; 0];
        fn CFDictionaryCreate(
            allocator: CFRef,
            keys: *const CFRef,
            values: *const CFRef,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFRef;
        fn CFRelease(cf: CFRef);
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        static kAXTrustedCheckOptionPrompt: CFRef;
        fn AXIsProcessTrustedWithOptions(options: CFRef) -> u8;
    }
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            ptr::addr_of!(kCFTypeDictionaryKeyCallBacks) as *const c_void,
            ptr::addr_of!(kCFTypeDictionaryValueCallBacks) as *const c_void,
        );
        let trusted = AXIsProcessTrustedWithOptions(options) != 0;
        if !options.is_null() {
            CFRelease(options);
        }
        trusted
    }
}
#[cfg(not(target_os = "macos"))]
pub fn prompt_accessibility() -> bool {
    false
}

/// Show the macOS Accessibility prompt and report whether trust is now granted.
pub fn request_accessibility() -> bool {
    prompt_accessibility()
}

/// Report current permission state (called by the onboarding screen + its Re-check).
pub fn permission_status() -> PermissionReport {
    let automation = match ubilink_core::probe_automation_permission(&OsaScriptRunner) {
        PermissionState::Granted => "granted",
        PermissionState::Denied => "denied",
        PermissionState::NotDetermined => "not_determined",
    };
    PermissionReport {
        accessibility: accessibility_trusted(),
        automation,
    }
}

/// The System Settings URL for a privacy pane ("accessibility" or "automation").
pub fn settings_pane_url(pane: &str) -> Result<&'static str, String> {
    match pane {
        "accessibility" => {
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        }
        "automation" => {
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
        }
        other => Err(format!("unknown settings pane: {other}")),
    }
}

/// `open_settings` with the launcher injected (GPUI Phase 0 R6/O4): the GPUI shell's
/// automation builds record the pane URL in their opener sink instead of opening
/// System Settings. Runs `open <url>`, the same command `open_settings` spawns.
pub fn open_settings_with<R: ubilink_core::CommandRunner>(
    runner: &R,
    pane: &str,
) -> Result<(), String> {
    let url = settings_pane_url(pane)?;
    runner.run("open", &[url]).map(|_| ())
}

/// Open the relevant System Settings privacy pane so the user can grant a permission.
pub fn open_settings(pane: &str) -> Result<(), String> {
    let url = settings_pane_url(pane)?;
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map_err(|e| format!("couldn't open System Settings: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Record(RefCell<Vec<String>>);
    impl ubilink_core::CommandRunner for Record {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            self.0
                .borrow_mut()
                .push(format!("{program} {}", args.join(" ")));
            Ok(String::new())
        }
    }

    #[test]
    fn open_settings_with_runs_open_on_the_pane_url_through_the_runner() {
        let r = Record(RefCell::new(Vec::new()));
        open_settings_with(&r, "automation").unwrap();
        assert_eq!(
            *r.0.borrow(),
            ["open x-apple.systempreferences:com.apple.preference.security?Privacy_Automation"]
        );
        assert_eq!(
            open_settings_with(&r, "camera"),
            Err("unknown settings pane: camera".into())
        );
        assert_eq!(r.0.borrow().len(), 1, "an unknown pane runs nothing");
    }
}
