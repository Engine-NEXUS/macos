//! Shortcut Map (shortcut brain, phase E1): which key chord does **this application** use for **this
//! action**?  Pure data + lookup, no OS calls.
//!
//! Resolution order for `(foreground exe, action)`:
//!   1. an exact-executable table (`"brave.exe": {…}`)      → `Scope::Exe`
//!   2. the application's family table (browser, vscode, …) → `Scope::Family`
//!   3. the `"*"` table of near-universal Windows defaults  → `Scope::Global`
//!      — **skipped** for `no_global` families (terminals/consoles), where `Ctrl+C` is an interrupt.
//! No match is reported as `NoBinding`, carrying whether the application is *known* (see `Lookup`).
//!
//! The seed (`seed_windows.json`) is hand-written from each application's documented defaults. Later
//! phases add harvested / taught / observed sources on top (docs/features/103 §4); the lookup API does
//! not change.

use std::collections::HashMap;

use once_cell::sync::Lazy;
use serde_json::Value;

use super::keys::{parse_chord, Chord};

/// The closed action vocabulary shared with the understanding track (docs/features/103 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    NewTab,
    CloseTab,
    ReopenTab,
    NextTab,
    PrevTab,
    GotoTab,
    Refresh,
    GoBack,
    GoForward,
    AddressBar,
    FindInPage,
    Save,
    Copy,
    Paste,
    Cut,
    Undo,
    Redo,
    SelectAll,
    ScrollDown,
    ScrollUp,
    ZoomIn,
    ZoomOut,
    SwitchWindow,
}

impl Action {
    pub const ALL: [Action; 23] = [
        Action::NewTab,
        Action::CloseTab,
        Action::ReopenTab,
        Action::NextTab,
        Action::PrevTab,
        Action::GotoTab,
        Action::Refresh,
        Action::GoBack,
        Action::GoForward,
        Action::AddressBar,
        Action::FindInPage,
        Action::Save,
        Action::Copy,
        Action::Paste,
        Action::Cut,
        Action::Undo,
        Action::Redo,
        Action::SelectAll,
        Action::ScrollDown,
        Action::ScrollUp,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::SwitchWindow,
    ];

    /// Stable id used in the seed file, logs and (later) the NLU/lexicon.
    pub fn id(self) -> &'static str {
        match self {
            Action::NewTab => "new_tab",
            Action::CloseTab => "close_tab",
            Action::ReopenTab => "reopen_tab",
            Action::NextTab => "next_tab",
            Action::PrevTab => "prev_tab",
            Action::GotoTab => "goto_tab_n",
            Action::Refresh => "refresh",
            Action::GoBack => "go_back",
            Action::GoForward => "go_forward",
            Action::AddressBar => "address_bar",
            Action::FindInPage => "find_in_page",
            Action::Save => "save",
            Action::Copy => "copy",
            Action::Paste => "paste",
            Action::Cut => "cut",
            Action::Undo => "undo",
            Action::Redo => "redo",
            Action::SelectAll => "select_all",
            Action::ScrollDown => "scroll_down",
            Action::ScrollUp => "scroll_up",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::SwitchWindow => "switch_window",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.id() == id)
    }

    /// Short phrase for spoken replies ("New tab sir.", "I don't have a back shortcut for…").
    pub fn label(self) -> &'static str {
        match self {
            Action::NewTab => "new tab",
            Action::CloseTab => "tab closed",
            Action::ReopenTab => "tab reopened",
            Action::NextTab => "next tab",
            Action::PrevTab => "previous tab",
            Action::GotoTab => "switched tab",
            Action::Refresh => "refreshed",
            Action::GoBack => "back",
            Action::GoForward => "forward",
            Action::AddressBar => "address bar",
            Action::FindInPage => "find",
            Action::Save => "saved",
            Action::Copy => "copied",
            Action::Paste => "pasted",
            Action::Cut => "cut",
            Action::Undo => "undone",
            Action::Redo => "redone",
            Action::SelectAll => "selected all",
            Action::ScrollDown => "scrolled down",
            Action::ScrollUp => "scrolled up",
            Action::ZoomIn => "zoomed in",
            Action::ZoomOut => "zoomed out",
            Action::SwitchWindow => "switched window",
        }
    }

    /// True when the action's seed template takes the tab number (`{n}`).
    pub fn takes_number(self) -> bool {
        matches!(self, Action::GotoTab)
    }
}

/// Where a binding came from (becomes a trust ladder once harvested/taught sources exist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Seed,
}

/// Which table of the seed matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Exe(String),
    Family(String),
    Global,
}

impl Scope {
    pub fn describe(&self) -> String {
        match self {
            Scope::Exe(e) => format!("exe:{e}"),
            Scope::Family(f) => format!("family:{f}"),
            Scope::Global => "global".to_string(),
        }
    }
}

/// Result of a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found { chord: Chord, scope: Scope, source: Source },
    /// No chord for this (app, action). `known_app` = the executable belongs to a seeded family — then
    /// "no binding" is an informative answer ("Notepad has no refresh shortcut"). For an unknown
    /// executable it means "I know nothing about this app".
    NoBinding { known_app: bool, family: Option<String> },
    /// The action needs a number (tab 1-9) and the argument is missing or out of range.
    BadArg(String),
}

