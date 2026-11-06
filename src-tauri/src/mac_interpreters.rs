//! Finding a real Python on a Mac (docs/features/155 card M12).
//!
//! Why this is not just "run `python3 --version`": `/usr/bin/python3` on a Mac without the Xcode command-line tools is
//! an Apple stub. Running it does not print a version, it pops up a "install developer tools" dialog. NEXUS starts the
//! speech server in the background, so that dialog would appear out of nowhere. The stub is therefore only tried when
//! `xcode-select -p` says the tools are installed.
//!
//! A GUI app started from Finder or the Dock gets a minimal `PATH` (no Homebrew), so well-known install locations are
//! tried as well as `PATH`.

use std::path::{Path, PathBuf};

/// Apple's launcher stubs. Present on every Mac, a real interpreter only if the developer tools are installed.
pub(crate) fn is_apple_python_stub(path: &Path) -> bool {
    matches!(path.to_str(), Some("/usr/bin/python3") | Some("/usr/bin/python"))
}

/// First `name` found on `path_var` that exists and is not Apple's stub. Pure apart from reading the file system.
pub(crate) fn path_lookup(name: &str, path_var: &str) -> Option<PathBuf> {
    for dir in path_var.split(':').filter(|d| !d.is_empty()) {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() && !is_apple_python_stub(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Well-known install locations, best first: Homebrew (Apple silicon, then Intel), pyenv, python.org framework builds.
pub(crate) fn well_known_candidates(home: Option<&str>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin/python3"),
        PathBuf::from("/usr/local/bin/python3"),
    ];
    if let Some(h) = home.filter(|h| !h.is_empty()) {
        v.push(Path::new(h).join(".pyenv/shims/python3"));
    }
    for ver in ["3.13", "3.12", "3.11", "3.10"] {
        v.push(PathBuf::from(format!("/Library/Frameworks/Python.framework/Versions/{ver}/bin/python3")));
    }
    v
}

#[cfg(target_os = "macos")]
fn runs(path: &Path) -> bool {
    std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// True when the developer tools are installed, i.e. `/usr/bin/python3` is a real interpreter and not the stub.
#[cfg(target_os = "macos")]
fn command_line_tools_installed() -> bool {
    std::process::Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A working interpreter as a path string, or `None` (callers then say plainly that Python is missing).
#[cfg(target_os = "macos")]
pub(crate) fn find_python() -> Option<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let mut tried: Vec<PathBuf> = Vec::new();
    for name in ["python3", "python"] {
        if let Some(p) = path_lookup(name, &path_var) {
            tried.push(p);
        }
    }
    tried.extend(well_known_candidates(std::env::var("HOME").ok().as_deref()));
    for p in tried {
        if p.is_file() && runs(&p) {
            tracing::info!("mac_interpreters: using Python at {}", p.display());
            return Some(p.to_string_lossy().to_string());
        }
    }
    if command_line_tools_installed() && runs(Path::new("/usr/bin/python3")) {
        return Some("/usr/bin/python3".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexus-mac-interp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch dir");
        d
    }

    #[test]
    fn apple_stub_is_recognised_and_real_interpreters_are_not() {
        assert!(is_apple_python_stub(Path::new("/usr/bin/python3")));
        assert!(is_apple_python_stub(Path::new("/usr/bin/python")));
        assert!(!is_apple_python_stub(Path::new("/opt/homebrew/bin/python3")));
        assert!(!is_apple_python_stub(Path::new("/usr/local/bin/python3")));
        // Same file name elsewhere is not the stub.
        assert!(!is_apple_python_stub(Path::new("/usr/bin/python3.12")));
    }

    // `:`-separated PATHs and absolute /paths are Unix notions; the module only runs on a Mac.
    #[cfg(unix)]
    #[test]
    fn path_lookup_finds_the_first_existing_entry_in_order() {
        let a = scratch_dir("a");
        let b = scratch_dir("b");
        std::fs::write(b.join("python3"), b"").expect("write");
        let path = format!("{}:{}", a.display(), b.display());
        assert_eq!(path_lookup("python3", &path), Some(b.join("python3")));
        std::fs::write(a.join("python3"), b"").expect("write");
        assert_eq!(path_lookup("python3", &path), Some(a.join("python3")), "earlier PATH entry wins");
        assert_eq!(path_lookup("python3", ""), None);
        assert_eq!(path_lookup("python3", "::"), None, "empty entries are ignored, not treated as the cwd");
        assert_eq!(path_lookup("definitely-not-here", &path), None);
    }

    #[test]
    fn path_lookup_never_returns_the_apple_stub() {
        // Only meaningful where /usr/bin/python3 exists (macOS, most Linux); elsewhere it is trivially None.
        assert_eq!(path_lookup("python3", "/usr/bin"), None);
    }

    // `:`-separated PATHs and absolute /paths are Unix notions; the module only runs on a Mac.
    #[cfg(unix)]
    #[test]
    fn well_known_candidates_cover_homebrew_both_architectures_pyenv_and_python_org() {
        let v = well_known_candidates(Some("/Users/me"));
        assert_eq!(v[0], Path::new("/opt/homebrew/bin/python3"));
        assert_eq!(v[1], Path::new("/usr/local/bin/python3"));
        assert!(v.contains(&PathBuf::from("/Users/me/.pyenv/shims/python3")));
        assert!(v.iter().any(|p| p.to_string_lossy().contains("Python.framework/Versions/3.12/")));
        assert!(!v.iter().any(is_stub_ref), "the stub is never a well-known candidate");
        // No HOME: no pyenv entry, and nothing relative.
        let none = well_known_candidates(None);
        assert!(none.iter().all(|p| p.is_absolute()));
        assert!(!none.iter().any(|p| p.to_string_lossy().contains("pyenv")));
        let empty = well_known_candidates(Some(""));
        assert!(empty.iter().all(|p| p.is_absolute()));
    }

    fn is_stub_ref(p: &PathBuf) -> bool {
        is_apple_python_stub(p)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn find_python_returns_something_that_runs_on_the_ci_runner() {
        // GitHub's macOS image ships Homebrew Python, so a real interpreter must be found, and never the stub
        // without the developer tools (which would have shown a dialog).
        let p = find_python().expect("a Python on the macOS runner");
        assert!(runs(Path::new(&p)), "{p} must run");
    }
}
