//! Lazy STT server manager Î“Ã‡Ã¶ starts the local faster-whisper STT server
//! on-demand when the wake word fires, and kills it after idle to save
//! ~340 MB of RAM at idle.
//!
//! The STT server (server/stt_server.py) uses Moonshine Streaming local
//! takes ~340 MB of RAM. Instead of running it constantly, we:
//!   1. Spawn it when the wake word is detected
//!   2. Kill it after 60 seconds of no transcription requests
//!
//! This saves ~340 MB of RAM at idle (the vast majority of the time).

use std::io::BufRead;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static STT_CHILD: Mutex<Option<Child>> = Mutex::new(None);
static STT_RUNNING: AtomicBool = AtomicBool::new(false);
/// Guard against concurrent ensure_stt_running calls â€” set to true atomically
/// before spawning, so a second caller sees it and returns immediately.
static STT_STARTING: AtomicBool = AtomicBool::new(false);
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

const STT_IDLE_TIMEOUT: Duration = Duration::from_secs(60); // 60s idle timeout
const STT_KEEP_ALIVE: bool = false; // Kill idle STT fallback after 60s to enforce strict <150MB RAM limit
const STT_PORT: u16 = 39217;

/// Get the STT server script path.
fn stt_script_path() -> Option<std::path::PathBuf> {
    // Try several locations relative to the executable and project root
    let candidates = [
        // Development: src-tauri/target/release/../../../server/stt_server.py
        //   current_exe = .../src-tauri/target/release/nexus.exe
        //   parent() = .../target/release
        //   parent() = .../target
        //   parent() = .../src-tauri
        //   parent() = .../ULTRON  (project root)
        //   join("server/stt_server.py") = .../ULTRON/server/stt_server.py
        std::env::current_exe()
            .ok()?
            .parent()?          // target/release
            .parent()?          // target
            .parent()?          // src-tauri
            .parent()?          // project root (ULTRON)
            .join("server")
            .join("stt_server.py"),
        // Fallback: src-tauri/target/release/../../server/stt_server.py
        //   (in case the project root is src-tauri itself)
        std::env::current_exe()
            .ok()?
            .parent()?          // target/release
            .parent()?          // target
            .parent()?          // src-tauri
            .join("server")
            .join("stt_server.py"),
        // Production: <app_dir>/resources/server/stt_server.py
        std::env::current_exe()
            .ok()?
            .parent()?
            .join("resources")
            .join("server")
            .join("stt_server.py"),
    ];

    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }
    None
}

/// Check if the STT server is already running (external or our child).
/// Uses a raw TCP connection to avoid tokio runtime dependency Î“Ã‡Ã¶ this
/// function is called from non-tokio threads (wake-word thread, hotkey handler).
pub(crate) fn is_stt_responsive() -> bool {
    // Use a simple TCP connection + HTTP GET instead of reqwest, which
    // requires a tokio runtime that may not be available on the calling thread.
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let addr = format!("127.0.0.1:{STT_PORT}");
    let timeout = Duration::from_millis(500);

    let stream_result = TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:39217".parse().unwrap()),
        timeout,
    );

    match stream_result {
        Ok(mut stream) => {
            stream.set_read_timeout(Some(timeout)).ok();
            stream.set_write_timeout(Some(timeout)).ok();
            let request = "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
            if stream.write_all(request.as_bytes()).is_err() {
                return false;
            }
            let mut response = Vec::new();
            if stream.read_to_end(&mut response).is_err() {
                return false;
            }
            let response_str = String::from_utf8_lossy(&response);
            // Check for HTTP 200
            response_str.contains("200 OK") || response_str.contains("\"ok\":true")
        }
        Err(_) => false,
    }
}

/// Environment for the local STT child process. Moonshine language is locked
/// here rather than inherited from the operator shell: English-only operation
/// must not depend on ambient process configuration.
fn stt_child_env() -> Vec<(String, String)> {
    vec![
        ("MOONSHINE_MODEL".to_string(), read_moonshine_model()),
        ("MOONSHINE_LANG".to_string(), "en".to_string()),
    ]
}

