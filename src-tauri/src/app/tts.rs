//! 읽어 주기 (read aloud, P2): the operating system's own voice, offline — `IPC_CONTRACT.md` §11a.
//!
//! * **macOS**: `/usr/bin/say`, text on stdin (never argv: no length limit, and a text that
//!   starts with `-` cannot turn into an option). `-v` picks a voice for the text's language —
//!   Korean text gets `Yuna` when it is installed (it ships with macOS), else the first `ko_*`
//!   voice `say -v ?` lists; other text keeps the system voice. `-r` is words per minute.
//! * **Windows**: `powershell.exe -EncodedCommand` running `System.Speech.Synthesis`
//!   (`Add-Type -AssemblyName System.Speech`), the text on stdin read as UTF-8 bytes (the
//!   console code page would mangle Hangul), a voice whose culture matches the language, and
//!   `Rate` −10…10. No console window (`CREATE_NO_WINDOW`).
//! * **Elsewhere**: `unsupported`.
//!
//! One utterance at a time for the whole app. A new `tts_speak` stops the current one;
//! `tts_stop`, dropping [`Tts`] and the app's `RunEvent::Exit` kill the process. The frontend
//! polls `tts_status` while it shows the 읽어 주기 bar, which is how it learns the voice ended.
//!
//! **Sentences (v0.3, V4).** `tts_speak` may carry the text already split into sentences (the
//! frontend splits it, Korean-aware, and keeps each sentence's characters on the page). They
//! are queued here and spoken one utterance each: a driver thread starts the next one as soon
//! as the previous ends and reports `tts-progress { sentenceIndex }` through the listener the
//! app installs ([`Tts::set_progress_listener`]), so the viewer can highlight the sentence
//! being read and a 속도 change can continue from it (`startIndex`) instead of the beginning.
//!
//! No pdfium here, so none of this runs on the engine thread.

use crate::ipc::types::{TtsEngine, TtsStatus};
use crate::ipc::EngineError;
use parking_lot::Mutex;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Longest text one `tts_speak` accepts (a dense page is ~5 000 characters).
pub const MAX_TEXT_CHARS: usize = 200_000;
/// `rate` is a multiplier of the voice's normal speed, clamped to this range.
pub const MIN_RATE: f32 = 0.5;
pub const MAX_RATE: f32 = 2.0;
/// `say`'s speaking rate at `rate` 1.0, in words per minute.
pub const SAY_BASE_WPM: f32 = 185.0;

/// What one `tts_speak` asks for, after validation.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakRequest {
    pub text: String,
    /// A BCP-47-ish language (`ko`, `ko-KR`, `en_US`), or `None` to guess from the text.
    pub lang: Option<String>,
    /// 0.5 … 2.0; 1.0 = the voice's normal speed.
    pub rate: f32,
}

/// A running utterance.
pub trait Utterance: Send {
    /// Still speaking.
    fn running(&mut self) -> bool;
    /// Stop now (idempotent).
    fn stop(&mut self);
}

/// Something that can speak: the OS voice, or a fake in tests.
pub trait Speaker: Send + Sync {
    /// `None` when this platform has no offline voice.
    fn engine(&self) -> Option<TtsEngine>;
    /// Starts speaking; returns the utterance and the voice picked, if one was picked.
    fn start(
        &self,
        request: &SpeakRequest,
    ) -> Result<(Box<dyn Utterance>, Option<String>), EngineError>;
}

struct Current {
    utterance: Box<dyn Utterance>,
    voice: Option<String>,
    /// Sentence mode (V4): what is left to read after `utterance`.
    queue: Option<Queue>,
}

/// The sentences of one `tts_speak`, and which one is being read.
struct Queue {
    sentences: Vec<String>,
    index: usize,
    lang: Option<String>,
    rate: f32,
}

impl Queue {
    fn has_next(&self) -> bool {
        self.index + 1 < self.sentences.len()
    }
}

/// How often the sentence driver checks whether the current sentence has ended.
pub const SENTENCE_POLL: Duration = Duration::from_millis(40);

/// `tts-progress`: `Some(i)` = sentence `i` started, `None` = the queue finished.
pub type ProgressListener = Arc<dyn Fn(Option<u32>) + Send + Sync>;

/// The app's one voice (Tauri managed state).
pub struct Tts {
    speaker: Box<dyn Speaker>,
    current: Mutex<Option<Current>>,
    last_voice: Mutex<Option<String>>,
    /// Bumped by every speak / stop, so a sentence driver of an older queue retires.
    generation: AtomicU64,
    progress: Mutex<Option<ProgressListener>>,
}

