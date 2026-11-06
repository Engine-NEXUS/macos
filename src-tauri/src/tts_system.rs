//! The Windows built-in voice (WinRT `SpeechSynthesizer`, the installed OneCore voices): the
//! "never silent, never late" safety net of the voice stack.
//!
//! Measured on the user's laptop (2026-10-15, read-only System.Speech probe, 16 % battery):
//! a short line renders in 10-45 ms (first call ~440 ms), fully offline, no model, no network, no
//! extra RAM. The cloud voice was returning 403 on every request and the local neural voice took
//! 6-20 s per line in the same logs, so this is what plays when nothing better is ready in time.
//! It is a DIFFERENT speaker from the persona voice by construction; it is only used for a line
//! whose first audio would otherwise be late.

/// Preferred voice names, best first (female US voices match the default persona; any `en-` voice
/// is accepted after them).
#[cfg(any(windows, test))]
const PREFERRED: &[&str] = &["zira", "aria", "jenny", "hazel", "david", "mark"];

/// Pick the index of the best voice from `(display_name, language)` pairs. Pure.
#[cfg(any(windows, test))]
pub(crate) fn pick_voice(voices: &[(String, String)]) -> Option<usize> {
    let is_en = |l: &str| l.to_lowercase().starts_with("en-");
    for want in PREFERRED {
        if let Some(i) = voices
            .iter()
            .position(|(n, l)| is_en(l) && n.to_lowercase().contains(want))
        {
            return Some(i);
        }
    }
    voices.iter().position(|(_, l)| is_en(l))
}

/// Render `text` with the Windows voice. Runs on a blocking thread (WinRT calls block).
pub async fn synthesize(text: &str) -> Result<(Vec<f32>, u32), String> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || synthesize_blocking(&text))
        .await
        .map_err(|e| format!("system voice join: {e}"))?
}

#[cfg(windows)]
fn synthesize_blocking(text: &str) -> Result<(Vec<f32>, u32), String> {
    use windows::core::HSTRING;
    use windows::Media::SpeechSynthesis::SpeechSynthesizer;
    use windows::Storage::Streams::DataReader;

    let synth = SpeechSynthesizer::new().map_err(|e| format!("system voice: {e}"))?;
    if let Ok(all) = SpeechSynthesizer::AllVoices() {
        let count = all.Size().unwrap_or(0);
        let names: Vec<(String, String)> = (0..count)
            .map(|i| match all.GetAt(i) {
                Ok(v) => (
                    v.DisplayName().map(|s| s.to_string()).unwrap_or_default(),
                    v.Language().map(|s| s.to_string()).unwrap_or_default(),
                ),
                Err(_) => (String::new(), String::new()),
            })
            .collect();
        if let Some(i) = pick_voice(&names) {
            if let Ok(v) = all.GetAt(i as u32) {
                let _ = synth.SetVoice(&v);
            }
        }
    }
    let stream = synth
        .SynthesizeTextToStreamAsync(&HSTRING::from(text))
        .map_err(|e| format!("system voice: {e}"))?
        .get()
        .map_err(|e| format!("system voice: {e}"))?;
    let size = stream.Size().map_err(|e| format!("system voice: {e}"))? as u32;
    if size == 0 {
        return Err("system voice produced no audio".into());
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(|e| format!("system voice: {e}"))?)
        .map_err(|e| format!("system voice: {e}"))?;
    reader
        .LoadAsync(size)
        .map_err(|e| format!("system voice: {e}"))?
        .get()
        .map_err(|e| format!("system voice: {e}"))?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).map_err(|e| format!("system voice: {e}"))?;
    crate::tts_wav::decode_wav(&bytes).ok_or_else(|| "system voice returned an unsupported audio format".to_string())
}

/// macOS: the built-in system voice via `say`, written as a WAV and decoded,
/// so the same envelope/word-timing path wraps the instant safety net.
#[cfg(target_os = "macos")]
fn synthesize_blocking(text: &str) -> Result<(Vec<f32>, u32), String> {
    let dir = std::env::temp_dir().join("nexus-say");
    let _ = std::fs::create_dir_all(&dir);
    let wav = dir.join(format!("line-{}.wav", std::process::id()));
    // WAVE needs an explicit data format: without one `say` exits 1 with "Opening output file failed: fmt?".
    // 16-bit little-endian PCM is what `decode_wav` reads.
    let res = std::process::Command::new("say")
        .args(["--file-format=WAVE", "--data-format=LEI16@22050", "-o"])
        .arg(&wav)
        .arg(text)
        .output()
        .map_err(|e| format!("macOS say: {e}"))
        .and_then(|out| {
            if out.status.success() {
                let bytes = std::fs::read(&wav).map_err(|e| format!("macOS say wav: {e}"))?;
                crate::tts_wav::decode_wav(&bytes)
                    .ok_or_else(|| "macOS say returned an unsupported audio format".to_string())
            } else {
                Err(format!(
                    "macOS say failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        });
    let _ = std::fs::remove_file(&wav);
    res
}

#[cfg(not(any(windows, target_os = "macos")))]
fn synthesize_blocking(_text: &str) -> Result<(Vec<f32>, u32), String> {
    Err("the system voice is only implemented on Windows and macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(n: &str, l: &str) -> (String, String) {
        (n.to_string(), l.to_string())
    }

    #[test]
    fn prefers_zira_over_david_and_never_picks_a_foreign_language() {
        let voices = vec![
            v("Microsoft Hedda", "de-DE"),
            v("Microsoft David", "en-US"),
            v("Microsoft Zira", "en-US"),
        ];
        assert_eq!(pick_voice(&voices), Some(2));
        let only_foreign = vec![v("Microsoft Hedda", "de-DE"), v("Microsoft Zira", "fr-FR")];
        assert_eq!(pick_voice(&only_foreign), None);
    }

    #[test]
    fn falls_back_to_any_english_voice() {
        let voices = vec![v("Microsoft Heera", "en-IN"), v("Microsoft Hedda", "de-DE")];
        assert_eq!(pick_voice(&voices), Some(0));
        assert_eq!(pick_voice(&[]), None);
    }

    /// Live check against the installed Windows voices (needs a Windows desktop with English voices).
    #[cfg(windows)]
    #[test]
    #[ignore = "needs the Windows speech voices; run with --ignored"]
    fn the_windows_voice_renders_a_short_line_quickly() {
        let t0 = std::time::Instant::now();
        let (samples, rate) = synthesize_blocking("Ok sir.").expect("renders");
        let ms = t0.elapsed().as_millis();
        eprintln!("system voice: {} samples @ {rate} Hz in {ms} ms", samples.len());
        assert!(rate >= 8_000);
        assert!(samples.len() as u32 > rate / 5, "at least 0.2 s of audio");
        assert!(samples.iter().any(|s| s.abs() > 0.01), "not silence");
        assert!(ms < 3_000, "first call may be ~0.5 s, never seconds: {ms} ms");
    }

    /// `say` ships with every macOS install (CI runners included) and writes to a file, so no audio device is needed.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_voice_renders_a_short_line() {
        let (samples, rate) = synthesize_blocking("Ok sir.").expect("renders");
        assert_eq!(rate, 22_050);
        assert!(samples.len() as u32 > rate / 5, "at least 0.2 s of audio");
        assert!(samples.iter().any(|s| s.abs() > 0.01), "not silence");
    }
}
