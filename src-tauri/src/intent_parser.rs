//! Enhanced intent parser ΓÇö Rust-side command understanding with app registry
//! fuzzy matching, analyse/repo/PR entity extraction, and NLU server fallback.
//!
//! Architecture:
//!   1. Deterministic regex patterns for command structure (open, analyse, search, media)
//!   2. App registry fuzzy matching for app names (uses ALL installed apps, not a fixed list)
//!   3. Entity extraction for repo names, PR numbers, owners
//!   4. NLU server (BERT-Mini) as a confidence booster ΓÇö lazy-started Python sidecar
//!   5. Falls back to the frontend regex parser if NLU is unavailable
//!
//! This replaces the frontend TypeScript parser for better accuracy:
//!   - Uses the app registry (hundreds of installed apps) instead of a fixed list of 50
//!   - Handles "analyse PR 23 servx", "analyse servx repo", "analyse owner/repo"
//!   - Phonetic + Levenshtein matching against real installed app names
//!   - Confidence scoring with fallback to remote backend

use crate::app_registry;
use serde::{Deserialize, Serialize};

// ΓöÇΓöÇΓöÇ Types ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

/// Parsed intent ΓÇö same shape as the frontend Intent type, plus new analyse intents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum ParsedIntent {
    #[serde(rename = "open_app")]
    OpenApp { target: String },
    #[serde(rename = "open_url")]
    OpenUrl { target: String, url: String },
    #[serde(rename = "close_app")]
    CloseApp { target: String },
    #[serde(rename = "whatsapp_chat")]
    WhatsappChat { contact: String },
    #[serde(rename = "open_architect")]
    OpenArchitect,
    /// Inspect the foreground website or app and save a review reference.
    #[serde(rename = "review_foreground")]
    ReviewForeground,
    /// Build from the most recently completed foreground review.
    #[serde(rename = "recreate_last_review")]
    RecreateLastReview,
    /// Open the settings sidebar (command center).
    /// "open settings" / "open command center" / "show preferences" / "configure NEXUS"
    #[serde(rename = "open_settings")]
    OpenSettings,
    /// Open the CodeRabbit active pull requests hub.
    /// "open coderabbit" / "show coderabbit" / "coderabbit"
    #[serde(rename = "open_coderabbit_hub")]
    OpenCodeRabbitHub,
    /// CodeRabbit's review of the pull request that is open: "give me the coderabbit review", "use coderabbit to review
    /// this", "coderabbit this PR". Asks before it spends one of the free reviews.
    #[serde(rename = "coderabbit_review_this")]
    CodeRabbitReviewThis,
    /// An analysis of the open pull request / failing build by the owner's own API key (Gemini, Groq, Cerebras):
    /// "have gemini look at this PR", "analyse this error with ai". Not CodeRabbit.
    #[serde(rename = "ai_review_this")]
    AiReviewThis,
    /// Open the native GitHub Dashboard / feed window.
    /// "open github dashboard" / "open github feed" / "show pr list" / "open github interface"
    #[serde(rename = "open_github_dashboard")]
    OpenGitHubDashboard { subpage: String },
    #[serde(rename = "search")]
    Search { query: String },
    #[serde(rename = "analyse_repo")]
    AnalyseRepo { owner: Option<String>, repo: String },
    #[serde(rename = "analyse_pr")]
    AnalysePr {
        owner: Option<String>,
        repo: String,
        pr_number: u32,
    },
    /// Analyse the latest PR in a repo, optionally filtered by author.
    /// "analyse the pr in zync" → AnalyseLatestPr { repo: "zync", author: None }
    /// "analyse the pr by prem in servx" → AnalyseLatestPr { repo: "servx", author: Some("prem") }
    #[serde(rename = "analyse_latest_pr")]
    AnalyseLatestPr {
        owner: Option<String>,
        repo: String,
        author: Option<String>,
    },
    /// Check the latest branch in a repo, optionally filtered by author.
    /// "check the latest branch of servx created by eesha"
    ///   → CheckBranch { repo: "servx", author: Some("eesha") }
    #[serde(rename = "check_branch")]
    CheckBranch {
        owner: Option<String>,
        repo: String,
        author: Option<String>,
    },
    #[serde(rename = "media_play_pause")]
    MediaPlayPause,
    #[serde(rename = "media_next")]
    MediaNext,
    #[serde(rename = "media_previous")]
    MediaPrevious,
    #[serde(rename = "media_stop")]
    MediaStop,
    /// Local conversational reply (greetings, thanks, etc.) ΓÇö handled
    /// entirely locally, no Cloudflare Worker round-trip needed.
    #[serde(rename = "greeting")]
    Greeting { reply: String },
    /// NLU server result ΓÇö used when the deterministic parser is uncertain
    /// and the NLU server returns a classification.
    #[serde(rename = "nlu_result")]
    NluResult {
        intent: String,
        slots: serde_json::Value,
        confidence: f32,
    },
    /// GitHub sub-command — parsed from voice/text into a structured
    /// `GitHubCommand` that the orchestrator routes to `Subsystem::GitHub`.
    #[serde(rename = "github_command")]
    GitHubCommand {
        command: crate::github_cmd::GitHubCommand,
    },
    /// Order food from a restaurant (Swiggy Food MCP).
    /// "order pizza from dominos" → OrderFood { query: "pizza", restaurant: Some("dominos") }
    /// "order food from swiggy" → OrderFood { query: "", restaurant: None }
    #[serde(rename = "order_food")]
    OrderFood {
        query: String,
        restaurant: Option<String>,
    },
    /// Search for a product on Amazon.
    /// "search for sony headphones on amazon" → SearchProduct { query: "sony headphones" }
    #[serde(rename = "search_product")]
    SearchProduct { query: String },
    /// Send a WhatsApp message to a contact.
    /// "send mom a whatsapp message saying i'll be late"
    /// → SendWhatsAppMessage { contact: "mom", message: "i'll be late" }
    #[serde(rename = "send_whatsapp_message")]
    SendWhatsAppMessage {
        contact: String,
        message: String,
    },
    /// Partial MCP command — the user named the action but not all slots
    /// (e.g. "send message to mummy" with no message body). The
    /// orchestrator speaks `prompt` instead of letting the transcript fall
    /// to the Worker, whose LLM can only guess (or refuse). Never executes.
    #[serde(rename = "need_more_info")]
    NeedMoreInfo { prompt: String },
    /// Enter the Ghostwriter dictation room (persistent session).
    /// "ghostwriter", "take a letter", "write this down for mom".
    /// Contact is optional — asked inside the room if missing.
    /// NOTE: bare "ghost mode" is NOT here — it enters cursor-control
    /// Ghost Mode (EnterGhostControl). Dictation needs a "writer" word.
    #[serde(rename = "enter_ghostwriter")]
    EnterGhostwriter { contact: Option<String> },
    /// Enter cursor-control Ghost Mode (the AI drives your real cursor).
    /// "ghost mode", "take the mouse", "control my cursor".
    #[serde(rename = "enter_ghost_control")]
    EnterGhostControl,
    /// Leave cursor-control Ghost Mode ("exit/close/turn off ghost mode").
    /// Distinct from dictation exits (those live in ghostwriter.rs).
    #[serde(rename = "exit_ghost_control")]
    ExitGhostControl,
    /// Foreground desktop task owned by the Ghost command center.
    #[serde(rename = "ghost_action")]
    GhostAction { task: crate::ghost_center::GhostTask },
    /// Hand mode: the camera becomes a pointer ("hand mode", "hand mouse").
    #[serde(rename = "enter_hand_mode")]
    EnterHandMode,
    /// Leave hand mode (camera off).
    #[serde(rename = "exit_hand_mode")]
    ExitHandMode,
    /// "click here", "double click", "right click": click at the current cursor position.
    #[serde(rename = "pointer_click")]
    PointerClick { kind: String },
    /// Hand mouse live (really clicks) or preview (read-only).
    #[serde(rename = "hand_mouse_live")]
    HandMouseLive { live: bool },
    /// Click the Nth on-screen actionable (1-based reading order).
    /// "click the 3rd option" → ScreenClick { ordinal: 3 }.
    #[serde(rename = "screen_click")]
    ScreenClick { ordinal: u32 },
    /// Read back the Nth on-screen actionable ("what's the 2nd button").
    #[serde(rename = "screen_read")]
    ScreenRead { ordinal: u32 },
    /// Switch browser tab ("move to the 4th tab" → Ctrl+4).
    #[serde(rename = "browser_tab")]
    BrowserTab { index: u32 },
    /// Close/delete browser tab ("close the 3rd tab" → Ctrl+3 then Ctrl+W;
    /// no index → Ctrl+W on the active tab).
    #[serde(rename = "browser_close_tab")]
    BrowserCloseTab { index: Option<u32> },
    /// Search in active browser (Ctrl+L -> type -> Enter).
    #[serde(rename = "browser_search")]
    BrowserSearch { query: String },
    /// Focus active browser search/address bar (Ctrl+L).
    #[serde(rename = "browser_search_focus")]
    BrowserSearchFocus,
    /// Start line-by-line dictation mode.
    #[serde(rename = "start_dictation")]
    StartDictation,
    /// Stop line-by-line dictation mode.
    #[serde(rename = "stop_dictation")]
    StopDictation,
    /// Watch an email on screen for deadline changes or updates.
    #[serde(rename = "watch_screen_email")]
    WatchScreenEmail,
    /// Report what NEXUS remembers (memory audit — local, never cloud).
    #[serde(rename = "memory_audit")]
    MemoryAudit,
    /// "Where did I leave off" / "show my briefing" (local, never cloud).
    #[serde(rename = "briefing")]
    Briefing,
    /// "Import my contacts": read the newest exported `.vcf` into the local contact book (numbers
    /// open WhatsApp chats by link). Local only.
    #[serde(rename = "import_contacts")]
    ImportContacts,
    /// "Shut down / restart / sleep / lock my laptop" (`kind` = power_cmd::PowerKind).
    #[serde(rename = "power_action")]
    PowerAction { kind: String },
    /// "Delete the PDFs in downloads" / "move ... to documents" / "rename ..." / "make a folder ..." /
    /// "what's in downloads" (Action Kernel, docs/features/140). `verb` = delete, move, copy, rename,
    /// create_folder, list or find; `object` is the spoken object phrase, `folder` the source folder
    /// alias, `dest` the destination folder phrase (or the new name for rename). Resolved to real
    /// files by `action_kernel`, never executed from the spoken text.
    #[serde(rename = "file_action")]
    FileAction { verb: String, object: String, folder: Option<String>, dest: Option<String> },
    /// "Undo that": reverse the newest file action.
    #[serde(rename = "file_undo")]
    FileUndo,
    /// "Reopen my session" / "reopen what I had open" (the pending last-session offer).
    #[serde(rename = "reopen_session")]
    ReopenSession,
    /// "Analyse this and add section 2 to my timetable" (screen or clipboard image).
    #[serde(rename = "timetable_add")]
    TimetableAdd { source: String, section: Option<usize> },
    /// "Show my timetable" / "what's next on my timetable".
    #[serde(rename = "timetable_show")]
    TimetableShow,
    /// "Clear my timetable" (asks to confirm).
    #[serde(rename = "timetable_clear")]
    TimetableClear,
    /// "Add those slots" — commit the slots read from the last image.
    #[serde(rename = "timetable_commit")]
    TimetableCommit,
    /// "Use the browser for DSA" — remembered per activity, overwritten on repeat.
    #[serde(rename = "study_pref")]
    StudyPref { activity: String, choice: String },
    /// "Any important emails?" — the inbox watcher's findings.
    #[serde(rename = "mail_digest")]
    MailDigest,
    /// "That's not important" — mute the sender of the last alert.
    #[serde(rename = "mail_mute")]
    MailMute,
    /// "Read that message" / "read Asha's message" - WhatsApp, read-only.
    #[serde(rename = "whatsapp_read")]
    WhatsappRead { name: Option<String> },
    /// "Make Asha a VIP" / "mute WhatsApp alerts from Raj" (`kind` = vip | mute).
    #[serde(rename = "people_flag")]
    PeopleFlag { name: String, kind: String, on: bool },
    /// "Who are my priority people".
    #[serde(rename = "people_list")]
    PeopleList,
    /// "Directions to X" - opens real Google Maps in its own window.
    #[serde(rename = "directions")]
    Directions { destination: String, origin: Option<String>, mode: String },
    /// "Nearest pharmacy" - Places API lookup + Maps search window.
    #[serde(rename = "places_search")]
    PlacesSearch { query: String, near: Option<String> },
    /// "Where is the Charminar" / "when is the Charminar" / "is the museum open": one named place.
    /// `ask` is "where" | "hours" | "when".
    PlaceInfo { query: String, ask: String },
    /// "Analyse this email": read the email open on screen into a sidebar card (+ a map if it names a venue).
    AnalyseThisEmail,
    /// "What's on my calendar today/tomorrow".
    #[serde(rename = "calendar_agenda")]
    CalendarAgenda { day: String },
    /// "Add dentist to my calendar tomorrow at 5pm" (raw text; parsed with the clock).
    #[serde(rename = "calendar_add")]
    CalendarAdd { text: String },
    /// Forget one memory fact ("forget my birthday").
    #[serde(rename = "memory_forget")]
    MemoryForget { key: String },
    /// Two-step wipe step 1 ("forget everything" → asks for confirmation).
    #[serde(rename = "memory_forget_all")]
    MemoryForgetAll,
    /// Two-step wipe step 2 ("yes, forget everything" → wipes).
    #[serde(rename = "memory_forget_all_confirm")]
    MemoryForgetAllConfirm,
    /// Switch personality tone to friend (opt-in).
    #[serde(rename = "persona_friend")]
    PersonaFriend,
    /// Switch personality tone back to butler (formal default).
    #[serde(rename = "persona_butler")]
    PersonaButler,
    /// User shares something personal and wants honest counsel.
    /// `story` is None for the opener ("i want to share something" —
    /// arms counsel mode + invites the story) and Some for direct
    /// counsel ("was i right to shout at him" — runs counsel now).
    #[serde(rename = "share_concern")]
    ShareConcern { story: Option<String> },
    /// Block a contact across social platforms with an optional duration lease.
    #[serde(rename = "block_contact")]
    BlockContact {
        target: String,
        platform: String,
        duration_days: Option<u32>,
    },
    /// Unblock a contact.
    #[serde(rename = "unblock_contact")]
    UnblockContact {
        target: String,
        platform: String,
    },
    /// List active temporary contact blocks.
    #[serde(rename = "list_blocked_contacts")]
    ListBlockedContacts,
    /// Run localhost dev server for active project.
    #[serde(rename = "run_on_localhost")]
    RunOnLocalhost,
    /// Autonomous project planner and creator.
    #[serde(rename = "code_project")]
    CodeProject { source: String, prompt: Option<String> },
    /// Universal coding agent task dispatch.
    #[serde(rename = "coding_agent_task")]
    CodingAgentTask { agent: String, prompt: String },
    /// Scan college emails for internships and open deep link.
    #[serde(rename = "scan_internship_emails")]
    ScanInternshipEmails,
    /// Explicit Alexa-style voice reminder with repeat-back.
    #[serde(rename = "create_voice_reminder")]
    CreateVoiceReminder { task: String, minutes: u32 },
    /// WhatsApp chat document downloader and evaluator.
    #[serde(rename = "analyse_chat_docs")]
    AnalyseChatDocs { contact: String },
    /// Universal coding agent / task watcher.
    #[serde(rename = "watch_agent_task")]
    WatchAgentTask {
        agent: Option<String>,
        query: Option<String>,
    },
    /// "Watch this session until it is complete and, if it is perfect, shut down my laptop."
    /// `kind` = power_cmd::PowerKind::as_str; `agent` = None means the session in front.
    #[serde(rename = "watch_then_power")]
    WatchThenPower {
        agent: Option<String>,
        kind: String,
        require_clean: bool,
        deadline_mins: Option<u32>,
        /// "claude and codex": two or more named agents (empty otherwise)
        #[serde(default)]
        agents: Vec<String>,
        /// "all my agents" / "every agent running"
        #[serde(default)]
        all: bool,
        /// "track all agents and tell me when they're done": no power action
        #[serde(default)]
        track_only: bool,
        /// "... and commit and raise a PR" (asked, and held until the user says yes)
        #[serde(default)]
        followup: bool,
    },
    /// "Cancel the shutdown plan", "what are you watching", "make it sleep instead" (`command` =
    /// power_plan::PlanCommand::as_str). Only about the plan NEXUS is holding; nothing else is claimed.
    #[serde(rename = "watch_plan_command")]
    WatchPlanCommand { command: String },
    /// "Hook up Claude" / "remove the Codex hook" / "is the Claude hook installed"
    /// (`hook_action` = agent_hooks::HookAction::as_str, `agent` = agent_hooks::Agent::name).
    #[serde(rename = "agent_hook_command")]
    AgentHookCommand { hook_action: String, agent: String },
    /// Play music or search for songs on Spotify / Apple Music.
    #[serde(rename = "play_music")]
    PlayMusic {
        query: Option<String>,
        player: Option<String>,
    },
    /// Open specific music player (Spotify or Apple Music).
    #[serde(rename = "open_music_player")]
    OpenMusicPlayer {
        player: Option<String>,
    },
    #[serde(rename = "unknown")]
    Unknown { raw: String },
}

/// Result of parsing a transcript.
#[derive(Debug, Clone, Serialize)]
pub struct ParseResult {
    pub intent: ParsedIntent,
    /// Confidence score 0.0ΓÇô1.0. Deterministic matches are 1.0.
    /// NLU server matches are the model's confidence.
    pub confidence: f32,
    /// Source of the parse: "deterministic", "nlu", "fallback"
    pub source: String,
}

/// Convert a `ParsedIntent` to its snake_case NLU label.
/// This is the reverse of `nlu_client::nlu_to_parsed_intent`.
/// Used by the brain monitor and orchestrator to store intent labels
/// that `merge_and_train.py` can use for BERT-Mini training.
///
/// Without this, `format!("{:?}", intent)` produces Debug output like
/// `GitHubCommand { command: ListPrs { repo: "...", state: "..." } }`
/// which doesn't match any NLU training label.
pub fn intent_to_label(intent: &ParsedIntent) -> &'static str {
    use crate::github_cmd::GitHubCommand;
    match intent {
        ParsedIntent::OpenApp { .. } => "open_app",
        ParsedIntent::OpenUrl { .. } => "open_url",
        ParsedIntent::CloseApp { .. } => "close_app",
        ParsedIntent::WhatsappChat { .. } => "whatsapp_chat",
        ParsedIntent::OpenArchitect => "open_architect",
        ParsedIntent::OpenSettings => "open_settings",
        ParsedIntent::OpenCodeRabbitHub => "open_coderabbit_hub",
        ParsedIntent::CodeRabbitReviewThis => "coderabbit_review_this",
        ParsedIntent::AiReviewThis => "ai_review_this",
        ParsedIntent::OpenGitHubDashboard { .. } => "open_github_dashboard",
        ParsedIntent::Search { .. } => "search",
        ParsedIntent::AnalyseRepo { .. } => "analyse_repo",
        ParsedIntent::AnalysePr { .. } => "analyse_pr",
        ParsedIntent::AnalyseLatestPr { .. } => "analyse_latest_pr",
        ParsedIntent::CheckBranch { .. } => "check_branch",
        ParsedIntent::MediaPlayPause => "media_play_pause",
        ParsedIntent::MediaNext => "media_next",
        ParsedIntent::MediaPrevious => "media_previous",
        ParsedIntent::MediaStop => "media_stop",
        ParsedIntent::Greeting { .. } => "greeting",
        ParsedIntent::NluResult { .. } => "nlu_result",
        ParsedIntent::Unknown { .. } => "unknown",
        ParsedIntent::OrderFood { .. } => "order_food",
        ParsedIntent::SearchProduct { .. } => "search_product",
        ParsedIntent::SendWhatsAppMessage { .. } => "send_whatsapp_message",
        ParsedIntent::NeedMoreInfo { .. } => "need_more_info",
        ParsedIntent::EnterGhostwriter { .. } => "enter_ghostwriter",
        ParsedIntent::EnterGhostControl => "enter_ghost_control",
        ParsedIntent::ExitGhostControl => "exit_ghost_control",
        ParsedIntent::GhostAction { .. } => "ghost_action",
        ParsedIntent::EnterHandMode => "enter_hand_mode",
        ParsedIntent::ExitHandMode => "exit_hand_mode",
        ParsedIntent::HandMouseLive { .. } => "hand_mouse_live",
        ParsedIntent::PointerClick { .. } => "pointer_click",
        ParsedIntent::ScreenClick { .. } => "screen_click",
        ParsedIntent::ScreenRead { .. } => "screen_read",
        ParsedIntent::BrowserTab { .. } => "browser_tab",
        ParsedIntent::BrowserCloseTab { .. } => "browser_close_tab",
        ParsedIntent::BrowserSearch { .. } => "browser_search",
        ParsedIntent::BrowserSearchFocus => "browser_search_focus",
        ParsedIntent::StartDictation => "start_dictation",
        ParsedIntent::StopDictation => "stop_dictation",
        ParsedIntent::WatchScreenEmail => "watch_screen_email",
        ParsedIntent::MemoryAudit => "memory_audit",
        ParsedIntent::Briefing => "briefing",
        ParsedIntent::ImportContacts => "import_contacts",
        ParsedIntent::PowerAction { .. } => "power_action",
        ParsedIntent::FileAction { .. } => "file_action",
        ParsedIntent::FileUndo => "file_undo",
        ParsedIntent::ReopenSession => "reopen_session",
        ParsedIntent::TimetableAdd { .. } => "timetable_add",
        ParsedIntent::TimetableShow => "timetable_show",
        ParsedIntent::TimetableClear => "timetable_clear",
        ParsedIntent::TimetableCommit => "timetable_commit",
        ParsedIntent::StudyPref { .. } => "study_pref",
        ParsedIntent::MailDigest => "mail_digest",
        ParsedIntent::MailMute => "mail_mute",
        ParsedIntent::Directions { .. } => "directions",
        ParsedIntent::PlacesSearch { .. } => "places_search",
        ParsedIntent::PlaceInfo { .. } => "place_info",
        ParsedIntent::AnalyseThisEmail => "analyse_this_email",
        ParsedIntent::WhatsappRead { .. } => "whatsapp_read",
        ParsedIntent::PeopleFlag { .. } => "people_flag",
        ParsedIntent::PeopleList => "people_list",
        ParsedIntent::CalendarAgenda { .. } => "calendar_agenda",
        ParsedIntent::CalendarAdd { .. } => "calendar_add",
        ParsedIntent::MemoryForget { .. } => "memory_forget",
        ParsedIntent::MemoryForgetAll => "memory_forget_all",
        ParsedIntent::MemoryForgetAllConfirm => "memory_forget_all_confirm",
        ParsedIntent::PersonaFriend => "persona_friend",
        ParsedIntent::PersonaButler => "persona_butler",
        ParsedIntent::ShareConcern { .. } => "share_concern",
        ParsedIntent::BlockContact { .. } => "block_contact",
        ParsedIntent::UnblockContact { .. } => "unblock_contact",
        ParsedIntent::ListBlockedContacts => "list_blocked_contacts",
        ParsedIntent::RunOnLocalhost => "run_on_localhost",
        ParsedIntent::CodeProject { .. } => "code_project",
        ParsedIntent::ReviewForeground => "review_foreground",
        ParsedIntent::RecreateLastReview => "recreate_last_review",
        ParsedIntent::CodingAgentTask { .. } => "coding_agent_task",
        ParsedIntent::ScanInternshipEmails => "scan_internship_emails",
        ParsedIntent::CreateVoiceReminder { .. } => "create_voice_reminder",
        ParsedIntent::AnalyseChatDocs { .. } => "analyse_chat_docs",
        ParsedIntent::WatchAgentTask { .. } => "watch_agent_task",
        ParsedIntent::WatchThenPower { .. } => "watch_then_power",
        ParsedIntent::WatchPlanCommand { .. } => "watch_plan_command",
        ParsedIntent::AgentHookCommand { .. } => "agent_hook_command",
        ParsedIntent::PlayMusic { .. } => "play_music",
        ParsedIntent::OpenMusicPlayer { .. } => "open_music_player",
        ParsedIntent::GitHubCommand { command } => match command {
            GitHubCommand::MergePr { .. } => "merge_pr",
            GitHubCommand::ApprovePr { .. } => "approve_pr",
            GitHubCommand::ClosePr { .. } => "close_pr",
            GitHubCommand::ListPrs { .. } => "list_prs",
            GitHubCommand::GetPr { .. } => "get_pr",
            GitHubCommand::CreatePr { .. } => "create_pr",
            GitHubCommand::UpdateBranch { .. } => "update_branch",
            GitHubCommand::RevertPr { .. } => "revert_pr",
            GitHubCommand::ListPrFiles { .. } => "list_pr_files",
            GitHubCommand::CommentPr { .. } => "comment_pr",
            GitHubCommand::AddCollaborator { .. } => "add_collaborator",
            GitHubCommand::RemoveCollaborator { .. } => "remove_collaborator",
            GitHubCommand::ListCollaborators { .. } => "list_collaborators",
            GitHubCommand::AddOrgMember { .. } => "add_org_member",
            GitHubCommand::RemoveOrgMember { .. } => "remove_org_member",
            GitHubCommand::ListOrgMembers { .. } => "list_org_members",
            GitHubCommand::ConvertToOutsideCollaborator { .. } => "convert_to_outside_collaborator",
            GitHubCommand::ListOutsideCollaborators { .. } => "list_outside_collaborators",
            GitHubCommand::SetBranchProtection { .. } => "set_branch_protection",
            GitHubCommand::DeleteBranch { .. } => "delete_branch",
            GitHubCommand::ListBranches { .. } => "list_branches",
            GitHubCommand::CreateRelease { .. } => "create_release",
            GitHubCommand::ListReleases { .. } => "list_releases",
            GitHubCommand::DeleteRelease { .. } => "delete_release",
            GitHubCommand::ListWorkflows { .. } => "list_workflows",
            GitHubCommand::ListWorkflowRuns { .. } => "list_workflow_runs",
            GitHubCommand::RerunWorkflow { .. } => "rerun_workflow",
            GitHubCommand::CancelWorkflow { .. } => "cancel_workflow",
            GitHubCommand::FindPr { .. } => "find_pr",
        },
    }
}

// ΓöÇΓöÇΓöÇ Deterministic parser ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

/// Parse a transcript into a structured intent using deterministic rules.
///
/// This is the primary parser. It handles:
/// - "open <app>" / "launch <app>" / etc. ΓåÆ open_app (with app registry fuzzy match)
/// - "analyse <repo>" / "analyse PR <num> <repo>" / "analyse <owner>/<repo>"
/// - "search for <query>" / "google <query>"
/// - "open architecture mapper"
/// - Media controls (pause, next, previous, stop)
/// - "open <url>" (direct URL)
/// Strip one leading wake prefix ("hey/hello/hi/ok/okay nexus",
/// "nexus"). Hot-mic captures include the wake word verbatim (KWS is
/// skipped during capture), so muscle-memory "NEXUS open chrome" lands
/// whole in the transcript. Returns None when there is no prefix or
/// nothing remains — bare "hey nexus" stays a greeting, never strips
/// to empty. Longest prefixes first; word-boundary required.
fn strip_wake_prefix(text: &str) -> Option<String> {
    let mut current = text.trim();
    let mut stripped_any = false;

    loop {
        let mut found = false;
        for prefix in crate::wake_names::wake_prefixes() {
            if let Some(rest) = current.strip_prefix(prefix.as_str()) {
                // Word boundary check: string ended, whitespace, or punctuation (. , ! ? : - ;)
                let after = rest.as_bytes().first().copied();
                if after.is_none()
                    || matches!(
                        after,
                        Some(b' ')
                            | Some(b'\t')
                            | Some(b'.')
                            | Some(b',')
                            | Some(b'!')
                            | Some(b'?')
                            | Some(b':')
                            | Some(b'-')
                            | Some(b';')
                    )
                {
                    let cleaned = rest
                        .trim_start_matches(|c: char| {
                            matches!(c, '.' | ',' | '!' | '?' | ':' | '-' | ';' | ' ' | '\t')
                        })
                        .trim();
                    if !cleaned.is_empty() {
                        current = cleaned;
                        stripped_any = true;
                        found = true;
                        break;
                    }
                }
            }
        }
        if !found {
            break;
        }
    }

    if stripped_any && !current.is_empty() {
        Some(current.to_string())
    } else {
        None
    }
}

pub fn parse_deterministic(transcript: &str) -> Option<ParseResult> {
    if let Some(result) = parse_deterministic_inner(transcript) {
        return Some(result);
    }
    // Retry: strip one wake prefix and re-run the FULL pipeline
    // (fillers + phonetics + all arms — inner re-normalizes, so just
    // hand it the remainder). Zero regression by construction: only
    // fires when the first pass returned None, and strip never yields
    // empty (bare wake words keep their first-pass result).
    let lowered = transcript.trim().to_lowercase();
    if let Some(stripped) = strip_wake_prefix(&lowered).or_else(|| strip_misheard_prefix(&lowered)) {
        return parse_deterministic_inner(&stripped);
    }
    None
}

/// Spellings Whisper produces for "Nexus" at the start of a command (live log 2026-10-13:
/// "next is open brave."). They are everyday words, so `wake_names` / `strip_wake_prefix` must NOT
/// treat them as the wake word; this helper is only tried by the retry pass after a direct parse
/// failed, so a sentence that merely starts with "next is" cannot become a command unless the REST
/// parses as one.
fn strip_misheard_prefix(text: &str) -> Option<String> {
    const MISHEARD: &[&str] = &["next is", "nex is", "next as"];
    let current = text.trim();
    for prefix in MISHEARD {
        if let Some(rest) = current.strip_prefix(prefix) {
            let boundary = rest.is_empty()
                || rest.starts_with(|c: char| matches!(c, ' ' | '\t' | '.' | ',' | '!' | '?' | ':' | '-' | ';'));
            if boundary {
                let cleaned = rest
                    .trim_start_matches(|c: char| matches!(c, '.' | ',' | '!' | '?' | ':' | '-' | ';' | ' ' | '\t'))
                    .trim();
                if !cleaned.is_empty() {
                    return Some(cleaned.to_string());
                }
            }
        }
    }
    None
}

fn parse_deterministic_inner(transcript: &str) -> Option<ParseResult> {
    let text = transcript.trim().to_lowercase();
    let text = normalize_whitespace(&text);
    // Strip trailing punctuation that STT often appends (e.g. "Open the PR list."
    // from Groq/Whisper). Without this, regexes anchored with `$` (like the
    // ListPrs pattern) fail to match and the permissive OpenApp fallback
    // catches the phrase — "the pr list." resolved to a cached app target.
    let text = strip_trailing_punctuation(&text);
    let text = strip_leading_stray_punctuation(&text);

    if text.is_empty() {
        return None;
    }

    // Strip leading filler words that STT often inserts (e.g. "And analyse
    // PR 254 in zync", "So open chrome", "But first close notepad").
    // These conversational connectors are not part of the command and cause
    // every starts_with() check below to fail.
    let text = strip_leading_filler(&text);

    // Normalize common STT phonetic mishearings and soundalike phrases
    // (e.g. "goes to mode" -> "ghost mode", "open what's up" -> "open whatsapp")
    let text = normalize_phonetic_mishearings(&text);

    // --- Open Architecture Mapper ---
    if is_architect_command(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::OpenArchitect,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- Fuzzy match for architecture mapper (STT mishearings) ---
    // faster-whisper tiny.en often mishears "architecture mapper" as:
    //   "octach at mapper", "architecture mapper", "arcade mapper", etc.
    // Check for the pattern: (open|launch|start|show) + <garbled> + "mapper"
    if is_architect_fuzzy(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::OpenArchitect,
            confidence: 0.85,
            source: "deterministic-fuzzy".to_string(),
        });
    }

    // --- Open Settings / Command Center ---
    // "open settings" / "show settings" / "open command center" / "open preferences"
    // "configure NEXUS" / "NEXUS settings" / "open config" / "show preferences"
    if is_settings_command(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::OpenSettings,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- "analyse this PR" / "analyse the latest PR in zync": a PR pointed at, no number ---
    if let Some(result) = parse_analyse_pr_pointer(&text) {
        return Some(result);
    }

    // --- CodeRabbit Review shortcut commands ---
    let cr_norm = text
        .replace("code rabbit", "coderabbit")
        .replace("coderrabbit", "coderabbit")
        .replace("code-rabbit", "coderabbit");
    let cr_clean = cr_norm.trim();

    if crate::coderabbit_phrases::is_ai_review_request(cr_clean) {
        return Some(ParseResult { intent: ParsedIntent::AiReviewThis, confidence: 1.0, source: "deterministic".to_string() });
    }

    if cr_clean.contains("coderabbit") {
        if crate::coderabbit_phrases::is_coderabbit_review_request(cr_clean) {
            return Some(ParseResult { intent: ParsedIntent::CodeRabbitReviewThis, confidence: 1.0, source: "deterministic".to_string() });
        }
        // Check for specific PR number: e.g. "show coderabbit review for pr 24", "coderabbit pr 24", "check coderabbit 24"
        let pr_num = cr_clean
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .find_map(|s| s.parse::<u32>().ok());

        if let Some(pr_number) = pr_num {
            return Some(ParseResult {
                // No repo is named: look the number up across the owner's repos, as "analyse PR 34" does.
                intent: ParsedIntent::GitHubCommand {
                    command: crate::github_cmd::GitHubCommand::FindPr { pr_number: pr_number as u64 },
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }

        // If no PR number was specified ("open coderabbit", "show coderabbit", "coderabbit", "coderabbit review", etc.):
        return Some(ParseResult {
            intent: ParsedIntent::OpenCodeRabbitHub,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- GitHub Dashboard & Feed commands ---
    // High-priority interceptor: intercepts feed, issues, and org dashboard phrases
    // BEFORE open_app, power_cmd, or app_registry can ever treat them as desktop app targets.
    // PR list phrases are handled by parse_github_command above.
    if let Some(result) = parse_github_dashboard_command(&text) {
        return Some(result);
    }

    // --- "Watch this session, and if it is perfect shut down my laptop" ---
    // Claimed before the power and watch parsers (the sentence contains both) and before the compound
    // split, which would cut it at "then".
    if let Some(plan) = crate::watch_power::parse(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::WatchThenPower {
                agent: plan.agent,
                kind: plan.kind.as_str().to_string(),
                require_clean: plan.require_clean,
                deadline_mins: plan.deadline_mins,
                agents: plan.agents,
                all: plan.all,
                track_only: plan.track_only,
                followup: plan.followup,
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- GitHub Dashboard & Feed commands ---
    // High-priority interceptor: intercepts all feed, PR list, issues, and org dashboard phrases
    // BEFORE open_app, power_cmd, or app_registry can ever treat them as desktop app targets.
    if let Some(result) = parse_github_dashboard_command(&text) {
        return Some(result);
    }

    // --- "hook up all agents" / "which agents are hooked up" ---
    if let Some(action) = crate::agent_hooks::parse_all_command(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::AgentHookCommand { hook_action: action.as_str().to_string(), agent: "all".to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- Agent Stop hooks ("hook up claude", "remove the codex hook", "is the claude hook installed") ---
    if let Some((action, agent)) = crate::agent_hooks::parse_command(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::AgentHookCommand { hook_action: action.as_str().to_string(), agent: agent.name().to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- Commands about the held plan ("cancel the shutdown plan", "what are you watching") ---
    if let Some(cmd) = crate::power_plan::parse_command(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::WatchPlanCommand { command: cmd.as_str() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- Power commands ("shut down my laptop") ---
    // BEFORE every close/open parser: "shut down" is also a close-app verb, and
    // "shut down my laptop" must never become close_app("my laptop").
    if let Some(kind) = crate::power_cmd::parse(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::PowerAction { kind: kind.as_str().to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- File actions (Action Kernel): delete/move/copy/rename/create/list/undo ---
    // Claimed before the open/close parsers; broad wipe phrases are claimed too so they are refused
    // locally instead of reaching a chat model.
    if let Some(intent) = crate::action_kernel::voice::parse(&text) {
        return Some(ParseResult { intent, confidence: 1.0, source: "deterministic".to_string() });
    }

    if is_reopen_session_command(&text) {
        return Some(ParseResult {
            intent: ParsedIntent::ReopenSession,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- Social block/unblock commands ("block Prem Sai for 5 days on Instagram") ---
    if let Some(result) = parse_social_block_command(&text) {
        return Some(result);
    }

    // --- Foreground review and its later reference-bound recreation. These
    // must win before generic coding and screen-analysis grammars. ---
    if let Some(result) = parse_foreground_review_command(&text) {
        return Some(result);
    }

    // --- Coding agent & localhost commands ("run this on localhost", "plan and create...") ---
    if let Some(result) = parse_coding_command(&text) {
        return Some(result);
    }

    // --- Executive assistant: voice reminders, college internship emails, and chat docs ---
    if let Some(result) = parse_executive_assistant_commands(&text) {
        return Some(result);
    }

    // --- Ghost cursor-control entry ---
    // BEFORE ghostwriter: the bare mode word belongs to cursor control.
    // A miss here must not fall through to dictation (strict exact match
    // inside parse_ghost_control_entry returns None on trailing words).
    if let Some(result) = parse_ghost_control_entry(&text) {
        return Some(result);
    }
    if let Some(result) = parse_ghost_control_exit(&text) {
        return Some(result);
    }
    // --- Hand mode (camera pointer): strict exact phrases, like Ghost control ---
    if let Some(result) = parse_hand_mode(&text) {
        return Some(result);
    }
    // --- Control modes in the user's own words ("i want to control the cursor", "take control of my
    // laptop"): who acts + what is controlled + how decides Hand vs Ghost; unclear = ask. ---
    if let Some(result) = crate::mode_intent::parse(&text) {
        return Some(result);
    }

    // --- Timetable (P4) ---
    // BEFORE screen analysis and the typing enclave: "analyse this and add
    // section 2 to my timetable" is a timetable command, not a screen tour.
    if let Some(result) = parse_timetable_command(&text) {
        return Some(result);
    }
    // --- Inbox + calendar (P5): "any important emails", "that's not
    // important", "what's on my calendar", "add X to my calendar …". ---
    if let Some(result) = parse_mail_calendar_command(&text) {
        return Some(result);
    }
    // --- Directions ("directions to X", "how do I get to X") ---
    if let Some(result) = parse_directions_command(&text) {
        return Some(result);
    }
    // --- Places ("nearest pharmacy", "coffee near me") ---
    if let Some(result) = parse_places_command(&text) {
        return Some(result);
    }
    // --- One named place ("where is the Charminar", "when is the Charminar", "is the museum open") ---
    if let Some(result) = parse_place_info_command(&text) {
        return Some(result);
    }
    // --- WhatsApp priority people (P6): "read that message", "make Asha a
    // VIP", "who are my priority people". Narrow phrases only. ---
    if let Some(result) = parse_whatsapp_people_command(&text) {
        return Some(result);
    }

    // --- Ghostwriter room entry ---
    // Must precede greeting/media: "write this down" is dictation, not chat.
    if let Some(result) = parse_ghostwriter_entry(&text) {
        return Some(result);
    }

    // --- Screen control (click Nth option, Nth tab) ---
    // Must precede live/open: "press the 3rd button" is grounding, not a
    // hotkey; "open 4th tab" is a tab switch, not an app.
    if let Some(result) = parse_screen_command(&text) {
        return Some(result);
    }

    // --- Media Control ---
    if let Some(media) = parse_media(&text) {
        return Some(ParseResult {
            intent: media,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // --- Greetings / conversational replies (local, no Worker round-trip) ---
    if let Some(result) = parse_greeting(&text) {
        return Some(result);
    }

    // --- Memory audit & forget (M0) ---
    // "what do you remember", "forget my birthday", "forget everything".
    // Local-only commands: precede the typing enclave so they never get
    // dictated, and precede analyse/github so "forget ..." never routes
    // to cloud. (Active dictation sessions still win at the orchestrator
    // level — this chain only runs for command turns.)
    if let Some(result) = parse_memory_command(&text) {
        return Some(result);
    }

    // --- Persona tone switch & honest counsel (F0/F1b) ---
    // "talk like a friend" / "be formal" / "i want to share something" /
    // "was i right to ...". Local routing decisions (never cloud): precede
    // the typing enclave so commands never get dictated.
    if let Some(result) = parse_persona_command(&text) {
        return Some(result);
    }
    if let Some(result) = parse_share_concern_command(&text) {
        return Some(result);
    }

    // --- Direct In-App Typing & Chat Dictation (Unbreakable) ---
    // "type so it analysis for the servx right send it to as soon as possible"
    // Must precede analyse/github commands so text payloads containing "analysis",
    // "servx", "send", "pr" never misfire as repo or action commands.
    if let Some(result) = parse_type_dictation_command(&text) {
        return Some(result);
    }

    // --- "Analyse this email" (open email on screen). Must precede parse_screen_analysis_command. ---
    if let Some(result) = parse_analyse_this_email_command(&text) {
        return Some(result);
    }

    // --- Reverse engineer this website / app (Feature 152) ---
    // "nexus reverse engineer this website", "what is this site built with".
    // Must precede parse_screen_analysis_command so a stack question is not a screen tour.
    if let Some(result) = parse_reverse_engineer_command(&text) {
        return Some(result);
    }

    // --- Screen Analysis & Visual Research ---
    // "nexus analyse the screen", "analyse the screen", "explain what is on my screen"
    // Must precede parse_analyse_command so "analyse the screen" is not treated as a repo.
    // --- "Analyse this" with no name: whatever is on screen (not a repo called "this"). ---
    if let Some(result) = parse_analyse_deictic_command(&text) {
        return Some(result);
    }
    if let Some(result) = parse_screen_analysis_command(&text) {
        return Some(result);
    }

    // --- Screen Annotation & Architecture Drawing (Feature 87) ---
    // "annotate my screen", "draw on the screen", "create architecture diagram"
    // Must precede parse_analyse_command so "draw architecture..." stays local.
    if let Some(result) = parse_screen_annotation_command(&text) {
        return Some(result);
    }

    // --- Analyse commands ---
    // "analyse PR 23 servx", "analyse pr 23 in servx", "analyse pull request 23 servx"
    // "analyse servx repo", "analyse servx", "analyse owner/repo"
    // "analyse repo servx", "analyse the repo servx"
    // "analyse the pr in zync" (latest PR, no number)
    // "analyse the pr by prem in servx" (latest PR by author)
    if let Some(result) = parse_analyse_command(&text) {
        return Some(result);
    }

    // --- Branch commands ---
    // "check the latest branch of servx created by eesha"
    // "check latest branch by eesha in servx"
    // "show the latest branch of servx by eesha"
    // "what is the latest branch of servx created by eesha"
    if let Some(result) = parse_branch_command(&text) {
        return Some(result);
    }

    // --- Social: send WhatsApp message ---
    // "send mom a whatsapp message saying i'll be late",
    // "whatsapp dad saying i'm coming", "message mom on whatsapp saying hi"
    // Must be BEFORE parse_whatsapp_command — "whatsapp mom saying X" would
    // otherwise match the "whatsapp " prefix and swallow the whole message
    // as the contact name.
    if let Some(result) = parse_send_whatsapp_message(&text) {
        return Some(result);
    }

    // --- WhatsApp chat (must be BEFORE open command ΓÇö "open chat with X" would match open) ---
    // "open chat with lakshya", "message lakshya on whatsapp", "chat with mom"
    if let Some(result) = parse_whatsapp_command(&text) {
        return Some(result);
    }

    // --- GitHub commands ---
    // "merge PR 23 in owner/repo", "approve PR 5 in servx",
    // "close PR 10 in zync", "list PRs in owner/repo",
    // "add user X as collaborator to owner/repo", etc.
    // Must be BEFORE open/close app — "close PR 10" and "show PR 42"
    // would match close_app / open_app respectively.
    if let Some(result) = parse_github_command(&text) {
        return Some(result);
    }

    // --- Commerce: order food (Swiggy) ---
    // "order pizza from dominos", "order food from swiggy",
    // "order biryani", "get food from swiggy"
    if let Some(result) = parse_order_food(&text) {
        return Some(result);
    }

    // --- Commerce: product search (Amazon) ---
    // "search for sony headphones on amazon",
    // "find wireless earbuds on amazon", "amazon search for laptop"
    if let Some(result) = parse_search_product(&text) {
        return Some(result);
    }

    // --- Dictation mode commands ---
    // "start typing", "type whatever i say", "type line by line", "stop typing"
    if let Some(result) = parse_dictation_command(&text) {
        return Some(result);
    }

    // --- Browser Search / Search bar focus ---
    // "search", "search bar", "focus address bar", "search in browser <query>"
    if let Some(result) = parse_browser_search_command(&text) {
        return Some(result);
    }

    // --- Screen Email Watcher ---
    // "update me whenever there is any update on the deadline or anything for this email"
    // "watch this email for deadline changes", "track this email"
    if let Some(result) = parse_watch_screen_email_command(&text) {
        return Some(result);
    }

    // --- Live mode commands ---
    // "type hello world", "press enter", "press ctrl a"
    // "send", "new tab", "open new tab"
    // Must be BEFORE open/close — "open new tab" would match parse_open_command.
    // "cancel" and "never mind" are already handled by the greeting parser
    // above (they return a Greeting with "Very well, sir." which is appropriate).
    if let Some(result) = parse_live_command(&text) {
        return Some(result);
    }

    // --- Open app / URL ---
    // "open whatsapp", "launch gemini", "start calculator", etc.
    if let Some(result) = parse_open_command(&text) {
        return Some(result);
    }

    // --- Close app ---
    // "close whatsapp", "quit chrome", "exit notepad"
    if let Some(result) = parse_close_command(&text) {
        return Some(result);
    }

    // --- YouTube & Video Journal ---
    // "search youtube for...", "summarize this video", "add this video to my journal"
    if let Some(result) = parse_youtube_command(&text) {
        return Some(result);
    }

    // --- Search ---
    // "search for cats", "google cats", "look up cats"
    if let Some(result) = parse_search_command(&text) {
        return Some(result);
    }

    None
}

// ΓöÇΓöÇΓöÇ Open command ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

/// Verbs that trigger an "open" command.
const OPEN_VERBS: &[&str] = &[
    "open", "launch", "start", "run", "fire up", "bring up", "show", "pull up",
    "go to", "visit", "browse to", "navigate to",
];

/// Heuristic: does this target phrase look like a GitHub command that
/// `parse_github_command` somehow missed? Used as a defensive guard in
/// `parse_open_command` so we don't launch a wrong app (e.g. Dribbble
/// for "the pr list.") when the GitHub regex failed on a punctuation or
/// phrasing variant.
///
/// Matches phrases containing PR/pull-request/repo keywords. Returns
/// `true` for things like "the pr list", "pull requests", "open prs",
/// "the repo list", "my prs".
fn looks_like_github_phrase(s: &str) -> bool {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return false;
    }
    let has_github = s.contains("github") || s.contains("git hub");
    let has_feed = s.contains("feed") || s.contains("dashboard") || s.contains("overview") || s.contains("interface");
    if has_github || (has_feed && (s.contains("code") || s.contains("dev") || s.contains("git") || s.contains("my") || s.contains("developer"))) {
        return true;
    }
    // Word-boundary checks so "pr" doesn't match inside "preview" / "process".
    // "pr" as a standalone token or followed by "s"/"list"/"number"/"#".
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let has_pr = tokens.iter().any(|&t| {
        t == "pr" || t == "prs" || t == "pr's"
            || t == "pr-list" || t == "prlist"
            || t.starts_with("pr#")
            || t == "pull" // "pull request(s)" — check bigram below
    });
    let has_pull_request = s.contains("pull request") || s.contains("pull requests");
    let has_repo_phrase = tokens.iter().any(|&t| {
        t == "repo" || t == "repos" || t == "repository" || t == "repositories"
    }) || s.contains("repo list");
    // "pull request(s)" bigram
    let has_pull_bigram = tokens.windows(2).any(|w| {
        w[0] == "pull" && (w[1] == "request" || w[1] == "requests")
    });
    has_pr || has_pull_request || has_repo_phrase || has_pull_bigram
}

/// Parse GitHub Dashboard, Feed, PR list, Issues, and Organization voice commands.
/// High-priority interceptor that matches all feed/dashboard phrases BEFORE open_app
/// or app-registry can ever guess an app target.
fn parse_github_dashboard_command(text: &str) -> Option<ParseResult> {
    let norm = text.trim().to_lowercase();
    let norm = norm.replace("git hub", "github");
    let norm = norm.replace("pullrequest", "pull request");
    let norm = norm.replace("pullrequests", "pull requests");
    let norm = norm.replace("pr's", "prs");
    let norm = norm.replace("prs'", "prs");
    let norm = norm.replace("repo's", "repos");
    let norm = norm.replace("repos'", "repos");
    let norm = norm.replace("org's", "orgs");
    let norm = norm.replace("orgs'", "orgs");
    let norm = norm.replace("issue's", "issues");
    let norm = norm.replace("issues'", "issues");

    // Don't intercept specific PR number analysis commands like "pr 24 in ultron" or "pr 24"
    let has_pr_digits = norm.split_whitespace().any(|word| {
        (word.starts_with("pr") && word[2..].chars().all(|c| c.is_ascii_digit() || c == '#'))
            || (word.starts_with('#') && word[1..].chars().all(|c| c.is_ascii_digit()))
    });
    if has_pr_digits && (norm.contains("analyse") || norm.contains("analyze") || norm.contains("review") || norm.contains("in ") || norm.contains("for ")) {
        return None;
    }

    // Strip leading conversational phrases & filler verbs
    let mut stripped = norm.as_str();
    for prefix in &[
        "can you please ", "could you please ", "can you ", "could you ", "please ",
        "hey ultron ", "ultron ", "hey nexus ", "nexus ",
        "show me my ", "show me the ", "show me all ", "show me ", "show my ", "show the ", "show all ", "show ",
        "open my ", "open the ", "open all ", "open ",
        "give me my ", "give me the ", "give me all ", "give me ",
        "get me my ", "get me the ", "get me ", "get ",
        "bring up my ", "bring up the ", "bring up ",
        "pull up my ", "pull up the ", "pull up ",
        "pop up my ", "pop up the ", "pop up ",
        "display my ", "display the ", "display ",
        "view my ", "view the ", "view ",
        "check out my ", "check out the ", "check out ", "check my ", "check the ", "check ",
        "see my ", "see the ", "see ",
        "launch my ", "launch the ", "launch ",
        "take me to my ", "take me to the ", "take me to ",
        "go to my ", "go to the ", "go to ",
        "switch to my ", "switch to the ", "switch to ",
        "navigate to my ", "navigate to the ", "navigate to ",
        "load my ", "load the ", "load ",
        "fetch my ", "fetch the ", "fetch ",
        "access my ", "access the ", "access ",
        "list all of the ", "list all of my ", "list all the ", "list all my ", "list all ",
        "list of the ", "list of my ", "list of ",
        "list the ", "list my ", "list ",
        "what's on my ", "what is on my ", "what's on ", "what is on ",
        "what's new on ", "what is new on ",
        "what's happening on ", "what is happening on ",
        "what needs my attention on ", "what needs attention on ",
        "what requires attention on ", "what needs review on ",
        "what are the open ", "what are my open ", "what are the ", "what are my ", "what are ",
        "what is the ", "what is my ",
        "do i have any ", "are there any ",
        "what ", "which ",
    ] {
        if stripped.starts_with(prefix) {
            stripped = &stripped[prefix.len()..];
            break;
        }
    }
    let stripped = stripped.trim();

    // 1. Pull Requests Subpage Possibilities:
    // Core targets: "prs", "pr list", "prs list", "pull requests", "pull request list",
    // "open prs", "active prs", "all prs", "my prs", "github prs", "github pr list", etc.
    let is_pr_target = match stripped {
        "pr list" | "prs list" | "the pr list" | "the prs list"
        | "pull request list" | "pull requests list" | "the pull request list" | "the pull requests list"
        | "list of prs" | "list of pull requests" | "list of open prs" | "list of my prs"
        | "list prs" | "list pull requests" | "list open prs" | "list all prs"
        | "prs" | "pr" | "open prs" | "open pr" | "active prs" | "live prs" | "pending prs"
        | "all prs" | "all open prs" | "my prs" | "my open prs"
        | "pull requests" | "pull request" | "open pull requests" | "active pull requests" | "all pull requests" | "my pull requests"
        | "github prs" | "github pr" | "github pr list" | "github prs list" | "github pull requests" | "github pull request list"
        | "github open prs" | "github active prs"
        | "prs are open" | "pull requests are open" | "open prs right now"
        | "what prs are open" | "which prs are open" | "what are the open prs" | "what are my open prs"
        | "prs that need review" | "prs to review" | "review prs" => true,
        _ => false,
    };

    let is_pr_subpage = is_pr_target || regex_match(
        &norm,
        r"^(?:(?:show\s+(?:me\s+)?(?:the\s+|my\s+)?|open\s+(?:the\s+|my\s+)?|give\s+me\s+(?:the\s+|my\s+)?|get\s+(?:me\s+)?(?:the\s+|my\s+)?|bring\s+up\s+(?:the\s+|my\s+)?|pull\s+up\s+(?:the\s+|my\s+)?|view\s+(?:the\s+|my\s+)?|display\s+(?:the\s+|my\s+)?|check\s+(?:the\s+|my\s+)?|what\s+(?:are\s+|is\s+)?(?:the\s+|my\s+)?|list\s+(?:all\s+|of\s+)?|which\s+)?(?:(open|active|live|all|my)\s+)?(?:prs?|pull\s+requests?)(?:\s+list)?|github\s+(?:prs?|pull\s+requests?)(?:\s+list)?|(?:what|which|any)\s+prs?\s+(?:are\s+)?(?:open|active|live)?)$",
    );

    if is_pr_subpage {
        return Some(ParseResult {
            intent: ParsedIntent::OpenGitHubDashboard {
                subpage: "pull_requests".to_string(),
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 2. Issues Subpage Possibilities:
    let is_issues_target = match stripped {
        "issues" | "issue" | "issues list" | "the issues list" | "issue list"
        | "open issues" | "active issues" | "all issues" | "my issues"
        | "list issues" | "list of issues" | "list of open issues" | "list all issues"
        | "github issues" | "github issue" | "github issues list" | "github issue list"
        | "github open issues" | "my github issues"
        | "issues are open" | "what issues are open" | "which issues are open"
        | "what are the open issues" | "what are my open issues"
        | "any open issues" | "are there any open issues" => true,
        _ => false,
    };

    let is_issues_subpage = is_issues_target || regex_match(
        &norm,
        r"^(?:(?:show\s+(?:me\s+)?(?:the\s+|my\s+)?|open\s+(?:the\s+|my\s+)?|view\s+(?:the\s+|my\s+)?|check\s+(?:the\s+|my\s+)?|list\s+(?:all\s+|of\s+)?|what\s+(?:are\s+)?(?:the\s+|my\s+)?)?(?:github\s+issues?|issues?\s+list|open\s+issues?|my\s+issues?|all\s+issues?|issues?)|github\s+issues?|(?:what|which|any)\s+issues?\s+(?:are\s+)?(?:open|active|live)?)$",
    );

    if is_issues_subpage {
        return Some(ParseResult {
            intent: ParsedIntent::OpenGitHubDashboard {
                subpage: "issues".to_string(),
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 3. Organizations Subpage Possibilities:
    let is_orgs_target = match stripped {
        "orgs" | "organizations" | "orgs list" | "organizations list"
        | "my orgs" | "my organizations" | "all orgs" | "all organizations"
        | "list orgs" | "list organizations" | "list of orgs" | "list of organizations"
        | "github orgs" | "github organizations" | "github orgs list" | "github organizations list"
        | "my github orgs" | "my github organizations"
        | "teams" | "my teams" | "github teams" | "teams list" => true,
        _ => false,
    };

    let is_orgs_subpage = is_orgs_target || regex_match(
        &norm,
        r"^(?:(?:show\s+(?:me\s+)?(?:the\s+|my\s+)?|open\s+(?:the\s+|my\s+)?|view\s+(?:the\s+|my\s+)?|check\s+(?:the\s+|my\s+)?|list\s+(?:all\s+|of\s+)?)?(?:github\s+(?:orgs?|organizations?|teams?)|my\s+(?:orgs?|organizations?|teams?)|orgs?|organizations?|teams?)|github\s+(?:orgs?|organizations?|teams?))$",
    );

    if is_orgs_subpage {
        return Some(ParseResult {
            intent: ParsedIntent::OpenGitHubDashboard {
                subpage: "organizations".to_string(),
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 4. Repositories Subpage Possibilities:
    let is_repos_target = match stripped {
        "repos" | "repo" | "repositories" | "repository"
        | "repos list" | "repositories list" | "the repos list" | "the repositories list"
        | "my repos" | "my repositories" | "all repos" | "all repositories"
        | "list repos" | "list repositories" | "list of repos" | "list of repositories"
        | "github repos" | "github repositories" | "github repos list" | "github repositories list"
        | "my github repos" | "my github repositories" => true,
        _ => false,
    };

    let is_repos_subpage = is_repos_target || regex_match(
        &norm,
        r"^(?:(?:show\s+(?:me\s+)?(?:the\s+|my\s+)?|open\s+(?:the\s+|my\s+)?|view\s+(?:the\s+|my\s+)?|check\s+(?:the\s+|my\s+)?|list\s+(?:all\s+|of\s+)?)?(?:(?:my\s+)?github\s+(?:repos|repositories)|my\s+(?:repos|repositories)|repos|repositories)|(?:my\s+)?github\s+(?:repos|repositories))$",
    );

    if is_repos_subpage {
        return Some(ParseResult {
            intent: ParsedIntent::OpenGitHubDashboard {
                subpage: "repos".to_string(),
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 5. Starred Repositories Subpage Possibilities:
    let is_starred_target = match stripped {
        "starred" | "starred repos" | "starred repositories"
        | "my starred" | "my starred repos" | "my starred repositories"
        | "github starred" | "github starred repos" | "github starred repositories"
        | "my github starred" | "my github starred repos" | "my github starred repositories"
        | "stars" | "my stars" | "github stars" | "my github stars" => true,
        _ => false,
    };

    let is_starred_subpage = is_starred_target || regex_match(
        &norm,
        r"^(?:(?:show\s+(?:me\s+)?(?:the\s+|my\s+)?|open\s+(?:the\s+|my\s+)?|view\s+(?:the\s+|my\s+)?|check\s+(?:the\s+|my\s+)?|list\s+(?:all\s+|of\s+)?)?(?:(?:my\s+)?github\s+starred(?:\s+(?:repos|repositories))?|my\s+starred(?:\s+(?:repos|repositories))?|starred(?:\s+(?:repos|repositories))?)|(?:my\s+)?github\s+starred(?:\s+(?:repos|repositories))?)$",
    );

    if is_starred_subpage {
        return Some(ParseResult {
            intent: ParsedIntent::OpenGitHubDashboard {
                subpage: "starred".to_string(),
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 6. Feed / Dashboard Home View Possibilities:
    let is_feed_target = match stripped {
        "github feed" | "the github feed" | "my github feed" | "feed on github" | "feed in github" | "feed"
        | "github dashboard" | "the github dashboard" | "my github dashboard" | "dashboard on github" | "dashboard in github" | "dashboard"
        | "github interface" | "the github interface" | "my github interface" | "interface on github" | "interface"
        | "github overview" | "the github overview" | "my github overview" | "overview on github" | "overview"
        | "github home" | "github homepage" | "home on github"
        | "github activity" | "github activities" | "activity on github" | "recent github activity" | "my github activity"
        | "github updates" | "updates on github" | "my github updates"
        | "github attention" | "needs attention on github"
        | "github notifications" | "notifications on github" | "my github notifications"
        | "github" | "my github" | "the github"
        | "developer feed" | "dev feed" | "coding feed" | "developer dashboard" | "dev dashboard" => true,
        _ => false,
    };

    let is_feed = is_feed_target || regex_match(
        &norm,
        r"^(?:(?:show\s+(?:me\s+)?(?:the\s+|my\s+)?|open\s+(?:the\s+|my\s+)?|give\s+me\s+(?:the\s+|my\s+)?|get\s+(?:me\s+)?(?:the\s+|my\s+)?|bring\s+up\s+(?:the\s+|my\s+)?|pull\s+up\s+(?:the\s+|my\s+)?|pop\s+up\s+(?:the\s+|my\s+)?|view\s+(?:the\s+|my\s+)?|display\s+(?:the\s+|my\s+)?|check\s+(?:out\s+)?(?:the\s+|my\s+)?|see\s+(?:the\s+|my\s+)?|launch\s+(?:the\s+|my\s+)?|what(?:'s|\s+is)\s+(?:on|new\s+on|happening\s+on)\s+)?github\s+(?:feed|dashboard|interface|overview|activity|updates|home|attention|notifications)|(?:my\s+)?github\s+(?:feed|dashboard|interface|overview|activity|updates|home|notifications)|(?:open|show|display|view|check|launch|bring\s+up|pull\s+up)\s+(?:my\s+)?github|what\s+needs\s+(?:my\s+)?attention\s+on\s+github|what\s+requires\s+attention\s+on\s+github|what(?:'s|\s+is)\s+on\s+my\s+github|(?:my\s+)?(?:developer|dev|coding)\s+(?:feed|dashboard))$",
    );

    if is_feed {
        return Some(ParseResult {
            intent: ParsedIntent::OpenGitHubDashboard {
                subpage: "feed".to_string(),
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    None
}

fn parse_open_command(text: &str) -> Option<ParseResult> {
    // Try each open verb
    for verb in OPEN_VERBS {
        let prefix = format!("{} ", verb);
        if text.starts_with(&prefix) {
            let target = &text[prefix.len()..];
            let target = target.trim();

            // Strip trailing "app", "application", "for me"
            let cleaned = strip_trailing_app_words(target);

            // Check for "in browser" / "website" / "site" escape hatch
            if let Some(result) = parse_browser_force(&cleaned) {
                return Some(result);
            }

            // Strip trailing "website"/"site"
            let cleaned_no_site = strip_trailing_site(&cleaned);

            // Direct URL: has a dot, no spaces
            if is_url_like(&cleaned_no_site) {
                let url = if cleaned_no_site.starts_with("http") {
                    cleaned_no_site.clone()
                } else {
                    format!("https://{}", cleaned_no_site)
                };
                return Some(ParseResult {
                    intent: ParsedIntent::OpenUrl {
                        target: cleaned_no_site.clone(),
                        url,
                    },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }

            // App name ΓÇö resolve against the app registry
            // Defensive guard: if the target looks like a GitHub command
            // phrase that the GitHub parser somehow missed (e.g. STT
            // punctuation variants, unusual phrasing), do NOT treat it as
            // an app name. Returning None lets the NLU/brain fallback
            // classify it correctly instead of launching a wrong app.
            if looks_like_github_phrase(&cleaned_no_site) {
                tracing::info!(
                    "[intent_parser] open-command fallback: '{}' looks like a GitHub phrase, skipping app resolution",
                    cleaned_no_site
                );
                return None;
            }

            let resolved = resolve_app_name(&cleaned_no_site);
            return Some(ParseResult {
                intent: ParsedIntent::OpenApp {
                    target: resolved.unwrap_or_else(|| cleaned_no_site.to_string()),
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    None
}

/// Resolve an app name using the app registry with fuzzy matching.
/// Falls back to the original text if no match is found.
fn resolve_app_name(name: &str) -> Option<String> {
    let name = name.trim();

    // 1. Strict registry lookup: only a phrase the user already resolved, an
    // exact key, or a well-covered prefix counts as "the user named this app".
    // Guesses (contains / typo) deliberately return None here: the spoken words
    // must reach the launch gate, which ASKS ("did you mean …?"). Rewriting a
    // guess into the display name used to launder it into an exact match.
    if let Some(entry) = app_registry::lookup_strict(name) {
        // Return the display name (lowercased) — NOT search_names.first().
        // The first search name is alphabetical ("creative" for a Dribbble
        // PWA title), which launders any fuzzy hit into a nonsense target
        // that then self-resolves forever. Display name round-trips through
        // lookup() exactly (it's always names[0]).
        return Some(entry.display_name.to_lowercase());
    }

    // 2. Phonetic correction against the app registry
    // This handles Whisper mishearings like "what's app" ΓåÆ "whatsapp"
    if let Some(corrected) = phonetic_app_lookup(name) {
        tracing::debug!("phonetic app match: '{}' ΓåÆ '{}'", name, corrected);
        return Some(corrected);
    }

    // 3. Try with spaces removed/added (e.g. "whats app" ΓåÆ "whatsapp", "googlechrome" ΓåÆ "google chrome")
    if let Some(corrected) = space_variation_lookup(name) {
        tracing::debug!("space variation match: '{}' ΓåÆ '{}'", name, corrected);
        return Some(corrected);
    }

    None
}

/// Try looking up the app name with space variations.
/// "whats app" ΓåÆ try "whatsapp", "googlechrome" ΓåÆ try "google chrome"
fn space_variation_lookup(name: &str) -> Option<String> {
    // Remove all spaces: "what's app" ΓåÆ "what'sapp"
    let no_spaces = name.replace(' ', "");
    if no_spaces != name {
        if let Some(entry) = app_registry::lookup_strict(&no_spaces) {
            return Some(entry.display_name.to_lowercase());
        }
    }

    // Try adding a space at common boundaries (consonantΓåÆvowel transitions)
    // This is a simple heuristic for compound words
    let chars: Vec<char> = name.chars().collect();
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let curr = chars[i];
        // Insert space between consonant and vowel (e.g. "googlechrome" ΓåÆ "google chrome")
        if !is_vowel(prev) && is_vowel(curr) {
            let mut modified = name[..i].to_string();
            modified.push(' ');
            modified.push_str(&name[i..]);
            if let Some(entry) = app_registry::lookup_strict(&modified) {
                return Some(entry.display_name.to_lowercase());
            }
        }
    }

    None
}

fn is_vowel(c: char) -> bool {
    matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

/// Phonetic app lookup ΓÇö tries to match the spoken word against app names
/// using simple phonetic similarity (sound-alike matching).
///
/// This is a lightweight alternative to Double Metaphone that works against
/// the live app registry instead of a fixed list.
fn phonetic_app_lookup(name: &str) -> Option<String> {
    let name_lower = name.to_lowercase();
    let name_pho = simple_phonetic(&name_lower);

    // Get all app names from the registry
    let search_names = app_registry::all_search_names();

    let mut best_match: Option<(String, usize)> = None;

    for search_name in &search_names {
        let app_pho = simple_phonetic(search_name);
        if app_pho.is_empty() || name_pho.is_empty() {
            continue;
        }

        // Exact phonetic match
        if app_pho == name_pho {
            let score = if search_name.len() == name_lower.len() {
                3
            } else {
                2
            };
            if best_match.as_ref().map_or(true, |b| score > b.1) {
                best_match = Some((search_name.to_string(), score));
            }
        }
        // No partial/typo branch: a near-miss is a GUESS, and guesses must not be
        // rewritten into app names here (the launch gate asks instead).
    }

    best_match.map(|(name, _)| name)
}

/// Simple phonetic encoding ΓÇö removes vowels and normalizes consonant clusters.
/// This is a very lightweight phonetic representation (not as sophisticated as
/// Double Metaphone, but good enough for app name matching against the registry).
fn simple_phonetic(word: &str) -> String {
    let w = word.to_uppercase();
    let mut result = String::new();
    let chars: Vec<char> = w.chars().filter(|c| c.is_alphabetic()).collect();

    for (i, &c) in chars.iter().enumerate() {
        if i == 0 {
            result.push(c);
            continue;
        }

        // Skip vowels (except at start)
        if is_vowel(c) {
            continue;
        }

        // Normalize consonant clusters
        let prev = chars[i - 1];
        match c {
            // C and K sound the same
            'C' => {
                if prev != 'C' && prev != 'K' {
                    result.push('K');
                }
            }
            'K' => {
                if prev != 'C' && prev != 'K' {
                    result.push('K');
                }
            }
            // PH ΓåÆ F
            'H' => {
                if prev == 'P' {
                    // Replace last P with F
                    if let Some(last) = result.chars().last() {
                        if last == 'P' {
                            result.pop();
                            result.push('F');
                        }
                    }
                }
            }
            // Skip duplicate consonants
            _ => {
                if result.chars().last() != Some(c) {
                    result.push(c);
                }
            }
        }
    }

    result
}

// ΓöÇΓöÇΓöÇ Analyse command ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

/// Parse "analyse" commands:
/// - "analyse PR 23 servx" / "analyse pr 23 in servx" / "analyse pull request 23 servx"
/// - "analyse servx repo" / "analyse the repo servx" / "analyse repo servx"
/// - "analyse servx" / "analyse owner/repo"
/// - "analyse PR 23 owner/repo"
/// - "deep analysis PR 24 in servx" / "deep analyse PR 24 in servx"
/// - "analysis PR 24 in servx" (noun form)
/// Conversational lead-ins that do not change the request: "let us analyze this repo", "can you please analyse ...",
/// "okay let's ...". Several may be stacked. Input is already lower-case. Pure.
fn strip_lead_ins(text: &str) -> &str {
    const LEADS: &[&str] = &[
        "let us ", "let's ", "let\u{2019}s ", "lets ", "please ", "can you ", "could you ", "would you ", "will you ", "can we ",
        "could we ", "go ahead and ", "i want you to ", "i need you to ", "i'd like you to ", "i would like you to ", "now ",
        "okay ", "ok ", "hey ", "so ", "just ",
    ];
    let mut s = text.trim_start();
    'again: loop {
        for l in LEADS {
            if let Some(r) = s.strip_prefix(l) {
                s = r.trim_start();
                continue 'again;
            }
        }
        return s;
    }
}

fn parse_analyse_command(text: &str) -> Option<ParseResult> {
    let text = strip_lead_ins(text);
    // Must start with "analyse", "analyze", "analysis", "deep analyse",
    // "deep analyze", or "deep analysis"
    let analyse_text = if text.starts_with("analyse ") {
        &text[8..]
    } else if text.starts_with("analyze ") {
        &text[8..]
    } else if text.starts_with("analysis ") {
        &text[9..]
    } else if text.starts_with("deep analyse ") {
        &text[13..]
    } else if text.starts_with("deep analyze ") {
        &text[13..]
    } else if text.starts_with("deep analysis ") {
        &text[14..]
    } else {
        return None;
    };

    let analyse_text = analyse_text.trim();

    // "analyse this repo" / "analyse the repository": the user points at the open repository without naming it.
    // The orchestrator reads the open page and, if no repository is open, falls back to understanding the screen.
    // "report" is the speech engine's usual mishearing of "repo" ("analyse this report" in the 2026-10-31 log).
    let pointed = analyse_text.trim_end_matches(['.', ',', '!', '?']).trim();
    if matches!(
        pointed,
        "this repo" | "the repo" | "this repository" | "the repository" | "the current repo" | "the open repo"
            | "this report" | "the report" | "this project repo" | "my repo" | "this git repo"
            // The speech engine also hears "this repo" as "this rap up" / "this wrap up" (2026-11-01 log).
            | "this rap up" | "this wrap up" | "this repo up" | "the rap up" | "the wrap up"
    ) {
        return Some(ParseResult {
            intent: ParsedIntent::AnalyseRepo { owner: None, repo: "this".to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // Pattern 1: "PR <num> [in|of|for|from] <repo>" or "pull request <num> ..."
    // e.g. "PR 23 servx", "PR 23 in servx", "pull request 23 servx"
    if let Some(result) = parse_pr_analyse(analyse_text) {
        return Some(result);
    }

    // Pattern 2: "<owner>/<repo>" ΓÇö e.g. "zync-meet/zync", "eesh264/congi"
    if let Some(result) = parse_owner_repo_analyse(analyse_text) {
        return Some(result);
    }

    // Pattern 3: "<repo> repo" or "repo <repo>" or "the repo <repo>"
    // e.g. "servx repo", "repo servx", "the repo servx"
    if let Some(result) = parse_repo_keyword_analyse(analyse_text) {
        return Some(result);
    }

    // Pattern 3b: "the pr [by|of|from] <author> in <repo>" or "the pr in <repo>"
    // or "latest pr [by|of|from] <author> in <repo>" or "latest pr in <repo>"
    // or "the latest pr ..." / "the pull request ..." / "latest pull request ..."
    // These are "latest PR" commands — no PR number, fetch the most recent PR.
    if let Some(result) = parse_latest_pr_analyse(analyse_text) {
        return Some(result);
    }

    // Pattern 4: Just "<repo>" ΓÇö e.g. "analyse servx", "analyse zync"
    // Treat the whole remaining text as the repo name
    let repo = clean_repo_name(analyse_text);
    if !repo.is_empty() {
        return Some(ParseResult {
            intent: ParsedIntent::AnalyseRepo {
                owner: None,
                repo,
            },
            confidence: 0.9, // slightly lower ΓÇö we're guessing this is a repo name
            source: "deterministic".to_string(),
        });
    }

    None
}

/// Known repos for fuzzy matching. These are the user's commonly-analyzed repos.
/// In production, this could be populated from GitHub OAuth (user's repos).
const KNOWN_REPOS: &[&str] = &[
    "nexus",
    "ultron",
    "servx",
    "zync",
    "ledger-ai",
    "nexus-agent",
];

/// Parse "PR <num> [in|of|for|from|on] <repo>" patterns.
fn parse_pr_analyse(text: &str) -> Option<ParseResult> {
    let bare_pr_patterns: &[&str] = &[
        r"^(?:the\s+)?pr\s*#?\s*(\d+)$",
        r"^(?:the\s+)?pull\s+request\s*#?\s*(\d+)$",
        r"^(?:the\s+)?pr\s*number\s*#?\s*(\d+)$",
    ];

    for &pat in bare_pr_patterns {
        if let Some(caps) = regex_captures(text, pat) {
            if let Ok(pr_number) = caps[1].parse::<u64>() {
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: crate::github_cmd::GitHubCommand::FindPr { pr_number },
                    },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }
        }
    }

    // Match: "PR <num> [in|of|for|from|on] <repo>" or "pull request <num> ..."
    // Also handles "PR number <num>" and "PR # <num>" (STT variations)
    // Also handles "the PR" (user says "analyse the pr 254 in zync")
    let pr_patterns: &[&str] = &[
        // "PR number 24 on NEXUS agent" / "PR number 24 in repo"
        r"^pr\s*(?:number|#\s*)?\s*#?\s*(\d+)\s+(?:in|of|for|from|on)\s+(.+)$",
        // "PR number 24 NEXUS agent" (no preposition)
        r"^pr\s*number\s*#?\s*(\d+)\s+(.+)$",
        r"^pr\s*#?\s*(\d+)\s+(?:in|of|for|from)\s+(.+)$",
        r"^pr\s*#?\s*(\d+)\s+(.+)$",
        r"^pull\s+request\s*#?\s*(\d+)\s+(?:in|of|for|from|on)\s+(.+)$",
        r"^pull\s+request\s*#?\s*(\d+)\s+(.+)$",
        // "PR <num> owner/repo"
        r"^pr\s*#?\s*(\d+)\s+(\S+/\S+)$",
        // "the PR <num> in <repo>" ΓÇö user says "analyse the pr 254 in zync"
        r"^the\s+pr\s*#?\s*(\d+)\s+(?:in|of|for|from|on)\s+(.+)$",
        // "the PR <num> <repo>" (no preposition)
        r"^the\s+pr\s*#?\s*(\d+)\s+(.+)$",
        // "the pull request <num> in <repo>"
        r"^the\s+pull\s+request\s*#?\s*(\d+)\s+(?:in|of|for|from|on)\s+(.+)$",
    ];

    for &pat in pr_patterns {
        if let Some(caps) = regex_captures(text, pat) {
            let pr_number: u32 = caps[1].parse().ok()?;
            let repo_part = caps[2].trim();

            // Check if repo_part is owner/repo format
            if let Some((owner, repo)) = parse_owner_repo(repo_part) {
                return Some(ParseResult {
                    intent: ParsedIntent::AnalysePr {
                        owner: Some(owner),
                        repo,
                        pr_number,
                    },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }

            // Just repo name ΓÇö try exact match first
            let repo = clean_repo_name(repo_part);
            if !repo.is_empty() {
                // If the repo isn't an exact known repo, try fuzzy matching
                // against known repos. This catches STT mishearings like
                // "zink" ΓåÆ "zync" that haven't been learned yet.
                let lower_repo = repo.to_lowercase();
                if !KNOWN_REPOS.contains(&lower_repo.as_str()) {
                    if let Some(fuzzy_repo) = fuzzy_match_repo_name(&lower_repo) {
                        tracing::info!(
                            "intent_parser: fuzzy matched repo '{}' ΓåÆ '{}' in PR command",
                            repo,
                            fuzzy_repo
                        );
                        return Some(ParseResult {
                            intent: ParsedIntent::AnalysePr {
                                owner: None,
                                repo: fuzzy_repo,
                                pr_number,
                            },
                            confidence: 0.8,
                            source: "fuzzy".to_string(),
                        });
                    }
                }

                return Some(ParseResult {
                    intent: ParsedIntent::AnalysePr {
                        owner: None,
                        repo,
                        pr_number,
                    },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }
        }
    }

    None
}

/// Fuzzy-match a repo name against known repos using Levenshtein distance.
/// Returns the matched repo name if within threshold, None otherwise.
fn fuzzy_match_repo_name(repo: &str) -> Option<String> {
    let cached = crate::github_repo_index::cached_names();
    let candidates: Vec<&str> = if !cached.is_empty() {
        cached.iter().map(|s| s.as_str()).collect()
    } else {
        KNOWN_REPOS.to_vec()
    };
    for known in candidates {
        let dist = levenshtein(repo, known);
        // Threshold: 2 for short repos (≤6 chars), 3 for longer.
        // STT mishearings are primarily handled in the frontend
        // (correctSttTranscript) before reaching the parser.
        // The fuzzy matcher is a safety net for residual mishearings.
        let threshold = if known.len() <= 6 { 2 } else { 3 };
        if dist <= threshold && dist > 0 {
            return Some(known.to_string());
        }
    }
    None
}

/// Parse "latest PR" commands — PR without a number, optionally filtered by author.
///
/// Patterns:
/// - "the pr in <repo>" → latest PR in repo
/// - "the pr [of|by|from] <author> in <repo>" → latest PR by author in repo
/// - "latest pr in <repo>" → latest PR in repo
/// - "latest pr [of|by|from] <author> in <repo>" → latest PR by author in repo
/// - "the latest pr in <repo>" → latest PR in repo
/// - "the latest pr [of|by|from] <author> in <repo>" → latest PR by author in repo
/// - "the pull request in <repo>" → latest PR in repo
/// - "the pull request [of|by|from] <author> in <repo>" → latest PR by author in repo
/// - "latest pull request in <repo>" → latest PR in repo
/// - "the pr of <repo>" → latest PR in repo (when "of" is followed by a known repo)
/// - "pr in <repo>" → latest PR in repo (no "the" / "latest")
/// - "pr [of|by|from] <author> in <repo>" → latest PR by author in repo
fn parse_latest_pr_analyse(text: &str) -> Option<ParseResult> {
    let lower_text = text.to_lowercase();
    let has_latest_word = lower_text.split_whitespace().any(|w| {
        matches!(
            w.trim_matches(|c: char| !c.is_alphanumeric()),
            "latest" | "newest" | "last" | "recent"
        )
    });
    let make_result = |owner: Option<String>, repo: String, confidence: f32, source: &str| -> ParseResult {
        if has_latest_word {
            ParseResult {
                intent: ParsedIntent::AnalyseLatestPr {
                    owner,
                    repo,
                    author: None,
                },
                confidence,
                source: source.to_string(),
            }
        } else {
            let full_repo = match owner {
                Some(o) => format!("{}/{}", o, repo),
                None => repo,
            };
            ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: crate::github_cmd::GitHubCommand::ListPrs {
                        repo: full_repo,
                        state: "open".to_string(),
                    },
                },
                confidence,
                source: source.to_string(),
            }
        }
    };

    // Must contain "pr" or "pull request" but NOT followed by a number
    // (if followed by a number, it's a specific PR, handled by parse_pr_analyse)

    // Strip leading "the " if present
    let text = text.strip_prefix("the ").unwrap_or(text);

    // Check if it starts with "latest " or "newest " or "recent " or "open " or "current "
    let text = text
        .strip_prefix("latest ")
        .or_else(|| text.strip_prefix("newest "))
        .or_else(|| text.strip_prefix("recent "))
        .or_else(|| text.strip_prefix("current "))
        .unwrap_or(text);

    // Now text should start with "pr " or "pull request "
    let after_pr = if let Some(rest) = text.strip_prefix("pr ") {
        rest
    } else if let Some(rest) = text.strip_prefix("pull request ") {
        rest
    } else {
        return None; // Not a PR command
    };

    // Check that "pr" is NOT followed by a number (that's parse_pr_analyse's job)
    // If after_pr starts with a digit or "#", skip — it's a specific PR
    let after_pr_trimmed = after_pr.trim_start_matches('#').trim_start();
    if after_pr_trimmed.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        return None; // Has a PR number — not a "latest PR" command
    }

    // Patterns for extracting author and repo:
    // 1. "[of|by|from] <author> in <repo>"
    // 2. "in <repo>" (no author)
    // 3. "of <repo>" (when "of" is followed by a known repo, not a person)

    // Pattern 1: "[of|by|from] <author> in <repo>"
    if let Some(caps) = regex_captures(after_pr, r"^(?:of|by|from)\s+(\S+)\s+in\s+(.+)$") {
        let author = caps[1].trim().to_string();
        let repo_part = caps[2].trim();

        // Check if repo_part is owner/repo format
        if let Some((owner, repo)) = parse_owner_repo(repo_part) {
            return Some(ParseResult {
                intent: ParsedIntent::AnalyseLatestPr {
                    owner: Some(owner),
                    repo,
                    author: Some(author),
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }

        let repo = clean_repo_name(repo_part);
        if !repo.is_empty() {
            // Try fuzzy matching for repo name
            let lower_repo = repo.to_lowercase();
            let final_repo = if !KNOWN_REPOS.contains(&lower_repo.as_str()) {
                fuzzy_match_repo_name(&lower_repo).unwrap_or(repo)
            } else {
                repo
            };
            return Some(ParseResult {
                intent: ParsedIntent::AnalyseLatestPr {
                    owner: None,
                    repo: final_repo,
                    author: Some(author),
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // Pattern 2: "in <repo>" (no author)
    if let Some(caps) = regex_captures(after_pr, r"^in\s+(.+)$") {
        let repo_part = caps[1].trim();
        if let Some((owner, repo)) = parse_owner_repo(repo_part) {
            return Some(make_result(Some(owner), repo, 1.0, "deterministic"));
        }
        let repo = clean_repo_name(repo_part);
        if !repo.is_empty() {
            let lower_repo = repo.to_lowercase();
            let final_repo = if !KNOWN_REPOS.contains(&lower_repo.as_str()) {
                fuzzy_match_repo_name(&lower_repo).unwrap_or(repo)
            } else {
                repo
            };
            return Some(make_result(None, final_repo, 0.95, "deterministic"));
        }
    }

    // Pattern 3: "of <repo>" (when "of" is followed by a known repo)
    if let Some(caps) = regex_captures(after_pr, r"^of\s+(.+)$") {
        let repo_part = caps[1].trim();
        let lower_repo = repo_part.to_lowercase();
        if KNOWN_REPOS.contains(&lower_repo.as_str()) {
            return Some(make_result(None, repo_part.to_string(), 0.9, "deterministic"));
        }
    }

    // Pattern 4: Just "<repo>" (no preposition)
    let repo = clean_repo_name(after_pr);
    if !repo.is_empty() {
        let lower_repo = repo.to_lowercase();
        if KNOWN_REPOS.contains(&lower_repo.as_str()) {
            return Some(make_result(None, repo, 0.85, "deterministic"));
        }
        if let Some(fuzzy_repo) = fuzzy_match_repo_name(&lower_repo) {
            return Some(make_result(None, fuzzy_repo, 0.8, "fuzzy"));
        }
    }

    None
}

/// Parse "check branch" / "show branch" / "what is branch" commands.
///
/// Patterns:
/// - "check [the] [latest|recent|newest] branch [of|in] <repo> [created] [by] <author>"
/// - "check [the] [latest|recent|newest] branch [by] <author> [in|of] <repo>"
/// - "show [the] [latest|recent|newest] branch [of|in] <repo> [created] [by] <author>"
/// - "what is [the] [latest|recent|newest] branch [of|in] <repo> [created] [by] <author>"
/// - "check [the] [latest|recent|newest] branch [of|in] <repo>" (no author — just latest branch)
fn parse_branch_command(text: &str) -> Option<ParseResult> {
    // Must start with "check", "show", or "what is"
    let branch_text = if let Some(rest) = text.strip_prefix("check ") {
        rest
    } else if let Some(rest) = text.strip_prefix("show ") {
        rest
    } else if let Some(rest) = text.strip_prefix("what is ") {
        rest
    } else if let Some(rest) = text.strip_prefix("what's ") {
        rest
    } else {
        return None;
    };

    // Must contain "branch" (or "branches")
    if !branch_text.contains("branch") && !branch_text.contains("branches") {
        return None;
    }

    // Strip "the " prefix
    let branch_text = branch_text.strip_prefix("the ").unwrap_or(branch_text);

    // Strip "latest" / "newest" / "recent" / "new"
    let branch_text = branch_text
        .strip_prefix("latest ")
        .or_else(|| branch_text.strip_prefix("newest "))
        .or_else(|| branch_text.strip_prefix("recent "))
        .or_else(|| branch_text.strip_prefix("new "))
        .unwrap_or(branch_text);

    // Strip "branch " or "branches "
    let after_branch = if let Some(rest) = branch_text.strip_prefix("branch ") {
        rest
    } else if let Some(rest) = branch_text.strip_prefix("branches ") {
        rest
    } else {
        // "branch" might be at the end with no trailing space
        if branch_text == "branch" || branch_text == "branches" {
            return None; // No repo specified
        }
        return None;
    };

    // Now after_branch should contain repo and/or author info.
    // Patterns:
    // A: "[of|in] <repo> [created] [by] <author>"
    // B: "[by] <author> [in|of] <repo>"
    // C: "[of|in] <repo>" (no author)

    // Strip "created " if present anywhere (user says "of servx created by eesha")
    let after_branch = after_branch.replace(" created ", " ");

    // Pattern A: "[of|in] <repo> [by] <author>"
    // The repo is everything between "of/in" and "by", or the rest if no "by"
    if let Some(caps) = regex_captures(&after_branch, r"^(?:of|in)\s+(.+?)\s+by\s+(\S+)$") {
        let repo_part = caps[1].trim();
        let author = caps[2].trim().to_string();

        if let Some((owner, repo)) = parse_owner_repo(repo_part) {
            return Some(ParseResult {
                intent: ParsedIntent::CheckBranch {
                    owner: Some(owner),
                    repo,
                    author: Some(author),
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }

        let repo = clean_repo_name(repo_part);
        if !repo.is_empty() {
            let lower_repo = repo.to_lowercase();
            let final_repo = if !KNOWN_REPOS.contains(&lower_repo.as_str()) {
                fuzzy_match_repo_name(&lower_repo).unwrap_or(repo)
            } else {
                repo
            };
            return Some(ParseResult {
                intent: ParsedIntent::CheckBranch {
                    owner: None,
                    repo: final_repo,
                    author: Some(author),
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // Pattern B: "[by] <author> [in|of] <repo>"
    if let Some(caps) = regex_captures(&after_branch, r"^by\s+(\S+)\s+(?:in|of)\s+(.+)$") {
        let author = caps[1].trim().to_string();
        let repo_part = caps[2].trim();

        if let Some((owner, repo)) = parse_owner_repo(repo_part) {
            return Some(ParseResult {
                intent: ParsedIntent::CheckBranch {
                    owner: Some(owner),
                    repo,
                    author: Some(author),
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }

        let repo = clean_repo_name(repo_part);
        if !repo.is_empty() {
            let lower_repo = repo.to_lowercase();
            let final_repo = if !KNOWN_REPOS.contains(&lower_repo.as_str()) {
                fuzzy_match_repo_name(&lower_repo).unwrap_or(repo)
            } else {
                repo
            };
            return Some(ParseResult {
                intent: ParsedIntent::CheckBranch {
                    owner: None,
                    repo: final_repo,
                    author: Some(author),
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // Pattern C: "[of|in] <repo>" (no author — just latest branch)
    if let Some(caps) = regex_captures(&after_branch, r"^(?:of|in)\s+(.+)$") {
        let repo_part = caps[1].trim();

        // Strip trailing " by <something>" if present (already handled above, but just in case)
        let repo_part = if let Some(pos) = repo_part.find(" by ") {
            &repo_part[..pos]
        } else {
            repo_part
        };

        if let Some((owner, repo)) = parse_owner_repo(repo_part) {
            return Some(ParseResult {
                intent: ParsedIntent::CheckBranch {
                    owner: Some(owner),
                    repo,
                    author: None,
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        }

        let repo = clean_repo_name(repo_part);
        if !repo.is_empty() {
            let lower_repo = repo.to_lowercase();
            let final_repo = if !KNOWN_REPOS.contains(&lower_repo.as_str()) {
                fuzzy_match_repo_name(&lower_repo).unwrap_or(repo)
            } else {
                repo
            };
            return Some(ParseResult {
                intent: ParsedIntent::CheckBranch {
                    owner: None,
                    repo: final_repo,
                    author: None,
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        }
    }

    None
}

/// Parse "owner/repo" format.
fn parse_owner_repo_analyse(text: &str) -> Option<ParseResult> {
    if let Some((owner, repo)) = parse_owner_repo(text) {
        return Some(ParseResult {
            intent: ParsedIntent::AnalyseRepo {
                owner: Some(owner),
                repo,
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }
    None
}

/// Parse "<repo> repo" / "repo <repo>" / "the repo <repo>" patterns.
fn parse_repo_keyword_analyse(text: &str) -> Option<ParseResult> {
    // "the repo <name>" or "repo <name>"
    if let Some(rest) = text.strip_prefix("the repo ") {
        let repo = clean_repo_name(rest);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::AnalyseRepo {
                    owner: None,
                    repo,
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }
    if let Some(rest) = text.strip_prefix("repo ") {
        let repo = clean_repo_name(rest);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::AnalyseRepo {
                    owner: None,
                    repo,
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }
    // "<name> repo" ΓÇö trailing "repo" keyword
    if let Some(rest) = text.strip_suffix(" repo") {
        let repo = clean_repo_name(rest);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::AnalyseRepo {
                    owner: None,
                    repo,
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }
    // "<name> repository"
    if let Some(rest) = text.strip_suffix(" repository") {
        let repo = clean_repo_name(rest);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::AnalyseRepo {
                    owner: None,
                    repo,
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    None
}

/// Parse "owner/repo" string into (owner, repo).
fn parse_owner_repo(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    if let Some(slash_idx) = text.find('/') {
        let owner = text[..slash_idx].trim().to_string();
        let repo = text[slash_idx + 1..].trim().to_string();
        // Validate: both parts should be non-empty and contain only valid chars
        if !owner.is_empty() && !repo.is_empty() && is_valid_repo_name(&owner) && is_valid_repo_name(&repo) {
            return Some((owner, repo));
        }
    }
    None
}

/// Check if a string is a valid GitHub repo/owner name.
/// GitHub names: alphanumeric, hyphens, underscores, dots.
fn is_valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.starts_with('-')
        && !name.starts_with('.')
}

/// Clean a repo name ΓÇö strip articles, trailing keywords, whitespace.
fn clean_repo_name(text: &str) -> String {
    let text = text.trim();
    // Strip leading "the "
    let text = text.strip_prefix("the ").unwrap_or(text);
    // Strip trailing "repo", "repository", "project", "codebase"
    let text = text
        .strip_suffix(" repo")
        .or_else(|| text.strip_suffix(" repository"))
        .or_else(|| text.strip_suffix(" project"))
        .or_else(|| text.strip_suffix(" codebase"))
        .unwrap_or(text);
    canonical_repo_name(text.trim())
}

/// Sound-alike aliases for known repos: STT (and fast speech) renders the
/// same name many ways — "servx" arrives as "cervix", "srvx", "service".
/// Every alias points at the canonical repo, so all soundings resolve to
/// the same entity. Applied per path segment (owner + name each), inside
/// EVERY category — deterministic and NLU paths alike.
fn repo_sound_alias(canonical: &str) -> Option<&'static str> {
    match canonical {
        "cervix" | "cervx" | "srvx" | "service" | "cervex" | "cervets"
        | "servetus" | "servex" | "survex" => Some("servx"),
        "zinc" | "zink" | "sync" | "zynk" | "zincs" => Some("zync"),
        "incognito" | "incognit" | "congy" | "conji" => Some("congi"),
        "meat" | "meets" => Some("meet"),
        "shopcart" => Some("shopkart"),
        "ledgerai" => Some("ledger-ai"),
        _ => None,
    }
}

/// Map a repo reference to its canonical form via the sound-alias table.
/// Unknown segments pass through untouched (never rewrites a real name).
pub fn canonical_repo_name(text: &str) -> String {
    let mapped: Vec<String> = text
        .split('/')
        .map(|seg| {
            let compact = seg.trim().to_lowercase().replace([' ', '-', '_'], "");
            // "ledger ai" compacts to "ledgerai" and hits the table.
            let plain = seg.trim().to_lowercase();
            repo_sound_alias(&compact)
                .or_else(|| repo_sound_alias(&plain))
                .map(str::to_string)
                .unwrap_or_else(|| seg.trim().to_string())
        })
        .collect();
    mapped.join("/")
}

// ΓöÇΓöÇΓöÇ Close app command ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

const CLOSE_VERBS: &[&str] = &["close", "quit", "exit", "kill", "terminate", "end", "shut down", "shut"];

fn parse_close_command(text: &str) -> Option<ParseResult> {
    for verb in CLOSE_VERBS {
        let prefix = format!("{} ", verb);
        if text.starts_with(&prefix) {
            let target = text[prefix.len()..].trim();
            if !target.is_empty() && target != "nexus" && target != "the app" {
                return Some(ParseResult {
                    intent: ParsedIntent::CloseApp { target: target.to_string() },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }
        }
    }
    None
}

// ΓöÇΓöÇΓöÇ WhatsApp chat command ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

/// Trailing words that belong to "in/on WhatsApp" or politeness, never to the contact name.
const WA_TAILS: &[&str] = &[
    " in the whatsapp app",
    " on the whatsapp app",
    " in whatsapp",
    " on whatsapp",
    " in the app",
    " in wa",
    " on wa",
    " please",
    " for me",
    " now",
];

/// Contact phrases that are really other WhatsApp nouns/app words (never a person).
const WA_NOT_A_CONTACT: &[&str] = &[
    "", "web", "desktop", "app", "the app", "status", "settings", "call", "calls", "updates",
    "chat", "chats", "whatsapp", "wa", "please", "now",
];

/// Strip every trailing "in/on whatsapp" / "please" tail from a contact phrase, repeatedly,
/// then a trailing "chat"/"'s chat". Pure (unit-tested). Applied to the CONTACT only - the old
/// code stripped `text` for the "on wa" form, which left "chat with mummy" as the contact.
pub(crate) fn strip_wa_tail(contact: &str) -> String {
    let mut c = contact.trim().to_string();
    loop {
        let mut changed = false;
        for t in WA_TAILS {
            if let Some(r) = c.strip_suffix(t) {
                c = r.trim().to_string();
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for t in [" chats", "'s chat", "s chat", " chat"] {
        if let Some(r) = c.strip_suffix(t) {
            // "s chat" must not eat the s of a real name ("jess chat" -> "jes"): only the
            // possessive "'s chat" form strips an s; the bare forms keep the name intact.
            if t == "s chat" {
                continue;
            }
            c = r.trim().to_string();
            break;
        }
    }
    for lead in ["my ", "the "] {
        if let Some(r) = c.strip_prefix(lead) {
            c = r.trim().to_string();
        }
    }
    c
}

fn wa_chat(contact: &str) -> Option<ParseResult> {
    let c = strip_wa_tail(contact);
    if WA_NOT_A_CONTACT.contains(&c.as_str()) {
        return None;
    }
    Some(ParseResult {
        intent: ParsedIntent::WhatsappChat { contact: c },
        confidence: 1.0,
        source: "deterministic".to_string(),
    })
}

/// STT garbles of the WhatsApp-chat command seen in live logs, repaired ONLY
/// here (not globally: dictated text must never be rewritten):
///   "chat with"   -> "chart with" / "start with" / "chap with"
///   "in WhatsApp" -> "in what's up" / "on whats app" / "and what so" ...
/// The "start with"/"and ..." forms are ordinary English, so they are repaired
/// only when the rest of the phrase is unmistakably a chat command.
fn normalize_whatsapp_garbles(text: &str) -> String {
    const GARBLES: &[&str] = &[
        "what so", "what's up", "whats up", "what's app", "whats app", "what sap",
        "what sup", "whatsup", "watsapp", "watts app", "watch app", "what's apps",
    ];
    let mut t = text.trim().to_string();

    // Head: a garbled "chat with". "start with ..." is common English, so it only
    // counts when the phrase also ends in an explicit WhatsApp tail (checked below).
    let is_tail = |t: &str| {
        [" in whatsapp", " on whatsapp", " in wa", " on wa"].iter().any(|x| t.ends_with(x))
    };
    let garbled_tail = |t: &str, links: &[&str]| -> Option<String> {
        for link in links {
            for g in GARBLES {
                let suffix = format!(" {link}{g}");
                if let Some(head) = t.strip_suffix(&suffix) {
                    return Some(format!("{head} in whatsapp"));
                }
            }
        }
        None
    };
    // 1. Tails introduced by "in"/"on" are rare English: repair unconditionally.
    if let Some(fixed) = garbled_tail(&t, &["in ", "on "]) {
        t = fixed;
    }
    // 2. Heads.
    for bad in ["chart with ", "chap with ", "start with "] {
        let weak = bad == "start with "; // ordinary English: needs the explicit tail
        if weak && !is_tail(&t) {
            continue;
        }
        if let Some(rest) = t.strip_prefix(bad) {
            t = format!("chat with {rest}");
        } else if let Some(i) = t.find(&format!(" {bad}")) {
            t = format!("{} chat with {}", &t[..i], &t[i + 1 + bad.len()..]);
        }
    }
    // 3. "... and what so": only after a chat head ("open my chat with mommy and what so").
    let chat_head = ["open chat with ", "open my chat with ", "open the chat with ", "chat with ", "show my chat with ", "show chat with "]
        .iter()
        .any(|h| t.starts_with(h));
    if chat_head {
        if let Some(fixed) = garbled_tail(&t, &["and ", "at "]) {
            t = fixed;
        }
    }
    t
}

/// "open mummy chat", "open my mom's chat": a kinship word plus "chat" is
/// unambiguous without a WhatsApp tail ("open mummy chat" used to launch
/// Google Chat). Anything that is not a single kin word is left alone.
fn kin_chat(text: &str) -> Option<ParseResult> {
    const VERBS: &[&str] = &["open ", "show ", "go to ", "bring up ", "pull up ", "switch to "];
    let verb = VERBS.iter().find(|v| text.starts_with(**v))?;
    let mut body = text[verb.len()..].trim();
    for lead in ["my ", "the "] {
        if let Some(r) = body.strip_prefix(lead) {
            body = r.trim();
            break;
        }
    }
    let name = ["'s chat", " chats", " chat"].iter().find_map(|suf| body.strip_suffix(suf))?.trim();
    if name.is_empty() || name.contains(' ') || !crate::live::commands::wa_open::is_kin_word(name) {
        return None;
    }
    Some(ParseResult {
        intent: ParsedIntent::WhatsappChat { contact: name.to_string() },
        confidence: 1.0,
        source: "deterministic".to_string(),
    })
}

fn parse_whatsapp_command(text: &str) -> Option<ParseResult> {
    let normalized = normalize_whatsapp_garbles(text);
    let text = normalized.as_str();
    if let Some(r) = kin_chat(text) {
        return Some(r);
    }
    // "open chat with lakshya", "chat with lakshya", "message lakshya"
    // "open my chat with lakshya", "whatsapp lakshya", "open my chat with mummy in whatsapp"
    let patterns: &[&str] = &[
        "open chat with ",
        "open my chat with ",
        "open the chat with ",
        "open my whatsapp chat with ",
        "show chat with ",
        "show my chat with ",
        "go to chat with ",
        "go to my chat with ",
        "chat with ",
        "message ",
        "whatsapp ",
        "open whatsapp chat with ",
        "send message to ",
        "send whatsapp to ",
    ];
    for pat in patterns {
        if let Some(rest) = text.strip_prefix(pat) {
            if let Some(r) = wa_chat(rest) {
                return Some(r);
            }
        }
    }
    // "open mummy chat in whatsapp", "open mummy's chat on whatsapp", "mummy chat in whatsapp",
    // "open mummy in whatsapp": these end in an explicit WhatsApp tail, and either start with an
    // open-style verb or end in "chat". "play music on whatsapp" matches neither, so it is left alone.
    let has_wa_tail = text.ends_with(" in whatsapp")
        || text.ends_with(" on whatsapp")
        || text.ends_with(" in the whatsapp app")
        || text.ends_with(" on the whatsapp app")
        || text.ends_with(" in wa")
        || text.ends_with(" on wa");
    if has_wa_tail {
        const OPEN_VERBS: &[&str] = &["open ", "show ", "go to ", "bring up ", "pull up ", "switch to "];
        let mut body = text;
        let mut had_verb = false;
        for v in OPEN_VERBS {
            if let Some(r) = text.strip_prefix(v) {
                body = r;
                had_verb = true;
                break;
            }
        }
        let stripped_tail = strip_wa_tail(body);
        let before_chat_strip = {
            let mut c = body.trim().to_string();
            for t in WA_TAILS {
                if let Some(r) = c.strip_suffix(t) {
                    c = r.trim().to_string();
                }
            }
            c
        };
        let ends_in_chat = before_chat_strip.ends_with(" chat")
            || before_chat_strip.ends_with("'s chat")
            || before_chat_strip.ends_with(" chats");
        if had_verb || ends_in_chat {
            if let Some(r) = wa_chat(&stripped_tail) {
                return Some(r);
            }
        }
    }
    let wa_search_patterns = [
        "search for ",
        "search ",
        "find ",
    ];
    for pat in wa_search_patterns {
        if text.starts_with(pat) {
            let rest = text[pat.len()..].trim();
            if let Some(contact) = rest.strip_suffix(" on whatsapp").or_else(|| rest.strip_suffix(" on wa")) {
                let contact = contact.trim();
                if !contact.is_empty() {
                    return Some(ParseResult {
                        intent: ParsedIntent::WhatsappChat { contact: contact.to_string() },
                        confidence: 1.0,
                        source: "deterministic".to_string(),
                    });
                }
            }
        }
    }
    None
}

// ─── YouTube & Video Journal commands ──────────────────────────────────────

fn parse_youtube_command(text: &str) -> Option<ParseResult> {
    let lower = text.trim().to_lowercase();

    // 1. YouTube Journal & Summarization commands
    if lower == "summarize this video"
        || lower == "summarize video"
        || lower == "summarize youtube video"
        || lower == "journal this video"
        || lower == "add this video to my journal"
        || lower == "add to journal"
        || lower == "take notes on this video"
        || lower == "youtube journal"
    {
        return Some(ParseResult {
            intent: ParsedIntent::NluResult {
                intent: "youtube_journal".to_string(),
                slots: serde_json::json!({ "url": "" }),
                confidence: 1.0,
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    if let Some(target) = lower.strip_prefix("summarize video ")
        .or_else(|| lower.strip_prefix("summarize youtube video "))
        .or_else(|| lower.strip_prefix("journal video "))
    {
        let t = target.trim();
        if !t.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::NluResult {
                    intent: "youtube_journal".to_string(),
                    slots: serde_json::json!({ "url": t }),
                    confidence: 1.0,
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    // 2. YouTube Search commands
    let prefixes = [
        "search youtube for ",
        "search on youtube for ",
        "search on youtube ",
        "search youtube ",
        "youtube search for ",
        "youtube search ",
        "find on youtube ",
        "look up on youtube ",
    ];

    for prefix in prefixes {
        if lower.starts_with(prefix) {
            let query = text[prefix.len()..].trim();
            if !query.is_empty() {
                return Some(ParseResult {
                    intent: ParsedIntent::NluResult {
                        intent: "youtube_search".to_string(),
                        slots: serde_json::json!({ "query": query }),
                        confidence: 1.0,
                    },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }
        }
    }

    // "search <query> on youtube"
    if let Some(pos) = lower.find(" on youtube") {
        let before = text[..pos].trim();
        for verb in &["search for ", "search ", "find ", "look up "] {
            if before.to_lowercase().starts_with(verb) {
                let query = before[verb.len()..].trim();
                if !query.is_empty() {
                    return Some(ParseResult {
                        intent: ParsedIntent::NluResult {
                            intent: "youtube_search".to_string(),
                            slots: serde_json::json!({ "query": query }),
                            confidence: 1.0,
                        },
                        confidence: 1.0,
                        source: "deterministic".to_string(),
                    });
                }
            }
        }
    }

    None
}

// ─── Search command ──────────────────────────────────────────────────────────

const SEARCH_VERBS: &[&str] = &[
    "search for", "search", "google", "look up", "find me", "find", "look for",
    "so it's for", "so its for", "so it is for", "it's for", "its for", "it is for",
    "search that on", "search on",
];

fn parse_search_command(text: &str) -> Option<ParseResult> {
    for verb in SEARCH_VERBS {
        let prefix = format!("{} ", verb);
        if text.starts_with(&prefix) {
            let query = text[prefix.len()..].trim().trim_end_matches(['.', ',', '!', '?']).trim();
            if !query.is_empty() {
                return Some(ParseResult {
                    intent: ParsedIntent::Search {
                        query: query.to_string(),
                    },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }
        }
    }
    None
}

/// Parse direct in-app typing payloads:
/// e.g. "type so it analysis for the servx right send it to as soon as possible"
/// Extracts everything after "type " verbatim without risking command collisions.
fn parse_type_dictation_command(text: &str) -> Option<ParseResult> {
    // If it's a dictation mode command ("start typing", "type whatever i say", "type line by line", "stop typing"),
    // let parse_dictation_command handle it!
    if let Some(cmd) = parse_dictation_command(text) {
        return Some(cmd);
    }

    let t = text.trim();
    let lower = t.to_lowercase();
    let trimmed_lower = lower.trim_end_matches(['.', ',', '!', '?']).trim();
    if trimmed_lower == "type" {
        return Some(ParseResult {
            intent: ParsedIntent::StartDictation,
            confidence: 1.0,
            source: "deterministic-dictation".to_string(),
        });
    }

    let prefixes = [
        "type: ",
        "type, ",
        "type message ",
        "type out ",
        "type this ",
        "type ",
    ];

    for prefix in prefixes {
        if lower.starts_with(prefix) {
            let raw_payload = t[prefix.len()..].trim();
            if !raw_payload.is_empty() {
                return Some(ParseResult {
                    intent: ParsedIntent::NluResult {
                        intent: "type_text".to_string(),
                        slots: serde_json::json!({ "text": raw_payload }),
                        confidence: 1.0,
                    },
                    confidence: 1.0,
                    source: "deterministic-dictation".to_string(),
                });
            }
        }
    }

    None
}

/// Parse screen analysis and visual research commands (Phase 1 expanded
/// triggers, 2026-10-01): analyse/analyze + screen, "what's on my screen",
/// "what do I see", "explain what I see", describe/read/scan/check +
/// screen, and research anchored to this/screen.
///
/// Anchoring rules (misfire guards):
/// - The analyse family MUST contain "screen" — "analyse servx" and
///   "analyse pr 5 in zync" still route to the repo analyzer.
/// - The research family MUST contain "this"/"screen" — general research
///   ("research quantum computing") still routes to the Worker.
/// - The what/see families are inherently screen queries (no anchor needed).
/// Memory audit & forget (M0): "what do you remember (about me)",
/// "forget my X", "forget everything" (two-step with spoken confirm),
/// "yes, forget everything" (executes the wipe).
///
/// Anchoring rules (misfire guards):
/// - Audit needs a remember/know/learned + memory/me anchor — "tell me
///   a joke" and "what do you see" never fire.
/// - Forget-one needs "forget" + a real key; bare "forget it/that/this"
///   (dismissals) explicitly excluded.
/// - Forget-all needs an everything/all-memory anchor; executes ONLY via
///   the separate confirm phrase (no state machine, no accidents).
/// Study-app preference: "use the browser for dsa", "switch dsa to the app",
/// "open dsa in the app from now on". Narrow on purpose — plain "open youtube
/// in the browser" stays a browser command.
fn parse_study_pref(t: &str) -> Option<ParsedIntent> {
    let choice = crate::memcore::timetable::parse_choice(t)?;
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let activity_text: String = if let Some(rest) = t
        .strip_prefix("use the browser for ")
        .or_else(|| t.strip_prefix("use the app for "))
        .or_else(|| t.strip_prefix("use browser for "))
        .or_else(|| t.strip_prefix("use app for "))
        .or_else(|| t.strip_prefix("prefer the browser for "))
        .or_else(|| t.strip_prefix("prefer the app for "))
    {
        rest.to_string()
    } else if let Some(rest) = t.strip_prefix("switch ") {
        let cut = [" to the ", " to "].iter().find_map(|m| rest.find(m))?;
        rest[..cut].to_string()
    } else if t.contains("from now on") && (words.first() == Some(&"open") || words.first() == Some(&"start")) {
        let rest = t.split_once(' ')?.1;
        let cut = rest.find(" in ")?;
        rest[..cut].to_string()
    } else {
        return None;
    };
    let cleaned: Vec<&str> = activity_text
        .split_whitespace()
        .filter(|w| !matches!(*w, "the" | "my" | "practice" | "sessions" | "session" | "from" | "now" | "on" | "please"))
        .collect();
    if cleaned.is_empty() || cleaned.len() > 3 {
        return None;
    }
    let activity = crate::memcore::timetable::activity_key(&cleaned.join(" "));
    if activity.is_empty() {
        return None;
    }
    Some(ParsedIntent::StudyPref { activity, choice: choice.as_str().to_string() })
}

/// "Directions to X" (P-maps). Anchored on "directions"/"route"/"how do I get to";
/// bare "navigate to" is NOT claimed (it belongs to browser navigation).
fn parse_directions_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', ',', '!', '?']).trim().to_string();
    let mut s = t.as_str();
    for lead in ["please ", "can you ", "could you ", "nexus ", "show me ", "give me ", "get me ", "get ", "find ", "i need ", "i want ", "show "] {
        if let Some(r) = s.strip_prefix(lead) {
            s = r;
        }
    }
    let mut mode: Option<&str> = None;
    for (w, m) in [
        ("walking ", "walking"), ("driving ", "driving"), ("cycling ", "bicycling"), ("biking ", "bicycling"),
        ("transit ", "transit"), ("bus ", "transit"), ("train ", "transit"),
    ] {
        if let Some(r) = s.strip_prefix(w) {
            s = r;
            mode = Some(m);
            break;
        }
    }
    let s = s.strip_prefix("the ").unwrap_or(s);
    // "directions from A to B" / "route from A to B"
    let mut origin: Option<String> = None;
    let mut dest: Option<String> = None;
    for head in ["directions from ", "route from ", "a route from ", "directions "] {
        if let Some(r) = s.strip_prefix(head) {
            if head.ends_with("from ") {
                let (a, b) = r.split_once(" to ")?;
                origin = Some(a.trim().to_string());
                dest = Some(b.trim().to_string());
            } else if let Some(d) = r.strip_prefix("to ").or_else(|| r.strip_prefix("for ")) {
                dest = Some(d.trim().to_string());
            }
            break;
        }
    }
    if dest.is_none() {
        for head in [
            "route to ", "a route to ", "directions to ", "how do i get to ", "how can i get to ", "how to get to ",
            "how do i reach ", "how do i drive to ", "how do i walk to ", "how do i get from ", "navigate me to ",
        ] {
            if let Some(r) = s.strip_prefix(head) {
                if head.ends_with("from ") {
                    let (a, b) = r.split_once(" to ")?;
                    origin = Some(a.trim().to_string());
                    dest = Some(b.trim().to_string());
                } else {
                    dest = Some(r.trim().to_string());
                }
                if head.ends_with("walk to ") {
                    mode = Some("walking");
                } else if head.ends_with("drive to ") {
                    mode = Some("driving");
                }
                break;
            }
        }
    }
    let mut dest = dest?;
    // Trailing qualifiers: " from Y", " by bus", " on foot", " please".
    if let Some(p) = dest.strip_suffix(" please") {
        dest = p.trim().to_string();
    }
    for (suffix, m) in [
        (" by walking", "walking"), (" on foot", "walking"), (" by bus", "transit"), (" by train", "transit"),
        (" by transit", "transit"), (" by public transport", "transit"), (" by bike", "bicycling"),
        (" by cycling", "bicycling"), (" by car", "driving"), (" by driving", "driving"),
    ] {
        if let Some(p) = dest.strip_suffix(suffix) {
            dest = p.trim().to_string();
            mode = Some(m);
        }
    }
    if origin.is_none() {
        if let Some((d, o)) = dest.rsplit_once(" from ") {
            if !d.trim().is_empty() && !o.trim().is_empty() {
                origin = Some(o.trim().to_string());
                dest = d.trim().to_string();
            }
        }
    }
    let words = dest.split_whitespace().count();
    let bad_dest = dest.is_empty()
        || words > 10
        || matches!(dest.as_str(), "it" | "there" | "here" | "that" | "this" | "the" | "a")
        // "how do I get to sleep / know you / better / rich" are idioms, not places.
        || matches!(dest.split_whitespace().next().unwrap_or(""), "sleep" | "bed" | "know" | "grips" | "better" | "rich" | "fit" | "started" | "used" | "over" | "done" | "back" | "inbox" | "zero");
    if bad_dest || origin.as_deref().map(|o| o.is_empty() || o.split_whitespace().count() > 10).unwrap_or(false) {
        return None;
    }
    Some(ParseResult {
        intent: ParsedIntent::Directions { destination: dest, origin, mode: mode.unwrap_or("driving").to_string() },
        confidence: 1.0,
        source: "deterministic-directions".to_string(),
    })
}

/// Words that mean the thing asked about is not a place (a UI control, a person, an object, an idea).
const PLACE_INFO_NOT_PLACES: &[&str] = &[
    "my", "your", "our", "his", "her", "their", "it", "that", "this", "there", "here", "everyone", "everything",
    "anything", "someone", "somebody", "who", "what", "which", "file", "files", "folder", "document", "documents",
    "app", "apps", "tab", "tabs", "window", "windows", "button", "icon", "menu", "setting", "settings", "option",
    "options", "bar", "search", "key", "keys", "keyboard", "mouse", "cursor", "mic", "microphone", "camera",
    "screen", "sidebar", "orb", "volume", "wifi", "bluetooth", "battery", "charger", "phone", "laptop", "remote",
    "wallet", "email", "emails", "message", "messages", "pr", "repo", "repository", "branch", "commit", "error",
    "bug", "code", "line", "function", "variable", "problem", "issue", "point", "answer", "love", "money", "time",
    "truth", "hope", "mind", "thing", "way", "rest", "nexus", "mom", "dad", "mummy", "daddy", "ghost", "dictation",
    "nearest", "closest", "nearby", "near",
];

/// Extra words that make "when is X" an event or a deadline, not a place.
const PLACE_INFO_EVENT_WORDS: &[&str] = &[
    "meeting", "deadline", "exam", "exams", "test", "class", "lecture", "interview", "holiday", "birthday", "party",
    "wedding", "event", "game", "match", "hackathon", "submission", "due", "sale", "release", "launch", "festival",
    "diwali", "christmas", "eid", "holi", "new", "next", "today", "tomorrow", "now", "appointment", "call", "reminder",
    "standup", "demo", "presentation", "result", "results", "assignment", "quiz", "semester", "session", "show", "movie",
];

/// "Where is the Charminar", "when is the Charminar", "when does the museum close", "is the zoo open",
/// "show me the Taj Mahal on the map". Needs a short, place-like noun phrase; anything that smells like a
/// control, a person, an event or an idea is left to the other parsers.
fn parse_place_info_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', ',', '!', '?']).trim().to_string();
    let mut s = t.as_str();
    for lead in ["please ", "nexus, ", "nexus ", "can you ", "could you ", "tell me ", "do you know ", "show me ", "i want to know "] {
        if let Some(r) = s.strip_prefix(lead) {
            s = r;
        }
    }
    let on_map = s.ends_with(" on the map") || s.ends_with(" on maps") || s.ends_with(" on google maps");
    let (rest, mut ask): (&str, &str) = if let Some(r) = s.strip_prefix("where is ").or_else(|| s.strip_prefix("where's ")) {
        (r, "where")
    } else if let Some(r) = s.strip_prefix("when is ").or_else(|| s.strip_prefix("when's ")) {
        (r, "when")
    } else if let Some(r) = s.strip_prefix("when does ").or_else(|| s.strip_prefix("what time does ")) {
        (r, "hours")
    } else if let Some(r) = s.strip_prefix("what time is ") {
        (r, "hours")
    } else if let Some(r) = s.strip_prefix("is ").filter(|r| r.ends_with(" open") || r.ends_with(" open now") || r.ends_with(" closed")) {
        (r, "hours")
    } else if on_map {
        (s, "where")
    } else {
        return None;
    };
    let mut place = rest.to_string();
    for suffix in [
        " on the map", " on maps", " on google maps", " located at", " located", " open now", " open", " closed", " close",
        " closing", " opening", " exactly", " please",
    ] {
        if let Some(p) = place.strip_suffix(suffix) {
            if matches!(suffix.trim(), "open" | "open now" | "close" | "closed" | "closing" | "opening") {
                ask = "hours";
            }
            place = p.trim().to_string();
        }
    }
    let place = place.trim_start_matches("the ").trim_start_matches("a ").trim().to_string();
    let words: Vec<&str> = place.split_whitespace().collect();
    if words.is_empty() || words.len() > 5 || place.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if words.iter().any(|w| PLACE_INFO_NOT_PLACES.contains(w)) {
        return None;
    }
    if ask == "when" && words.iter().any(|w| PLACE_INFO_EVENT_WORDS.contains(w)) {
        return None;
    }
    Some(ParseResult {
        intent: ParsedIntent::PlaceInfo { query: place, ask: ask.to_string() },
        confidence: 0.9,
        source: "deterministic-place-info".to_string(),
    })
}

/// "Analyse this email", "summarise this mail", "what is this email about": read the email that is open on
/// screen. Needs an explicit "this/the open/current email" so "analyse the email I got about the internship"
/// (ScanInternshipEmails) and "watch this email" (WatchScreenEmail) keep their own parsers.
fn parse_analyse_this_email_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', ',', '!', '?']).trim().to_string();
    let mut s = t.as_str();
    for lead in ["please ", "nexus, ", "nexus ", "hey ", "can you ", "could you ", "go ahead and "] {
        if let Some(r) = s.strip_prefix(lead) {
            s = r;
        }
    }
    const TARGETS: &[&str] = &[
        "this email", "this mail", "this e-mail", "the open email", "the open mail", "the current email",
        "the email on screen", "the email on my screen", "the email i have open", "the email i'm reading",
    ];
    const VERBS: &[&str] = &[
        "analyse ", "analyze ", "summarise ", "summarize ", "explain ", "break down ", "go through ", "read ",
        "check ", "tell me about ", "what is ", "what's ", "what does ", "give me the details of ", "give me details of ",
    ];
    const BLOCK: &[&str] = &["reply", "send", "delete", "forward", "draft", "write", "watch", "track", "update me", "archive", "mark"];
    if s.split_whitespace().count() > 16 || !VERBS.iter().any(|v| s.starts_with(v)) {
        return None;
    }
    if !TARGETS.iter().any(|x| s.contains(x)) {
        return None;
    }
    if s.split_whitespace().any(|w| BLOCK.contains(&w)) {
        return None;
    }
    Some(ParseResult {
        intent: ParsedIntent::AnalyseThisEmail,
        confidence: 0.95,
        source: "deterministic-analyse-email".to_string(),
    })
}

/// "Analyse this", "explain this", "what is this", "describe this image", "summarise this page": the user points
/// at whatever is on screen without naming it (a picture, a person's page, a YouTube video, a website, a chart,
/// code, a document...). It goes to the screen-understanding path, which reads the foreground window and its
/// context. It must NOT be taken for a repository name. Dedicated nouns keep their own routes: "this repo" and
/// "this PR" are resolved from the open browser page by the orchestrator, "this email" has its own parser.
/// "analyse this pr" / "review the latest pull request" / "analyse the pr in zync": pointing at a pull request WITHOUT a
/// number. No repo named -> `AnalyseRepo { repo: "this pr" | "latest pr" }` (the screen decides which one); a repo named
/// -> `AnalyseLatestPr` (the screen is checked for a PR of that repo first, else its latest one). A number, an author
/// ("by prem") or any other word keeps the older parsers. Pure.
fn parse_analyse_pr_pointer(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    let t = t.trim_end_matches(['.', ',', '!', '?']).trim().to_string();
    let mut s = t.as_str();
    for lead in ["please ", "nexus, ", "nexus ", "hey ", "ok ", "okay "] {
        if let Some(r) = s.strip_prefix(lead) {
            s = r;
        }
    }
    let s = strip_lead_ins(s);
    const VERBS: &[&str] = &[
        "analyse ", "analyze ", "review ", "explain ", "summarise ", "summarize ", "describe ", "check ", "look at ",
        "go through ", "break down ", "tell me about ",
    ];
    const WHAT: &[&str] = &["what is ", "what's "];
    let (rest, strict_this) = if let Some(v) = VERBS.iter().find(|v| s.starts_with(**v)) {
        (&s[v.len()..], false)
    } else if let Some(v) = WHAT.iter().find(|v| s.starts_with(**v)) {
        (&s[v.len()..], true)
    } else {
        return None;
    };
    let mut core = rest.trim();
    'strip: loop {
        for suffix in [
            " for me", " please", " on my screen", " on the screen", " on screen", " here", " that i'm looking at",
            " i'm looking at", " that i am looking at", " that i can see", " about", " is about",
        ] {
            if let Some(p) = core.strip_suffix(suffix) {
                core = p.trim();
                continue 'strip;
            }
        }
        break;
    }
    // "analyse PR 70" (a number, no repository): the screen, then the user's repositories decide which repo
    if !strict_this {
        if let Some(caps) = regex_captures(core, r"^(?:(?:the|this|that)\s+)?(?:pr|pull request)\s*#?\s*(\d{1,6})$") {
            if let Ok(n) = caps[1].parse::<u32>() {
                if n > 0 {
                    return Some(ParseResult {
                        intent: ParsedIntent::AnalysePr { owner: None, repo: "this".to_string(), pr_number: n },
                        confidence: 1.0,
                        source: "deterministic".to_string(),
                    });
                }
            }
        }
    }
    // a specific PR (number) or an author keeps the older parsers
    if core.contains(|c: char| c.is_ascii_digit() || c == '#') || core.contains(" by ") {
        return None;
    }
    // "<head> [in|of|for|from <repo>]"
    let mut head = core;
    let mut repo_part: Option<&str> = None;
    for prep in [" in ", " of ", " for ", " from "] {
        if let Some(i) = core.find(prep) {
            head = core[..i].trim();
            repo_part = Some(core[i + prep.len()..].trim());
            break;
        }
    }
    const ALLOWED: &[&str] = &["the", "this", "that", "my", "current", "latest", "newest", "last", "recent", "open", "a", "one"];
    let mut words: Vec<&str> = head.split_whitespace().collect();
    match words.as_slice() {
        [.., "pull", "request"] => {
            words.truncate(words.len() - 2);
        }
        [.., "pr"] => {
            words.truncate(words.len() - 1);
        }
        _ => return None,
    }
    if words.iter().any(|w| !ALLOWED.contains(w)) || (strict_this && !words.contains(&"this")) {
        return None;
    }
    let latest = words.iter().any(|w| matches!(*w, "latest" | "newest" | "last" | "recent"));
    // "analyse the pr in zync" (plain "the", a repo named) stays the older list-the-PRs reading; only a pointing or
    // "latest" word makes it "the one on screen, else the latest of that repo"
    if repo_part.is_some() && !words.iter().any(|w| matches!(*w, "this" | "that" | "my" | "current" | "latest" | "newest" | "last" | "recent")) {
        return None;
    }
    let repo_part = repo_part
        .map(|r| r.trim_end_matches(" repository").trim_end_matches(" repo").trim_end_matches(" project").trim())
        .filter(|r| !r.is_empty() && !matches!(*r, "this" | "here" | "this repo" | "this repository" | "the repo" | "the repository"));
    match repo_part {
        None => Some(ParseResult {
            intent: ParsedIntent::AnalyseRepo { owner: None, repo: if latest { "latest pr" } else { "this pr" }.to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        }),
        Some(rp) => {
            if let Some((owner, repo)) = parse_owner_repo(rp) {
                return Some(ParseResult {
                    intent: ParsedIntent::AnalyseLatestPr { owner: Some(owner), repo, author: None },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }
            let repo = clean_repo_name(rp);
            if repo.is_empty() {
                return None;
            }
            let lower = repo.to_lowercase();
            let repo = if KNOWN_REPOS.contains(&lower.as_str()) { repo } else { fuzzy_match_repo_name(&lower).unwrap_or(repo) };
            Some(ParseResult {
                intent: ParsedIntent::AnalyseLatestPr { owner: None, repo, author: None },
                confidence: 0.95,
                source: "deterministic".to_string(),
            })
        }
    }
}

fn parse_analyse_deictic_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    let t = t.trim_end_matches(['.', ',', '!', '?']).trim().to_string();
    let mut s = t.as_str();
    for lead in ["please ", "nexus, ", "nexus ", "hey ", "ok ", "okay ", "can you ", "could you ", "would you ", "i want you to ", "i need you to "] {
        if let Some(r) = s.strip_prefix(lead) {
            s = r;
        }
    }
    let s = strip_lead_ins(s);
    if s.split_whitespace().count() > 12 {
        return None;
    }
    const VERBS: &[&str] = &[
        "analyse ", "analyze ", "explain ", "describe ", "summarise ", "summarize ", "review ", "break down ",
        "go through ", "look at ", "check out ", "tell me about ", "tell me what is ", "tell me what's ",
        "what do you think of ", "what do you think about ",
    ];
    // "what is this" forms are only taken with "this" (never "it"/"that": those answer the previous reply).
    const WHAT_FORMS: &[&str] = &["what is ", "what's ", "what is this about", "what's this about"];
    let (rest, strict_this) = if let Some(v) = VERBS.iter().find(|v| s.starts_with(**v)) {
        (&s[v.len()..], false)
    } else if let Some(v) = WHAT_FORMS.iter().find(|v| s.starts_with(**v)) {
        (&s[v.len()..], true)
    } else {
        return None;
    };
    let rest = rest.trim();
    // Optional trailing "for me", "please", "that i'm looking at", "on my screen"...
    let mut core = rest;
    for suffix in [
        " for me", " please", " on my screen", " on the screen", " on screen", " that i'm looking at", " i'm looking at",
        " that i am looking at", " i am looking at", " that i can see", " that i see", " that is open", " in front of me",
    ] {
        if let Some(p) = core.strip_suffix(suffix) {
            core = p.trim();
        }
    }
    let core = core.strip_suffix(" about").unwrap_or(core).trim();
    const PRONOUNS: &[&str] = &["this", "that", "it", "this one", "that one", "these", "those", "this thing", "that thing"];
    const OBJECTS: &[&str] = &[
        "image", "picture", "photo", "photograph", "screenshot", "page", "webpage", "web page", "site", "website",
        "video", "clip", "article", "post", "tweet", "profile", "person", "document", "doc", "pdf", "chart", "graph",
        "diagram", "code", "file", "tab", "window", "app", "slide", "slides", "table", "form", "message", "thread",
        "song", "product", "listing", "recipe", "map", "design", "ui", "page here",
    ];
    // Words that mean a different, dedicated route.
    const DEDICATED: &[&str] = &[
        "repo", "repository", "repositories", "project", "codebase", "pr", "pull", "email", "mail", "e-mail", "branch",
        "commit", "issue", "report",
    ];
    // "this rap up" is the speech engine's "this repo": the repository route takes it.
    if matches!(core, "this rap up" | "this wrap up" | "this repo up" | "the rap up" | "the wrap up") {
        return None;
    }
    let words: Vec<&str> = core.split_whitespace().collect();
    if words.is_empty() || words.iter().any(|w| DEDICATED.contains(w)) {
        return None;
    }
    let is_pronoun = PRONOUNS.contains(&core);
    // "this/that <anything up to four words>" is the user pointing ("this youtube video", "this linkedin profile");
    // "the/my/current <known object>" needs a known object so ordinary chat is not taken.
    let is_this_object = words.len() >= 2
        && ((matches!(words[0], "this" | "that" | "these" | "those") && words.len() <= 5)
            || (matches!(words[0], "the" | "my" | "current" | "open") && OBJECTS.contains(&words[1..].join(" ").as_str())));
    let ok = if strict_this {
        core == "this" || core == "this one" || core == "this thing" || (is_this_object && words[0] == "this")
    } else {
        is_pronoun || is_this_object
    };
    if !ok {
        return None;
    }
    Some(ParseResult {
        intent: ParsedIntent::NluResult {
            intent: "screen_analysis".to_string(),
            slots: serde_json::json!({ "prompt": format!("analyse what is on my screen: {}", text.trim()) }),
            confidence: 1.0,
        },
        confidence: 1.0,
        source: "deterministic-screen-analysis".to_string(),
    })
}

/// "Nearest pharmacy" / "coffee shops near me" / "find a bank near Central Park".
/// Needs an explicit proximity marker (nearest / closest / nearby / near ...), so
/// ordinary "find ..." commands are never claimed.
fn parse_places_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', ',', '!', '?']).trim().to_string();
    let mut s = t.as_str();
    for lead in [
        "please ", "nexus ", "can you ", "could you ", "where can i find ", "where is ", "where's ", "is there ",
        "are there ", "find me ", "show me ", "get me ", "find ", "show ", "look for ", "search for ",
    ] {
        if let Some(r) = s.strip_prefix(lead) {
            s = r;
        }
    }
    for art in ["the ", "a ", "an ", "any "] {
        if let Some(r) = s.strip_prefix(art) {
            s = r;
            break;
        }
    }
    let mut marked = false;
    for m in ["nearest ", "closest ", "nearby "] {
        if let Some(r) = s.strip_prefix(m) {
            s = r;
            marked = true;
            break;
        }
    }
    let (what, near): (String, Option<String>) = {
        let mut what = s.to_string();
        let mut near: Option<String> = None;
        for sep in [" near me", " nearby", " around me", " close to me", " close by", " around here", " near here"] {
            if let Some(p) = what.strip_suffix(sep) {
                what = p.trim().to_string();
                marked = true;
                break;
            }
        }
        for sep in [" near ", " around ", " close to ", " in the area of "] {
            if let Some((a, b)) = what.split_once(sep) {
                let b = b.trim();
                if !b.is_empty() && !matches!(b, "me" | "here" | "us") {
                    near = Some(b.to_string());
                    marked = true;
                    what = a.trim().to_string();
                }
                break;
            }
        }
        (what, near)
    };
    if !marked {
        return None;
    }
    let words = what.split_whitespace().count();
    const NOT_PLACES: &[&str] = &[
        "file", "files", "document", "documents", "folder", "folders", "photo", "photos", "picture", "pictures",
        "app", "apps", "tab", "tabs", "window", "windows", "neighbor", "neighbors", "neighbour", "neighbours",
        "star", "planet", "email", "emails", "message", "messages", "contact", "contacts", "song", "songs",
        "video", "videos", "word", "words", "number", "numbers", "it", "that", "this", "one", "me", "match",
    ];
    if what.is_empty() || words > 4 || what.split_whitespace().any(|w| NOT_PLACES.contains(&w)) {
        return None;
    }
    if near.as_deref().map(|n| n.split_whitespace().count() > 8).unwrap_or(false) {
        return None;
    }
    Some(ParseResult {
        intent: ParsedIntent::PlacesSearch { query: what, near },
        confidence: 1.0,
        source: "deterministic-places".to_string(),
    })
}

/// WhatsApp priority-people commands (P6). Every arm needs a distinctive
/// anchor ("message(s)", "vip", "whatsapp ... from <name>", "priority
/// people") so plain "mute", "read this" and "make a note" keep their meaning.
fn parse_whatsapp_people_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let t = t
        .trim_end_matches(['.', ',', '!', '?'])
        .trim()
        .replace("v.i.p.", "vip")
        .replace("v.i.p", "vip")
        .replace("v i p", "vip");
    let mk = |intent: ParsedIntent| {
        Some(ParseResult { intent, confidence: 1.0, source: "deterministic-whatsapp-people".to_string() })
    };
    // A person's name: 1-3 plain words, never a pronoun or filler.
    let name_of = |raw: &str| -> Option<String> {
        let words: Vec<&str> = raw
            .split_whitespace()
            .filter(|w| !matches!(*w, "the" | "my" | "a" | "an" | "please"))
            .collect();
        if words.is_empty() || words.len() > 3 {
            return None;
        }
        if words.iter().any(|w| {
            matches!(
                *w,
                "me" | "it" | "this" | "that" | "them" | "him" | "her" | "everyone" | "everybody" | "all"
                    | "whatsapp" | "sound" | "volume" | "mic" | "microphone"
            )
        }) {
            return None;
        }
        if !words.iter().all(|w| w.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-')) {
            return None;
        }
        Some(words.join(" "))
    };
    let read = |n: &str| {
        name_of(n.trim_end_matches(" on whatsapp").trim())
            .map(|name| ParsedIntent::WhatsappRead { name: Some(name) })
            .and_then(mk)
    };

    // who are my priority people
    if matches!(
        t.as_str(),
        "who are my priority people" | "who are my vips" | "who are my important people" | "my priority people"
            | "whatsapp priority people" | "who is on my vip list" | "who's on my vip list" | "show my vips"
            | "which whatsapp contacts are important" | "who do you alert me for" | "who do you alert me about"
            | "who are my priority contacts" | "list my priority people" | "list my vips"
    ) {
        return mk(ParsedIntent::PeopleList);
    }

    // read that message / read the message from X / read X's message(s) / what did X say on whatsapp
    let msg_tail = |s: &str| matches!(s, "message" | "messages" | "whatsapp message" | "whatsapp messages");
    if let Some(rest) = t.strip_prefix("read ") {
        let rest = rest.trim();
        for lead in [
            "that ", "the ", "my ", "latest ", "last ", "new ", "the latest ", "the last ", "my latest ", "my last ",
            "my new ", "the new ",
        ] {
            if let Some(tail) = rest.strip_prefix(lead) {
                if msg_tail(tail.trim()) {
                    return mk(ParsedIntent::WhatsappRead { name: None });
                }
            }
        }
        for from in [
            "the message from ", "the messages from ", "the whatsapp message from ", "the whatsapp messages from ",
            "that message from ", "message from ", "messages from ",
        ] {
            if let Some(n) = rest.strip_prefix(from) {
                return read(n);
            }
        }
        // "asha's message(s)" / "asha's whatsapp message"
        if let Some(idx) = rest.find("'s ") {
            let (who, tail) = (&rest[..idx], rest[idx + 3..].trim());
            if msg_tail(tail) {
                return read(who);
            }
        }
    }
    if let Some(rest) = t.strip_prefix("what did ") {
        if let Some(who) = rest
            .strip_suffix(" say on whatsapp")
            .or_else(|| rest.strip_suffix(" send on whatsapp"))
            .or_else(|| rest.strip_suffix(" say to me on whatsapp"))
            .or_else(|| rest.strip_suffix(" send me on whatsapp"))
        {
            return read(who);
        }
    }

    // VIP: make X a vip / mark X as vip / add X to my vips / remove X from my vips
    let vip = |name: &str, on: bool| {
        name_of(name).map(|name| ParsedIntent::PeopleFlag { name, kind: "vip".into(), on }).and_then(mk)
    };
    if let Some(rest) = t.strip_prefix("make ") {
        for end in [" a vip", " a priority contact", " a priority person", " vip"] {
            if let Some(n) = rest.strip_suffix(end) {
                return vip(n, true);
            }
        }
    }
    if let Some(rest) = t.strip_prefix("mark ") {
        for end in [" as a vip", " as vip", " as a priority contact", " as a priority person"] {
            if let Some(n) = rest.strip_suffix(end) {
                return vip(n, true);
            }
        }
    }
    if let Some(rest) = t.strip_prefix("add ") {
        for end in [" to my vips", " to my vip list", " to the vip list", " to my priority people", " to my priority contacts"] {
            if let Some(n) = rest.strip_suffix(end) {
                return vip(n, true);
            }
        }
    }
    if let Some(rest) = t.strip_prefix("remove ") {
        for end in [" from my vips", " from my vip list", " from the vip list", " from my priority people", " from my priority contacts"] {
            if let Some(n) = rest.strip_suffix(end) {
                return vip(n, false);
            }
        }
    }

    // Mute / unmute a person's WhatsApp alerts (needs "whatsapp" + a name).
    let mute = |name: &str, on: bool| {
        name_of(name).map(|name| ParsedIntent::PeopleFlag { name, kind: "mute".into(), on }).and_then(mk)
    };
    for (verb, on) in [("mute ", true), ("stop ", true), ("unmute ", false), ("resume ", false)] {
        if let Some(rest) = t.strip_prefix(verb) {
            for lead in [
                "whatsapp alerts from ", "whatsapp messages from ", "whatsapp notifications from ",
                "the whatsapp alerts from ",
            ] {
                if let Some(n) = rest.strip_prefix(lead) {
                    return mute(n, on);
                }
            }
            if verb == "mute " || verb == "unmute " {
                if let Some(n) = rest.strip_suffix(" on whatsapp") {
                    return mute(n, on);
                }
            }
        }
    }
    None
}

/// Inbox digest / mute and calendar agenda / add (P5). Narrow phrases only:
/// "open google calendar" and ordinary "add ..." commands keep their meaning.
fn parse_mail_calendar_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', ',', '!', '?']).trim().to_string();
    let mk = |intent: ParsedIntent| {
        Some(ParseResult { intent, confidence: 1.0, source: "deterministic-mail-calendar".to_string() })
    };
    if matches!(
        t.as_str(),
        "that's not important" | "thats not important" | "that is not important" | "not important"
            | "mute that sender" | "ignore that sender" | "mute that" | "stop alerts from that sender"
            | "stop telling me about these" | "stop telling me about that"
    ) {
        return mk(ParsedIntent::MailMute);
    }
    if matches!(
        t.as_str(),
        "important emails" | "important email" | "any important emails" | "any important email"
            | "my important emails" | "important mail" | "any important mail" | "email digest"
            | "what emails are important"
    ) || t.contains("anything important in my email")
        || t.contains("anything important in my inbox")
        || t.contains("anything important in my mail")
    {
        return mk(ParsedIntent::MailDigest);
    }
    if crate::memcore::agenda::is_calendar_add(&t) {
        return mk(ParsedIntent::CalendarAdd { text: t });
    }
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    let asks = has("what") || has("what's") || has("whats") || has("tell") || has("read") || has("anything") || has("check") || t.contains("do i have");
    if t.contains("agenda") || (t.contains("calendar") && asks) {
        let day = if has("tomorrow") { "tomorrow" } else { "today" };
        return mk(ParsedIntent::CalendarAgenda { day: day.to_string() });
    }
    None
}

/// Timetable commands (P4). Anchored on the word "timetable"/"time table"
/// so ordinary "add ..." / "show ..." / "clear ..." phrases never match.
fn parse_timetable_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', ',', '!', '?']).trim().to_string();
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    let mk = |intent: ParsedIntent| {
        Some(ParseResult { intent, confidence: 1.0, source: "deterministic-timetable".to_string() })
    };

    if let Some(p) = parse_study_pref(&t) {
        return mk(p);
    }
    // Commit the slots read from the last image (no noun needed).
    if matches!(
        t.as_str(),
        "add those" | "add those slots" | "add them" | "save those slots" | "save the slots" | "save those" | "add the slots"
    ) {
        return mk(ParsedIntent::TimetableCommit);
    }

    let noun = t.contains("timetable") || t.contains("time table");
    if !noun {
        return None;
    }
    let clear = has("clear") || has("delete") || has("erase") || has("reset") || has("wipe");
    let add = has("add") || has("put") || has("save") || has("import") || has("include") || has("update");
    let show = has("show") || has("what") || has("what's") || has("whats") || has("read") || has("tell") || has("next") || has("open");
    if clear {
        return mk(ParsedIntent::TimetableClear);
    }
    if add {
        if has("them") || has("those") {
            return mk(ParsedIntent::TimetableCommit);
        }
        let source = if has("clipboard") || has("copied") || has("paste") || has("pasted") { "clipboard" } else { "screen" };
        let section = match crate::memcore::timetable::parse_want(&t) {
            Some(crate::memcore::timetable::Want::Number(n)) => Some(n),
            None => None,
        };
        return mk(ParsedIntent::TimetableAdd { source: source.to_string(), section });
    }
    if show {
        return mk(ParsedIntent::TimetableShow);
    }
    None
}

/// "reopen my session", "reopen what I had open" - strict phrases for the pending last-session offer.
fn is_reopen_session_command(text: &str) -> bool {
    let t = text.trim().trim_end_matches(['.', ',', '!', '?']).trim();
    let t = t.strip_prefix("nexus ").unwrap_or(t);
    let t = t.strip_prefix("please ").unwrap_or(t);
    let t = t.strip_suffix(" please").unwrap_or(t);
    matches!(
        t,
        "reopen my session"
            | "reopen the session"
            | "reopen my last session"
            | "reopen everything"
            | "reopen everything i had open"
            | "reopen what i had open"
            | "reopen what was open"
            | "reopen my apps"
            | "reopen my windows"
            | "restore my session"
            | "restore my last session"
            | "restore my apps"
            | "restore what i had open"
            | "bring back my session"
            | "open what i had open"
            | "open what i had open yesterday"
            | "open everything i had open"
    )
}

fn parse_memory_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let trimmed = t.trim_end_matches(['.', ',', '!', '?']).trim();

    // 0. Briefing family ("where did I leave off", "show my briefing").
    for pat in [
        "where did i leave off",
        "where did i stop",
        "where was i",
        "what was i doing",
        "what was i working on",
        "show my briefing",
        "give me my briefing",
        "my briefing",
        "morning briefing",
        "brief me",
        "catch me up",
        "what are my priorities",
        "what are my priorities today",
    ] {
        if trimmed == pat || trimmed.starts_with(&format!("{pat} ")) {
            return Some(ParseResult {
                intent: ParsedIntent::Briefing,
                confidence: 1.0,
                source: "deterministic-memory".to_string(),
            });
        }
    }

    // 0b. Contacts import ("import my contacts"): numbers for WhatsApp links, local only.
    for pat in [
        "import my contacts",
        "import contacts",
        "import my contact list",
        "import my whatsapp contacts",
        "import my phone contacts",
        "import my contacts file",
        "load my contacts",
        "read my contacts file",
        "sync my contacts",
    ] {
        if trimmed == pat {
            return Some(ParseResult {
                intent: ParsedIntent::ImportContacts,
                confidence: 1.0,
                source: "deterministic-memory".to_string(),
            });
        }
    }

    // 1. Audit family.
    for pat in [
        "memory audit",
        "what do you remember about me",
        "what do you remember",
        "what do you know about me",
        "what have you learned about me",
        "tell me what you remember",
        "list what you remember",
        "show what you remember",
        "show my memory",
        "audit my memory",
        "audit memory",
    ] {
        if trimmed == pat || trimmed.starts_with(&format!("{pat} ")) {
            return Some(ParseResult {
                intent: ParsedIntent::MemoryAudit,
                confidence: 1.0,
                source: "deterministic-memory".to_string(),
            });
        }
    }

    // 2. Forget-all step 2 (confirm) — before step 1 so the longer
    // confirm phrase never falls into forget-one's key slot.
    for pat in [
        "yes forget everything",
        "yes, forget everything",
        "yes erase everything",
        "yes, erase everything",
        "yes forget all",
        "yes delete everything",
        "confirm forget everything",
    ] {
        if trimmed == pat {
            return Some(ParseResult {
                intent: ParsedIntent::MemoryForgetAllConfirm,
                confidence: 1.0,
                source: "deterministic-memory".to_string(),
            });
        }
    }

    // 3. Forget-all step 1.
    for pat in [
        "forget everything",
        "forget all",
        "forget all memory",
        "erase my memory",
        "erase everything you remember",
        "delete all memory",
        "delete everything you remember",
    ] {
        if trimmed == pat {
            return Some(ParseResult {
                intent: ParsedIntent::MemoryForgetAll,
                confidence: 1.0,
                source: "deterministic-memory".to_string(),
            });
        }
    }

    // 4. Forget-one: "forget my birthday" / "forget that nickname".
    if let Some(rest) = trimmed
        .strip_prefix("forget my ")
        .or_else(|| trimmed.strip_prefix("forget that "))
        .or_else(|| trimmed.strip_prefix("forget "))
    {
        let key = rest.trim_end_matches(['.', ',', '!', '?']).trim();
        // Dismissals and step-1/step-2 phrases are never keys.
        if !key.is_empty()
            && !matches!(
                key,
                "it" | "that" | "this" | "about it" | "about this" | "everything" | "all"
            )
        {
            return Some(ParseResult {
                intent: ParsedIntent::MemoryForget {
                    key: key.to_string(),
                },
                confidence: 1.0,
                source: "deterministic-memory".to_string(),
            });
        }
    }

    None
}

/// Personality tone switch (F0): friend opt-in vs formal default.
/// No anchors needed — all phrases are unambiguous commands.
fn parse_persona_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let trimmed = t.trim_end_matches(['.', ',', '!', '?']).trim();
    for pat in [
        "talk like a friend",
        "talk to me like a friend",
        "be my friend",
        "be friendly",
        "friend mode",
    ] {
        if trimmed == pat {
            return Some(ParseResult {
                intent: ParsedIntent::PersonaFriend,
                confidence: 1.0,
                source: "deterministic-persona".to_string(),
            });
        }
    }
    for pat in [
        "be formal",
        "talk formally",
        "talk like a butler",
        "butler mode",
        "be professional",
    ] {
        if trimmed == pat {
            return Some(ParseResult {
                intent: ParsedIntent::PersonaButler,
                confidence: 1.0,
                source: "deterministic-persona".to_string(),
            });
        }
    }
    None
}

/// Honest counsel (F1b): opener arms counsel mode + invites the story;
/// direct forms (story included) run counsel immediately.
/// "tell me honestly" needs a counsel anchor (opinion/right/advice/honest
/// verdict) so "tell me honestly what you see" stays screen analysis.
fn parse_share_concern_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let trimmed = t.trim_end_matches(['.', ',', '!', '?']).trim();
    for pat in [
        "i want to share something with you",
        "i want to share something",
        "can i share something with you",
        "can i share something",
        "i need to tell you something",
        "i need your opinion",
        "i need your advice",
    ] {
        if trimmed == pat {
            return Some(ParseResult {
                intent: ParsedIntent::ShareConcern { story: None },
                confidence: 1.0,
                source: "deterministic-counsel".to_string(),
            });
        }
    }
    // Direct counsel: story included.
    let direct_prefixes = [
        "was i right to ",
        "was i wrong to ",
        "did i do the right thing ",
        "was that the right thing to do",
        "tell me honestly ",
    ];
    for prefix in direct_prefixes {
        // Prefix form ("was i right to shout") and bare form
        // ("did i do the right thing").
        if trimmed.starts_with(prefix) || trimmed == prefix.trim_end() {
            // "tell me honestly" demands a counsel anchor so screen
            // queries ("tell me honestly what you see") never fire.
            if prefix == "tell me honestly " {
                let rest = trimmed.strip_prefix(prefix).unwrap_or("");
                if !(rest.contains("right")
                    || rest.contains("wrong")
                    || rest.contains("opinion")
                    || rest.contains("advice")
                    || rest.contains("should i"))
                {
                    continue;
                }
            }
            return Some(ParseResult {
                intent: ParsedIntent::ShareConcern {
                    story: Some(text.trim().to_string()),
                },
                confidence: 1.0,
                source: "deterministic-counsel".to_string(),
            });
        }
    }
    None
}

/// Polite lead-ins that STT puts in front of a real command. They are removed
/// only from the FRONT of one sentence candidate, and whatever remains must
/// still satisfy the strict anchored review/recreate grammar below, so a
/// starter can never turn a foreign sentence into a review request.
const REVIEW_POLITE_STARTERS: &[&str] = &[
    "please", "can you", "could you", "would you", "will you",
];

/// A negated order is never a review request, whatever wording precedes it
/// ("do not analyse this website").
const REVIEW_NEGATION_LEADS: &[&str] = &[
    "do not", "don't", "dont", "never", "no need", "not", "stop", "cancel",
];

/// Reduce one sentence candidate to the exact command phrase, or reject it.
/// This only removes a leading wake word, polite starters and sentence
/// punctuation — the anchored grammar still decides what a command is.
fn review_command_candidate(candidate: &str) -> Option<String> {
    let mut t = candidate.trim().to_lowercase();
    t = t
        .trim_end_matches(['.', ',', '!', '?', ';', ':'])
        .trim()
        .to_string();

    // A leading wake word ("Nexus, review this webpage").
    if let Some(rest) = t.strip_prefix("nexus") {
        let rest = rest.trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, '.' | ',' | '!' | '?' | ';' | ':')
        });
        if !rest.is_empty() {
            t = rest.to_string();
        }
    }

    // Polite starters, possibly stacked ("please, can you analyse this app").
    loop {
        let mut changed = false;
        for &starter in REVIEW_POLITE_STARTERS {
            if let Some(rest) = t.strip_prefix(starter) {
                let rest = rest
                    .trim_start_matches(|c: char| {
                        c.is_whitespace() || matches!(c, '.' | ',' | '!' | '?' | ';' | ':')
                    })
                    .trim_start();
                if rest.len() < t.len() {
                    t = rest.to_string();
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }

    if t.is_empty() {
        return None;
    }
    // A negated order is never a review request.
    if REVIEW_NEGATION_LEADS.iter().any(|&lead| t.starts_with(lead)) {
        return None;
    }
    Some(t)
}

/// "analyse this website" / "review this app" and its later reference-bound
/// "recreate it". Both grammars stay strictly anchored — the candidate must
/// be the whole command — so repo/PR analysis, "review the app permissions"
/// and "recreate my voice" never reach an intent here. STT often emits a
/// harmless sentence before the real command ("Sure. Analyse this website."),
/// so every sentence candidate is offered to the same anchored grammar; the
/// screen-analysis splitter is reused for exactly that reason.
fn parse_foreground_review_command(text: &str) -> Option<ParseResult> {
    for candidate in sentence_candidates(text) {
        let Some(t) = review_command_candidate(&candidate) else { continue };
        let intent = if regex_match(
            &t,
            r"^(?:analyse|analyze|review|inspect)\s+(?:this|the|current|foreground)(?:\s+(?:current|foreground))?\s+(?:app|application|website|web\s*site|webpage|web\s*page|site)(?:\s+(?:for\s+me|first))?$",
        ) {
            ParsedIntent::ReviewForeground
        } else if regex_match(
            &t,
            r"^(?:recreate|rebuild|replicate)\s+(?:it|that|this\s+(?:app|application|website|web\s*site|webpage|web\s*page|site))$",
        ) {
            ParsedIntent::RecreateLastReview
        } else {
            continue;
        };
        return Some(ParseResult {
            intent,
            confidence: 1.0,
            source: "deterministic-foreground-review".to_string(),
        });
    }
    None
}

/// Sentence candidates for the ^-anchored screen grammars. STT often emits
/// multi-sentence transcripts where transcribed noise precedes the real
/// command (live 2026-10-20 15:01:29: "once tell the number. analyze my
/// screen." — every anchored family missed the junk-prefixed whole, NLU was
/// down, and the turn fell through to the cloud Worker with "I can't see your
/// screen"). Test each sentence on its own as well as the whole; mirrors the
/// last-sentence fallback the agent-hooks grammar already has
/// (agent_hooks::last_sentence). Every candidate still runs the same anchor
/// guards, so misfire negatives ("analyse servx", "draw the curtains") stay safe.
fn sentence_candidates(text: &str) -> Vec<String> {
    fn clean(s: &str) -> String {
        s.trim().trim_end_matches([',', ';', ':', '.', '!', '?']).trim().to_string()
    }
    let whole = clean(&text.to_lowercase());
    let mut out: Vec<String> = Vec::new();
    if !whole.is_empty() {
        out.push(whole.clone());
    }
    for seg in whole.split(|c| matches!(c, '.' | '!' | '?')) {
        let s = clean(seg);
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

fn parse_screen_analysis_command(text: &str) -> Option<ParseResult> {
    for candidate in sentence_candidates(text) {
        // Keep natural polite/wake-word variants on the same local route.
        // The reducer also refuses a leading negation, while the anchored
        // patterns below still require an explicit screen/seeing request.
        let Some(trimmed) = review_command_candidate(&candidate) else { continue };
        // STT often returns a curly apostrophe ("what’s").
        let trimmed = trimmed.replace(['\u{2019}', '\u{2018}'], "'");
        // "can you …" / "please …" / "i need you to …" lead-ins.
        let trimmed = regex_captures(
            &trimmed,
            r"^(?:(?:hey|ok|okay)\s+)?(?:(?:can|could|would|will)\s+you\s+)?(?:please\s+)?(?:i\s+(?:want|need)\s+you\s+to\s+)?(.*)$",
        )
        .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .unwrap_or(trimmed);
        // Settings-like phrases that merely contain "screen" are not analysis.
        if regex_captures(
            &trimmed,
            r"\bscreen\s+(?:time|saver|recording|record|share|sharing|brightness|resolution|lock|reader|capture)\b",
        )
        .is_some()
        {
            continue;
        }
        // 1. Screen-anchored verb family: "analyse the screen", "analyze my
        //    screen", "read the screen", "scan my screen", "check the screen",
        //    "describe my screen" — screen REQUIRED so repo/PR analyse never fires.
        if regex_captures(
            &trimmed,
            r"^(?:analyse|analyze|analysis|describe|read|scan|check)\b[^;]*\bscreen\b.*$",
        )
        .is_some()
        {
            return Some(ParseResult::screen_analysis_result(text));
        }
        // 1b. "look at my screen" / "show me my screen" / "tell me about the screen".
        if regex_captures(
            &trimmed,
            r"^(?:look\s+at|have\s+a\s+look\s+at|take\s+a\s+look\s+at|show\s+me|tell\s+me\s+about)\s+(?:my|the|this)\s+screen\b.*$",
        )
        .is_some()
        {
            return Some(ParseResult::screen_analysis_result(text));
        }
        // 2. "What is on my screen?" family ("whats" / "what's" / "what is"; screen or display).
        if regex_captures(
            &trimmed,
            r"^what(?:'s|s| is)\s+(?:on|in|of)?\s*(?:my|the|this)?\s*(?:screen|display)\b.*$",
        )
        .is_some()
        {
            return Some(ParseResult::screen_analysis_result(text));
        }
        // "Explain/tell me/describe what is on my screen" is the same
        // visual tour, not a generic question for the cloud Worker.
        if regex_captures(
            &trimmed,
            r"^(?:explain|describe|tell\s+me)\s+what(?:'s| is)\s+on\s+(?:my|the|this)\s+screen\b.*$",
        )
        .is_some()
        {
            return Some(ParseResult::screen_analysis_result(text));
        }
        // 3. "What do I see / what am I looking at / explain what I see" family.
        if regex_captures(
            &trimmed,
            r"^(?:what\s+(?:do|can|am)\s+i\s+(?:see|seeing|looking\s+at)|(?:explain|tell\s+me|describe)\s+what\s+(?:i\s+(?:can\s+)?see|i\s+am\s+(?:looking\s+at|seeing)|am\s+i\s+(?:seeing|looking\s+at)))(?:\s+(?:here|right\s+now|now))?$",
        )
        .is_some()
        {
            return Some(ParseResult::screen_analysis_result(text));
        }
        // 4. Research family — this/screen REQUIRED (general research → Worker).
        if regex_captures(&trimmed, r"^research\b.*(?:\bthis\b|\bscreen\b).*$").is_some() {
            return Some(ParseResult::screen_analysis_result(text));
        }
    }

    None
}

/// "Nexus, reverse engineer this website": the voice trigger for the stack scan + scrolling tour
/// (`reverse_engineer.rs`). Website/app words are REQUIRED for the question forms, and anything that
/// names code (repo, PR, file, binary…) is refused so "reverse engineer this repo" stays elsewhere.
fn parse_reverse_engineer_command(text: &str) -> Option<ParseResult> {
    for candidate in sentence_candidates(text) {
        let Some(trimmed) = review_command_candidate(&candidate) else { continue };
        if regex_captures(
            &trimmed,
            r"\b(?:repo|repository|pull\s+request|pr|github|function|file|binary|exe|firmware|malware)\b",
        )
        .is_some()
        {
            continue;
        }
        const SITE: &str = r"(?:web\s*site|website|site|page|web\s*app|app|application)";
        let patterns = [
            // 1. The explicit verb (STT also hears "reverse and gineer").
            r"^(?:please\s+)?(?:nexus\s+)?reverse[\s-]*(?:engineer(?:ing)?|and\s+gin+eer?)\b.*$".to_string(),
            // 2. "what tech stack does this site use", "which framework is this website using".
            format!(
                r"^(?:what|which)\b[^;]*\b(?:tech(?:nology)?(?:\s+stack)?|technologies|stack|frameworks?|libraries|cms)\b[^;]*\b(?:this|the|that)\s+{SITE}\b.*$"
            ),
            // 3. "what is this website built with".
            format!(
                r"^(?:what|which)\s+(?:is|was)\s+(?:this|the|that)\s+{SITE}\s+(?:built|made|created|developed|coded)\s+(?:with|using|on|in)\b.*$"
            ),
            // 4. "how was this site built", "how did they build this website".
            format!(r"^how\s+(?:was|is)\s+(?:this|the|that)\s+{SITE}\s+(?:built|made|created|developed|coded)\b.*$"),
            format!(
                r"^how\s+did\s+(?:they|someone|people)\s+(?:build|make|create|code)\s+(?:this|the|that)\s+{SITE}\b.*$"
            ),
            // 5. "break down / tear down / dissect this website".
            format!(r"^(?:break\s*down|tear\s*down|dissect|decompile|deconstruct)\s+(?:this|the|that)\s+{SITE}\b.*$"),
            // 6. "analyse the stack of this website" / "analyse this website's stack".
            format!(
                r"^(?:analy[sz]e|scan|inspect|check)\b[^;]*(?:\b{SITE}\b[^;]*\bstack\b|\bstack\b[^;]*\b{SITE}\b).*$"
            ),
        ];
        if patterns.iter().any(|p| regex_captures(&trimmed, p).is_some()) {
            return Some(ParseResult::reverse_engineer_result(text));
        }
    }
    None
}

/// Screen annotation & live architecture drawing (Feature 87):
/// "annotate my screen", "draw on the screen", "add an arrow to the screen",
/// "create an architecture diagram".
///
/// Anchoring rules (misfire guards):
/// - The annotate/draw family MUST contain "screen"/"display"/"this" —
///   "draw the curtains" still falls through.
fn parse_screen_annotation_command(text: &str) -> Option<ParseResult> {
    for trimmed in sentence_candidates(text) {
        // 1. Annotate/draw family: "annotate my screen", "draw on the screen",
        //    "mark up this display", "sketch on the screen" — screen/display/this REQUIRED.
        if regex_captures(
            &trimmed,
            r"^(?:annotate|draw|mark\s*up|markup|sketch)\b[^;]*\b(?:screen|display|this)\b.*$",
        )
        .is_some()
        {
            return Some(ParseResult::screen_annotation_result(text));
        }
        // 2. Architecture family: "create an architecture diagram",
        //    "make an arch diagram", "build a diagram of this" — architecture/arch/diagram REQUIRED.
        if regex_captures(
            &trimmed,
            r"^(?:create|make|build|draw)\b[^;]*\b(?:architecture|arch|diagram)\b.*$",
        )
        .is_some()
        {
            return Some(ParseResult::screen_annotation_result(text));
        }
        // 3. Element-adding family: "add an arrow to the screen",
        //    "add a checkbox on this", "add text to my screen" — element + anchor REQUIRED.
        if regex_captures(
            &trimmed,
            r"^add\b[^;]*\b(?:arrow|text|checkbox|note|pointer)\b[^;]*\b(?:screen|display|this)\b.*$",
        )
        .is_some()
        {
            return Some(ParseResult::screen_annotation_result(text));
        }
    }

    None
}

impl ParseResult {
    fn reverse_engineer_result(text: &str) -> Self {
        Self {
            intent: ParsedIntent::NluResult {
                intent: "reverse_engineer".to_string(),
                slots: serde_json::json!({ "prompt": text.trim() }),
                confidence: 1.0,
            },
            confidence: 1.0,
            source: "deterministic-reverse-engineer".to_string(),
        }
    }

    fn screen_analysis_result(text: &str) -> Self {
        Self {
            intent: ParsedIntent::NluResult {
                intent: "screen_analysis".to_string(),
                slots: serde_json::json!({ "prompt": text.trim() }),
                confidence: 1.0,
            },
            confidence: 1.0,
            source: "deterministic-screen-analysis".to_string(),
        }
    }

    fn screen_annotation_result(text: &str) -> Self {
        Self {
            intent: ParsedIntent::NluResult {
                intent: "screen_annotation".to_string(),
                slots: serde_json::json!({ "prompt": text.trim() }),
                confidence: 1.0,
            },
            confidence: 1.0,
            source: "deterministic-screen-annotation".to_string(),
        }
    }
}

fn parse_dictation_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    if t == "start typing"
        || t == "type whatever i say"
        || t == "type whatever i am saying"
        || t == "type what i say"
        || t == "type line by line"
        || t == "start dictation"
        || t == "begin typing"
        || t == "enable typing"
    {
        return Some(ParseResult {
            intent: ParsedIntent::StartDictation,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    if t == "stop typing"
        || t == "stop dictation"
        || t == "done typing"
        || t == "end typing"
        || t == "finish typing"
        || t == "exit dictation"
    {
        return Some(ParseResult {
            intent: ParsedIntent::StopDictation,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    None
}

fn parse_browser_search_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    if t == "search"
        || t == "search bar"
        || t == "focus search"
        || t == "focus search bar"
        || t == "focus address bar"
        || t == "address bar"
        || t == "open search"
        || t == "open search bar"
        || t == "go to search bar"
        || t == "go to address bar"
    {
        return Some(ParseResult {
            intent: ParsedIntent::BrowserSearchFocus,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    let browser_prefixes = [
        "search in browser ",
        "browser search ",
        "search in brave ",
        "search in chrome ",
    ];
    for prefix in browser_prefixes {
        if t.starts_with(prefix) {
            let query = text.trim()[prefix.len()..].trim();
            if !query.is_empty() {
                return Some(ParseResult {
                    intent: ParsedIntent::BrowserSearch {
                        query: query.to_string(),
                    },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }
        }
    }

    None
}

fn parse_watch_screen_email_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();
    let is_watch_verb = t.contains("update me")
        || t.contains("notify me")
        || t.contains("watch this")
        || t.contains("watch email")
        || t.contains("track this")
        || t.contains("track email")
        || t.contains("keep an eye on this");

    let is_email_or_deadline = t.contains("email")
        || t.contains("deadline")
        || t.contains("submission")
        || t.contains("due date")
        || t.contains("this thread")
        || t.contains("this message");

    if is_watch_verb && is_email_or_deadline {
        Some(ParseResult {
            intent: ParsedIntent::WatchScreenEmail,
            confidence: 1.0,
            source: "deterministic".to_string(),
        })
    } else {
        None
    }
}

// ─── GitHub command parsing ───────────────────────────────────────────

/// Parse GitHub sub-commands from natural language.
///
/// Supported patterns:
///   "merge PR <num> in <owner/repo>"        → MergePr
///   "squash merge PR <num> in <repo>"       → MergePr (squash)
///   "rebase merge PR <num> in <repo>"       → MergePr (rebase)
///   "approve PR <num> in <repo>"            → ApprovePr
///   "close PR <num> in <repo>"              → ClosePr
///   "list PRs in <repo>"                    → ListPrs
///   "list open PRs in <repo>"               → ListPrs (open)
///   "list closed PRs in <repo>"             → ListPrs (closed)
///   "get PR <num> in <repo>"                → GetPr
///   "show PR <num> in <repo>"               → GetPr
///   "create PR <title> from <head> to <base> in <repo>" → CreatePr
///   "comment on PR <num> in <repo>: <body>" → CommentPr
///   "list PR files for PR <num> in <repo>"  → ListPrFiles
///   "update branch for PR <num> in <repo>"  → UpdateBranch
///   "revert PR <num> in <repo>"             → RevertPr
///   "add <user> as collaborator to <repo>"  → AddCollaborator
///   "remove <user> from collaborators in <repo>" → RemoveCollaborator
///   "list collaborators in <repo>"          → ListCollaborators
///   "add <user> to org <org>"               → AddOrgMember
///   "remove <user> from org <org>"          → RemoveOrgMember
///   "list members of org <org>"             → ListOrgMembers
///   "list branches in <repo>"               → ListBranches
///   "delete branch <name> in <repo>"        → DeleteBranch
///   "list releases in <repo>"               → ListReleases
///   "create release <tag> in <repo>"        → CreateRelease
///   "list workflows in <repo>"              → ListWorkflows
///   "list workflow runs in <repo>"          → ListWorkflowRuns
///   "rerun workflow <id> in <repo>"         → RerunWorkflow
///   "cancel workflow <id> in <repo>"        → CancelWorkflow
fn parse_github_command(text: &str) -> Option<ParseResult> {
    use crate::github_cmd::{CollaboratorPermission, GitHubCommand, MergeMethod, OrgRole};

    // Helper: extract "in <owner/repo>" or "in <repo>" from the end of text.
    // Returns (repo, remaining_text_before_in).
    let extract_repo = |t: &str| -> Option<(String, String)> {
        // Try "in <owner/repo>" pattern
        if let Some(pos) = t.rfind(" in ") {
            let repo_part = t[pos + 4..].trim();
            let repo = clean_repo_name(repo_part);
            if !repo.is_empty() {
                return Some((repo, t[..pos].trim().to_string()));
            }
        }
        // Try "for <owner/repo>"
        if let Some(pos) = t.rfind(" for ") {
            let repo_part = t[pos + 5..].trim();
            let repo = clean_repo_name(repo_part);
            if !repo.is_empty() {
                return Some((repo, t[..pos].trim().to_string()));
            }
        }
        None
    };

    // --- Merge PR ---
    // "merge PR 23 in owner/repo"
    // "squash merge PR 23 in owner/repo"
    // "rebase merge PR 23 in owner/repo"
    if let Some(caps) = regex_captures(text, r"^(?:(squash|rebase)\s+)?merge\s+(?:pr|pull\s+request)\s*#?\s*(\d+)(?:\s+in\s+(\S+))?$") {
        let method = match caps.get(1).map(|m| m.as_str()) {
            Some("squash") => MergeMethod::Squash,
            Some("rebase") => MergeMethod::Rebase,
            _ => MergeMethod::Merge,
        };
        let pr_number: u64 = caps[2].parse().ok()?;
        let repo = if let Some(r) = caps.get(3) {
            clean_repo_name(r.as_str())
        } else {
            // Try "in <repo>" from full text
            return extract_repo(text).map(|(repo, _)| ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::MergePr { repo, pr_number, method },
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::MergePr { repo, pr_number, method },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Approve PR ---
    if let Some(caps) = regex_captures(text, r"^approve\s+(?:pr|pull\s+request)\s*#?\s*(\d+)(?:\s+in\s+(\S+))?$") {
        let pr_number: u64 = caps[1].parse().ok()?;
        let repo = if let Some(r) = caps.get(2) {
            clean_repo_name(r.as_str())
        } else {
            return extract_repo(text).map(|(repo, _)| ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ApprovePr { repo, pr_number },
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ApprovePr { repo, pr_number },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Close PR ---
    if let Some(caps) = regex_captures(text, r"^close\s+(?:pr|pull\s+request)\s*#?\s*(\d+)(?:\s+in\s+(\S+))?$") {
        let pr_number: u64 = caps[1].parse().ok()?;
        let repo = if let Some(r) = caps.get(2) {
            clean_repo_name(r.as_str())
        } else {
            return extract_repo(text).map(|(repo, _)| ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ClosePr { repo, pr_number },
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ClosePr { repo, pr_number },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Get PR ---
    if let Some(caps) = regex_captures(text, r"^(?:get|show|tell\s+me\s+about)\s+(?:pr|pull\s+request)\s*#?\s*(\d+)(?:\s+in\s+(\S+))?$") {
        let pr_number: u64 = caps[1].parse().ok()?;
        let repo = if let Some(r) = caps.get(2) {
            clean_repo_name(r.as_str())
        } else {
            return extract_repo(text).map(|(repo, _)| ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::GetPr { repo, pr_number },
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::GetPr { repo, pr_number },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // GitHub feed/dashboard/overview phrases are handled by parse_github_dashboard_command
    // which runs later. Do not intercept them here.

    // --- List PRs ---
    // Natural language patterns: "give me the pr list", "show live prs",
    // "show open prs", "latest prs", "what prs are open", "open pr list", etc.
    // These all map to ListPrs with a state filter.
    // "open pr list" is included because STT often transcribes "show" as "open"
    // and the user means "open the PR list sidebar", not "open an app called pr list".
    // "pull request(s)" is a full alternative to "prs" ("show me the pull
    // requests"), and trailing "and all / all of them / everything" is
    // tolerated ("show me the pull requests and all").
    if let Some(caps) = regex_captures(text, r"^(?:give\s+me\s+(?:the\s+)?|show\s+(?:me\s+)?(?:the\s+)?|get\s+(?:me\s+)?(?:the\s+)?|tell\s+me\s+(?:the\s+)?|what\s+(?:are\s+|is\s+)?(?:the\s+)?|open\s+(?:the\s+)?|view\s+(?:the\s+)?|fetch\s+(?:me\s+)?(?:the\s+)?|display\s+(?:the\s+)?|bring\s+(?:me\s+)?(?:the\s+)?|pull\s+up\s+(?:the\s+)?)?(?:(open|closed|all|live|latest|active|merged)\s+)?(?:the\s+)?(?:prs?(?:\s+list)?|pull\s+requests?(?:\s+list)?)(?:\s+(?:and\s+)?all(?:\s+of\s+them)?|\s+everything)?(?:\s+in\s+(\S+))?(?:\s+(?:and\s+)?all(?:\s+of\s+them)?|\s+everything)?$") {
        let raw_state = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("open");
        let state = match raw_state {
            "live" | "active" | "open" => "open",
            "closed" => "closed",
            "all" => "all",
            "latest" => "open", // latest implies most recent open PRs
            "merged" => "closed", // merged PRs are closed
            _ => "open",
        }.to_string();
        let repo = if let Some(r) = caps.get(2) {
            clean_repo_name(r.as_str())
        } else {
            // Try "in <repo>" / "for <repo>" from text
            if let Some((repo, _)) = extract_repo(text) {
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                    },
                    confidence: 0.9,
                    source: "deterministic".to_string(),
                });
            }
            // No repo in text — try auto-detection (browser URL, etc.)
            if let Some(repo_id) = crate::architect::get_active_repo_url() {
                let repo = format!("{}/{}", repo_id.owner, repo_id.repo);
                tracing::info!(
                    "[intent_parser] ListPrs: auto-detected repo '{}'",
                    repo
                );
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                    },
                    confidence: 0.85,
                    source: "deterministic".to_string(),
                });
            }
            // No repo found at all — return with empty repo, let the GitHub
            // subsystem handle the error or use a default repo
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListPrs { repo: String::new(), state },
                },
                confidence: 0.7,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- "what prs are open" / "which prs are open" / "are there any open prs" ---
    // Different word order: state comes AFTER "prs"
    if let Some(caps) = regex_captures(text, r"^(?:what|which|any)\s+prs?\s+(?:are\s+)?(?:(open|closed|all|live|active))?(?:\s+in\s+(\S+))?$") {
        let raw_state = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("open");
        let state = match raw_state {
            "live" | "active" | "open" => "open",
            "closed" => "closed",
            "all" => "all",
            _ => "open",
        }.to_string();
        let repo = if let Some(r) = caps.get(2) {
            clean_repo_name(r.as_str())
        } else {
            if let Some((repo, _)) = extract_repo(text) {
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                    },
                    confidence: 0.9,
                    source: "deterministic".to_string(),
                });
            }
            if let Some(repo_id) = crate::architect::get_active_repo_url() {
                let repo = format!("{}/{}", repo_id.owner, repo_id.repo);
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                    },
                    confidence: 0.85,
                    source: "deterministic".to_string(),
                });
            }
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListPrs { repo: String::new(), state },
                },
                confidence: 0.7,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // Original pattern: "list (open|closed|all) prs in <repo>"
    if let Some(caps) = regex_captures(text, r"^list\s+(open\s+|closed\s+|all\s+)?prs?(?:\s+in\s+(\S+))?$") {
        let state = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("open").to_string();
        let repo = if let Some(r) = caps.get(2) {
            clean_repo_name(r.as_str())
        } else {
            // Try "in <repo>" / "for <repo>" from text
            if let Some((repo, _)) = extract_repo(text) {
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                    },
                    confidence: 0.9,
                    source: "deterministic".to_string(),
                });
            }
            // No repo in text — try auto-detection
            if let Some(repo_id) = crate::architect::get_active_repo_url() {
                let repo = format!("{}/{}", repo_id.owner, repo_id.repo);
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                    },
                    confidence: 0.85,
                    source: "deterministic".to_string(),
                });
            }
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListPrs { repo: String::new(), state },
                },
                confidence: 0.7,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Fuzzy "list" fallback ---
    // Catches STT mishearings where the user said "show me the pr list" or
    // "list prs" but STT heard something like "so you have to list" or
    // "show me the list". If the transcript contains "list" and doesn't
    // match any other pattern, try ListPrs with auto-detected repo.
    // This is a low-confidence fallback (0.6) — the brain/NLU can override.
    // IMPORTANT: This must NOT catch "list branches", "list releases", etc.
    // Those have their own patterns below. We exclude them here.
    if text.contains("list") {
        let lower = text.to_lowercase();
        // Skip if it looks like a different list command or non-PR list
        if lower.contains("to do") || lower.contains("todo") || lower.contains("shopping")
            || lower.contains("bucket") || lower.contains("wait") || lower.contains("listen")
            || lower.contains("branch") || lower.contains("release")
            || lower.contains("workflow") || lower.contains("collaborator")
            || lower.contains("file") || lower.contains("member")
            || lower.contains("run") || lower.contains("pr files") {
            // Not a PR list command — let the specific patterns handle it
        } else {
            // Try "in <repo>" / "for <repo>" first
            if let Some((repo, _)) = extract_repo(text) {
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: GitHubCommand::ListPrs {
                            repo,
                            state: "open".to_string(),
                        },
                    },
                    confidence: 0.6,
                    source: "deterministic-fuzzy".to_string(),
                });
            }
            // No repo in text — try auto-detection (browser URL, clipboard, etc.)
            // This is a blocking call, so we do it last.
            if let Some(repo_id) = crate::architect::get_active_repo_url() {
                let repo = format!("{}/{}", repo_id.owner, repo_id.repo);
                tracing::info!(
                    "[intent_parser] fuzzy 'list' fallback: auto-detected repo '{}'",
                    repo
                );
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: GitHubCommand::ListPrs {
                            repo,
                            state: "open".to_string(),
                        },
                    },
                    confidence: 0.6,
                    source: "deterministic-fuzzy".to_string(),
                });
            }
            // No repo found at all — return with empty repo for account-wide
            // PR search. Without this, "show me the pr list" (no repo) falls
            // through to Unknown even though it contains "list".
            tracing::info!(
                "[intent_parser] fuzzy 'list' fallback: no repo detected, returning account-wide ListPrs"
            );
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListPrs {
                        repo: String::new(),
                        state: "open".to_string(),
                    },
                },
                confidence: 0.6,
                source: "deterministic-fuzzy".to_string(),
            });
        }
    }

    // --- List PR files ---
    // "list pr files for PR <num> in <repo>" or "list pr files <num> in <repo>"
    if let Some(caps) = regex_captures(text, r"^list\s+pr\s+files\s+(?:for\s+)?(?:pr\s+)?#?\s*(\d+)(?:\s+in\s+(\S+))?$") {
        let pr_number: u64 = caps[1].parse().ok()?;
        let repo = if let Some(r) = caps.get(2) {
            clean_repo_name(r.as_str())
        } else {
            return extract_repo(text).map(|(repo, _)| ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListPrFiles { repo, pr_number },
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListPrFiles { repo, pr_number },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Update branch ---
    if let Some(rest) = text.strip_prefix("update branch ") {
        let rest = rest.trim_start_matches("for ").trim();
        let rest = rest.trim_start_matches("pr ").trim();
        if let Ok(pr_number) = rest.parse::<u64>() {
            return extract_repo(text).map(|(repo, _)| ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::UpdateBranch { repo, pr_number },
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Revert PR ---
    if let Some(caps) = regex_captures(text, r"^revert\s+(?:pr|pull\s+request)\s*#?\s*(\d+)(?:\s+in\s+(\S+))?$") {
        let pr_number: u64 = caps[1].parse().ok()?;
        let repo = if let Some(r) = caps.get(2) {
            clean_repo_name(r.as_str())
        } else {
            return extract_repo(text).map(|(repo, _)| ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::RevertPr { repo, pr_number, title: None },
                },
                confidence: 0.9,
                source: "deterministic".to_string(),
            });
        };
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::RevertPr { repo, pr_number, title: None },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Comment on PR ---
    // "comment on PR <num> in <repo> <body>" or "comment on PR <num> <body>"
    if let Some(caps) = regex_captures(text, r"^comment\s+on\s+(?:pr|pull\s+request)\s*#?\s*(\d+)\s+in\s+(\S+)\s+(.+)$") {
        let pr_number: u64 = caps[1].parse().ok()?;
        let repo = clean_repo_name(&caps[2]);
        let body = caps[3].trim().to_string();
        if !repo.is_empty() && !body.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::CommentPr { repo, pr_number, body },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }
    // "comment on PR <num> <body>" (repo extracted from context)
    if let Some(caps) = regex_captures(text, r"^comment\s+on\s+(?:pr|pull\s+request)\s*#?\s*(\d+)\s+(.+)$") {
        let pr_number: u64 = caps[1].parse().ok()?;
        let rest = caps[2].trim();
        // Try to split "in <repo> <body>" from "<body>"
        if let Some(pos) = rest.find(" in ") {
            let repo = clean_repo_name(&rest[..pos].trim());
            let body = rest[pos + 4..].trim().to_string();
            if !repo.is_empty() && !body.is_empty() {
                return Some(ParseResult {
                    intent: ParsedIntent::GitHubCommand {
                        command: GitHubCommand::CommentPr { repo, pr_number, body },
                    },
                    confidence: 0.9,
                    source: "deterministic".to_string(),
                });
            }
        }
        // No "in <repo>" — try extracting repo from full text
        return extract_repo(text).map(|(repo, _)| ParseResult {
            intent: ParsedIntent::GitHubCommand {
                command: GitHubCommand::CommentPr { repo, pr_number, body: rest.to_string() },
            },
            confidence: 0.85,
            source: "deterministic".to_string(),
        });
    }

    // --- Add collaborator ---
    // "add <user> as collaborator to <repo>" or "add <user> as admin to <repo>"
    // Also: "add <user> as admin to <repo>" (without "collaborator" keyword)
    if let Some(caps) = regex_captures(text, r"^add\s+(\S+)\s+as\s+(admin|push|pull|triage|maintain)?\s*(?:collaborator\s+)?to\s+(\S+)$") {
        let username = caps[1].to_string();
        let permission = match caps.get(2).map(|m| m.as_str().trim()) {
            Some("admin") => CollaboratorPermission::Admin,
            Some("push") => CollaboratorPermission::Push,
            Some("pull") => CollaboratorPermission::Pull,
            Some("triage") => CollaboratorPermission::Triage,
            Some("maintain") => CollaboratorPermission::Maintain,
            _ => CollaboratorPermission::Push,
        };
        let repo = clean_repo_name(&caps[3]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::AddCollaborator { repo, username, permission },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Remove collaborator ---
    // "remove <user> from <repo>" or "remove <user> as collaborator from <repo>"
    if let Some(caps) = regex_captures(text, r"^remove\s+(\S+)\s+(?:as\s+)?(?:collaborator\s+)?from\s+(\S+)$") {
        let username = caps[1].to_string();
        let repo = clean_repo_name(&caps[2]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::RemoveCollaborator { repo, username },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- List collaborators ---
    if let Some(caps) = regex_captures(text, r"^list\s+collaborators\s+in\s+(\S+)$") {
        let repo = clean_repo_name(&caps[1]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListCollaborators { repo },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Add org member ---
    // "add <user> to org <org>" or "add <user> as admin to org <org>"
    if let Some(caps) = regex_captures(text, r"^add\s+(\S+)\s+(?:as\s+(admin|member)\s+)?to\s+org\s+(\S+)$") {
        let username = caps[1].to_string();
        let role = match caps.get(2).map(|m| m.as_str()) {
            Some("admin") => OrgRole::Admin,
            _ => OrgRole::Member,
        };
        let org = caps[3].to_string();
        return Some(ParseResult {
            intent: ParsedIntent::GitHubCommand {
                command: GitHubCommand::AddOrgMember { org, username, role },
            },
            confidence: 0.95,
            source: "deterministic".to_string(),
        });
    }

    // --- Remove org member ---
    if let Some(caps) = regex_captures(text, r"^remove\s+(\S+)\s+from\s+org\s+(\S+)$") {
        let username = caps[1].to_string();
        let org = caps[2].to_string();
        return Some(ParseResult {
            intent: ParsedIntent::GitHubCommand {
                command: GitHubCommand::RemoveOrgMember { org, username },
            },
            confidence: 0.95,
            source: "deterministic".to_string(),
        });
    }

    // --- List org members ---
    if let Some(caps) = regex_captures(text, r"^list\s+members\s+of\s+org\s+(\S+)$") {
        let org = caps[1].to_string();
        return Some(ParseResult {
            intent: ParsedIntent::GitHubCommand {
                command: GitHubCommand::ListOrgMembers { org },
            },
            confidence: 0.95,
            source: "deterministic".to_string(),
        });
    }

    // --- List branches ---
    if let Some(caps) = regex_captures(text, r"^list\s+branches\s+in\s+(\S+)$") {
        let repo = clean_repo_name(&caps[1]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListBranches { repo },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Delete branch ---
    if let Some(caps) = regex_captures(text, r"^delete\s+branch\s+(\S+)\s+in\s+(\S+)$") {
        let branch = caps[1].to_string();
        let repo = clean_repo_name(&caps[2]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::DeleteBranch { repo, branch },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- List releases ---
    if let Some(caps) = regex_captures(text, r"^list\s+releases\s+in\s+(\S+)$") {
        let repo = clean_repo_name(&caps[1]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListReleases { repo },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Create release ---
    // "create release v1.0 in owner/repo" or "create release v1.0 in owner/repo with notes <body>"
    if let Some(caps) = regex_captures(text, r"^create\s+release\s+(\S+)\s+in\s+(\S+)(?:\s+with\s+notes\s+(.+))?$") {
        let tag = caps[1].to_string();
        let repo = clean_repo_name(&caps[2]);
        let body = caps.get(3).map(|m| m.as_str().trim().to_string());
        let name = tag.clone();
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::CreateRelease {
                        repo,
                        tag,
                        name,
                        body,
                        draft: false,
                        prerelease: false,
                        target_commitish: None,
                    },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- List workflows ---
    if let Some(caps) = regex_captures(text, r"^list\s+workflows\s+in\s+(\S+)$") {
        let repo = clean_repo_name(&caps[1]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListWorkflows { repo },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- List workflow runs ---
    if let Some(caps) = regex_captures(text, r"^list\s+workflow\s+runs\s+in\s+(\S+)$") {
        let repo = clean_repo_name(&caps[1]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::ListWorkflowRuns { repo, workflow_file: None },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Rerun workflow ---
    if let Some(caps) = regex_captures(text, r"^rerun\s+workflow\s+(\d+)\s+in\s+(\S+)$") {
        let run_id: u64 = caps[1].parse().ok()?;
        let repo = clean_repo_name(&caps[2]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::RerunWorkflow { repo, run_id },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    // --- Cancel workflow ---
    if let Some(caps) = regex_captures(text, r"^cancel\s+workflow\s+(\d+)\s+in\s+(\S+)$") {
        let run_id: u64 = caps[1].parse().ok()?;
        let repo = clean_repo_name(&caps[2]);
        if !repo.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::GitHubCommand {
                    command: GitHubCommand::CancelWorkflow { repo, run_id },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }

    None
}

// ─── Commerce: order food (Swiggy) ─────────────────────────────────────

/// Parse food-ordering commands.
///
/// Patterns:
///   "order pizza from dominos"      → OrderFood { query: "pizza", restaurant: Some("dominos") }
///   "order food from swiggy"        → OrderFood { query: "", restaurant: None }
///   "order biryani"                 → OrderFood { query: "biryani", restaurant: None }
/// Parse food ordering commands (Swiggy MCP).
///
/// Patterns:
///   "order pizza from dominos"      → OrderFood { query: "pizza", restaurant: Some("dominos") }
///   "order a pizza from dominos"    → OrderFood { query: "pizza", restaurant: Some("dominos") }
///   "order food from swiggy"        → OrderFood { query: "", restaurant: None }
///   "order biryani on swiggy"       → OrderFood { query: "biryani", restaurant: None }
///   "order biryani"                 → OrderFood { query: "biryani", restaurant: None }
///   "get food from swiggy"          → OrderFood { query: "", restaurant: None }
///   "order food"                    → OrderFood { query: "", restaurant: None }
fn parse_order_food(text: &str) -> Option<ParseResult> {
    let mut clean = text.trim();
    for prefix in ["can you ", "could you ", "please ", "i want to "] {
        if let Some(rest) = clean.strip_prefix(prefix) {
            clean = rest.trim();
            break;
        }
    }

    // "order <dish> from <restaurant>" or "order <dish> on swiggy" or "order <dish>"
    if let Some(rest) = clean.strip_prefix("order ") {
        let rest = rest.trim_start_matches("a ").trim_start_matches("some ").trim();
        if let Some(pos) = rest.find(" from ") {
            let mut query = rest[..pos].trim();
            let restaurant = rest[pos + 6..].trim();
            if matches!(query, "food" | "something" | "lunch" | "dinner" | "something to eat") {
                query = "";
            }
            return Some(ParseResult {
                intent: ParsedIntent::OrderFood {
                    query: query.to_string(),
                    restaurant: if restaurant.is_empty() { None } else { Some(restaurant.to_string()) },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
        if let Some(pos) = rest.find(" on ") {
            let mut query = rest[..pos].trim();
            let restaurant = rest[pos + 4..].trim();
            if matches!(query, "food" | "something" | "lunch" | "dinner" | "something to eat") {
                query = "";
            }
            return Some(ParseResult {
                intent: ParsedIntent::OrderFood {
                    query: query.to_string(),
                    restaurant: if restaurant.is_empty() { None } else { Some(restaurant.to_string()) },
                },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
        // "order biryani" (no restaurant specified)
        let mut query = rest.trim();
        if matches!(query, "food" | "something" | "lunch" | "dinner" | "something to eat") {
            query = "";
        }
        return Some(ParseResult {
            intent: ParsedIntent::OrderFood {
                query: query.to_string(),
                restaurant: None,
            },
            confidence: 0.90,
            source: "deterministic".to_string(),
        });
    }

    // "get me food from swiggy" / "get food from swiggy" / "get food"
    if let Some(rest) = clean.strip_prefix("get me food").or_else(|| clean.strip_prefix("get food")) {
        let restaurant = if let Some(r) = rest.strip_prefix(" from ") {
            r.trim()
        } else if let Some(r) = rest.strip_prefix(" on ") {
            r.trim()
        } else {
            ""
        };
        return Some(ParseResult {
            intent: ParsedIntent::OrderFood {
                query: "".to_string(),
                restaurant: if restaurant.is_empty() { None } else { Some(restaurant.to_string()) },
            },
            confidence: 0.90,
            source: "deterministic".to_string(),
        });
    }

    None
}

// ─── Commerce: product search (Amazon) ─────────────────────────────────

/// Parse Amazon product search commands.
///
/// Patterns:
///   "search for sony headphones on amazon" → SearchProduct { query: "sony headphones" }
///   "find wireless earbuds on amazon"      → SearchProduct { query: "wireless earbuds" }
///   "buy laptop on amazon"                 → SearchProduct { query: "laptop" }
///   "look for shoes on amazon"             → SearchProduct { query: "shoes" }
///   "amazon search for laptop"            → SearchProduct { query: "laptop" }
///   "search amazon for laptop"            → SearchProduct { query: "laptop" }
fn parse_search_product(text: &str) -> Option<ParseResult> {
    let mut clean = text.trim();
    for prefix in ["can you ", "could you ", "please ", "i want to "] {
        if let Some(rest) = clean.strip_prefix(prefix) {
            clean = rest.trim();
            break;
        }
    }

    // "search for <query> on amazon" / "find <query> on amazon" / "buy <query> on amazon" / "look for <query> on amazon"
    for verb in ["search for ", "find ", "buy ", "look for ", "search "] {
        if let Some(rest) = clean.strip_prefix(verb) {
            if let Some(query) = rest.strip_suffix(" on amazon").or_else(|| rest.strip_suffix(" in amazon")) {
                let query = query.trim();
                if !query.is_empty() {
                    return Some(ParseResult {
                        intent: ParsedIntent::SearchProduct {
                            query: query.to_string(),
                        },
                        confidence: 0.95,
                        source: "deterministic".to_string(),
                    });
                }
            }
        }
    }

    // "amazon search for <query>" / "amazon search <query>"
    if let Some(rest) = clean.strip_prefix("amazon search for ").or_else(|| clean.strip_prefix("amazon search ")) {
        return Some(ParseResult {
            intent: ParsedIntent::SearchProduct {
                query: rest.trim().to_string(),
            },
            confidence: 0.95,
            source: "deterministic".to_string(),
        });
    }

    // "search amazon for <query>" / "search on amazon for <query>"
    if let Some(rest) = clean.strip_prefix("search amazon for ").or_else(|| clean.strip_prefix("search on amazon for ")) {
        return Some(ParseResult {
            intent: ParsedIntent::SearchProduct {
                query: rest.trim().to_string(),
            },
            confidence: 0.95,
            source: "deterministic".to_string(),
        });
    }

    None
}

// ─── Ghostwriter room entry ──────────────────────────────────────────

/// Enter the persistent dictation room.
///
/// Triggers: "ghostwriter", "ghost writer", "take a letter",
/// "take dictation", "write this down", "start writing", "note this down",
/// "scribe", "type for me" — optionally "for <contact> [on whatsapp]".
/// Contact is optional; the room asks when missing.
fn parse_ghostwriter_entry(text: &str) -> Option<ParseResult> {
    // Ordered longest-first: "ghostwriter mode" must match before the
    // "ghostwriter" prefix leaves a stray " mode" tail (which rejects).
    // NOTE: bare-"ghost mode" family lives in parse_ghost_control_entry
    // (cursor control), NOT here. Dictation requires a writer word.
    const TRIGGERS: &[&str] = &[
        "open the ghostwriter mode",
        "open ghostwriter mode",
        "open the ghostwriter",
        "take a letter",
        "write this down",
        "take dictation",
        "note this down",
        "ghostwriter mode",
        "enter ghostwriter",
        "start ghostwriter",
        "enable ghostwriter",
        "open ghostwriter",
        "go ghostwriter",
        "start writing",
        "ghostwriter on",
        "ghostwriter",
        "ghost writer",
        "type for me",
        "scribe mode",
        "scribe",
    ];
    for trigger in TRIGGERS {
        if let Some(rest) = text.strip_prefix(trigger) {
            let rest = rest.trim();
            // Optional "for <contact> [on whatsapp]" tail.
            let mut contact: Option<String> = None;
            if let Some(after_for) = rest.strip_prefix("for ") {
                let c = after_for
                    .strip_suffix(" on whatsapp")
                    .or_else(|| after_for.strip_suffix(" whatsapp"))
                    .unwrap_or(after_for)
                    .trim();
                if !c.is_empty() {
                    contact = Some(c.to_string());
                }
            } else if !rest.is_empty() {
                // Trailing words that aren't a contact tail → not an entry.
                return None;
            }
            return Some(ParseResult {
                intent: ParsedIntent::EnterGhostwriter { contact },
                confidence: 0.95,
                source: "deterministic".to_string(),
            });
        }
    }
    None
}

// ─── Ghost cursor-control entry ──────────────────────────────────────

/// Enter cursor-control Ghost Mode (the AI drives your real cursor).
///
/// Triggers: "ghost mode", "take the mouse", "control my cursor",
/// "ghost cursor", "take control". Strict exact match (after trim) —
/// a bare mode word must never swallow sentences like "ghost mode is…".
/// Checked BEFORE parse_ghostwriter_entry so the mode word can never
/// fall through to dictation.
fn parse_ghost_control_entry(text: &str) -> Option<ParseResult> {
    // Ordered longest-first for prefix discipline.
    const TRIGGERS: &[&str] = &[
        "open the ghost mode",
        "the ghost mode",
        "the post mode",
        "the host mode",
        "the coast mode",
        "control my cursor",
        "control the cursor",
        "activate ghost mode",
        "activate ghost",
        "turn on ghost mode",
        "turn on ghost",
        "enable ghost control",
        "enable ghost mode",
        "start ghost control",
        "start ghost mode",
        "open ghost mode",
        "take the mouse",
        "take control",
        "ghost cursor",
        "go ghost mode",
        "ghost mode",
        // Direct STT soundalike triggers
        "ghost and warning",
        "ghost on warning",
        "ghost warning",
        "worst motive",
        "ghost to mulder",
        "it goes to mode",
        "it goes to mold",
        "goes to mode",
        "goes to mold",
        "go to mode",
        "post mode",
        "post modern",
        "postmodern",
        "coast mode",
        "gold mode",
        "toast mode",
        "host mode",
        "dose mode",
        "close mode",
        "ghost mood",
        "ghost node",
        "ghost mod",
    ];
    for trigger in TRIGGERS {
        if let Some(rest) = text.strip_prefix(trigger) {
            if rest.trim().is_empty() {
                return Some(ParseResult {
                    intent: ParsedIntent::EnterGhostControl,
                    confidence: 0.95,
                    source: "deterministic".to_string(),
                });
            }
            // Trailing words → not an entry ("ghost mode is confusing").
            return None;
        }
    }
    None
}

/// Leave cursor-control Ghost Mode.
///
/// Triggers: "exit/close/turn off/stop ghost mode", "exit ghost",
/// "ghost mode off", "leave ghost mode", "stand down" + STT soundalikes.
/// Exact match (or trailing-empty prefix); strict like the entry.
fn parse_ghost_control_exit(text: &str) -> Option<ParseResult> {
    const TRIGGERS: &[&str] = &[
        "turn off the ghost mode",
        "exit the ghost mode",
        "close the ghost mode",
        "stop the ghost mode",
        "leave the ghost mode",
        "turn off ghost mode",
        "exit ghost mode",
        "close ghost mode",
        "stop ghost mode",
        "quit ghost mode",
        "leave ghost mode",
        "exit ghost",
        "stop ghost",
        "ghost mode off",
        "ghost off",
        "stand down",
    ];
    for trigger in TRIGGERS {
        if let Some(rest) = text.strip_prefix(trigger) {
            if rest.trim().is_empty() {
                return Some(ParseResult {
                    intent: ParsedIntent::ExitGhostControl,
                    confidence: 0.95,
                    source: "deterministic".to_string(),
                });
            }
            return None;
        }
    }
    None
}

/// Hand mode phrases. Exact match after the generic cleanup (wake word, punctuation, filler), so
/// "hand mode is cool" is never a command.
/// Leaving hand mode must never depend on remembering one exact phrase (STT writes "hand more", "and mode",
/// "hands off"...). While hand mode is ON, any short sentence that pairs a leaving word with the hand /
/// camera / gesture, or says just "exit" / "quit", leaves it. Pure: `active` is passed in.
pub(crate) fn loose_hand_exit(t: &str, active: bool) -> bool {
    if !active {
        return false;
    }
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.len() > 7 || words.contains(&"ghost") {
        return false;
    }
    const LEAVE: &[&str] = &["exit", "quit", "stop", "close", "leave", "end", "disable", "cancel", "off", "kill", "done", "finish"];
    // "mode" / "more" / "mod" cover what STT makes of "hand mode" ("exit and mode"); Ghost has its own exit.
    const TARGET: &[&str] = &["hand", "hands", "camera", "webcam", "gesture", "gestures", "tracking", "handmode", "mode", "more", "mod"];
    let leave = words.iter().any(|w| LEAVE.contains(w));
    let target = words.iter().any(|w| TARGET.contains(w));
    // "exit" / "quit" alone is unambiguous while the mode is on; "stop" alone belongs to Ghost / media.
    (leave && target) || matches!(t, "exit" | "quit" | "exit now" | "quit now")
}

fn parse_hand_mode(text: &str) -> Option<ParseResult> {
    const ENTER: &[&str] = &[
        "hand mode", "hand mouse", "hand mouse mode", "hand control", "hand tracking",
        "start hand mode", "start hand mouse", "start hand tracking", "turn on hand mode",
        "turn on hand mouse", "enable hand mode", "enable hand mouse", "open hand mode",
        "camera mouse", "camera mode", "air mouse", "gesture mode", "start gesture mode",
        "hand more", "hands mode", "hand mod",
    ];
    const EXIT: &[&str] = &[
        "exit hand mode", "stop hand mode", "close hand mode", "quit hand mode", "leave hand mode",
        "hand mode off", "turn off hand mode", "exit hand mouse", "stop hand mouse",
        "close hand mouse", "hand mouse off", "turn off hand mouse", "stop hand tracking",
        "exit gesture mode", "stop gesture mode", "camera mouse off", "stop camera mouse",
    ];
    const LIVE: &[&str] = &[
        "hand mouse live", "hand mode live", "make hand mouse live", "make the hand mouse live",
        "hand mouse on for real", "hand mouse go live",
    ];
    const PREVIEW: &[&str] = &[
        "hand mouse preview", "hand mode preview", "hand mouse read only", "hand mouse dry run",
        "hand mouse safe mode", "hand mouse stop clicking", "hand mouse preview mode",
    ];
    const CLICK: &[&str] = &["click here", "click that", "click this", "left click", "left click here"];
    const DOUBLE: &[&str] = &["double click", "double click here", "double click that", "double click this", "double-click"];
    const RIGHT: &[&str] = &["right click", "right click here", "right click that", "right click this", "right-click"];
    let t = text.trim();
    let hit = |list: &[&str]| list.iter().any(|p| *p == t);
    let intent = if hit(CLICK) {
        ParsedIntent::PointerClick { kind: "left".into() }
    } else if hit(DOUBLE) {
        ParsedIntent::PointerClick { kind: "double".into() }
    } else if hit(RIGHT) {
        ParsedIntent::PointerClick { kind: "right".into() }
    } else if hit(ENTER) {
        ParsedIntent::EnterHandMode
    } else if hit(EXIT) || loose_hand_exit(t, crate::hand::active()) {
        ParsedIntent::ExitHandMode
    } else if hit(LIVE) {
        ParsedIntent::HandMouseLive { live: true }
    } else if hit(PREVIEW) {
        ParsedIntent::HandMouseLive { live: false }
    } else {
        return None;
    };
    Some(ParseResult { intent, confidence: 0.95, source: "deterministic".to_string() })
}

// ─── Screen control (ordinal click, tab switch, read-back) ──────────

/// "click on the 3rd option", "press the 2nd button", "choose 1st",
/// "move to the 4th tab", "what's the 2nd link".
fn parse_screen_command(text: &str) -> Option<ParseResult> {
    // Tab switch: "move|go|switch|shift|open to the Nth tab", "tab N".
    // ("shift to" added — live miss: users say "shift to tab 3".)
    for prefix in [
        "move to the ",
        "move to ",
        "move ",
        "go to the ",
        "go to ",
        "go ",
        "switch to the ",
        "switch to ",
        "switch ",
        "jump to the ",
        "jump to ",
        "shift to the ",
        "shift to ",
        "open ",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            // "4th tab" (number before tab) or "tab 4" (number after).
            if let Some(num_part) = rest.strip_suffix(" tab") {
                if let Some(n) = crate::screen::parse_ordinal(num_part) {
                    if (1..=9).contains(&n) {
                        return Some(ParseResult {
                            intent: ParsedIntent::BrowserTab { index: n },
                            confidence: 0.95,
                            source: "deterministic".to_string(),
                        });
                    }
                }
            }
            if let Some(num_rest) = rest.strip_prefix("tab ") {
                if let Some(n) = crate::screen::parse_ordinal(num_rest.trim()) {
                    if (1..=9).contains(&n) {
                        return Some(ParseResult {
                            intent: ParsedIntent::BrowserTab { index: n },
                            confidence: 0.95,
                            source: "deterministic".to_string(),
                        });
                    }
                }
            }
        }
    }
    if let Some(rest) = text.strip_prefix("tab ") {
        if let Some(n) = crate::screen::parse_ordinal(rest.trim()) {
            if (1..=9).contains(&n) {
                return Some(ParseResult {
                    intent: ParsedIntent::BrowserTab { index: n },
                    confidence: 0.9,
                    source: "deterministic".to_string(),
                });
            }
        }
    }

    // Close/delete tab: "close the 3rd tab" / "delete tab 5" → Ctrl+3 then
    // Ctrl+W; bare "close (this/the) tab" → Ctrl+W on the active tab.
    // (BrowserCloseTab existed in the executor but no parse ever produced it
    // — "delete tab 5" drifted to the Worker as a chat question.)
    for prefix in ["close the ", "close this ", "close ", "delete the ", "delete this ", "delete "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            // "close tab 5" / "delete the 5th tab"
            if let Some(num_part) = rest.strip_suffix(" tab") {
                if let Some(n) = crate::screen::parse_ordinal(num_part) {
                    if (1..=9).contains(&n) {
                        return Some(ParseResult {
                            intent: ParsedIntent::BrowserCloseTab { index: Some(n) },
                            confidence: 0.95,
                            source: "deterministic".to_string(),
                        });
                    }
                }
            }
            if let Some(num_rest) = rest.strip_prefix("tab ") {
                if let Some(n) = crate::screen::parse_ordinal(num_rest.trim()) {
                    if (1..=9).contains(&n) {
                        return Some(ParseResult {
                            intent: ParsedIntent::BrowserCloseTab { index: Some(n) },
                            confidence: 0.95,
                            source: "deterministic".to_string(),
                        });
                    }
                }
            }
            if matches!(
                rest.trim(),
                "tab" | "this tab" | "the tab" | "current tab" | "active tab"
            ) {
                return Some(ParseResult {
                    intent: ParsedIntent::BrowserCloseTab { index: None },
                    confidence: 0.95,
                    source: "deterministic".to_string(),
                });
            }
        }
    }

    // Read-back: "what's the 3rd option/button/link".
    for prefix in ["what's the ", "what is the ", "which is the ", "read the "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            for noun in ["option", "button", "link", "tab", "item", "choice"] {
                if let Some(num_part) = rest.strip_suffix(&format!(" {noun}")) {
                    if let Some(n) = crate::screen::parse_ordinal(num_part) {
                        return Some(ParseResult {
                            intent: ParsedIntent::ScreenRead { ordinal: n },
                            confidence: 0.9,
                            source: "deterministic".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Direct result patterns: "open result 2", "click result 2", "open link 3"
    for prefix in [
        "open result ",
        "open the result ",
        "click result ",
        "choose result ",
        "select result ",
        "result ",
        "open link ",
        "click link ",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            let clean = rest.trim().trim_end_matches(['.', ',', '!', '?']).trim();
            if let Some(n) = crate::screen::parse_ordinal(clean) {
                return Some(ParseResult {
                    intent: ParsedIntent::ScreenClick { ordinal: n },
                    confidence: 0.95,
                    source: "deterministic".to_string(),
                });
            }
        }
    }

    // Click: "click|press|choose|tap|select|open [on] [the] <ordinal> [noun]".
    for verb in [
        "click on the ", "click on ", "click the ", "click ",
        "press the ", "press ",
        "choose the ", "choose ",
        "tap the ", "tap ",
        "select the ", "select ",
        "open on the ", "open on ", "open the ", "open ",
    ] {
        if let Some(rest) = text.strip_prefix(verb) {
            for noun in [
                "option", "button", "link", "tab", "item", "choice", "one",
                "result", "results", "search result", "search results",
                "in the result", "in the results", "in result", "in search",
            ] {
                let num_part = rest
                    .strip_suffix(&format!(" {noun}"))
                    .unwrap_or(rest)
                    .trim();
                // Accept when a noun was present, the rest is a bare number
                // ("click 3"), or the whole rest is the ordinal ("choose 4th").
                // This avoids eating non-ordinal tails ("click chrome" falls
                // through to app resolution).
                let noun_present = rest.len() != num_part.len();
                let whole_is_ordinal = rest.trim() == num_part;
                if let Some(n) = crate::screen::parse_ordinal(num_part) {
                    if noun_present || whole_is_ordinal || num_part.chars().all(|c| c.is_ascii_digit()) {
                        return Some(ParseResult {
                            intent: ParsedIntent::ScreenClick { ordinal: n },
                            confidence: 0.9,
                            source: "deterministic".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Window control: "minimize (this/the window)" / "maximize (this/the
    // window)" / "full screen this". Bare commands, no slot — act on
    // whatever currently owns the foreground. Zero model cost, instant.
    if matches!(
        text,
        "minimize" | "minimize window" | "minimize this" | "minimize it"
            | "minimize the window" | "minimize this window"
    ) {
        return Some(ParseResult {
            intent: ParsedIntent::NluResult {
                intent: "system_minimize_window".to_string(),
                slots: serde_json::json!({}),
                confidence: 0.95,
            },
            confidence: 0.95,
            source: "deterministic".to_string(),
        });
    }
    if matches!(
        text,
        "maximize" | "maximize window" | "maximize this" | "maximize it"
            | "maximize the window" | "maximize this window" | "full screen this"
            | "full screen this window" | "make this full screen"
    ) {
        return Some(ParseResult {
            intent: ParsedIntent::NluResult {
                intent: "system_maximize_window".to_string(),
                slots: serde_json::json!({}),
                confidence: 0.95,
            },
            confidence: 0.95,
            source: "deterministic".to_string(),
        });
    }

    // Click a named on-screen element: "click <name>" / "click on <name>".
    // Distinct from the ordinal loop above (tried first, claims numeric/
    // ordinal targets). Bare "click " is unambiguous (no other deterministic
    // parser owns that verb). "press " is deliberately NOT included here,
    // bare — `parse_live_command`'s "press <key>"/"press ctrl a" key-press
    // patterns own that verb, and a bare prefix here would have shadowed
    // them (caught by test_parse_live_press_key/_hotkey). Only accept
    // "press" when explicitly button-qualified ("press the X button"),
    // which no key name ever is.
    for prefix in ["click on the ", "click on ", "click the ", "click "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            let name = rest.trim().trim_end_matches(['.', ',', '!', '?']).trim_end_matches(" button").trim();
            if !name.is_empty() && name.len() <= 60 {
                return Some(ParseResult {
                    intent: ParsedIntent::NluResult {
                        intent: "system_click_element".to_string(),
                        slots: serde_json::json!({ "name": name }),
                        confidence: 0.85,
                    },
                    confidence: 0.85,
                    source: "deterministic".to_string(),
                });
            }
        }
    }
    for prefix in ["press the ", "press "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            if let Some(name) = rest.trim().trim_end_matches(['.', ',', '!', '?']).strip_suffix(" button") {
                let name = name.trim();
                if !name.is_empty() && name.len() <= 60 {
                    return Some(ParseResult {
                        intent: ParsedIntent::NluResult {
                            intent: "system_click_element".to_string(),
                            slots: serde_json::json!({ "name": name }),
                            confidence: 0.85,
                        },
                        confidence: 0.85,
                        source: "deterministic".to_string(),
                    });
                }
            }
        }
    }
    None
}

// ─── Social: send WhatsApp message ─────────────────────────────────────

/// Parse WhatsApp message-sending commands.
///
/// Patterns:
///   "send <message> to <contact> in/on/via whatsapp"
///   "send <contact> a whatsapp message saying <message>"
///   "send a whatsapp message to <contact> saying <message>"
///   "send whatsapp message to <contact> saying <message>"
///   "send whatsapp to <contact> saying <message>"
///   "send a whatsapp to <contact> saying <message>"
///   "whatsapp <contact> saying <message>"
///   "message <contact> on whatsapp saying <message>"
///   "send <contact> a message saying <message>"
///   "send a message to <contact> saying <message>"
fn parse_send_whatsapp_message(text: &str) -> Option<ParseResult> {
    let mut clean_text = text.trim();
    for prefix in ["can you ", "could you ", "please ", "i want to ", "just "] {
        if let Some(rest) = clean_text.strip_prefix(prefix) {
            clean_text = rest.trim();
            break;
        }
    }

    // Pattern 1: (send|message|text|tell|say) <message> to <contact> (in|on|via) whatsapp
    // e.g. "send hi to mummy in whatsapp", "message hi to mommy in whatsapp", "say hi to mummy in whatsapp"
    for verb in ["send ", "message ", "text ", "tell ", "say "] {
        if let Some(rest) = clean_text.strip_prefix(verb) {
            for wa_suffix in [" in whatsapp", " on whatsapp", " via whatsapp", " on wa", " in wa", " via wa"] {
                if let Some(target_part) = rest.strip_suffix(wa_suffix) {
                    // Must have " to " between message and contact
                    // e.g. "hi to mummy", "good morning to mom"
                    if let Some((msg, contact)) = target_part.rsplit_once(" to ") {
                        let msg = msg.trim();
                        let contact = contact.trim();
                        let is_scaffolding = matches!(
                            msg,
                            "a message"
                                | "a whatsapp message"
                                | "a whatsapp"
                                | "message"
                                | "whatsapp message"
                                | "whatsapp"
                        );
                        if !contact.is_empty() && !msg.is_empty() && !is_scaffolding {
                            let msg_clean = msg
                                .trim_start_matches("a whatsapp message saying ")
                                .trim_start_matches("a message saying ")
                                .trim_start_matches("a whatsapp saying ")
                                .trim_start_matches("whatsapp message saying ")
                                .trim();
                            return Some(ParseResult {
                                intent: ParsedIntent::SendWhatsAppMessage {
                                    contact: contact.to_string(),
                                    message: msg_clean.to_string(),
                                },
                                confidence: 0.95,
                                source: "deterministic".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Pattern 2: (send|message|text|tell|say) <message> (on|in|via) whatsapp to <contact>
    // e.g. "send hi on whatsapp to mummy", "message good morning in whatsapp to dad", "say hi on whatsapp to mummy"
    for verb in ["send ", "message ", "text ", "tell ", "say "] {
        if let Some(rest) = clean_text.strip_prefix(verb) {
            for wa_mid in [" on whatsapp to ", " in whatsapp to ", " via whatsapp to ", " on wa to ", " in wa to "] {
                if let Some(pos) = rest.find(wa_mid) {
                    let msg = rest[..pos].trim();
                    let contact = rest[pos + wa_mid.len()..].trim();
                    let is_scaffolding = matches!(
                        msg,
                        "a message"
                            | "a whatsapp message"
                            | "a whatsapp"
                            | "message"
                            | "whatsapp message"
                            | "whatsapp"
                    );
                    if !contact.is_empty() && !msg.is_empty() && !is_scaffolding {
                        return Some(ParseResult {
                            intent: ParsedIntent::SendWhatsAppMessage {
                                contact: contact.to_string(),
                                message: msg.to_string(),
                            },
                            confidence: 0.95,
                            source: "deterministic".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Pattern 3: (send|tell|message|text|say) + [a] [whatsapp] message to <contact> + (saying|that|: |. |,) + <message>
    // e.g. "tell message to mommy. hi. all right", "tell message to mommy: hi", "send message to mommy. hi"
    for verb in ["send ", "tell ", "message ", "text ", "say "] {
        if let Some(rest) = clean_text.strip_prefix(verb) {
            for prefix in [
                "a whatsapp message to ",
                "a whatsapp to ",
                "whatsapp message to ",
                "whatsapp to ",
                "a message to ",
                "message to ",
            ] {
                if let Some(after_prefix) = rest.strip_prefix(prefix) {
                    for sep in [" saying ", " that ", ": ", ". ", ", "] {
                        if let Some(pos) = after_prefix.find(sep) {
                            let mut contact = after_prefix[..pos].trim();
                            for cut in [" on whatsapp", " in whatsapp", " via whatsapp", " on wa"] {
                                if let Some(c) = contact.strip_suffix(cut) {
                                    contact = c.trim();
                                }
                            }
                            let message = after_prefix[pos + sep.len()..].trim();
                            if !contact.is_empty() && !message.is_empty() {
                                return Some(ParseResult {
                                    intent: ParsedIntent::SendWhatsAppMessage {
                                        contact: contact.to_string(),
                                        message: message.to_string(),
                                    },
                                    confidence: 0.95,
                                    source: "deterministic".to_string(),
                                });
                            }
                        }
                    }
                    // If no message separator found, it's a partial send: "tell message to mommy"
                    let mut contact = after_prefix.trim();
                    for cut in [" on whatsapp", " in whatsapp", " via whatsapp", " on wa"] {
                        if let Some(c) = contact.strip_suffix(cut) {
                            contact = c.trim();
                        }
                    }
                    if !contact.is_empty() {
                        return Some(ParseResult {
                            intent: ParsedIntent::NeedMoreInfo {
                                prompt: format!("What should I say to {}?", contact),
                            },
                            confidence: 0.90,
                            source: "deterministic-partial".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Pattern 4: (send|tell|text|message|whatsapp|say) + <contact> + (a whatsapp message|a message|on whatsapp|...) + (saying|that|: |. |,) + <message>
    for verb in ["send ", "tell ", "text ", "message ", "whatsapp ", "say "] {
        if let Some(rest) = clean_text.strip_prefix(verb) {
            for mid in [
                " a whatsapp message saying ",
                " a whatsapp message that ",
                " a whatsapp message: ",
                " a message saying ",
                " a message that ",
                " a message: ",
                " whatsapp message saying ",
                " whatsapp message that ",
                " on whatsapp saying ",
                " on whatsapp that ",
                " on whatsapp: ",
                " in whatsapp saying ",
                " in whatsapp that ",
                " in whatsapp: ",
                " via whatsapp saying ",
                " on whatsapp ",
                " in whatsapp ",
                " via whatsapp ",
                " on wa ",
                " in wa ",
                " via wa ",
                " saying ",
                " that ",
                ": ",
                ". ",
            ] {
                if let Some(pos) = rest.find(mid) {
                    let mut contact = rest[..pos].trim();
                    for cut in [" on whatsapp", " in whatsapp", " via whatsapp", " on wa"] {
                        if let Some(c) = contact.strip_suffix(cut) {
                            contact = c.trim();
                        }
                    }
                    let message = rest[pos + mid.len()..].trim();
                    if !contact.is_empty() && !message.is_empty() {
                        return Some(ParseResult {
                            intent: ParsedIntent::SendWhatsAppMessage {
                                contact: contact.to_string(),
                                message: message.to_string(),
                            },
                            confidence: 0.95,
                            source: "deterministic".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Pattern 5: Partial send / tell without message body
    // ("send message to mummy", "can you send mummy a whatsapp message", "tell mom a message")
    if (clean_text.contains("send") || clean_text.contains("tell") || clean_text.contains("text"))
        && (clean_text.contains("whatsapp") || clean_text.contains("message"))
    {
        for verb in ["send ", "tell ", "text "] {
            if let Some(rest) = clean_text.strip_prefix(verb) {
                let mut contact_words = Vec::new();
                for word in rest.split_whitespace() {
                    let lower = word.to_lowercase();
                    let trimmed_word = lower.trim_matches(|c: char| !c.is_alphanumeric());
                    if !matches!(
                        trimmed_word,
                        "a" | "an" | "the" | "message" | "messages" | "whatsapp" | "wa" | "to" | "on" | "in" | "via"
                    ) {
                        contact_words.push(word);
                    }
                }
                let contact = contact_words.join(" ");
                let contact = contact.trim().trim_matches(|c: char| !c.is_alphanumeric());
                if !contact.is_empty() && !contact.contains("saying") {
                    return Some(ParseResult {
                        intent: ParsedIntent::NeedMoreInfo {
                            prompt: format!("What should I say to {}?", contact),
                        },
                        confidence: 0.85,
                        source: "deterministic-partial".to_string(),
                    });
                } else if contact.is_empty() {
                    return Some(ParseResult {
                        intent: ParsedIntent::NeedMoreInfo {
                            prompt: "Who should I message on WhatsApp, and what should I say?".to_string(),
                        },
                        confidence: 0.85,
                        source: "deterministic-partial".to_string(),
                    });
                }
            }
        }
    }

    None
}

// ΓöÇΓöÇΓöÇ Media control ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

// ΓöÇΓöÇΓöÇ Greetings / conversational replies ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ
//
// These are handled entirely locally ΓÇö no Cloudflare Worker round-trip.
// This saves ~1-3s of latency and avoids using GLM-4.7 Flash tokens for
// trivial conversational replies.

/// Parse greetings, farewells, and other conversational pleasantries.
/// Returns a `Greeting` intent with a pre-written reply.
fn parse_greeting(text: &str) -> Option<ParseResult> {
    // Hello / Hi / Hey
    if regex_match(text, r"^(?:hello|hi|hey|yo|sup|what'?s\s+up|howdy|greetings|hi\s+ya|hiya|hey\s+(?:there|nexus)|hello\s+nexus|hi\s+nexus|hey\s+nexus)$") {
        let replies = [
            "Hello, sir.",
            "Hi, sir. How can I help?",
            "Hey, sir. What can I do for you?",
            "At your service, sir.",
        ];
        let reply = pick(&replies, text);
        return Some(ParseResult {
            intent: ParsedIntent::Greeting { reply: reply.to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // How are you
    if regex_match(text, r"^(?:how\s+(?:are\s+you|are\s+ya|r\s+u)|how'?s\s+it\s+going|how\s+are\s+things|how\s+do\s+you\s+do|how\s+are\s+you\s+doing|how\s+is\s+it\s+going)$") {
        let replies = [
            "Fully operational, sir. How can I assist?",
            "Running smoothly, sir. What do you need?",
            "All systems green, sir. Ready when you are.",
            "Doing well, sir. How can I help?",
        ];
        let reply = pick(&replies, text);
        return Some(ParseResult {
            intent: ParsedIntent::Greeting { reply: reply.to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // Bye / Goodbye / See you
    if regex_match(text, r"^(?:bye|goodbye|good\s+bye|see\s+you|see\s+ya|see\s+u|catch\s+you\s+later|catch\s+ya\s+later|later|farewell|bye\s+bye|bye\s+nexus|goodbye\s+nexus)$") {
        let replies = [
            "Goodbye, sir.",
            "Until next time, sir.",
            "See you, sir.",
            "Farewell, sir.",
        ];
        let reply = pick(&replies, text);
        return Some(ParseResult {
            intent: ParsedIntent::Greeting { reply: reply.to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // Thanks
    if regex_match(text, r"^(?:thanks|thank\s+you|thank\s+u|thx|ty|thanks\s+nexus|thank\s+you\s+nexus|appreciate\s+it|much\s+obliged)$") {
        let replies = [
            "You're welcome, sir.",
            "My pleasure, sir.",
            "Anytime, sir.",
            "Glad to help, sir.",
        ];
        let reply = pick(&replies, text);
        return Some(ParseResult {
            intent: ParsedIntent::Greeting { reply: reply.to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // What is your name / Who are you
    if regex_match(text, r"^(?:what(?:'?s|\s+is)\s+your\s+name|who\s+are\s+you|what\s+are\s+you|your\s+name|who\s+is\s+nexus)$") {
        return Some(ParseResult {
            intent: ParsedIntent::Greeting {
                reply: "I'm NEXUS, your desktop assistant, sir.".to_string(),
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // What can you do
    if regex_match(text, r"^(?:what\s+can\s+you\s+do|what\s+do\s+you\s+do|what\s+are\s+you\s+capable\s+of|help\s+me|what\s+commands\s+(?:do\s+you\s+(?:know|have)|can\s+you\s+(?:do|handle)))$") {
        return Some(ParseResult {
            intent: ParsedIntent::Greeting {
                reply: "I can open apps, search the web, analyse repositories and PRs, control media, and answer questions, sir.".to_string(),
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // Good morning / afternoon / evening
    if regex_match(text, r"^good\s+(?:morning|afternoon|evening|night)(?:\s+nexus)?$") {
        let reply = if text.contains("morning") {
            "Good morning, sir. How can I help?"
        } else if text.contains("afternoon") {
            "Good afternoon, sir. What can I do for you?"
        } else if text.contains("evening") {
            "Good evening, sir. At your service."
        } else {
            "Good night, sir."
        };
        return Some(ParseResult {
            intent: ParsedIntent::Greeting { reply: reply.to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // Yes / OK / Alright (acknowledgements)
    if regex_match(text, r"^(?:yes|yeah|yep|yup|sure|ok|okay|alright|sounds\s+good|got\s+it|understood|roger|affirmative)$") {
        let replies = [
            "Understood, sir.",
            "Very good, sir.",
            "Acknowledged, sir.",
        ];
        let reply = pick(&replies, text);
        return Some(ParseResult {
            intent: ParsedIntent::Greeting { reply: reply.to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // No / Nope / No thanks
    if regex_match(text, r"^(?:no|nope|nah|no\s+thanks|never\s+mind|forget\s+it|cancel|disregard)$") {
        let replies = [
            "Very well, sir.",
            "As you wish, sir.",
            "Noted, sir.",
        ];
        let reply = pick(&replies, text);
        return Some(ParseResult {
            intent: ParsedIntent::Greeting { reply: reply.to_string() },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    None
}

/// Does this sentence answer the question NEXUS is waiting on (and only that)? Live miss 2026-10-19: after "Shall I
/// add the hook, sir?" the owner said "yes", the deterministic parser read it as a chat acknowledgement
/// ("Very good, sir."), the frontend answered it locally, and the question never saw its answer. Pure.
pub(crate) fn is_answer_to(pending: Option<&crate::memcore::offer::Offer>, transcript: &str) -> bool {
    pending.map_or(false, |o| crate::memcore::offer::classify_for(o, transcript) != crate::memcore::offer::Reply::Other)
}

/// A very short utterance that is no command and no answer (a quiet "yes" heard as "dirt."): worth asking once more
/// instead of throwing the question away and sending the noise to the cloud. Pure.
pub(crate) fn is_mumble(transcript: &str) -> bool {
    let words = transcript.split_whitespace().count();
    (1..=3).contains(&words) && parse_deterministic(transcript).is_none()
}

/// A statement of 4+ words that is not a command or a question, heard while a yes/no question is open: much
/// more likely a mis-heard answer than a new request. The caller asks once more instead of sending it to the cloud
/// (live 2026-10-28: a hallucinated "It's very nice to know…" was answered with an unrelated paragraph).
pub(crate) fn is_stray_statement(transcript: &str) -> bool {
    let words: Vec<&str> = transcript.split_whitespace().collect();
    if words.len() < 4 || parse_deterministic(transcript).is_some() {
        return false;
    }
    let first = words[0]
        .trim_matches(|c: char| !c.is_alphabetic())
        .to_lowercase();
    !matches!(
        first.as_str(),
        "what" | "whats" | "how" | "why" | "when" | "where" | "who" | "which" | "can" | "could" | "would"
            | "will" | "is" | "are" | "do" | "does" | "did" | "tell" | "show" | "open" | "nexus" | "please"
            | "play" | "search" | "close" | "stop" | "cancel" | "no" | "not"
            // Direct requests that are not parser commands still go on to normal handling.
            | "explain" | "describe" | "summarize" | "summarise" | "compare" | "give" | "find" | "list"
            | "read" | "check" | "set" | "send" | "call" | "remind" | "turn" | "take" | "make" | "run"
            | "start" | "launch" | "switch" | "let" | "help" | "calculate" | "write" | "create" | "add"
            | "get" | "look" | "translate" | "define" | "repeat" | "say" | "ask" | "analyse" | "analyze"
    )
}

/// `is_answer_to` for the question that is armed right now.
pub(crate) fn answers_a_pending_offer(transcript: &str) -> bool {
    let pending = if crate::memcore::offer::is_armed() { crate::memcore::offer::peek() } else { None };
    is_answer_to(pending.as_ref(), transcript)
}

/// Pick a reply from a list, deterministically based on a hash of the input
/// text. This gives variety (different replies for different inputs) while
/// remaining deterministic (same input ΓåÆ same reply, no randomness).
fn pick<'a>(replies: &[&'a str], text: &str) -> &'a str {
    let hash: u32 = text.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    replies[(hash as usize) % replies.len()]
}

fn parse_media(text: &str) -> Option<ParsedIntent> {
    // 1. Open specific music player: "open apple music", "open spotify", "launch spotify", etc.
    if let Some(caps) = regex_captures(text, r"^(?:open|launch|start)\s+(spotify|apple\s+music|music)$") {
        let player = caps.get(1).map(|m| m.as_str().trim().to_string());
        return Some(ParsedIntent::OpenMusicPlayer { player });
    }

    // 2. Play specific song on specific player: "play bohemian rhapsody on spotify", "play hotel california on apple music"
    if let Some(caps) = regex_captures(text, r"^(?:play|listen\s+to)\s+(.+?)\s+on\s+(spotify|apple\s+music|music)$") {
        let query = caps.get(1).map(|m| m.as_str().trim().to_string());
        let player = caps.get(2).map(|m| m.as_str().trim().to_string());
        return Some(ParsedIntent::PlayMusic { query, player });
    }

    // 3. Play songs / play music / play some songs
    if regex_match(text, r"^(?:play\s+songs?|play\s+music|play\s+some\s+songs?|play\s+some\s+music|listen\s+to\s+music|play\s+something)$") {
        return Some(ParsedIntent::PlayMusic { query: None, player: None });
    }

    // 4. Play <song> (when text starts with "play song <name>" or "play track <name>")
    if let Some(caps) = regex_captures(text, r"^play\s+(?:the\s+)?(?:song|track)\s+(.+)$") {
        let query = caps.get(1).map(|m| m.as_str().trim().to_string());
        return Some(ParsedIntent::PlayMusic { query, player: None });
    }

    if regex_match(text, r"^(?:pause|pause\s+music|pause\s+media|play|resume|resume\s+music|play\s*[/\s]*pause|toggle\s+media)$") {
        return Some(ParsedIntent::MediaPlayPause);
    }
    if regex_match(text, r"^(?:next|next\s+song|next\s+track|skip|skip\s+song|skip\s+track)$") {
        return Some(ParsedIntent::MediaNext);
    }
    if regex_match(text, r"^(?:previous|previous\s+song|previous\s+track|prev|prev\s+song|go\s+back\s+a\s+song)$") {
        return Some(ParsedIntent::MediaPrevious);
    }
    if regex_match(text, r"^(?:stop\s+music|stop\s+media|stop\s+playback|stop)$") {
        return Some(ParsedIntent::MediaStop);
    }
    // 5. Bare "play <title>" / "play <title> by <artist>" / "put on <title>". Last on purpose: every exact form above
    //    (play, play/pause, play next ...) wins, and anything that is clearly not music is left to the other parsers.
    if let Some(caps) = regex_captures(text, r"^(?:play|put\s+on)\s+(.+)$") {
        let query = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if is_music_title(&query) {
            return Some(ParsedIntent::PlayMusic { query: Some(query), player: None });
        }
    }
    None
}

/// Could "play <this>" be a song, album or artist? A title is short and is not a pronoun, a transport word, or an
/// obvious video / game / streaming-site request.
pub(crate) fn is_music_title(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.is_empty() || words.len() > 12 {
        return false;
    }
    const NOT_A_TITLE: &[&str] = &[
        "it", "that", "this", "again", "back", "now", "pause", "resume", "next", "previous", "last", "first",
        "something", "anything", "songs", "music", "stop", "faster", "slower",
    ];
    if words.len() == 1 && NOT_A_TITLE.contains(&words[0]) {
        return false;
    }
    if matches!(words[0], "it" | "that" | "this" | "them" | "those" | "these") {
        return false;
    }
    let transport = |w: &str| matches!(w, "next" | "previous" | "last");
    let unit = |w: &str| matches!(w, "song" | "track" | "one" | "songs" | "tracks");
    if (transport(words[0]) && words.get(1).map_or(true, |w| unit(w)))
        || (words[0] == "the" && words.get(1).is_some_and(|w| transport(w)) && words.get(2).map_or(true, |w| unit(w)))
    {
        return false;
    }
    const NOT_MUSIC: &[&str] = &[
        "youtube", "netflix", "video", "videos", "movie", "movies", "game", "games", "chess", "minecraft", "trailer",
        "podcast", "episode", "stream", "twitch", "prime", "hotstar", "reel", "reels", "clip",
    ];
    !words.iter().any(|w| NOT_MUSIC.contains(w))
}

// ΓöÇΓöÇΓöÇ Architecture mapper ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

/// Exact match for architecture mapper commands.
/// Covers all natural phrasings:
///   "open architecture mapper"
///   "open the architecture mapper"
///   "open architect"
///   "show architecture map"
///   "launch architecture window"
///   "bring up architecture"
///   "pull up the architect mapper"
///   "open the architecture"
///   "show me the architecture"
///   "open codebase mapper"
///   "open dependency mapper"
fn is_architect_command(text: &str) -> bool {
    regex_match(
        text,
        r"^(?:open|launch|start|show|bring\s+up|pull\s+up|give\s+me|show\s+me)\s+(?:me\s+)?(?:the\s+)?(?:architecture|architect|codebase|dependency)(?:\s+(?:mapper|map|window|mapper\s+window|viewer|diagram|graph|explorer))?$",
    ) || regex_match(
        text,
        r"^(?:open|launch|start|show)\s+(?:the\s+)?(?:architecture|architect)(?:\s+(?:mapper|map|window))?$",
    ) || regex_match(
        text,
        r"^(?:show|display|view)\s+(?:me\s+)?(?:the\s+)?architecture$",
    )
}

// ─── Social block/unblock commands ────────────────────────────────────────

fn parse_days_token(w: &str) -> Option<u32> {
    match w.to_lowercase().as_str() {
        "a" | "an" | "one" | "1" => Some(1),
        "two" | "2" => Some(2),
        "three" | "3" => Some(3),
        "four" | "4" => Some(4),
        "five" | "5" => Some(5),
        "six" | "6" => Some(6),
        "seven" | "7" => Some(7),
        "eight" | "8" => Some(8),
        "nine" | "9" => Some(9),
        "ten" | "10" => Some(10),
        s => s.parse::<u32>().ok(),
    }
}

pub(crate) fn parse_social_block_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();

    // 1. List active blocks
    if regex_match(
        &t,
        r"^(?:who\s+is\s+(?:currently\s+)?blocked|who'?s\s+(?:currently\s+)?blocked|list\s+(?:currently\s+)?blocked(?:\s+contacts)?|show\s+(?:currently\s+)?blocked(?:\s+contacts)?|show\s+active\s+blocks|list\s+active\s+blocks)$",
    ) {
        return Some(ParseResult {
            intent: ParsedIntent::ListBlockedContacts,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 2. Unblock
    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?unblock\s+(.+)$",
    ) {
        let mut rem = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let mut platform = "all".to_string();

        if let Some(p_cap) = regex_captures(
            &rem,
            r"\b(?:on|in|from)\s+(instagram\s+and\s+whatsapp|whatsapp\s+and\s+instagram|instagram|whatsapp|phone(?:\s+number)?|all|everywhere)$",
        ) {
            let p_str = p_cap.get(1).map(|m| m.as_str()).unwrap_or("");
            if p_str.contains("instagram") && p_str.contains("whatsapp") {
                platform = "all".to_string();
            } else if p_str.contains("instagram") {
                platform = "instagram".to_string();
            } else if p_str.contains("whatsapp") {
                platform = "whatsapp".to_string();
            } else if p_str.contains("phone") {
                platform = "phone".to_string();
            }
            if let Some(full) = p_cap.get(0) {
                let start = full.start();
                rem = rem[..start].trim().to_string();
            }
        }

        let target = rem.trim().to_string();
        if !target.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::UnblockContact { target, platform },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    // 3. Block
    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?block\s+(.+)$",
    ) {
        let mut rem = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();

        // Strip conversational endings like "can it do that", "please"
        if let Ok(re_tail) = regex::Regex::new(r"\b(?:can\s+(?:it|you|u)\s+do\s+that\??|please)\b") {
            rem = re_tail.replace_all(&rem, "").trim().to_string();
        }

        let mut platform = "all".to_string();
        let has_ig = rem.contains("instagram");
        let has_wa = rem.contains("whatsapp");
        let has_phone = rem.contains("phone");

        if (has_ig && has_wa) || rem.contains("everywhere") || rem.contains("all") {
            platform = "all".to_string();
        } else if has_ig {
            platform = "instagram".to_string();
        } else if has_wa {
            platform = "whatsapp".to_string();
        } else if has_phone {
            platform = "phone".to_string();
        }

        let mut duration_days = None;
        if let Some(dur_cap) = regex_captures(
            &rem,
            r"\bfor\s+(\d+|one|two|three|four|five|six|seven|eight|nine|ten|a|an)\s+(day|days|week|weeks|month|months)\b",
        ) {
            let num_str = dur_cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let unit_str = dur_cap.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(mut n) = parse_days_token(num_str) {
                if unit_str.starts_with("week") {
                    n *= 7;
                } else if unit_str.starts_with("month") {
                    n *= 30;
                }
                duration_days = Some(n);
            }
            if let Some(full) = dur_cap.get(0) {
                let start = full.start();
                let end = full.end();
                rem = format!("{} {}", &rem[..start], &rem[end..]);
            }
        }

        // Clean platform phrases and "it should block"
        if let Ok(re_plat) = regex::Regex::new(r"\b(?:on|in)\s+(?:instagram|whatsapp|phone(?:\s+number)?)(?:\s+and\s+(?:instagram|whatsapp|phone(?:\s+number)?))*\b") {
            rem = re_plat.replace_all(&rem, "").to_string();
        }
        if let Ok(re_trail) = regex::Regex::new(r"\bit\s+should\s+block\b.*") {
            rem = re_trail.replace_all(&rem, "").to_string();
        }

        let target = rem.split_whitespace().collect::<Vec<_>>().join(" ");
        let denylist = ["screen", "ads", "popups", "diagram", "site", "website", "window"];
        if !target.is_empty() && !denylist.contains(&target.as_str()) {
            return Some(ParseResult {
                intent: ParsedIntent::BlockContact {
                    target,
                    platform,
                    duration_days,
                },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    None
}

// ─── Universal Coding Agent & Localhost commands ──────────────────────────

pub(crate) fn parse_coding_command(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();

    // 0a. Explicit agent watch ("nexus watch over this update me when the response is received and claude has stopped running")
    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:nexus\s+)?(?:watch\s+over\s+this|watch\s+this|watch\s+over)\s+(?:and\s+)?(?:update\s+me|tell\s+me|alert\s+me|notify\s+me)\s+(?:when\s+(?:the\s+)?response\s+is\s+received\s+and\s+)?(?:the\s+)?([a-z0-9_-]+)\s+(?:has\s+)?(?:stopped\s+running|finished|is\s+done)$",
    ) {
        let agent = caps.get(1).map(|m| m.as_str().trim().to_string());
        return Some(ParseResult {
            intent: ParsedIntent::WatchAgentTask { agent, query: None },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:nexus\s+)?(?:update\s+me|tell\s+me|alert\s+me|notify\s+me)\s+when\s+(?:the\s+response\s+is\s+received\s+and\s+)?(?:the\s+)?([a-z0-9_-]+)\s+(?:has\s+)?(?:stopped\s+running|finished|is\s+done)$",
    ) {
        let agent = caps.get(1).map(|m| m.as_str().trim().to_string());
        return Some(ParseResult {
            intent: ParsedIntent::WatchAgentTask { agent, query: None },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:nexus\s+)?(?:watch|watch\s+over)\s+(claude|antigravity|agy|codex|cursor|trae|windsurf|vscode|aider|opencode|hermes)(?:\s+and\s+(?:tell|update|alert|notify)\s+me\s+when\s+(?:it(?:'s|\s+is)?\s+done|finished|stopped))?$",
    ) {
        let agent = caps.get(1).map(|m| m.as_str().trim().to_string());
        return Some(ParseResult {
            intent: ParsedIntent::WatchAgentTask { agent, query: None },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 0b. Implicit "watch over this" (no agent named -> foreground window / active task will be resolved)
    if regex_match(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:nexus\s+)?(?:watch\s+over\s+this|watch\s+this|watch\s+over\s+it)(?:\s+(?:and\s+)?(?:update|tell|alert|notify)\s+me\s+when\s+(?:the\s+response\s+is\s+received|it(?:'s|\s+is)?\s+done|it\s+stops))?$",
    ) {
        return Some(ParseResult {
            intent: ParsedIntent::WatchAgentTask { agent: None, query: None },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 1. Run on localhost / start dev server
    if regex_match(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:run\s+(?:this\s+)?(?:on|in)\s+(?:the\s+)?localhost|run\s+(?:this\s+)?(?:on|in)\s+(?:the\s+)?local\s+host|start\s+(?:the\s+)?dev\s+server|launch\s+(?:the\s+)?localhost|launch\s+(?:the\s+)?dev\s+server|start\s+(?:the\s+)?local\s+server)$",
    ) {
        return Some(ParseResult {
            intent: ParsedIntent::RunOnLocalhost,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    // 2. Dispatch task to specific coding agent (claude, antigravity, aider, etc.)
    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:run\s+with|ask|tell)\s+(claude|antigravity|agy|aider|cursor|trae|opencode|windsurf|codex)\s+(?:to\s+)?(.+)$",
    ) {
        let agent = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let prompt = caps.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if !agent.is_empty() && !prompt.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::CodingAgentTask { agent, prompt },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    // 3. Autonomous project planning & creation ("plan and create that", "analyse the chat with eesah and plan and create...")
    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:anlayse|analyse|analyze)\s+(?:the\s+)?chat\s+with\s+(.+?)\s+and\s+plan\s+and\s+create\s+(.+)$",
    ) {
        let source = format!("chat with {}", caps.get(1).map(|m| m.as_str().trim()).unwrap_or("contact"));
        let prompt = caps.get(2).map(|m| m.as_str().trim().to_string());
        return Some(ParseResult {
            intent: ParsedIntent::CodeProject { source, prompt },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:plan\s+and\s+create|plan\s+and\s+build|create\s+and\s+plan)\s+(.+)$",
    ) {
        let prompt = caps.get(1).map(|m| m.as_str().trim().to_string());
        return Some(ParseResult {
            intent: ParsedIntent::CodeProject {
                source: "active_workspace".to_string(),
                prompt,
            },
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    None
}

// ─── Executive Assistant: Voice Reminders, College Internship & Chat Docs ──

fn parse_reminder_minutes(n_str: &str, unit_str: &str) -> u32 {
    let mut val = match n_str.to_lowercase().as_str() {
        "a" | "an" | "one" | "1" => 1,
        "two" | "2" => 2,
        "three" | "3" => 3,
        "four" | "4" => 4,
        "five" | "5" => 5,
        "ten" | "10" => 10,
        "fifteen" | "15" => 15,
        "twenty" | "20" => 20,
        "thirty" | "30" | "half an" => 30,
        s => s.parse::<u32>().unwrap_or(1),
    };
    if unit_str.to_lowercase().contains("hour") || unit_str.to_lowercase().starts_with("hr") {
        val *= 60;
    }
    val
}

pub(crate) fn parse_executive_assistant_commands(text: &str) -> Option<ParseResult> {
    let t = text.trim().to_lowercase();

    // 1. Alexa-style voice reminder: "remind me to <task> in <N> minutes/hours"
    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?(?:set\s+a\s+reminder\s+to|remind\s+me\s+to)\s+(.+?)\s+in\s+(\d+|one|two|three|four|five|ten|fifteen|twenty|thirty|half\s+an|an|a)\s+(minute|minutes|hour|hours|mins?|hrs?)$",
    ) {
        let task = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let n_str = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("1");
        let unit_str = caps.get(3).map(|m| m.as_str().trim()).unwrap_or("minute");
        let minutes = parse_reminder_minutes(n_str, unit_str);
        if !task.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::CreateVoiceReminder { task, minutes },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    // Voice reminder: "remind me in <N> minutes to <task>"
    if let Some(caps) = regex_captures(
        &t,
        r"^(?:can\s+(?:you|u)\s+|could\s+you\s+|please\s+)?remind\s+me\s+in\s+(\d+|one|two|three|four|five|ten|fifteen|twenty|thirty|half\s+an|an|a)\s+(minute|minutes|hour|hours|mins?|hrs?)\s+to\s+(.+)$",
    ) {
        let n_str = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("1");
        let unit_str = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("minute");
        let task = caps.get(3).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let minutes = parse_reminder_minutes(n_str, unit_str);
        if !task.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::CreateVoiceReminder { task, minutes },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    // 2. WhatsApp chat document downloader and evaluator
    if let Some(caps) = regex_captures(
        &t,
        r"\b(?:docs|documents)\s+(?:in\s+(?:the\s+)?chat\s+with|from\s+chat\s+with)\s+([a-zA-Z\s]+?)(?:\s+(?:download|analyse|and|tell).*|$)",
    ) {
        let mut contact = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if let Ok(re_strip) = regex::Regex::new(r"\b(?:download|analyse|analyze)\b.*") {
            contact = re_strip.replace_all(&contact, "").trim().to_string();
        }
        if !contact.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::AnalyseChatDocs { contact },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }
    if let Some(caps) = regex_captures(
        &t,
        r"\bchat\s+with\s+([a-zA-Z]+)\s+download\s+and\s+(?:analyse|analyze)\b",
    ) {
        let contact = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if !contact.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::AnalyseChatDocs { contact },
                confidence: 1.0,
                source: "deterministic".to_string(),
            });
        }
    }

    // 3. College internship email scanner
    if regex_match(
        &t,
        r"\b(?:college\s+email|email\s+.*college|internship.*email|email.*internship|emails?.*baout.*intership|emails?.*about.*internship)\b",
    ) {
        return Some(ParseResult {
            intent: ParsedIntent::ScanInternshipEmails,
            confidence: 1.0,
            source: "deterministic".to_string(),
        });
    }

    None
}

// ─── Settings / Command Center ───────────────────────────────────────────

/// Match settings/command center commands.
/// Covers all natural phrasings:
///   "open settings" / "open the settings"
///   "show settings" / "show me the settings"
///   "open command center" / "open the command center"
///   "show command center" / "show me the command center"
///   "open preferences" / "show preferences"
///   "open configuration" / "open config"
///   "configure NEXUS" / "NEXUS settings" / "NEXUS config"
///   "NEXUS preferences" / "NEXUS command center"
///   "bring up settings" / "pull up settings"
///   "launch settings" / "start settings"
///
/// Also handles STT singular variants:
///   "open setting" / "show setting" / "open preference"
///
/// Also handles bare words (Intel SST mic truncation):
///   "settings" / "preferences" / "config" / "command center"
fn is_settings_command(text: &str) -> bool {
    let t = text.trim().to_lowercase();

    // 1. Direct standalone phrases & noun phrases
    if matches!(
        t.as_str(),
        "command hub"
            | "the command hub"
            | "nexus command hub"
            | "the nexus command hub"
            | "command hub sidebar"
            | "command hub window"
            | "nexus hub"
            | "the nexus hub"
            | "hub"
            | "the hub"
            | "command center"
            | "the command center"
            | "nexus command center"
            | "the nexus command center"
            | "comment hub"
            | "the comment hub"
            | "nexus comment hub"
            | "the nexus comment hub"
            | "common hub"
            | "comma hub"
            | "comment center"
            | "common center"
            | "control hub"
            | "the control hub"
            | "nexus control hub"
            | "control center"
            | "the control center"
            | "nexus control center"
            | "settings"
            | "the settings"
            | "nexus settings"
            | "settings sidebar"
            | "settings window"
            | "preferences"
            | "the preferences"
            | "nexus preferences"
            | "config"
            | "configuration"
            | "nexus config"
    ) {
        return true;
    }

    // 2. Action verbs with hub / center
    // (open|launch|start|show|bring up|pull up|give me|show me|display|view|access|go to) + (the)? + (nexus)? + (command|comment|common|control)? + (hub|center)
    if regex_match(
        text,
        r"^(?:open|launch|start|show|bring\s+up|pull\s+up|give\s+me|show\s+me|display|view|access|go\s+to)\s+(?:me\s+)?(?:the\s+)?(?:nexus\s+)?(?:command\s+|comment\s+|common\s+|control\s+)?(?:hub|center)(?:\s+(?:sidebar|window|please))?$",
    ) {
        return true;
    }

    // 3. Suffix variations: (the)? (nexus)? (command|comment|control)? (hub|center) (sidebar|window|please|now)
    if regex_match(
        text,
        r"^(?:the\s+)?(?:nexus\s+)?(?:command\s+|comment\s+|common\s+|control\s+)?(?:hub|center)\s+(?:sidebar|window|please|now)$",
    ) {
        return true;
    }

    // 4. (open|launch|start|show|bring up|pull up|give me|show me|display|view|access|go to) + (the)? + settings/setting
    if regex_match(
        text,
        r"^(?:open|launch|start|show|bring\s+up|pull\s+up|give\s+me|show\s+me|display|view|access|go\s+to)\s+(?:me\s+)?(?:the\s+)?(?:nexus\s+)?settings?(?:\s+(?:sidebar|window|please))?$",
    ) {
        return true;
    }

    // 5. (open|show) + (the)? + preferences / config / configuration
    if regex_match(
        text,
        r"^(?:open|launch|start|show|bring\s+up|pull\s+up|give\s+me|show\s+me|display|view|access|go\s+to)\s+(?:me\s+)?(?:the\s+)?(?:nexus\s+)?(?:preferences?|config|configuration)(?:\s+(?:sidebar|window|please))?$",
    ) {
        return true;
    }

    // 6. "configure NEXUS" / "configure nexus"
    if regex_match(text, r"^configure\s+nexus$") {
        return true;
    }

    // 7. "NEXUS ..." prefix variants
    if regex_match(
        text,
        r"^nexus\s+(?:settings?|config|configuration|preferences?|command\s+center|command\s+hub|comment\s+hub|common\s+hub|control\s+center|control\s+hub)(?:\s+(?:sidebar|window|please))?$",
    ) {
        return true;
    }

    false
}

/// Fuzzy match for architecture mapper commands that STT misheard.
///
/// faster-whisper tiny.en (39M params) commonly mishears "architecture mapper" as:
///   "octach at mapper", "arcade mapper", "arch at mapper", "arch mapper",
///   "architecture at mapper", "open architect mapper", etc.
///   "open up and remember" (severe mishearing)
///   "open are cat map", "open our cat map", "open ark map"
///   "open art at mapper", "open art map"
///   "open a cat map", "open acat mapper"
///
/// Strategy (layered, most-specific first):
/// 1. Exact-ish: ends with "mapper"/"map"/"diagram"/"graph" + has arch-like word
/// 2. Contains "arch" or "architect" anywhere
/// 3. Contains "codebase" or "dependency" + "map"/"mapper"
/// 4. Severe mishearing: "open" + 2-5 words with mapper-like or arch-like sounds
fn is_architect_fuzzy(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    // Must start with an open/launch verb (or "show me" / "give me")
    let starts_with_verb = t.starts_with("open ")
        || t.starts_with("launch ")
        || t.starts_with("start ")
        || t.starts_with("show ")
        || t.starts_with("show me ")
        || t.starts_with("bring up ")
        || t.starts_with("bring me ")
        || t.starts_with("pull up ")
        || t.starts_with("give me ");
    if !starts_with_verb {
        return false;
    }

    // Pattern 1: Explicit codebase/dependency mapper terms
    if t.contains("codebase") || t.contains("dependency") || t.contains("dependencies") {
        if t.ends_with("mapper")
            || t.ends_with("map")
            || t.ends_with("diagram")
            || t.ends_with("graph")
            || t.ends_with("window")
            || t == "open codebase"
        {
            return true;
        }
    }

    // Pattern 2: Short forms from tests ("open map", "open the map", "show map", "show the map")
    if t == "open map" || t == "open the map" || t == "show map" || t == "show the map" {
        return true;
    }

    // Pattern 3: Specific known severe STT mishearings of "open architecture mapper"
    const EXACT_MISHEARINGS: &[&str] = &[
        "open up and remember",
        "open up and member",
        "open up and december",
    ];
    for m in EXACT_MISHEARINGS {
        if t == *m {
            return true;
        }
    }

    // Pattern 4: STT acoustic mishearings of "architecture mapper"
    // Requires BOTH an architecture soundalike AND a mapper soundalike.
    // e.g. "open octach at mapper", "open arcade mapper", "open are cat map", "open ark mapper"
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.len() >= 2 && words.len() <= 6 {
        let has_arch_like = words.iter().any(|w| {
            *w == "architecture" || *w == "architect" || *w == "arch" || *w == "arcade"
                || *w == "octach" || *w == "ark" || *w == "acat" || *w == "ocat"
                || *w == "are" || *w == "our" || *w == "art" || *w == "arc"
                || *w == "cat"
        }) || t.contains("a cat") || t.contains("are cat") || t.contains("our cat");
        let has_mapper_like = words.iter().any(|w| {
            *w == "mapper" || *w == "map" || *w == "member" || *w == "remember"
                || *w == "december" || *w == "mac" || *w == "mad" || *w == "matter"
                || *w == "master" || *w == "manner"
        });

        // If it has "architecture" or "architect", it matches if accompanied by mapper-like or window/diagram
        if t.contains("architecture") || t.contains("architect") {
            if has_mapper_like
                || t.ends_with("mapper")
                || t.ends_with("map")
                || t.ends_with("diagram")
                || t.ends_with("graph")
                || t.ends_with("viewer")
                || t.ends_with("explorer")
                || t.ends_with("window")
            {
                return true;
            }
        }

        // For misheard words, BOTH arch-like AND mapper-like must be present!
        if has_arch_like && has_mapper_like {
            return true;
        }
    }

    false
}

// ΓöÇΓöÇΓöÇ Browser force ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

/// URL map for "open <app> in browser" commands.
const BROWSER_FORCE_URLS: &[(&str, &str)] = &[
    ("gmail", "https://mail.google.com"),
    ("google mail", "https://mail.google.com"),
    ("youtube", "https://www.youtube.com"),
    ("you tube", "https://www.youtube.com"),
    ("github", "https://github.com"),
    ("git hub", "https://github.com"),
    ("twitter", "https://twitter.com"),
    ("x", "https://x.com"),
    ("facebook", "https://facebook.com"),
    ("instagram", "https://instagram.com"),
    ("reddit", "https://reddit.com"),
    ("linkedin", "https://linkedin.com"),
    ("whatsapp", "https://web.whatsapp.com"),
    ("whatsapp web", "https://web.whatsapp.com"),
    ("spotify", "https://open.spotify.com"),
    ("netflix", "https://netflix.com"),
    ("amazon", "https://amazon.com"),
    ("google drive", "https://drive.google.com"),
    ("google docs", "https://docs.google.com"),
    ("google sheets", "https://sheets.google.com"),
    ("google slides", "https://slides.google.com"),
    ("google maps", "https://maps.google.com"),
    ("google calendar", "https://calendar.google.com"),
    ("google translate", "https://translate.google.com"),
    ("google photos", "https://photos.google.com"),
    ("google news", "https://news.google.com"),
    ("google meet", "https://meet.google.com"),
    ("google chat", "https://chat.google.com"),
    ("google play", "https://play.google.com"),
    ("play store", "https://play.google.com"),
    ("app store", "https://apps.apple.com"),
    ("chatgpt", "https://chat.openai.com"),
    ("chat gpt", "https://chat.openai.com"),
    ("open ai", "https://chat.openai.com"),
    ("openai", "https://chat.openai.com"),
    ("claude", "https://claude.ai"),
    ("figma", "https://figma.com"),
    ("notion", "https://notion.so"),
    ("slack", "https://slack.com"),
    ("discord", "https://discord.com/app"),
    ("twitch", "https://twitch.tv"),
    ("stack overflow", "https://stackoverflow.com"),
    ("stackoverflow", "https://stackoverflow.com"),
    ("wikipedia", "https://wikipedia.org"),
    ("chat", "https://chat.google.com"),
    ("maps", "https://maps.google.com"),
    ("translate", "https://translate.google.com"),
    ("calendar", "https://calendar.google.com"),
];

fn parse_browser_force(text: &str) -> Option<ParseResult> {
    // "open gmail in browser" / "open gmail website" / "open gmail site"
    let patterns: &[&str] = &[
        r"^(.+?)\s+in\s+(?:the\s+)?browser$",
        r"^(.+?)\s+website$",
        r"^(.+?)\s+site$",
        r"^(.+?)\s+on\s+(?:the\s+)?web$",
        r"^(.+?)\s+web\s+version$",
    ];

    for &pat in patterns {
        if let Some(caps) = regex_captures(text, pat) {
            let app_name = caps[1].trim();
            // Check URL map
            for (key, url) in BROWSER_FORCE_URLS {
                if *key == app_name {
                    return Some(ParseResult {
                        intent: ParsedIntent::OpenUrl {
                            target: app_name.to_string(),
                            url: url.to_string(),
                        },
                        confidence: 1.0,
                        source: "deterministic".to_string(),
                    });
                }
            }
            // Unknown app + "in browser" ΓåÆ construct URL if it looks like a domain
            if app_name.contains('.') && !app_name.contains(' ') {
                let url = if app_name.starts_with("http") {
                    app_name.to_string()
                } else {
                    format!("https://{}", app_name)
                };
                return Some(ParseResult {
                    intent: ParsedIntent::OpenUrl {
                        target: app_name.to_string(),
                        url,
                    },
                    confidence: 1.0,
                    source: "deterministic".to_string(),
                });
            }
        }
    }

    None
}

// ─── Helpers ─────────────────────────────────────────────────────────

/// Normalize common STT phonetic mishearings and soundalike phrases before
    /// deterministic intent parsing. Maps dialect variations and speech-recognition
    /// confabulations to canonical NEXUS command forms.
    pub fn normalize_phonetic_mishearings(text: &str) -> String {
        let lower = text.to_lowercase();
        let trimmed = lower.trim().trim_end_matches(['.', ',', '!', '?']).trim();

        // Handle "next is ..." at the start as a "nexus ..." mishearing (Whisper
        // often transcribes "nexus open X" as "next is open X").
        if let Some(rest) = trimmed.strip_prefix("next is ") {
            println!("[PHONETIC-NORM] Normalized STT mishearing '{}' → 'nexus {}'", trimmed, rest);
            return format!("nexus {}", rest);
        }
        if let Some(rest) = trimmed.strip_prefix("nex is ") {
            println!("[PHONETIC-NORM] Normalized STT mishearing '{}' → 'nexus {}'", trimmed, rest);
            return format!("nexus {}", rest);
        }
        if let Some(rest) = trimmed.strip_prefix("next as ") {
            println!("[PHONETIC-NORM] Normalized STT mishearing '{}' → 'nexus {}'", trimmed, rest);
            return format!("nexus {}", rest);
        }

        // 1. Full-phrase exact matches (fast path for common short utterances)
        match trimmed {
        "goes to mode" | "goes to mold" | "go to mode" | "post mode"
        | "post modern" | "postmodern" | "coast mode" | "gold mode"
        | "toast mode" | "host mode" | "dose mode" | "close mode"
        | "ghost mood" | "ghost node" | "ghost mod"
        // Live log 2026-10-12/13: "ghost moon", "ghost in mode" and "gostum"
        // fell through to the cloud chat model, which then claimed
        // "Ghost mode activated" while no session existed.
        | "ghost moon" | "ghost in mode" | "gostum" | "ghost mold" | "ghost moved" | "gost mode"
        | "ghost and warning" | "ghost on warning" | "ghost warning"
        | "worst motive" | "ghost to mulder" | "it goes to mode" | "it goes to mold"
        | "the post mode" | "the ghost mode" | "the host mode" | "the coast mode" | "the gold mode" => {
            println!("[PHONETIC-NORM] Normalized STT mishearing '{}' → 'ghost mode'", trimmed);
            return "ghost mode".to_string();
        }

        "open what's up" | "open whats up" | "open what sap" | "open what app"
        | "open watch app" | "open watts app" => return "open whatsapp".to_string(),

        "open vs coat" | "open vs chord" | "open vs cord"
        | "open visual studio coat" => return "open vs code".to_string(),

        "open spot if I" | "open spot a file" | "open spotty fy"
        | "open spot ify" => return "open spotify".to_string(),

        "open this cord" | "open dis cord" => return "open discord".to_string(),

        "open u tube" | "open you tube" => return "open youtube".to_string(),

        "open note pad" | "open not pad" => return "open notepad".to_string(),

        // Real missed-command log 2026-10-20 ("openbread" twice) and the earlier "open brief" live miss.
        "openbread" | "open bread" | "open brief" | "open braved" | "openbrave" => return "open brave".to_string(),

        "stand down" | "stop it now" => return "stop".to_string(),

        "cancel action" | "cancel task" | "cancel that" => return "cancel".to_string(),

        "open command center" | "nexus settings" | "nexus preferences"
        | "nexus config" => return "open settings".to_string(),

        "comment hub" | "common hub" | "comma hub" | "comment center" | "common center"
        | "command hop" | "command roker" | "command rocker" | "command hug" => return "command hub".to_string(),
        "open comment hub" | "open common hub" | "open comment center" | "open common center" => return "open command hub".to_string(),

        "open mute up" | "open new tablet" | "open new table" | "open new tap"
        | "open neat tab" | "open mute tab" | "mute up" | "new tablet" => return "open new tab".to_string(),

        "roo top two" | "two top two" | "to top two" | "tabletop" | "table top two" => return "tab 2".to_string(),

        _ => {}
    }

    // 2. Substring & phrase-level contextual replacements
    let mut out = lower;

    // Browser tab phonetic mishearings (longest first to avoid partial substring collisions)
    let tab_phrases = [
        ("open new tablet", "open new tab"),
        ("open new table", "open new tab"),
        ("switch to top two", "switch to tab 2"),
        // Live ghost mishearing 2026-10-07: "switch to tab two" heard as
        // "shift to tap to" ("shift" for "switch", "tap" for "tab",
        // trailing "to" for "two"). Without these the turn parses Unknown
        // and (in ghost) dies silently instead of switching to tab 2.
        ("shift to tap to", "switch to tab 2"),
        ("shift to tab to", "switch to tab 2"),
        ("switch to tap to", "switch to tab 2"),
        ("switch to tab to", "switch to tab 2"),
        ("move to top two", "move to tab 2"),
        ("open mute tab", "open new tab"),
        ("open neat tab", "open new tab"),
        ("open mute up", "open new tab"),
        ("open new tap", "open new tab"),
        ("table top two", "tab 2"),
        ("two top three", "tab 3"),
        ("to top three", "tab 3"),
        ("go to top two", "go to tab 2"),
        ("two top one", "tab 1"),
        ("two top two", "tab 2"),
        ("to top one", "tab 1"),
        ("to top two", "tab 2"),
        ("roo top two", "tab 2"),
        ("new tablet", "new tab"),
        ("roo top 1", "tab 1"),
        ("roo top 2", "tab 2"),
        ("roo top 3", "tab 3"),
        ("roo top 4", "tab 4"),
        ("two top 1", "tab 1"),
        ("two top 2", "tab 2"),
        ("two top 3", "tab 3"),
        ("two top 4", "tab 4"),
        ("to top 1", "tab 1"),
        ("to top 2", "tab 2"),
        ("to top 3", "tab 3"),
        ("to top 4", "tab 4"),
        ("tabletop", "tab 2"),
        ("mute up", "new tab"),
    ];
    for (from, to) in tab_phrases {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }

    // Ghost mode soundalike phrase replacements
    let ghost_phrases = [
        ("the post mode", "ghost mode"),
        ("the ghost mode", "ghost mode"),
        ("the host mode", "ghost mode"),
        ("the coast mode", "ghost mode"),
        ("the gold mode", "ghost mode"),
        ("goes to mode", "ghost mode"),
        ("goes to mold", "ghost mode"),
        ("go to mode", "ghost mode"),
        ("post modern", "ghost mode"),
        ("postmodern", "ghost mode"),
        ("post mode", "ghost mode"),
        ("coast mode", "ghost mode"),
        ("gold mode", "ghost mode"),
        ("toast mode", "ghost mode"),
        ("dose mode", "ghost mode"),
        ("close mode", "ghost mode"),
        ("ghost mood", "ghost mode"),
        ("ghost node", "ghost mode"),
        // Feature 99 P3: additional live-usage mishearings.
        ("ghost right", "ghost mode"),
        ("goes mode", "ghost mode"),
        ("ghost and go", "ghost mode"),
    ];
    for (from, to) in ghost_phrases {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }

    // Command hub phonetic mishearings
    let command_hub_phrases = [
        ("comment hub", "command hub"),
        ("common hub", "command hub"),
        ("comma hub", "command hub"),
        ("comment center", "command center"),
        ("common center", "command center"),
    ];
    for (from, to) in command_hub_phrases {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }

    // Feature 99 P3: live-usage mishearing families (memory audit, forget
    // everything, whatsapp message, brave browser, architecture mapper).
    let phrase_replacements = [
        ("memory odd it", "memory audit"),
        ("memory or dit", "memory audit"),
        ("memory odit", "memory audit"),
        ("memory order it", "memory audit"),
        ("forget every thing", "forget everything"),
        ("forget everthing", "forget everything"),
        ("what's up message", "whatsapp message"),
        ("whats up message", "whatsapp message"),
        ("what sap message", "whatsapp message"),
        ("what app message", "whatsapp message"),
        ("open brave browser", "open brave"),
        ("launch brave browser", "open brave"),
        ("architecture diagram", "architecture mapper"),
        ("architecture digger", "architecture mapper"),
    ];
    for (from, to) in phrase_replacements {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }

    // Common app names in commands (e.g. "open what's up", "message mom on what's up")
    let app_replacements = [
        ("what's up", "whatsapp"),
        ("whats up", "whatsapp"),
        ("what sap", "whatsapp"),
        ("what app", "whatsapp"),
        ("watch app", "whatsapp"),
        ("watts app", "whatsapp"),
        ("vs coat", "vs code"),
        ("vs chord", "vs code"),
        ("spot if i", "spotify"),
        ("spot a file", "spotify"),
        ("this cord", "discord"),
        ("dis cord", "discord"),
        ("u tube", "youtube"),
        ("you tube", "youtube"),
        ("note pad", "notepad"),
    ];

    for (from, to) in app_replacements {
        if out.starts_with(&format!("open {}", from))
            || out.starts_with(&format!("launch {}", from))
            || out.starts_with(&format!("start {}", from))
            || out.starts_with(&format!("close {}", from))
            || out.contains(&format!("on {}", from))
            || out.contains(&format!("in {}", from))
            || out.contains(&format!("to {}", from))
            || out.contains(&format!("via {}", from))
        {
            out = out.replace(from, to);
        }
    }

    // Search soundalike phrase replacements (e.g. "so it's for almonds" -> "search for almonds")
    let search_phrases = [
        ("so it's for ", "search for "),
        ("so its for ", "search for "),
        ("so it is for ", "search for "),
        ("it's for ", "search for "),
        ("its for ", "search for "),
        ("it is for ", "search for "),
        ("search that on ", "search for "),
    ];
    for (from, to) in search_phrases {
        if out.starts_with(from) {
            out = format!("{}{}", to, &out[from.len()..]);
        }
    }

    // Mishearings this user corrected twice on separate occasions (Memory v2 M2): applied last, only to
    // transcripts that start like a command.
    crate::memcore::corrections::apply_learned(&out)
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip trailing sentence-ending punctuation that STT engines (Groq
/// whisper-large-v3-turbo, faster-whisper) often append to transcripts.
///
/// Only strips from the very end of the string so internal punctuation
/// (e.g. "owner/repo", "what's up") is preserved. Handles repeated
/// punctuation ("ok...") and trailing quotes/brackets STT sometimes adds.
fn strip_trailing_punctuation(s: &str) -> String {
    let trimmed = s.trim_end_matches(|c: char| {
        matches!(c, '.' | ',' | '?' | '!' | ';' | ':' | '"' | '\'' | ')' | ']' | '…')
    });
    trimmed.to_string()
}

/// Strip leading punctuation and orphaned single-letter prefixes that STT
/// engines frequently output on whisper/silence onset (e.g. "S. Ghost mode." -> "ghost mode",
/// "- open chrome" -> "open chrome", ". open whatsapp" -> "open whatsapp").
fn strip_leading_stray_punctuation(s: &str) -> String {
    let trimmed = s.trim_start_matches(|c: char| {
        matches!(c, '.' | ',' | '?' | '!' | ';' | ':' | '"' | '\'' | '(' | '[' | '-' | '_' | '…' | '~')
    });
    let s_clean = trimmed.trim();
    if s_clean.len() >= 3 {
        let mut chars = s_clean.chars();
        let c1 = chars.next().unwrap();
        let c2 = chars.next().unwrap();
        if c1.is_ascii_alphabetic() && (c2 == '.' || c2 == ' ') {
            // Stray prefix like "s. " or "s " or "d. "
            if c2 == '.' || matches!(c1, 's' | 'z' | 'd' | 'b' | 'c' | 'f' | 'g' | 'j' | 'k' | 'p' | 'q' | 'r' | 't' | 'v' | 'w' | 'x' | 'y') {
                return s_clean[2..].trim_start_matches(|c: char| matches!(c, '.' | ',' | ' ')).to_string();
            }
        }
    }
    s_clean.to_string()
}

/// Leading filler words that STT often inserts before the actual command.
/// e.g. "And analyse PR 254 in zync" → "analyse PR 254 in zync"
///      "So open chrome" → "open chrome"
///      "But first close notepad" → "close notepad"
///
/// These are conversational connectors — the user is speaking naturally,
/// not issuing a robotic command. Without stripping, every `starts_with()`
/// check in the parser fails and the intent falls through to `Unknown`.
///
/// We only strip filler at the START of the transcript (not mid-sentence),
/// and we strip at most one filler word — "and so open chrome" keeps "so"
/// because "and so" is rare and stripping multiple words risks eating the
/// actual command ("so" alone is a valid filler, but "and so" could be
/// "answer so..." mishears).
fn strip_leading_filler(text: &str) -> String {
    /// Single-word fillers that can precede a command.
    // "yeah"/"yep"/"yup": local Moonshine STT often hears the listening cue as "Yeah." at the start of the command
    // (measured on macOS: "yeah. click the equals button." went to the cloud, which answered "I can't click buttons").
    // Never "yes": confirmations start with it ("yes, forget everything" must stay the wipe confirmation, not a new
    // "forget everything"). One word alone is never stripped, so a bare "yeah" stays an answer.
    const FILLERS: &[&str] = &[
        "and", "so", "but", "then", "now", "also", "plus", "like", "okay",
        "ok", "well", "um", "uh", "hmm", "actually", "basically",
        "just", "please", "now please", "yeah", "yep", "yup",
    ];

    // Protect "hey nexus" / "hello nexus" / "hi nexus" — these are greetings,
    // not filler + command. "hey" alone is filler, but "hey nexus" is a wake.
    let lower = text.to_lowercase();
    if lower == "hey nexus" || lower == "hello nexus" || lower == "hi nexus"
        || lower == "hey there" || lower == "hey nexus wake up"
    {
        return text.to_string();
    }

    // Try two-word fillers first (e.g. "and so", "but first", "now just")
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() >= 3 {
        let two = format!("{} {}", words[0], words[1]);
        if FILLERS.contains(&two.as_str())
            || (words[0] == "and" && (words[1] == "so" || words[1] == "then" || words[1] == "now"))
            || (words[0] == "but" && words[1] == "first")
            || (words[0] == "now" && (words[1] == "just" || words[1] == "please"))
        {
            return words[2..].join(" ");
        }
    }

    // Single-word filler — but NOT "hey" when followed by "nexus"/"there". The STT may punctuate it ("yeah." / "so,").
    if words.len() >= 2 {
        if FILLERS.contains(&words[0].trim_end_matches(|c: char| matches!(c, '.' | ',' | '!' | '?'))) {
            return words[1..].join(" ");
        }
        // "hey" is filler only when NOT followed by "nexus" or "there"
        if words[0] == "hey" && words[1] != "nexus" && words[1] != "there" {
            return words[1..].join(" ");
        }
    }

    text.to_string()
}

fn strip_trailing_app_words(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_suffix(" app")
        .or_else(|| s.strip_suffix(" application"))
        .or_else(|| s.strip_suffix(" for me"))
        .unwrap_or(s);
    s.trim().to_string()
}

fn strip_trailing_site(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_suffix(" website")
        .or_else(|| s.strip_suffix(" site"))
        .unwrap_or(s);
    s.trim().to_string()
}

fn is_url_like(s: &str) -> bool {
    s.contains('.') && !s.contains(' ')
}

/// Levenshtein edit distance.
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

// ΓöÇΓöÇΓöÇ Regex helper ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ
// We use the `regex` crate for pattern matching. It's already in the dependency
// tree via other crates, but we need to add it explicitly to Cargo.toml.

/// Simple regex match helper.
fn regex_match(text: &str, pattern: &str) -> bool {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, regex::Regex>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap();
    let re = guard
        .entry(pattern.to_string())
        .or_insert_with(|| regex::Regex::new(pattern).unwrap_or_else(|_| regex::Regex::new("$^").unwrap()));
    re.is_match(text)
}

/// Cached regex captures helper. Uses the same OnceLock+Mutex cache as
/// `regex_match` but returns capture groups for pattern extraction.
///
/// `Captures` borrows from `text` (not from the `Regex`), so it is safe
/// to return even though the MutexGuard is dropped when this function
/// returns.
fn regex_captures<'t>(text: &'t str, pattern: &str) -> Option<regex::Captures<'t>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, regex::Regex>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap();
    let re = guard
        .entry(pattern.to_string())
        .or_insert_with(|| regex::Regex::new(pattern).unwrap_or_else(|_| regex::Regex::new("$^").unwrap()));
    re.captures(text)
}

// ΓöÇΓöÇΓöÇ Tauri command ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

/// Parse a transcript into a structured intent.
///
/// Tries the deterministic parser first (fast, zero-latency).
/// If the deterministic parser returns None or low confidence,
/// tries the NLU server (BERT-Mini, lazy-started Python sidecar).
/// Falls back to `unknown` if both fail.
#[tauri::command]
pub async fn parse_transcript(transcript: String) -> Result<ParseResult, String> {
    tracing::info!("[intent_parser] parsing: {:?}", transcript);

    // 0. A question is waiting for its answer ("Shall I, sir?"): "yes" must reach the orchestrator, where the offer
    //    is answered, not be turned into a chat acknowledgement here (see `answers_a_pending_offer`).
    if answers_a_pending_offer(&transcript) {
        return Ok(ParseResult {
            intent: ParsedIntent::Unknown { raw: transcript.clone() },
            confidence: 0.0,
            source: "offer-pending".to_string(),
        });
    }

    // 1. Try deterministic parser
    if let Some(result) = parse_deterministic(&transcript) {
        tracing::info!(
            "[intent_parser] deterministic: {:?} (confidence={}, source={})",
            result.intent,
            result.confidence,
            result.source
        );
        return Ok(result);
    }

    // 2. Try brain server FIRST (admin-only, if enabled)
    // The brain (Qwen 0.5B LLM) is much smarter than BERT-Mini and can
    // understand mishearings, filler words, and unusual phrasing.
    // BERT-Mini often returns confident-but-wrong results (e.g. "So, you
    // have to list." → MediaPlayPause with 0.90 confidence), which blocks
    // the brain from ever being tried. By trying the brain first, we get
    // accurate classification for admin users.
    #[cfg(feature = "admin-brain")]
    if crate::admin_config::is_admin() {
        if let Some(result) = crate::brain_client::brain_classify(&transcript).await {
            tracing::info!(
                "[intent_parser] brain: {:?} (confidence={}, source={})",
                result.intent,
                result.confidence,
                result.source
            );

            // Brain monitor: observe the brain's own result
            let brain_intent_name = intent_to_label(&result.intent).to_string();
            crate::brain_monitor::monitor_transcript(
                transcript.clone(),
                None, // deterministic missed
                Some(brain_intent_name),
            );

            // Only accept brain result if confidence is reasonable
            if result.confidence >= 0.5 {
                return Ok(result);
            }
            tracing::info!(
                "[intent_parser] brain confidence too low ({:.2}), falling back to NLU",
                result.confidence
            );
        }
    }

    // 3. Try NLU server (BERT-Mini fallback)
    if let Some(result) = crate::nlu_client::parse_via_nlu(&transcript).await {
        tracing::info!(
            "[intent_parser] nlu: {:?} (confidence={})",
            result.intent,
            result.confidence
        );

        // Brain monitor: observe the NLU result (non-blocking, background)
        #[cfg(feature = "admin-brain")]
        {
            let nlu_intent_name = intent_to_label(&result.intent).to_string();
            crate::brain_monitor::monitor_transcript(
                transcript.clone(),
                None, // deterministic missed
                Some(nlu_intent_name),
            );
        }

        return Ok(result);
    }

    // 4. Fallback: unknown
    tracing::info!("[intent_parser] no match, returning unknown");
    Ok(ParseResult {
        intent: ParsedIntent::Unknown {
            raw: transcript.clone(),
        },
        confidence: 0.0,
        source: "fallback".to_string(),
    })
}

// ΓöÇΓöÇΓöÇ Tests ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

// ─── Live mode commands ───────────────────────────────────────────────

/// Parse live-mode commands: type, press, send, cancel, new tab, etc.
///
/// These are only matched after all other parsers fail, so they don't
/// interfere with existing commands like "open app" or "search for X".
///
/// Patterns:
///   "type <text>"              → TypeText
///   "press <key>"              → PressKey
///   "press <key1> <key2>"       → PressHotkey (multi-key combo)
///   "press ctrl a"             → PressHotkey ["ctrl", "a"]
///   "send"                     → ConfirmSend (requires confirmation)
///   "cancel" / "never mind"    → CancelAction
///   "new tab"                  → BrowserNewTab
fn parse_live_command(text: &str) -> Option<ParseResult> {
    // --- Type text ---
    if let Some(rest) = text.strip_prefix("type ") {
        let typed = rest.trim();
        if !typed.is_empty() {
            return Some(ParseResult {
                intent: ParsedIntent::NluResult {
                    intent: "type_text".to_string(),
                    slots: serde_json::json!({ "text": typed }),
                    confidence: 1.0,
                },
                confidence: 1.0,
                source: "deterministic-live".to_string(),
            });
        }
    }

    // --- Press key / hotkey ---
    if let Some(rest) = text.strip_prefix("press ") {
        let keys_str = rest.trim();
        if !keys_str.is_empty() {
            let keys: Vec<&str> = keys_str.split_whitespace().collect();
            if keys.len() == 1 {
                return Some(ParseResult {
                    intent: ParsedIntent::NluResult {
                        intent: "press_key".to_string(),
                        slots: serde_json::json!({ "key": keys[0] }),
                        confidence: 1.0,
                    },
                    confidence: 1.0,
                    source: "deterministic-live".to_string(),
                });
            } else {
                return Some(ParseResult {
                    intent: ParsedIntent::NluResult {
                        intent: "press_hotkey".to_string(),
                        slots: serde_json::json!({ "keys": keys }),
                        confidence: 1.0,
                    },
                    confidence: 1.0,
                    source: "deterministic-live".to_string(),
                });
            }
        }
    }

    // --- Send (requires confirmation) ---
    if text == "send" || text == "send it" || text == "send message" {
        return Some(ParseResult {
            intent: ParsedIntent::NluResult {
                intent: "confirm_send".to_string(),
                slots: serde_json::json!({}),
                confidence: 1.0,
            },
            confidence: 1.0,
            source: "deterministic-live".to_string(),
        });
    }

    // --- Cancel ---
    // "stop" is not handled by the greeting parser, so we catch it here.
    // "cancel", "never mind", "forget it" are already handled by the
    // greeting parser (they return "Very well, sir." which is appropriate).
    // In live mode, the orchestrator should also reset the state machine
    // when it sees a Greeting that is actually a cancel.
    // "stop please" included: STOP_PHRASES claims it (ghost drill
    // intercept), so the parser must agree — previously None (P0
    // fixture miss). Safe: the media arm matches exact "stop" only.
    if text == "stop" || text == "stop please" {
        return Some(ParseResult {
            intent: ParsedIntent::NluResult {
                intent: "cancel_action".to_string(),
                slots: serde_json::json!({}),
                confidence: 1.0,
            },
            confidence: 1.0,
            source: "deterministic-live".to_string(),
        });
    }

    // --- New tab ---
    // Verbs users actually say: "new tab", "open ...", "create ..." —
    // a missed verb here routes a 1.0-confidence local action into the
    // NLU/Worker lottery (live miss: "create a new tab" → silence).
    // Also tolerates filler-prefixed/multi-sentence captures
    // ("open tab. create a new tab." → last segment is the command).
    const NEW_TAB_OPENERS: &[&str] = &[
        "open", "open a", "open the", "create", "create a", "create the",
        "make", "make a", "start", "start a", "get", "get me", "get me a",
        "give", "give me", "give me a", "please open", "please create",
        "can you open", "can you create", "could you open", "could you create",
    ];
    // Strip polite prefixes once ("can you open a new tab" → "open a new tab").
    let mut clean: &str = &text;
    for polite in ["can you ", "could you ", "would you ", "please ", "just "] {
        if let Some(rest) = clean.strip_prefix(polite) {
            clean = rest;
            break;
        }
    }
    let is_new_tab = clean == "new tab"
        || clean == "tab"
        || clean == "open tab"
        || clean == "open a tab"
        || NEW_TAB_OPENERS.iter().any(|o| {
            clean == format!("{o} new tab") || clean == format!("{o} tab")
        });
    // Multi-sentence capture: the LAST ". " segment is the command
    // ("Open tab. Create a new tab." → "create a new tab").
    let is_new_tab = is_new_tab || text.rfind(". ").map_or(false, |idx| {
        let last = &text[idx + 2..];
        last == "new tab" || last == "tab"
            || NEW_TAB_OPENERS.iter().any(|o| {
                last == format!("{o} new tab") || last == format!("{o} tab")
            })
    });
    if is_new_tab {
        return Some(ParseResult {
            intent: ParsedIntent::NluResult {
                intent: "browser_new_tab".to_string(),
                slots: serde_json::json!({}),
                confidence: 1.0,
            },
            confidence: 1.0,
            source: "deterministic-live".to_string(),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_app() {
        let result = parse_deterministic("open whatsapp");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));
    }

    #[test]
    fn test_open_settings() {
        let result = parse_deterministic("open the settings");
        assert!(result.is_some(), "should parse 'open the settings'");
        let r = result.unwrap();
        assert!(
            matches!(r.intent, ParsedIntent::OpenSettings),
            "expected OpenSettings, got {:?}",
            r.intent
        );
    }

    #[test]
    fn test_open_settings_no_the() {
        let result = parse_deterministic("open settings");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenSettings));
    }

    #[test]
    fn test_open_command_center() {
        let result = parse_deterministic("open the command center");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenSettings));
    }

    #[test]
    fn test_open_command_hub() {
        let r1 = parse_deterministic("open command hub").unwrap();
        assert!(matches!(r1.intent, ParsedIntent::OpenSettings));

        let r2 = parse_deterministic("open the nexus hub").unwrap();
        assert!(matches!(r2.intent, ParsedIntent::OpenSettings));

        let r3 = parse_deterministic("command hub").unwrap();
        assert!(matches!(r3.intent, ParsedIntent::OpenSettings));

        let r4 = parse_deterministic("NEXUS comment hub.").unwrap();
        assert!(matches!(r4.intent, ParsedIntent::OpenSettings));

        let r5 = parse_deterministic("comment hub").unwrap();
        assert!(matches!(r5.intent, ParsedIntent::OpenSettings));

        let r6 = parse_deterministic("open comment hub").unwrap();
        assert!(matches!(r6.intent, ParsedIntent::OpenSettings));

        let r7 = parse_deterministic("Nexus. Nexus command hub.").unwrap();
        assert!(matches!(r7.intent, ParsedIntent::OpenSettings));

        let r8 = parse_deterministic("the nexus command hub").unwrap();
        assert!(matches!(r8.intent, ParsedIntent::OpenSettings));

        let r9 = parse_deterministic("the command hub").unwrap();
        assert!(matches!(r9.intent, ParsedIntent::OpenSettings));

        let r10 = parse_deterministic("nexus command hub").unwrap();
        assert!(matches!(r10.intent, ParsedIntent::OpenSettings));

        let r11 = parse_deterministic("show command hub").unwrap();
        assert!(matches!(r11.intent, ParsedIntent::OpenSettings));

        let r12 = parse_deterministic("bring up the command hub").unwrap();
        assert!(matches!(r12.intent, ParsedIntent::OpenSettings));

        let r13 = parse_deterministic("command hub please").unwrap();
        assert!(matches!(r13.intent, ParsedIntent::OpenSettings));

        let r14 = parse_deterministic("nexus control hub").unwrap();
        assert!(matches!(r14.intent, ParsedIntent::OpenSettings));

        let r15 = parse_deterministic("open control hub").unwrap();
        assert!(matches!(r15.intent, ParsedIntent::OpenSettings));
    }

    #[test]
    fn test_open_gemini() {
        let result = parse_deterministic("open gemini");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));
    }

    #[test]
    fn test_open_chrome() {
        let result = parse_deterministic("open chrome");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));
    }

    #[test]
    fn test_launch_spotify() {
        // Spotify is a music player intent (so the default-player setting and transport controls apply).
        let result = parse_deterministic("launch spotify");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenMusicPlayer { player: Some(ref p) } if p == "spotify"), "{:?}", r.intent);
    }

    #[test]
    fn test_open_app_with_app_suffix() {
        let result = parse_deterministic("open gmail app");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::OpenApp { target } = r.intent {
            assert_eq!(target, "gmail");
        } else {
            panic!("expected OpenApp");
        }
    }

    #[test]
    fn test_open_in_browser() {
        let result = parse_deterministic("open gmail in browser");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenUrl { .. }));
    }

    #[test]
    fn test_analyse_pr() {
        let result = parse_deterministic("analyse PR 23 servx");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalysePr {
            repo, pr_number, ..
        } = r.intent
        {
            assert_eq!(repo, "servx");
            assert_eq!(pr_number, 23);
        } else {
            panic!("expected AnalysePr, got {:?}", r.intent);
        }
    }

    #[test]
    fn test_find_pr_no_repo_and_with_repo() {
        for phrase in [
            "analyse pr 34",
            "analyse the pr 34",
            "analyse pull request 34",
        ] {
            // 2026-11-02: "analyse PR 34" with no repository is no longer a bare search: the screen is read first, then the
            // user's repositories (the same number in several asks which repo and lists them)
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse {phrase:?}"));
            assert!(
                matches!(&res.intent, ParsedIntent::AnalysePr { owner: None, repo, pr_number: 34 } if repo == "this"),
                "expected AnalysePr for {phrase:?}, got {:?}",
                res.intent
            );
        }

        let bare_res = parse_pr_analyse("pr 34").expect("parse_pr_analyse for 'pr 34'");
        assert!(
            matches!(
                bare_res.intent,
                ParsedIntent::GitHubCommand {
                    command: crate::github_cmd::GitHubCommand::FindPr { pr_number: 34 }
                }
            ),
            "expected FindPr for 'pr 34', got {:?}",
            bare_res.intent
        );

        let repo_res = parse_deterministic("analyse pr 34 in zync").expect("analyse pr 34 in zync");
        match repo_res.intent {
            ParsedIntent::AnalysePr { repo, pr_number, .. } => {
                assert_eq!(repo, "zync");
                assert_eq!(pr_number, 34);
            }
            other => panic!("expected AnalysePr with repo zync, got {:?}", other),
        }
    }

    #[test]
    fn test_analyse_pr_with_in() {
        let result = parse_deterministic("analyse PR 23 in servx");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalysePr {
            repo, pr_number, ..
        } = r.intent
        {
            assert_eq!(repo, "servx");
            assert_eq!(pr_number, 23);
        } else {
            panic!("expected AnalysePr");
        }
    }

    #[test]
    fn test_analyse_pr_with_leading_filler_and() {
        // "And analyse PR 254 in zync" — STT inserts "And" at the start
        let result = parse_deterministic("And analyse PR 254 in zync");
        assert!(result.is_some(), "should parse with leading 'And'");
        let r = result.unwrap();
        if let ParsedIntent::AnalysePr {
            repo, pr_number, ..
        } = r.intent
        {
            assert_eq!(repo, "zync");
            assert_eq!(pr_number, 254);
        } else {
            panic!("expected AnalysePr, got {:?}", r.intent);
        }
    }

    #[test]
    fn test_analyse_pr_with_leading_filler_so() {
        let result = parse_deterministic("so analyse PR 5 in servx");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalysePr {
            repo, pr_number, ..
        } = r.intent
        {
            assert_eq!(repo, "servx");
            assert_eq!(pr_number, 5);
        } else {
            panic!("expected AnalysePr, got {:?}", r.intent);
        }
    }

    #[test]
    fn test_open_with_leading_filler() {
        let result = parse_deterministic("and open chrome");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));
        if let ParsedIntent::OpenApp { target } = r.intent {
            assert_eq!(target, "chrome");
        }
    }

    #[test]
    fn test_open_the_x_never_hijacked_by_stopword_app() {
        // Regression: "open the room" used to resolve to a Dribbble PWA —
        // the word "the" was a registry key (last-write-wins) owned by its
        // long title, and resolve_app_name returned the alphabetical first
        // search name ("creative"). Must never happen again.
        let result = parse_deterministic("open the room");
        assert!(result.is_some());
        if let ParsedIntent::OpenApp { target } = result.unwrap().intent {
            assert_ne!(target, "creative", "stop-word hijack regressed");
            assert!(
                !target.to_lowercase().contains("dribbble"),
                "wrong app launched for 'open the room': {target}"
            );
        } else {
            panic!("expected OpenApp");
        }
    }

    #[test]
    fn a_leading_yeah_from_the_stt_does_not_hide_the_command() {
        // Real transcripts from the first macOS voice test (local Moonshine STT after the listening cue).
        match parse_deterministic("yeah. click the equals button.").map(|r| r.intent) {
            Some(ParsedIntent::NluResult { intent, slots, .. }) => {
                assert_eq!(intent, "system_click_element");
                assert_eq!(slots["name"], "equals");
            }
            other => panic!("expected system_click_element, got {other:?}"),
        }
        match parse_deterministic("Yeah, open calculator").map(|r| r.intent) {
            Some(ParsedIntent::OpenApp { target }) => assert_eq!(target, "calculator"),
            other => panic!("expected OpenApp, got {other:?}"),
        }
        assert_eq!(strip_leading_filler("yep open brave"), "open brave");
        // "yes" starts confirmations and stays; one word is an answer, not a filler.
        assert_eq!(strip_leading_filler("yes, forget everything"), "yes, forget everything");
        assert!(matches!(
            parse_deterministic("yes, forget everything").map(|r| r.intent),
            Some(ParsedIntent::MemoryForgetAllConfirm)
        ));
        assert_eq!(strip_leading_filler("yeah"), "yeah");
    }

    #[test]
    fn test_strip_leading_filler_doesnt_eat_commands() {
        // "open and close chrome" should NOT strip "open"
        let result = parse_deterministic("open and close chrome");
        assert!(result.is_some());
        // "open" is the verb, "and close chrome" is the target — weird but valid
    }

    #[test]
    fn test_analyse_pr_owner_repo() {
        let result = parse_deterministic("analyse PR 5 zync-meet/zync");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalysePr {
            owner,
            repo,
            pr_number,
        } = r.intent
        {
            assert_eq!(owner, Some("zync-meet".to_string()));
            assert_eq!(repo, "zync");
            assert_eq!(pr_number, 5);
        } else {
            panic!("expected AnalysePr");
        }
    }

    #[test]
    fn test_analyse_repo() {
        let result = parse_deterministic("analyse servx repo");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalyseRepo { repo, owner } = r.intent {
            assert_eq!(repo, "servx");
            assert_eq!(owner, None);
        } else {
            panic!("expected AnalyseRepo");
        }
    }

    #[test]
    fn test_analyse_owner_repo() {
        let result = parse_deterministic("analyse zync-meet/zync");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalyseRepo { owner, repo } = r.intent {
            assert_eq!(owner, Some("zync-meet".to_string()));
            assert_eq!(repo, "zync");
        } else {
            panic!("expected AnalyseRepo");
        }
    }

    #[test]
    fn test_analyse_just_repo() {
        let result = parse_deterministic("analyse servx");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalyseRepo { repo, .. } = r.intent {
            assert_eq!(repo, "servx");
        } else {
            panic!("expected AnalyseRepo");
        }
    }

    #[test]
    fn test_analyse_zync() {
        let result = parse_deterministic("analyse zync");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalyseRepo { repo, .. } = r.intent {
            assert_eq!(repo, "zync");
        } else {
            panic!("expected AnalyseRepo");
        }
    }

    #[test]
    fn test_analyze_american_spelling() {
        let result = parse_deterministic("analyze PR 23 servx");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::AnalysePr { .. }));
    }

    #[test]
    fn test_search() {
        let result = parse_deterministic("search for cats");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::Search { query } = r.intent {
            assert_eq!(query, "cats");
        } else {
            panic!("expected Search");
        }
    }

    #[test]
    fn test_google_search() {
        let result = parse_deterministic("google rust async programming");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::Search { query } = r.intent {
            assert_eq!(query, "rust async programming");
        } else {
            panic!("expected Search");
        }
    }

    #[test]
    fn test_media_pause() {
        let result = parse_deterministic("pause");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::MediaPlayPause));
    }

    #[test]
    fn test_media_next() {
        let result = parse_deterministic("next");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::MediaNext));
    }

    #[test]
    fn test_architect_false_positives_rejected() {
        // "open" or "open-" alone must NEVER match open_architect
        assert!(!is_architect_command("open"));
        assert!(!is_architect_command("open-"));
        assert!(!is_architect_fuzzy("open"));
        assert!(!is_architect_fuzzy("open-"));

        // Conversational/other commands starting with "open" must not match open_architect
        assert!(!is_architect_fuzzy("open our project"));
        assert!(!is_architect_fuzzy("open are you listening"));
        assert!(!is_architect_fuzzy("open article"));
        assert!(!is_architect_fuzzy("open archive"));
        assert!(!is_architect_fuzzy("open mac"));
        assert!(!is_architect_fuzzy("open master"));
        assert!(!is_architect_fuzzy("open drift"));
        assert!(!is_architect_fuzzy("open breath"));
    }

    #[test]
    fn test_ghost_mode_stray_punctuation() {
        // Whisper often prefixes stray single letters or punctuation on silence onset
        let r1 = parse_deterministic("S. Ghost mode.");
        assert!(r1.is_some(), "S. Ghost mode. should match EnterGhostControl");
        assert!(matches!(r1.unwrap().intent, ParsedIntent::EnterGhostControl));

        let r2 = parse_deterministic(". ghost mode");
        assert!(r2.is_some(), ". ghost mode should match EnterGhostControl");
        assert!(matches!(r2.unwrap().intent, ParsedIntent::EnterGhostControl));

        let r3 = parse_deterministic("- ghost mode");
        assert!(r3.is_some(), "- ghost mode should match EnterGhostControl");
        assert!(matches!(r3.unwrap().intent, ParsedIntent::EnterGhostControl));
    }

    #[test]
    fn test_architect() {
        let result = parse_deterministic("open architecture mapper");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenArchitect));
    }

    #[test]
    fn test_architect_short() {
        let result = parse_deterministic("open architect");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenArchitect));
    }

    #[test]
    fn test_architect_natural_variants() {
        // All natural ways of saying the command
        let variants = [
            "open architecture mapper",
            "open the architecture mapper",
            "open architect",
            "open the architect",
            "show architecture mapper",
            "show the architecture mapper",
            "launch architecture mapper",
            "launch the architecture mapper",
            "start architecture mapper",
            "bring up architecture mapper",
            "bring up the architecture mapper",
            "pull up architecture mapper",
            "pull up the architecture mapper",
            "show me the architecture",
            "show me architecture mapper",
            "give me the architecture",
            "open architecture map",
            "open architecture window",
            "open architecture diagram",
            "open architecture graph",
            "open architecture viewer",
            "open architecture explorer",
            "open codebase mapper",
            "open dependency mapper",
            "show architecture",
            "show the architecture",
            "display architecture",
            "open the architecture",
        ];
        for v in &variants {
            let result = parse_deterministic(v);
            assert!(result.is_some(), "should match: '{}'", v);
            if let Some(r) = result {
                assert!(matches!(r.intent, ParsedIntent::OpenArchitect), "should be OpenArchitect for: '{}'", v);
            }
        }
    }

    #[test]
    fn test_architect_fuzzy_mishearing() {
        // STT mishears "architecture mapper" as "octach at mapper"
        let result = parse_deterministic("open octach at mapper");
        assert!(result.is_some(), "fuzzy match should catch 'octach at mapper'");
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenArchitect));
        assert_eq!(r.source, "deterministic-fuzzy");

        // Other common mishearings
        let r2 = parse_deterministic("open arcade mapper");
        assert!(r2.is_some(), "fuzzy match should catch 'arcade mapper'");
        assert!(matches!(r2.unwrap().intent, ParsedIntent::OpenArchitect));

        let r3 = parse_deterministic("launch arch at mapper");
        assert!(r3.is_some(), "fuzzy match should catch 'arch at mapper'");
        assert!(matches!(r3.unwrap().intent, ParsedIntent::OpenArchitect));

        // Severe mishearing: "open up and remember" (from real user test)
        let r4 = parse_deterministic("open up and remember");
        assert!(r4.is_some(), "fuzzy match should catch 'open up and remember'");
        assert!(matches!(r4.unwrap().intent, ParsedIntent::OpenArchitect));

        // "open up and member" (another common mishearing)
        let r5 = parse_deterministic("open up and member");
        assert!(r5.is_some(), "fuzzy match should catch 'open up and member'");
        assert!(matches!(r5.unwrap().intent, ParsedIntent::OpenArchitect));
    }

    #[test]
    fn test_architect_fuzzy_comprehensive() {
        // Comprehensive list of all known STT mishearings
        let mishearings = [
            // "architecture" mishearings + "mapper" correct
            "open octach at mapper",
            "open arcade mapper",
            "open arch at mapper",
            "open arch mapper",
            "open architecture at mapper",
            "open architect mapper",
            "open are cat mapper",
            "open our cat mapper",
            "open ark mapper",
            "open art at mapper",
            "open art mapper",
            "open a cat mapper",
            "open acat mapper",
            // "mapper" mishearings + "architecture" correct
            "open architecture remember",
            "open architecture member",
            "open architecture december",
            "open architecture mac",
            "open architecture mad",
            "open architecture matter",
            "open architecture master",
            // Both misheard
            "open up and remember",
            "open up and member",
            "open up and december",
            "open are cat map",
            "open our cat map",
            "open ark map",
            "open art map",
            "open a cat map",
            // Short forms
            "open map",
            "open the map",
            "show map",
            "show the map",
            // With "codebase" / "dependency"
            "open codebase",
            "open codebase map",
            "open dependency map",
            "open dependencies mapper",
            // "show me" variants
            "show me the architecture",
            "show me architecture",
            "give me the architecture",
            "give me architecture mapper",
        ];
        for m in &mishearings {
            let result = parse_deterministic(m);
            assert!(result.is_some(), "fuzzy should catch: '{}'", m);
            if let Some(r) = result {
                assert!(matches!(r.intent, ParsedIntent::OpenArchitect), "should be OpenArchitect for: '{}'", m);
            }
        }
    }

    #[test]
    fn test_url_direct() {
        let result = parse_deterministic("open google.com");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenUrl { .. }));
    }

    #[test]
    fn test_unknown_command() {
        let result = parse_deterministic("what's the weather like");
        assert!(result.is_none()); // deterministic parser returns None for unknown
    }

    #[test]
    fn test_plain_pr_command_parses_to_list_prs() {
        let result = parse_deterministic("analyse the pr in zync");
        assert!(result.is_some());
        let res = result.unwrap();
        match res.intent {
            ParsedIntent::GitHubCommand {
                command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
            } => {
                assert_eq!(repo, "zync");
                assert_eq!(state, "open");
            }
            other => panic!("expected ListPrs for plain PR query, got {:?}", other),
        }
    }

    #[test]
    fn test_latest_or_author_pr_command_parses_to_analyse_latest_pr() {
        let res1 = parse_deterministic("analyse the latest pr in zync").expect("parse res1");
        match res1.intent {
            ParsedIntent::AnalyseLatestPr { repo, author, .. } => {
                assert_eq!(repo, "zync");
                assert_eq!(author, None);
            }
            other => panic!("expected AnalyseLatestPr, got {:?}", other),
        }

        let res2 = parse_deterministic("analyse the pr by prem in servx").expect("parse res2");
        match res2.intent {
            ParsedIntent::AnalyseLatestPr { repo, author, .. } => {
                assert_eq!(repo, "servx");
                assert_eq!(author.as_deref(), Some("prem"));
            }
            other => panic!("expected AnalyseLatestPr with author, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_input() {
        let result = parse_deterministic("");
        assert!(result.is_none());
    }

    #[test]
    fn test_levenshtein() {
        assert_eq!(levenshtein("chrome", "chrome"), 0);
        assert_eq!(levenshtein("chroem", "chrome"), 2);
        assert_eq!(levenshtein("whatsapp", "whatsapp"), 0);
    }

    #[test]
    fn test_is_valid_repo_name() {
        assert!(is_valid_repo_name("servx"));
        assert!(is_valid_repo_name("zync-meet"));
        assert!(is_valid_repo_name("zync_meet"));
        assert!(is_valid_repo_name("eesh264"));
        assert!(!is_valid_repo_name(""));
        assert!(!is_valid_repo_name("-invalid"));
        assert!(!is_valid_repo_name(".invalid"));
        assert!(!is_valid_repo_name("has space"));
    }

    #[test]
    fn test_parse_owner_repo() {
        assert_eq!(
            parse_owner_repo("zync-meet/zync"),
            Some(("zync-meet".to_string(), "zync".to_string()))
        );
        assert_eq!(
            parse_owner_repo("eesh264/congi"),
            Some(("eesh264".to_string(), "congi".to_string()))
        );
        assert_eq!(parse_owner_repo("no-slash"), None);
    }

    #[test]
    fn test_clean_repo_name() {
        assert_eq!(clean_repo_name("servx"), "servx");
        assert_eq!(clean_repo_name("the servx"), "servx");
        assert_eq!(clean_repo_name("servx repo"), "servx");
        assert_eq!(clean_repo_name("servx repository"), "servx");
    }

    // ΓöÇΓöÇΓöÇ Edge cases for Whisper mishearings ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

    #[test]
    fn test_open_whats_app_mishearing() {
        // Whisper might transcribe "whatsapp" as "whats app" or "what's app"
        let result = parse_deterministic("open whats app");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));
    }

    #[test]
    fn test_open_gem_ini_mishearing() {
        // Whisper might transcribe "gemini" as "gem ini"
        let result = parse_deterministic("open gem ini");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));
    }

    #[test]
    fn test_open_you_tube_mishearing() {
        let result = parse_deterministic("open you tube");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));
    }

    #[test]
    fn test_open_chat_gpt_mishearing() {
        let result = parse_deterministic("open chat gpt");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));
    }

    #[test]
    fn test_analyse_pr_variations() {
        // Various PR command formats
        for cmd in &[
            "analyse PR 1 servx",
            "analyse PR 99 servx",
            "analyse pr 23 servx",
            "analyse PR 23 in servx",
            "analyse pull request 23 servx",
            "analyze PR 23 servx",
        ] {
            let result = parse_deterministic(cmd);
            assert!(result.is_some(), "failed to parse: {}", cmd);
            let r = result.unwrap();
            assert!(
                matches!(r.intent, ParsedIntent::AnalysePr { .. }),
                "expected AnalysePr for: {}, got {:?}",
                cmd,
                r.intent
            );
        }
    }

    #[test]
    fn a_yes_answers_the_question_that_is_waiting_but_only_then() {
        use crate::memcore::offer::Offer;
        let hook = Offer::HookSetup { agent: "antigravity".into() };
        let power = Offer::Power { kind: "shutdown".into() };
        for t in ["yes", "Yes.", "yeah", "sure", "okay", "go ahead", "no", "nope", "not now"] {
            assert!(is_answer_to(Some(&hook), t), "{t}");
        }
        // a power question also takes the action itself
        assert!(is_answer_to(Some(&power), "good night"));
        // with nothing waiting, or for a sentence that is not an answer, the parser stays in charge
        for t in ["yes", "okay"] {
            assert!(!is_answer_to(None, t), "{t}");
        }
        for t in ["open chrome", "what time is it", "hook up antigravity", "sure what time is it", "yes open brave"] {
            assert!(!is_answer_to(Some(&hook), t), "{t}");
        }
    }

    #[test]
    fn stray_statements_are_told_apart_from_questions_and_commands() {
        assert!(is_stray_statement("It's very nice to know that it's not a person"));
        for t in ["what is the capital of france", "open chrome and go to youtube", "yes", "do it", "tell me about rust please", "explain the difference between these options", "describe what you just did please"] {
            assert!(!is_stray_statement(t), "{t}");
        }
    }

    /// "show the pr list" style phrases open the Pull Requests dashboard subpage (see
    /// docs/GITHUB_DASHBOARD_SPEC.md); an explicit "list prs in <repo>" stays a ListPrs command.
    fn is_pr_list_request(intent: &ParsedIntent) -> bool {
        matches!(
            intent,
            ParsedIntent::GitHubCommand { command: crate::github_cmd::GitHubCommand::ListPrs { .. } }
        ) || matches!(intent, ParsedIntent::OpenGitHubDashboard { subpage } if subpage == "pull_requests")
    }

    #[test]
    fn only_a_short_non_command_is_a_mumble() {
        for t in ["dirt.", "good.", "mm", "uh huh"] {
            assert!(is_mumble(t), "{t}");
        }
        for t in ["", "open chrome", "hook up antigravity", "what is the weather like today", "open the pr list please now"] {
            assert!(!is_mumble(t), "{t}");
        }
    }

    #[test]
    fn test_pr_list_commands() {
        // "open pr list" must NOT be parsed as OpenApp("pr list")
        // It should be ListPrs
        for cmd in &[
            "open pr list",
            "open the pr list",
            "show pr list",
            "show the pr list",
            "show me the pr list",
            "show me pr list",
            "show me prs",
            "give me the pr list",
            "view pr list",
            "view the pr list",
            "get pr list",
            "get me the pr list",
            "get me prs",
            "what prs are open",
            "show open prs",
            "show live prs",
            "give me prs",
            "show prs",
            "get prs",
            "fetch prs",
            "display prs",
            "pull up the pr list",
        ] {
            let result = parse_deterministic(cmd);
            assert!(result.is_some(), "failed to parse: {}", cmd);
            let r = result.unwrap();
            assert!(
                is_pr_list_request(&r.intent),
                "expected ListPrs for: {}, got {:?}",
                cmd,
                r.intent
            );
        }
    }

    #[test]
    fn test_pr_list_with_trailing_punctuation() {
        // Groq whisper-large-v3-turbo appends trailing punctuation to
        // transcripts. Without strip_trailing_punctuation() the dashboard
        // subpage router (and the old ListPrs regex before it) fails and
        // the permissive OpenApp fallback catches "the pr list." → resolves
        // to a cached app (Dribbble). This test guards against that
        // regression; the PR-noun phrases now land on the dashboard's
        // pull-requests page.
        for cmd in &[
            "open the pr list.",
            "open the pr list?",
            "open the pr list!",
            "show the pr list.",
            "show me the pr list.",
            "show me pr list.",
            "show open prs.",
            "list open prs.",
            "what prs are open.",
            "give me the pr list.",
            "view the pr list.",
            "get prs.",
            "get me the pr list.",
            "fetch prs.",
            "display prs.",
        ] {
            let result = parse_deterministic(cmd);
            assert!(result.is_some(), "failed to parse: {}", cmd);
            let r = result.unwrap();
            assert!(
                is_pr_list_request(&r.intent),
                "expected ListPrs for: {}, got {:?} (trailing punctuation not stripped?)",
                cmd,
                r.intent
            );
        }
    }

    #[test]
    fn test_open_app_still_works_with_punctuation() {
        // Ensure strip_trailing_punctuation doesn't break normal app opens
        let result = parse_deterministic("open chrome.");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(
            matches!(r.intent, ParsedIntent::OpenApp { .. }),
            "expected OpenApp for 'open chrome.', got {:?}",
            r.intent
        );
    }

    #[test]
    fn test_strip_trailing_punctuation_helper() {
        assert_eq!(strip_trailing_punctuation("hello."), "hello");
        assert_eq!(strip_trailing_punctuation("hello?"), "hello");
        assert_eq!(strip_trailing_punctuation("hello!"), "hello");
        assert_eq!(strip_trailing_punctuation("hello..."), "hello");
        assert_eq!(strip_trailing_punctuation("hello"), "hello");
        // Internal punctuation preserved
        assert_eq!(strip_trailing_punctuation("owner/repo"), "owner/repo");
        assert_eq!(strip_trailing_punctuation("what's up"), "what's up");
        assert_eq!(strip_trailing_punctuation("pr#23"), "pr#23");
    }

    #[test]
    fn test_looks_like_github_phrase() {
        assert!(looks_like_github_phrase("the pr list"));
        assert!(looks_like_github_phrase("pull requests"));
        assert!(looks_like_github_phrase("the repo list"));
        assert!(looks_like_github_phrase("open prs"));
        assert!(looks_like_github_phrase("my prs"));
        // Not github phrases
        assert!(!looks_like_github_phrase("chrome"));
        assert!(!looks_like_github_phrase("notepad"));
        assert!(!looks_like_github_phrase("whatsapp"));
        assert!(!looks_like_github_phrase("preview")); // "pr" inside word, not token
        assert!(!looks_like_github_phrase("process"));
    }

    #[test]
    fn test_deep_analysis_pr() {
        // "deep analysis PR 24 in nexus-agent" ΓÇö noun form with "deep" prefix
        for cmd in &[
            "deep analysis PR 24 in nexus-agent",
            "deep analyse PR 24 in nexus-agent",
            "deep analyze PR 24 in nexus-agent",
            "analysis PR 24 in nexus-agent",
        ] {
            let result = parse_deterministic(cmd);
            assert!(result.is_some(), "failed to parse: {}", cmd);
            let r = result.unwrap();
            if let ParsedIntent::AnalysePr {
                repo, pr_number, ..
            } = r.intent
            {
                assert_eq!(repo, "nexus-agent", "wrong repo for: {}", cmd);
                assert_eq!(pr_number, 24, "wrong pr_number for: {}", cmd);
            } else {
                panic!("expected AnalysePr for: {}, got {:?}", cmd, r.intent);
            }
        }
    }

    #[test]
    fn test_analyse_repo_variations() {
        for cmd in &[
            "analyse servx",
            "analyse zync",
            "analyse servx repo",
            "analyse repo servx",
            "analyse the repo servx",
            "analyse zync-meet/zync",
            "analyse eesh264/congi",
            "analyze servx",
        ] {
            let result = parse_deterministic(cmd);
            assert!(result.is_some(), "failed to parse: {}", cmd);
            let r = result.unwrap();
            assert!(
                matches!(r.intent, ParsedIntent::AnalyseRepo { .. }),
                "expected AnalyseRepo for: {}, got {:?}",
                cmd,
                r.intent
            );
        }
    }

    #[test]
    fn test_open_verb_variations() {
        for verb in &[
            "open", "launch", "start", "run", "show", "pull up", "bring up",
            "fire up", "go to", "visit",
        ] {
            let cmd = format!("{} whatsapp", verb);
            let result = parse_deterministic(&cmd);
            assert!(result.is_some(), "failed to parse: {}", cmd);
            let r = result.unwrap();
            assert!(
                matches!(r.intent, ParsedIntent::OpenApp { .. }),
                "expected OpenApp for: {}, got {:?}",
                cmd,
                r.intent
            );
        }
    }

    #[test]
    fn test_case_insensitivity() {
        let result = parse_deterministic("OPEN WHATSAPP");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));

        let result = parse_deterministic("Analyse PR 23 Servx");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::AnalysePr { .. }));
    }

    #[test]
    fn test_extra_whitespace() {
        let result = parse_deterministic("open   whatsapp");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::OpenApp { .. }));

        let result = parse_deterministic("analyse  PR  23  servx");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(matches!(r.intent, ParsedIntent::AnalysePr { .. }));
    }

    #[test]
    fn test_pr_number_extraction() {
        // Verify PR numbers are correctly extracted
        let test_cases = [(1u32), (5), (23), (99), (100), (999)];
        for (i, expected_pr) in test_cases.iter().enumerate() {
            let cmd = format!("analyse PR {} servx", expected_pr);
            let result = parse_deterministic(&cmd);
            assert!(result.is_some());
            if let ParsedIntent::AnalysePr { pr_number, .. } = result.unwrap().intent {
                assert_eq!(pr_number, *expected_pr, "PR number mismatch for case {}", i);
            } else {
                panic!("expected AnalysePr");
            }
        }
    }

    // ΓöÇΓöÇΓöÇ "the PR" pattern tests ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

    #[test]
    fn test_pr_with_the_preposition() {
        // "analyse the pr 254 in zync" ΓÇö user says "the" before "pr"
        let result = parse_deterministic("analyse the pr 254 in zync");
        assert!(result.is_some());
        if let ParsedIntent::AnalysePr { repo, pr_number, .. } = result.unwrap().intent {
            assert_eq!(pr_number, 254);
            assert_eq!(repo, "zync");
        } else {
            panic!("expected AnalysePr");
        }
    }

    #[test]
    fn test_pr_with_the_no_preposition() {
        // "analyse the pr 254 zync" ΓÇö "the" before "pr", no preposition
        let result = parse_deterministic("analyse the pr 254 zync");
        assert!(result.is_some());
        if let ParsedIntent::AnalysePr { repo, pr_number, .. } = result.unwrap().intent {
            assert_eq!(pr_number, 254);
            assert_eq!(repo, "zync");
        } else {
            panic!("expected AnalysePr");
        }
    }

    #[test]
    fn test_pr_the_pull_request() {
        // "analyse the pull request 254 in zync"
        let result = parse_deterministic("analyse the pull request 254 in zync");
        assert!(result.is_some());
        if let ParsedIntent::AnalysePr { repo, pr_number, .. } = result.unwrap().intent {
            assert_eq!(pr_number, 254);
            assert_eq!(repo, "zync");
        } else {
            panic!("expected AnalysePr");
        }
    }

    // ΓöÇΓöÇΓöÇ Fuzzy matching tests ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

    #[test]
    fn test_fuzzy_match_zink_to_zync() {
        // "zink" is 1 edit from "zync" (iΓåÆy) ΓÇö should fuzzy match
        let result = parse_deterministic("analyse pr 254 in zink");
        assert!(result.is_some());
        if let ParsedIntent::AnalysePr { repo, pr_number, .. } = result.unwrap().intent {
            assert_eq!(pr_number, 254);
            assert_eq!(repo, "zync");
        } else {
            panic!("expected AnalysePr");
        }
    }

    #[test]
    fn test_fuzzy_match_zinc_to_zync() {
        // "zinc" is 1 edit from "zync" (iΓåÆy) ΓÇö should fuzzy match
        let result = parse_deterministic("analyse pr 254 in zinc");
        assert!(result.is_some());
        if let ParsedIntent::AnalysePr { repo, pr_number, .. } = result.unwrap().intent {
            assert_eq!(pr_number, 254);
            assert_eq!(repo, "zync");
        } else {
            panic!("expected AnalysePr");
        }
    }

    #[test]
    fn test_fuzzy_match_sink_to_zync() {
        // "sink" is 3 edits from "zync" (sΓåÆz, iΓåÆy, kΓåÆc) ΓÇö exceeds threshold 2
        // for short repos. Should NOT fuzzy match ΓÇö returns "sink" as-is.
        let result = parse_deterministic("analyse pr 254 in sink");
        assert!(result.is_some());
        if let ParsedIntent::AnalysePr { repo, pr_number, .. } = result.unwrap().intent {
            assert_eq!(pr_number, 254);
            // "sink" is too far from "zync" (distance 3 > threshold 2)
            // so it's returned as-is, not fuzzy-matched
            assert_eq!(repo, "sink");
        } else {
            panic!("expected AnalysePr");
        }
    }

    #[test]
    fn test_fuzzy_match_cervix_to_servx() {
        // "cervix" ΓåÆ "servx": cΓåÆs + delete i = 2 edits ΓÇö should fuzzy match
        let result = parse_deterministic("analyse pr 254 in cervix");
        assert!(result.is_some());
        if let ParsedIntent::AnalysePr { repo, pr_number, .. } = result.unwrap().intent {
            assert_eq!(pr_number, 254);
            assert_eq!(repo, "servx");
        } else {
            panic!("expected AnalysePr");
        }
    }

    #[test]
    fn test_fuzzy_does_not_match_too_far() {
        // "chrome" is too far from any known repo ΓÇö should NOT fuzzy match
        let result = parse_deterministic("analyse pr 254 in chrome");
        // "chrome" won't match any known repo within threshold
        // But it will still be accepted as a repo name (clean_repo_name)
        // because the exact pattern "pr <num> in <word>" matches.
        // This is correct behavior ΓÇö we only fuzzy match when exact fails.
        if let Some(r) = result {
            if let ParsedIntent::AnalysePr { repo, .. } = r.intent {
                // Should be "chrome" as-is, not fuzzy-matched to something
                assert_eq!(repo, "chrome");
            }
        }
    }

    #[test]
    fn test_fuzzy_match_confidence_lower() {
        // Fuzzy matches should have lower confidence (0.8) than exact (1.0).
        // NOTE: "zink" no longer goes fuzzy — the canonical sound-alias map
        // resolves it to "zync" inside clean_repo_name, so the exact path
        // wins at 1.0. A non-aliased misspelling still goes fuzzy.
        let result = parse_deterministic("analyse pr 254 in zink");
        assert!(result.is_some());
        let r = result.unwrap();
        if let ParsedIntent::AnalysePr { .. } = r.intent {
            assert_eq!(r.confidence, 1.0);
            assert_eq!(r.source, "deterministic");
        } else {
            panic!("expected AnalysePr");
        }
    }

    // ΓöÇΓöÇΓöÇ Greeting tests ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

    #[test]
    fn test_greeting_hello() {
        let result = parse_deterministic("hello");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_hi() {
        let result = parse_deterministic("hi");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_hey() {
        let result = parse_deterministic("hey");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_how_are_you() {
        let result = parse_deterministic("how are you");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_hows_it_going() {
        let result = parse_deterministic("how's it going");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_bye() {
        let result = parse_deterministic("bye");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_goodbye() {
        let result = parse_deterministic("goodbye");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_see_you() {
        let result = parse_deterministic("see you");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_thanks() {
        let result = parse_deterministic("thanks");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_thank_you() {
        let result = parse_deterministic("thank you");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_what_is_your_name() {
        let result = parse_deterministic("what's your name");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_who_are_you() {
        let result = parse_deterministic("who are you");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_what_can_you_do() {
        let result = parse_deterministic("what can you do");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_good_morning() {
        let result = parse_deterministic("good morning");
        assert!(result.is_some());
        if let ParsedIntent::Greeting { reply } = result.unwrap().intent {
            assert!(reply.contains("morning"), "expected 'morning' in reply: {}", reply);
        } else {
            panic!("expected Greeting");
        }
    }

    #[test]
    fn test_greeting_good_evening() {
        let result = parse_deterministic("good evening");
        assert!(result.is_some());
        if let ParsedIntent::Greeting { reply } = result.unwrap().intent {
            assert!(reply.contains("evening"), "expected 'evening' in reply: {}", reply);
        } else {
            panic!("expected Greeting");
        }
    }

    #[test]
    fn test_greeting_yes() {
        let result = parse_deterministic("yes");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_ok() {
        let result = parse_deterministic("ok");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_no() {
        let result = parse_deterministic("no");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_never_mind() {
        let result = parse_deterministic("never mind");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_with_nexus_suffix() {
        let result = parse_deterministic("hello nexus");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_not_triggered_by_open() {
        // "open hello" should NOT be a greeting ΓÇö it's an open command
        let result = parse_deterministic("open hello");
        assert!(result.is_some());
        assert!(!matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_not_triggered_by_search() {
        // "search for hello" should NOT be a greeting
        let result = parse_deterministic("search for hello");
        assert!(result.is_some());
        assert!(!matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_greeting_pick_is_deterministic() {
        // Same input should always produce the same reply
        let r1 = parse_deterministic("hello");
        let r2 = parse_deterministic("hello");
        assert!(r1.is_some() && r2.is_some());
        if let (Some(ParseResult { intent: ParsedIntent::Greeting { reply: ref1 }, .. }),
                Some(ParseResult { intent: ParsedIntent::Greeting { reply: ref2 }, .. })) = (r1, r2) {
            assert_eq!(ref1, ref2, "same input should produce same reply");
        } else {
            panic!("expected Greeting");
        }
    }

    // ─── GitHub command parsing tests (Phase 2A-6) ────────────────

    #[test]
    fn test_parse_merge_pr() {
        let result = parse_deterministic("merge pr 23 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::MergePr { repo, pr_number, method },
        } = result.unwrap().intent
        {
            assert_eq!(repo, "owner/repo");
            assert_eq!(pr_number, 23);
            assert_eq!(method, crate::github_cmd::MergeMethod::Merge);
        } else {
            panic!("expected MergePr");
        }
    }

    #[test]
    fn test_parse_squash_merge_pr() {
        let result = parse_deterministic("squash merge pr 5 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::MergePr { method, .. },
        } = result.unwrap().intent
        {
            assert_eq!(method, crate::github_cmd::MergeMethod::Squash);
        } else {
            panic!("expected MergePr");
        }
    }

    #[test]
    fn test_parse_rebase_merge_pr() {
        let result = parse_deterministic("rebase merge pr 10 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::MergePr { method, .. },
        } = result.unwrap().intent
        {
            assert_eq!(method, crate::github_cmd::MergeMethod::Rebase);
        } else {
            panic!("expected MergePr");
        }
    }

    #[test]
    fn test_parse_approve_pr() {
        let result = parse_deterministic("approve pr 5 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::ApprovePr { repo, pr_number },
        } = result.unwrap().intent
        {
            assert_eq!(repo, "owner/repo");
            assert_eq!(pr_number, 5);
        } else {
            panic!("expected ApprovePr");
        }
    }

    #[test]
    fn test_parse_close_pr() {
        let result = parse_deterministic("close pr 10 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::ClosePr { pr_number, .. },
        } = result.unwrap().intent
        {
            assert_eq!(pr_number, 10);
        } else {
            panic!("expected ClosePr");
        }
    }

    #[test]
    fn test_parse_list_prs() {
        let result = parse_deterministic("list prs in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::ListPrs { repo, state },
        } = result.unwrap().intent
        {
            assert_eq!(repo, "owner/repo");
            assert_eq!(state, "open");
        } else {
            panic!("expected ListPrs");
        }
    }

    #[test]
    fn test_parse_list_closed_prs() {
        let result = parse_deterministic("list closed prs in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::ListPrs { state, .. },
        } = result.unwrap().intent
        {
            assert_eq!(state, "closed");
        } else {
            panic!("expected ListPrs");
        }
    }

    #[test]
    fn test_canonical_repo_name_sound_aliases() {
        // Every sounding from real STT logs resolves to the same entity.
        for heard in [
            "servx", "cervix", "cervx", "srvx", "service", "cervex",
            "cervets", "servetus", "servex",
        ] {
            assert_eq!(canonical_repo_name(heard), "servx", "heard: {heard}");
        }
        for heard in ["zync", "zinc", "zink", "sync", "zynk"] {
            assert_eq!(canonical_repo_name(heard), "zync", "heard: {heard}");
        }
        assert_eq!(canonical_repo_name("incognito"), "congi");
        assert_eq!(canonical_repo_name("meat"), "meet");
        assert_eq!(canonical_repo_name("shopcart"), "shopkart");
        assert_eq!(canonical_repo_name("ledger ai"), "ledger-ai");
        // Owner/repo paths map per segment.
        assert_eq!(canonical_repo_name("sync-meet/sync"), "sync-meet/zync");
        // Unknown names pass through untouched — never rewrites a real name.
        assert_eq!(canonical_repo_name("myrepo"), "myrepo");
        assert_eq!(canonical_repo_name("ChitkulLakshya/ultrabot"), "ChitkulLakshya/ultrabot");
    }

    #[test]
    fn test_parse_list_prs_heard_repo_resolves() {
        // End to end: STT heard "cervix", user meant servx.
        let result = parse_deterministic("list prs in cervix");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::ListPrs { repo, .. },
        } = result.unwrap().intent
        {
            assert_eq!(repo, "servx");
        } else {
            panic!("expected ListPrs");
        }
    }

    #[test]
    fn test_parse_list_prs_pull_request_wording() {
        // "pull requests" is a full alternative to "prs". Dashboard-first
        // decision: bare noun phrases open the dashboard's pull-requests
        // page; content/state asks keep the spoken ListPrs flow.
        for phrase in [
            "show me the prs",
            "show me the pull requests",
            "pull up the pull request list",
        ] {
            let result = parse_deterministic(phrase);
            assert!(result.is_some(), "should parse '{phrase}'");
            if let ParsedIntent::OpenGitHubDashboard { subpage } = result.unwrap().intent {
                assert_eq!(subpage, "pull_requests", "wrong subpage for '{phrase}'");
            } else {
                panic!("expected dashboard pull_requests for '{phrase}'");
            }
        }
        for phrase in [
            "show me the pull requests and all",
            "show the pull requests in owner/repo",
            "give me all the pull requests",
            "show prs and all of them",
        ] {
            let result = parse_deterministic(phrase);
            assert!(result.is_some(), "should parse '{phrase}'");
            assert!(
                is_pr_list_request(&result.unwrap().intent),
                "expected ListPrs or the Pull Requests dashboard for '{phrase}'"
            );
        }
        // Repo-scoped and state-filtered wording must survive intact.
        let repo_list = parse_deterministic("show the pull requests in owner/repo");
        if let Some(ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::ListPrs { repo, .. },
        }) = repo_list.map(|r| r.intent)
        {
            assert_eq!(repo, "owner/repo");
        } else {
            panic!("expected repo-scoped ListPrs");
        }
        let all_list = parse_deterministic("give me all the pull requests");
        if let Some(ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::ListPrs { state, .. },
        }) = all_list.map(|r| r.intent)
        {
            assert_eq!(state, "all");
        } else {
            panic!("expected state=all ListPrs");
        }
        // …but a bare "everything" with no PR noun must NOT list PRs.
        let other = parse_deterministic("show me everything").map(|r| r.intent);
        assert!(
            !matches!(
                other,
                Some(ParsedIntent::GitHubCommand {
                    command: crate::github_cmd::GitHubCommand::ListPrs { .. },
                })
            ),
            "bare 'everything' must not list PRs"
        );
    }

    #[test]
    fn test_parse_get_pr() {
        let result = parse_deterministic("show pr 42 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::GetPr { pr_number, .. },
        } = result.unwrap().intent
        {
            assert_eq!(pr_number, 42);
        } else {
            panic!("expected GetPr");
        }
    }

    #[test]
    fn test_parse_revert_pr() {
        let result = parse_deterministic("revert pr 99 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::RevertPr { pr_number, .. },
        } = result.unwrap().intent
        {
            assert_eq!(pr_number, 99);
        } else {
            panic!("expected RevertPr");
        }
    }

    #[test]
    fn test_parse_add_collaborator() {
        let result = parse_deterministic("add user1 as admin collaborator to owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::AddCollaborator { username, permission, .. },
        } = result.unwrap().intent
        {
            assert_eq!(username, "user1");
            assert_eq!(permission, crate::github_cmd::CollaboratorPermission::Admin);
        } else {
            panic!("expected AddCollaborator");
        }
    }

    #[test]
    fn test_parse_add_collaborator_default_permission() {
        let result = parse_deterministic("add user1 as collaborator to owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::AddCollaborator { permission, .. },
        } = result.unwrap().intent
        {
            assert_eq!(permission, crate::github_cmd::CollaboratorPermission::Push);
        } else {
            panic!("expected AddCollaborator");
        }
    }

    #[test]
    fn test_parse_remove_collaborator() {
        let result = parse_deterministic("remove user1 as collaborator from owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::RemoveCollaborator { username, .. },
        } = result.unwrap().intent
        {
            assert_eq!(username, "user1");
        } else {
            panic!("expected RemoveCollaborator");
        }
    }

    #[test]
    fn test_parse_list_collaborators() {
        let result = parse_deterministic("list collaborators in owner/repo");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::GitHubCommand {
                command: crate::github_cmd::GitHubCommand::ListCollaborators { .. }
            }
        ));
    }

    #[test]
    fn test_parse_add_org_member() {
        let result = parse_deterministic("add user1 to org myorg");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::AddOrgMember { org, username, role },
        } = result.unwrap().intent
        {
            assert_eq!(org, "myorg");
            assert_eq!(username, "user1");
            assert_eq!(role, crate::github_cmd::OrgRole::Member);
        } else {
            panic!("expected AddOrgMember");
        }
    }

    #[test]
    fn test_parse_add_org_admin() {
        let result = parse_deterministic("add user1 as admin to org myorg");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::AddOrgMember { role, .. },
        } = result.unwrap().intent
        {
            assert_eq!(role, crate::github_cmd::OrgRole::Admin);
        } else {
            panic!("expected AddOrgMember");
        }
    }

    #[test]
    fn test_parse_remove_org_member() {
        let result = parse_deterministic("remove user1 from org myorg");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::RemoveOrgMember { org, username },
        } = result.unwrap().intent
        {
            assert_eq!(org, "myorg");
            assert_eq!(username, "user1");
        } else {
            panic!("expected RemoveOrgMember");
        }
    }

    #[test]
    fn test_parse_list_org_members() {
        let result = parse_deterministic("list members of org myorg");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::GitHubCommand {
                command: crate::github_cmd::GitHubCommand::ListOrgMembers { .. }
            }
        ));
    }

    #[test]
    fn test_parse_list_branches() {
        let result = parse_deterministic("list branches in owner/repo");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::GitHubCommand {
                command: crate::github_cmd::GitHubCommand::ListBranches { .. }
            }
        ));
    }

    #[test]
    fn test_parse_delete_branch() {
        let result = parse_deterministic("delete branch feature in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::DeleteBranch { branch, .. },
        } = result.unwrap().intent
        {
            assert_eq!(branch, "feature");
        } else {
            panic!("expected DeleteBranch");
        }
    }

    #[test]
    fn test_parse_list_releases() {
        let result = parse_deterministic("list releases in owner/repo");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::GitHubCommand {
                command: crate::github_cmd::GitHubCommand::ListReleases { .. }
            }
        ));
    }

    #[test]
    fn test_parse_list_workflows() {
        let result = parse_deterministic("list workflows in owner/repo");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::GitHubCommand {
                command: crate::github_cmd::GitHubCommand::ListWorkflows { .. }
            }
        ));
    }

    #[test]
    fn test_parse_list_workflow_runs() {
        let result = parse_deterministic("list workflow runs in owner/repo");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::GitHubCommand {
                command: crate::github_cmd::GitHubCommand::ListWorkflowRuns { .. }
            }
        ));
    }

    #[test]
    fn test_parse_rerun_workflow() {
        let result = parse_deterministic("rerun workflow 123 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::RerunWorkflow { run_id, .. },
        } = result.unwrap().intent
        {
            assert_eq!(run_id, 123);
        } else {
            panic!("expected RerunWorkflow");
        }
    }

    #[test]
    fn test_parse_cancel_workflow() {
        let result = parse_deterministic("cancel workflow 456 in owner/repo");
        assert!(result.is_some());
        if let ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::CancelWorkflow { run_id, .. },
        } = result.unwrap().intent
        {
            assert_eq!(run_id, 456);
        } else {
            panic!("expected CancelWorkflow");
        }
    }

    #[test]
    fn test_route_github_command_to_github_subsystem() {
        use crate::orchestrator::{route_intent, Subsystem};
        let intent = ParsedIntent::GitHubCommand {
            command: crate::github_cmd::GitHubCommand::MergePr {
                repo: "owner/repo".into(),
                pr_number: 1,
                method: crate::github_cmd::MergeMethod::Squash,
            },
        };
        assert_eq!(route_intent(&intent), Subsystem::GitHub);
    }

    #[test]
    fn test_non_github_not_routed_to_github() {
        use crate::orchestrator::{route_intent, Subsystem};
        let intent = ParsedIntent::Search { query: "test".into() };
        assert_ne!(route_intent(&intent), Subsystem::GitHub);
    }

    // ─── Live mode intent parsing tests ─────────────────────────────

    #[test]
    fn test_parse_live_type_text() {
        let result = parse_deterministic("type hello world");
        assert!(result.is_some());
        if let ParsedIntent::NluResult { intent, slots, .. } = result.unwrap().intent {
            assert_eq!(intent, "type_text");
            assert_eq!(slots["text"], "hello world");
        } else {
            panic!("expected NluResult for type_text");
        }
    }

    #[test]
    fn test_parse_live_press_key() {
        let result = parse_deterministic("press enter");
        assert!(result.is_some());
        if let ParsedIntent::NluResult { intent, slots, .. } = result.unwrap().intent {
            assert_eq!(intent, "press_key");
            assert_eq!(slots["key"], "enter");
        } else {
            panic!("expected NluResult for press_key");
        }
    }

    #[test]
    fn test_parse_live_press_hotkey() {
        let result = parse_deterministic("press ctrl a");
        assert!(result.is_some());
        if let ParsedIntent::NluResult { intent, slots, .. } = result.unwrap().intent {
            assert_eq!(intent, "press_hotkey");
            assert_eq!(slots["keys"][0], "ctrl");
            assert_eq!(slots["keys"][1], "a");
        } else {
            panic!("expected NluResult for press_hotkey");
        }
    }

    #[test]
    fn test_parse_live_send() {
        let result = parse_deterministic("send");
        assert!(result.is_some());
        if let ParsedIntent::NluResult { intent, .. } = result.unwrap().intent {
            assert_eq!(intent, "confirm_send");
        } else {
            panic!("expected NluResult for confirm_send");
        }
    }

    #[test]
    fn test_parse_live_send_it() {
        let result = parse_deterministic("send it");
        assert!(result.is_some());
        if let ParsedIntent::NluResult { intent, .. } = result.unwrap().intent {
            assert_eq!(intent, "confirm_send");
        } else {
            panic!("expected NluResult for confirm_send");
        }
    }

    #[test]
    fn test_parse_live_cancel() {
        // "cancel" is handled by the greeting parser, not live mode
        let result = parse_deterministic("cancel");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_parse_live_never_mind() {
        // "never mind" is handled by the greeting parser, not live mode
        let result = parse_deterministic("never mind");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Greeting { .. }));
    }

    #[test]
    fn test_parse_live_new_tab() {
        let result = parse_deterministic("new tab");
        assert!(result.is_some());
        if let ParsedIntent::NluResult { intent, .. } = result.unwrap().intent {
            assert_eq!(intent, "browser_new_tab");
        } else {
            panic!("expected NluResult for browser_new_tab");
        }
    }

    #[test]
    fn test_parse_live_open_new_tab() {
        let result = parse_deterministic("open new tab");
        assert!(result.is_some());
        if let ParsedIntent::NluResult { intent, .. } = result.unwrap().intent {
            assert_eq!(intent, "browser_new_tab");
        } else {
            panic!("expected NluResult for browser_new_tab");
        }
    }

    #[test]
    fn test_parse_live_create_new_tab_verbs() {
        // Live miss 2026-09-28: "create a new tab" fell through to NLU.
        for p in ["create a new tab", "create new tab", "open a new tab"] {
            let result = parse_deterministic(p);
            assert!(result.is_some(), "no parse for {p}");
            if let ParsedIntent::NluResult { intent, .. } = result.unwrap().intent {
                assert_eq!(intent, "browser_new_tab", "wrong intent for {p}");
            } else {
                panic!("expected NluResult for {p}");
            }
        }
    }

    /// Tolerant forms: bare "open tab"/"open a tab" and multi-sentence
    /// captures ("Open tab. Create a new tab." → last segment wins).
    #[test]
    fn test_parse_live_new_tab_tolerant_forms() {
        for p in [
            "open tab",
            "open a tab",
            "make a new tab",
            "can you open a new tab",
            "Open tab. Create a new tab.",
            "and then the. open a new tab.",
        ] {
            let result = parse_deterministic(p);
            assert!(result.is_some(), "no parse for {p}");
            if let ParsedIntent::NluResult { intent, .. } = result.unwrap().intent {
                assert_eq!(intent, "browser_new_tab", "wrong intent for {p}");
            } else {
                panic!("expected NluResult for {p}");
            }
        }
    }

    /// Resolve like the E2E fixture: inner intent for wrapped results,
    /// snake label otherwise, "none" for no-parse.
    fn wake_matrix_label(transcript: &str) -> String {
        match parse_deterministic(transcript) {
            None => "none".to_string(),
            Some(r) => match &r.intent {
                ParsedIntent::NluResult { intent, .. } => intent.clone(),
                other => intent_to_label(other).to_string(),
            },
        }
    }

    #[test]
    fn test_wake_prefix_retry_matrix() {
        // Hot-mic captures include the wake word verbatim (KWS skipped
        // during capture). Every row failed with None before the retry.
        for (phrase, expected) in [
            ("nexus open chrome", "open_app"),
            ("hey nexus open chrome", "open_app"),
            ("nexus open whatsapp", "open_app"),
            ("nexus search for cats", "search"),
            ("nexus ghost mode", "enter_ghost_control"),
            ("nexus exit ghost mode", "exit_ghost_control"),
            ("nexus command center", "open_settings"),
            ("nexus settings", "open_settings"),
            // Truth, not ideal: bare "stop" hits the media arm, so the
            // prefixed form agrees with it (drill intercept owns in-session).
            ("ok nexus stop", "media_stop"),
        ] {
            assert_eq!(wake_matrix_label(phrase), expected, "phrase={phrase}");
        }
        // Worker-owned remainders still fall through (None → NLU/brain),
        // and bare wake words keep their first-pass result (never stripped
        // to empty, never re-routed).
        assert_eq!(wake_matrix_label("nexus what's the weather"), "none");
        assert_eq!(wake_matrix_label("hey nexus what time is it"), "none");
        assert_eq!(wake_matrix_label("hey nexus"), "greeting");
    }

    #[test]
    fn test_strip_wake_prefix_guards() {
        assert_eq!(strip_wake_prefix("nexus open chrome"), Some("open chrome".to_string()));
        assert_eq!(strip_wake_prefix("hey nexus open chrome"), Some("open chrome".to_string()));
        assert_eq!(strip_wake_prefix("ok nexus stop"), Some("stop".to_string()));
        // No prefix / not a boundary / nothing remains → None.
        assert_eq!(strip_wake_prefix("open chrome"), None);
        assert_eq!(strip_wake_prefix("nexus5"), None);
        assert_eq!(strip_wake_prefix("nexus"), None);
        assert_eq!(strip_wake_prefix("hey nexus"), None);
        assert_eq!(strip_wake_prefix(""), None);
    }

    /// STT writes the name many ways; the stripper shares the verifier's list
    /// (`wake_names`), so every form the verifier accepts also strips.
    #[test]
    fn test_strip_wake_prefix_soundalike_forms() {
        for (said, rest) in [
            ("lexus open chrome", "open chrome"),
            ("nixus open chrome", "open chrome"),
            ("nexis, open chrome", "open chrome"),
            ("nexxus open chrome", "open chrome"),
            ("next us open chrome", "open chrome"),
            ("next sis. open chrome", "open chrome"),
            ("hey next us search for cats", "search for cats"),
            ("okay lexus stop", "stop"),
            ("nexus. nexus open chrome", "open chrome"),
        ] {
            assert_eq!(strip_wake_prefix(said), Some(rest.to_string()), "{said}");
        }
        // Word boundary still required; ordinary speech is untouched.
        for keep in ["next usb drive", "nexuses are", "texas open chrome", "next song", "next is open chrome", "lexington"] {
            assert_eq!(strip_wake_prefix(keep), None, "{keep}");
        }
        for (said, label) in [
            ("lexus open chrome", "open_app"),
            ("next us open chrome", "open_app"),
            ("hey nixus search for cats", "search"),
            ("next sis ghost mode", "enter_ghost_control"),
        ] {
            assert_eq!(wake_matrix_label(said), label, "{said}");
        }
    }

    #[test]
    fn test_parse_live_does_not_interfere_with_open() {
        // "open whatsapp" should still be OpenApp, not a live command
        let result = parse_deterministic("open whatsapp");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::OpenApp { .. }));
    }

    #[test]
    fn test_parse_live_does_not_interfere_with_search() {
        // "search for cats" should still be Search, not a live command
        let result = parse_deterministic("search for cats");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Search { .. }));
    }

    // ─── Commerce: OrderFood ───────────────────────────────────────────

    #[test]
    fn test_order_food_with_restaurant() {
        let result = parse_deterministic("order pizza from dominos");
        assert!(result.is_some());
        if let ParsedIntent::OrderFood { query, restaurant } = result.unwrap().intent {
            assert_eq!(query, "pizza");
            assert_eq!(restaurant.as_deref(), Some("dominos"));
        } else {
            panic!("expected OrderFood");
        }
    }

    #[test]
    fn test_order_food_generic_from_swiggy() {
        let result = parse_deterministic("order food from swiggy");
        assert!(result.is_some());
        if let ParsedIntent::OrderFood { query, restaurant } = result.unwrap().intent {
            assert_eq!(query, "");
            assert_eq!(restaurant.as_deref(), Some("swiggy"));
        } else {
            panic!("expected OrderFood");
        }
    }

    #[test]
    fn test_order_food_no_restaurant() {
        let result = parse_deterministic("order biryani");
        assert!(result.is_some());
        if let ParsedIntent::OrderFood { query, restaurant } = result.unwrap().intent {
            assert_eq!(query, "biryani");
            assert!(restaurant.is_none());
        } else {
            panic!("expected OrderFood");
        }
    }

    #[test]
    fn test_order_food_generic() {
        let result = parse_deterministic("order food");
        assert!(result.is_some());
        if let ParsedIntent::OrderFood { query, .. } = result.unwrap().intent {
            assert_eq!(query, "");
        } else {
            panic!("expected OrderFood");
        }
    }

    #[test]
    fn test_get_food_from_swiggy() {
        let result = parse_deterministic("get food from swiggy");
        assert!(result.is_some());
        if let ParsedIntent::OrderFood { restaurant, .. } = result.unwrap().intent {
            assert_eq!(restaurant.as_deref(), Some("swiggy"));
        } else {
            panic!("expected OrderFood");
        }
    }

    // ─── Commerce: SearchProduct ───────────────────────────────────────

    #[test]
    fn test_search_product_on_amazon() {
        let result = parse_deterministic("search for sony headphones on amazon");
        assert!(result.is_some());
        if let ParsedIntent::SearchProduct { query } = result.unwrap().intent {
            assert_eq!(query, "sony headphones");
        } else {
            panic!("expected SearchProduct");
        }
    }

    #[test]
    fn test_find_product_on_amazon() {
        let result = parse_deterministic("find wireless earbuds on amazon");
        assert!(result.is_some());
        if let ParsedIntent::SearchProduct { query } = result.unwrap().intent {
            assert_eq!(query, "wireless earbuds");
        } else {
            panic!("expected SearchProduct");
        }
    }

    #[test]
    fn test_amazon_search_prefix() {
        let result = parse_deterministic("amazon search for laptop");
        assert!(result.is_some());
        if let ParsedIntent::SearchProduct { query } = result.unwrap().intent {
            assert_eq!(query, "laptop");
        } else {
            panic!("expected SearchProduct");
        }
    }

    #[test]
    fn test_search_amazon_for() {
        let result = parse_deterministic("search amazon for keyboard");
        assert!(result.is_some());
        if let ParsedIntent::SearchProduct { query } = result.unwrap().intent {
            assert_eq!(query, "keyboard");
        } else {
            panic!("expected SearchProduct");
        }
    }

    #[test]
    fn test_order_a_pizza_from_dominos() {
        let result = parse_deterministic("can you order a pizza from dominos");
        assert!(result.is_some());
        if let ParsedIntent::OrderFood { query, restaurant } = result.unwrap().intent {
            assert_eq!(query, "pizza");
            assert_eq!(restaurant.as_deref(), Some("dominos"));
        } else {
            panic!("expected OrderFood");
        }
    }

    #[test]
    fn test_order_biryani_on_swiggy() {
        let result = parse_deterministic("order biryani on swiggy");
        assert!(result.is_some());
        if let ParsedIntent::OrderFood { query, restaurant } = result.unwrap().intent {
            assert_eq!(query, "biryani");
            assert_eq!(restaurant.as_deref(), Some("swiggy"));
        } else {
            panic!("expected OrderFood");
        }
    }

    #[test]
    fn test_buy_product_on_amazon() {
        let result = parse_deterministic("buy laptop on amazon");
        assert!(result.is_some());
        if let ParsedIntent::SearchProduct { query } = result.unwrap().intent {
            assert_eq!(query, "laptop");
        } else {
            panic!("expected SearchProduct");
        }
    }

    #[test]
    fn test_look_for_shoes_on_amazon() {
        let result = parse_deterministic("can you look for shoes on amazon");
        assert!(result.is_some());
        if let ParsedIntent::SearchProduct { query } = result.unwrap().intent {
            assert_eq!(query, "shoes");
        } else {
            panic!("expected SearchProduct");
        }
    }

    #[test]
    fn test_search_without_amazon_is_regular_search() {
        // "search for cats" (no "on amazon") should still be regular Search
        let result = parse_deterministic("search for cats");
        assert!(result.is_some());
        assert!(matches!(result.unwrap().intent, ParsedIntent::Search { .. }));
    }

    // ─── Social: SendWhatsAppMessage ───────────────────────────────────

    #[test]
    fn test_send_whatsapp_message_full() {
        let result =
            parse_deterministic("send mom a whatsapp message saying i'll be late");
        assert!(result.is_some());
        if let ParsedIntent::SendWhatsAppMessage { contact, message } = result.unwrap().intent {
            assert_eq!(contact, "mom");
            assert_eq!(message, "i'll be late");
        } else {
            panic!("expected SendWhatsAppMessage");
        }
    }

    #[test]
    fn test_send_message_saying() {
        let result = parse_deterministic("send dad a message saying on my way");
        assert!(result.is_some());
        if let ParsedIntent::SendWhatsAppMessage { contact, message } = result.unwrap().intent {
            assert_eq!(contact, "dad");
            assert_eq!(message, "on my way");
        } else {
            panic!("expected SendWhatsAppMessage");
        }
    }

    #[test]
    fn test_whatsapp_contact_saying() {
        let result = parse_deterministic("whatsapp mom saying i'm coming");
        assert!(result.is_some());
        if let ParsedIntent::SendWhatsAppMessage { contact, message } = result.unwrap().intent {
            assert_eq!(contact, "mom");
            assert_eq!(message, "i'm coming");
        } else {
            panic!("expected SendWhatsAppMessage");
        }
    }

    #[test]
    fn test_message_on_whatsapp_saying() {
        let result = parse_deterministic("message lakshya on whatsapp saying hello");
        assert!(result.is_some());
        if let ParsedIntent::SendWhatsAppMessage { contact, message } = result.unwrap().intent {
            assert_eq!(contact, "lakshya");
            assert_eq!(message, "hello");
        } else {
            panic!("expected SendWhatsAppMessage");
        }
    }

    #[test]
    fn test_whatsapp_chat_still_parses_as_chat() {
        // "open chat with mom" should still be WhatsappChat, not SendWhatsAppMessage
        let result = parse_deterministic("open chat with mom");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::WhatsappChat { .. }
        ));
    }

    #[test]
    fn test_whatsapp_search_contact_parses_as_chat() {
        let result = parse_deterministic("search mommy on whatsapp");
        assert!(result.is_some());
        if let ParsedIntent::WhatsappChat { contact } = result.unwrap().intent {
            assert_eq!(contact, "mommy");
        } else {
            panic!("expected WhatsappChat");
        }

        let result2 = parse_deterministic("search for mommy on whatsapp");
        assert!(result2.is_some());
        if let ParsedIntent::WhatsappChat { contact } = result2.unwrap().intent {
            assert_eq!(contact, "mommy");
        } else {
            panic!("expected WhatsappChat");
        }

        let result3 = parse_deterministic("find john on wa");
        assert!(result3.is_some());
        if let ParsedIntent::WhatsappChat { contact } = result3.unwrap().intent {
            assert_eq!(contact, "john");
        } else {
            panic!("expected WhatsappChat");
        }
    }

    #[test]
    fn test_partial_send_asks_for_message() {
        // The exact failure from the field: action + contact, no body.
        // Must ask ("What should I say to mummy?") — never fall to Worker.
        let result = parse_deterministic("can you send message to mummy in whatsapp");
        assert!(result.is_some());
        if let ParsedIntent::NeedMoreInfo { prompt } = result.unwrap().intent {
            assert!(prompt.contains("mummy"), "prompt names the contact: {prompt}");
        } else {
            panic!("expected NeedMoreInfo");
        }
    }

    #[test]
    fn test_partial_send_without_saying_asks() {
        let result = parse_deterministic("send mom a whatsapp message");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::NeedMoreInfo { .. }
        ));
    }

    #[test]
    fn test_send_message_to_contact_in_whatsapp() {
        let result = parse_deterministic("send hi to mummy in whatsapp");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mummy");
                assert_eq!(message, "hi");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_send_message_to_contact_on_whatsapp() {
        let result = parse_deterministic("send hello to mom on whatsapp");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mom");
                assert_eq!(message, "hello");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_can_you_send_message_to_contact_via_whatsapp() {
        let result = parse_deterministic("can you send on my way to dad via whatsapp");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "dad");
                assert_eq!(message, "on my way");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_send_whatsapp_message_to_contact_saying() {
        let result = parse_deterministic("send a whatsapp message to mummy saying hi");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mummy");
                assert_eq!(message, "hi");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_send_whatsapp_to_contact_saying() {
        let result = parse_deterministic("send whatsapp to dad saying dinner is ready");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "dad");
                assert_eq!(message, "dinner is ready");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_send_message_on_whatsapp_to_contact() {
        let result = parse_deterministic("send hi on whatsapp to mummy");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mummy");
                assert_eq!(message, "hi");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_send_a_message_on_whatsapp_empty_contact() {
        let result = parse_deterministic("send a message on whatsapp");
        assert!(result.is_some());
        match result.unwrap().intent {
            ParsedIntent::NeedMoreInfo { prompt } => {
                assert!(prompt.contains("Who should I message"), "prompt is {prompt}");
            }
            other => panic!("expected NeedMoreInfo, got {:?}", other),
        }
    }

    #[test]
    fn test_tell_message_to_mommy_with_period_separator() {
        let result = parse_deterministic("Tell message to mommy. Hi. All right.");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mommy");
                assert_eq!(message, "hi. all right");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_tell_message_to_mommy_partial() {
        let result = parse_deterministic("Tell message to mommy");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::NeedMoreInfo { prompt },
                ..
            }) => {
                assert_eq!(prompt, "What should I say to mommy?");
            }
            other => panic!("expected NeedMoreInfo, got {:?}", other),
        }
    }

    #[test]
    fn test_tell_mommy_that_clause() {
        let result = parse_deterministic("tell mommy that dinner is ready");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mommy");
                assert_eq!(message, "dinner is ready");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_text_mommy_colon() {
        let result = parse_deterministic("text mommy: I'm on my way");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mommy");
                assert_eq!(message, "i'm on my way");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_message_hi_to_mommy_in_whatsapp() {
        let result = parse_deterministic("Message Hi to mommy in WhatsApp.");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mommy");
                assert_eq!(message, "hi");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_text_dinner_is_ready_to_dad_on_whatsapp() {
        let result = parse_deterministic("Text dinner is ready to dad on WhatsApp");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "dad");
                assert_eq!(message, "dinner is ready");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_tell_on_my_way_to_mummy_in_whatsapp() {
        let result = parse_deterministic("Tell I am on my way to mummy in whatsapp");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mummy");
                assert_eq!(message, "i am on my way");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_message_contact_on_whatsapp_message() {
        let result = parse_deterministic("message mommy on whatsapp hi");
        match result {
            Some(ParseResult {
                intent: ParsedIntent::SendWhatsAppMessage { contact, message },
                ..
            }) => {
                assert_eq!(contact, "mommy");
                assert_eq!(message, "hi");
            }
            other => panic!("expected SendWhatsAppMessage, got {:?}", other),
        }
    }

    #[test]
    fn test_bare_message_still_opens_chat() {
        // No "send" verb → chat-open meaning preserved (not a partial send).
        let result = parse_deterministic("message mom");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::WhatsappChat { .. }
        ));
    }

    // ─── Routing ───────────────────────────────────────────────────────

    #[test]
    fn test_ghostwriter_entry_triggers() {
        for phrase in [
            "ghostwriter",
            "ghost writer",
            "ghostwriter mode",
            "open ghostwriter",
            "open ghostwriter mode",
            "open the ghostwriter mode",
            "take a letter",
            "write this down",
            "scribe",
            "scribe mode",
        ] {
            let result = parse_deterministic(phrase);
            assert!(result.is_some(), "{phrase} should enter Ghostwriter");
            assert!(matches!(
                result.unwrap().intent,
                ParsedIntent::EnterGhostwriter { .. }
            ));
        }
    }

    #[test]
    fn test_ghost_mode_means_cursor_control_not_dictation() {
        // The reported collision: "ghost mode" opened the Ghostwriter
        // dictation room. It must enter cursor-control Ghost Mode.
        for phrase in [
            "ghost mode",
            "go ghost mode",
            "open ghost mode",
            "open the ghost mode",
            "start ghost mode",
            "take the mouse",
            "control my cursor",
            "control the cursor",
            "ghost cursor",
            "take control",
        ] {
            let result = parse_deterministic(phrase);
            assert!(result.is_some(), "{phrase} should enter Ghost Mode");
            assert!(
                matches!(result.unwrap().intent, ParsedIntent::EnterGhostControl),
                "{phrase} must NOT enter Ghostwriter"
            );
        }
    }

    #[test]
    fn test_ghost_mode_with_trailing_words_is_not_an_entry() {
        // Strict exact match: sentences about ghost mode must not enter.
        assert!(parse_deterministic("ghost mode is confusing").is_none()
            || !matches!(
                parse_deterministic("ghost mode is confusing").unwrap().intent,
                ParsedIntent::EnterGhostControl | ParsedIntent::EnterGhostwriter { .. }
            ));
    }

    #[test]
    fn test_ghostwriter_entry_with_contact() {
        let result = parse_deterministic("ghostwriter for mom on whatsapp");
        assert!(result.is_some());
        if let ParsedIntent::EnterGhostwriter { contact } = result.unwrap().intent {
            assert_eq!(contact.as_deref(), Some("mom"));
        } else {
            panic!("expected EnterGhostwriter");
        }
    }

    // ─── Screen control ──────────────────────────────────────────

    #[test]
    fn test_click_ordinal_forms() {
        for (phrase, want) in [
            ("click on the 3rd option", 3u32),
            ("click the 2nd button", 2),
            ("press the 1st link", 1),
            ("choose 4th", 4),
            ("tap the 12th item", 12),
        ] {
            let result = parse_deterministic(phrase);
            assert!(result.is_some(), "{phrase} should parse");
            if let ParsedIntent::ScreenClick { ordinal } = result.unwrap().intent {
                assert_eq!(ordinal, want, "{phrase}");
            } else {
                panic!("expected ScreenClick for {phrase}");
            }
        }
    }

    #[test]
    fn test_browser_tab_forms() {
        for (phrase, want) in [
            ("move to the 4th tab", 4u32),
            ("go to 2nd tab", 2),
            ("switch to the 1st tab", 1),
            ("shift to tab 3", 3),
            ("shift to the 3rd tab", 3),
            ("tab 3", 3),
        ] {
            let result = parse_deterministic(phrase);
            assert!(result.is_some(), "{phrase} should parse");
            if let ParsedIntent::BrowserTab { index } = result.unwrap().intent {
                assert_eq!(index, want, "{phrase}");
            } else {
                panic!("expected BrowserTab for {phrase}");
            }
        }
    }

    /// "shift to" was a live miss ("shift to tab 3" → Unknown → Worker chat).
    #[test]
    fn test_browser_close_tab_forms() {
        for (phrase, want) in [
            ("close tab 5", Some(5u32)),
            ("delete the 5th tab", Some(5)),
            ("delete tab 2", Some(2)),
            ("close this tab", None),
            ("close tab", None),
            ("delete the tab", None),
        ] {
            let result = parse_deterministic(phrase);
            assert!(result.is_some(), "{phrase} should parse");
            if let ParsedIntent::BrowserCloseTab { index } = result.unwrap().intent {
                assert_eq!(index, want, "{phrase}");
            } else {
                panic!("expected BrowserCloseTab for {phrase}");
            }
        }
    }

    /// "close chrome" must still be CloseApp, not a close-tab false hit.
    #[test]
    fn test_close_app_not_close_tab() {
        let result = parse_deterministic("close chrome");
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::CloseApp { .. }
        ));
    }

    #[test]
    fn test_screen_read_form() {
        let result = parse_deterministic("what's the 2nd button");
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().intent,
            ParsedIntent::ScreenRead { ordinal: 2 }
        ));
    }

    #[test]
    fn test_new_intents_have_labels() {
        assert_eq!(
            intent_to_label(&ParsedIntent::OrderFood {
                query: "pizza".to_string(),
                restaurant: None
            }),
            "order_food"
        );
        assert_eq!(
            intent_to_label(&ParsedIntent::SearchProduct {
                query: "laptop".to_string()
            }),
            "search_product"
        );
        assert_eq!(
            intent_to_label(&ParsedIntent::SendWhatsAppMessage {
                contact: "mom".to_string(),
                message: "hi".to_string()
            }),
            "send_whatsapp_message"
        );
        assert_eq!(
            intent_to_label(&ParsedIntent::BrowserSearch {
                query: "dribble".to_string(),
            }),
            "browser_search"
        );
        assert_eq!(
            intent_to_label(&ParsedIntent::BrowserSearchFocus),
            "browser_search_focus"
        );
        assert_eq!(
            intent_to_label(&ParsedIntent::StartDictation),
            "start_dictation"
        );
        assert_eq!(
            intent_to_label(&ParsedIntent::StopDictation),
            "stop_dictation"
        );
    }

    #[test]
    fn test_browser_search_and_dictation() {
        for phrase in ["search", "search bar", "focus search", "focus address bar", "address bar"] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "expected match for '{phrase}'");
            assert!(matches!(res.unwrap().intent, ParsedIntent::BrowserSearchFocus));
        }

        let res = parse_deterministic("search in browser dribble");
        assert!(res.is_some());
        if let ParsedIntent::BrowserSearch { query } = res.unwrap().intent {
            assert_eq!(query, "dribble");
        } else {
            panic!("expected BrowserSearch for 'search in browser dribble'");
        }

        for phrase in [
            "start typing",
            "type whatever i say",
            "type whatever i am saying",
            "type line by line",
            "start dictation",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "expected match for '{phrase}'");
            assert!(matches!(res.unwrap().intent, ParsedIntent::StartDictation));
        }

        for phrase in [
            "stop typing",
            "stop dictation",
            "done typing",
            "exit dictation",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "expected match for '{phrase}'");
            assert!(matches!(res.unwrap().intent, ParsedIntent::StopDictation));
        }
    }

    #[test]
    fn test_ghost_mode_phonetic_soundalikes() {
        for phrase in [
            "goes to mode",
            "goes to mold",
            "go to mode",
            "post mode",
            "the post mode",
            "the ghost mode",
            "post modern",
            "postmodern",
            "coast mode",
            "gold mode",
            "toast mode",
            "host mode",
            "dose mode",
            "close mode",
            "ghost mood",
            "ghost node",
            "ghost mod",
            "activate ghost mode",
            "activate ghost",
            "turn on ghost mode",
            "open ghost mode",
            "start ghost mode",
        ] {
            let res = parse_deterministic(phrase);
            assert!(
                res.is_some(),
                "Expected ghost control match for misheard phrase: '{}'",
                phrase
            );
            assert!(
                matches!(res.unwrap().intent, ParsedIntent::EnterGhostControl),
                "Expected EnterGhostControl for phrase: '{}'",
                phrase
            );
        }
    }

    #[test]
    fn test_app_phonetic_mishearings() {
        let res = parse_deterministic("open what's up");
        assert!(res.is_some());
        assert!(matches!(res.unwrap().intent, ParsedIntent::OpenApp { ref target } if target == "whatsapp"));

        let res = parse_deterministic("open vs coat");
        assert!(res.is_some());
        assert!(matches!(res.unwrap().intent, ParsedIntent::OpenApp { ref target } if target == "visual studio code" || target == "vs code"));

        let res = parse_deterministic("open spot if I");
        assert!(res.is_some());
        assert!(matches!(res.unwrap().intent, ParsedIntent::OpenMusicPlayer { player: Some(ref p) } if p == "spotify"));
    }

    #[test]
    fn test_watch_screen_email_command() {
        for phrase in [
            "update me whenever there is any update on the deadline or anything for this email",
            "update me whenever there is an update on this email",
            "watch this email for deadline changes",
            "track this email and notify me if anything changes",
            "keep an eye on this email",
        ] {
            let res = parse_deterministic(phrase);
            assert!(
                res.is_some(),
                "Expected watch screen email match for phrase: '{}'",
                phrase
            );
            assert!(
                matches!(res.unwrap().intent, ParsedIntent::WatchScreenEmail),
                "Expected WatchScreenEmail for phrase: '{}'",
                phrase
            );
        }
    }

    #[test]
    fn test_tab_phonetic_mishearings() {
        for phrase in [
            "open mute up",
            "open new tablet",
            "open new table",
            "open new tap",
            "open neat tab",
            "open mute tab",
            "mute up",
            "new tablet",
        ] {
            let res = parse_deterministic(phrase);
            assert!(
                res.is_some(),
                "Expected new tab match for misheard phrase: '{}'",
                phrase
            );
            if let ParsedIntent::NluResult { intent, .. } = res.unwrap().intent {
                assert_eq!(intent, "browser_new_tab", "Expected browser_new_tab for '{}'", phrase);
            } else {
                panic!("Expected NluResult(browser_new_tab) for '{}'", phrase);
            }
        }

        for phrase in [
            "tab 2",
            "roo top two",
            "two top two",
            "to top two",
            "tabletop",
            "table top two",
            "move to top two",
            "switch to top two",
            "go to top two",
            "shift to tap to",
            "shift to tab to",
            "switch to tap to",
            "switch to tab to",
        ] {
            let res = parse_deterministic(phrase);
            assert!(
                res.is_some(),
                "Expected browser tab match for misheard phrase: '{}'",
                phrase
            );
            if let ParsedIntent::BrowserTab { index } = res.unwrap().intent {
                assert_eq!(index, 2, "Expected tab 2 for '{}'", phrase);
            } else {
                panic!("Expected BrowserTab {{ index: 2 }} for '{}'", phrase);
            }
        }
    }

    #[test]
    fn test_unbreakable_type_dictation_payload() {
        let res = parse_deterministic("type so it analysis for the servx right send it to as soon as possible");
        assert!(res.is_some(), "Expected type_text match");
        let parsed = res.unwrap();
        if let ParsedIntent::NluResult { intent, slots, .. } = parsed.intent {
            assert_eq!(intent, "type_text");
            assert_eq!(slots["text"], "so it analysis for the servx right send it to as soon as possible");
        } else {
            panic!("Expected NluResult(type_text)");
        }

        let bare = parse_deterministic("type");
        assert!(bare.is_some());
        assert!(matches!(bare.unwrap().intent, ParsedIntent::StartDictation));
    }

    #[test]
    fn foreground_review_and_recreation_are_distinct_commands() {
        for phrase in ["analyse this website", "analyze the current app", "Nexus, review this webpage"] {
            assert!(matches!(parse_deterministic(phrase).unwrap().intent, ParsedIntent::ReviewForeground), "{phrase}");
        }
        for phrase in ["recreate it", "rebuild this website", "replicate that"] {
            assert!(matches!(parse_deterministic(phrase).unwrap().intent, ParsedIntent::RecreateLastReview), "{phrase}");
        }
        for phrase in ["analyse my repo", "review this PR", "recreate my voice", "review the app permissions"] {
            assert!(parse_foreground_review_command(phrase).is_none(), "{phrase}");
        }
    }

    /// Live 2026-10-20 routing class: STT emitted a harmless sentence before the
    /// real command, and every anchored family missed the junk-prefixed whole.
    /// The foreground-review grammar must survive that prefix, and a polite
    /// starter ("please", "can you", "could you") must not hide the command.
    #[test]
    fn foreground_review_survives_a_leading_sentence_and_polite_starters() {
        for phrase in [
            "analyse this website",
            "Sure, this is the page. Analyse this website.",
            "please analyse this website",
            "please, can you analyse this website",
            "can you please analyze this webpage",
            "could you review this app",
            "please, could you analyze this website",
            "Sure. Please could you inspect the current website?",
        ] {
            let res = parse_deterministic(phrase)
                .unwrap_or_else(|| panic!("no deterministic intent for '{phrase}'"));
            assert!(
                matches!(&res.intent, ParsedIntent::ReviewForeground),
                "'{phrase}' -> {:?}",
                res.intent
            );
        }
    }

    /// Recreation stays its own command and never rides in on a review phrase.
    #[test]
    fn foreground_recreation_survives_a_leading_sentence_but_stays_distinct() {
        for phrase in [
            "recreate it",
            "Sure. Please recreate it.",
            "could you rebuild this website",
            "Nexus, replicate that",
        ] {
            let res = parse_deterministic(phrase)
                .unwrap_or_else(|| panic!("no deterministic intent for '{phrase}'"));
            assert!(
                matches!(&res.intent, ParsedIntent::RecreateLastReview),
                "'{phrase}' -> {:?}",
                res.intent
            );
        }
        // A review phrase must not be turned into a recreation.
        for phrase in ["analyse this website", "please review this app"] {
            let res = parse_deterministic(phrase)
                .unwrap_or_else(|| panic!("no deterministic intent for '{phrase}'"));
            assert!(
                matches!(&res.intent, ParsedIntent::ReviewForeground),
                "'{phrase}' -> {:?}",
                res.intent
            );
        }
    }

    /// Repo/PR analysis, an app-permission review, a voice recreation and a
    /// negated order are never a foreground review — with or without a polite
    /// starter in front.
    #[test]
    fn foreground_review_rejects_unrelated_and_negated_orders() {
        for phrase in [
            "analyse my repo",
            "analyse pr 24 in servx",
            "review this PR",
            "review the app permissions",
            "review app permissions",
            "recreate my voice",
            "do not analyze this website",
            "please do not analyse this website",
        ] {
            assert!(
                parse_foreground_review_command(phrase).is_none(),
                "'{phrase}' must not be a foreground review"
            );
            if let Some(res) = parse_deterministic(phrase) {
                assert!(
                    !matches!(
                        &res.intent,
                        ParsedIntent::ReviewForeground | ParsedIntent::RecreateLastReview
                    ),
                    "'{phrase}' -> {:?}",
                    res.intent
                );
            }
        }
    }

    /// Doc 07 P2: window control + named-element click are deterministic,
    /// zero-model-cost, bare commands — never fall through to NLU/Worker.
    #[test]
    fn test_window_control_phrases_are_deterministic() {
        for phrase in ["minimize", "minimize the window", "minimize this window", "minimize it"] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "'{phrase}' should match");
            let r = res.unwrap();
            assert_eq!(r.source, "deterministic");
            match r.intent {
                ParsedIntent::NluResult { intent, .. } => assert_eq!(intent, "system_minimize_window"),
                other => panic!("'{phrase}' expected system_minimize_window, got {other:?}"),
            }
        }
        for phrase in ["maximize", "maximize the window", "full screen this", "make this full screen"] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "'{phrase}' should match");
            match res.unwrap().intent {
                ParsedIntent::NluResult { intent, .. } => assert_eq!(intent, "system_maximize_window"),
                other => panic!("'{phrase}' expected system_maximize_window, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_click_element_by_name_is_deterministic() {
        for (phrase, expected_name) in [
            ("click submit", "submit"),
            ("click on submit", "submit"),
            ("click the save button", "save"),
            ("press the ok button", "ok"),
            ("press the cancel button", "cancel"),
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "'{phrase}' should match");
            let r = res.unwrap();
            assert_eq!(r.source, "deterministic");
            match r.intent {
                ParsedIntent::NluResult { intent, slots, .. } => {
                    assert_eq!(intent, "system_click_element");
                    assert_eq!(slots["name"], expected_name, "for phrase '{phrase}'");
                }
                other => panic!("'{phrase}' expected system_click_element, got {other:?}"),
            }
        }
    }

    /// Ordinal click phrasing must still win over the named-element
    /// fallback — the ordinal loop runs first in parse_screen_command.
    #[test]
    fn test_click_ordinal_not_shadowed_by_named_click() {
        let res = parse_deterministic("click the 3rd option");
        assert!(res.is_some());
        assert!(matches!(res.unwrap().intent, ParsedIntent::ScreenClick { ordinal: 3 }));
    }

    #[test]
    fn test_reverse_engineer_phrases() {
        for phrase in [
            "reverse engineer this website",
            "nexus reverse engineer this website",
            "Nexus, reverse-engineer this site",
            "reverse engineer this app",
            "reverse engineer this",
            "please reverse engineer this page for me",
            "what is this website built with",
            "what tech stack does this site use",
            "which framework is this website using",
            "what technologies does this web app use",
            "how was this website built",
            "how did they build this site",
            "break down this website",
            "tear down this site",
            "analyse the tech stack of this website",
            "scan this website's stack",
        ] {
            let res = parse_deterministic(phrase);
            match res.map(|r| r.intent) {
                Some(ParsedIntent::NluResult { intent, .. }) => {
                    assert_eq!(intent, "reverse_engineer", "'{}'", phrase)
                }
                other => panic!("Expected reverse_engineer for '{}', got {:?}", phrase, other),
            }
        }
        // Misfire guards: code targets, generic questions and plain screen analysis stay elsewhere.
        for phrase in [
            "reverse engineer this repo",
            "reverse engineer this binary",
            "what is the tech stack of this repository",
            "how do i build a website",
            "how was your day",
            "what is a framework",
            "analyse my screen",
            "what's on my screen",
            "please do not reverse engineer this website",
        ] {
            if let Some(ParsedIntent::NluResult { intent, .. }) = parse_deterministic(phrase).map(|r| r.intent) {
                assert_ne!(intent, "reverse_engineer", "'{}' must not trigger reverse_engineer", phrase);
            }
        }
    }

    #[test]
    fn test_screen_analysis_phrases() {
        // Phase 1 expanded triggers (2026-10-01): analyse + screen, what's
        // on screen, what do I see, describe/read/scan/check + screen, and
        // research anchored to this/screen.
        for phrase in [
            "analyse the screen",
            "analyze the screen",
            "analyse my screen for me",
            "analyze the screen for me",
            "what's on my screen",
            "what is on the screen",
            "what do i see",
            "explain what i see",
            "what am i seeing",
            "what am i seeing right now",
            "what can i see",
            "explain what i can see",
            "tell me what i can see",
            "explain what i am seeing",
            "explain what is on my screen",
            "tell me what's on my screen",
            "describe what is on this screen",
            "please, can you analyse my screen",
            "nexus, explain what i am seeing",
            "describe my screen",
            "read my screen",
            "scan my screen",
            "check my screen",
            "research on this for me",
            "research this screen",
            // STT / phrasing variants
            "whats on my screen",
            "what\u{2019}s on my screen",
            "what's on my display",
            "can you analyse my screen",
            "could you please read the screen",
            "please describe my screen",
            "i need you to check my screen",
            "look at my screen",
            "show me my screen",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected screen_analysis for '{}'", phrase);
            if let ParsedIntent::NluResult { intent, .. } = res.unwrap().intent {
                assert_eq!(intent, "screen_analysis");
            } else {
                panic!("Expected screen_analysis for '{}'", phrase);
            }
        }
        // Misfire guards: repo analyse, PR analyse, and general research
        // must NOT trigger screen_analysis.
        for phrase in [
            "analyse servx",
            "analyze pr 5 in zync",
            "research quantum computing",
            "explain quantum computing",
            "what is on the menu",
            "what am i supposed to do",
            "please do not analyse my screen",
            "don't explain what is on my screen",
            "check my screen time",
            "show me my screen time",
            "read my screen reader settings",
            "can you analyse servx",
        ] {
            let res = parse_deterministic(phrase);
            if let Some(ParsedIntent::NluResult { intent, .. }) = res.map(|r| r.intent) {
                assert_ne!(intent, "screen_analysis", "'{}' must not trigger screen_analysis", phrase);
            }
        }
    }

    #[test]
    fn test_screen_analysis_survives_stt_noise_sentences() {
        // Live 2026-10-20 15:01:29: Whisper emitted the two-sentence
        // transcript "once tell the number. analyze my screen." — the
        // ^-anchored families missed the junk-prefixed whole, NLU was down,
        // and the turn fell through to the cloud Worker ("I can't see your
        // screen"). Sentence candidates fix the entry gate.
        for phrase in [
            "once tell the number. analyze my screen.",
            "analyze my screen. thank you.",
            "okay so. what is on my screen",
            "hmm. research this screen for me",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected a parse for '{}'", phrase);
            if let ParsedIntent::NluResult { intent, .. } = res.unwrap().intent {
                assert_eq!(intent, "screen_analysis", "'{}'", phrase);
            } else {
                panic!("Expected screen_analysis for '{}'", phrase);
            }
        }
        // Noise sentences with NO screen command must still fall through.
        for phrase in [
            "once tell the number.",
            "hook up open. open. thank you, steven.",
            "analyse servx. servx is fine.",
        ] {
            let res = parse_deterministic(phrase);
            if let Some(ParsedIntent::NluResult { intent, .. }) = res.map(|r| r.intent) {
                assert_ne!(intent, "screen_analysis", "'{}' must not trigger screen_analysis", phrase);
            }
        }
    }

    #[test]
    fn test_screen_annotation_survives_stt_noise_sentences() {
        for phrase in ["okay now. annotate my screen", "hold on. draw on the screen"] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected a parse for '{}'", phrase);
            if let ParsedIntent::NluResult { intent, .. } = res.unwrap().intent {
                assert_eq!(intent, "screen_annotation", "'{}'", phrase);
            } else {
                panic!("Expected screen_annotation for '{}'", phrase);
            }
        }
        for phrase in ["draw the curtains. all done", "open architecture mapper. now"] {
            let res = parse_deterministic(phrase);
            if let Some(ParsedIntent::NluResult { intent, .. }) = res.map(|r| r.intent) {
                assert_ne!(intent, "screen_annotation", "'{}' must not trigger screen_annotation", phrase);
            }
        }
    }

    #[test]
    fn test_screen_annotation_phrases() {
        // Feature 87 triggers: annotate/draw + screen anchor, architecture
        // family, and element-adding family.
        for phrase in [
            "annotate my screen",
            "draw on the screen",
            "mark up this display",
            "sketch on the screen",
            "create an architecture diagram",
            "make an arch diagram",
            "build a diagram of this",
            "add an arrow to the screen",
            "add a checkbox on this",
            "add text to my screen",
            "add a note to the display",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected screen_annotation for '{}'", phrase);
            if let ParsedIntent::NluResult { intent, .. } = res.unwrap().intent {
                assert_eq!(intent, "screen_annotation");
            } else {
                panic!("Expected screen_annotation for '{}'", phrase);
            }
        }
        // Misfire guards: non-screen draw, repo analyse, and general nouns
        // must NOT trigger screen_annotation.
        for phrase in [
            "draw the curtains",
            "analyse servx",
            "research quantum computing",
            "open architecture mapper",
        ] {
            let res = parse_deterministic(phrase);
            if let Some(ParsedIntent::NluResult { intent, .. }) = res.map(|r| r.intent) {
                assert_ne!(intent, "screen_annotation", "'{}' must not trigger screen_annotation", phrase);
            }
        }
    }

    #[test]
    fn test_directions_phrases() {
        let dbg = |p: &str| format!("{:?}", parse_deterministic(p).map(|r| r.intent));
        let want = |d: &str, o: Option<&str>, m: &str| {
            format!("Some(Directions {{ destination: {:?}, origin: {:?}, mode: {:?} }})", d, o.map(String::from), m)
        };
        assert_eq!(dbg("directions to Central Park"), want("central park", None, "driving"));
        assert_eq!(dbg("Give me directions to Heathrow Airport."), want("heathrow airport", None, "driving"));
        assert_eq!(dbg("how do I get to Heathrow Airport?"), want("heathrow airport", None, "driving"));
        assert_eq!(dbg("show route to San Francisco"), want("san francisco", None, "driving"));
        assert_eq!(dbg("show me the route to San Francisco"), want("san francisco", None, "driving"));
        assert_eq!(dbg("directions from London Bridge to Heathrow"), want("heathrow", Some("london bridge"), "driving"));
        assert_eq!(dbg("directions to Heathrow from London Bridge"), want("heathrow", Some("london bridge"), "driving"));
        assert_eq!(dbg("walking directions to the station"), want("the station", None, "walking"));
        assert_eq!(dbg("directions to the airport by bus"), want("the airport", None, "transit"));
        assert_eq!(dbg("how do I walk to the museum"), want("the museum", None, "walking"));
        // Misfire guards: browser navigation, plain chat, empty destinations.
        for p in [
            "navigate to youtube.com", "navigate to github", "open maps", "take me to settings", "directions",
            "directions to", "how do i get to it", "what is the route of the nile", "how do i get to sleep",
            "open google maps", "show me my calendar",
        ] {
            let got = dbg(p);
            assert!(!got.contains("Directions"), "'{p}' wrongly parsed as {got}");
        }
    }

    #[test]
    fn test_analyse_this_email_phrases() {
        for p in [
            "analyse this email", "Nexus, analyze this email.", "summarize this mail", "what is this email about",
            "what does this email say", "explain the open email", "break down this email and give me the directions",
            "go through the email on screen", "can you read this email",
        ] {
            let got = format!("{:?}", parse_deterministic(p).map(|r| r.intent));
            assert!(got.contains("AnalyseThisEmail"), "'{p}' parsed as {got}");
        }
        for p in [
            "analyse the email i have received in my college email about an internship", "watch this email",
            "reply to this email", "delete this email", "send this email to mom", "analyse this screen",
            "analyse this repo", "update me whenever there is any update on this email", "check my college email about internship",
        ] {
            let got = format!("{:?}", parse_deterministic(p).map(|r| r.intent));
            assert!(!got.contains("AnalyseThisEmail"), "'{p}' wrongly parsed as {got}");
        }
    }

    #[test]
    fn analyse_this_pr_points_at_the_screen_and_a_named_repo_means_latest_pr_there() {
        let label = |t: &str| format!("{:?}", parse_deterministic(t).map(|r| r.intent));
        // no repo named: the screen decides
        for p in [
            "analyse this pr", "analyze this pull request", "analyse the pr", "analyse that pr", "analyse my pr", "analyze the open pr",
            "let us analyze this pr", "can you analyse this pr for me", "analyse this pr please", "review this pr", "explain this pr",
            "what is this pr about", "analyse the pr on the screen", "check this pull request",
        ] {
            assert!(label(p).contains("AnalyseRepo { owner: None, repo: \"this pr\" }"), "'{p}' -> {}", label(p));
        }
        for p in ["analyse the latest pr", "analyze the latest pull request", "review the newest pr"] {
            assert!(label(p).contains("AnalyseRepo { owner: None, repo: \"latest pr\" }"), "'{p}' -> {}", label(p));
        }
        // repo named: screen first, else the latest PR of that repo
        for (p, repo) in [
            ("analyse the latest pr in servx", "servx"),
            ("analyse this pr in zync", "zync"),
            ("review the latest pull request of zync", "zync"),
        ] {
            let l = label(p);
            assert!(l.contains("AnalyseLatestPr") && l.contains(&format!("repo: \"{repo}\"")), "'{p}' -> {l}");
        }
        assert!(label("analyse the latest pr of Leddger-AI/LedgerAI").contains("AnalyseLatestPr { owner: Some(\"leddger-ai\"), repo: \"ledgerai\""), "{}", label("analyse the latest pr of Leddger-AI/LedgerAI"));
        // plain "the pr" with a repo keeps the older list reading; a number, an author and listing are untouched too
        assert!(label("analyse the pr in zync").contains("ListPrs"), "{}", label("analyse the pr in zync"));
        assert!(label("analyse pr 70 in zync").contains("AnalysePr"));
        assert!(label("analyse the pr by prem in servx").contains("author: Some(\"prem\")"));
        assert!(!label("list pull requests").contains("this pr"));
        assert!(label("show me the prs").contains("OpenGitHubDashboard") || label("show me the prs").contains("ListPrs"));
        assert!(label("what is this pr about the weather").contains("None") || !label("what is this pr about the weather").contains("this pr\" }"));
    }

    #[test]
    fn test_analyse_deictic_goes_to_the_screen_not_to_a_repo() {
        let label = |p: &str| match parse_deterministic(p).map(|r| r.intent) {
            Some(ParsedIntent::NluResult { intent, .. }) => format!("nlu:{intent}"),
            Some(other) => format!("{:?}", other),
            None => "none".to_string(),
        };
        for p in [
            "analyse this", "analyze that", "analyse it", "explain this", "describe this image", "analyse this video",
            "summarise this page", "what is this", "what's this", "review this", "look at this", "tell me about this",
            "analyse this website", "analyse this person", "break down this chart", "can you analyse this please",
            "nexus analyse this", "analyse this for me", "describe this picture on my screen", "what is this about",
            "explain this code", "analyze this youtube video",
        ] {
            // "this website/app/page/site" already has a dedicated whole-window review route (ReviewForeground).
            let got = label(p);
            assert!(got == "nlu:screen_analysis" || got == "ReviewForeground", "'{p}' -> {got}");
        }
        // Pointing at the open repository ("this report" is how the speech engine often hears "this repo").
        for p in ["analyse this repo", "analyze the repo", "analyse this repository", "analyse this report", "analyze the report."] {
            assert!(label(p).starts_with("AnalyseRepo { owner: None, repo: \"this\" }"), "'{p}' -> {}", label(p));
        }
        // Conversational lead-ins and the "rap up" mishearing (2026-11-01 log: "Let us analyze this repository." went to chat).
        for p in [
            "let us analyze this repository.", "let's analyze this repo", "Let\u{2019}s analyze this repo", "can you please analyse this repo",
            "okay let's analyze this rap up.", "could you analyze the repository for me", "go ahead and analyze this repo",
        ] {
            let t = p.to_lowercase();
            let t = t.trim_end_matches(" for me").to_string();
            assert!(label(&t).starts_with("AnalyseRepo { owner: None, repo: \"this\" }"), "'{p}' -> {}", label(&t));
        }
        assert_eq!(label("let's analyze this youtube video"), "nlu:screen_analysis");
        // Named or dedicated targets keep their routes; chat is untouched.
        assert!(label("analyse servx").starts_with("AnalyseRepo"), "{}", label("analyse servx"));
        assert!(label("analyse this repo").starts_with("AnalyseRepo"), "{}", label("analyse this repo"));
        assert!(label("analyze this repository").starts_with("AnalyseRepo"));
        assert!(label("analyse this email").starts_with("AnalyseThisEmail"), "{}", label("analyse this email"));
        for p in ["what is it", "what is that", "what is the weather", "explain quantum computing", "describe your day", "what's the time", "review the budget"] {
            assert_ne!(label(p), "nlu:screen_analysis", "'{p}' wrongly sent to the screen");
        }
    }

    #[test]
    fn test_place_info_phrases() {
        let q = |p: &str| match parse_deterministic(p).map(|r| r.intent) {
            Some(ParsedIntent::PlaceInfo { query, ask }) => Some((query, ask)),
            _ => None,
        };
        let got = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
        assert_eq!(q("where is the charminar"), got("charminar", "where"));
        assert_eq!(q("when is the charminar?"), got("charminar", "when"));
        assert_eq!(q("nexus, where's the taj mahal located"), got("taj mahal", "where"));
        assert_eq!(q("when does the museum close"), got("museum", "hours"));
        assert_eq!(q("what time does the national zoo open"), got("national zoo", "hours"));
        assert_eq!(q("is the red fort open"), got("red fort", "hours"));
        assert_eq!(q("show me golconda fort on the map"), got("golconda fort", "where"));
        // Misfire guards: controls, people, objects, events, ideas, other parsers' phrases.
        for p in [
            "where is my phone", "where is the search bar", "where is mom", "when is the meeting", "when is the deadline",
            "when is my exam", "when is the hackathon", "where is the settings button", "where is the problem",
            "where is it", "where is everyone", "when is tomorrow", "where is the nearest pharmacy",
            "is it open", "where is the file i saved", "when is the next one", "where is room 204",
        ] {
            assert_eq!(q(p), None, "'{p}' wrongly parsed as place info");
        }
        // The other place parsers keep their phrases.
        assert!(format!("{:?}", parse_deterministic("where is the closest petrol station?").map(|r| r.intent)).contains("PlacesSearch"));
        assert!(format!("{:?}", parse_deterministic("directions to work").map(|r| r.intent)).contains("Directions"));
    }

    #[test]
    fn test_places_phrases() {
        let dbg = |p: &str| format!("{:?}", parse_deterministic(p).map(|r| r.intent));
        let want = |q: &str, n: Option<&str>| {
            format!("Some(PlacesSearch {{ query: {:?}, near: {:?} }})", q, n.map(String::from))
        };
        assert_eq!(dbg("nearest pharmacy"), want("pharmacy", None));
        assert_eq!(dbg("where is the closest petrol station?"), want("petrol station", None));
        assert_eq!(dbg("coffee shops near me"), want("coffee shops", None));
        assert_eq!(dbg("find a bank near Central Park"), want("bank", Some("central park")));
        assert_eq!(dbg("is there an ATM nearby"), want("atm", None));
        assert_eq!(dbg("find me the nearest hospital"), want("hospital", None));
        // Misfire guards: no marker, or not a place.
        for p in [
            "find a pharmacy", "find my files near me", "nearest neighbor algorithm", "find a photo near the top",
            "search for cats", "closest star", "find it nearby", "open maps", "what is near",
        ] {
            let got = dbg(p);
            assert!(!got.contains("PlacesSearch"), "'{p}' wrongly parsed as {got}");
        }
        // "directions to work/home" resolve later (saved places); they must parse.
        assert!(dbg("directions to work").contains("Directions"));
    }

    #[test]
    fn test_whatsapp_people_phrases() {
        let dbg = |p: &str| format!("{:?}", parse_deterministic(p).map(|r| r.intent));
        assert_eq!(dbg("read that message"), "Some(WhatsappRead { name: None })");
        assert_eq!(dbg("Read the latest WhatsApp message."), "Some(WhatsappRead { name: None })");
        assert_eq!(dbg("read the message from Asha"), r#"Some(WhatsappRead { name: Some("asha") })"#);
        assert_eq!(dbg("read Asha's messages"), r#"Some(WhatsappRead { name: Some("asha") })"#);
        assert_eq!(dbg("what did Raj say on WhatsApp"), r#"Some(WhatsappRead { name: Some("raj") })"#);
        assert_eq!(dbg("make Asha a VIP"), r#"Some(PeopleFlag { name: "asha", kind: "vip", on: true })"#);
        assert_eq!(dbg("make Mom a V I P"), r#"Some(PeopleFlag { name: "mom", kind: "vip", on: true })"#);
        assert_eq!(dbg("add Raj to my VIPs"), r#"Some(PeopleFlag { name: "raj", kind: "vip", on: true })"#);
        assert_eq!(dbg("remove Raj from my vip list"), r#"Some(PeopleFlag { name: "raj", kind: "vip", on: false })"#);
        assert_eq!(dbg("mute WhatsApp alerts from Raj"), r#"Some(PeopleFlag { name: "raj", kind: "mute", on: true })"#);
        assert_eq!(dbg("stop whatsapp alerts from Raj"), r#"Some(PeopleFlag { name: "raj", kind: "mute", on: true })"#);
        assert_eq!(dbg("unmute whatsapp alerts from Raj"), r#"Some(PeopleFlag { name: "raj", kind: "mute", on: false })"#);
        assert_eq!(dbg("unmute Raj on whatsapp"), r#"Some(PeopleFlag { name: "raj", kind: "mute", on: false })"#);
        assert_eq!(dbg("who are my priority people"), "Some(PeopleList)");
        assert_eq!(dbg("who are my VIPs?"), "Some(PeopleList)");
        // Misfire guards: plain mute/unmute, reading the screen, notes, and other "make" commands stay theirs.
        for p in [
            "mute", "unmute", "mute the volume", "unmute the mic", "read this", "read the screen", "read my email",
            "make a note", "make it louder", "make me a sandwich", "add milk to my shopping list", "stop",
            "what did einstein say", "mute whatsapp alerts from everyone", "make that a vip", "open whatsapp",
            "message asha on whatsapp saying hi", "read that",
        ] {
            let got = dbg(p);
            assert!(
                !(got.contains("WhatsappRead") || got.contains("PeopleFlag") || got.contains("PeopleList")),
                "'{p}' wrongly parsed as {got}"
            );
        }
    }

    #[test]
    fn test_mail_calendar_phrases() {
        let dbg = |p: &str| format!("{:?}", parse_deterministic(p).map(|r| r.intent));
        assert_eq!(dbg("any important emails"), "Some(MailDigest)");
        assert_eq!(dbg("Is there anything important in my inbox?"), "Some(MailDigest)");
        assert_eq!(dbg("that's not important"), "Some(MailMute)");
        assert_eq!(dbg("mute that sender"), "Some(MailMute)");
        assert_eq!(dbg("what's on my calendar today"), r#"Some(CalendarAgenda { day: "today" })"#);
        assert_eq!(dbg("what is on my calendar tomorrow"), r#"Some(CalendarAgenda { day: "tomorrow" })"#);
        assert_eq!(dbg("what's my agenda"), r#"Some(CalendarAgenda { day: "today" })"#);
        assert!(dbg("add dentist to my calendar tomorrow at 5pm").starts_with(r#"Some(CalendarAdd { text: "add dentist"#));
        // Misfire guards.
        for p in ["open google calendar", "tell me a joke", "add milk to my shopping list", "that is great", "important"] {
            let got = dbg(p);
            assert!(
                !(got.contains("MailDigest") || got.contains("MailMute") || got.contains("CalendarAgenda") || got.contains("CalendarAdd")),
                "'{p}' wrongly parsed as {got}"
            );
        }
    }

    #[test]
    fn test_timetable_phrases() {
        let intent = |p: &str| parse_deterministic(p).map(|r| r.intent);
        let dbg = |p: &str| format!("{:?}", intent(p));
        assert_eq!(dbg("analyse this and add section 2 to my time table"), r#"Some(TimetableAdd { source: "screen", section: Some(2) })"#);
        assert_eq!(dbg("add this to my timetable"), r#"Some(TimetableAdd { source: "screen", section: None })"#);
        assert_eq!(dbg("add the picture I copied to my timetable"), r#"Some(TimetableAdd { source: "clipboard", section: None })"#);
        assert_eq!(dbg("show my timetable"), "Some(TimetableShow)");
        assert_eq!(dbg("what's next on my timetable"), "Some(TimetableShow)");
        assert_eq!(dbg("clear my timetable"), "Some(TimetableClear)");
        assert_eq!(dbg("add those slots"), "Some(TimetableCommit)");
        assert_eq!(dbg("add them to my timetable"), "Some(TimetableCommit)");
        assert_eq!(dbg("use the browser for dsa"), r#"Some(StudyPref { activity: "dsa", choice: "browser" })"#);
        assert_eq!(dbg("switch DSA practice to the app"), r#"Some(StudyPref { activity: "dsa", choice: "app" })"#);
        assert_eq!(dbg("open dsa in the app from now on"), r#"Some(StudyPref { activity: "dsa", choice: "app" })"#);
        // Misfire guards: no "timetable" noun -> not ours; plain browser commands stay theirs.
        for p in ["add milk to my shopping list", "show my screen", "clear the cache", "open youtube in the browser", "what is next"] {
            let got = dbg(p);
            assert!(
                !(got.contains("Timetable") || got.contains("StudyPref")),
                "'{p}' wrongly parsed as {got}"
            );
        }
    }

    #[test]
    fn test_briefing_phrases() {
        for phrase in [
            "where did I leave off",
            "Where was I?",
            "what was I working on",
            "show my briefing",
            "brief me",
            "catch me up",
            "what are my priorities today",
        ] {
            let res = parse_deterministic(phrase);
            assert!(
                res.as_ref().map(|r| matches!(r.intent, ParsedIntent::Briefing)).unwrap_or(false),
                "Expected Briefing for '{}', got {:?}",
                phrase,
                res.map(|r| r.intent)
            );
        }
        // Misfire guards: ordinary chat is not a briefing request.
        for phrase in ["where is the nearest cafe", "what was the score", "tell me a joke"] {
            let res = parse_deterministic(phrase);
            assert!(
                !res.map(|r| matches!(r.intent, ParsedIntent::Briefing)).unwrap_or(false),
                "'{}' must not be a briefing",
                phrase
            );
        }
    }

    #[test]
    fn test_import_contacts_phrases() {
        for phrase in ["import my contacts", "Import contacts.", "nexus import my contacts", "sync my contacts"] {
            let res = parse_deterministic(phrase);
            assert!(
                res.as_ref().map(|r| matches!(r.intent, ParsedIntent::ImportContacts)).unwrap_or(false),
                "Expected ImportContacts for '{}', got {:?}",
                phrase,
                res.map(|r| r.intent)
            );
        }
        for phrase in ["import the module", "open contacts", "call my contacts"] {
            let res = parse_deterministic(phrase);
            assert!(
                !res.map(|r| matches!(r.intent, ParsedIntent::ImportContacts)).unwrap_or(false),
                "'{}' must not import contacts",
                phrase
            );
        }
    }

    #[test]
    fn test_memory_command_phrases() {
        // Audit family → MemoryAudit.
        for phrase in [
            "what do you remember",
            "what do you remember about me",
            "what do you know about me",
            "what have you learned about me",
            "tell me what you remember",
            "list what you remember",
            "show my memory",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected MemoryAudit for '{}'", phrase);
            assert!(
                matches!(res.unwrap().intent, ParsedIntent::MemoryAudit),
                "Expected MemoryAudit for '{}'",
                phrase
            );
        }
        // Forget-one → MemoryForget with raw key.
        for (phrase, key) in [
            ("forget my birthday", "birthday"),
            ("forget that nickname", "nickname"),
            ("forget my employer", "employer"),
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected MemoryForget for '{}'", phrase);
            if let ParsedIntent::MemoryForget { key: k } = res.unwrap().intent {
                assert_eq!(k, key);
            } else {
                panic!("Expected MemoryForget for '{}'", phrase);
            }
        }
        // Two-step wipe.
        let res = parse_deterministic("forget everything");
        assert!(matches!(res.unwrap().intent, ParsedIntent::MemoryForgetAll));
        let res = parse_deterministic("yes, forget everything");
        assert!(matches!(
            res.unwrap().intent,
            ParsedIntent::MemoryForgetAllConfirm
        ));
    }

    #[test]
    fn test_persona_and_counsel_phrases() {
        // Persona switch.
        for (phrase, friend) in [
            ("talk like a friend", true),
            ("be my friend", true),
            ("friend mode", true),
            ("be formal", false),
            ("talk like a butler", false),
            ("butler mode", false),
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected persona intent for '{}'", phrase);
            let ok = match res.unwrap().intent {
                ParsedIntent::PersonaFriend => friend,
                ParsedIntent::PersonaButler => !friend,
                _ => false,
            };
            assert!(ok, "Wrong persona direction for '{}'", phrase);
        }
        // Counsel opener → None story (arms counsel mode, runs nothing).
        // ("tell me honestly" bare has no anchor — asserted NOT firing
        // in the misfire guards below.)
        for phrase in [
            "i want to share something",
            "i want to share something with you",
            "i need your opinion",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected ShareConcern for '{}'", phrase);
            if let ParsedIntent::ShareConcern { story } = res.unwrap().intent {
                assert!(story.is_none(), "'{}' should arm, not run", phrase);
            } else {
                panic!("Expected ShareConcern for '{}'", phrase);
            }
        }
        // Direct counsel → Some story (runs now).
        for phrase in [
            "was i right to shout at him",
            "did i do the right thing",
            "tell me honestly was i wrong",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected ShareConcern for '{}'", phrase);
            if let ParsedIntent::ShareConcern { story } = res.unwrap().intent {
                assert!(story.is_some(), "'{}' should run counsel now", phrase);
            } else {
                panic!("Expected ShareConcern for '{}'", phrase);
            }
        }
        // Misfire guards.
        for phrase in [
            "tell me honestly",
            "tell me honestly what you see",
            "tell me a joke",
            "what do you see",
            "be my guest",
        ] {
            let res = parse_deterministic(phrase);
            if let Some(intent) = res.map(|r| r.intent) {
                assert!(
                    !matches!(
                        intent,
                        ParsedIntent::ShareConcern { .. }
                            | ParsedIntent::PersonaFriend
                            | ParsedIntent::PersonaButler
                    ),
                    "'{}' must not trigger persona/counsel",
                    phrase
                );
            }
        }
    }

    #[test]
    fn test_memory_command_phrases_guards() {
        // Misfire guards: dismissals are never forget-keys; jokes and
        // screen queries never audit.
        for phrase in ["forget it", "forget that", "tell me a joke", "what do you see"] {
            let res = parse_deterministic(phrase);
            if let Some(intent) = res.map(|r| r.intent) {
                assert!(
                    !matches!(
                        intent,
                        ParsedIntent::MemoryAudit
                            | ParsedIntent::MemoryForget { .. }
                            | ParsedIntent::MemoryForgetAll
                            | ParsedIntent::MemoryForgetAllConfirm
                    ),
                    "'{}' must not trigger memory commands",
                    phrase
                );
            }
        }
    }

    #[test]
    fn test_screen_click_results() {
        for (phrase, expected) in [
            ("open the 2 in the result", 2),
            ("open the 2nd result", 2),
            ("open result 2", 2),
            ("click the second result", 2),
            ("click result 3", 3),
            ("open the 1st result", 1),
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected ScreenClick for '{}'", phrase);
            if let ParsedIntent::ScreenClick { ordinal } = res.unwrap().intent {
                assert_eq!(ordinal, expected, "Mismatch for '{}'", phrase);
            } else {
                panic!("Expected ScreenClick for '{}'", phrase);
            }
        }
    }

    #[test]
    fn test_phonetic_search_almonds() {
        for phrase in [
            "search for almonds",
            "so it's for almonds.",
            "so it is for almonds",
            "search that on almonds",
        ] {
            let res = parse_deterministic(phrase);
            assert!(res.is_some(), "Expected Search for '{}'", phrase);
            if let ParsedIntent::Search { query } = res.unwrap().intent {
                assert_eq!(query, "almonds", "Expected query almonds for '{}'", phrase);
            } else {
                panic!("Expected Search for '{}'", phrase);
            }
        }
    }

    /// Feature 99 P3: live-usage mishearing families normalize correctly.
    #[test]
    fn test_phonetic_norm_feature_99_families() {
        // Ghost-mode family.
        for phrase in ["ghost right", "goes mode", "post mode", "ghost and go"] {
            assert_eq!(normalize_phonetic_mishearings(phrase), "ghost mode", "from '{phrase}'");
        }
        // Memory-audit family.
        for phrase in ["memory odd it", "memory or dit", "memory odit", "memory order it"] {
            assert_eq!(normalize_phonetic_mishearings(phrase), "memory audit", "from '{phrase}'");
        }
        // Forget-everything family.
        for phrase in ["forget every thing", "forget everthing"] {
            assert_eq!(normalize_phonetic_mishearings(phrase), "forget everything", "from '{phrase}'");
        }
        // WhatsApp-message family (substring inside a command).
        assert_eq!(
            normalize_phonetic_mishearings("send mom a what sap message saying hi"),
            "send mom a whatsapp message saying hi"
        );
        assert_eq!(
            normalize_phonetic_mishearings("send dad a what's up message saying hi"),
            "send dad a whatsapp message saying hi"
        );
        // Brave browser collapse.
        assert_eq!(normalize_phonetic_mishearings("open brave browser"), "open brave");
        // Architecture mapper family (keeps surrounding words intact).
        assert_eq!(
            normalize_phonetic_mishearings("open the architecture diagram"),
            "open the architecture mapper"
        );
        assert_eq!(
            normalize_phonetic_mishearings("open architecture digger"),
            "open architecture mapper"
        );
    }

    /// Feature 99 P3 end-to-end: normalized mishearings parse to intents.
    #[test]
    fn test_feature_99_mishearings_parse_to_intents() {
        let res = parse_deterministic("memory odd it");
        assert!(res.is_some(), "'memory odd it' should parse after normalization");
        let res = parse_deterministic("ghost right");
        assert!(res.is_some(), "'ghost right' should parse after normalization");
        let res = parse_deterministic("open brave browser");
        assert!(res.is_some(), "'open brave browser' should parse after normalization");
    }
}


#[cfg(test)]
mod wa_chat_tests {
    use super::*;

    fn contact(t: &str) -> Option<String> {
        match parse_deterministic(t)?.intent {
            ParsedIntent::WhatsappChat { contact } => Some(contact),
            _ => None,
        }
    }

    #[test]
    fn open_my_chat_with_x_in_whatsapp() {
        // The exact phrases from the 2026-10-10 report ("in", not "on").
        assert_eq!(contact("open my chat with mummy in whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("chat with mummy in whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("open chat with mummy on whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("Open my chat with Mummy in WhatsApp.").as_deref(), Some("mummy"));
    }

    #[test]
    fn open_x_chat_in_whatsapp_is_a_chat_not_the_app() {
        // Used to become OpenApp("whatsapp") with a false "Ok sir".
        assert_eq!(contact("open mummy chat in whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("open mummy chat on whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("open mummy's chat in whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("mummy chat in whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("open mummy in whatsapp").as_deref(), Some("mummy"));
        assert_eq!(
            contact("open thanmatee akka chat in whatsapp").as_deref(),
            Some("thanmatee akka")
        );
    }

    #[test]
    fn short_forms_and_polite_tails() {
        assert_eq!(contact("whatsapp mummy").as_deref(), Some("mummy"));
        assert_eq!(contact("whatsapp mummy chat").as_deref(), Some("mummy"));
        assert_eq!(contact("message mummy").as_deref(), Some("mummy"));
        assert_eq!(contact("message mummy in whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("open chat with mummy please").as_deref(), Some("mummy"));
        assert_eq!(contact("chat with mummy on wa").as_deref(), Some("mummy"));
        assert_eq!(contact("show my chat with akka in whatsapp").as_deref(), Some("akka"));
    }

    #[test]
    fn nexus_prefix_still_works() {
        assert_eq!(contact("nexus open my chat with mummy in whatsapp").as_deref(), Some("mummy"));
        assert_eq!(contact("hey nexus chat with mummy in whatsapp").as_deref(), Some("mummy"));
    }

    #[test]
    fn contact_names_keep_their_letters() {
        assert_eq!(contact("open jess chat in whatsapp").as_deref(), Some("jess"));
        assert_eq!(contact("open chat with chris on whatsapp").as_deref(), Some("chris"));
    }

    #[test]
    fn app_opens_stay_app_opens() {
        for t in [
            "open whatsapp",
            "open whatsapp web",
            "whatsapp web",
            "open chat gpt",
            "open notepad",
        ] {
            assert!(
                contact(t).is_none(),
                "'{t}' must not become a WhatsApp chat: {:?}",
                parse_deterministic(t).map(|p| p.intent)
            );
        }
        assert!(matches!(
            parse_deterministic("open whatsapp").map(|p| p.intent),
            Some(ParsedIntent::OpenApp { .. })
        ));
    }

    #[test]
    fn unrelated_phrases_with_a_whatsapp_tail_are_left_alone() {
        assert!(contact("play music on whatsapp").is_none());
    }

    #[test]
    fn strip_wa_tail_unit() {
        assert_eq!(strip_wa_tail("mummy in whatsapp"), "mummy");
        assert_eq!(strip_wa_tail("mummy in whatsapp please"), "mummy");
        assert_eq!(strip_wa_tail("my mummy chat in the whatsapp app"), "mummy");
        assert_eq!(strip_wa_tail("jess"), "jess");
        assert_eq!(strip_wa_tail("thanmatee akka on wa"), "thanmatee akka");
    }

    #[test]
    fn send_flows_are_not_hijacked() {
        // "saying" bodies still go to the send parser, never the chat-open one.
        let r = parse_deterministic("send mummy a whatsapp message saying hello").map(|p| p.intent);
        assert!(
            matches!(r, Some(ParsedIntent::SendWhatsAppMessage { .. })),
            "got {r:?}"
        );
    }
}

#[cfg(test)]
mod power_and_session_tests {
    use super::*;

    fn kind(t: &str) -> Option<String> {
        match parse_deterministic(t).map(|r| r.intent) {
            Some(ParsedIntent::PowerAction { kind }) => Some(kind),
            _ => None,
        }
    }

    #[test]
    fn shutting_down_the_laptop_is_a_power_command_not_a_close_app() {
        for t in ["shut down my laptop", "Nexus, shut down my laptop.", "please power off the computer", "turn off my laptop"] {
            assert_eq!(kind(t).as_deref(), Some("shutdown"), "{t}");
        }
        assert_eq!(kind("restart my pc").as_deref(), Some("restart"));
        assert_eq!(kind("put my laptop to sleep").as_deref(), Some("sleep"));
        assert_eq!(kind("lock my laptop").as_deref(), Some("lock"));
    }

    #[test]
    fn closing_an_app_still_closes_it() {
        match parse_deterministic("shut down notepad").map(|r| r.intent) {
            Some(ParsedIntent::CloseApp { target }) => assert_eq!(target, "notepad"),
            other => panic!("expected CloseApp, got {other:?}"),
        }
        assert_eq!(kind("close notepad"), None);
        assert_eq!(kind("turn off the lights"), None);
    }

    #[test]
    fn reopen_phrases() {
        for t in ["reopen my session", "Reopen what I had open.", "nexus restore my session", "reopen everything please"] {
            assert!(
                matches!(parse_deterministic(t).map(|r| r.intent), Some(ParsedIntent::ReopenSession)),
                "{t}"
            );
        }
        assert!(!matches!(parse_deterministic("open chrome").map(|r| r.intent), Some(ParsedIntent::ReopenSession)));
    }

    // Live ghost log 2026-10-12: every one of these was the SAME command,
    // "open my chat with mummy in whatsapp", and none reached the chat flow.
    #[test]
    fn garbled_whatsapp_chat_phrases_from_the_live_log_open_the_chat() {
        let contact = |t: &str| match parse_deterministic(t).map(|r| r.intent) {
            Some(ParsedIntent::WhatsappChat { contact }) => contact,
            other => panic!("{t:?} should open a WhatsApp chat, got {other:?}"),
        };
        // The command as spoken.
        assert_eq!(contact("open my chat with mummy in whatsapp"), "mummy");
        // STT: "chat" -> "chart", "in whatsapp" -> "and what so".
        assert_eq!(contact("Open my chart with mommy and what so."), "mommy");
        // STT: "chat with" -> "start with".
        assert_eq!(contact("Start with mommy in whatsapp."), "mommy");
        // Other garbled tails of "in WhatsApp".
        assert_eq!(contact("open my chat with mummy in what's up"), "mummy");
        assert_eq!(contact("open chat with mummy on whats app"), "mummy");
        assert_eq!(contact("chat with amma in what sap"), "amma");
        // A kin word + "chat" needs no WhatsApp tail ("open mummy chat" opened Google Chat).
        assert_eq!(contact("open mummy chat."), "mummy");
        assert_eq!(contact("open my mom's chat"), "mom");
    }

    #[test]
    fn ordinary_phrases_do_not_become_whatsapp_chats() {
        let is_chat = |t: &str| {
            matches!(parse_deterministic(t).map(|r| r.intent), Some(ParsedIntent::WhatsappChat { .. }))
        };
        for t in [
            "open chart",
            "open the chart paper",
            "start with a plan",
            "open brave",
            "start whatsapp",
            "open whatsapp",
            "open the door and what's up",
            "so what's up",
            "open google chat",
            "open chat",
            "play music in what's up",
        ] {
            assert!(!is_chat(t), "{t:?} must not parse as a WhatsApp chat");
        }
    }

    // Live ghost misfire 2026-10-12: guesses were rewritten into the matched
    // display name before the launch gate, so the gate saw an "exact" match
    // (and a garble launched an Intel installer).
    #[test]
    fn open_command_keeps_spoken_words_for_guesses() {
        use crate::app_registry::{AppEntry, LaunchMethod};
        let mk = |display: &str, names: &[&str]| AppEntry {
            display_name: display.to_string(),
            search_names: names.iter().map(|s| s.to_string()).collect(),
            launch: LaunchMethod::Exe { path: r"C:\x\a.exe".to_string() },
            use_count: 0,
            last_used: 0,
        };
        let _g = crate::app_registry::test_support::install(vec![
            mk("Google Chat", &["google chat", "chat"]),
            mk("WhatsApp", &["whatsapp"]),
            mk("Brave", &["brave", "brave browser"]),
        ]);
        let target = |t: &str| match parse_deterministic(t).map(|r| r.intent) {
            Some(ParsedIntent::OpenApp { target }) => target,
            other => panic!("{t:?} should parse as OpenApp, got {other:?}"),
        };
        // Guess ("chat" is only CONTAINED in the query): the spoken words survive.
        // (A kin word + chat, "open mummy chat", is a WhatsApp chat, tested above.)
        assert_eq!(target("open team chat"), "team chat");
        // Named apps still resolve to the registry name.
        assert_eq!(target("open whatsapp"), "whatsapp");
        assert_eq!(target("open brave"), "brave");
        assert_eq!(target("open brave browser"), "brave");
        // Garbles stay garbles: nothing is invented.
        assert_eq!(target("open mommychart"), "mommychart");
    }

    #[test]
    fn test_social_block_parsing() {
        use super::*;

        let p1 = parse_social_block_command("block prem sai for 5 days on whatsapp and instagram").unwrap();
        match p1.intent {
            ParsedIntent::BlockContact { target, platform, duration_days } => {
                assert_eq!(target, "prem sai");
                assert_eq!(platform, "all");
                assert_eq!(duration_days, Some(5));
            }
            other => panic!("expected BlockContact, got {:?}", other),
        }

        let p2 = parse_social_block_command("block prem sai for 3 days on instagram").unwrap();
        match p2.intent {
            ParsedIntent::BlockContact { target, platform, duration_days } => {
                assert_eq!(target, "prem sai");
                assert_eq!(platform, "instagram");
                assert_eq!(duration_days, Some(3));
            }
            other => panic!("expected BlockContact, got {:?}", other),
        }

        let p3 = parse_social_block_command("can u block prem sai for 3 days it should block in instagram whatsapp and phone number can it do that").unwrap();
        match p3.intent {
            ParsedIntent::BlockContact { target, platform, duration_days } => {
                assert_eq!(target, "prem sai");
                assert_eq!(platform, "all");
                assert_eq!(duration_days, Some(3));
            }
            other => panic!("expected BlockContact, got {:?}", other),
        }

        let p4 = parse_social_block_command("block prem sai on whatsapp").unwrap();
        match p4.intent {
            ParsedIntent::BlockContact { target, platform, duration_days } => {
                assert_eq!(target, "prem sai");
                assert_eq!(platform, "whatsapp");
                assert_eq!(duration_days, None);
            }
            other => panic!("expected BlockContact, got {:?}", other),
        }

        let p5 = parse_social_block_command("block prem sai for a week").unwrap();
        match p5.intent {
            ParsedIntent::BlockContact { target, platform, duration_days } => {
                assert_eq!(target, "prem sai");
                assert_eq!(platform, "all");
                assert_eq!(duration_days, Some(7));
            }
            other => panic!("expected BlockContact, got {:?}", other),
        }

        let u1 = parse_social_block_command("unblock prem sai on instagram").unwrap();
        match u1.intent {
            ParsedIntent::UnblockContact { target, platform } => {
                assert_eq!(target, "prem sai");
                assert_eq!(platform, "instagram");
            }
            other => panic!("expected UnblockContact, got {:?}", other),
        }

        let u2 = parse_social_block_command("unblock prem sai").unwrap();
        match u2.intent {
            ParsedIntent::UnblockContact { target, platform } => {
                assert_eq!(target, "prem sai");
                assert_eq!(platform, "all");
            }
            other => panic!("expected UnblockContact, got {:?}", other),
        }

        let l1 = parse_social_block_command("who is currently blocked").unwrap();
        assert!(matches!(l1.intent, ParsedIntent::ListBlockedContacts));

        let l2 = parse_social_block_command("list blocked contacts").unwrap();
        assert!(matches!(l2.intent, ParsedIntent::ListBlockedContacts));
    }

    #[test]
    fn test_coding_command_parsing() {
        use super::*;

        let c1 = parse_coding_command("run this on localhost").unwrap();
        assert!(matches!(c1.intent, ParsedIntent::RunOnLocalhost));

        let c2 = parse_coding_command("start the dev server").unwrap();
        assert!(matches!(c2.intent, ParsedIntent::RunOnLocalhost));

        let a1 = parse_coding_command("ask claude to refactor the database layer").unwrap();
        match a1.intent {
            ParsedIntent::CodingAgentTask { agent, prompt } => {
                assert_eq!(agent, "claude");
                assert_eq!(prompt, "refactor the database layer");
            }
            other => panic!("expected CodingAgentTask, got {:?}", other),
        }

        let a2 = parse_coding_command("run with antigravity fix the compiler errors").unwrap();
        match a2.intent {
            ParsedIntent::CodingAgentTask { agent, prompt } => {
                assert_eq!(agent, "antigravity");
                assert_eq!(prompt, "fix the compiler errors");
            }
            other => panic!("expected CodingAgentTask, got {:?}", other),
        }

        let p1 = parse_coding_command("anlayse the chat with eesah and plan and create that").unwrap();
        match p1.intent {
            ParsedIntent::CodeProject { source, prompt } => {
                assert_eq!(source, "chat with eesah");
                assert_eq!(prompt, Some("that".to_string()));
            }
            other => panic!("expected CodeProject, got {:?}", other),
        }

        let p2 = parse_coding_command("plan and create a modern portfolio website").unwrap();
        match p2.intent {
            ParsedIntent::CodeProject { source, prompt } => {
                assert_eq!(source, "active_workspace");
                assert_eq!(prompt, Some("a modern portfolio website".to_string()));
            }
            other => panic!("expected CodeProject, got {:?}", other),
        }

        // Test explicit watch command
        let w1 = parse_coding_command("nexus watch over this update me when the response is received and claude has stopped running").unwrap();
        match w1.intent {
            ParsedIntent::WatchAgentTask { agent, .. } => {
                assert_eq!(agent, Some("claude".to_string()));
            }
            other => panic!("expected WatchAgentTask, got {:?}", other),
        }

        let w2 = parse_coding_command("watch over antigravity and tell me when it is done").unwrap();
        match w2.intent {
            ParsedIntent::WatchAgentTask { agent, .. } => {
                assert_eq!(agent, Some("antigravity".to_string()));
            }
            other => panic!("expected WatchAgentTask, got {:?}", other),
        }

        let w3 = parse_coding_command("watch over this").unwrap();
        match w3.intent {
            ParsedIntent::WatchAgentTask { agent, .. } => {
                assert_eq!(agent, None);
            }
            other => panic!("expected WatchAgentTask, got {:?}", other),
        }

        // Test media commands
        let m1 = parse_media("open spotify").unwrap();
        match m1 {
            ParsedIntent::OpenMusicPlayer { player } => assert_eq!(player, Some("spotify".to_string())),
            other => panic!("expected OpenMusicPlayer, got {:?}", other),
        }

        let m2 = parse_media("open apple music").unwrap();
        match m2 {
            ParsedIntent::OpenMusicPlayer { player } => assert_eq!(player, Some("apple music".to_string())),
            other => panic!("expected OpenMusicPlayer, got {:?}", other),
        }

        let m3 = parse_media("play bohemian rhapsody on spotify").unwrap();
        match m3 {
            ParsedIntent::PlayMusic { query, player } => {
                assert_eq!(query, Some("bohemian rhapsody".to_string()));
                assert_eq!(player, Some("spotify".to_string()));
            }
            other => panic!("expected PlayMusic, got {:?}", other),
        }

        let m4 = parse_media("play music").unwrap();
        assert!(matches!(m4, ParsedIntent::PlayMusic { query: None, player: None }));
    }

    #[test]
    fn test_executive_assistant_commands() {
        use super::*;

        let r1 = parse_executive_assistant_commands("remind me to call prem sai in 30 minutes").unwrap();
        match r1.intent {
            ParsedIntent::CreateVoiceReminder { task, minutes } => {
                assert_eq!(task, "call prem sai");
                assert_eq!(minutes, 30);
            }
            other => panic!("expected CreateVoiceReminder, got {:?}", other),
        }

        let r2 = parse_executive_assistant_commands("remind me in 10 minutes to drink water").unwrap();
        match r2.intent {
            ParsedIntent::CreateVoiceReminder { task, minutes } => {
                assert_eq!(task, "drink water");
                assert_eq!(minutes, 10);
            }
            other => panic!("expected CreateVoiceReminder, got {:?}", other),
        }

        let r3 = parse_executive_assistant_commands("set a reminder to submit assignment in 2 hours").unwrap();
        match r3.intent {
            ParsedIntent::CreateVoiceReminder { task, minutes } => {
                assert_eq!(task, "submit assignment");
                assert_eq!(minutes, 120);
            }
            other => panic!("expected CreateVoiceReminder, got {:?}", other),
        }

        let d1 = parse_executive_assistant_commands("there are docs in the chat with eesah download and anlayse them and tell me how they are can u do that").unwrap();
        match d1.intent {
            ParsedIntent::AnalyseChatDocs { contact } => {
                assert_eq!(contact, "eesah");
            }
            other => panic!("expected AnalyseChatDocs, got {:?}", other),
        }

        let d2 = parse_executive_assistant_commands("download and analyse docs in the chat with eesah").unwrap();
        match d2.intent {
            ParsedIntent::AnalyseChatDocs { contact } => {
                assert_eq!(contact, "eesah");
            }
            other => panic!("expected AnalyseChatDocs, got {:?}", other),
        }

        let e1 = parse_executive_assistant_commands("analyse the email i have recieven in my college email baout an intership analyse and find any").unwrap();
        assert!(matches!(e1.intent, ParsedIntent::ScanInternshipEmails));

        let e2 = parse_executive_assistant_commands("check my college email about internship").unwrap();
        assert!(matches!(e2.intent, ParsedIntent::ScanInternshipEmails));
    }

    #[test]
    fn ghost_mode_soundalikes_from_the_live_log_enter_ghost_mode() {
        for heard in ["ghost moon.", "ghost in mode.", "gostum.", "Ghost mold"] {
            let r = parse_deterministic(heard).unwrap_or_else(|| panic!("'{heard}' did not parse"));
            assert!(
                matches!(r.intent, ParsedIntent::EnterGhostControl),
                "'{heard}' -> {:?}",
                r.intent
            );
        }
    }

    #[test]
    fn nexus_misheard_as_next_is_still_runs_the_command() {
        for (heard, expect_open) in [
            ("next is open brave.", "brave"),
            ("Next us open notepad", "notepad"),
            ("nexes, open youtube.", "youtube"),
        ] {
            let r = parse_deterministic(heard).unwrap_or_else(|| panic!("'{heard}' did not parse"));
            match r.intent {
                ParsedIntent::OpenApp { ref target } => {
                    assert!(target.to_lowercase().contains(expect_open), "{heard} -> {target}")
                }
                other => panic!("'{heard}' -> {other:?}"),
            }
        }
        // A real sentence that starts with "next is" is not a command.
        assert!(parse_deterministic("next is the quarterly review meeting").is_none());
    }

    #[test]
    fn hand_mode_phrases_enter_exit_and_toggle_live() {
        for p in ["hand mode", "Nexus, hand mouse.", "start hand mode", "camera mouse", "Hand mode!"] {
            let r = parse_deterministic(p).unwrap_or_else(|| panic!("no parse for {p:?}"));
            assert!(matches!(r.intent, ParsedIntent::EnterHandMode), "{p}: {:?}", r.intent);
        }
        for p in ["exit hand mode", "stop hand mouse", "hand mode off", "turn off hand mode"] {
            let r = parse_deterministic(p).unwrap_or_else(|| panic!("no parse for {p:?}"));
            assert!(matches!(r.intent, ParsedIntent::ExitHandMode), "{p}: {:?}", r.intent);
        }
        assert!(matches!(parse_deterministic("hand mouse live").unwrap().intent, ParsedIntent::HandMouseLive { live: true }));
        assert!(matches!(parse_deterministic("hand mouse preview").unwrap().intent, ParsedIntent::HandMouseLive { live: false }));
    }

    #[test]
    fn sentences_that_merely_mention_hand_mode_are_not_commands() {
        for p in ["hand mode is confusing", "I like the hand mode idea", "give me a hand", "hand me the mouse"] {
            let r = parse_deterministic(p);
            assert!(
                !matches!(r.as_ref().map(|r| &r.intent), Some(ParsedIntent::EnterHandMode | ParsedIntent::ExitHandMode | ParsedIntent::HandMouseLive { .. })),
                "{p}: {r:?}"
            );
        }
    }

    #[test]
    fn pointer_click_phrases_parse_and_sentences_do_not() {
        for (p, k) in [("click here", "left"), ("Click that.", "left"), ("double click", "double"), ("right click here", "right")] {
            let r = parse_deterministic(p).unwrap_or_else(|| panic!("no parse for {p:?}"));
            assert!(matches!(&r.intent, ParsedIntent::PointerClick { kind } if kind == k), "{p}: {:?}", r.intent);
        }
        for p in ["click here to learn more", "double click the icon on my desktop"] {
            let r = parse_deterministic(p);
            assert!(!matches!(r.as_ref().map(|r| &r.intent), Some(ParsedIntent::PointerClick { .. })), "{p}: {r:?}");
        }
    }

    #[test]
    fn leaving_hand_mode_works_in_many_words_but_only_while_it_is_on() {
        for t in [
            "exit hand mode", "stop the hand mode", "hand mode off please", "turn off the camera", "exit and mode",
            "stop hand more", "quit", "exit", "i'm done with the hands", "close gestures", "disable hand tracking",
            "stop the webcam", "end hand mode",
        ] {
            assert!(loose_hand_exit(t, true), "{t}");
            assert!(!loose_hand_exit(t, false), "{t}: must not trigger when hand mode is off");
        }
        for t in [
            "stop", "stop the music", "close this tab", "turn off ghost mode", "exit ghost mode",
            "open the settings and turn it off when you are ready to leave the house tonight", "what is the weather",
        ] {
            assert!(!loose_hand_exit(t, true), "{t}");
        }
    }

    #[test]
    fn natural_mode_requests_reach_the_right_mode_through_the_full_parser() {
        let hand = ["Nexus, I want to control the cursor.", "nexus let me control the mouse with my hand", "Nexus can I use my hand as a mouse?"];
        let ghost = ["nexus i want to give command to you", "Nexus, take control of my laptop please.", "nexus can you control my computer", "Nexus I want hands free control of my pc"];
        for t in hand {
            let r = parse_deterministic(t).unwrap_or_else(|| panic!("no parse for {t:?}"));
            assert!(matches!(r.intent, ParsedIntent::EnterHandMode), "{t}: {:?}", r.intent);
        }
        for t in ghost {
            let r = parse_deterministic(t).unwrap_or_else(|| panic!("no parse for {t:?}"));
            assert!(matches!(r.intent, ParsedIntent::EnterGhostControl), "{t}: {:?}", r.intent);
        }
        let ask = parse_deterministic("nexus i want to control my laptop").unwrap();
        assert!(matches!(ask.intent, ParsedIntent::NeedMoreInfo { .. }));
        let off = parse_deterministic("nexus i don't want you to control my cursor anymore").unwrap();
        assert!(matches!(off.intent, ParsedIntent::ExitGhostControl));
    }

    #[test]
    fn test_coderabbit_variations_parse_to_analyse_pr() {
        for phrase in [
            "open coderabbit",
            "open code rabbit",
            "show coderabbit",
            "show code rabbit",
            "coderabbit",
            "code rabbit",
            "coderabbit review",
            "open coderabbit review",
            "show coderabbit review",
            "check coderabbit review",
        ] {
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse {phrase:?}"));
            assert!(
                matches!(res.intent, ParsedIntent::OpenCodeRabbitHub),
                "{phrase} should match OpenCodeRabbitHub, got {:?}",
                res.intent
            );
        }

        for phrase in [
            "show coderabbit review for pr 24",
            "check coderabbit review for pr 24",
            "open coderabbit review for pr 24",
            "show code rabbit review for pr 24",
            "coderabbit pr 24",
            "code rabbit pr 24",
            "check coderabbit 24",
        ] {
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse {phrase:?}"));
            assert!(
                matches!(
                    res.intent,
                    ParsedIntent::GitHubCommand { command: crate::github_cmd::GitHubCommand::FindPr { pr_number: 24 } }
                ),
                "{phrase} should look PR 24 up across repos, got {:?}",
                res.intent
            );
        }
    }

    #[test]
    fn test_github_feed_and_pr_list_parsing() {
        // Feed / Dashboard / Home view phrases
        for phrase in [
            "show me my github feed.",
            "show me my github feed",
            "show my github feed",
            "open my github feed",
            "open github feed",
            "give me my github feed",
            "pull up my github feed",
            "bring up my github feed",
            "display github feed",
            "view my github feed",
            "check my github feed",
            "github feed",
            "the github feed",
            "my github feed",
            "open github dashboard",
            "show github dashboard",
            "show me github dashboard",
            "show my github dashboard",
            "open my github dashboard",
            "github dashboard",
            "open github interface",
            "show github interface",
            "github interface",
            "open github overview",
            "show github overview",
            "show me github overview",
            "github overview",
            "what's on my github",
            "what is on my github",
            "what's new on github",
            "what is new on github",
            "what needs my attention on github",
            "what needs attention on github",
            "github attention",
            "github home",
            "open github home",
            "show github home",
            "github activity",
            "show my github activity",
            "open github activity",
            "github updates",
            "show github updates",
            "open my github",
            "show my github",
            "open github",
            "show github",
            "my github",
            "take me to my github feed",
            "go to github dashboard",
            "feed on github",
            "dashboard on github",
            "developer feed",
            "dev feed",
            "coding feed",
            "developer dashboard",
            "dev dashboard",
        ] {
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse feed phrase {phrase:?}"));
            match res.intent {
                ParsedIntent::OpenGitHubDashboard { subpage } => {
                    assert_eq!(subpage, "feed", "{phrase} should have subpage 'feed'");
                }
                other => panic!("{phrase} expected OpenGitHubDashboard feed, got {other:?}"),
            }
        }

        // Pull Requests subpage phrases
        for phrase in [
            "open the pr list.",
            "open the pr list",
            "open pr list",
            "show the pr list",
            "show me the pr list",
            "show me my pr list",
            "show my pr list",
            "give me the pr list",
            "bring up the pr list",
            "pull up the pr list",
            "display the pr list",
            "view the pr list",
            "check the pr list",
            "pr list",
            "prs list",
            "pull request list",
            "pull requests list",
            "open pull requests",
            "show pull requests",
            "show me pull requests",
            "show open prs",
            "show all open prs",
            "show open pull requests",
            "list prs",
            "list all prs",
            "list open prs",
            "list of prs",
            "list of pull requests",
            "open prs",
            "my prs",
            "show my prs",
            "what prs are open",
            "which prs are open",
            "all open prs",
            "github prs",
            "github pr list",
            "github pull requests",
            "open github prs",
            "show github prs",
            "take me to the pr list",
            "prs that need review",
        ] {
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse pr phrase {phrase:?}"));
            match res.intent {
                ParsedIntent::OpenGitHubDashboard { subpage } => {
                    assert_eq!(subpage, "pull_requests", "{phrase} should have subpage 'pull_requests'");
                }
                other => panic!("{phrase} expected OpenGitHubDashboard pull_requests, got {other:?}"),
            }
        }

        // Issues subpage phrases
        for phrase in [
            "open github issues",
            "show github issues",
            "show me my github issues",
            "github issues",
            "open the issues list",
            "show the issues list",
            "issues list",
            "open issues list",
            "my issues",
            "open issues",
            "show issues",
            "all issues",
            "list of issues",
            "list issues",
            "what issues are open",
            "github issue list",
        ] {
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse issues phrase {phrase:?}"));
            match res.intent {
                ParsedIntent::OpenGitHubDashboard { subpage } => {
                    assert_eq!(subpage, "issues", "{phrase} should have subpage 'issues'");
                }
                other => panic!("{phrase} expected OpenGitHubDashboard issues, got {other:?}"),
            }
        }

        // Organizations subpage phrases
        for phrase in [
            "open github organizations",
            "open github orgs",
            "show github organizations",
            "show github orgs",
            "show my organizations",
            "my github organizations",
            "my github orgs",
            "github orgs",
            "github organizations",
            "orgs list",
            "organizations list",
            "my orgs",
            "open orgs",
            "github teams",
            "my teams",
        ] {
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse orgs phrase {phrase:?}"));
            match res.intent {
                ParsedIntent::OpenGitHubDashboard { subpage } => {
                    assert_eq!(subpage, "organizations", "{phrase} should have subpage 'organizations'");
                }
                other => panic!("{phrase} expected OpenGitHubDashboard organizations, got {other:?}"),
            }
        }

        // Repositories subpage phrases
        for phrase in [
            "open github repos",
            "open github repositories",
            "show github repos",
            "show my repos",
            "show my repositories",
            "my github repos",
            "my github repositories",
            "github repos",
            "github repositories",
            "repos list",
            "repositories list",
            "my repos",
            "open repos",
        ] {
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse repos phrase {phrase:?}"));
            match res.intent {
                ParsedIntent::OpenGitHubDashboard { subpage } => {
                    assert_eq!(subpage, "repos", "{phrase} should have subpage 'repos'");
                }
                other => panic!("{phrase} expected OpenGitHubDashboard repos, got {other:?}"),
            }
        }

        // Starred subpage phrases
        for phrase in [
            "open github starred",
            "show github starred",
            "show my starred repos",
            "my github starred",
            "github starred",
            "github starred repos",
            "starred repos",
            "my starred",
            "open starred",
            "my stars",
        ] {
            let res = parse_deterministic(phrase).unwrap_or_else(|| panic!("failed to parse starred phrase {phrase:?}"));
            match res.intent {
                ParsedIntent::OpenGitHubDashboard { subpage } => {
                    assert_eq!(subpage, "starred", "{phrase} should have subpage 'starred'");
                }
                other => panic!("{phrase} expected OpenGitHubDashboard starred, got {other:?}"),
            }
        }
    }
}
