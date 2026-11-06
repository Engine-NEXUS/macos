//! site_probe — a safe website technology scanner (spec_p4b.md, P4B)
//!
//! The probe is a MEDIUM-level scanner: it fetches the first HTML document of
//! the user's URL (read-only GET, ≤1.5 MB, whole-probe ≤20 s), follows at most
//! 5 redirects, fetches the first 6 <script src> (same-site only) plus the
//! first 2 <link rel=stylesheet> sub-resources (read-only GETs, ≤400 KB each,
//! 8 s timeout), collects cookie NAMES across all redirect hops, and pins DNS
//! per hop (every resolved address must be public, or loopback in tests).
//!
//! The result is built by analyse — a pure function; nothing else touches
//! the network — using the shipped `site_fingerprints.json` table (read-only
//! include_str!; never network, never filesystem, never app-data).
//!
//! Statuses from a per-signal weighted product:
//! Detected ≥ 0.80, Likely ≥ 0.50, Inferred ≥ 0.30; below 0.30 dropped.

use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderValue, SET_COOKIE,
};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

const MAX_HTML_BYTES: usize = 1_572_864;
const MAX_SUBRESOURCE_BYTES: usize = 409_600;
const MAX_SCRIPTS: usize = 6;
const MAX_STYLESHEETS: usize = 2;
const MAX_REDIRECTS: usize = 5;
const TOTAL_TIMEOUT_MS: u64 = 20_000;
const HOP_TIMEOUT_SECS: u64 = 10;
const CONNECT_TIMEOUT_SECS: u64 = 4;
const SUB_TIMEOUT_SECS: u64 = 8;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) NEXUS-StackScan/1.0";
const NOTE_TRUNCATED: &str = "page truncated at 1.5 MB";

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Detected,
    Likely,
    Inferred,
}

impl Status {
    fn from_conf(c: f32) -> Status {
        if c >= 0.80 {
            Status::Detected
        } else if c >= 0.50 {
            Status::Likely
        } else {
            Status::Inferred
        }
    }
}

// ---------------------------------------------------------------------------
// Fingerprint table (compiled once from a read-only JSON in the installer)
// ---------------------------------------------------------------------------

const FINGERPRINT_JSON: &str = include_str!("site_fingerprints.json");

#[derive(Deserialize)]
struct SignalJson {
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    contains: Option<String>,
    #[serde(default)]
    regex: Option<String>,
    #[serde(default)]
    weight: f32,
    evidence: String,
}

#[derive(Deserialize)]
struct TechJson {
    id: String,
    name: String,
    category: String,
    #[serde(default)]
    website: Option<String>,
    #[serde(default)]
    signals: Vec<SignalJson>,
    #[serde(default)]
    implies: Vec<String>,
}

#[derive(Deserialize)]
struct TableJson {
    #[serde(rename = "version", default)]
    _version: u8,
    technologies: Vec<TechJson>,
}

struct CompiledSignal {
    kind: &'static str,
    name: Option<String>,
    contains: Option<String>,
    regex: Option<Regex>,
    cookie_name: Option<String>,
    weight: f32,
    evidence: String,
    support_only: bool,
}

struct CompiledTech {
    id: String,
    name: String,
    category: String,
    website: Option<String>,
    signals: Vec<CompiledSignal>,
    implies: Vec<String>,
}

fn normalize_kind(k: &str) -> Option<&'static str> {
    match k {
        "script_text" | "generic" | "js" | "script" => Some("script_text"),
        "stylesheet" | "css_text" => Some("css_text"),
        "script_src" => Some("script_src"),
        "header" => Some("header"),
        "html" => Some("html"),
        "cookie" => Some("cookie"),
        "link_href" => Some("link_href"),
        "url" => Some("url"),
        "meta" => Some("meta"),
        _ => {
            tracing::warn!("site_probe: unknown signal kind in fingerprint table: {k}");
            None
        }
    }
}