#[derive(Debug, Default)]
struct FamilyInfo {
    no_global: bool,
}

/// The loaded, validated map.
#[derive(Debug)]
pub struct ShortcutMap {
    /// lowercase exe → family name
    exe_family: HashMap<String, String>,
    families: HashMap<String, FamilyInfo>,
    /// scope key ("*", family name, or lowercase exe) → action → chord templates (first preferred)
    tables: HashMap<String, HashMap<Action, Vec<String>>>,
}

static SEED_JSON: &str = include_str!("seed_windows.json");

/// The bundled map. A parse failure (caught by the `seed_is_valid` test, so it cannot ship) degrades to an
/// empty map: every lookup is then `NoBinding` and callers fall back to their legacy path.
pub static SEED: Lazy<ShortcutMap> = Lazy::new(|| match ShortcutMap::from_json(SEED_JSON) {
    Ok(m) => m,
    Err(e) => {
        tracing::error!("[SHORTCUT] seed failed to load ({e}) - shortcut brain disabled");
        ShortcutMap::empty()
    }
});

static SEED_MACOS_JSON: &str = include_str!("seed_macos.json");

/// The macOS map: Cmd chords, apps keyed by lower-case bundle id (docs/features/155, 156). Same fail-safe as `SEED`.
pub static SEED_MACOS: Lazy<ShortcutMap> = Lazy::new(|| match ShortcutMap::from_json(SEED_MACOS_JSON) {
    Ok(m) => m,
    Err(e) => {
        tracing::error!("[SHORTCUT] macOS seed failed to load ({e}) - shortcut brain disabled");
        ShortcutMap::empty()
    }
});

/// The map for the OS we are running on. On Windows and Linux this is exactly `SEED`.
pub fn active_seed() -> &'static ShortcutMap {
    if cfg!(target_os = "macos") {
        &SEED_MACOS
    } else {
        &SEED
    }
}

impl ShortcutMap {
    pub fn empty() -> Self {
        Self { exe_family: HashMap::new(), families: HashMap::new(), tables: HashMap::new() }
    }

    /// Parse and **validate**: unknown actions, unparseable chords, a `{n}` template on a non-numbered action
    /// (or its absence on `goto_tab_n`), an executable in two families, and tables for undeclared families
    /// are all errors — a typo in the seed fails a test instead of silently dropping a shortcut.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| format!("seed json: {e}"))?;
        let mut m = ShortcutMap::empty();

        let fams = root.get("families").and_then(|v| v.as_object()).ok_or("seed: missing 'families'")?;
        for (name, def) in fams {
            let no_global = def.get("no_global").and_then(|v| v.as_bool()).unwrap_or(false);
            m.families.insert(name.clone(), FamilyInfo { no_global });
            let exes = def.get("exes").and_then(|v| v.as_array()).ok_or(format!("family '{name}': missing exes"))?;
            for e in exes {
                let exe = e.as_str().ok_or(format!("family '{name}': exe is not a string"))?.to_lowercase();
                if let Some(prev) = m.exe_family.insert(exe.clone(), name.clone()) {
                    return Err(format!("exe '{exe}' is in two families ('{prev}' and '{name}')"));
                }
            }
        }

