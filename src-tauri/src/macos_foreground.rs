//! Which app is in front on macOS: the counterpart of the Win32 foreground helpers in `browser_url.rs`.
//!
//! * **Identity** (name, bundle id, pid) comes from `lsappinfo list`: no permission needed, about 15 ms, and the
//!   frontmost app's record carries an `(in front)` marker on its header line. (`lsappinfo info <asn>` printed nothing
//!   on a CI runner, so the list is parsed instead.)
//! * **Window title** comes from System Events (about 0.12 s once warm). That needs the user's Automation permission, so
//!   it is bounded by a timeout: a pending permission prompt must never hang the caller. No permission, no title: the
//!   title is then empty and every safety check falls back to the bundle id.
//!
//! The parsers are pure and tested against output captured from a real macOS 14 runner (see the fixtures below).
//! Everything that starts a process is macOS-only and exercised by the macOS CI job.

/// The frontmost application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontApp {
    /// Display name, e.g. `Google Chrome`.
    pub name: String,
    /// Bundle identifier, e.g. `com.google.Chrome`. None for helper processes that have none.
    pub bundle_id: Option<String>,
    pub pid: u32,
}

impl FrontApp {
    /// The stable lower-case key used where Windows uses the executable name (`brave.exe`): the bundle id, or the
    /// display name when the process has none. Shortcut families and the safety lists match on this.
    pub fn key(&self) -> String {
        self.bundle_id.clone().unwrap_or_else(|| self.name.clone()).to_lowercase()
    }
}

/// Pull the frontmost app out of `lsappinfo list` output.
///
/// A record looks like (real output, macOS 14.8):
/// ```text
///  8) "Finder" ASN:0x0-c00c: (in front)
///     bundleID="com.apple.finder"
///     bundle path="/System/Library/CoreServices/Finder.app"
///     pid = 307 type="Foreground" flavor=3 ...
/// ```
/// Helper processes print `bundleID=[ NULL ]`. Returns None when no record is marked `(in front)` or it has no pid.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_front_app(list_output: &str) -> Option<FrontApp> {
    struct Rec {
        name: String,
        front: bool,
        bundle_id: Option<String>,
        pid: Option<u32>,
    }
    fn finish(rec: Option<Rec>) -> Option<FrontApp> {
        let r = rec?;
        if !r.front {
            return None;
        }
        Some(FrontApp { name: r.name, bundle_id: r.bundle_id, pid: r.pid? })
    }

    let mut cur: Option<Rec> = None;
    for line in list_output.lines() {
        let t = line.trim();
        // header: `<n>) "<name>" ASN:0x0-xxxx: [(in front)]`
        if let Some((num, rest)) = t.split_once(')') {
            let rest = rest.trim_start();
            if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) && rest.starts_with('"') {
                if let Some(found) = finish(cur.take()) {
                    return Some(found);
                }
                let end = rest.find("\" ASN:")?;
                cur = Some(Rec {
                    name: rest[1..end].to_string(),
                    // only the part AFTER the ASN counts: an app may have "(in front)" in its own name
                    front: rest[end..].contains("(in front)"),
                    bundle_id: None,
                    pid: None,
                });
                continue;
            }
        }
        let Some(rec) = cur.as_mut() else { continue };
        if let Some(v) = t.strip_prefix("bundleID=") {
            let v = v.trim();
            if let Some(inner) = v.strip_prefix('"') {
                if let Some(end) = inner.find('"') {
                    rec.bundle_id = Some(inner[..end].to_string());
                }
            }
        } else if let Some(v) = t.strip_prefix("pid = ") {
            let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
            rec.pid = digits.parse().ok();
        }
    }
    finish(cur)
}

/// AppleScript that returns the front window's title of the process with the pid given as `argv`. The pid is data,
/// not script text. No windows or a window without a name give an empty string (a `try` swallows "Invalid index",
/// which is what System Events reports for an app with no window).
#[cfg(any(target_os = "macos", test))]
pub(crate) const TITLE_SCRIPT: &[&str] = &[
    "on run argv",
    "set targetPid to (item 1 of argv) as integer",
    "tell application \"System Events\"",
    "set t to \"\"",
    "try",
    "set p to first application process whose unix id is targetPid",
    "set t to name of front window of p",
    "if t is missing value then set t to \"\"",
    "end try",
    "return t",
    "end tell",
    "end run",
];