static TABLE: Lazy<Vec<CompiledTech>> = Lazy::new(|| {
    match serde_json::from_str::<TableJson>(FINGERPRINT_JSON) {
        Ok(t) => t
            .technologies
            .into_iter()
            .map(|t| CompiledTech {
                id: t.id.clone(),
                name: t.name.clone(),
                category: t.category.clone(),
                website: t.website,
                signals: t
                    .signals
                    .iter()
                    .filter_map(|s| {
                        let kind = normalize_kind(&s.kind)?;
                        let regex = s.regex.as_ref().and_then(|r| {
                            match Regex::new(r) {
                                Ok(re) => Some(re),
                                Err(e) => {
                                    tracing::warn!("site_probe: bad regex '{r}': {e}");
                                    None
                                }
                            }
                        });
                        Some(CompiledSignal {
                            kind,
                            name: s.name.as_ref().map(|n| n.to_ascii_lowercase()),
                            contains: s.contains.as_ref().map(|c| c.to_ascii_lowercase()),
                            regex,
                            cookie_name: s.name.as_ref().map(|n| n.to_ascii_lowercase())
                                .filter(|_| s.kind == "cookie" && s.contains.is_none() && s.regex.is_none()),
                            weight: s.weight,
                            evidence: s.evidence.clone(),
                            support_only: s.weight < 0.30,
                        })
                    })
                    .collect(),
                implies: t.implies.clone(),
            })
            .collect(),
        Err(e) => {
            tracing::warn!("site_probe: failed to parse fingerprint table: {e}");
            Vec::new()
        }
    }
});

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Detection {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) website: Option<String>,
    pub(crate) confidence: f32,
    pub(crate) status: Status,
    pub(crate) evidence: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct SecurityHeaders {
    pub(crate) hsts: bool,
    pub(crate) csp: bool,
    pub(crate) xfo: bool,
    pub(crate) xcto: bool,
    pub(crate) referrer_policy: bool,
    pub(crate) permissions_policy: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct PageStats {
    pub(crate) html_bytes: usize,
    pub(crate) js_bytes: usize,
    pub(crate) css_bytes: usize,
    pub(crate) script_count: usize,
    pub(crate) inline_script_count: usize,
    pub(crate) stylesheet_count: usize,
    pub(crate) image_count: usize,
    pub(crate) third_party_hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SiteReport {
    pub(crate) url: String,
    pub(crate) final_url: String,
    pub(crate) status_code: u16,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) lang: String,
    pub(crate) detections: Vec<Detection>,
    pub(crate) security: SecurityHeaders,
    pub(crate) stats: PageStats,
    pub(crate) notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Probe refusals with a short spoken line
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) enum ProbeRefusal {
    BadUrl,
    NotHttp,
    PrivateAddress,
    Sensitive,
    NotHtml,
    TooManyRedirects,
    Timeout,
    Network(String),
}

impl ProbeRefusal {
    pub(crate) fn spoken(&self) -> String {
        match self {
            ProbeRefusal::BadUrl => "That does not look like a web address, sir.".into(),
            ProbeRefusal::NotHttp => "I can only open that over http or https, sir.".into(),
            ProbeRefusal::PrivateAddress => "I only scan sites on the public internet, sir.".into(),
            ProbeRefusal::Sensitive => "I do not scan private or banking sites, sir.".into(),
            ProbeRefusal::NotHtml => "That address did not serve an HTML page, sir.".into(),
            ProbeRefusal::TooManyRedirects => "That site redirected too many times, sir.".into(),
            ProbeRefusal::Timeout => "The scan took too long, sir.".into(),
            ProbeRefusal::Network(err) => format!("The site did not answer, sir — {err}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ProbeOpts {
    // Test-only: let 127.0.0.1 through so the mock server can be reached.
    pub(crate) allow_loopback: bool,
    pub(crate) total_timeout_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Confidence math
// ---------------------------------------------------------------------------

pub(crate) fn combine_confidence(weights: &[f32]) -> f32 {
    let prod: f32 = weights.iter().map(|w| (1.0 - *w).max(0.0)).product();
    (1.0 - prod).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// HTML regex helpers (lazy compiled regexes only — no DOM crate)
// ---------------------------------------------------------------------------

macro_rules! tag_regex {
    ($name:ident, $tag:literal) => {
        static $name: Lazy<Regex> =
            Lazy::new(|| Regex::new(concat!(r"(?is)<", $tag, r"\b[^>]*>")).unwrap());
    };
}

tag_regex!(RE_META_TAG, "meta");
tag_regex!(RE_SCRIPT_TAG, "script");
tag_regex!(RE_LINK_TAG, "link");
tag_regex!(RE_IMG_TAG, "img");
tag_regex!(RE_IFRAME_TAG, "iframe");
tag_regex!(RE_HTML_TAG, "html");

macro_rules! attr_regex {
    ($name:ident, $attr:literal) => {
        static $name: Lazy<Regex> = Lazy::new(|| {
            Regex::new(concat!(
                r#"(?i)\b"#,
                $attr,
                r#"\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#
            ))
            .unwrap()
        });
    };
}

attr_regex!(RE_ATTR_SRC, "src");
attr_regex!(RE_ATTR_HREF, "href");
attr_regex!(RE_ATTR_REL, "rel");
attr_regex!(RE_ATTR_NAME, "name");
attr_regex!(RE_ATTR_CONTENT, "content");
attr_regex!(RE_ATTR_LANG, "lang");

static RE_TITLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<title\b[^>]*>(.*?)</title>").unwrap());

static RE_ENTITY_MATCH: Lazy<Regex> =
    Lazy::new(|| Regex::new("(?i)&(amp|lt|gt|quot|apos|#39|nbsp);").unwrap());

fn decode_entity(name: &str) -> &str {
    match name.to_ascii_lowercase().as_str() {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" | "39" => "'",
        "nbsp" => " ",
        _ => "",
    }
}

fn decode_entities(text: &str) -> String {
    RE_ENTITY_MATCH
        .replace_all(text, |c: &Captures| decode_entity(&c[1]).to_string())
        .into_owned()
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out.trim().to_string()
}

fn attr_value(tag: &str, re: &Regex) -> Option<String> {
    let c = re.captures(tag)?;
    for g in c.iter().skip(1).flatten() {
        return Some(decode_entities(g.as_str()));
    }
    None
}

fn same_site(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.parse::<IpAddr>().is_ok() || b.parse::<IpAddr>().is_ok() {
        return false;
    }
    let key = |h: &str| -> Vec<String> {
        let labels: Vec<&str> = h.split('.').collect();
        let n = labels.len();
        if n >= 2 {
            vec![labels[n - 2].to_string(), labels[n - 1].to_string()]
        } else {
            labels.iter().map(|s| s.to_string()).collect()
        }
    };
    key(a) == key(b)
}

// ---------------------------------------------------------------------------
// analyse — pure function of (url, status, headers, html, scripts, styles)
// ---------------------------------------------------------------------------

struct MatchCtx {
    html_lower: String,
    headers: Vec<(String, String)>,
    cookie_names: Vec<String>,
    metas: Vec<(String, String)>,
    script_srcs: Vec<String>,
    link_hrefs: Vec<String>,
    scripts: Vec<String>,
    styles: Vec<String>,
    host: String,
}

fn value_matches(sig: &CompiledSignal, raw: &str) -> bool {
    if let Some(needle) = &sig.contains {
        return raw.to_ascii_lowercase().contains(needle);
    }
    if let Some(re) = &sig.regex {
        return re.is_match(raw);
    }
    true
}

fn signal_matches(sig: &CompiledSignal, ctx: &MatchCtx) -> bool {
    match sig.kind {
        "html" => {
            if let Some(needle) = &sig.contains {
                ctx.html_lower.contains(needle)
            } else if let Some(re) = &sig.regex {
                re.is_match(&ctx.html_lower)
            } else {
                false
            }
        }
        "header" => {
            if let Some(want) = &sig.name {
                ctx.headers
                    .iter()
                    .any(|(h, v)| h.eq_ignore_ascii_case(want) && value_matches(sig, v))
            } else {
                false
            }
        }
        "cookie" => ctx.cookie_names.iter().any(|c| {
            if let Some(n) = &sig.cookie_name {
                c.to_ascii_lowercase().starts_with(n)
            } else if let Some(sub) = &sig.contains {
                c.to_ascii_lowercase().contains(sub)
            } else if let Some(re) = &sig.regex {
                re.is_match(c)
            } else {
                false
            }
        }),
        "meta" => {
            if let Some(want) = &sig.name {
                ctx.metas
                    .iter()
                    .any(|(m, content)| m == want && value_matches(sig, content))
            } else {
                false
            }
        }
        "script_src" => ctx.script_srcs.iter().any(|u| value_matches(sig, u)),
        "link_href" => ctx.link_hrefs.iter().any(|u| value_matches(sig, u)),
        "script_text" => ctx.scripts.iter().any(|t| value_matches(sig, t)),
        "css_text" => ctx.styles.iter().any(|t| value_matches(sig, t)),
        "url" => value_matches(sig, &ctx.host),
        _ => false,
    }
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

fn push_unique_pair(list: &mut Vec<(String, String)>, name: String, value: String) {
    if !list.iter().any(|(n, v)| n == &name && v == &value) {
        list.push((name, value));
    }
}

pub(crate) fn analyse(
    url: &Url,
    status_code: u16,
    headers: &[(String, String)],
    html: &str,
    scripts: &[String],
    styles: &[String],
) -> SiteReport {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();

    let mut cookie_names: Vec<String> = Vec::new();
    for (h, v) in headers {
        if h.eq_ignore_ascii_case("set-cookie") {
            let name = v.split(';').next().unwrap_or("");
            let name = name.split('=').next().unwrap_or("").trim().to_string();
            if !name.is_empty() {
                push_unique(&mut cookie_names, name);
            }
        }
    }

    let mut metas: Vec<(String, String)> = Vec::new();
    let mut description = String::new();
    for m in RE_META_TAG.find_iter(html) {
        let tag = m.as_str();
        let name = attr_value(tag, &RE_ATTR_NAME)
            .map(|n| n.to_ascii_lowercase())
            .unwrap_or_default();
        let content = attr_value(tag, &RE_ATTR_CONTENT).unwrap_or_default();
        if !name.is_empty() {
            if name == "description" && description.is_empty() {
                description = content.clone();
            }
            metas.push((name, content));
        }
    }
    let html_lang = RE_HTML_TAG
        .find(html)
        .and_then(|m| attr_value(m.as_str(), &RE_ATTR_LANG))
        .unwrap_or_default();
    let title = RE_TITLE
        .captures(html)
        .map(|c| collapse_ws(&decode_entities(c.get(1).map(|t| t.as_str()).unwrap_or(""))))
        .unwrap_or_default();

    let mut script_count = 0usize;
    let mut inline_script_count = 0usize;
    let mut stylesheet_count = 0usize;
    let mut image_count = 0usize;
    let mut script_srcs: Vec<String> = Vec::new();
    let mut link_hrefs: Vec<String> = Vec::new();
    let mut extra_urls: Vec<String> = Vec::new();
    for m in RE_SCRIPT_TAG.find_iter(html) {
        script_count += 1;
        match attr_value(m.as_str(), &RE_ATTR_SRC) {
            Some(src) if !src.is_empty() => {
                script_srcs.push(src.clone());
                extra_urls.push(src);
            }
            _ => inline_script_count += 1,
        }
    }
    for m in RE_LINK_TAG.find_iter(html) {
        let tag = m.as_str();
        let rel = attr_value(tag, &RE_ATTR_REL)
            .map(|r| r.to_ascii_lowercase())
            .unwrap_or_default();
        if let Some(href) = attr_value(tag, &RE_ATTR_HREF) {
            if !href.is_empty() {
                link_hrefs.push(href.clone());
                extra_urls.push(href);
            }
        }
        if rel.contains("stylesheet") {
            stylesheet_count += 1;
        }
    }
    for m in RE_IMG_TAG.find_iter(html) {
        image_count += 1;
        if let Some(src) = attr_value(m.as_str(), &RE_ATTR_SRC) {
            if !src.is_empty() {
                extra_urls.push(src);
            }
        }
    }
    for m in RE_IFRAME_TAG.find_iter(html) {
        if let Some(src) = attr_value(m.as_str(), &RE_ATTR_SRC) {
            if !src.is_empty() {
                extra_urls.push(src);
            }
        }
    }

    let ctx = MatchCtx {
        html_lower: html.to_ascii_lowercase(),
        headers: headers.to_vec(),
        cookie_names,
        metas,
        script_srcs,
        link_hrefs,
        scripts: scripts.to_vec(),
        styles: styles.to_vec(),
        host: host.clone(),
    };

    let mut detections: Vec<Detection> = Vec::new();
    for tech in TABLE.iter() {
        let mut hits: Vec<(f32, &CompiledSignal)> = Vec::new();
        let mut support_evidence: Vec<String> = Vec::new();
        for sig in &tech.signals {
            if signal_matches(sig, &ctx) {
                if sig.support_only {
                    push_unique(&mut support_evidence, sig.evidence.clone());
                } else {
                    hits.push((sig.weight, sig));
                }
            }
        }
        let weights: Vec<f32> = hits.iter().map(|(w, _)| *w).collect();
        let confidence = combine_confidence(&weights);
        if confidence < 0.30 {
            continue;
        }
        let mut evidence: Vec<String> = hits.iter().map(|(_, s)| s.evidence.clone()).collect();
        for ev in support_evidence {
            push_unique(&mut evidence, ev);
        }
        detections.push(Detection {
            id: tech.id.clone(),
            name: tech.name.clone(),
            category: tech.category.clone(),
            website: tech.website.clone(),
            confidence,
            status: Status::from_conf(confidence),
            evidence,
        });
    }

    // first-level implies pass (confirmed detections only)
    detections.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    for det in detections.clone() {
        let Some(tech) = TABLE.iter().find(|t| t.id == det.id) else { continue };
        for imp_id in &tech.implies {
            if detections.iter().any(|d| &d.id == imp_id) {
                continue;
            }
            let Some(imp_tech) = TABLE.iter().find(|t| &t.id == imp_id) else { continue };
            let confidence = det.confidence * 0.8;
            detections.push(Detection {
                id: imp_id.clone(),
                name: imp_tech.name.clone(),
                category: imp_tech.category.clone(),
                website: imp_tech.website.clone(),
                confidence,
                status: Status::from_conf(confidence),
                evidence: vec![format!("implied by {}", tech.name)],
            });
        }
    }
    detections.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });

    let present = |name: &str| headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(name));
    let security = SecurityHeaders {
        hsts: present("strict-transport-security"),
        csp: present("content-security-policy"),
        xfo: present("x-frame-options"),
        xcto: present("x-content-type-options"),
        referrer_policy: present("referrer-policy"),
        permissions_policy: present("permissions-policy"),
    };

    let stats = PageStats {
        html_bytes: html.len(),
        js_bytes: scripts.iter().map(|s| s.len()).sum(),
        css_bytes: styles.iter().map(|s| s.len()).sum(),
        script_count,
        inline_script_count,
        stylesheet_count,
        image_count,
        third_party_hosts: third_party_hosts(url, &host, &extra_urls),
    };

    SiteReport {
        url: sanitized_url(url),
        final_url: sanitized_url(url),
        status_code,
        title,
        description,
        lang: html_lang,
        detections,
        security,
        stats,
        notes: Vec::new(),
    }
}

fn third_party_hosts(base: &Url, page_host: &str, raw_urls: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in raw_urls {
        let Ok(abs) = base.join(u) else { continue };
        let Some(h) = abs.host_str() else { continue };
        let hl = h.to_ascii_lowercase();
        if hl.is_empty() || hl == page_host || same_site(&hl, page_host) {
            continue;
        }
        if !out.contains(&hl) {
            out.push(hl);
        }
    }
    out.sort();
    out.truncate(30);
    out
}

fn sanitized_url(url: &Url) -> String {
    let mut out = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        out.push_str(&format!(":{port}"));
    }
    out.push_str(url.path());
    out
}

// ---------------------------------------------------------------------------
// group_by_category — detections grouped in the fixed panel order
// ---------------------------------------------------------------------------

const CATEGORY_ORDER: &[&str] = &[
    "meta-framework",
    "frontend-framework",
    "ui-library",
    "css-framework",
    "animation",
    "state-management",
    "build-tool",
    "cms",
    "headless-cms",
    "ecommerce",
    "hosting",
    "cdn",
    "web-server",
    "backend-framework",
    "backend-language",
    "database-baas",
    "auth",
    "payments",
    "analytics",
    "tag-manager",
    "advertising",
    "search",
    "fonts",
    "maps",
    "video",
    "chat-support",
    "monitoring",
    "security",
    "ab-testing",
    "email-marketing",
    "cookie-consent",
    "icons",
    "programming-language",
];

fn category_label(category: &str) -> String {
    match category {
        "meta-framework" => "Meta-Framework".into(),
        "frontend-framework" => "Frontend Framework".into(),
        "ui-library" => "UI Library".into(),
        "css-framework" => "CSS Framework".into(),
        "animation" => "Animation".into(),
        "state-management" => "State Management".into(),
        "build-tool" => "Build Tool".into(),
        "cms" => "CMS".into(),
        "headless-cms" => "Headless CMS".into(),
        "ecommerce" => "E-commerce".into(),
        "hosting" => "Hosting".into(),
        "cdn" => "CDN".into(),
        "web-server" => "Web Server".into(),
        "backend-framework" => "Backend Framework".into(),
        "backend-language" => "Backend Language".into(),
        "database-baas" => "Database / BaaS".into(),
        "auth" => "Auth".into(),
        "payments" => "Payments".into(),
        "analytics" => "Analytics".into(),
        "tag-manager" => "Tag Manager".into(),
        "advertising" => "Advertising".into(),
        "search" => "Search".into(),
        "fonts" => "Fonts".into(),
        "maps" => "Maps".into(),
        "video" => "Video".into(),
        "chat-support" => "Chat & Support".into(),
        "monitoring" => "Monitoring".into(),
        "security" => "Security".into(),
        "ab-testing" => "A/B Testing".into(),
        "email-marketing" => "Email Marketing".into(),
        "cookie-consent" => "Cookie Consent".into(),
        "icons" => "Icons".into(),
        "programming-language" => "Programming Language".into(),
        other => {
            let mut out = String::new();
            for (i, ch) in other.chars().enumerate() {
                if ch == '_' {
                    out.push(' ');
                } else if i == 0 {
                    out.extend(ch.to_uppercase());
                } else {
                    out.push(ch);
                }
            }
            out
        }
    }
}

pub(crate) fn group_by_category(report: &SiteReport) -> Vec<(String, Vec<&Detection>)> {
    let rank = |cat: &str| -> usize {
        CATEGORY_ORDER
            .iter()
            .position(|c| c == &cat)
            .unwrap_or(usize::MAX)
    };
    let mut groups: Vec<(usize, String, Vec<&Detection>)> = Vec::new();
    for det in &report.detections {
        let r = rank(&det.category);
        match groups.iter_mut().find(|(rr, _, _)| *rr == r) {
            Some((_, _, dets)) => dets.push(det),
            None => groups.push((r, category_label(&det.category), vec![det])),
        }
    }
    groups.sort_by_key(|(r, _, _)| *r);
    groups
        .into_iter()
        .map(|(_, label, dets)| (label, dets))
        .collect()
}

// ---------------------------------------------------------------------------
// report_markdown
// ---------------------------------------------------------------------------

pub(crate) fn report_markdown(title: &str, report: &SiteReport) -> String {
    let head = if report.title.is_empty() {
        title.to_string()
    } else {
        format!("{} — {}", title, report.title)
    };
    let mut out = format!("## {head}\n\n");
    out.push_str(&format!(
        "### URL: {}\nStatus: {}\n\n",
        report.url, report.status_code
    ));
    if report.detections.is_empty() {
        out.push_str("Not detectable from outside.\n");
    } else {
        for (category, dets) in group_by_category(report) {
            out.push_str(&format!("### {category}\n"));
            for det in dets {
                let word = match det.status {
                    Status::Detected => "Detected",
                    Status::Likely => "Likely",
                    Status::Inferred => "Inferred",
                };
                out.push_str(&format!(
                    "- **{}** ({}, {:.0}%)\n",
                    det.name, word, det.confidence * 100.0
                ));
                for ev in &det.evidence {
                    out.push_str(&format!("  - {ev}\n"));
                }
            }
        }
    }
    let sp = &report.stats;
    out.push_str(&format!(
        "\n### Page facts\n- script tags: {} ({} inline)\n- stylesheets: {}\n- images: {}\n- html: {}\n- js analysed: {}\n- css analysed: {}\n- third-party hosts: {}\n",
        sp.script_count,
        sp.inline_script_count,
        sp.stylesheet_count,
        sp.image_count,
        fmt_bytes(sp.html_bytes),
        fmt_bytes(sp.js_bytes),
        fmt_bytes(sp.css_bytes),
        if sp.third_party_hosts.is_empty() {
            "none".to_string()
        } else {
            sp.third_party_hosts.join(", ")
        }
    ));
    let sec = &report.security;
    let yes_no = |b: bool| if b { "yes" } else { "no" };
    out.push_str(&format!(
        "\n### Security headers\n- HSTS: {}\n- CSP: {}\n- X-Frame-Options: {}\n- X-Content-Type-Options: {}\n- Referrer-Policy: {}\n- Permissions-Policy: {}\n",
        yes_no(sec.hsts),
        yes_no(sec.csp),
        yes_no(sec.xfo),
        yes_no(sec.xcto),
        yes_no(sec.referrer_policy),
        yes_no(sec.permissions_policy),
    ));
    if !report.notes.is_empty() {
        out.push_str("\n### Notes\n");
        for n in &report.notes {
            out.push_str(&format!("- {n}\n"));
        }
    }
    out
}

fn fmt_bytes(n: usize) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

// ---------------------------------------------------------------------------
// ip_is_public (v4 + v6; used by guard and DNS-pinning)
// ---------------------------------------------------------------------------

pub(crate) fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_is_public(v4),
        IpAddr::V6(v6) => v6_is_public(v6),
    }
}

fn v4_is_public(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    !(v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_multicast()
        || v4.is_unspecified()
        // 0.0.0.0/8 this network
        || o[0] == 0
        // 100.64.0.0/10 CGNAT
        || (o[0] == 100 && (o[1] & 0xC0) == 64)
        // 192.0.0.0/24 IETF protocol assignments
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        // 198.18.0.0/15 benchmarking
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        // 240.0.0.0/4 reserved
        || o[0] >= 240)
}

fn v6_is_public(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() {
        return false;
    }
    let s = v6.segments();
    // ::ffff:x.y.z.w mapped and ::x.y.z.w compatible embedded IPv4
    if s[..5].iter().all(|x| *x == 0) && s[5] == 0xffff {
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return v4_is_public(v4);
    }
    if s[..6].iter().all(|x| *x == 0) {
        // ::a.b.c.d compatible form (excluding :: and ::1 refused above)
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return v4_is_public(v4);
    }
    // NAT64 64:ff9b::/96 embeds a plain IPv4 host
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6].iter().all(|x| *x == 0) {
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return v4_is_public(v4);
    }
    // ff00::/8 multicast
    if (s[0] & 0xff00) == 0xff00 {
        return false;
    }
    // fc00::/7 unique-local
    if (s[0] & 0xfe00) == 0xfc00 {
        return false;
    }
    // fe80::/10 link-local
    if (s[0] & 0xffc0) == 0xfe80 {
        return false;
    }
    // 2001:db8::/32 documentation
    if s[0] == 0x2001 && s[1] == 0x0db8 {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// URL guard (spec table) — strict public helper, relaxed seam for probe/tests
// ---------------------------------------------------------------------------

fn guard_url_relaxed(raw: &str, allow_loopback: bool) -> Result<Url, ProbeRefusal> {
    let mut url = Url::parse(raw).map_err(|_| ProbeRefusal::BadUrl)?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ProbeRefusal::NotHttp);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ProbeRefusal::BadUrl);
    }
    let host_raw = url.host_str().ok_or(ProbeRefusal::BadUrl)?.to_ascii_lowercase();
    let host = if host_raw.starts_with('[') && host_raw.ends_with(']') {
        host_raw[1..host_raw.len() - 1].to_string()
    } else {
        host_raw
    };
    let banned = host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
        || host.ends_with(".home.arpa");
    if banned {
        return Err(ProbeRefusal::PrivateAddress);
    }
    let ip_check = |ip: IpAddr| -> bool {
        ip_is_public(ip) || (allow_loopback && ip.is_loopback())
    };
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        if !ip_check(IpAddr::V4(v4)) {
            return Err(ProbeRefusal::PrivateAddress);
        }
    } else if let Ok(v6) = host.parse::<Ipv6Addr>() {
        if !ip_check(IpAddr::V6(v6)) {
            return Err(ProbeRefusal::PrivateAddress);
        }
    }
    if crate::screen_context::is_sensitive("", "", Some(url.as_str())) {
        return Err(ProbeRefusal::Sensitive);
    }
    url.set_fragment(None);
    Ok(url)
}

