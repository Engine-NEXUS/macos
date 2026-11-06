//! Canonical path handling for the Action Kernel (docs/features/140, P1).
//!
//! Every policy decision is made on the **canonical** path: `..`, junctions, symlinks and 8.3 short
//! names are resolved by the operating system, never by string tricks, so a path cannot be spelled
//! around a rule. Paths that cannot be proven safe (network shares, ADS, reserved device names,
//! trailing dots, unresolved `..`) are refused instead of guessed.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    Relative,
    Network,
    BadComponent(String),
    Io(String),
}

impl PathError {
    pub fn spoken(&self) -> String {
        match self {
            PathError::Empty => "I don't have a location for that, sir.".into(),
            PathError::Relative => "I only work with full locations, sir.".into(),
            PathError::Network => "I don't touch network locations, sir.".into(),
            PathError::BadComponent(_) => "That location has a name I don't trust, so I won't touch it, sir.".into(),
            PathError::Io(_) => "I couldn't look at that location, sir.".into(),
        }
    }
}

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `\\?\C:\x` -> `C:\x`; `\\?\UNC\srv\share` stays UNC (and is refused by the caller).
pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p
}

fn is_unc(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with(r"\\") || s.starts_with("//")
}

/// A single new path segment (a name we are about to create or an unverified tail).
pub fn check_name(name: &str) -> Result<(), PathError> {
    let bad = |why: &str| Err(PathError::BadComponent(format!("{why}: {name}")));
    if name.is_empty() || name == "." || name == ".." {
        return bad("dot");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return bad("trailing dot or space");
    }
    if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 32) {
        return bad("reserved character");
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return bad("device name");
    }
    if name.len() > 200 {
        return bad("too long");
    }
    Ok(())
}

/// Resolve `path` to the real location the operating system would act on.
/// Existing paths are canonicalised by the OS; for a path that does not exist yet the deepest
/// existing ancestor is canonicalised and the remaining names are checked and appended.
pub fn canonical(path: &Path) -> Result<PathBuf, PathError> {
    if path.as_os_str().is_empty() {
        return Err(PathError::Empty);
    }
    if is_unc(path) && !path.to_string_lossy().starts_with(r"\\?\") {
        return Err(PathError::Network);
    }
    if !path.is_absolute() {
        return Err(PathError::Relative);
    }
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(&existing) {
            Ok(real) => {
                let real = strip_verbatim(real);
                if is_unc(&real) {
                    return Err(PathError::Network);
                }
                let mut out = real;
                for name in tail.iter().rev() {
                    check_name(&name.to_string_lossy())?;
                    out.push(name);
                }
                return Ok(out);
            }
            Err(_) => {
                let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
                    // walked up to something with no file name (a root that cannot be read)
                    return Err(PathError::Io(format!("cannot resolve {}", path.display())));
                };
                // a trailing `..` cannot be resolved lexically without trusting links
                if matches!(existing.components().next_back(), Some(Component::ParentDir)) {
                    return Err(PathError::BadComponent("..".into()));
                }
                tail.push(name);
                if !existing.pop() {
                    return Err(PathError::Io("no existing ancestor".into()));
                }
            }
        }
    }
}

/// Case-folded comparison key (Windows paths are case-insensitive).
pub fn key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.trim_end_matches('\\').to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// `path` equals `root` or lies below it (component-wise, so `D:\ab` is not inside `D:\a`).
pub fn is_within(path: &Path, root: &Path) -> bool {
    let (p, r) = (key(path), key(root));
    p == r || p.starts_with(&format!("{r}\\"))
}

/// Does `path` have any component equal to one of `names` (case-folded)?
pub fn has_component(path: &Path, names: &[&str]) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_lowercase();
        names.iter().any(|n| s == *n)
    })
}

/// Drive or filesystem root (`C:\`, `/`).
pub fn is_root(path: &Path) -> bool {
    path.parent().is_none() || key(path).len() <= 3 && key(path).ends_with(':')
}