impl Tts {
    /// The platform's voice (`say`, SAPI, or none).
    pub fn system() -> Self {
        Self::with_speaker(system_speaker())
    }

    pub fn with_speaker(speaker: Box<dyn Speaker>) -> Self {
        Self {
            speaker,
            current: Mutex::new(None),
            last_voice: Mutex::new(None),
            generation: AtomicU64::new(0),
            progress: Mutex::new(None),
        }
    }

    /// Where `tts-progress` goes (the app emits the event from it).
    pub fn set_progress_listener(&self, listener: impl Fn(Option<u32>) + Send + Sync + 'static) {
        *self.progress.lock() = Some(Arc::new(listener));
    }

    fn report(&self, index: Option<usize>) {
        let listener = self.progress.lock().clone();
        if let Some(listener) = listener {
            listener(index.map(|i| i as u32));
        }
    }

    /// `tts_speak`: stops whatever is speaking, then speaks `text`.
    pub fn speak(
        &self,
        text: &str,
        lang: Option<&str>,
        rate: Option<f32>,
    ) -> Result<TtsStatus, EngineError> {
        let request = validate(text, lang, rate)?;
        if self.speaker.engine().is_none() {
            return Err(EngineError::unsupported(
                "read aloud needs the macOS or Windows system voice",
            ));
        }
        let mut current = self.current.lock();
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(mut previous) = current.take() {
            previous.utterance.stop();
        }
        let (utterance, voice) = self.speaker.start(&request)?;
        *self.last_voice.lock() = voice.clone();
        *current = Some(Current {
            utterance,
            voice,
            queue: None,
        });
        drop(current);
        Ok(self.status())
    }

    /// `tts_speak` with `sentences` (V4): stops whatever is speaking, then reads the sentences
    /// from `start` on, one utterance each, reporting each start through the progress
    /// listener. The language is picked once for the whole text (a Korean page keeps its
    /// Korean voice through an English sentence).
    pub fn speak_sentences(
        self: &Arc<Self>,
        sentences: Vec<String>,
        start: usize,
        lang: Option<&str>,
        rate: Option<f32>,
    ) -> Result<TtsStatus, EngineError> {
        if sentences.iter().all(|s| s.trim().is_empty()) {
            return Err(EngineError::invalid("there is no text to read"));
        }
        if start >= sentences.len() {
            return Err(EngineError::invalid(format!(
                "startIndex {start} is past the last of {} sentences",
                sentences.len()
            )));
        }
        let whole = sentences.join(" ");
        let base = validate(&whole, lang, rate)?;
        if self.speaker.engine().is_none() {
            return Err(EngineError::unsupported(
                "read aloud needs the macOS or Windows system voice",
            ));
        }
        let lang = base.lang.clone().or_else(|| voice_language(&base));
        let request = SpeakRequest {
            text: sentences[start].clone(),
            lang: lang.clone(),
            rate: base.rate,
        };
        let generation = {
            let mut current = self.current.lock();
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(mut previous) = current.take() {
                previous.utterance.stop();
            }
            let (utterance, voice) = self.speaker.start(&request)?;
            *self.last_voice.lock() = voice.clone();
            *current = Some(Current {
                utterance,
                voice,
                queue: Some(Queue {
                    sentences,
                    index: start,
                    lang,
                    rate: base.rate,
                }),
            });
            generation
        };
        self.report(Some(start));
        let tts = Arc::clone(self);
        std::thread::Builder::new()
            .name("seepdf-tts-queue".into())
            .spawn(move || tts.drive(generation))
            .map_err(|e| EngineError::io(format!("tts queue thread: {e}")))?;
        Ok(self.status())
    }

    /// The sentence driver: starts the next sentence when the current one ends; retires when
    /// another speak / stop bumped the generation.
    fn drive(&self, generation: u64) {
        loop {
            std::thread::sleep(SENTENCE_POLL);
            let event = {
                let mut current = self.current.lock();
                if self.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                let Some(c) = current.as_mut() else {
                    return;
                };
                if c.utterance.running() {
                    continue;
                }
                let Some(queue) = c.queue.as_mut() else {
                    return;
                };
                if !queue.has_next() {
                    *current = None;
                    None
                } else {
                    queue.index += 1;
                    let request = SpeakRequest {
                        text: queue.sentences[queue.index].clone(),
                        lang: queue.lang.clone(),
                        rate: queue.rate,
                    };
                    let index = queue.index;
                    match self.speaker.start(&request) {
                        Ok((utterance, voice)) => {
                            c.utterance = utterance;
                            if voice.is_some() {
                                c.voice = voice.clone();
                                *self.last_voice.lock() = voice;
                            }
                            Some(index)
                        }
                        Err(e) => {
                            tracing::warn!("read aloud: sentence {index} failed to start: {e}");
                            *current = None;
                            None
                        }
                    }
                }
            };
            self.report(event);
            if event.is_none() {
                return;
            }
        }
    }

