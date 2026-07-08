//! Text-to-speech via an embedded Kokoro-82M ONNX model (ort) + a pure-Rust G2P
//! (misaki-rs). No Python, no uv, no espeak, no external process — one binary.
//!
//! Pipeline: text → misaki-rs → IPA phonemes → Kokoro char→id vocab → token ids →
//! ONNX (tokens/style/speed) → 24 kHz f32 waveform → cached WAV. The TUI only ever
//! *plays* the cached file through the earphone gate, so runtime latency is ~0 after
//! the first synth and nothing sounds unless the bound earphone is present.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::hash::DefaultHasher;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, ensure, Context, Result};
use misaki_rs::{Language, G2P};
use ort::inputs;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;

const SAMPLE_RATE: u32 = 24_000;
const STYLE_DIM: usize = 256;
/// Kokoro's phoneme budget per forward pass (sans the two pad tokens).
const MAX_PHONEMES: usize = 510;

#[derive(Clone)]
pub struct Tts {
    pub cache_dir: PathBuf,
    pub model: PathBuf,
    pub voices: PathBuf,
    pub voice: String,
    pub speed: f32,
}

impl Tts {
    /// Stable content-addressed path for a clip (deterministic across runs).
    pub fn cache_path(&self, text: &str) -> PathBuf {
        let mut h = DefaultHasher::new();
        text.trim().hash(&mut h);
        self.voice.hash(&mut h);
        ((self.speed * 100.0) as i32).hash(&mut h);
        self.cache_dir.join(format!("{:016x}.wav", h.finish()))
    }

    pub fn models_present(&self) -> bool {
        self.model.exists() && self.voices.exists()
    }
}

/// A warm Kokoro engine: the ONNX session, the voice style pack, and the G2P are
/// loaded once and stay resident, so synthesis after the first call is fast. Driven
/// from a worker thread (the ONNX run blocks).
pub struct TtsServer {
    session: Session,
    voices: HashMap<String, Vec<[f32; STYLE_DIM]>>,
    g2p: G2P,
}

impl TtsServer {
    pub fn start(tts: &Tts) -> Result<Self> {
        ensure!(
            tts.models_present(),
            "Kokoro model not found at {} — download it (first-run setup, or see README).",
            tts.model.display()
        );
        let session = Session::builder()
            .context("ort session builder")?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(&tts.model)
            .with_context(|| format!("loading Kokoro model {}", tts.model.display()))?;
        let voices = load_voices(&tts.voices)
            .with_context(|| format!("loading voices {}", tts.voices.display()))?;
        // Kokoro's US-English voices (af_*/am_*) were trained on Misaki en-us phonemes.
        let g2p = G2P::new(Language::EnglishUS);
        Ok(Self {
            session,
            voices,
            g2p,
        })
    }

    /// Synthesize `text` to WAV at `out` (blocking). First call pays the graph optimize.
    pub fn synth(&mut self, text: &str, out: &Path, voice: &str, speed: f32) -> Result<()> {
        // 1. text → IPA phoneme string (pure Rust; OOV words are spelled out, never espeak).
        let (phonemes, _tokens) = self
            .g2p
            .g2p(text)
            .map_err(|e| anyhow!("g2p failed for {text:?}: {e:?}"))?;

        // 2. phonemes → Kokoro token ids (chars outside the vocab are dropped).
        let vocab = kokoro_vocab();
        let ids: Vec<i64> = phonemes
            .chars()
            .filter_map(|c| vocab.get(&c).copied())
            .take(MAX_PHONEMES)
            .collect();
        ensure!(!ids.is_empty(), "no pronounceable phonemes for {text:?}");

        // 3. voice style vector — Kokoro indexes the pack by phoneme count, clamped.
        let styles = self
            .voices
            .get(voice)
            .with_context(|| format!("voice {voice} not in the voice pack"))?;
        let style = styles[ids.len().min(styles.len() - 1)];

        // 4. tokens: [0, ...ids, 0]  (i64 [1, L+2]). ort's (shape, &[T]) tuple form keeps
        //    us ndarray-free, so there's no version coupling to ort's internal ndarray.
        let seq_len = ids.len() + 2;
        let mut tokens = vec![0i64; seq_len];
        tokens[1..seq_len - 1].copy_from_slice(&ids);
        let speed_buf = [speed]; // f32 [1] — thewh1teagle Kokoro export takes float speed

        // 5. run: our model's inputs are named tokens / style / speed; output is `audio`.
        let outputs = self.session.run(inputs![
            "tokens" => TensorRef::from_array_view(([1_i64, seq_len as i64], tokens.as_slice()))?,
            "style" => TensorRef::from_array_view(([1_i64, STYLE_DIM as i64], &style[..]))?,
            "speed" => TensorRef::from_array_view(([1_i64], &speed_buf[..]))?,
        ])?;
        let (_, audio) = outputs.iter().next().context("model produced no output")?;
        let (_shape, samples) = audio.try_extract_tensor::<f32>()?;

        // 6. cache as a mono WAV for instant replay via the earphone gate.
        write_wav(out, samples, SAMPLE_RATE)
            .with_context(|| format!("writing clip {}", out.display()))?;
        Ok(())
    }
}

