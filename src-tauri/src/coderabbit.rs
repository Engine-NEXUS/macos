//! CodeRabbit's official CLI owns the browser login and its local credentials.
//! NEXUS only reads the CLI's structured authentication state.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(90);
static AUTH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRabbitStatus {
    pub state: &'static str,
    pub message: String,
    pub region: Option<String>,
    pub organization: Option<String>,
}

pub(crate) fn cli_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        // 1. Check official per-user install directory
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            for binary_name in &["coderabbit.exe", "cr.exe"] {
                let path = PathBuf::from(&local).join("Programs").join("coderabbit").join(binary_name);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            for binary_name in &["coderabbit.exe", "cr.exe"] {
                let path = PathBuf::from(&profile)
                    .join("AppData")
                    .join("Local")
                    .join("Programs")
                    .join("coderabbit")
                    .join(binary_name);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        // 2. Fallback: check directories on PATH
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                for binary_name in &["coderabbit.exe", "cr.exe"] {
                    let candidate = dir.join(binary_name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        let path = std::env::var_os("PATH")?;
        for name in &["coderabbit", "cr"] {
            if let Some(found) = std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file()) {
                return Some(found);
            }
        }
        None
    }
}

pub(crate) async fn run_cli(
    path: &PathBuf,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    run_cli_at(path, args, timeout, None).await
}

/// `run_cli` with a working directory (a local review reviews the workspace it is started in).
pub(crate) async fn run_cli_at(
    path: &PathBuf,
    args: &[&str],
    timeout: Duration,
    cwd: Option<&std::path::Path>,
) -> Result<std::process::Output, String> {
    let mut cmd = Command::new(path);
    cmd.args(args).kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.as_std_mut().creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| "CodeRabbit did not respond in time. Please try again.".to_string())?
        .map_err(|_| "Could not start the CodeRabbit CLI.".to_string())
}

fn auth_record(stdout: &[u8]) -> Option<Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .last()
}

fn text_field<'a>(record: &'a Value, key: &str) -> Option<&'a str> {
    record
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| record.get("data")?.get(key)?.as_str())
}

fn authenticated(record: &Value) -> Option<bool> {
    for data in [Some(record), record.get("data")].into_iter().flatten() {
        if let Some(value) = data
            .get("authenticated")
            .and_then(Value::as_bool)
            .or_else(|| data.get("isAuthenticated").and_then(Value::as_bool))
        {
            return Some(value);
        }
        if let Some(status) = data.get("status").and_then(Value::as_str) {
            match status.to_ascii_lowercase().as_str() {
                "authenticated" | "connected" => return Some(true),
                "unauthenticated" | "not_authenticated" | "disconnected" => return Some(false),
                _ => {}
            }
        }
    }
    None
}

#[tauri::command]
pub async fn coderabbit_status() -> CodeRabbitStatus {
    let Some(path) = cli_path() else {
        return CodeRabbitStatus {
            state: "not_installed",
            message: "Install the official CodeRabbit CLI to connect your account.".into(),
            region: None,
            organization: None,
        };
    };
    match run_cli(&path, &["auth", "status", "--agent"], STATUS_TIMEOUT).await {
        Ok(output) => {
            let record = auth_record(&output.stdout);
            let positive = record.as_ref().and_then(authenticated);
            let region = record
                .as_ref()
                .and_then(|r| text_field(r, "region"))
                .map(str::to_owned);
            let organization = record
                .as_ref()
                .and_then(|r| {
                    text_field(r, "organizationName").or_else(|| text_field(r, "organization"))
                })
                .map(str::to_owned);
            let (state, message) = match (output.status.success(), positive) {
                (true, Some(true)) => ("connected", "CodeRabbit account connected."),
                (_, Some(false)) => ("disconnected", "Sign in to connect CodeRabbit."),
                _ => (
                    "error",
                    "Could not verify CodeRabbit sign-in. Check the CLI and try again.",
                ),
            };
            CodeRabbitStatus {
                state,
                message: message.into(),
                region,
                organization,
            }
        }
        Err(message) => CodeRabbitStatus {
            state: "error",
            message,
            region: None,
            organization: None,
        },
    }
}