#[cfg(test)]
pub(crate) fn guard_url(raw: &str) -> Result<Url, ProbeRefusal> {
    guard_url_relaxed(raw, false)
}

// ---------------------------------------------------------------------------
// probe — DNS-pinned network fetch + analyse
// ---------------------------------------------------------------------------

async fn resolve_and_check(url: &Url, allow_loopback: bool) -> Result<IpAddr, ProbeRefusal> {
    let host_raw = url.host_str().ok_or(ProbeRefusal::BadUrl)?;
    let host = host_raw.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = tokio::net::lookup_host((host.clone(), port))
        .await
        .map_err(|e| ProbeRefusal::Network(format!("DNS lookup failed: {e}")))?;
    let mut chosen: Option<IpAddr> = None;
    for a in addrs {
        let ip = a.ip();
        if !(ip_is_public(ip) || (allow_loopback && ip.is_loopback())) {
            return Err(ProbeRefusal::PrivateAddress);
        }
        if chosen.is_none() {
            chosen = Some(ip);
        }
    }
    chosen.ok_or_else(|| ProbeRefusal::Network("DNS lookup returned no addresses".into()))
}

fn client_for(url: &Url, pinned: IpAddr) -> Result<Client, ProbeRefusal> {
    let host_raw = url.host_str().ok_or(ProbeRefusal::BadUrl)?;
    let host = host_raw.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(HOP_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .resolve(
            &host,
            SocketAddr::new(pinned, if port == 0 { 80 } else { port }),
        )
        .build()
        .map_err(|e| ProbeRefusal::Network(format!("client build failed: {e}")))
}

async fn send_get(url: &Url, allow_loopback: bool) -> Result<reqwest::Response, ProbeRefusal> {
    let ip = resolve_and_check(url, allow_loopback).await?;
    let client = client_for(url, ip)?;
    client
        .get(url.as_str())
        .header(ACCEPT, "text/html,application/xhtml+xml,*/*;q=0.8")
        .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"))
        .send()
        .await
        .map_err(|e| ProbeRefusal::Network(format!("request failed: {e}")))
}

fn headers_to_vec(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect()
}

fn looks_like_html(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(h, v)| h.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().contains("html"))
}

