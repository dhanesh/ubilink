//! The opener seam (spec O4: "opens go to a sink"; Phase 1 S2: "the GPUI shell adds no
//! opener of its own, and automation builds route opens to the opener sink").
//!
//! Every URL or System Settings pane the app opens goes through a
//! `ubilink_core::CommandRunner` — core's own I/O seam, which `Backend::open_link`
//! already takes so it can re-validate the URI first (D16), and which ubilink-app's
//! `open_link_with` / `open_settings_with` take too. The shell wraps whatever real
//! runner it was given (ubilink-app's `SpawnRunner`) in [`Opener`]:
//!
//! - release build: the wrapped runner runs, unchanged — `Opener` adds no behaviour;
//! - `automation` build: the command is recorded in the sink the `opened` verb reads
//!   (R6) and nothing is launched, so a probe never opens a browser tab or System
//!   Settings on the host.

use ubilink_core::CommandRunner;

/// Wraps the real runner; see the module doc for what each build does with it.
// Phase 1's views are the callers (open link, open settings pane); R6 wires the seam
// now, so until then rustc sees no use outside tests.
#[allow(dead_code)]
pub struct Opener<R>(pub R);

impl<R: CommandRunner> CommandRunner for Opener<R> {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        #[cfg(feature = "automation")]
        {
            crate::automation::store::record_open(program, args);
            Ok(String::new())
        }
        #[cfg(not(feature = "automation"))]
        {
            self.0.run(program, args)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<String>>);
    impl CommandRunner for &Recorder {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            self.0
                .borrow_mut()
                .push(format!("{program} {}", args.join(" ")));
            Ok("ran".into())
        }
    }

    #[cfg(not(feature = "automation"))]
    #[test]
    fn release_opener_runs_the_real_runner_unchanged() {
        let real = Recorder::default();
        assert_eq!(
            Opener(&real).run("open", &["--", "https://a"]),
            Ok("ran".into())
        );
        assert_eq!(*real.0.borrow(), ["open -- https://a"]);
    }

    #[cfg(feature = "automation")]
    #[test]
    fn automation_opener_records_and_never_launches() {
        let real = Recorder::default();
        let url = "https://example.invalid/opener-test";
        assert_eq!(Opener(&real).run("open", &["--", url]), Ok(String::new()));
        assert!(real.0.borrow().is_empty(), "the real runner must not run");
        let sink = crate::automation::store::opened();
        assert!(sink
            .iter()
            .any(|o| o.program == "open" && o.args == ["--", url] && o.target == url));
    }
}
