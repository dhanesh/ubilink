//! The automation socket (S1, S2): a Unix socket at
//! `$TMPDIR/ubilink-<uid>/<instance>/auto.sock`, inside directories created 0700, the
//! socket itself 0600, and every connection's peer uid checked with `getpeereid`
//! before a byte is read. `<instance>` is `UBILINK_INSTANCE` (default "default"), so
//! two probe instances never share a socket (R8).
//!
//! S2: deliberately a different path and listener from the single-instance socket
//! (`shell::single_instance`, `org.dhanesh.ubilink.instance.sock`), which carries one fixed
//! word and is compiled into every build; this one exists only with the `automation`
//! feature.
//!
//! std's `UnixListener` rather than interprocess: it hands out the raw fd that
//! `getpeereid` needs. `UnixStream::peer_cred` is still unstable on Rust 1.98, and no
//! new crate is allowed (no `libc`), so the two libSystem calls are declared here.

use std::ffi::c_int;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

pub const INSTANCE_ENV: &str = "UBILINK_INSTANCE";
pub const DEFAULT_INSTANCE: &str = "default";
pub const SOCKET_NAME: &str = "auto.sock";

/// `sockaddr_un.sun_path` is 104 bytes on macOS, NUL included.
const SUN_PATH_MAX: usize = 103;

extern "C" {
    // libSystem (always linked on macOS). uid_t/gid_t are u32 there.
    fn getpeereid(fd: c_int, euid: *mut u32, egid: *mut u32) -> c_int;
    fn getuid() -> u32;
}

pub fn current_uid() -> u32 {
    // SAFETY: getuid takes no arguments, cannot fail, and touches no memory of ours.
    unsafe { getuid() }
}

/// The effective uid of the process on the other end of `stream`, or `None` if the
/// kernel would not say.
pub fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let (mut uid, mut gid) = (u32::MAX, u32::MAX);
    // SAFETY: the fd is a live socket owned by `stream` for the duration of the call,
    // and both out-pointers are valid, aligned u32s on our stack.
    let rc = unsafe { getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (rc == 0).then_some(uid)
}

/// S1: only a peer whose uid is ours may talk to the channel. An unknown peer (the
/// kernel refused to say) is refused too.
pub fn peer_allowed(own_uid: u32, peer: Option<u32>) -> bool {
    peer == Some(own_uid)
}

/// The instance name from the env value. Restricted to `[A-Za-z0-9_-]{1,32}` because it
/// becomes a path component: `../x` or `a/b` must not escape the per-user directory.
pub fn instance_name(value: Option<&str>) -> Result<String, String> {
    let name = match value {
        None => return Ok(DEFAULT_INSTANCE.into()),
        Some(v) => v,
    };
    let ok = (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(name.into())
    } else {
        Err(format!(
            "{INSTANCE_ENV} must be 1-32 chars of [A-Za-z0-9_-], got {name:?}"
        ))
    }
}

/// `<tmp>/ubilink-<uid>/<instance>/auto.sock`, refusing a path too long to bind.
pub fn socket_path(tmp: &Path, uid: u32, instance: &str) -> Result<PathBuf, String> {
    let p = tmp
        .join(format!("ubilink-{uid}"))
        .join(instance)
        .join(SOCKET_NAME);
    let len = p.as_os_str().len();
    if len > SUN_PATH_MAX {
        return Err(format!(
            "socket path is {len} bytes, over the {SUN_PATH_MAX}-byte limit: {}",
            p.display()
        ));
    }
    Ok(p)
}

/// Create `dir` 0700 if missing; if present it must be a real directory (not a
/// symlink) owned by `uid`, and is tightened to 0700. Another user's directory is an
/// error, never reused.
pub fn ensure_private_dir(dir: &Path, uid: u32) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(dir)?;
        }
        Err(e) => return Err(e),
        Ok(meta) => {
            if !meta.file_type().is_dir() {
                return Err(io::Error::other(format!(
                    "{} exists and is not a directory",
                    dir.display()
                )));
            }
            if meta.uid() != uid {
                return Err(io::Error::other(format!(
                    "{} is owned by uid {}, not {uid}",
                    dir.display(),
                    meta.uid()
                )));
            }
        }
    }
    // Also covers a dir created under a permissive umask or left by an older build.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Make the private directories, clear a stale socket, bind, and chmod the socket
/// 0600. A socket that still answers belongs to a live instance of the same name:
/// refuse rather than steal it.
pub fn bind(path: &Path, uid: u32) -> io::Result<UnixListener> {
    let instance_dir = path.parent().ok_or_else(|| io::Error::other("no parent"))?;
    let user_dir = instance_dir
        .parent()
        .ok_or_else(|| io::Error::other("no parent"))?;
    ensure_private_dir(user_dir, uid)?;
    ensure_private_dir(instance_dir, uid)?;
    if fs::symlink_metadata(path).is_ok() {
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("{} is served by a live instance", path.display()),
            ));
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    // The 0700 directory already keeps other users out between bind and chmod; the
    // 0600 mode is the second wall S1 asks for.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_per_user_and_per_instance() {
        let p = socket_path(Path::new("/tmp/x"), 501, "t6").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/x/ubilink-501/t6/auto.sock"));
        let long = "a".repeat(120);
        assert!(socket_path(Path::new(&long), 501, "t6")
            .unwrap_err()
            .contains("limit"));
    }

    #[test]
    fn instance_names_cannot_escape_the_user_dir() {
        assert_eq!(instance_name(None).unwrap(), "default");
        assert_eq!(instance_name(Some("t6")).unwrap(), "t6");
        assert_eq!(instance_name(Some("probe_A-2")).unwrap(), "probe_A-2");
        for bad in ["", "..", "../x", "a/b", "a b", &"x".repeat(33)] {
            assert!(instance_name(Some(bad)).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn only_the_same_uid_is_allowed() {
        assert!(peer_allowed(501, Some(501)));
        assert!(!peer_allowed(501, Some(0)), "not even root");
        assert!(!peer_allowed(501, Some(502)));
        assert!(!peer_allowed(501, None), "unknown peer is refused");
    }

    #[test]
    fn getpeereid_reports_our_own_uid_for_a_local_pair() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a), Some(current_uid()));
        assert!(peer_allowed(current_uid(), peer_uid(&a)));
    }

    #[test]
    fn bind_makes_0700_dirs_and_a_0600_socket_and_refuses_a_live_one() {
        let tmp = std::env::temp_dir().join(format!("ubn-auto-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let uid = current_uid();
        let path = socket_path(&tmp, uid, "t").unwrap();
        let listener = bind(&path, uid).unwrap();
        let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(path.parent().unwrap().parent().unwrap()), 0o700);
        // A second bind while the first still listens must not steal it.
        let err = bind(&path, uid).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        // Once the listener is gone the stale file is replaced.
        drop(listener);
        let again = bind(&path, uid);
        assert!(again.is_ok(), "stale socket replaced: {again:?}");
        // A loosened directory is tightened back to 0700.
        let dir = path.parent().unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(dir, uid).unwrap();
        assert_eq!(mode(dir), 0o700);
        // A directory claimed for another uid is refused.
        assert!(ensure_private_dir(dir, uid.wrapping_add(1)).is_err());
        let _ = fs::remove_dir_all(&tmp);
    }
}