#[tauri::command]
pub async fn coderabbit_login(region: Option<String>) -> Result<CodeRabbitStatus, String> {
    let _auth_guard = AUTH_LOCK
        .try_lock()
        .map_err(|_| "A CodeRabbit account action is already in progress.")?;
    let path = cli_path().ok_or("Install the official CodeRabbit CLI first.")?;
    let mut args = vec!["auth", "login", "--agent"];
    match region.as_deref() {
        Some("eu") => args.extend(["--region", "eu"]),
        Some("us") | None => args.extend(["--region", "us"]),
        _ => return Err("Choose the US or EU region.".into()),
    }
    let output = run_cli(&path, &args, LOGIN_TIMEOUT).await?;
    if !output.status.success() {
        return Err("CodeRabbit sign-in was not completed. Please retry in your browser.".into());
    }
    let status = coderabbit_status().await;
    if status.state != "connected" {
        return Err(status.message);
    }
    Ok(status)
}

#[tauri::command]
pub async fn coderabbit_logout() -> Result<CodeRabbitStatus, String> {
    let _auth_guard = AUTH_LOCK
        .try_lock()
        .map_err(|_| "A CodeRabbit account action is already in progress.")?;
    let path = cli_path().ok_or("CodeRabbit CLI is not installed.")?;
    let output = run_cli(&path, &["auth", "logout", "--agent"], STATUS_TIMEOUT).await?;
    if !output.status.success() {
        return Err("CodeRabbit sign-out failed. Please try again.".into());
    }
    Ok(coderabbit_status().await)
}

