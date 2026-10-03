//! The router (Phase 0 R13; Phase 1 O2/TN4 build on it): the single place a [`Route`]
//! maps to its view constructor and its view-state, and the single place each route's
//! dismiss policy is recorded. Four stub views (capture/browser/onboarding/settings)
//! register here now, each in its own file, so the four agents who port them for real
//! in Phase 1 can touch only their own `views/<name>.rs` without fighting over a shared
//! registration point (the reason this task exists ahead of the views themselves).
//!
//! [`TABLE`] is the router: a fixed array of [`RouteEntry`], one per [`Route`] variant,
//! each holding a `build` constructor (type-erased to [`AnyView`] so `panel.rs` can
//! hold one child across every route without matching on the concrete view type), a
//! `state` accessor for Task 6's automation `state` verb, and the [`Dismiss`] policy
//! R5 (Task 5) needs for the click-away handler. `Route::hides_on_deactivate` reads
//! this table instead of its own match arm, so the shell's click-away handler
//! (`shell/panel.rs`) and the router agree by construction, not by two copies of the
//! same rule staying in sync.
//!
//! [`ViewState`] is `#[serde(untagged)]` over each view's own small state struct, so
//! the JSON the `state` verb returns is the stub's own flat object (e.g.
//! `{ "view": "capture", "phase": "stub" }`), not a tagged wrapper around it.
//!
//! Task 7 (Phase 0 R7, TN5): a fifth route, `Route::A11ySpike`, exists only in
//! automation builds. It is not one of the four product views Phase 1 ports — it is
//! a throwaway probe view proving gpui-kit's list/input/switch/dialog widgets reach
//! AccessKit with their visible text, so Phase 1's own views can copy a proven
//! pattern instead of discovering one live. Every place that variant touches (the
//! enum, `TABLE`, `ViewState`) is gated the same way, so a release build — and
//! `router_registers_four_views` run without `--features automation` — never see it.

#[cfg(feature = "automation")]
pub mod a11y_spike;
pub mod browser;
pub mod capture;
pub mod onboarding;
pub mod settings;

use gpui_kit::{AnyView, App};
use serde::{Deserialize, Serialize};

/// Every route the panel can show. TN4's event pump and `shell/panel.rs` deal only in
/// this enum, never a concrete view type, so the shell stays GPUI-view-agnostic and
/// Phase 1 can swap a stub for its real view without touching the shell at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Capture,
    Browser,
    Onboarding,
    Settings,
    /// Task 7's accessibility spike (R7, TN5): automation builds only. Opened by the
    /// `a11y-spike` probe through `open_view`, never reachable from the tray or a
    /// hotkey, and absent entirely from a release binary.
    #[cfg(feature = "automation")]
    A11ySpike,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Capture => "capture",
            Route::Browser => "browser",
            Route::Onboarding => "onboarding",
            Route::Settings => "settings",
            #[cfg(feature = "automation")]
            Route::A11ySpike => "a11y_spike",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Route::Capture => "Capture",
            Route::Browser => "Browse links",
            Route::Onboarding => "Welcome to ubilink",
            Route::Settings => "Settings",
            #[cfg(feature = "automation")]
            Route::A11ySpike => "Accessibility spike",
        }
    }

    /// R5 (Task 5): capture and browser hide on loss of key status; settings and
    /// onboarding are places the user works in (and may switch apps from, to grant a
    /// permission), so they stay open. This asks `TABLE` for the answer instead of
    /// matching a second time, so there is exactly one place this policy is decided.
    pub fn hides_on_deactivate(self) -> bool {
        entry_for(self).dismiss == Dismiss::HideOnDeactivate
    }
}

/// Whether a route's panel closes when the window loses key status (R5, Task 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dismiss {
    /// Click-away closes the panel: capture and browser are glanceable actions, not
    /// places the user lingers.
    HideOnDeactivate,
    /// Click-away leaves the panel open: onboarding and settings are places the user
    /// works in, and may switch apps from (e.g. to grant a permission) without meaning
    /// to dismiss the panel.
    StayOpen,
}

/// One row of the router table: a route, its dismiss policy, its view constructor and
/// its view-state accessor.
pub struct RouteEntry {
    pub route: Route,
    pub dismiss: Dismiss,
    /// Builds the stub view as a type-erased [`AnyView`] — what `shell/panel.rs` asks
    /// for instead of matching on `Route` itself.
    pub build: fn(&mut App) -> AnyView,
    /// The view-state Task 6's automation `state` verb serialises for this route.
    /// Only that verb reads it (and `state_for`/`ViewState::to_json` below), so in a
    /// release build — which has no automation module (S1) — rustc's dead-code
    /// analysis rightly sees no reader; the allow is scoped to that build.
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    pub state: fn() -> ViewState,
}

/// The router: one row per [`Route`] variant. `router_registers_four_views` proves
/// every product route is here exactly once, in both feature configurations.
#[cfg(not(feature = "automation"))]
pub const TABLE: [RouteEntry; 4] = [
    RouteEntry {
        route: Route::Capture,
        dismiss: Dismiss::HideOnDeactivate,
        build: capture::build,
        state: capture::state,
    },
    RouteEntry {
        route: Route::Browser,
        dismiss: Dismiss::HideOnDeactivate,
        build: browser::build,
        state: browser::state,
    },
    RouteEntry {
        route: Route::Onboarding,
        dismiss: Dismiss::StayOpen,
        build: onboarding::build,
        state: onboarding::state,
    },
    RouteEntry {
        route: Route::Settings,
        dismiss: Dismiss::StayOpen,
        build: settings::build,
        state: settings::state,
    },
];

