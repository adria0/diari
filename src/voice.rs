//! Voice notes: record from the microphone, transcribe on the device with
//! Whisper (whisper.cpp), and add the text to a note. The audio itself is
//! never stored.
//!
//! The speech model is downloaded once, into the app's data folder.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Whisper "small", 5-bit quantized: good enough for Catalan, and small and
/// fast enough for a phone.
pub const MODEL_FILE: &str = "ggml-small-q5_1.bin";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin";
pub const MODEL_SIZE_MB: u32 = 190;
/// What Whisper expects: 16 kHz mono.
pub const SAMPLE_RATE: u32 = 16_000;
/// Recordings stop on their own after this long.
pub const MAX_SECS: u64 = 10 * 60;

pub fn model_path(data_dir: &Path) -> PathBuf {
    data_dir.join("whisper").join(MODEL_FILE)
}

/// Download the model (if it isn't there yet). `progress` gets the bytes
/// done and the total, when known.
pub fn download_model(data_dir: &Path, progress: &dyn Fn(u64, Option<u64>)) -> Result<PathBuf, String> {
    let path = model_path(data_dir);
    if path.exists() {
        return Ok(path);
    }
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = path.with_extension("part");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();
    let resp = agent.get(MODEL_URL).call().map_err(|e| e.to_string())?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let mut body = resp.into_body().into_reader();
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let (mut done, mut reported) = (0u64, 0u64);
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - reported >= 1024 * 1024 {
            reported = done;
            progress(done, total);
        }
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    if total.is_some_and(|t| t != done) {
        let _ = std::fs::remove_file(&part);
        return Err("download incomplete".into());
    }
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The loaded model, kept between voice notes (loading takes a moment).
static MODEL: Mutex<Option<(PathBuf, WhisperContext)>> = Mutex::new(None);

/// Text spoken in `samples` (16 kHz mono). `language` is a Whisper code
/// (`ca`, `es`, `en`…) or `auto`.
pub fn transcribe(model: &Path, samples: &[f32], language: &str) -> Result<String, String> {
    // Whisper wants at least a second of audio.
    if samples.len() < SAMPLE_RATE as usize / 2 {
        return Ok(String::new());
    }
    let mut padded;
    let samples = if samples.len() < SAMPLE_RATE as usize + SAMPLE_RATE as usize / 10 {
        padded = samples.to_vec();
        padded.resize(SAMPLE_RATE as usize * 11 / 10, 0.0);
        &padded[..]
    } else {
        samples
    };

    let mut guard = MODEL.lock().unwrap();
    if guard.as_ref().map(|(p, _)| p.as_path()) != Some(model) {
        *guard = None;
        let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
            .map_err(|e| format!("could not load the speech model: {e}"))?;
        *guard = Some((model.to_path_buf(), ctx));
    }
    let ctx = &guard.as_ref().unwrap().1;
    let mut state = ctx.create_state().map_err(|e| e.to_string())?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 4);
    params.set_n_threads(threads as i32);
    let language = language.trim();
    params.set_language(Some(if language.is_empty() { "auto" } else { language }));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_suppress_blank(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    state.full(params, samples).map_err(|e| format!("transcription failed: {e}"))?;
    let text: Vec<String> = state
        .as_iter()
        .filter_map(|s| s.to_str_lossy().ok().map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
        .collect();
    Ok(clean(&text.join(" ")))
}

/// Drop what Whisper makes up from silence or noise: sound tags like
/// `[Música]` or `(silenci)`, credits it learned from subtitles, and text
/// with no letters at all (a lone `#`, `...`).
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = out.to_lowercase();
    if lower.contains("amara.org") || !out.chars().any(char::is_alphabetic) {
        return String::new();
    }
    out
}

/// Loudness of the audio (root mean square, 0–1), for the log.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Mix interleaved `channels` down to mono and resample `rate` to 16 kHz
/// (linear interpolation: fine for speech).
pub fn to_whisper_format(interleaved: &[f32], channels: usize, rate: u32) -> Vec<f32> {
    let channels = channels.max(1);
    let mono: Vec<f32> =
        interleaved.chunks(channels).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    if rate == SAMPLE_RATE || mono.is_empty() {
        return mono;
    }
    let ratio = rate as f64 / SAMPLE_RATE as f64;
    let n = (mono.len() as f64 / ratio) as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * ratio;
            let j = pos as usize;
            let frac = (pos - j as f64) as f32;
            let a = mono[j];
            let b = *mono.get(j + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

/// A recording in progress. `stop` gives the audio, ready for `transcribe`.
pub struct Recorder {
    inner: imp::Recorder,
    started: std::time::Instant,
}

impl Recorder {
    pub fn start() -> Result<Self, String> {
        Ok(Self { inner: imp::Recorder::start()?, started: std::time::Instant::now() })
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Stop and return the audio as 16 kHz mono.
    pub fn stop(self) -> Result<Vec<f32>, String> {
        self.inner.stop()
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{Arc, Mutex};

    pub struct Recorder {
        stream: cpal::Stream,
        buf: Arc<Mutex<Vec<f32>>>,
        channels: usize,
        rate: u32,
    }

    impl Recorder {
        pub fn start() -> Result<Self, String> {
            let device = cpal::default_host().default_input_device().ok_or("no microphone found")?;
            let config = device.default_input_config().map_err(|e| format!("microphone: {e}"))?;
            let (channels, rate) = (config.channels() as usize, config.sample_rate().0);
            let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
            let err = |e: cpal::StreamError| log::warn!("microphone: {e}");
            let b = buf.clone();
            let stream = match config.sample_format() {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &config.into(),
                    move |d: &[f32], _: &_| b.lock().unwrap().extend_from_slice(d),
                    err,
                    None,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &config.into(),
                    move |d: &[i16], _: &_| b.lock().unwrap().extend(d.iter().map(|&s| s as f32 / 32768.0)),
                    err,
                    None,
                ),
                cpal::SampleFormat::I32 => device.build_input_stream(
                    &config.into(),
                    move |d: &[i32], _: &_| {
                        b.lock().unwrap().extend(d.iter().map(|&s| s as f32 / 2_147_483_648.0))
                    },
                    err,
                    None,
                ),
                other => return Err(format!("unsupported microphone format {other}")),
            }
            .map_err(|e| format!("microphone: {e}"))?;
            stream.play().map_err(|e| format!("microphone: {e}"))?;
            Ok(Self { stream, buf, channels, rate })
        }

        pub fn stop(self) -> Result<Vec<f32>, String> {
            drop(self.stream);
            let data = std::mem::take(&mut *self.buf.lock().unwrap());
            Ok(super::to_whisper_format(&data, self.channels, self.rate))
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    pub struct Recorder {
        stop: Arc<AtomicBool>,
        thread: JoinHandle<Result<Vec<i16>, String>>,
    }

    impl Recorder {
        pub fn start() -> Result<Self, String> {
            crate::android::ensure_mic_permission()?;
            let stop = Arc::new(AtomicBool::new(false));
            let s = stop.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("diari-record".into())
                .spawn(move || crate::android::record_audio(super::SAMPLE_RATE, &s, |r| {
                    let _ = tx.send(r);
                }))
                .map_err(|e| e.to_string())?;
            // Wait until the microphone has started (or failed to).
            match rx.recv() {
                Ok(Ok(())) => Ok(Self { stop, thread }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(e)
                }
                Err(_) => Err(thread.join().ok().and_then(|r| r.err()).unwrap_or_else(|| "microphone failed".into())),
            }
        }

        pub fn stop(self) -> Result<Vec<f32>, String> {
            self.stop.store(true, Ordering::SeqCst);
            let pcm = self.thread.join().map_err(|_| "recording crashed".to_string())??;
            Ok(pcm.iter().map(|&s| s as f32 / 32768.0).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_drops_made_up_text() {
        assert_eq!(clean("#"), "");
        assert_eq!(clean(" [Música] "), "");
        assert_eq!(clean("(silenci) ..."), "");
        assert_eq!(clean("Subtítols realitzats per la comunitat d'Amara.org"), "");
        assert_eq!(clean("Comprar pa [soroll] i llet."), "Comprar pa i llet.");
        assert_eq!(clean("Demà a les 10"), "Demà a les 10");
    }

    #[test]
    fn resample_and_downmix() {
        // Stereo 48 kHz, one second: becomes 16 000 mono samples.
        let stereo: Vec<f32> = (0..48_000).flat_map(|_| [0.5f32, -0.5]).collect();
        let out = to_whisper_format(&stereo, 2, 48_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|s| s.abs() < 1e-6));
        let mono: Vec<f32> = (0..32_000).map(|i| i as f32).collect();
        let out = to_whisper_format(&mono, 1, 32_000);
        assert_eq!(out.len(), 16_000);
        assert_eq!(out[1], 2.0);
    }

    /// `WHISPER_MODEL=…/ggml-small-q5_1.bin VOICE_WAV=…/ca.wav cargo test --release --lib voice::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live() {
        let model = PathBuf::from(std::env::var("WHISPER_MODEL").expect("set WHISPER_MODEL"));
        let wav = std::env::var("VOICE_WAV").expect("set VOICE_WAV");
        let mut r = hound::WavReader::open(wav).unwrap();
        let spec = r.spec();
        let samples: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        let samples = to_whisper_format(&samples, spec.channels as usize, spec.sample_rate);
        let t = std::time::Instant::now();
        let text = transcribe(&model, &samples, "ca").unwrap();
        println!("{text}\n({:.1}s for {:.1}s of audio)", t.elapsed().as_secs_f32(), samples.len() as f32 / 16000.);
        assert!(!text.is_empty());
    }
}
