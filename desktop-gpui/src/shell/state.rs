//! `ShellState`: the shell's observable state, serialisable so Task 6's automation
//! `state` verb can return it and `probe.py run shell` (R4's acceptance criterion) can
//! assert on it: tray_present, three hotkeys registered, window kind popup, background
//! blurred, activation accessory.
//!
//! Every field is filled from the real objects: the tray from tray-icon's live
//! NSStatusItem, the hotkeys from each `register` result, the activation policy read
//! back from `NSApp.activationPolicy`, and the window kind and background from a live
//! panel's NSWindow (see `native::inspect_panel`). Until a panel has been opened and
//! inspected, `window_kind` and `background` are "unknown" and `window_verified` is
//! false: the shell never reports what it merely asked GPUI for. Once inspected, they
//! keep the last observed values after the panel hides, and `window_verified` stays
//! true.

use objc2_app_kit::NSApplicationActivationPolicy;
use serde::{Deserialize, Serialize};

use super::native::PanelFacts;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShellState {
    /// The NSStatusItem exists and is visible.
    pub tray_present: bool,
    /// Display names ("Cmd+Shift+K", …) of the hotkeys that registered successfully.
    pub hotkeys_registered: Vec<String>,
    /// `UBILINK_SKIP_HOTKEYS=1`: registration was deliberately not attempted, so an
    /// automation instance never grabs the user's hotkeys.
    pub hotkeys_skipped: bool,
    /// Every registration failure, "name: error; …" (the Tauri shell's lesson: a dead
    /// hotkey must be visible, not just logged where a packaged app has no terminal).
    pub hotkey_error: Option<String>,
    /// "popup" for a `WindowKind::PopUp` NSPanel at NSPopUpWindowLevel.
    pub window_kind: String,
    /// "blurred" when the panel carries an NSVisualEffectView behind GPUI's content.
    pub background: String,
    /// True once `window_kind`/`background` were read back from a live panel.
    pub window_verified: bool,
    /// `NSApp.activationPolicy`, read back: "accessory", "regular" or "prohibited".
    pub activation_policy: String,
    pub visible: bool,
    /// The panel's route ("capture", "browser", "settings", "onboarding").
    pub route: Option<String>,
    /// CGDirectDisplayID of the display the panel last opened on.
    pub display_id: Option<u32>,
}

impl Default for ShellState {
    fn default() -> Self {
        ShellState {
            tray_present: false,
            hotkeys_registered: Vec::new(),
            hotkeys_skipped: false,
            hotkey_error: None,
            window_kind: "unknown".into(),
            background: "unknown".into(),
            window_verified: false,
            // "unknown" until read back, so a state that never queried AppKit can't pass
            // for accessory.
            activation_policy: "unknown".into(),
            visible: false,
            route: None,
            display_id: None,
        }
    }
}

/// The name `ShellState::activation_policy` reports for an AppKit policy value.
pub fn policy_name(policy: NSApplicationActivationPolicy) -> &'static str {
    if policy == NSApplicationActivationPolicy::Accessory {
        "accessory"
    } else if policy == NSApplicationActivationPolicy::Regular {
        "regular"
    } else if policy == NSApplicationActivationPolicy::Prohibited {
        "prohibited"
    } else {
        "unknown"
    }
}

impl ShellState {
    /// Record a policy value read back from `NSApp.activationPolicy`.
    pub fn record_policy(&mut self, policy: NSApplicationActivationPolicy) {
        self.activation_policy = policy_name(policy).into();
    }

    /// Record what AppKit reports for a live panel.
    pub fn record_panel(&mut self, facts: PanelFacts) {
        self.window_kind = if facts.popup { "popup" } else { "other" }.into();
        self.background = if facts.blurred { "blurred" } else { "opaque" }.into();
        self.window_verified = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_state_reports_real_policy() {
        // Starts as "unknown", not "accessory": only a read-back can claim a policy.
        let mut s = ShellState::default();
        assert_eq!(s.activation_policy, "unknown");
        s.record_policy(NSApplicationActivationPolicy::Accessory);
        assert_eq!(s.activation_policy, "accessory");
        // GPUI's did_finish_launching forces Regular; that must show up as regular,
        // so a lost re-assert would fail the shell probe instead of hiding.
        s.record_policy(NSApplicationActivationPolicy::Regular);
        assert_eq!(s.activation_policy, "regular");
        s.record_policy(NSApplicationActivationPolicy::Prohibited);
        assert_eq!(s.activation_policy, "prohibited");
        s.record_policy(NSApplicationActivationPolicy(42));
        assert_eq!(s.activation_policy, "unknown");
    }

    #[test]
    fn window_fields_are_unknown_until_a_live_panel_is_recorded() {
        let mut s = ShellState::default();
        assert_eq!(
            (s.window_kind.as_str(), s.background.as_str()),
            ("unknown", "unknown")
        );
        assert!(!s.window_verified);
        s.record_panel(PanelFacts {
            popup: true,
            blurred: true,
        });
        assert_eq!(
            (s.window_kind.as_str(), s.background.as_str()),
            ("popup", "blurred")
        );
        assert!(s.window_verified);
        // A panel AppKit built differently must not pass for popup/blurred.
        s.record_panel(PanelFacts {
            popup: false,
            blurred: false,
        });
        assert_eq!(
            (s.window_kind.as_str(), s.background.as_str()),
            ("other", "opaque")
        );
    }

    #[test]
    fn shell_state_serialises_with_the_fields_the_probe_reads() {
        let s = ShellState {
            tray_present: true,
            hotkeys_registered: vec![
                "Cmd+Shift+K".into(),
                "Cmd+Shift+L".into(),
                "Cmd+Shift+,".into(),
            ],
            hotkeys_skipped: false,
            hotkey_error: None,
            window_kind: "popup".into(),
            background: "blurred".into(),
            window_verified: true,
            activation_policy: "accessory".into(),
            visible: true,
            route: Some("capture".into()),
            display_id: Some(1),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["tray_present"], true);
        assert_eq!(v["hotkeys_registered"].as_array().unwrap().len(), 3);
        assert_eq!(v["hotkey_error"], serde_json::Value::Null);
        assert_eq!(v["window_kind"], "popup");
        assert_eq!(v["background"], "blurred");
        assert_eq!(v["activation_policy"], "accessory");
        assert_eq!(v["route"], "capture");
        assert_eq!(v["display_id"], 1);
        let back: ShellState = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }
}