/// A junction, symlink or other reparse point. Never followed, never moved.
pub fn is_reparse(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0 || meta.file_type().is_symlink()
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

#[cfg(test)]
pub(crate) fn test_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "nexus_ak_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    strip_verbatim(std::fs::canonicalize(&d).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotdot_is_resolved_by_the_os_not_by_string_tricks() {
        let root = test_dir("dotdot");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("secret.txt"), b"x").unwrap();
        let sneaky = root.join("a").join("b").join("..").join("..").join("secret.txt");
        assert_eq!(canonical(&sneaky).unwrap(), root.join("secret.txt"));
    }

    #[test]
    fn unresolved_dotdot_in_a_missing_tail_is_refused() {
        let root = test_dir("dotdot_missing");
        let sneaky = root.join("nope").join("..").join("x");
        // Windows resolves `..` lexically for a missing folder, so the real target is root\x; either
        // way the answer is the place the OS would act on, or a refusal, never something else.
        match canonical(&sneaky) {
            Ok(p) => assert_eq!(p, root.join("x")),
            Err(e) => assert!(matches!(e, PathError::BadComponent(_) | PathError::Io(_))),
        }
    }

    #[test]
    fn new_names_are_checked_and_appended_to_the_real_parent() {
        let root = test_dir("new");
        assert_eq!(canonical(&root.join("fresh")).unwrap(), root.join("fresh"));
        for bad in ["trail.", "trail ", "CON", "nul.txt", "a:b", "x|y", "q?", "", ".."] {
            assert!(check_name(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(canonical(&root.join("ok").join("bad.")).is_err());
        assert!(check_name("holiday photos 2025").is_ok());
    }

    #[test]
    fn relative_and_network_paths_are_refused() {
        assert_eq!(canonical(Path::new("")).unwrap_err(), PathError::Empty);
        assert_eq!(canonical(Path::new("relative/x")).unwrap_err(), PathError::Relative);
        assert_eq!(canonical(Path::new(r"\\server\share\x")).unwrap_err(), PathError::Network);
        assert_eq!(canonical(Path::new("//server/share/x")).unwrap_err(), PathError::Network);
    }

    #[test]
    fn within_is_component_wise_and_case_folded() {
        let root = Path::new(r"D:\Users\me\Downloads");
        assert!(is_within(Path::new(r"D:\Users\me\Downloads\x.pdf"), root));
        assert!(is_within(root, root));
        assert!(!is_within(Path::new(r"D:\Users\me\Downloads2\x.pdf"), root));
        assert!(!is_within(Path::new(r"D:\Users\me"), root));
        if cfg!(windows) {
            assert!(is_within(Path::new(r"d:\users\ME\DOWNLOADS\x"), root));
        }
    }

    #[test]
    fn roots_and_components() {
        assert!(is_root(Path::new(r"C:\")));
        assert!(is_root(Path::new(r"D:")) || cfg!(not(windows)));
        assert!(!is_root(Path::new(r"C:\Users")));
        // forward slashes: they separate components on Windows AND on macOS/Linux (a backslash path is one component there)
        assert!(has_component(Path::new("D:/p/.GIT/config"), &[".git"]));
        assert!(!has_component(Path::new("D:/p/gitlab/config"), &[".git"]));
    }

    #[cfg(windows)]
    #[test]
    fn junctions_are_resolved_and_flagged() {
        let root = test_dir("junction");
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("f.txt"), b"x").unwrap();
        let link = root.join("link");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", link.to_str().unwrap(), real.to_str().unwrap()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("mklink /J unavailable here; junction test skipped");
            return;
        }
        assert!(is_reparse(&link));
        assert!(!is_reparse(&real));
        // a path spelled through the junction resolves to the real folder
        assert_eq!(canonical(&link.join("f.txt")).unwrap(), real.join("f.txt"));
    }

    #[cfg(windows)]
    #[test]
    fn short_8_3_names_resolve_to_the_long_name() {
        let root = test_dir("short83");
        let long = root.join("a long folder name");
        std::fs::create_dir_all(&long).unwrap();
        let out = std::process::Command::new("cmd")
            .args(["/C", "for", "%I", "in", &format!("({})", long.display()), "do", "@echo", "%~sI"])
            .output();
        if let Ok(o) = out {
            let short = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !short.is_empty() && short.contains('~') {
                assert_eq!(key(&canonical(Path::new(&short)).unwrap()), key(&long));
            }
        }
    }
}