/// `osascript` arguments that read the title of the window of `pid`.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn title_args(pid: u32) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    for line in TITLE_SCRIPT {
        args.push("-e".into());
        args.push((*line).into());
    }
    args.push(pid.to_string());
    args
}

/// The script prints the title followed by a newline; strip exactly that.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn clean_title(raw: &str) -> String {
    raw.trim_end_matches(['\n', '\r']).to_string()
}

#[cfg(target_os = "macos")]
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Run a command and capture stdout, killing it after `limit`. None on failure, non-zero exit or timeout. The output
/// is drained on its own thread so a large listing can never fill the pipe and stall the child.
#[cfg(target_os = "macos")]
pub(crate) fn run_capture(cmd: &mut Command, limit: Duration) -> Option<String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(8)),
            Err(_) => return None,
        }
    }
}

/// The frontmost app, or None when it cannot be read.
#[cfg(target_os = "macos")]
pub(crate) fn front_app() -> Option<FrontApp> {
    let out = run_capture(Command::new("lsappinfo").arg("list"), Duration::from_millis(1500))?;
    parse_front_app(&out)
}

/// Every app record of `lsappinfo list` output as (ASN, app), e.g. ("0x0-0x271271", Calculator). Records without a pid
/// are skipped.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_app_records(list_output: &str) -> Vec<(String, FrontApp)> {
    let mut out: Vec<(String, FrontApp)> = Vec::new();
    let mut cur: Option<(String, String, Option<String>, Option<u32>)> = None;
    let mut flush = |cur: &mut Option<(String, String, Option<String>, Option<u32>)>, out: &mut Vec<(String, FrontApp)>| {
        if let Some((asn, name, bundle_id, Some(pid))) = cur.take() {
            out.push((asn, FrontApp { name, bundle_id, pid }));
        }
    };
    for line in list_output.lines() {
        let t = line.trim();
        // header: `<n>) "<name>" ASN:0x0-xxxx: [(in front)]`
        if let Some((num, rest)) = t.split_once(')') {
            let rest = rest.trim_start();
            if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) && rest.starts_with('"') {
                flush(&mut cur, &mut out);
                if let Some(end) = rest.find("\" ASN:") {
                    let asn = rest[end + 6..].split(':').next().unwrap_or("").trim().to_string();
                    cur = Some((asn, rest[1..end].to_string(), None, None));
                }
                continue;
            }
        }
        let Some(rec) = cur.as_mut() else { continue };
        if let Some(v) = t.strip_prefix("bundleID=") {
            if let Some(inner) = v.trim().strip_prefix('"') {
                rec.2 = inner.find('"').map(|end| inner[..end].to_string());
            }
        } else if let Some(v) = t.strip_prefix("pid = ") {
            rec.3 = v.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok();
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// The ASNs of `lsappinfo visibleProcessList` output, front to back. The list holds regular apps only (not NEXUS,
/// which runs as a UI element), e.g. `ASN:0x0-0x205205-"Terax": ASN:0x0-0x271271-"Calculator":`.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_visible_order(visible_output: &str) -> Vec<String> {
    visible_output
        .split("ASN:")
        .filter_map(|part| part.split_once("-\"").map(|(asn, _)| asn.trim().to_string()))
        .filter(|asn| !asn.is_empty())
        .collect()
}

/// The front-most visible app that is not `own_pid`: the app the user was working in when NEXUS's own window (the
/// sidebar) took focus. Pure.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn first_visible_except(list_output: &str, visible_output: &str, own_pid: u32) -> Option<FrontApp> {
    let records = parse_app_records(list_output);
    parse_visible_order(visible_output)
        .iter()
        .filter_map(|asn| records.iter().find(|(a, _)| a == asn).map(|(_, app)| app))
        .find(|app| app.pid != own_pid)
        .cloned()
}

/// The app directly behind NEXUS (see `first_visible_except`), or None when it cannot be read.
#[cfg(target_os = "macos")]
pub(crate) fn app_behind_nexus() -> Option<FrontApp> {
    let limit = Duration::from_millis(1500);
    let visible = run_capture(Command::new("lsappinfo").arg("visibleProcessList"), limit)?;
    let list = run_capture(Command::new("lsappinfo").arg("list"), limit)?;
    first_visible_except(&list, &visible, std::process::id())
}