        let tables = root.get("bindings").and_then(|v| v.as_object()).ok_or("seed: missing 'bindings'")?;
        for (scope, entries) in tables {
            let is_exe = scope.ends_with(".exe");
            if scope != "*" && !is_exe && !m.families.contains_key(scope) {
                return Err(format!("bindings for undeclared family '{scope}'"));
            }
            let entries = entries.as_object().ok_or(format!("bindings '{scope}': not an object"))?;
            let mut table: HashMap<Action, Vec<String>> = HashMap::new();
            for (aid, val) in entries {
                let action = Action::from_id(aid).ok_or(format!("bindings '{scope}': unknown action '{aid}'"))?;
                let alts: Vec<String> = match val {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => a
                        .iter()
                        .map(|x| x.as_str().map(str::to_string).ok_or(format!("'{scope}.{aid}': non-string alternative")))
                        .collect::<Result<_, _>>()?,
                    _ => return Err(format!("'{scope}.{aid}': expected string or array")),
                };
                if alts.is_empty() {
                    return Err(format!("'{scope}.{aid}': empty alternatives"));
                }
                for t in &alts {
                    let has_n = t.contains("{n}");
                    if has_n != action.takes_number() {
                        return Err(format!(
                            "'{scope}.{aid}': template '{t}' {} the {{n}} placeholder",
                            if action.takes_number() { "must contain" } else { "must not contain" }
                        ));
                    }
                    // Validate by substituting a real digit.
                    parse_chord(&t.replace("{n}", "1")).map_err(|e| format!("'{scope}.{aid}': {e}"))?;
                }
                table.insert(action, alts);
            }
            m.tables.insert(scope.clone(), table);
        }
        Ok(m)
    }

    /// Family name of an executable (lowercase, e.g. `"brave.exe"` → `"browser"`).
    pub fn family_of(&self, exe: &str) -> Option<&str> {
        self.exe_family.get(&exe.to_lowercase()).map(|s| s.as_str())
    }

    /// Resolve the chord for `(exe, action)`. `arg` is the tab number for `GotoTab`.
    pub fn lookup(&self, exe: &str, action: Action, arg: Option<u32>) -> Lookup {
        let exe_l = exe.to_lowercase();
        let family = self.family_of(&exe_l).map(|s| s.to_string());

        if action.takes_number() {
            match arg {
                Some(n) if (1..=9).contains(&n) => {}
                Some(n) => return Lookup::BadArg(format!("tab {n} is out of range (1-9)")),
                None => return Lookup::BadArg("which tab number?".into()),
            }
        }

        let mut candidates: Vec<(String, Scope)> = vec![(exe_l.clone(), Scope::Exe(exe_l.clone()))];
        if let Some(f) = &family {
            candidates.push((f.clone(), Scope::Family(f.clone())));
        }
        let global_allowed = family
            .as_ref()
            .and_then(|f| self.families.get(f))
            .map(|i| !i.no_global)
            .unwrap_or(true);
        if global_allowed {
            candidates.push(("*".to_string(), Scope::Global));
        }

        for (key, scope) in candidates {
            if let Some(alts) = self.tables.get(&key).and_then(|t| t.get(&action)) {
                let template = &alts[0];
                let text = match arg {
                    Some(n) if action.takes_number() => template.replace("{n}", &n.to_string()),
                    _ => template.clone(),
                };
                match parse_chord(&text) {
                    Ok(chord) => return Lookup::Found { chord, scope, source: Source::Seed },
                    Err(_) => continue, // unreachable for a validated seed
                }
            }
        }
        Lookup::NoBinding { known_app: family.is_some(), family }
    }

    /// Every action that has a binding for this executable (powers "what can I do with the keyboard here?").
    pub fn actions_for(&self, exe: &str) -> Vec<Action> {
        Action::ALL
            .iter()
            .copied()
            .filter(|a| {
                let arg = if a.takes_number() { Some(1) } else { None };
                matches!(self.lookup(exe, *a, arg), Lookup::Found { .. })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord_of(exe: &str, a: Action, arg: Option<u32>) -> Option<String> {
        match SEED.lookup(exe, a, arg) {
            Lookup::Found { chord, .. } => Some(chord.display()),
            _ => None,
        }
    }

    #[test]
    fn seed_is_valid() {
        // Fails the build if the shipped JSON has a typo (unknown action, bad chord, double-booked exe…).
        ShortcutMap::from_json(SEED_JSON).expect("seed must parse and validate");
        assert!(SEED.family_of("brave.exe").is_some(), "seed actually loaded");
    }

    #[test]
    fn action_ids_roundtrip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
            assert!(seen.insert(a.id()), "duplicate id {}", a.id());
        }
        assert_eq!(Action::from_id("nope"), None);
    }

    /// Golden table: the legacy hard-coded chords must resolve identically for browsers (parity), and the
    /// new ones must be what the applications document.
    #[test]
    fn browser_golden_chords() {
        for exe in ["brave.exe", "chrome.exe", "msedge.exe", "firefox.exe", "BRAVE.EXE"] {
            assert_eq!(chord_of(exe, Action::NewTab, None).as_deref(), Some("Ctrl+T"), "{exe}");
            assert_eq!(chord_of(exe, Action::CloseTab, None).as_deref(), Some("Ctrl+W"));
            assert_eq!(chord_of(exe, Action::ReopenTab, None).as_deref(), Some("Ctrl+Shift+T"));
            assert_eq!(chord_of(exe, Action::NextTab, None).as_deref(), Some("Ctrl+Tab"));
            assert_eq!(chord_of(exe, Action::PrevTab, None).as_deref(), Some("Ctrl+Shift+Tab"));
            assert_eq!(chord_of(exe, Action::GoBack, None).as_deref(), Some("Alt+Left"));
            assert_eq!(chord_of(exe, Action::GoForward, None).as_deref(), Some("Alt+Right"));
            assert_eq!(chord_of(exe, Action::AddressBar, None).as_deref(), Some("Ctrl+L"));
            assert_eq!(chord_of(exe, Action::Refresh, None).as_deref(), Some("F5"));
            assert_eq!(chord_of(exe, Action::ZoomIn, None).as_deref(), Some("Ctrl+="));
        }
    }

    #[test]
    fn goto_tab_substitutes_the_number_and_validates_range() {
        assert_eq!(chord_of("brave.exe", Action::GotoTab, Some(3)).as_deref(), Some("Ctrl+3"));
        assert_eq!(chord_of("brave.exe", Action::GotoTab, Some(9)).as_deref(), Some("Ctrl+9"));
        assert_eq!(chord_of("code.exe", Action::GotoTab, Some(2)).as_deref(), Some("Alt+2"));
        assert!(matches!(SEED.lookup("brave.exe", Action::GotoTab, Some(0)), Lookup::BadArg(_)));
        assert!(matches!(SEED.lookup("brave.exe", Action::GotoTab, Some(10)), Lookup::BadArg(_)));
        assert!(matches!(SEED.lookup("brave.exe", Action::GotoTab, None), Lookup::BadArg(_)));
    }

    #[test]
    fn precedence_exe_then_family_then_global() {
        let json = r#"{
          "families": {"browser": {"exes": ["brave.exe", "chrome.exe"]}},
          "bindings": {
            "*": {"refresh": "Ctrl+R", "copy": "Ctrl+C"},
            "browser": {"refresh": "F5"},
            "brave.exe": {"refresh": "Ctrl+Shift+R"}
          }}"#;
        let m = ShortcutMap::from_json(json).unwrap();
        let got = |exe: &str, a| match m.lookup(exe, a, None) {
            Lookup::Found { chord, scope, .. } => Some((chord.display(), scope)),
            _ => None,
        };
        assert_eq!(got("brave.exe", Action::Refresh), Some(("Ctrl+Shift+R".into(), Scope::Exe("brave.exe".into()))));
        assert_eq!(got("chrome.exe", Action::Refresh), Some(("F5".into(), Scope::Family("browser".into()))));
        assert_eq!(got("unknown.exe", Action::Refresh), Some(("Ctrl+R".into(), Scope::Global)));
        // Family without its own copy entry falls through to global.
        assert_eq!(got("chrome.exe", Action::Copy), Some(("Ctrl+C".into(), Scope::Global)));
    }

    /// SAFETY: in terminals `Ctrl+C` interrupts the running process, `Ctrl+Z` suspends it and `Ctrl+S`
    /// freezes output. The global table must never apply there.
    #[test]
    fn terminals_never_inherit_global_edit_shortcuts() {
        for exe in ["windowsterminal.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "conhost.exe"] {
            for a in [Action::Copy, Action::Cut, Action::Undo, Action::Redo, Action::Save, Action::SelectAll] {
                let r = SEED.lookup(exe, a, None);
                // Windows Terminal has its own explicit copy; everything else must be NoBinding.
                match r {
                    Lookup::Found { chord, scope, .. } => {
                        assert_eq!(exe, "windowsterminal.exe", "{exe} {a:?} must not bind");
                        assert_eq!(a, Action::Copy);
                        assert_ne!(scope, Scope::Global);
                        assert_eq!(chord.display(), "Ctrl+Shift+C", "terminal copy is NOT Ctrl+C");
                    }
                    Lookup::NoBinding { known_app, .. } => assert!(known_app, "{exe} is a known app"),
                    Lookup::BadArg(_) => unreachable!(),
                }
            }
        }
        assert_eq!(chord_of("windowsterminal.exe", Action::Paste, None).as_deref(), Some("Ctrl+Shift+V"));
    }

    #[test]
    fn unknown_and_known_apps_report_differently() {
        // Unknown app: only the global table applies; app-specific actions are NoBinding(known_app=false).
        assert_eq!(chord_of("someapp.exe", Action::Copy, None).as_deref(), Some("Ctrl+C"));
        assert_eq!(
            SEED.lookup("someapp.exe", Action::NewTab, None),
            Lookup::NoBinding { known_app: false, family: None }
        );
        // Known non-browser app without that action: informative "no shortcut here".
        assert_eq!(
            SEED.lookup("notepad.exe", Action::Refresh, None),
            Lookup::NoBinding { known_app: true, family: Some("notepad".into()) }
        );
    }

    #[test]
    fn app_specific_mappings_differ_where_apps_differ() {
        assert_eq!(chord_of("code.exe", Action::NextTab, None).as_deref(), Some("Ctrl+PageDown"));
        assert_eq!(chord_of("notepad.exe", Action::NewTab, None).as_deref(), Some("Ctrl+N"));
        assert_eq!(chord_of("explorer.exe", Action::AddressBar, None).as_deref(), Some("Ctrl+L"));
        assert_eq!(chord_of("windowsterminal.exe", Action::NewTab, None).as_deref(), Some("Ctrl+Shift+T"));
    }

    #[test]
    fn validation_rejects_bad_seeds() {
        let bad = |bindings: &str| {
            ShortcutMap::from_json(&format!(
                r#"{{"families": {{"b": {{"exes": ["a.exe"]}}}}, "bindings": {bindings}}}"#
            ))
        };
        assert!(bad(r#"{"b": {"nonsense_action": "Ctrl+T"}}"#).is_err(), "unknown action");
        assert!(bad(r#"{"b": {"new_tab": "Ctrl+Nope"}}"#).is_err(), "bad chord");
        assert!(bad(r#"{"b": {"new_tab": "Ctrl+{n}"}}"#).is_err(), "{{n}} on a non-numbered action");
        assert!(bad(r#"{"b": {"goto_tab_n": "Ctrl+1"}}"#).is_err(), "numbered action without {{n}}");
        assert!(bad(r#"{"ghost": {"new_tab": "Ctrl+T"}}"#).is_err(), "undeclared family");
        assert!(
            ShortcutMap::from_json(r#"{"families": {"a": {"exes": ["x.exe"]}, "b": {"exes": ["x.exe"]}}, "bindings": {}}"#)
                .is_err(),
            "exe in two families"
        );
    }

    #[test]
    fn actions_for_lists_only_bound_actions() {
        let notepad = SEED.actions_for("notepad.exe");
        assert!(notepad.contains(&Action::Save) && notepad.contains(&Action::NewTab));
        assert!(!notepad.contains(&Action::Refresh));
        let term = SEED.actions_for("cmd.exe");
        assert!(term.is_empty(), "plain consoles bind nothing: {term:?}");
    }

    #[test]
    fn seed_all_action_names_exist_in_action_vocabulary() {
        let v: serde_json::Value = serde_json::from_str(SEED_JSON).expect("valid json");
        let bindings = v["bindings"].as_object().expect("bindings object");
        for (scope, table) in bindings {
            let entries = table.as_object().expect("scope entries object");
            for action_name in entries.keys() {
                assert!(
                    Action::from_id(action_name).is_some(),
                    "unknown action id '{action_name}' in scope '{scope}'"
                );
            }
        }
        // Also verify the total size of Action::ALL matches the documented vocabulary size.
        assert_eq!(Action::ALL.len(), 23);
    }

    #[test]
    fn seed_every_chord_template_parses() {
        let v: serde_json::Value = serde_json::from_str(SEED_JSON).expect("valid json");
        let bindings = v["bindings"].as_object().expect("bindings object");
        for (scope, table) in bindings {
            let entries = table.as_object().expect("scope entries object");
            for (aid, val) in entries {
                let templates: Vec<String> = match val {
                    serde_json::Value::String(s) => vec![s.clone()],
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .map(|x| x.as_str().unwrap().to_string())
                        .collect(),
                    _ => panic!("unexpected value shape in {scope}.{aid}"),
                };
                for t in templates {
                    let probe = if t.contains("{n}") { t.replace("{n}", "1") } else { t.clone() };
                    let chord = parse_chord(&probe);
                    assert!(
                        chord.is_ok(),
                        "failed to parse chord '{t}' in {scope}.{aid}: {:?}",
                        chord.err()
                    );
                }
            }
        }
    }

    #[test]
    fn seed_no_scope_binds_two_actions_to_same_chord() {
        let v: serde_json::Value = serde_json::from_str(SEED_JSON).expect("valid json");
        let bindings = v["bindings"].as_object().expect("bindings object");
        for (scope, table) in bindings {
            let entries = table.as_object().expect("scope entries object");
            let mut seen_chords: HashMap<String, Action> = HashMap::new();
            for (aid, val) in entries {
                let action = Action::from_id(aid).unwrap();
                let first_template = match val {
                    serde_json::Value::String(s) => s.as_str(),
                    serde_json::Value::Array(arr) => arr[0].as_str().unwrap(),
                    _ => panic!("unexpected shape"),
                };
                let probe = first_template.replace("{n}", "1");
                let canonical = parse_chord(&probe).unwrap().display();
                if let Some(prev) = seen_chords.insert(canonical.clone(), action) {
                    panic!(
                        "scope '{scope}' binds both {:?} and {:?} to chord '{canonical}'",
                        prev, action
                    );
                }
            }
        }
    }

    #[test]
    fn seed_every_action_has_family_or_global_binding() {
        for action in Action::ALL {
            let global_has = SEED.lookup("unknown_fallback_app.exe", action, Some(1));
            let mut family_has = false;
            for exe in [
                "chrome.exe",
                "code.exe",
                "explorer.exe",
                "notepad.exe",
                "windowsterminal.exe",
            ] {
                if let Lookup::Found { scope, .. } = SEED.lookup(exe, action, Some(1)) {
                    if matches!(scope, Scope::Family(_) | Scope::Exe(_)) {
                        family_has = true;
                        break;
                    }
                }
            }
            let is_global = matches!(global_has, Lookup::Found { scope: Scope::Global, .. });
            assert!(
                is_global || family_has,
                "action {:?} ({}) must have either a family binding or a global binding",
                action,
                action.id()
            );
        }
    }

    #[test]
    fn terminals_and_consoles_comprehensive_safety_check() {
        let terminal_consoles = [
            "windowsterminal.exe",
            "cmd.exe",
            "powershell.exe",
            "pwsh.exe",
            "conhost.exe",
            "openconsole.exe",
            "mintty.exe",
        ];
        let dangerous_actions = [
            (Action::Copy, "Ctrl+C"),
            (Action::Cut, "Ctrl+X"),
            (Action::Paste, "Ctrl+V"),
            (Action::Undo, "Ctrl+Z"),
            (Action::Redo, "Ctrl+Y"),
            (Action::SelectAll, "Ctrl+A"),
            (Action::Save, "Ctrl+S"),
        ];

        for exe in terminal_consoles {
            for (action, dangerous_chord) in dangerous_actions {
                let res = SEED.lookup(exe, action, None);
                match res {
                    Lookup::Found { chord, scope, .. } => {
                        assert_ne!(
                            scope,
                            Scope::Global,
                            "{exe} must NEVER inherit Scope::Global for {action:?}"
                        );
                        assert_ne!(
                            chord.display(),
                            dangerous_chord,
                            "{exe} must never receive dangerous chord {dangerous_chord} for {action:?}"
                        );
                    }
                    Lookup::NoBinding { known_app, .. } => {
                        assert!(known_app, "{exe} must be recognized as known app");
                    }
                    Lookup::BadArg(_) => unreachable!(),
                }
            }
        }

        // For plain consoles (not Windows Terminal), ALL 7 edit actions must be NoBinding.
        for console_exe in ["cmd.exe", "powershell.exe", "pwsh.exe", "conhost.exe", "openconsole.exe", "mintty.exe"] {
            for (action, _) in dangerous_actions {
                let res = SEED.lookup(console_exe, action, None);
                assert!(
                    matches!(res, Lookup::NoBinding { known_app: true, .. }),
                    "{console_exe} should have NoBinding for {action:?}, got: {res:?}"
                );
            }
        }
    }

    #[test]
    fn precedence_five_concrete_app_cases() {
        let json = r#"{
          "families": {
            "browser": {"exes": ["chrome.exe", "brave.exe"]},
            "vscode": {"exes": ["code.exe", "cursor.exe"]},
            "terminal": {"exes": ["windowsterminal.exe"], "no_global": true},
            "console": {"exes": ["cmd.exe", "powershell.exe"], "no_global": true},
            "explorer": {"exes": ["explorer.exe"]},
            "notepad": {"exes": ["notepad.exe"]}
          },
          "bindings": {
            "*": {
              "copy": "Ctrl+C",
              "save": "Ctrl+S",
              "refresh": "Ctrl+R",
              "find_in_page": "Ctrl+F",
              "cut": "Ctrl+X"
            },
            "browser": {
              "refresh": "F5",
              "new_tab": "Ctrl+T"
            },
            "vscode": {
              "new_tab": "Ctrl+N",
              "close_tab": "Ctrl+W"
            },
            "terminal": {
              "new_tab": "Ctrl+Shift+T",
              "copy": "Ctrl+Shift+C"
            },
            "explorer": {
              "new_tab": "Ctrl+T",
              "address_bar": "Ctrl+L"
            },
            "notepad": {
              "new_tab": "Ctrl+N"
            },
            "brave.exe": {
              "refresh": "Ctrl+Shift+R"
            },
            "cursor.exe": {
              "find_in_page": "Ctrl+K"
            },
            "powershell.exe": {
              "paste": "Ctrl+V"
            },
            "explorer.exe": {
              "address_bar": "Alt+D"
            },
            "notepad.exe": {
              "save": "Ctrl+Shift+S"
            }
          }
        }"#;
        let map = ShortcutMap::from_json(json).expect("valid custom precedence map");

        // Case 1: chrome vs brave sharing browser family.
        // brave.exe has exe override for refresh, chrome uses family, both use global copy.
        assert_eq!(
            map.lookup("brave.exe", Action::Refresh, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+Shift+R").unwrap(),
                scope: Scope::Exe("brave.exe".into()),
                source: Source::Seed,
            }
        );
        assert_eq!(
            map.lookup("chrome.exe", Action::Refresh, None),
            Lookup::Found {
                chord: parse_chord("F5").unwrap(),
                scope: Scope::Family("browser".into()),
                source: Source::Seed,
            }
        );
        assert_eq!(
            map.lookup("chrome.exe", Action::Copy, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+C").unwrap(),
                scope: Scope::Global,
                source: Source::Seed,
            }
        );

        // Case 2: code.exe vs cursor.exe sharing vscode family.
        // cursor.exe has exe override for find_in_page, code.exe falls back to global, both share family new_tab.
        assert_eq!(
            map.lookup("cursor.exe", Action::FindInPage, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+K").unwrap(),
                scope: Scope::Exe("cursor.exe".into()),
                source: Source::Seed,
            }
        );
        assert_eq!(
            map.lookup("code.exe", Action::FindInPage, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+F").unwrap(),
                scope: Scope::Global,
                source: Source::Seed,
            }
        );
        assert_eq!(
            map.lookup("cursor.exe", Action::NewTab, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+N").unwrap(),
                scope: Scope::Family("vscode".into()),
                source: Source::Seed,
            }
        );

        // Case 3: windowsterminal vs cmd.exe vs powershell.exe.
        // powershell.exe has exe override paste, cmd.exe has no binding (no_global),
        // windowsterminal.exe gets terminal family copy.
        assert_eq!(
            map.lookup("powershell.exe", Action::Paste, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+V").unwrap(),
                scope: Scope::Exe("powershell.exe".into()),
                source: Source::Seed,
            }
        );
        assert!(matches!(
            map.lookup("cmd.exe", Action::Paste, None),
            Lookup::NoBinding { known_app: true, .. }
        ));
        assert_eq!(
            map.lookup("windowsterminal.exe", Action::Copy, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+Shift+C").unwrap(),
                scope: Scope::Family("terminal".into()),
                source: Source::Seed,
            }
        );

        // Case 4: explorer.exe: exe override beats family, family beats global, global applies where unbound.
        assert_eq!(
            map.lookup("explorer.exe", Action::AddressBar, None),
            Lookup::Found {
                chord: parse_chord("Alt+D").unwrap(),
                scope: Scope::Exe("explorer.exe".into()),
                source: Source::Seed,
            }
        );
        assert_eq!(
            map.lookup("explorer.exe", Action::NewTab, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+T").unwrap(),
                scope: Scope::Family("explorer".into()),
                source: Source::Seed,
            }
        );
        assert_eq!(
            map.lookup("explorer.exe", Action::Save, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+S").unwrap(),
                scope: Scope::Global,
                source: Source::Seed,
            }
        );

        // Case 5: notepad.exe: exe override for save beats global, family beats global for new_tab, cut falls back.
        assert_eq!(
            map.lookup("notepad.exe", Action::Save, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+Shift+S").unwrap(),
                scope: Scope::Exe("notepad.exe".into()),
                source: Source::Seed,
            }
        );
        assert_eq!(
            map.lookup("notepad.exe", Action::NewTab, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+N").unwrap(),
                scope: Scope::Family("notepad".into()),
                source: Source::Seed,
            }
        );
        assert_eq!(
            map.lookup("notepad.exe", Action::Cut, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+X").unwrap(),
                scope: Scope::Global,
                source: Source::Seed,
            }
        );
    }

    #[test]
    fn unknown_exe_global_fallback_vs_missing_binding() {
        // Universal edit actions fall back to global table for an unknown application.
        assert_eq!(
            SEED.lookup("unknown_tool.exe", Action::Copy, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+C").unwrap(),
                scope: Scope::Global,
                source: Source::Seed,
            }
        );
        assert_eq!(
            SEED.lookup("unknown_tool.exe", Action::Save, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+S").unwrap(),
                scope: Scope::Global,
                source: Source::Seed,
            }
        );
        assert_eq!(
            SEED.lookup("unknown_tool.exe", Action::Undo, None),
            Lookup::Found {
                chord: parse_chord("Ctrl+Z").unwrap(),
                scope: Scope::Global,
                source: Source::Seed,
            }
        );

        // Actions not in global table return NoBinding with known_app=false.
        assert_eq!(
            SEED.lookup("unknown_tool.exe", Action::NewTab, None),
            Lookup::NoBinding { known_app: false, family: None }
        );
        assert_eq!(
            SEED.lookup("unknown_tool.exe", Action::CloseTab, None),
            Lookup::NoBinding { known_app: false, family: None }
        );
        assert_eq!(
            SEED.lookup("unknown_tool.exe", Action::GotoTab, Some(1)),
            Lookup::NoBinding { known_app: false, family: None }
        );
        assert_eq!(
            SEED.lookup("unknown_tool.exe", Action::Refresh, None),
            Lookup::NoBinding { known_app: false, family: None }
        );
    }

    #[test]
    fn known_apps_report_family_and_informative_nobinding() {
        assert_eq!(
            SEED.lookup("notepad.exe", Action::Refresh, None),
            Lookup::NoBinding { known_app: true, family: Some("notepad".into()) }
        );
        assert_eq!(
            SEED.lookup("notepad.exe", Action::GotoTab, Some(1)),
            Lookup::NoBinding { known_app: true, family: Some("notepad".into()) }
        );
        assert_eq!(
            SEED.lookup("notepad.exe", Action::GoBack, None),
            Lookup::NoBinding { known_app: true, family: Some("notepad".into()) }
        );
        assert_eq!(
            SEED.lookup("cmd.exe", Action::Refresh, None),
            Lookup::NoBinding { known_app: true, family: Some("console".into()) }
        );
        assert_eq!(
            SEED.lookup("cmd.exe", Action::Save, None),
            Lookup::NoBinding { known_app: true, family: Some("console".into()) }
        );
    }

    #[test]
    fn case_insensitive_executable_resolution_for_all_families() {
        assert_eq!(SEED.family_of("CHROME.EXE"), Some("browser"));
        assert_eq!(SEED.family_of("Chrome.exe"), Some("browser"));
        assert_eq!(SEED.family_of("CODE.EXE"), Some("vscode"));
        assert_eq!(SEED.family_of("Code - Insiders.exe"), Some("vscode"));
        assert_eq!(SEED.family_of("EXPLORER.EXE"), Some("explorer"));
        assert_eq!(SEED.family_of("NOTEPAD.EXE"), Some("notepad"));
        assert_eq!(SEED.family_of("WINDOWSTERMINAL.EXE"), Some("terminal"));
        assert_eq!(SEED.family_of("POWERSHELL.EXE"), Some("console"));
        assert_eq!(SEED.family_of("PWSH.EXE"), Some("console"));
        assert_eq!(SEED.family_of("OpenConsole.exe"), Some("console"));
        assert_eq!(SEED.family_of("Mintty.exe"), Some("console"));
        assert_eq!(SEED.family_of("NonExistent.exe"), None);
    }

    #[test]
    fn empty_shortcut_map_returns_no_binding_for_all() {
        let empty = ShortcutMap::empty();
        assert_eq!(empty.family_of("chrome.exe"), None);
        assert_eq!(
            empty.lookup("chrome.exe", Action::NewTab, None),
            Lookup::NoBinding { known_app: false, family: None }
        );
        assert!(empty.actions_for("chrome.exe").is_empty());
    }

    // ---- macOS seed (docs/features/155, 156). Pure data checks: they run on every OS, so a typo in
    // seed_macos.json fails the Windows build too.

    fn mac_chord(exe: &str, a: Action, n: Option<u32>) -> Option<String> {
        match SEED_MACOS.lookup(exe, a, n) {
            Lookup::Found { chord, .. } => Some(chord.display()),
            _ => None,
        }
    }

    /// Canonical display form of a chord string, so "Cmd+Shift+Z" compares equal however the modifiers are ordered.
    fn norm(text: &str) -> &'static str {
        Box::leak(parse_chord(text).expect("chord").display().into_boxed_str())
    }

    #[test]
    fn macos_seed_is_valid_and_loaded() {
        ShortcutMap::from_json(SEED_MACOS_JSON).expect("macOS seed must parse and validate");
        assert!(SEED_MACOS.family_of("com.google.chrome").is_some(), "macOS seed actually loaded");
    }

    #[test]
    fn macos_seed_uses_cmd_not_ctrl_for_the_editing_and_tab_basics() {
        // The whole point of a separate table: Ctrl+C on a Mac is not "copy".
        assert_eq!(mac_chord("com.apple.textedit", Action::Copy, None).as_deref(), Some(norm("Cmd+C")));
        assert_eq!(mac_chord("com.apple.textedit", Action::Paste, None).as_deref(), Some(norm("Cmd+V")));
        assert_eq!(mac_chord("com.google.chrome", Action::NewTab, None).as_deref(), Some(norm("Cmd+T")));
        assert_eq!(mac_chord("com.brave.browser", Action::CloseTab, None).as_deref(), Some(norm("Cmd+W")));
        assert_eq!(mac_chord("com.google.chrome", Action::GotoTab, Some(3)).as_deref(), Some(norm("Cmd+3")));
        assert_eq!(mac_chord("com.google.chrome", Action::AddressBar, None).as_deref(), Some(norm("Cmd+L")));
        // Tab switching is Ctrl+Tab in Chromium browsers on macOS (documented), not Cmd.
        assert_eq!(mac_chord("com.google.chrome", Action::NextTab, None).as_deref(), Some(norm("Ctrl+Tab")));
    }

    #[test]
    fn macos_seed_global_table_has_no_ctrl_chords_for_editing() {
        let v: serde_json::Value = serde_json::from_str(SEED_MACOS_JSON).expect("valid json");
        let global = v["bindings"]["*"].as_object().expect("global table");
        for (action, chord) in global {
            let c = chord.as_str().unwrap_or_default();
            assert!(!c.starts_with("Ctrl+"), "global '{action}' = {c}: Ctrl+ is not the Mac modifier");
        }
    }

    #[test]
    fn macos_seed_safari_does_not_inherit_the_chromium_number_row() {
        // Cmd+1..9 is "open favourite N" in Safari unless a setting is changed: no goto_tab_n binding there.
        assert!(!matches!(SEED_MACOS.lookup("com.apple.safari", Action::GotoTab, Some(2)), Lookup::Found { .. }));
        assert_eq!(mac_chord("com.apple.safari", Action::NewTab, None).as_deref(), Some(norm("Cmd+T")));
    }

    #[test]
    fn macos_terminals_do_not_inherit_the_global_table() {
        // Cmd+A / Cmd+S / Cmd+Z in a terminal are not safe defaults; only the explicit terminal table applies.
        for exe in ["com.apple.terminal", "com.googlecode.iterm2"] {
            for a in [Action::SelectAll, Action::Save, Action::Undo, Action::Cut, Action::Redo] {
                assert!(
                    !matches!(SEED_MACOS.lookup(exe, a, None), Lookup::Found { .. }),
                    "{exe} {a:?} must not bind"
                );
            }
            assert_eq!(mac_chord(exe, Action::Paste, None).as_deref(), Some(norm("Cmd+V")));
        }
    }

    #[test]
    fn macos_seed_bundle_ids_are_lowercase_and_not_exe_names() {
        let v: serde_json::Value = serde_json::from_str(SEED_MACOS_JSON).expect("valid json");
        for (fam, def) in v["families"].as_object().expect("families") {
            for e in def["exes"].as_array().expect("exes") {
                let id = e.as_str().expect("string");
                assert_eq!(id, id.to_lowercase(), "{fam}: '{id}' must be lower-case (lookup lower-cases)");
                assert!(!id.ends_with(".exe"), "{fam}: '{id}' is a Windows name");
                assert!(id.contains('.'), "{fam}: '{id}' does not look like a bundle id");
            }
        }
    }

    #[test]
    fn active_seed_is_the_windows_map_off_macos() {
        // Windows behaviour must be exactly as before the macOS map existed.
        if !cfg!(target_os = "macos") {
            assert!(std::ptr::eq(active_seed(), &*SEED));
        } else {
            assert!(std::ptr::eq(active_seed(), &*SEED_MACOS));
        }
    }
}