#[tauri::command]
pub async fn coderabbit_install() -> Result<CodeRabbitStatus, String> {
    let _auth_guard = AUTH_LOCK
        .try_lock()
        .map_err(|_| "A CodeRabbit account action is already in progress.")?;

    if cli_path().is_some() {
        return Ok(coderabbit_status().await);
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("powershell.exe");
        cmd.args(&[
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "irm https://cli.coderabbit.ai/install.ps1 | iex",
        ])
        .env("CR_CLI_SKIP_LOGIN", "1")
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .kill_on_drop(true);

        let output = tokio::time::timeout(INSTALL_TIMEOUT, cmd.output())
            .await
            .map_err(|_| "CodeRabbit CLI installation timed out. Please try again.".to_string())?
            .map_err(|e| format!("Could not start installer: {e}"))?;

        if !output.status.success() {
            let err_text = String::from_utf8_lossy(&output.stderr);
            let out_text = String::from_utf8_lossy(&output.stdout);
            let msg = if !err_text.trim().is_empty() {
                err_text.to_string()
            } else {
                out_text.to_string()
            };
            return Err(format!(
                "Installation failed: {}",
                msg.chars().take(300).collect::<String>()
            ));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = Command::new("sh");
        cmd.args(&[
            "-c",
            "curl -fsSL https://cli.coderabbit.ai/install.sh | sh",
        ])
        .env("CR_CLI_SKIP_LOGIN", "1")
        .kill_on_drop(true);

        let output = tokio::time::timeout(INSTALL_TIMEOUT, cmd.output())
            .await
            .map_err(|_| "CodeRabbit CLI installation timed out. Please try again.".to_string())?
            .map_err(|e| format!("Could not start installer: {e}"))?;

        if !output.status.success() {
            let err_text = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Installation failed: {}",
                err_text.chars().take(300).collect::<String>()
            ));
        }
    }

    // Settle loop: give filesystem / AV scanners a brief window to flush the binary
    for _ in 0..10 {
        if cli_path().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let status = coderabbit_status().await;
    if status.state == "not_installed" {
        return Err(
            "CodeRabbit installation completed, but the executable was not found. Please try again."
                .into(),
        );
    }
    Ok(status)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRabbitFinding {
    pub id: String,
    pub file: String,
    pub line_range: String,
    /// CodeRabbit's severity: critical | major | minor | trivial | info | unknown.
    pub severity: String,
    /// CodeRabbit's type: potentialIssue | refactor | nitpick | unknown (read from the comment header).
    pub kind: String,
    pub title: String,
    pub description: String,
    pub original_code: Option<String>,
    pub suggested_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRabbitWalkthroughItem {
    pub title: String,
    pub details: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRabbitStats {
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRabbitReviewData {
    pub pr_number: u32,
    pub repo: String,
    pub title: String,
    pub head_branch: String,
    pub base_branch: String,
    /// Head commit of the PR: one CodeRabbit review per commit.
    pub head_sha: String,
    pub verdict: String,
    pub summary: String,
    pub walkthrough: Vec<CodeRabbitWalkthroughItem>,
    pub findings: Vec<CodeRabbitFinding>,
    pub stats: CodeRabbitStats,
}

const GH_API: &str = "https://api.github.com";
const BOT_PREFIX: &str = "coderabbitai";

fn is_bot(v: &Value) -> bool {
    v.pointer("/user/login")
        .and_then(Value::as_str)
        .is_some_and(|l| l.to_lowercase().starts_with(BOT_PREFIX))
}

/// Drops the HTML comments CodeRabbit hides its state in and collapses blank runs.
fn clean_markdown(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    let mut lines: Vec<&str> = Vec::new();
    for l in out.lines() {
        if l.trim().is_empty() && lines.last().is_some_and(|p| p.trim().is_empty()) {
            continue;
        }
        lines.push(l.trim_end());
    }
    lines.join("\n").trim().to_string()
}

fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|x| x.as_str().map(str::to_string)).unwrap_or_else(|| "unknown".into())
}

fn first_title(body: &str) -> String {
    for l in body.lines() {
        let t = l.trim().trim_matches('*').trim();
        if t.is_empty() || t.starts_with('_') || t.starts_with("```") || t.starts_with('<') {
            continue;
        }
        let t = t.trim_matches('*').trim();
        return t.chars().take(140).collect();
    }
    "CodeRabbit comment".to_string()
}

fn suggestion_block(body: &str) -> Option<String> {
    let start = body.find("```suggestion")?;
    let after = &body[start..];
    let nl = after.find('\n')?;
    let end = after[nl + 1..].find("```")?;
    Some(after[nl + 1..nl + 1 + end].trim_end().to_string())
}

/// Pure: turns the four GitHub payloads into the review CodeRabbit itself posted. Nothing is invented;
/// a PR CodeRabbit never touched comes back as `NOT_REVIEWED` with no findings.
pub fn build_review(
    repo: &str,
    pr_number: u32,
    pr: &Value,
    issue_comments: &Value,
    inline: &Value,
    reviews: &Value,
) -> CodeRabbitReviewData {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let summary_body = list(issue_comments)
        .iter()
        .filter(|c| is_bot(c))
        .filter_map(|c| c.get("body").and_then(Value::as_str))
        .map(clean_markdown)
        .find(|b| !b.is_empty());
    let findings: Vec<CodeRabbitFinding> = list(inline)
        .iter()
        .filter(|c| is_bot(c))
        .enumerate()
        .map(|(i, c)| {
            let body = clean_markdown(c.get("body").and_then(Value::as_str).unwrap_or(""));
            let line = c
                .get("line")
                .or_else(|| c.get("original_line"))
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let start = c.get("start_line").and_then(Value::as_u64).unwrap_or(line);
            let (kind, severity) = crate::coderabbit_review::parse_header(&body);
            CodeRabbitFinding {
                id: format!("cr-{}", i + 1),
                file: c.get("path").and_then(Value::as_str).unwrap_or("").to_string(),
                line_range: if start == line { line.to_string() } else { format!("{start}-{line}") },
                severity: enum_str(&severity),
                kind: enum_str(&kind),
                title: first_title(&body),
                suggested_code: suggestion_block(&body),
                original_code: None,
                description: body,
            }
        })
        .collect();
    let verdict = list(reviews)
        .iter()
        .rev()
        .filter(|r| is_bot(r))
        .find_map(|r| match r.get("state").and_then(Value::as_str) {
            Some("CHANGES_REQUESTED") => Some("REQUEST_CHANGES"),
            Some("APPROVED") => Some("APPROVE"),
            Some("COMMENTED") => Some("COMMENT"),
            _ => None,
        })
        .unwrap_or(if summary_body.is_some() || !findings.is_empty() { "COMMENT" } else { "NOT_REVIEWED" });
    let n = |k: &str| pr.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    let s = |p: &str| pr.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
    let reviewed = verdict != "NOT_REVIEWED";
    CodeRabbitReviewData {
        pr_number,
        repo: repo.to_string(),
        title: s("/title"),
        head_branch: s("/head/ref"),
        base_branch: s("/base/ref"),
        head_sha: s("/head/sha"),
        verdict: verdict.to_string(),
        summary: summary_body.unwrap_or_else(|| {
            if reviewed {
                String::new()
            } else {
                "CodeRabbit has not reviewed this pull request yet.".to_string()
            }
        }),
        walkthrough: Vec::new(),
        findings,
        stats: CodeRabbitStats { files_changed: n("changed_files"), additions: n("additions"), deletions: n("deletions") },
    }
}

async fn gh_get(client: &reqwest::Client, token: &str, path: &str) -> Result<Value, String> {
    let resp = client
        .get(format!("{GH_API}{path}"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "nexus-assistant")
        .send()
        .await
        .map_err(|e| format!("GitHub request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub answered {} for {path}", resp.status()));
    }
    resp.json::<Value>().await.map_err(|e| format!("GitHub reply unreadable: {e}"))
}

pub async fn coderabbit_review_pr_data(
    pr_number: u32,
    repo: Option<String>,
) -> Result<CodeRabbitReviewData, String> {
    let Some(repo_name) = repo.filter(|r| !r.trim().is_empty()) else {
        return Err("Which repository is that PR in?".to_string());
    };
    if !repo_name.contains('/') {
        return Err(format!("I need the owner too, like owner/{repo_name}."));
    }
    let token = crate::github_data::resolve_token(None, None).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let base = format!("/repos/{repo_name}");
    let (p_pr, p_issue, p_inline, p_reviews) = (
        format!("{base}/pulls/{pr_number}"),
        format!("{base}/issues/{pr_number}/comments?per_page=100"),
        format!("{base}/pulls/{pr_number}/comments?per_page=100"),
        format!("{base}/pulls/{pr_number}/reviews?per_page=100"),
    );
    let (pr, issue_comments, inline, reviews) = tokio::try_join!(
        gh_get(&client, &token, &p_pr),
        gh_get(&client, &token, &p_issue),
        gh_get(&client, &token, &p_inline),
        gh_get(&client, &token, &p_reviews),
    )?;
    Ok(build_review(&repo_name, pr_number, &pr, &issue_comments, &inline, &reviews))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRabbitPrSummary {
    pub pr_number: u32,
    pub title: String,
    pub author: String,
    pub avatar_url: Option<String>,
    pub head_branch: String,
    pub base_branch: String,
    pub verdict: String,
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
    pub issues_summary: String,
    pub updated_at: String,
}

/// Open pull requests of `repo` with the verdict CodeRabbit gave each (or `NOT_REVIEWED`).
pub async fn list_active_prs(repo: &str) -> Result<Vec<CodeRabbitPrSummary>, String> {
    let token = crate::github_data::resolve_token(None, None).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let prs = gh_get(&client, &token, &format!("/repos/{repo}/pulls?state=open&per_page=10")).await?;
    let mut out = Vec::new();
    for pr in prs.as_array().cloned().unwrap_or_default() {
        let number = pr.get("number").and_then(Value::as_u64).unwrap_or(0) as u32;
        let reviews = gh_get(&client, &token, &format!("/repos/{repo}/pulls/{number}/reviews?per_page=100"))
            .await
            .unwrap_or(Value::Null);
        let rv = build_review(repo, number, &pr, &Value::Null, &Value::Null, &reviews);
        let s = |p: &str| pr.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
        out.push(CodeRabbitPrSummary {
            pr_number: number,
            title: s("/title"),
            author: s("/user/login"),
            avatar_url: pr.pointer("/user/avatar_url").and_then(Value::as_str).map(str::to_string),
            head_branch: s("/head/ref"),
            base_branch: s("/base/ref"),
            issues_summary: if rv.verdict == "NOT_REVIEWED" {
                "Not reviewed by CodeRabbit yet".to_string()
            } else {
                "Reviewed by CodeRabbit".to_string()
            },
            verdict: rv.verdict,
            files_changed: 0,
            additions: 0,
            deletions: 0,
            updated_at: s("/updated_at"),
        });
    }
    Ok(out)
}

/// What CodeRabbit already posted on GitHub, as the common report.
pub fn report_from_review(r: &CodeRabbitReviewData) -> crate::coderabbit_review::Report {
    use crate::coderabbit_review::{parse_severity, Finding, Kind, Report, Source};
    let kind_of = |k: &str| match k {
        "potentialIssue" => Kind::PotentialIssue,
        "refactor" => Kind::Refactor,
        "nitpick" => Kind::Nitpick,
        _ => Kind::Unknown,
    };
    let range = |lr: &str| {
        let mut it = lr.split('-').map(|n| n.trim().parse::<u32>().ok());
        let a = it.next().flatten();
        (a, it.next().flatten().or(a))
    };
    Report {
        source: Source::GithubBot,
        summary: r.summary.clone(),
        findings: r
            .findings
            .iter()
            .map(|f| {
                let (start_line, end_line) = range(&f.line_range);
                Finding {
                    id: f.id.clone(),
                    severity: parse_severity(&f.severity),
                    kind: kind_of(&f.kind),
                    kind_inferred: false,
                    file: f.file.clone(),
                    start_line,
                    end_line,
                    title: f.title.clone(),
                    comment: f.description.clone(),
                    suggestions: f.suggested_code.clone().into_iter().collect(),
                    agent_instructions: None,
                }
            })
            .collect(),
        prompt_for_agents: None,
        gate: None,
        unreviewed_files: 0,
        reused: false,
        notes: Vec::new(),
        usage: None,
    }
}

/// The pull request a workflow run belongs to, if any (`pull_requests[0].number` of the run).
pub async fn pr_number_for_run(repo: &str, run_id: &str, token: &str) -> Option<u32> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().ok()?;
    let run = gh_get(&client, token, &format!("/repos/{repo}/actions/runs/{run_id}")).await.ok()?;
    run.pointer("/pull_requests/0/number").and_then(Value::as_u64).map(|n| n as u32)
}

#[tauri::command]
pub async fn coderabbit_review_pr(
    pr_number: u32,
    repo: Option<String>,
) -> Result<CodeRabbitReviewData, String> {
    coderabbit_review_pr_data(pr_number, repo).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requires_explicit_positive_auth_status() {
        assert_eq!(
            authenticated(&serde_json::json!({"status":"authenticated"})),
            Some(true)
        );
        assert_eq!(
            authenticated(
                &serde_json::json!({"type":"auth_status","data":{"authenticated":false}})
            ),
            Some(false)
        );
        assert_eq!(authenticated(&serde_json::json!({"status":"ok"})), None);
    }

    #[test]
    fn test_cli_path_detection() {
        // The CLI is a per-user install; CI runners do not have it, so only a reported path is checked.
        if let Some(path) = cli_path() {
            assert!(path.is_file(), "cli_path() returned a path that is not a file: {:?}", path);
        }
    }

    fn pr() -> Value {
        json!({"title":"Fix pool","head":{"ref":"feat/x"},"base":{"ref":"main"},
               "changed_files":3,"additions":10,"deletions":2})
    }

    #[test]
    fn a_pr_coderabbit_never_touched_is_not_reviewed_and_has_no_findings() {
        let human = json!([{"user":{"login":"someone"},"body":"lgtm"}]);
        let r = build_review("o/r", 5, &pr(), &human, &json!([]), &json!([]));
        assert_eq!(r.verdict, "NOT_REVIEWED");
        assert!(r.findings.is_empty());
        assert!(r.summary.contains("not reviewed"));
        assert_eq!(r.stats.files_changed, 3);
        assert_eq!(r.head_branch, "feat/x");
    }

    #[test]
    fn bot_comments_become_the_review_with_severity_and_suggestion() {
        let issue = json!([
            {"user":{"login":"someone"},"body":"hi"},
            {"user":{"login":"coderabbitai[bot]"},"body":"<!-- hidden -->\n## Walkthrough\nPool is bounded."}
        ]);
        let inline = json!([{
            "user":{"login":"coderabbitai[bot]"},"path":"src/pool.rs","line":42,"start_line":40,
            "body":"_⚠️ Potential issue_ | _🔴 Critical_\n\n**Connection leak on error.**\n\n```suggestion\npool.recycle(conn);\n```"
        }]);
        let reviews = json!([{"user":{"login":"coderabbitai[bot]"},"state":"CHANGES_REQUESTED"}]);
        let r = build_review("o/r", 5, &pr(), &issue, &inline, &reviews);
        assert_eq!(r.verdict, "REQUEST_CHANGES");
        assert!(r.summary.starts_with("## Walkthrough"));
        assert!(!r.summary.contains("hidden"));
        assert_eq!(r.findings.len(), 1);
        let f = &r.findings[0];
        assert_eq!((f.severity.as_str(), f.file.as_str(), f.line_range.as_str()), ("critical", "src/pool.rs", "40-42"));
        assert_eq!(f.title, "Connection leak on error.");
        assert_eq!(f.suggested_code.as_deref(), Some("pool.recycle(conn);"));
    }

    #[tokio::test]
    async fn review_needs_an_owner_qualified_repo() {
        assert!(coderabbit_review_pr_data(1, None).await.is_err());
        assert!(coderabbit_review_pr_data(1, Some("zync".into())).await.is_err());
    }
}