/// Title of the front window of the process `pid`. Some("") means "no window / no name"; None means System Events could
/// not be asked (Automation permission missing or denied, or it timed out).
#[cfg(target_os = "macos")]
pub(crate) fn window_title(pid: u32) -> Option<String> {
    let out = run_capture(Command::new("osascript").args(title_args(pid)), Duration::from_millis(1500))?;
    Some(clean_title(&out))
}

#[cfg(test)]
mod behind_nexus_tests {
    use super::*;

    // Captured on a real Mac (macOS 27.2) with NEXUS's sidebar open: NEXUS is a UI element, so it is in `list` but not
    // in `visibleProcessList`.
    const LIST: &str = r#"
72) "Terax" ASN:0x0-0x205205: (in front)
    bundleID="app.crynta.terax"
    pid = 29010 token=[sess=100015 pid=29010 uid:501,501,501 g:20,20 pV:56103] type="Foreground" flavor=3
84) "nexus" ASN:0x0-0x269269:
    bundleID=[ NULL ]
    pid = 42389 token=[sess=100015 pid=42389 uid:501,501,501 g:20,20 pV:83917] type="UIElement" flavor=3
88) "Calculator" ASN:0x0-0x271271:
    bundleID="com.apple.calculator"
    pid = 43144 token=[sess=100015 pid=43144 uid:501,501,501 g:20,20 pV:85285] type="Foreground" flavor=3
"#;
    const VISIBLE: &str =
        r#"ASN:0x0-0x271271-"Calculator": ASN:0x0-0x205205-"Terax": ASN:0x0-0x256256-"Brave_Browser": ASN:0x0-0x38038-"Finder":"#;

    #[test]
    fn records_carry_asn_name_bundle_and_pid() {
        let r = parse_app_records(LIST);
        assert_eq!(r.len(), 3);
        assert_eq!(r[2].0, "0x0-0x271271");
        assert_eq!(r[2].1, FrontApp { name: "Calculator".into(), bundle_id: Some("com.apple.calculator".into()), pid: 43144 });
        assert_eq!(r[1].1.bundle_id, None, "NULL bundle id");
    }

    #[test]
    fn visible_order_is_front_to_back() {
        assert_eq!(parse_visible_order(VISIBLE), ["0x0-0x271271", "0x0-0x205205", "0x0-0x256256", "0x0-0x38038"]);
        assert!(parse_visible_order("").is_empty());
    }

    #[test]
    fn the_app_behind_nexus_is_the_first_visible_one_that_is_not_nexus() {
        let behind = first_visible_except(LIST, VISIBLE, 42389).unwrap();
        assert_eq!(behind.name, "Calculator");
        // If our own pid were first in the visible list it is skipped.
        let behind = first_visible_except(LIST, VISIBLE, 43144).unwrap();
        assert_eq!(behind.name, "Terax");
        // Apps missing from `list` (Brave, Finder here) are passed over, not invented.
        assert_eq!(first_visible_except(LIST, r#"ASN:0x0-0x256256-"Brave_Browser":"#, 1), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim from `lsappinfo list` on a macOS 14.8.9 GitHub runner (probe run 42026454683), shortened.
    const FINDER_FRONT: &str = r#" 1) "loginwindow" ASN:0x0-2002:
    bundleID="com.apple.loginwindow"
    bundle path="/System/Library/CoreServices/loginwindow.app"
    executable path="/System/Library/CoreServices/loginwindow.app/Contents/MacOS/loginwindow"
    pid = 142 type="UIElement" flavor=3 Version="3022.6.2.1.100" fileType="APPL" creator="lgnw" Arch=ARM64
    checkin time = 2026/11/05 12:13:18 ( 5 minutes, 10.6727 seconds ago )

 2) "universalaccessd" ASN:0x0-6006:
    bundleID=[ NULL ]
    bundle path="/usr/sbin/universalaccessd"
    executable path="/usr/sbin/universalaccessd"
    pid = 300 !signalled type="BackgroundOnly" flavor=3 Version=[ NULL ]  fileType="????" creator="????" Arch=ARM64

 8) "Finder" ASN:0x0-c00c: (in front)
    bundleID="com.apple.finder"
    bundle path="/System/Library/CoreServices/Finder.app"
    executable path="/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder"
    pid = 307 type="Foreground" flavor=3 Version="1632.7.1" fileType="FNDR" creator="MACS" Arch=ARM64
    checkin time = 2026/11/05 12:13:09 ( 8 minutes, 42.3696 seconds ago )
"#;

    const TEXTEDIT_FRONT: &str = r#"30) "Finder" ASN:0x0-c00c:
    bundleID="com.apple.finder"
    pid = 307 type="Foreground" flavor=3 Version="1632.7.1" fileType="FNDR" creator="MACS" Arch=ARM64

31) "TextEdit" ASN:0x0-2f02f: (in front)
    bundleID="com.apple.TextEdit"
    bundle path="/System/Applications/TextEdit.app"
    executable path="/System/Applications/TextEdit.app/Contents/MacOS/TextEdit"
    pid = 5799 type="Foreground" flavor=3 Version="404" fileType="APPL" creator="ttxt" Arch=ARM64 sandboxed
    parentASN=ASN:0x1-0x16a6:
    launch time =  3 seconds ago, 2026/11/05 12:21:52 ( 3.20481 seconds ago )