    /// `tts_stop`: stops the current utterance (and any queued sentences), if any.
    pub fn stop(&self) -> TtsStatus {
        let stopped = {
            let mut current = self.current.lock();
            self.generation.fetch_add(1, Ordering::SeqCst);
            current.take().map(|mut c| {
                c.utterance.stop();
                c.queue.is_some()
            })
        };
        if stopped == Some(true) {
            self.report(None);
        }
        self.status()
    }

    /// `tts_status`. A finished utterance is dropped here, so `speaking` goes false by itself.
    pub fn status(&self) -> TtsStatus {
        let mut current = self.current.lock();
        // Between two sentences the utterance has ended but the queue has not: still speaking.
        let speaking = match current.as_mut() {
            Some(c) => c.utterance.running() || c.queue.as_ref().is_some_and(Queue::has_next),
            None => false,
        };
        let voice = match current.as_ref() {
            Some(c) => c.voice.clone(),
            None => self.last_voice.lock().clone(),
        };
        let sentence_index = current
            .as_ref()
            .and_then(|c| c.queue.as_ref())
            .filter(|_| speaking)
            .map(|q| q.index as u32);
        // A finished queue is the driver's to clear: it also reports the end (`tts-progress`).
        if !speaking && current.as_ref().is_some_and(|c| c.queue.is_none()) {
            *current = None;
        }
        let engine = self.speaker.engine();
        TtsStatus {
            supported: engine.is_some(),
            speaking,
            engine,
            voice,
            sentence_index,
        }
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn validate(
    text: &str,
    lang: Option<&str>,
    rate: Option<f32>,
) -> Result<SpeakRequest, EngineError> {
    if text.trim().is_empty() {
        return Err(EngineError::invalid("there is no text to read"));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(EngineError::invalid(format!(
            "the text is longer than {MAX_TEXT_CHARS} characters"
        )));
    }
    let rate = rate.unwrap_or(1.0);
    if !rate.is_finite() || rate <= 0.0 {
        return Err(EngineError::invalid("rate must be a positive number"));
    }
    let lang = lang
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned);
    Ok(SpeakRequest {
        text: text.to_string(),
        lang,
        rate: rate.clamp(MIN_RATE, MAX_RATE),
    })
}

/// The language to pick a voice for: the request's (`ko-KR` → `ko`), else `ko` when the text
/// has Hangul, else `None` (the system voice).
pub fn voice_language(request: &SpeakRequest) -> Option<String> {
    if let Some(lang) = &request.lang {
        let primary = lang
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        return (!primary.is_empty()).then_some(primary);
    }
    let hangul = request
        .text
        .chars()
        .any(|c| matches!(c as u32, 0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F));
    hangul.then(|| "ko".to_string())
}

fn system_speaker() -> Box<dyn Speaker> {
    #[cfg(target_os = "macos")]
    {
        Box::new(SaySpeaker::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(SapiSpeaker)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Box::new(NoSpeaker)
    }
}

// ---------------------------------------------------------------------------------------
// A child process
// ---------------------------------------------------------------------------------------

/// An utterance that is a child process; its stdin is fed on a short-lived thread (a large
/// text must not block the command thread on a full pipe).
pub struct ProcessUtterance {
    child: Child,
}

impl ProcessUtterance {
    /// Spawns `command` with piped stdin and writes `input` to it.
    pub fn spawn(mut command: Command, input: Vec<u8>) -> Result<Self, EngineError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|e| EngineError::unsupported(&format!("start the system voice: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::Builder::new()
                .name("seepdf-tts-stdin".into())
                .spawn(move || {
                    // A killed child closes the pipe: the error is expected and ignored.
                    let _ = stdin.write_all(&input);
                })
                .map_err(|e| EngineError::io(format!("tts stdin thread: {e}")))?;
        }
        Ok(Self { child })
    }
}

impl Utterance for ProcessUtterance {
    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn stop(&mut self) {
        if self.running() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Drop for ProcessUtterance {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------------------
// macOS: say
// ---------------------------------------------------------------------------------------

/// One line of `say -v ?`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub name: String,
    /// `ko_KR`, `en_US`, …
    pub locale: String,
}

/// Parses `say -v ?`: `Name  xx_YY    # sample`. Names may contain spaces and parentheses
/// (`Eddy (한국어(한국))`), so the locale is the **last** token before `#`.
pub fn parse_say_voices(listing: &str) -> Vec<Voice> {
    listing
        .lines()
        .filter_map(|line| {
            let left = line.split(" # ").next()?.trim_end();
            let (name, locale) = left.rsplit_once(char::is_whitespace)?;
            let (name, locale) = (name.trim(), locale.trim());
            let looks_like_locale = locale.len() >= 2
                && locale
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            (!name.is_empty() && looks_like_locale).then(|| Voice {
                name: name.to_string(),
                locale: locale.to_string(),
            })
        })
        .collect()
}

/// The voice for `lang` (`ko`): `Yuna` first for Korean, else the first voice of that language.
pub fn pick_voice(voices: &[Voice], lang: &str) -> Option<String> {
    let prefix = format!("{}_", lang.to_ascii_lowercase());
    let matching: Vec<&Voice> = voices
        .iter()
        .filter(|v| v.locale.to_ascii_lowercase().starts_with(&prefix))
        .collect();
    if lang.eq_ignore_ascii_case("ko") {
        if let Some(v) = matching.iter().find(|v| v.name == "Yuna") {
            return Some(v.name.clone());
        }
    }
    matching.first().map(|v| v.name.clone())
}

/// `say`'s arguments for a request (text goes to stdin).
pub fn say_args(voice: Option<&str>, rate: f32, output: Option<&std::path::Path>) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(voice) = voice {
        args.push("-v".to_string());
        args.push(voice.to_string());
    }
    if (rate - 1.0).abs() > 0.01 {
        args.push("-r".to_string());
        args.push(format!("{}", (SAY_BASE_WPM * rate).round() as u32));
    }
    if let Some(output) = output {
        args.push("-o".to_string());
        args.push(output.display().to_string());
    }
    args
}

/// `/usr/bin/say`. `output` (tests only) renders to a file instead of the speakers.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct SaySpeaker {
    program: std::path::PathBuf,
    output: Option<std::path::PathBuf>,
    voices: std::sync::OnceLock<Vec<Voice>>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl SaySpeaker {
    pub fn new() -> Self {
        Self {
            program: "/usr/bin/say".into(),
            output: None,
            voices: std::sync::OnceLock::new(),
        }
    }

    /// Renders to `output` instead of the speakers — for tests that must stay silent.
    pub fn silent(output: std::path::PathBuf) -> Self {
        Self {
            output: Some(output),
            ..Self::new()
        }
    }

    /// `say -v ?`, once per process (≈ 100 ms).
    pub fn voices(&self) -> &[Voice] {
        self.voices.get_or_init(|| {
            Command::new(&self.program)
                .args(["-v", "?"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| parse_say_voices(&String::from_utf8_lossy(&o.stdout)))
                .unwrap_or_default()
        })
    }
}

impl Default for SaySpeaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Speaker for SaySpeaker {
    fn engine(&self) -> Option<TtsEngine> {
        self.program.is_file().then_some(TtsEngine::Say)
    }

    fn start(
        &self,
        request: &SpeakRequest,
    ) -> Result<(Box<dyn Utterance>, Option<String>), EngineError> {
        let voice = voice_language(request).and_then(|lang| pick_voice(self.voices(), &lang));
        let mut command = Command::new(&self.program);
        command.args(say_args(
            voice.as_deref(),
            request.rate,
            self.output.as_deref(),
        ));
        let utterance = ProcessUtterance::spawn(command, request.text.clone().into_bytes())?;
        Ok((Box::new(utterance), voice))
    }
}

// ---------------------------------------------------------------------------------------
// Windows: PowerShell + System.Speech
// ---------------------------------------------------------------------------------------

/// `SpeechSynthesizer.Rate` (−10 … 10, ±10 ≈ ×3 / ÷3) for a speed multiplier.
pub fn sapi_rate(rate: f32) -> i32 {
    ((rate.ln() / 3f32.ln()) * 10.0).round().clamp(-10.0, 10.0) as i32
}

/// The PowerShell script: text from stdin as UTF-8, a voice of culture `lang*` when installed.
pub fn sapi_script(lang: Option<&str>, rate: i32) -> String {
    let culture = lang
        .filter(|l| l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .map(|l| {
            format!(
                "$v = $s.GetInstalledVoices() | Where-Object {{ $_.Enabled -and $_.VoiceInfo.Culture.Name -like '{l}*' }} | Select-Object -First 1\n\
                 if ($v) {{ $s.SelectVoice($v.VoiceInfo.Name) }}\n"
            )
        })
        .unwrap_or_default();
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         Add-Type -AssemblyName System.Speech\n\
         $in = [Console]::OpenStandardInput()\n\
         $ms = New-Object System.IO.MemoryStream\n\
         $in.CopyTo($ms)\n\
         $t = [System.Text.Encoding]::UTF8.GetString($ms.ToArray())\n\
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer\n\
         {culture}\
         $s.Rate = {rate}\n\
         $s.Speak($t)\n"
    )
}

/// `-EncodedCommand` wants base64 of UTF-16LE.
pub fn encode_command(script: &str) -> String {
    let bytes: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// PowerShell + `System.Speech` (Windows).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct SapiSpeaker;

impl Speaker for SapiSpeaker {
    fn engine(&self) -> Option<TtsEngine> {
        cfg!(target_os = "windows").then_some(TtsEngine::Sapi)
    }

    fn start(
        &self,
        request: &SpeakRequest,
    ) -> Result<(Box<dyn Utterance>, Option<String>), EngineError> {
        let lang = voice_language(request);
        let script = sapi_script(lang.as_deref(), sapi_rate(request.rate));
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encode_command(&script),
        ]);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let utterance = ProcessUtterance::spawn(command, request.text.clone().into_bytes())?;
        Ok((Box::new(utterance), None))
    }
}

/// Platforms without an offline voice.
#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
pub struct NoSpeaker;

impl Speaker for NoSpeaker {
    fn engine(&self) -> Option<TtsEngine> {
        None
    }

    fn start(
        &self,
        _request: &SpeakRequest,
    ) -> Result<(Box<dyn Utterance>, Option<String>), EngineError> {
        Err(EngineError::unsupported("no system voice on this platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_voice_listing_parses_names_with_spaces() {
        let listing = "Albert              en_US    # Hello! My name is Albert.\n\
                       Eddy (한국어(한국))      ko_KR    # 안녕하세요. 제 이름은 Eddy입니다.\n\
                       Soumya              kn_IN    # Hello! My name is Soumya.\n\
                       Yuna                ko_KR    # 안녕하세요. 제 이름은 유나입니다.\n\
                       \n";
        let voices = parse_say_voices(listing);
        assert_eq!(voices.len(), 4);
        assert_eq!(voices[1].name, "Eddy (한국어(한국))");
        assert_eq!(voices[1].locale, "ko_KR");
        // Yuna wins for Korean; `kn_IN` is not Korean
        assert_eq!(pick_voice(&voices, "ko").as_deref(), Some("Yuna"));
        assert_eq!(
            pick_voice(&voices[..3], "ko").as_deref(),
            Some("Eddy (한국어(한국))")
        );
        assert_eq!(pick_voice(&voices, "en").as_deref(), Some("Albert"));
        assert_eq!(pick_voice(&voices, "fr"), None);
    }

    #[test]
    fn language_comes_from_the_request_or_the_text() {
        let req = |text: &str, lang: Option<&str>| SpeakRequest {
            text: text.into(),
            lang: lang.map(Into::into),
            rate: 1.0,
        };
        assert_eq!(
            voice_language(&req("안녕하세요", None)).as_deref(),
            Some("ko")
        );
        assert_eq!(voice_language(&req("Hello", None)), None);
        assert_eq!(
            voice_language(&req("Hello", Some("en-US"))).as_deref(),
            Some("en")
        );
        assert_eq!(
            voice_language(&req("x", Some("ko_KR"))).as_deref(),
            Some("ko")
        );
    }

    #[test]
    fn rates_map_to_each_engine() {
        assert_eq!(say_args(None, 1.0, None), Vec::<String>::new());
        assert_eq!(
            say_args(Some("Yuna"), 1.5, None),
            vec!["-v", "Yuna", "-r", "278"]
        );
        assert_eq!(sapi_rate(1.0), 0);
        assert_eq!(sapi_rate(3.0), 10);
        assert_eq!(sapi_rate(0.5), -6);
        assert_eq!(sapi_rate(2.0), 6);
        assert!(validate("x", None, Some(9.0)).unwrap().rate == MAX_RATE);
        assert!(validate("  ", None, None).is_err());
        assert!(validate("x", None, Some(f32::NAN)).is_err());
    }

    #[test]
    fn powershell_script_is_encoded_as_utf16_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        // "a" in UTF-16LE is 61 00
        assert_eq!(encode_command("a"), "YQA=");
        let script = sapi_script(Some("ko"), -3);
        assert!(script.contains("Add-Type -AssemblyName System.Speech"));
        assert!(script.contains("-like 'ko*'"));
        assert!(script.contains("$s.Rate = -3"));
        assert!(
            !sapi_script(Some("ko'; rm"), 0).contains("rm"),
            "odd languages are ignored"
        );
    }
}
