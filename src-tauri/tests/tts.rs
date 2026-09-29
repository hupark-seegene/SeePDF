//! P2 — 읽어 주기 (`tts_speak` / `tts_stop` / `tts_status`, `IPC_CONTRACT.md` §11a).
//!
//! The command semantics run against a fake speaker (what the commands call is exactly
//! [`Tts`]); on macOS the real `say` is driven too — rendering to a file, so the test is silent.

use seepdf_lib::app::tts::{NoSpeaker, SpeakRequest, Speaker, Tts, Utterance};
use seepdf_lib::ipc::types::TtsEngine;
use seepdf_lib::ipc::{EngineError, ErrorCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One fake utterance: running until stopped or "finished" by the test.
struct FakeUtterance {
    running: Arc<AtomicBool>,
}

impl Utterance for FakeUtterance {
    fn running(&mut self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeState {
    requests: Mutex<Vec<SpeakRequest>>,
    flags: Mutex<Vec<Arc<AtomicBool>>>,
}

/// Records every request; the test keeps a clone to look inside.
#[derive(Default, Clone)]
struct FakeSpeaker(Arc<FakeState>);

impl std::ops::Deref for FakeSpeaker {
    type Target = FakeState;
    fn deref(&self) -> &FakeState {
        &self.0
    }
}

impl Speaker for FakeSpeaker {
    fn engine(&self) -> Option<TtsEngine> {
        Some(TtsEngine::Say)
    }

    fn start(
        &self,
        request: &SpeakRequest,
    ) -> Result<(Box<dyn Utterance>, Option<String>), EngineError> {
        self.requests.lock().unwrap().push(request.clone());
        let flag = Arc::new(AtomicBool::new(true));
        self.flags.lock().unwrap().push(flag.clone());
        let voice = (request.lang.as_deref() == Some("ko")).then(|| "Fake-ko".to_string());
        Ok((Box::new(FakeUtterance { running: flag }), voice))
    }
}

#[test]
fn tts_speak_stop_and_status_with_a_fake_speaker() {
    let speaker = FakeSpeaker::default();
    let tts = Tts::with_speaker(Box::new(speaker.clone()));

    let idle = tts.status();
    assert!(idle.supported && !idle.speaking);
    assert_eq!(idle.engine, Some(TtsEngine::Say));

    let status = tts
        .speak("첫 번째 문장", Some("ko"), Some(1.25))
        .expect("speak");
    assert!(status.speaking);
    assert_eq!(status.voice.as_deref(), Some("Fake-ko"));
    {
        let requests = speaker.requests.lock().unwrap();
        assert_eq!(requests[0].text, "첫 번째 문장");
        assert_eq!(requests[0].rate, 1.25);
    }

    // a second speak stops the first utterance before starting
    tts.speak("second", None, Some(9.0)).expect("speak again");
    {
        let flags = speaker.flags.lock().unwrap();
        assert!(
            !flags[0].load(Ordering::SeqCst),
            "the first utterance was stopped"
        );
        assert!(flags[1].load(Ordering::SeqCst));
        assert_eq!(
            speaker.requests.lock().unwrap()[1].rate,
            2.0,
            "rate is clamped"
        );
    }
    assert!(tts.status().speaking);

    let stopped = tts.stop();
    assert!(!stopped.speaking);
    assert!(!speaker.flags.lock().unwrap()[1].load(Ordering::SeqCst));
    // stopping twice is harmless
    assert!(!tts.stop().speaking);

    // an utterance that ends by itself shows up as not speaking on the next status
    tts.speak("third", None, None).unwrap();
    speaker.flags.lock().unwrap()[2].store(false, Ordering::SeqCst);
    assert!(!tts.status().speaking);

    // nothing to say
    let err = tts.speak("   \n", None, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        speaker.requests.lock().unwrap().len(),
        3,
        "a refused request starts nothing"
    );

    // dropping the state stops a running utterance (app exit)
    tts.speak("fourth", None, None).unwrap();
    let last = speaker.flags.lock().unwrap()[3].clone();
    drop(tts);
    assert!(!last.load(Ordering::SeqCst));
}

/// v0.3 (V4): sentences are queued on the Rust side, one utterance each; every start is
/// reported (`tts-progress`), the queue survives the gap between two sentences, a new speak
/// from `startIndex` (a 속도 change) continues there, and stop ends the queue.
#[test]
fn tts_sentence_queue_reports_progress_and_restarts_mid_text() {
    use std::time::{Duration, Instant};

    let speaker = FakeSpeaker::default();
    let tts = Arc::new(Tts::with_speaker(Box::new(speaker.clone())));
    let progress: Arc<Mutex<Vec<Option<u32>>>> = Arc::default();
    {
        let progress = progress.clone();
        tts.set_progress_listener(move |i| progress.lock().unwrap().push(i));
    }
    let wait_for = |n: usize| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while speaker.flags.lock().unwrap().len() < n {
            assert!(Instant::now() < deadline, "sentence {n} never started");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let finish = |i: usize| speaker.flags.lock().unwrap()[i].store(false, Ordering::SeqCst);

    let sentences = vec![
        "첫 번째 문장입니다.".to_string(),
        "두 번째 문장입니다!".to_string(),
        "Third one?".to_string(),
    ];
    let status = tts
        .speak_sentences(sentences.clone(), 0, None, Some(1.0))
        .expect("speak sentences");
    assert!(status.speaking);
    assert_eq!(status.sentence_index, Some(0));
    assert_eq!(speaker.requests.lock().unwrap()[0].text, sentences[0]);
    // the language comes from the whole text: every sentence keeps the Korean voice
    assert_eq!(
        speaker.requests.lock().unwrap()[0].lang.as_deref(),
        Some("ko")
    );

    finish(0);
    wait_for(2);
    assert_eq!(speaker.requests.lock().unwrap()[1].text, sentences[1]);
    let status = tts.status();
    assert!(status.speaking);
    assert_eq!(status.sentence_index, Some(1));

    // 속도 change: the text restarts at the current sentence, at the new rate
    tts.speak_sentences(sentences.clone(), 1, None, Some(1.5))
        .expect("restart");
    {
        let flags = speaker.flags.lock().unwrap();
        assert!(
            !flags[1].load(Ordering::SeqCst),
            "the old sentence was stopped"
        );
        let requests = speaker.requests.lock().unwrap();
        assert_eq!(requests[2].text, sentences[1]);
        assert_eq!(requests[2].rate, 1.5);
    }
    finish(2);
    wait_for(4);
    assert_eq!(speaker.requests.lock().unwrap()[3].text, sentences[2]);
    assert_eq!(
        speaker.requests.lock().unwrap()[3].lang.as_deref(),
        Some("ko")
    );

    // the last sentence ends: the queue is done and says so
    finish(3);
    let deadline = Instant::now() + Duration::from_secs(5);
    while tts.status().speaking {
        assert!(Instant::now() < deadline, "the queue did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while progress.lock().unwrap().last() != Some(&None) {
        assert!(Instant::now() < deadline, "no end-of-queue progress");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        *progress.lock().unwrap(),
        vec![Some(0), Some(1), Some(1), Some(2), None]
    );
    // nothing else was started by a stale driver
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(speaker.requests.lock().unwrap().len(), 4);

    // stop ends a running queue and reports it
    tts.speak_sentences(sentences.clone(), 2, None, None)
        .unwrap();
    assert!(!tts.stop().speaking);
    assert_eq!(progress.lock().unwrap().last(), Some(&None));
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(
        speaker.requests.lock().unwrap().len(),
        5,
        "no sentence after stop"
    );

    // refused: nothing to read, or a start past the end
    let empty = tts
        .speak_sentences(vec![" ".into()], 0, None, None)
        .unwrap_err();
    assert_eq!(empty.code, ErrorCode::InvalidArgument);
    let past = tts.speak_sentences(sentences, 3, None, None).unwrap_err();
    assert_eq!(past.code, ErrorCode::InvalidArgument);
}

#[test]
fn tts_is_unsupported_without_a_system_voice() {
    let tts = Tts::with_speaker(Box::new(NoSpeaker));
    let status = tts.status();
    assert!(!status.supported && !status.speaking && status.engine.is_none());
    let err = tts.speak("hello", None, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use seepdf_lib::app::tts::{pick_voice, SaySpeaker};
    use std::time::{Duration, Instant};

    fn out_file(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("fixtures")
            .join("out")
            .join("p2-tts");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn say_lists_a_korean_voice() {
        let say = SaySpeaker::new();
        assert_eq!(say.engine(), Some(TtsEngine::Say), "/usr/bin/say exists");
        let voices = say.voices();
        assert!(!voices.is_empty(), "say -v ? lists voices");
        assert!(
            voices.iter().any(|v| v.locale.starts_with("ko_")),
            "a Korean voice is installed: {voices:?}"
        );
        assert!(pick_voice(voices, "ko").is_some());
    }

    #[test]
    fn say_speak_and_stop_round_trip() {
        // silent: `say -o <file>` renders instead of playing
        let path = out_file("long.aiff");
        let tts = Tts::with_speaker(Box::new(SaySpeaker::silent(path.clone())));
        let long = "안녕하세요. SeePDF 읽어 주기 시험입니다. ".repeat(200);
        let status = tts.speak(&long, None, Some(1.0)).expect("say starts");
        assert!(status.speaking, "running right after the start");
        assert!(
            status.voice.as_deref().is_some_and(|v| !v.is_empty()),
            "Hangul picks a Korean voice: {status:?}"
        );
        let stopped = tts.stop();
        assert!(!stopped.speaking, "stop kills the process");

        // a short text runs to completion on its own and really produced audio
        let short = out_file("short.aiff");
        let tts = Tts::with_speaker(Box::new(SaySpeaker::silent(short.clone())));
        tts.speak("짧은 문장입니다.", Some("ko-KR"), Some(1.5))
            .expect("say starts");
        let deadline = Instant::now() + Duration::from_secs(30);
        while tts.status().speaking {
            assert!(Instant::now() < deadline, "say did not finish");
            std::thread::sleep(Duration::from_millis(50));
        }
        let size = std::fs::metadata(&short).map(|m| m.len()).unwrap_or(0);
        assert!(size > 1000, "say wrote {size} bytes of audio");
    }
}