"#;

    #[test]
    fn the_real_finder_listing_gives_finder() {
        let app = parse_front_app(FINDER_FRONT).expect("a front app");
        assert_eq!(app, FrontApp { name: "Finder".into(), bundle_id: Some("com.apple.finder".into()), pid: 307 });
        assert_eq!(app.key(), "com.apple.finder");
    }

    #[test]
    fn the_real_textedit_listing_gives_textedit_with_a_multi_digit_pid_and_trailing_lines() {
        let app = parse_front_app(TEXTEDIT_FRONT).expect("a front app");
        assert_eq!(app.name, "TextEdit");
        assert_eq!(app.bundle_id.as_deref(), Some("com.apple.TextEdit"));
        assert_eq!(app.pid, 5799);
        assert_eq!(app.key(), "com.apple.textedit", "the key is lower-case");
    }

    #[test]
    fn only_the_record_marked_in_front_counts_whatever_its_position() {
        // front app FIRST in the list, followed by others
        let front_first = r#" 1) "Safari" ASN:0x0-1001: (in front)
    bundleID="com.apple.Safari"
    pid = 11 type="Foreground" flavor=3
 2) "Finder" ASN:0x0-1002:
    bundleID="com.apple.finder"
    pid = 22 type="Foreground" flavor=3
