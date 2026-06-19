// Satisfies: RT-5 (binding), RT-6, S1 (invariant), T1 (boundary), TN1 (capability
// broker), TN6 (isolation proof), SC-3 (revocable grants).
// See .manifold/linking-manifesto-app.md
//
// hooky-core — the trusted core's plugin-sandbox layer.
//
// Threat model (S1 / RT-5): third-party plugin code is UNTRUSTED. It must run
// outside the core's trust domain (T1), with NO ambient network / filesystem /
// process / link-graph access. The ONLY thing a plugin may do is ask the broker
// for a capability it declared in its manifest (RT-6); the broker checks the
// grant and either performs the mediated action or denies (deny-by-default).
//
// Two layers enforce this, belt-and-suspenders:
//   1. OS/runtime isolation (sandbox.rs): the plugin host is a SEPARATE Node
//      process launched with the Node Permission Model (`node --permission`) and
//      NO --allow-fs*/--allow-child-process/--allow-worker flags. fs, child_process
//      and worker_threads throw ERR_ACCESS_DENIED at the runtime level. The plugin
//      source is streamed in over the pipe and loaded via a `data:` URL import, so
//      the host needs zero filesystem allowances of its own. (Solution Option B.)
//   2. Capability brokering (broker.rs): every privileged action a plugin can take
//      is a capability request over the IPC pipe, checked against the granted
//      manifest. There is NO broker handler that returns the link graph wholesale —
//      the only read handler exposes the single focused resource the plugin was
//      invoked for (S1).
//
// Honest limitation (verified empirically, see plugins/README.md + DECISIONS):
// the Node Permission Model in v24 does NOT restrict outbound network (fetch
// succeeds under --permission). Net is therefore NOT sandboxed by this mechanism
// alone; OS-level hardening (macOS sandbox-exec / Linux seccomp) is the future
// path. We do not claim net is contained. fs + child_process + worker ARE
// provably denied, and the broker denies every ungranted capability.

pub mod adapter;
pub mod api;
pub mod broker;
pub mod capabilities;
pub mod graph;
pub mod integrations;
pub mod adapters_config;
pub mod live_capture;
pub mod panel;
pub mod release;
pub mod resource;
pub mod sandbox;
pub mod service;

pub use adapter::{capture_active, CaptureFailure, FileAdapter, OpenPlan, PermissionState, ResourceAdapter};
pub use api::{ApiError, LinkApi, MAX_INPUT_BYTES};
pub use broker::{Broker, Decision, FocusedResource, Manifest};
pub use integrations::{Installer, InstallError, SignedBundle, sign_bundle};
pub use live_capture::{CommandRunner, OsaScriptRunner, probe_automation_permission, FRONTMOST_APP_SCRIPT};
pub use adapters_config::{AdapterRegistry, AdapterSpec, ConfiguredAdapter};
pub use release::{public_hex, sign_to_json, signing_key_from_hex, verify_json, SignError, WireBundle};
pub use panel::{LinkView, PanelBackend, PanelView};
pub use capabilities::{Capability, CapabilityMatrix, LinkMechanism, Platform, Support};
pub use graph::LinkGraph;
pub use resource::{Link, MatchConfidence, ResolutionAction, ResourceIdentity, reresolve};
pub use sandbox::{Session, EscapeAttempt};
pub use service::{HookyService, ServiceError};