/// The Kokoro phoneme→token-id vocab (config.json's `vocab`, the standard v1.0 map).
/// Maps IPA/punctuation characters that misaki-rs emits to model token ids.
fn kokoro_vocab() -> &'static HashMap<char, i64> {
    static VOCAB: OnceLock<HashMap<char, i64>> = OnceLock::new();
    VOCAB.get_or_init(|| {
        [
            (';', 1), (':', 2), (',', 3), ('.', 4), ('!', 5), ('?', 6), ('—', 9),
            ('…', 10), ('"', 11), ('(', 12), (')', 13), ('\u{201c}', 14), ('\u{201d}', 15),
            (' ', 16), ('\u{0303}', 17), ('ʣ', 18), ('ʥ', 19), ('ʦ', 20), ('ʨ', 21),
            ('ᵝ', 22), ('ꭧ', 23), ('A', 24), ('I', 25), ('O', 31), ('Q', 33), ('S', 35),
            ('T', 36), ('W', 39), ('Y', 41), ('ᵊ', 42), ('a', 43), ('b', 44), ('c', 45),
            ('d', 46), ('e', 47), ('f', 48), ('h', 50), ('i', 51), ('j', 52), ('k', 53),
            ('l', 54), ('m', 55), ('n', 56), ('o', 57), ('p', 58), ('q', 59), ('r', 60),
            ('s', 61), ('t', 62), ('u', 63), ('v', 64), ('w', 65), ('x', 66), ('y', 67),
            ('z', 68), ('ɑ', 69), ('ɐ', 70), ('ɒ', 71), ('æ', 72), ('β', 75), ('ɔ', 76),
            ('ɕ', 77), ('ç', 78), ('ɖ', 80), ('ð', 81), ('ʤ', 82), ('ə', 83), ('ɚ', 85),
            ('ɛ', 86), ('ɜ', 87), ('ɟ', 90), ('ɡ', 92), ('ɥ', 99), ('ɨ', 101), ('ɪ', 102),
            ('ʝ', 103), ('ɯ', 110), ('ɰ', 111), ('ŋ', 112), ('ɳ', 113), ('ɲ', 114),
            ('ɴ', 115), ('ø', 116), ('ɸ', 118), ('θ', 119), ('œ', 120), ('ɹ', 123),
            ('ɾ', 125), ('ɻ', 126), ('ʁ', 128), ('ɽ', 129), ('ʂ', 130), ('ʃ', 131),
            ('ʈ', 132), ('ʧ', 133), ('ʊ', 135), ('ʋ', 136), ('ʌ', 138), ('ɣ', 139),
            ('ɤ', 140), ('χ', 142), ('ʎ', 143), ('ʒ', 147), ('ʔ', 148), ('ˈ', 156),
            ('ˌ', 157), ('ː', 158), ('ʰ', 162), ('ʲ', 164), ('↓', 169), ('→', 171),
            ('↗', 172), ('↘', 173), ('ᵻ', 177),
        ]
        .into_iter()
        .collect()
    })
}

/// Load `voices-v1.0.bin` — despite the name it is a numpy `.npz` (a ZIP of
/// `<voice>.npy`). Each voice is an `[N, 256]` f32 matrix of style vectors.
fn load_voices(path: &Path) -> Result<HashMap<String, Vec<[f32; STYLE_DIM]>>> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut voices = HashMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry
            .name()
            .trim_end_matches('/')
            .trim_end_matches(".npy")
            .to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        if let Ok(vectors) = parse_npy(&data) {
            voices.insert(name, vectors);
        }
    }
    ensure!(!voices.is_empty(), "no voices parsed from the pack");
    Ok(voices)
}

/// Minimal parser for a C-order little-endian `<f4` `.npy` reshaped to `[_, 256]`.
fn parse_npy(data: &[u8]) -> Result<Vec<[f32; STYLE_DIM]>> {
    ensure!(data.len() > 10 && &data[0..6] == b"\x93NUMPY", "not a .npy");
    let header_len = u16::from_le_bytes([data[8], data[9]]) as usize;
    let body = &data[10 + header_len..];
    ensure!(body.len() % 4 == 0, "npy body not f32-aligned");
    let floats = body.len() / 4;
    ensure!(
        floats % STYLE_DIM == 0,
        "npy not a multiple of the {STYLE_DIM}-d style"
    );
    let rows = floats / STYLE_DIM;
    let mut out = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut vec = [0f32; STYLE_DIM];
        for (c, slot) in vec.iter_mut().enumerate() {
            let o = (r * STYLE_DIM + c) * 4;
            *slot = f32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        }
        out.push(vec);
    }
    Ok(out)
}

/// Write mono 16-bit PCM WAV — small on disk and decodable by rodio for playback.
fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}