"#;
        assert_eq!(parse_front_app(front_first).unwrap().pid, 11);
        // nothing marked in front
        let none = " 1) \"Finder\" ASN:0x0-1002: \n    bundleID=\"com.apple.finder\"\n    pid = 22 type=\"Foreground\"\n";
        assert_eq!(parse_front_app(none), None);
        // empty / garbage
        assert_eq!(parse_front_app(""), None);
        assert_eq!(parse_front_app("not lsappinfo output at all"), None);
    }

    #[test]
    fn a_front_process_without_a_bundle_id_falls_back_to_its_name_and_a_record_without_a_pid_is_rejected() {
        let helper = r#" 3) "CoreServicesUIAgent" ASN:0x0-7007: (in front)
    bundleID=[ NULL ]
    pid = 55 type="UIElement" flavor=3
"#;
        let app = parse_front_app(helper).unwrap();
        assert_eq!(app.bundle_id, None);
        assert_eq!(app.key(), "coreservicesuiagent");
        let no_pid = " 3) \"Odd\" ASN:0x0-7007: (in front) \n    bundleID=\"com.example.odd\"\n";
        assert_eq!(parse_front_app(no_pid), None, "no pid, no identity");
    }

    #[test]
    fn app_names_with_spaces_and_the_word_front_inside_a_name_do_not_confuse_the_parser() {
        let list = concat!(
            " 4) \"Front Row (in front)\" ASN:0x0-3004: \n",
            "    bundleID=\"com.example.frontrow\"\n",
            "    pid = 901 type=\"Foreground\"\n",
            " 5) \"Google Chrome\" ASN:0x0-3003: (in front) \n",
            "    bundleID=\"com.google.Chrome\"\n",
            "    pid = 900 type=\"Foreground\"\n",
        );
        // the FIRST app's NAME contains "(in front)" but its header is not marked: the real front app is the second
        let app = parse_front_app(list).unwrap();
        assert_eq!((app.name.as_str(), app.pid), ("Google Chrome", 900));
        // an app with that text in its name is not the front app just because of its name
        let only_named = concat!(
            " 4) \"Front Row (in front)\" ASN:0x0-3004: \n",
            "    bundleID=\"com.example.frontrow\"\n",
            "    pid = 901 type=\"Foreground\"\n",
        );
        assert_eq!(parse_front_app(only_named), None);
    }

    #[test]
    fn title_script_takes_the_pid_as_an_argument_and_never_inside_the_script() {
        let args = title_args(4242);
        assert_eq!(args.last().map(String::as_str), Some("4242"));
        let script: Vec<&String> = args.iter().filter(|a| a.as_str() != "-e").collect();
        assert_eq!(script.len(), TITLE_SCRIPT.len() + 1);
        assert!(!script[..TITLE_SCRIPT.len()].iter().any(|l| l.contains("4242")));
        // an app with no window must not become an error: the lookup is inside a `try`
        let try_at = TITLE_SCRIPT.iter().position(|l| *l == "try").unwrap();
        let get_at = TITLE_SCRIPT.iter().position(|l| l.contains("front window")).unwrap();
        let end_try_at = TITLE_SCRIPT.iter().position(|l| *l == "end try").unwrap();
        assert!(try_at < get_at && get_at < end_try_at);
    }

    #[test]
    fn clean_title_strips_only_the_line_ending() {
        assert_eq!(clean_title("Untitled\n"), "Untitled");
        assert_eq!(clean_title("\n"), "");
        assert_eq!(clean_title("  spaced  \r\n"), "  spaced  ");
        assert_eq!(clean_title("a\nb\n"), "a\nb");
    }

    // ── macOS only: the real commands, run by the macOS CI job on a real Mac ─────────────────────────────

    /// The title script must COMPILE with Apple's compiler (syntax cannot be checked anywhere else).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_title_script_compiles_with_osacompile() {
        let out = std::env::temp_dir().join(format!("nexus-title-{}.scpt", std::process::id()));
        let mut cmd = Command::new("osacompile");
        cmd.arg("-o").arg(&out);
        for line in TITLE_SCRIPT {
            cmd.arg("-e").arg(line);
        }
        let res = cmd.output().expect("osacompile runs on macOS");
        let _ = std::fs::remove_file(&out);
        assert!(res.status.success(), "osacompile: {}", String::from_utf8_lossy(&res.stderr));
    }

    /// On a real Mac there is always a frontmost app; `lsappinfo` needs no permission, so this must work on CI.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_front_app_is_found_on_this_mac() {
        let app = front_app().expect("lsappinfo list names a frontmost app");
        assert!(app.pid > 0, "{app:?}");
        assert!(!app.name.is_empty(), "{app:?}");
        assert!(!app.key().is_empty());
        // asking again must give a consistent answer in the same instant (no flapping parser state)
        let again = front_app().expect("second read");
        assert!(again.pid > 0);
    }

    /// The title read may legitimately be None (Automation permission denied) but must return promptly and never panic;
    /// when it works for an app without a window the answer is the empty string.
    #[cfg(target_os = "macos")]
    #[test]
    fn reading_the_front_window_title_returns_promptly() {
        let app = front_app().expect("front app");
        let started = Instant::now();
        let title = window_title(app.pid);
        assert!(started.elapsed() < Duration::from_millis(3500), "timeout must bound the call");
        if let Some(t) = title {
            assert!(!t.contains('\n'), "a title is one line: {t:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn run_capture_returns_output_fails_on_nonzero_and_kills_a_slow_command() {
        assert_eq!(run_capture(Command::new("/bin/sh").args(["-c", "echo hi"]), Duration::from_secs(5)).as_deref(), Some("hi\n"));
        assert_eq!(run_capture(Command::new("/bin/sh").args(["-c", "exit 3"]), Duration::from_secs(5)), None);
        let started = Instant::now();
        assert_eq!(run_capture(Command::new("/bin/sh").args(["-c", "sleep 20"]), Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(5), "the slow command must be killed at the limit");
        // 400 KB of output: would deadlock if the pipe were not drained while waiting
        let big = run_capture(Command::new("/bin/sh").args(["-c", "head -c 400000 /dev/zero | tr '\\0' x"]), Duration::from_secs(10));
        assert_eq!(big.map(|s| s.len()), Some(400_000));
    }
}