fn looks_like_subresource_type(headers: &HeaderMap, is_script: bool) -> bool {
    let ct = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ct.is_empty() {
        return true;
    }
    if is_script {
        ct.contains("javascript") || ct.contains("ecmascript")
    } else {
        ct.contains("css")
    }
}

fn collect_cookie_names(headers: &HeaderMap, out: &mut Vec<String>) {
    for v in headers.get_all(SET_COOKIE) {
        let Ok(s) = v.to_str() else { continue };
        let name = s.split(';').next().unwrap_or("");
        let name = name.split('=').next().unwrap_or("").trim().to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
}

async fn stream_capped(
    mut resp: reqwest::Response,
    cap: usize,
) -> Result<(Vec<u8>, bool), ProbeRefusal> {
    let mut body: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| ProbeRefusal::Network(format!("read body failed: {e}")))?
    {
        if body.len() + chunk.len() > cap {
            let room = cap.saturating_sub(body.len());
            body.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
        if body.len() >= cap {
            break;
        }
    }
    Ok((body, truncated))
}

async fn fetch_subresource(url: &Url, is_script: bool, allow_loopback: bool) -> Option<(String, String)> {
    let res = tokio::time::timeout(Duration::from_secs(SUB_TIMEOUT_SECS), async move {
        let ip = resolve_and_check(url, allow_loopback).await.ok()?;
        let client = client_for(url, ip).ok()?;
        let resp = client
            .get(url.as_str())
            .header(ACCEPT, "*/*")
            .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"))
            .send()
            .await
            .ok()?;
        if !looks_like_subresource_type(resp.headers(), is_script) {
            return None;
        }
        let ct = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let (body, _trunc) = stream_capped(resp, MAX_SUBRESOURCE_BYTES).await.ok()?;
        Some((ct, String::from_utf8_lossy(&body).into_owned()))
    })
    .await
    .ok()?;
    res
}

fn pick_subresources(
    html: &str,
    base: &Url,
    allow_loopback: bool,
    skipped: &mut usize,
) -> Vec<(Url, bool)> {
    let page_host = base.host_str().unwrap_or_default().to_ascii_lowercase();
    let mut picks: Vec<(Url, bool)> = Vec::new();
    for m in RE_SCRIPT_TAG.find_iter(html) {
        if picks.len() >= MAX_SCRIPTS {
            break;
        }
        let Some(src) = attr_value(m.as_str(), &RE_ATTR_SRC) else { continue };
        if src.is_empty() {
            continue;
        }
        let Ok(abs) = base.join(&src) else { continue };
        let host = abs.host_str().unwrap_or_default().to_ascii_lowercase();
        if host.is_empty() || !(host == page_host || same_site(&host, &page_host)) {
            continue;
        }
        if guard_url_relaxed(abs.as_str(), allow_loopback).is_err() {
            *skipped += 1;
            continue;
        }
        picks.push((abs, true));
    }
    for m in RE_LINK_TAG.find_iter(html) {
        if picks.len() >= MAX_SCRIPTS + MAX_STYLESHEETS {
            break;
        }
        let tag = m.as_str();
        let rel = attr_value(tag, &RE_ATTR_REL)
            .map(|r| r.to_ascii_lowercase())
            .unwrap_or_default();
        if !rel.contains("stylesheet") {
            continue;
        }
        let Some(href) = attr_value(tag, &RE_ATTR_HREF) else { continue };
        if href.is_empty() {
            continue;
        }
        let Ok(abs) = base.join(&href) else { continue };
        if guard_url_relaxed(abs.as_str(), allow_loopback).is_err() {
            *skipped += 1;
            continue;
        }
        picks.push((abs, false));
    }
    picks
}

async fn fetch_and_analyse(raw: &str, opts: ProbeOpts) -> Result<SiteReport, ProbeRefusal> {
    let initial = guard_url_relaxed(raw, opts.allow_loopback)?;
    let mut current = initial.clone();
    let mut redirects = 0usize;
    let mut hop_cookies: Vec<String> = Vec::new();

    let (status_code, resp_headers, body, truncated) = loop {
        let resp = send_get(&current, opts.allow_loopback).await?;
        collect_cookie_names(resp.headers(), &mut hop_cookies);
        let hvec = headers_to_vec(resp.headers());
        if resp.status().is_redirection() {
            if redirects >= MAX_REDIRECTS {
                return Err(ProbeRefusal::TooManyRedirects);
            }
            let loc = hvec
                .iter()
                .find(|(h, _)| h.eq_ignore_ascii_case("location"))
                .map(|(_, v)| v.clone())
                .ok_or_else(|| ProbeRefusal::Network("redirect without location".into()))?;
            redirects += 1;
            let next = current
                .join(&loc)
                .map_err(|_| ProbeRefusal::Network("bad redirect location".into()))?;
            drop(resp);
            current = guard_url_relaxed(next.as_str(), opts.allow_loopback)?;
            continue;
        }
        if !looks_like_html(&hvec) {
            return Err(ProbeRefusal::NotHtml);
        }
        let status = resp.status().as_u16();
        let (body, truncated) = stream_capped(resp, MAX_HTML_BYTES).await?;
        break (status, hvec, body, truncated);
    };

    let html = String::from_utf8_lossy(&body).into_owned();
    let mut skipped = 0usize;
    let picks = pick_subresources(&html, &current, opts.allow_loopback, &mut skipped);

    let fetched: Vec<Option<(String, String)>> = futures_util::future::join_all(
        picks
            .iter()
            .map(|(url, is_script)| fetch_subresource(url, *is_script, opts.allow_loopback)),
    )
    .await;

    let mut scripts = Vec::new();
    let mut styles = Vec::new();
    for (res, (_url, is_script)) in fetched.into_iter().zip(picks.iter()) {
        match res {
            Some((ct, text)) => {
                let ok = if *is_script {
                    ct.is_empty() || ct.contains("javascript") || ct.contains("ecmascript")
                } else {
                    ct.is_empty() || ct.contains("css")
                };
                if ok {
                    if *is_script {
                        scripts.push(text);
                    } else {
                        styles.push(text);
                    }
                } else {
                    skipped += 1;
                }
            }
            None => skipped += 1,
        }
    }

    let mut merged = resp_headers;
    for name in &hop_cookies {
        push_unique_pair(&mut merged, "set-cookie".into(), format!("{name}="));
    }

    let mut report = analyse(&current, status_code, &merged, &html, &scripts, &styles);
    if truncated {
        report.notes.push(NOTE_TRUNCATED.into());
    }
    if skipped > 0 {
        report.notes.push(format!("{skipped} sub-resource(s) skipped"));
    }
    report.url = sanitized_url(&initial);
    Ok(report)
}

pub(crate) async fn probe_with(raw_url: &str, opts: ProbeOpts) -> Result<SiteReport, ProbeRefusal> {
    let total = Duration::from_millis(opts.total_timeout_ms.unwrap_or(TOTAL_TIMEOUT_MS));
    match tokio::time::timeout(total, fetch_and_analyse(raw_url, opts)).await {
        Ok(inner) => inner,
        Err(_) => Err(ProbeRefusal::Timeout),
    }
}

pub(crate) async fn probe(raw_url: &str) -> Result<SiteReport, ProbeRefusal> {
    probe_with(raw_url, ProbeOpts::default()).await
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(r: &'a SiteReport, id: &str) -> Option<&'a Detection> {
        r.detections.iter().find(|d| d.id == id)
    }

    fn det(id: &str, name: &str, category: &str, conf: f32) -> Detection {
        Detection {
            id: id.into(),
            name: name.into(),
            category: category.into(),
            website: None,
            confidence: conf,
            status: Status::from_conf(conf),
            evidence: vec!["test evidence".into()],
        }
    }

    // ----------------------------------------------------------------
    // combine_confidence
    // ----------------------------------------------------------------

    #[test]
    fn combine_confidence_edge_cases() {
        assert_eq!(combine_confidence(&[]), 0.0);
        let s = combine_confidence(&[0.5]);
        assert!((s - 0.5).abs() < 0.005);
        let t = combine_confidence(&[0.5, 0.5]);
        assert!((t - 0.75).abs() < 0.005);
        let c = combine_confidence(&[1.1]);
        assert!((c - 1.0).abs() < 1e-2, "saturated product must clamp: {c}");
    }

    // ----------------------------------------------------------------
    // ip_is_public
    // ----------------------------------------------------------------

    #[test]
    fn ip_is_public_v4_v6_tables() {
        let refused_v4 = [
            "127.0.0.1", "10.0.0.5", "172.16.0.9", "172.31.255.1", "192.168.1.1",
            "169.254.1.2", "255.255.255.255", "192.0.2.1", "198.51.100.7", "203.0.113.9",
            "224.0.0.5", "100.64.0.1", "100.127.255.1", "192.0.0.8",
            "198.18.0.1", "198.19.255.4", "240.0.0.1", "250.1.2.3", "0.0.0.0", "0.1.2.3",
        ];
        for a in refused_v4 {
            let ip: IpAddr = a.parse().unwrap();
            assert!(!ip_is_public(ip), "must refuse v4 {a}");
        }
        for a in ["8.8.8.8", "1.1.1.1", "151.101.1.140", "9.9.9.9", "100.128.0.1"] {
            let ip: IpAddr = a.parse().unwrap();
            assert!(ip_is_public(ip), "must allow v4 {a}");
        }
        let refused_v6 = [
            "::1", "::", "fe80::a1", "febf::1", "fc00::1", "fd12:3456::a",
            "ff02::1", "2001:db8::5", "::ffff:127.0.0.1",
        ];
        for a in refused_v6 {
            let ip: IpAddr = a.parse().unwrap();
            assert!(!ip_is_public(ip), "must refuse v6 {a}");
        }
        for a in ["2606:4700::1111", "2001:4860:4860::8888"] {
            let ip: IpAddr = a.parse().unwrap();
            assert!(ip_is_public(ip), "must allow v6 {a}");
        }
    }

    #[test]
    fn ip_mapped_zero_and_documentation_refused() {
        let ip: IpAddr = "::ffff:0.0.0.0".parse().unwrap();
        assert!(!ip_is_public(ip));
        let ip: IpAddr = "::ffff:192.0.2.9".parse().unwrap();
        assert!(!ip_is_public(ip), "documentation v4 host via a mapped v6 socket");
    }

    // ----------------------------------------------------------------
    // guard_url (spec refusals table)
    // ----------------------------------------------------------------

    #[test]
    fn guard_url_spec_table() {
        assert!(matches!(guard_url("file:///c:/x"), Err(ProbeRefusal::NotHttp)));
        assert!(matches!(guard_url("ftp://example.com"), Err(ProbeRefusal::NotHttp)));
        assert!(matches!(guard_url("not a url"), Err(ProbeRefusal::BadUrl)));
        assert!(matches!(guard_url("http://user:pw@example.com/x"), Err(ProbeRefusal::BadUrl)));
        assert!(matches!(guard_url("http://localhost:3000/"), Err(ProbeRefusal::PrivateAddress)));
        assert!(matches!(guard_url("http://127.0.0.1:8080/admin"), Err(ProbeRefusal::PrivateAddress)));
        assert!(matches!(guard_url("http://192.168.1.14/topology"), Err(ProbeRefusal::PrivateAddress)));
        assert!(matches!(guard_url("http://router.lan/"), Err(ProbeRefusal::PrivateAddress)));
        assert!(matches!(guard_url("https://www.paypal.com/signin"), Err(ProbeRefusal::Sensitive)));
        let guarded = guard_url("https://example.com/a?token=SECRET#frag");
        assert!(guarded.is_ok());
        let guarded = guarded.unwrap();
        assert_eq!(guarded.fragment(), None, "fragment must be stripped");
        assert_eq!(guarded.query(), Some("token=SECRET"), "query must be kept");
    }

    #[test]
    fn guard_url_whatwg_ip_normalization() {
        assert!(matches!(guard_url("http://2130706433/"), Err(ProbeRefusal::PrivateAddress)));
        assert!(matches!(guard_url("http://0x7f.1/"), Err(ProbeRefusal::PrivateAddress)));
    }

    #[test]
    fn sanitized_url_shape() {
        let u = Url::parse("http://2130706433/README.md?tab=readme#license").unwrap();
        assert_eq!(sanitized_url(&u), "http://127.0.0.1/README.md");
        let u2 = Url::parse("https://example.com/").unwrap();
        assert_eq!(sanitized_url(&u2), "https://example.com/");
        let u3 = Url::parse("http://127.0.0.1:8080/admin?x=1").unwrap();
        assert_eq!(sanitized_url(&u3), "http://127.0.0.1:8080/admin");
    }

    #[test]
    fn disguised_ipv4_inside_ipv6_is_judged_by_the_embedded_address() {
        use std::net::IpAddr;
        // The SSRF hole that the first draft had: ::ffff:a.b.c.d must be treated as a.b.c.d.
        for bad in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.5",
            "::ffff:192.168.1.1",
            "::ffff:169.254.169.254",
            "::ffff:100.64.0.1",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
        ] {
            let ip: IpAddr = bad.parse().unwrap();
            assert!(!ip_is_public(ip), "{bad} must NOT be public");
        }
        for good in ["::ffff:8.8.8.8", "::ffff:1.1.1.1", "64:ff9b::808:808", "2606:4700:4700::1111"] {
            let ip: IpAddr = good.parse().unwrap();
            assert!(ip_is_public(ip), "{good} should be public");
        }
        assert!(guard_url("http://[::ffff:127.0.0.1]/").is_err());
        assert!(guard_url("http://[::ffff:169.254.169.254]/latest/meta-data").is_err());
    }

    #[test]
    fn spoken_lines_are_short_and_sir_shaped() {
        assert!(ProbeRefusal::PrivateAddress.spoken().contains("public internet"));
        assert!(ProbeRefusal::Sensitive.spoken().contains("banking"));
    }

    // ----------------------------------------------------------------
    // header / cookie matching
    // ----------------------------------------------------------------

    #[test]
    fn header_signal_via_x_powered_by() {
        let url = Url::parse("https://example.com/case-a").unwrap();
        let headers = vec![
            ("x-powered-by".to_string(), "Next.js 14".to_string()),
        ];
        let r = analyse(&url, 200, &headers, "", &[], &[]);
        let d = find(&r, "nextjs").expect("Next.js must be detected");
        assert!(d.confidence >= 0.80, "conf {}", d.confidence);
        assert!(
            d.evidence.iter().any(|e| e == "X-Powered-By: Next.js"),
            "evidence must be the exact string {:?}",
            d.evidence
        );
    }

    #[test]
    fn cookie_signal_shopify_y_and_ga() {
        let url = Url::parse("https://shop.example.com/case-b").unwrap();
        let headers = vec![(
            "set-cookie".to_string(),
            "_shopify_y=deadbeef; path=/".to_string(),
        )];
        let r = analyse(&url, 200, &headers, "", &[], &[]);
        assert!(find(&r, "shopify").is_some(), "shopify via cookie prefix");

        let url = Url::parse("https://shop.example.com/case-c").unwrap();
        let headers = vec![("set-cookie".to_string(), "_ga=GA1.2.x; path=/".to_string())];
        let r = analyse(&url, 200, &headers, "", &[], &[]);
        let d = find(&r, "google-analytics").expect("GA via _ga cookie");
        assert!(d.confidence >= 0.80);
    }

    // ----------------------------------------------------------------
    // implies
    // ----------------------------------------------------------------

    #[test]
    fn implies_boosts_and_attaches_evidence() {
        let url = Url::parse("https://example.com/implies").unwrap();
        let headers = vec![("x-powered-by".to_string(), "Next.js".to_string())];
        let r = analyse(&url, 200, &headers, "", &[], &[]);
        let react = find(&r, "react").expect("react implied by nextjs");
        assert!(react.confidence >= 0.50, "0.95 * 0.8 ~= 0.7998, got {}", react.confidence);
        assert!(
            react.evidence.iter().any(|e| e.starts_with("implied by ")),
            "implied evidence missing"
        );
        let node = find(&r, "nodejs").expect("nodejs implied");
        assert!(node.confidence >= 0.50);
    }

    #[test]
    fn below_threshold_is_dropped() {
        let url = Url::parse("https://example.com/drop").unwrap();
        let r = analyse(&url, 200, &Vec::new(), "<div class=\"x\">nothing</div>", &[], &[]);
        assert!(find(&r, "shadcn-ui").is_none());
        assert!(r.detections.is_empty(), "no detections expected: {:?}", r.detections);
    }

    // ----------------------------------------------------------------
    // evidence dedupe
    // ----------------------------------------------------------------

    #[test]
    fn evidence_deduped_for_repeated_signals() {
        let url = Url::parse("https://example.com/dedup").unwrap();
        let html = concat!(
            "<link rel=\"stylesheet\" href=\"https://example.com/css/bootstrap.min.css\">",
            "<link rel=\"stylesheet\" href=\"https://example.com/css/bootstrap-theme.css\">"
        );
        let styles = [String::from(".btn-primary { color: red }")];
        let r = analyse(&url, 200, &Vec::new(), html, &[], &styles);
        let d = find(&r, "bootstrap").expect("bootstrap present");
        // two link-href hits dedupe to one entry; the css_text signal keeps its own
        assert_eq!(d.evidence.len(), 2, "different signals keep distinct evidence: {:?}", d.evidence);
        let hrefs = d.evidence.iter().filter(|e| e.contains("stylesheet href")).count();
        assert_eq!(hrefs, 1, "same-kind evidences must dedupe: {:?}", d.evidence);
        assert!(
            d.evidence.iter().any(|e| e.contains("Bootstrap button classes in CSS")),
            "css evidence expected: {:?}", d.evidence
        );
    }

    // ----------------------------------------------------------------
    // group_by_category ordering
    // ----------------------------------------------------------------

    #[test]
    fn group_by_category_is_fixed_order_with_unknown_last() {
        let report = SiteReport {
            url: String::new(),
            final_url: String::new(),
            status_code: 200,
            title: String::new(),
            description: String::new(),
            lang: String::new(),
            detections: vec![
                det("mystery", "Mystery", "zzz", 0.3),
                det("paypal", "PayPal", "payments", 0.3),
                det("cloudflare", "Cloudflare", "security", 0.3),
                det("vercel", "Vercel", "hosting", 0.3),
            ],
            security: Default::default(),
            stats: Default::default(),
            notes: Vec::new(),
        };
        let order: Vec<String> = group_by_category(&report)
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        let pos = |k: &str| {
            order
                .iter()
                .position(|x| *x == k)
                .unwrap_or_else(|| panic!("{k} not in {order:?}"))
        };
        assert!(pos("Hosting") < pos("Payments"), "hosting first");
        assert!(pos("Payments") < pos("Security"), "payments before security");
        assert_eq!(*order.last().unwrap(), "Zzz", "unknown category last");
    }

    // ----------------------------------------------------------------
    // report_markdown
    // ----------------------------------------------------------------

    #[test]
    fn report_markdown_header_and_fallback_line() {
        let report = SiteReport {
            url: String::new(),
            final_url: String::new(),
            status_code: 200,
            title: "Docs Home".into(),
            description: String::new(),
            lang: String::new(),
            detections: vec![det("nextjs", "Next.js", "meta-framework", 0.9)],
            security: Default::default(),
            stats: Default::default(),
            notes: Vec::new(),
        };
        let md = report_markdown("example.com", &report);
        assert!(md.starts_with("## example.com"), "title line: {md}");

        let empty = SiteReport {
            url: String::new(),
            final_url: String::new(),
            status_code: 200,
            title: String::new(),
            description: String::new(),
            lang: String::new(),
            detections: vec![],
            security: Default::default(),
            stats: Default::default(),
            notes: Vec::new(),
        };
        let md2 = report_markdown("example.com", &empty);
        assert!(md2.contains("Not detectable from outside"), "fallback missing: {md2}");
    }

    // ----------------------------------------------------------------
    // analyse tests from real fixtures
    // ----------------------------------------------------------------

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/site/{}", env!("CARGO_MANIFEST_DIR"), name);
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("fixture missing: {path}"))
    }

    fn headers_of(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(h, v)| (h.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn analyse_fixture_nextjs_vercel() {
        let url = Url::parse("https://example.com/f1").unwrap();
        let headers = headers_of(&[("x-powered-by", "Next.js"), ("x-vercel-id", "iad1::abc")]);
        let r = analyse(&url, 200, &headers, &fixture("nextjs_vercel.html"), &[], &[]);
        let n = find(&r, "nextjs").expect("nextjs");
        assert!(n.confidence >= 0.99, "{}", n.confidence);
        assert!(matches!(n.status, Status::Detected));
        let v = find(&r, "vercel").expect("vercel via X-Vercel-Id header");
        assert!(v.confidence >= 0.80, "{}", v.confidence);
        let react = find(&r, "react").expect("react implied");
        assert!(react.confidence >= 0.50);
        let node = find(&r, "nodejs").expect("nodejs implied");
        assert!(node.confidence >= 0.50);
    }

    #[test]
    fn analyse_fixture_wordpress_woocommerce_hidden() {
        let url = Url::parse("https://example.com/f2").unwrap();
        let headers = vec![(
            "set-cookie".to_string(),
            "woocommerce_cart_hash=abc; path=/".to_string(),
        )];
        // the Woocommerce script text is only in the fetched subresource
        let scripts = [String::from(
            "var wc_add_to_cart_params = {\"ajax_url\":\"\\\\/wp-admin\\\\/admin-ajax.php\"};",
        )];
        let r = analyse(&url, 200, &headers, &fixture("wordpress_woocommerce.html"), &scripts, &[]);
        let w = find(&r, "wordpress").expect("wordpress");
        assert!(w.confidence >= 0.99, "{}", w.confidence);
        let wc = find(&r, "woocommerce").expect("woocommerce must be present");
        assert!(wc.confidence >= 0.80, "{}", wc.confidence);
        assert!(find(&r, "shopify").is_none(), "shopify must be absent");
    }

    #[test]
    fn analyse_fixture_shopify() {
        let url = Url::parse("https://example.com/f3").unwrap();
        let headers = vec![(
            "set-cookie".to_string(),
            "_shopify_y=abc; path=/".to_string(),
        )];
        let r = analyse(&url, 200, &headers, &fixture("shopify.html"), &[], &[]);
        let s = find(&r, "shopify").expect("shopify");
        assert!(s.confidence >= 0.99, "{}", s.confidence);
        assert!(find(&r, "woocommerce").is_none(), "woocommerce must be absent");
    }

    #[test]
    fn analyse_fixture_nuxt_cloudflare_pages() {
        let url = Url::parse("https://nh.pages.dev/f4").unwrap();
        let headers = vec![("cf-ray".to_string(), "abc123".to_string())];
        let styles = [String::from(".nuxt-link-active { font-weight: bold }")];
        let r = analyse(&url, 200, &headers, &fixture("nuxt_cloudflare.html"), &[], &styles);
        let nu = find(&r, "nuxt").expect("nuxt");
        assert!(nu.confidence >= 0.99, "{}", nu.confidence);
        let vue = find(&r, "vue").expect("vue implied by nuxt");
        assert!(vue.confidence >= 0.50);
        let cf = find(&r, "cloudflare-cdn").expect("cloudflare-cdn");
        assert!(cf.confidence >= 0.80, "{}", cf.confidence);
        assert!(find(&r, "vercel").is_none(), "vercel must be absent");
    }

    #[test]
    fn analyse_fixture_static_tailwind_ga() {
        let url = Url::parse("https://example.com/f5").unwrap();
        let styles = [String::from(".md\\:shadow-lg { --tw-translate-x: 0 }")];
        let scripts = [String::from(
            "window.dataLayer = window.dataLayer || []; gtag('js', new Date());",
        )];
        let r = analyse(&url, 200, &Vec::new(), &fixture("static_tailwind_ga.html"), &scripts, &styles);
        let tw = find(&r, "tailwind").expect("tailwind");
        assert!(tw.confidence >= 0.80, "{}", tw.confidence);
        let ga = find(&r, "google-analytics").expect("GA");
        assert!(ga.confidence >= 0.80, "{}", ga.confidence);
        assert!(find(&r, "nextjs").is_none(), "nextjs must be absent");
    }

    #[test]
    fn analyse_fixture_angular() {
        let url = Url::parse("https://example.com/f6").unwrap();
        let r = analyse(&url, 200, &Vec::new(), &fixture("angular.html"), &[], &[]);
        let a = find(&r, "angular").expect("angular");
        assert!(a.confidence >= 0.80, "{}", a.confidence);
        assert!(find(&r, "angularjs").is_none(), "AngularJS must be absent");
        assert!(find(&r, "nextjs").is_none());
    }

    // ----------------------------------------------------------------
    // probe without a server
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn probe_refuses_loopback_without_permission() {
        match probe("http://127.0.0.1:1/").await {
            Err(ProbeRefusal::PrivateAddress) => {}
            other => panic!("expected PrivateAddress, got {other:?}"),
        }
    }

    // ----------------------------------------------------------------
    // mock server + network tests
    // ----------------------------------------------------------------

    type Handled = (u16, Vec<(String, String)>, Vec<u8>, Option<String>);

    fn response_bytes(status: u16, headers: &[(String, String)], body: &[u8]) -> Vec<u8> {
        let reason = match status {
            200 => "OK",
            404 => "Not Found",
            _ => "Found",
        };
        let mut out = format!("HTTP/1.1 {status} {reason}\r\n").into_bytes();
        for (h, v) in headers {
            out.extend_from_slice(format!("{h}: {v}\r\n").as_bytes());
        }
        out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
        out.extend_from_slice(b"connection: close\r\n\r\n");
        out.extend_from_slice(body);
        out
    }

    async fn spawn_mock(handles: Vec<Handled>) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for (status, headers, body, location) in handles {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 65536];
                let _ = tokio::time::timeout(std::time::Duration::from_secs(10), stream.readable()).await;
                let n = stream.try_read(&mut buf).unwrap_or(0);
                let _req = String::from_utf8_lossy(&buf[..n]).to_string();
                let mut resp_headers = Vec::new();
                if let Some(loc) = location {
                    resp_headers.push(("location".to_string(), loc));
                }
                resp_headers.extend(headers);
                let out = response_bytes(status, &resp_headers, &body);
                // `try_write` is non-blocking and writes only what fits in the socket buffer (much less on macOS than on
                // Windows), which cut the 5 MB test response short. `write_all` sends all of it.
                use tokio::io::AsyncWriteExt;
                let _ = stream.write_all(&out).await;
                let _ = stream.shutdown().await;
                drop(stream);
            }
        });
        addr
    }

    #[tokio::test]
    async fn probe_200_html_parses_and_collects_third_party() {
        let body = concat!(
            "<html>",
            "<meta name=\"generator\" content=\"WordPress 6.5\">",
            "<script src=\"/app.js\"></script>",
            "<img src=\"https://stats.example.net/pixel?id=1\">",
            "<iframe src=\"https://payments.example.org/checkout\"></iframe>",
            "</html>",
        );
        let body2 = "regeneratorRuntime = function(){};";
        let addr = spawn_mock(vec![
            (
                200,
                vec![("content-type".into(), "text/html".into())],
                body.as_bytes().to_vec(),
                None,
            ),
            (
                200,
                vec![("content-type".into(), "application/javascript".into())],
                body2.as_bytes().to_vec(),
                None,
            ),
        ])
        .await;
        let opts = ProbeOpts { allow_loopback: true, total_timeout_ms: Some(3_000) };
        let report = probe_with(&format!("http://{addr}/"), opts).await.expect("loopback probe");
        let wp = find(&report, "wordpress").expect("wordpress via generator meta");
        assert!(wp.confidence >= 0.80);
        let regen = find(&report, "regenerator-runtime").expect("regenerator via fetched subresource");
        assert!(regen.confidence >= 0.50, "{}", regen.confidence);
        let hosts = &report.stats.third_party_hosts;
        assert!(hosts.iter().any(|h| h == "stats.example.net"), "hosts: {hosts:?}");
        assert!(hosts.iter().any(|h| h == "payments.example.org"), "hosts: {hosts:?}");
    }

    #[tokio::test]
    async fn probe_rejects_non_html_content_type() {
        let addr = spawn_mock(vec![(
            200,
            vec![("content-type".into(), "application/json".into())],
            b"{\"ok\":true}".to_vec(),
            None,
        )])
        .await;
        let opts = ProbeOpts { allow_loopback: true, total_timeout_ms: Some(3_000) };
        match probe_with(&format!("http://{addr}/"), opts).await {
            Err(ProbeRefusal::NotHtml) => {}
            other => panic!("expected NotHtml, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_truncates_oversized_page() {
        let addr = spawn_mock(vec![(
            200,
            vec![("content-type".into(), "text/html".into())],
            vec![b'x'; 5 * 1024 * 1024],
            None,
        )])
        .await;
        let opts = ProbeOpts { allow_loopback: true, total_timeout_ms: Some(3_000) };
        let report = probe_with(&format!("http://{addr}/"), opts).await.expect("probe");
        assert!(
            report.notes.iter().any(|n| n == NOTE_TRUNCATED),
            "truncation note missing: {:?}",
            report.notes
        );
    }

    #[tokio::test]
    async fn probe_rejects_private_redirect_hop() {
        let addr = spawn_mock(vec![(
            302,
            vec![],
            Vec::new(),
            Some("http://169.254.169.254/latest/meta-data/".into()),
        )])
        .await;
        let opts = ProbeOpts { allow_loopback: true, total_timeout_ms: Some(3_000) };
        match probe_with(&format!("http://{addr}/"), opts).await {
            Err(ProbeRefusal::PrivateAddress) => {}
            other => panic!("expected PrivateAddress, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_rejects_too_many_redirects() {
        let mut handles = Vec::new();
        for _ in 0..6 {
            handles.push((
                302u16,
                vec![("content-type".into(), "text/html".into())],
                Vec::new(),
                Some("/".to_string()),
            ));
        }
        let addr = spawn_mock(handles).await;
        let opts = ProbeOpts { allow_loopback: true, total_timeout_ms: Some(4_000) };
        match probe_with(&format!("http://{addr}/"), opts).await {
            Err(ProbeRefusal::TooManyRedirects) => {}
            other => panic!("expected TooManyRedirects, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_reports_timeout_when_server_hangs() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(5), stream.readable()).await;
                let _ = stream.try_read(&mut buf);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
        let opts = ProbeOpts { allow_loopback: true, total_timeout_ms: Some(1_500) };
        match probe_with(&format!("http://{addr}/"), opts).await {
            Err(ProbeRefusal::Timeout) => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
    }
}