/// Automation builds carry a fifth row after the four product routes (Task 7, R7):
/// `Route::A11ySpike`, reachable only through the automation `open_view` verb. It
/// stays open on deactivate like onboarding/settings — a probe activating the a11y
/// tree should not fight a click-away dismissal.
#[cfg(feature = "automation")]
pub const TABLE: [RouteEntry; 5] = [
    RouteEntry {
        route: Route::Capture,
        dismiss: Dismiss::HideOnDeactivate,
        build: capture::build,
        state: capture::state,
    },
    RouteEntry {
        route: Route::Browser,
        dismiss: Dismiss::HideOnDeactivate,
        build: browser::build,
        state: browser::state,
    },
    RouteEntry {
        route: Route::Onboarding,
        dismiss: Dismiss::StayOpen,
        build: onboarding::build,
        state: onboarding::state,
    },
    RouteEntry {
        route: Route::Settings,
        dismiss: Dismiss::StayOpen,
        build: settings::build,
        state: settings::state,
    },
    RouteEntry {
        route: Route::A11ySpike,
        dismiss: Dismiss::StayOpen,
        build: a11y_spike::build,
        state: a11y_spike::state,
    },
];

/// The router table's row for `route`. Panics if `TABLE` is ever missing a variant —
/// a compile-time-checkable invariant this task's test proves holds, so that panic
/// should never fire outside a broken edit to `TABLE` itself.
pub fn entry_for(route: Route) -> &'static RouteEntry {
    TABLE
        .iter()
        .find(|e| e.route == route)
        .unwrap_or_else(|| panic!("router table (TABLE) has no entry for {route:?}"))
}

/// Build `route`'s stub view, type-erased to [`AnyView`]. `shell/panel.rs` calls this
/// once when the panel opens and again on every route switch.
pub fn build_for(route: Route, cx: &mut App) -> AnyView {
    (entry_for(route).build)(cx)
}

/// `route`'s current view-state, for Task 6's automation `state` verb.
#[cfg_attr(not(feature = "automation"), allow(dead_code))]
pub fn state_for(route: Route) -> ViewState {
    (entry_for(route).state)()
}

/// Every stub view's state, flattened so the serialised JSON is the view's own object
/// (`{ "view": ..., "phase": ... }`) rather than a tagged wrapper around it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ViewState {
    Capture(capture::State),
    Browser(browser::State),
    Onboarding(onboarding::State),
    Settings(settings::State),
    #[cfg(feature = "automation")]
    A11ySpike(a11y_spike::State),
}

impl ViewState {
    /// Serialise to the JSON Task 6's `state` verb returns for the live route.
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("every stub view-state serialises to JSON")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// RED/GREEN anchor for Task 13. Run with:
    /// `cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation router_registers_four_views`
    ///
    /// Task 7 adds a fifth, automation-only row (`Route::A11ySpike`) to `TABLE`. This
    /// test's intent — exactly the four product routes are registered for product
    /// use — must keep holding in both feature configurations, so it filters `TABLE`
    /// down to the product routes before counting rather than asserting the table's
    /// raw length (which is 4 without `automation`, 5 with it).
    #[test]
    fn router_registers_four_views() {
        const PRODUCT_ROUTES: [Route; 4] = [
            Route::Capture,
            Route::Browser,
            Route::Onboarding,
            Route::Settings,
        ];

        // Exactly one entry per product route: walk `TABLE` itself, not a separately
        // declared list, so a future edit that drops or duplicates a route fails here.
        let routes: Vec<Route> = TABLE.iter().map(|e| e.route).collect();
        let product_rows: Vec<Route> = routes
            .iter()
            .copied()
            .filter(|r| PRODUCT_ROUTES.contains(r))
            .collect();
        assert_eq!(
            product_rows.len(),
            4,
            "router table must register exactly four product views"
        );
        let unique: HashSet<&str> = product_rows.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            unique.len(),
            4,
            "every product route must be registered exactly once"
        );
        for expected in PRODUCT_ROUTES {
            assert!(
                routes.contains(&expected),
                "{expected:?} missing from the router table"
            );
        }

        // Each view-state serialises to JSON carrying its own route name (every row
        // in `TABLE`, including Task 7's automation-only spike row when present).
        for entry in TABLE.iter() {
            let json = (entry.state)().to_json();
            assert_eq!(
                json["view"],
                entry.route.as_str(),
                "view-state must name its own route"
            );
            assert_eq!(json["phase"], "stub");
        }

        // Dismiss policy exactly as R5/Task 5 specifies, read back through the table.
        assert_eq!(entry_for(Route::Capture).dismiss, Dismiss::HideOnDeactivate);
        assert_eq!(entry_for(Route::Browser).dismiss, Dismiss::HideOnDeactivate);
        assert_eq!(entry_for(Route::Onboarding).dismiss, Dismiss::StayOpen);
        assert_eq!(entry_for(Route::Settings).dismiss, Dismiss::StayOpen);
        assert!(Route::Capture.hides_on_deactivate());
        assert!(Route::Browser.hides_on_deactivate());
        assert!(!Route::Onboarding.hides_on_deactivate());
        assert!(!Route::Settings.hides_on_deactivate());
    }

    /// Task 7's fifth row exists, but only in automation builds, and does not count
    /// as one of the four product routes `router_registers_four_views` pins.
    #[cfg(feature = "automation")]
    #[test]
    fn router_also_registers_the_automation_only_a11y_spike_route() {
        assert_eq!(TABLE.len(), 5, "automation builds carry the spike row too");
        let entry = entry_for(Route::A11ySpike);
        assert_eq!(entry.dismiss, Dismiss::StayOpen);
        assert_eq!(Route::A11ySpike.as_str(), "a11y_spike");
        let json = (entry.state)().to_json();
        assert_eq!(json["view"], "a11y_spike");
        assert_eq!(json["phase"], "stub");
    }
}