/// Find a working Python interpreter.
///
/// Search order:
/// 1. `python`, `python3`, `py` on PATH (fast, works if PATH is updated)
/// 2. Windows registry: HKCU/HKLM PythonCore\3.13, 3.12, 3.11, 3.10
/// 3. Common per-user and system install locations
pub fn find_python() -> Option<String> {
    // macOS has its own search: the stock /usr/bin/python3 is an Apple stub that opens an install dialog (card M12).
    #[cfg(target_os = "macos")]
    {
        crate::mac_interpreters::find_python()
    }
    #[cfg(not(target_os = "macos"))]
    {
        find_python_generic()
    }
}

/// The Windows / Linux search (everything `find_python` did before macOS got its own).
#[cfg(not(target_os = "macos"))]
fn find_python_generic() -> Option<String> {
    // 1. Try PATH-based commands â€” verify each actually works
    for cmd in &["python", "python3", "py"] {
        if let Ok(output) = crate::no_window::command(cmd).arg("--version").output() {
            if output.status.success() {
                return Some(cmd.to_string());
            }
        }
    }

    // 2. Check Windows registry for Python install paths
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use winreg::RegKey;
        let hives = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE];
        for &hive in &hives {
            for ver in &["3.13", "3.12", "3.11", "3.10"] {
                let key_path = format!("SOFTWARE\\Python\\PythonCore\\{}\\InstallPath", ver);
                if let Ok(key) = RegKey::predef(hive).open_subkey(&key_path) {
                    if let Ok(install_dir) = key.get_value::<String, _>("") {
                        let exe = std::path::Path::new(&install_dir).join("python.exe");
                        if exe.exists() {
                            tracing::info!("lazy_stt: found Python {} via registry: {}", ver, exe.display());
                            return Some(exe.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }

    // 3. Check common per-user and system install locations
    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            for ver in &["Python313", "Python312", "Python311", "Python310"] {
                candidates.push(std::path::Path::new(&local_appdata).join("Programs").join("Python").join(ver).join("python.exe"));
            }
        }
        if let Ok(prog_files) = std::env::var("ProgramFiles") {
            for ver in &["Python313", "Python312", "Python311", "Python310"] {
                candidates.push(std::path::Path::new(&prog_files).join("Python").join(ver).join("python.exe"));
            }
        }
        for ver in &["Python313", "Python312", "Python311", "Python310"] {
            candidates.push(std::path::PathBuf::from(format!(r"C:\{}\python.exe", ver)));
        }

        for path in candidates {
            if path.exists() {
                if let Ok(output) = crate::no_window::command(&path).arg("--version").output() {
                    if output.status.success() {
                        tracing::info!("lazy_stt: found Python at {}", path.display());
                        return Some(path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    None
}

/// Start the STT server if it's not already running.
/// Called when the wake word fires or when a transcription is needed.
/// Uses STT_STARTING as a compare-and-swap guard to prevent two concurrent
/// callers from both spawning a child process.
pub fn ensure_stt_running() {
    // Already running?
    if STT_RUNNING.load(Ordering::Relaxed) {
        return;
    }

    // Atomically claim the "starting" slot â€” prevents concurrent spawns
    if STT_STARTING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        tracing::debug!("lazy_stt: another thread is already starting STT, skipping");
        return;
    }

    // Check if an external STT server is already running (e.g. started by run.ps1)
    if is_stt_responsive() {
        STT_RUNNING.store(true, Ordering::Relaxed);
        STT_STARTING.store(false, Ordering::SeqCst);
        tracing::info!("lazy_stt: external STT server already running on port {STT_PORT}");
        return;
    }

    // Find the script
    let script = match stt_script_path() {
        Some(p) => p,
        None => {
            STT_STARTING.store(false, Ordering::SeqCst);
            tracing::warn!("lazy_stt: stt_server.py not found â€” skipping (external server may be used)");
            return;
        }
    };

    tracing::info!("lazy_stt: starting STT server ({})", script.display());

    let python_cmd = match find_python() {
        Some(cmd) => cmd,
        None => {
            STT_STARTING.store(false, Ordering::SeqCst);
            tracing::error!(
                "lazy_stt: no Python interpreter found. Install Python 3.12+ and run: \
                 pip install moonshine-voice fastapi uvicorn python-multipart"
            );
            return;
        }
    };

    let child = crate::no_window::command(&python_cmd)
        .arg(&script)
        .envs(stt_child_env())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn();

    match child {
        Ok(mut c) => {
            let pid = c.id();
            // Drain the child's stderr on a dedicated thread: the pipe is
            // otherwise never read (Moonshine/uvicorn tracebacks lost) and
            // a filled 64KB pipe can block the child mid-transcription with
            // zero log evidence. Lines surface as [STT-PY] in the unified
            // console; traceback/error lines escalate to warn.
            if let Some(stderr) = c.stderr.take() {
                std::thread::Builder::new()
                    .name("stt-stderr-drain".into())
                    .spawn(move || {
                        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                            let msg = format!("[STT-PY] {line}");
                            let low = line.to_lowercase();
                            if low.contains("traceback")
                                || low.contains("error")
                                || low.contains("exception")
                                || low.contains("warn")
                            {
                                tracing::warn!("{msg}");
                            } else {
                                tracing::info!("{msg}");
                            }
                        }
                    })
                    .ok();
            }
            *STT_CHILD.lock().unwrap() = Some(c);
            STT_RUNNING.store(true, Ordering::Relaxed);
            STT_STARTING.store(false, Ordering::SeqCst);
            *LAST_REQUEST.lock().unwrap() = Some(Instant::now());
            tracing::info!("lazy_stt: STT server started (PID {pid})");

            // Wait for it to be ready (up to 15 seconds)
            std::thread::spawn(move || {
                for _ in 0..30 {
                    std::thread::sleep(Duration::from_millis(500));
                    if is_stt_responsive() {
                        tracing::info!("lazy_stt: STT server is ready");
                        return;
                    }
                }
                tracing::warn!("lazy_stt: STT server did not become ready in 15s");
            });
        }
        Err(e) => {
            STT_STARTING.store(false, Ordering::SeqCst);
            tracing::error!("lazy_stt: failed to start STT server: {e}");
        }
    }
}

/// Mark that a transcription request was just made (resets the idle timer).
pub fn mark_stt_request() {
    *LAST_REQUEST.lock().unwrap() = Some(Instant::now());
}

/// Idle-kill decision. During a Ghost Mode session the user pauses between
/// commands for a while; killing the sidecar then makes the next command pay a
/// 10–20 s model load. The sidecar only exists here for local-STT users, and it
/// is reclaimed the first check after the session ends.
fn idle_kill_due(idle: Duration, ghost_session_active: bool) -> bool {
    !ghost_session_active && idle > STT_IDLE_TIMEOUT
}

/// Check if the STT server has been idle for too long and kill it.
/// Called periodically from a background thread.
/// NOTE: STT is now kept alive permanently (STT_KEEP_ALIVE=true) because
/// the idle cost is only ~128 MB and killing it adds 10-15s delay on the
/// next command (cold model load). The monitor thread still runs for
/// future use but is a no-op.
pub fn check_stt_idle() {
    if STT_KEEP_ALIVE {
        return; // Never kill â€” keep STT always ready for zero delay
    }

    if !STT_RUNNING.load(Ordering::Relaxed) {
        return;
    }

    let should_kill = {
        let last = LAST_REQUEST.lock().unwrap();
        match *last {
            Some(t) => idle_kill_due(
                Instant::now().duration_since(t),
                crate::ghost::session_active(),
            ),
            None => false,
        }
    };

    if should_kill {
        tracing::info!("lazy_stt: STT server idle for {}s Î“Ã‡Ã¶ killing to save RAM", STT_IDLE_TIMEOUT.as_secs());
        let mut child_guard = STT_CHILD.lock().unwrap();
        if let Some(mut child) = child_guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        STT_RUNNING.store(false, Ordering::Relaxed);
        *LAST_REQUEST.lock().unwrap() = None;
    }
}

/// Start a background thread that periodically checks if STT should be killed.
pub fn start_idle_monitor() {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(10));
            check_stt_idle();
        }
    });
}

/// Pre-warm the local STT server shortly after boot so the first voice
/// command has zero cold-start delay (cold model load is 10-15s).
///
/// Only spawns when cloud STT won't be used (no Groq key or localSttOnly),
/// mirroring the wake-word path's decision in wakeword_oww.rs â€” Groq users
/// pay no RAM. Runs on a background thread; never blocks startup or paint.
pub fn spawn_prewarm(app_data_dir: std::path::PathBuf) {
    std::thread::Builder::new()
        .name("stt-prewarm".into())
        .spawn(move || {
            // Let boot settle (wake engine + first paint win the race).
            std::thread::sleep(Duration::from_secs(20));
            let groq_vault = crate::auth_vault::get_api_key("groq").unwrap_or_default();
            let settings_path = app_data_dir.join("settings.json");
            let (groq_key, local_only) = match std::fs::read_to_string(&settings_path) {
                Ok(content) => {
                    let json: serde_json::Value =
                        serde_json::from_str(&content).unwrap_or_default();
                    let key = if !groq_vault.is_empty() {
                        groq_vault
                    } else {
                        json.get("groqApiKey")
                            .or_else(|| json.get("groq_api_key"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string()
                    };
                    let local = crate::commands::local_stt_only_from_json(&json);
                    (key, local)
                }
                Err(_) => (groq_vault, crate::commands::DEFAULT_LOCAL_STT_ONLY),
            };
            if groq_key.is_empty() || local_only {
                tracing::info!(
                    "lazy_stt: pre-warming local STT server for zero-delay first command"
                );
                ensure_stt_running();
            } else {
                tracing::info!(
                    "lazy_stt: Groq cloud STT configured, skipping pre-warm (saves RAM)"
                );
            }
        })
        .ok();
}

/// Default local STT model. `small_streaming` (123M params, 7.84% WER,
/// ~300 MB) is what fits NEXUS's 600–700 MB local-usage budget alongside the
/// WebView2 stage and the wake engine; `medium_streaming` (245M, 6.65% WER,
/// ~400 MB) is an explicit opt-in for machines with RAM to spare, and
/// `tiny_streaming` (34M, 12% WER, ~100 MB) the ultra-low-RAM choice.
pub const DEFAULT_MOONSHINE_MODEL: &str = "small_streaming";

/// Read the Moonshine model name from settings.json. Falls back to
/// `DEFAULT_MOONSHINE_MODEL`; a user's explicit choice always wins.
fn read_moonshine_model() -> String {
    // Try <NEXUS data dir>/settings.json
    let path = crate::app_dirs::nexus_data_dir().map(|d| d.join("settings.json"));

    let Some(path) = path else {
        return DEFAULT_MOONSHINE_MODEL.to_string();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return DEFAULT_MOONSHINE_MODEL.to_string();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return DEFAULT_MOONSHINE_MODEL.to_string();
    };

    json.get("moonshineModel")
        .or_else(|| json.get("moonshine_model"))
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_MOONSHINE_MODEL)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_idle_kill_waits_for_ghost_session_to_end() {
        let long = STT_IDLE_TIMEOUT + Duration::from_secs(1);
        assert!(idle_kill_due(long, false));
        assert!(!idle_kill_due(long, true), "keep the sidecar warm during a ghost session");
        assert!(!idle_kill_due(Duration::from_secs(5), false));
        assert!(!idle_kill_due(STT_IDLE_TIMEOUT, false), "strictly greater than the timeout");
    }

    /// The local-STT default must stay inside the 600–700 MB budget.
    #[test]
    fn test_default_moonshine_model_is_ram_friendly() {
        assert_eq!(DEFAULT_MOONSHINE_MODEL, "small_streaming");
        assert_eq!(crate::commands::NexusSettings::default().moonshine_model, DEFAULT_MOONSHINE_MODEL);
    }

    #[test]
    fn test_local_child_locks_english_model() {
        let env: HashMap<String, String> = stt_child_env().into_iter().collect();
        assert_eq!(env.get("MOONSHINE_LANG"), Some(&"en".to_string()));
        assert!(!env
            .get("MOONSHINE_MODEL")
            .map(|model| model.is_empty())
            .unwrap_or(true));
    }
}
