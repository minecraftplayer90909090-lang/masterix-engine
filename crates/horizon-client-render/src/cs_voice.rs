//! CS Engine voice chat, the audio half: the microphone, the codec the
//! frames travel in, and the voices of other players mixed into the game's
//! own output, in the world where they stand (proximity, stereo).
//!
//! Voice travels at 16 kHz mono in 20 ms frames (320 samples), as IMA ADPCM
//! (4 bits a sample: 160 bytes a frame, 64 kbit/s a speaker) — pure Rust, no
//! native codec to build, and small enough for the game connection that
//! carries it (so it goes wherever the world does: e4mc, LAN, a server).
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

/// Sample rate voice travels at.
pub const RATE: u32 = 16_000;
/// Samples a frame (20 ms).
pub const FRAME: usize = 320;
/// Bytes an encoded frame takes: predictor (2), index (1), 160 nibble pairs.
pub const FRAME_BYTES: usize = 3 + FRAME / 2;

// ---- IMA ADPCM ---------------------------------------------------------

const INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408,
    449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630,
    9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

/// The codec's running state; a frame carries its own, so a lost frame hurts
/// only itself.
#[derive(Clone, Copy, Default)]
struct Adpcm {
    predictor: i32,
    index: i32,
}
impl Adpcm {
    fn encode_sample(&mut self, sample: i16) -> u8 {
        let step = STEP_TABLE[self.index as usize];
        let mut diff = i32::from(sample) - self.predictor;
        let mut code = 0u8;
        if diff < 0 {
            code = 8;
            diff = -diff;
        }
        let mut delta = step >> 3;
        if diff >= step {
            code |= 4;
            diff -= step;
            delta += step;
        }
        if diff >= step >> 1 {
            code |= 2;
            diff -= step >> 1;
            delta += step >> 1;
        }
        if diff >= step >> 2 {
            code |= 1;
            delta += step >> 2;
        }
        if code & 8 != 0 {
            self.predictor -= delta;
        } else {
            self.predictor += delta;
        }
        self.predictor = self.predictor.clamp(-32768, 32767);
        self.index = (self.index + i32::from(INDEX_TABLE[code as usize])).clamp(0, 88);
        code
    }
    fn decode_sample(&mut self, code: u8) -> i16 {
        let step = STEP_TABLE[self.index as usize];
        let mut delta = step >> 3;
        if code & 4 != 0 {
            delta += step;
        }
        if code & 2 != 0 {
            delta += step >> 1;
        }
        if code & 1 != 0 {
            delta += step >> 2;
        }
        if code & 8 != 0 {
            self.predictor -= delta;
        } else {
            self.predictor += delta;
        }
        self.predictor = self.predictor.clamp(-32768, 32767);
        self.index = (self.index + i32::from(INDEX_TABLE[(code & 15) as usize])).clamp(0, 88);
        self.predictor as i16
    }
}

/// A speaker's encoder: the codec's state carries from one frame to the
/// next, and each frame writes the state it starts from into its header, so
/// the decoder needs nothing but the frame (a lost one hurts only itself)
/// while the step size no longer restarts from the smallest at every 20 ms
/// (that restart was audible as a rasp on every frame edge).
#[derive(Clone, Copy, Default)]
pub struct Encoder {
    st: Adpcm,
}
impl Encoder {
    /// One frame of 320 samples as 163 bytes.
    pub fn encode(&mut self, samples: &[i16; FRAME]) -> [u8; FRAME_BYTES] {
        let mut out = [0u8; FRAME_BYTES];
        out[0..2].copy_from_slice(&(self.st.predictor as i16).to_be_bytes());
        out[2] = self.st.index as u8;
        for (i, pair) in samples.chunks(2).enumerate() {
            let hi = self.st.encode_sample(pair[0]);
            let lo = self.st.encode_sample(pair[1]);
            out[3 + i] = (hi << 4) | lo;
        }
        out
    }
    /// Silence passed: start the next frame afresh.
    pub fn reset(&mut self) {
        self.st = Adpcm::default();
    }
}

/// One frame of 320 samples as 163 bytes, from a fresh encoder.
pub fn encode_frame(samples: &[i16; FRAME]) -> [u8; FRAME_BYTES] {
    Encoder {
        st: Adpcm {
            predictor: i32::from(samples[0]),
            index: 0,
        },
    }
    .encode(samples)
}

/// A frame's 320 samples; `None` for a slice of the wrong size.
pub fn decode_frame(bytes: &[u8]) -> Option<[i16; FRAME]> {
    if bytes.len() != FRAME_BYTES {
        return None;
    }
    let mut st = Adpcm {
        predictor: i32::from(i16::from_be_bytes([bytes[0], bytes[1]])),
        index: i32::from(bytes[2].min(88)),
    };
    let mut out = [0i16; FRAME];
    for (i, b) in bytes[3..].iter().enumerate() {
        out[2 * i] = st.decode_sample(b >> 4);
        out[2 * i + 1] = st.decode_sample(b & 15);
    }
    Some(out)
}

// ---- Playback: other players' voices in the mixer -----------------------

/// A player's voice as it plays: a queue of samples at [`RATE`], where they
/// stand, and how far they carry.
struct Source {
    queue: VecDeque<f32>,
    at: [f32; 3],
    range: f32,
    /// Playing, or filling up to `START` first (a jitter buffer).
    playing: bool,
    /// Fractional position between `prev` and the queue's front.
    frac: f64,
    prev: f32,
    /// The queue ran dry: the last sample eases to silence instead of
    /// stopping dead (a click at every gap).
    fading: bool,
    fed: Instant,
}
/// Samples queued before a voice starts (100 ms): network jitter this large is
/// absorbed in silence rather than heard as gaps.
const START: usize = FRAME * 5;
/// A voice heard from everywhere at full volume (a group): its range is 0.
pub const GROUP_RANGE: f32 = 0.0;
/// Samples queued at most (400 ms): past it the oldest go, so a slow link
/// never builds a lag that outlives the sentence.
const CAP: usize = FRAME * 20;

/// The voices playing, part of the game's mixer.
pub struct VoiceMix {
    sources: HashMap<u128, Source>,
    /// Everyone's voices, 0 to 2.
    pub volume: f32,
}
impl Default for VoiceMix {
    fn default() -> Self {
        Self {
            sources: HashMap::new(),
            volume: 1.0,
        }
    }
}
impl VoiceMix {
    /// Samples of `who` (at [`RATE`]) speaking from `at`, heard up to `range`
    /// blocks away.
    pub fn push(&mut self, who: u128, at: [f32; 3], range: f32, samples: &[i16]) {
        let s = self.sources.entry(who).or_insert_with(|| Source {
            queue: VecDeque::with_capacity(CAP),
            at,
            range,
            playing: false,
            frac: 0.0,
            prev: 0.0,
            fading: false,
            fed: Instant::now(),
        });
        s.at = at;
        s.range = range;
        s.fed = Instant::now();
        s.queue
            .extend(samples.iter().map(|&v| f32::from(v) / 32768.0));
        while s.queue.len() > CAP {
            s.queue.pop_front();
        }
        if !s.playing && s.queue.len() >= START {
            s.playing = true;
        }
    }
    /// Who is heard right now (fed within the last 300 ms).
    pub fn speaking(&self) -> Vec<u128> {
        self.sources
            .iter()
            .filter(|(_, s)| s.fed.elapsed().as_millis() < 300)
            .map(|(id, _)| *id)
            .collect()
    }
    /// A player left: their voice goes.
    pub fn drop_source(&mut self, who: u128) {
        self.sources.remove(&who);
    }
    pub fn clear(&mut self) {
        self.sources.clear();
    }
    /// Mix the voices into `data` (interleaved, `channels` wide, at `rate`),
    /// heard from `listener` facing `basis` (forward, up, right): linear
    /// distance attenuation to each voice's range, panned by direction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mix(
        &mut self,
        data: &mut [f32],
        channels: usize,
        rate: u32,
        listener: [f64; 3],
        basis: ([f64; 3], [f64; 3], [f64; 3]),
        distance_gain: impl Fn(Option<f32>, f32) -> f32,
        stereo_gains: impl Fn(f32, f32, f32) -> [f32; 2],
    ) {
        let (forward, up, right) = basis;
        let dot = |a: [f64; 3], b: [f64; 3]| (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) as f32;
        let step = f64::from(RATE) / f64::from(rate.max(1));
        let volume = self.volume.clamp(0.0, 2.0);
        // Voices idle for a while are forgotten.
        self.sources
            .retain(|_, s| {
                s.playing || s.fading || !s.queue.is_empty() || s.fed.elapsed().as_secs() < 30
            });
        for s in self.sources.values_mut() {
            if !s.playing && !s.fading {
                continue;
            }
            let d = [
                f64::from(s.at[0]) - listener[0],
                f64::from(s.at[1]) - listener[1],
                f64::from(s.at[2]) - listener[2],
            ];
            let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32;
            // A group voice (range 0) is heard everywhere, in the middle, at
            // full volume; a proximity one falls off to its range, panned.
            let group = s.range <= GROUP_RANGE;
            let gain = if group {
                volume
            } else {
                volume * distance_gain(Some(s.range), distance)
            };
            // Near the listener a voice sits in the middle: no hard panning
            // when a friend stands right beside.
            let [mut l, mut r] = if group {
                [1.0, 1.0]
            } else {
                stereo_gains(dot(d, right), dot(d, up), dot(d, forward))
            };
            let near = (distance / 4.0).clamp(0.0, 1.0);
            let mid = (l + r) / 2.0;
            l = mid + (l - mid) * near;
            r = mid + (r - mid) * near;
            for frame in data.chunks_mut(channels) {
                // Advance to the output's position in the voice's samples.
                while s.playing && s.frac >= 1.0 {
                    match s.queue.pop_front() {
                        Some(v) => {
                            s.prev = v;
                            s.frac -= 1.0;
                        }
                        None => {
                            s.playing = false;
                            s.fading = s.prev.abs() > 1e-4;
                            s.frac = 0.0;
                        }
                    }
                }
                let v = if s.playing {
                    let next = s.queue.front().copied().unwrap_or(s.prev);
                    s.prev + (next - s.prev) * s.frac as f32
                } else if s.fading {
                    // Ease the last sample out over a few milliseconds.
                    s.prev *= 0.96;
                    if s.prev.abs() <= 1e-4 {
                        s.fading = false;
                    }
                    s.prev
                } else {
                    break;
                };
                if channels >= 2 {
                    frame[0] += v * gain * l;
                    frame[1] += v * gain * r;
                } else {
                    frame[0] += v * gain;
                }
                if s.playing {
                    s.frac += step;
                }
            }
        }
    }
}

// ---- Capture: the microphone --------------------------------------------

/// What the input callback collects: mono samples at the device's rate.
#[cfg_attr(any(not(feature = "audio"), target_os = "android"), allow(dead_code))]
struct MicBuf {
    samples: Vec<f32>,
    rate: u32,
    /// Resampler carry between takes.
    frac: f64,
    prev: f32,
    /// Peak of the last take, 0 to 1 (a level meter).
    level: f32,
}

/// The open microphone, feeding [`Mic::take`].
#[cfg(all(feature = "audio", not(target_os = "android")))]
pub struct Mic {
    _stream: cpal::Stream,
    buf: std::sync::Arc<std::sync::Mutex<MicBuf>>,
    pub device: String,
}
#[cfg(all(feature = "audio", not(target_os = "android")))]
impl Mic {
    /// The system's default input, in its default configuration; an error
    /// when there is none or it cannot be opened (no permission yet).
    pub fn open() -> Result<Self, String> {
        use cpal::SampleFormat as F;
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "no microphone".to_owned())?;
        let config = device
            .default_input_config()
            .map_err(|e| format!("microphone config: {e}"))?;
        let buf = std::sync::Arc::new(std::sync::Mutex::new(MicBuf {
            samples: Vec::with_capacity(8192),
            rate: config.sample_rate().0,
            frac: 0.0,
            prev: 0.0,
            level: 0.0,
        }));
        let stream = match config.sample_format() {
            F::F32 => build_input::<f32>(&device, &config, buf.clone()),
            F::I16 => build_input::<i16>(&device, &config, buf.clone()),
            F::I32 => build_input::<i32>(&device, &config, buf.clone()),
            F::U16 => build_input::<u16>(&device, &config, buf.clone()),
            F::I8 => build_input::<i8>(&device, &config, buf.clone()),
            F::F64 => build_input::<f64>(&device, &config, buf.clone()),
            other => Err(format!("microphone format {other:?} unsupported")),
        }?;
        use cpal::traits::StreamTrait;
        stream.play().map_err(|e| format!("microphone start: {e}"))?;
        Ok(Self {
            _stream: stream,
            buf,
            device: device.name().unwrap_or_default(),
        })
    }
    /// Everything captured since the last take, resampled to [`RATE`] mono,
    /// with `gain` applied.
    pub fn take(&self, gain: f32) -> Vec<i16> {
        let Ok(mut b) = self.buf.lock() else {
            return Vec::new();
        };
        take_resampled(&mut b, gain)
    }
    /// The last take's peak, 0 to 1.
    pub fn level(&self) -> f32 {
        self.buf.lock().map(|b| b.level).unwrap_or(0.0)
    }
    /// Why the microphone stopped working, if it did.
    pub fn problem(&self) -> Option<String> {
        None
    }
}
/// Android: the host app records (`AudioRecord`, voice communication preset
/// with echo cancellation) and pushes 16 kHz mono samples through
/// `android_runtime`; opening here only asks it to start.
#[cfg(target_os = "android")]
pub struct Mic {
    pub device: String,
    opened: Instant,
}
#[cfg(target_os = "android")]
impl Mic {
    pub fn open() -> Result<Self, String> {
        crate::android_runtime::host_mic_want(true);
        Ok(Self {
            device: "host microphone".into(),
            opened: Instant::now(),
        })
    }
    pub fn take(&self, gain: f32) -> Vec<i16> {
        let mut s = crate::android_runtime::host_mic_take();
        if (gain - 1.0).abs() > 0.01 {
            for v in &mut s {
                *v = (soft_clip(f32::from(*v) / 32768.0 * gain) * 32767.0) as i16;
            }
        }
        s
    }
    pub fn level(&self) -> f32 {
        crate::android_runtime::host_mic_level()
    }
    /// The host reported it cannot record (no permission, no device), or
    /// never started within 6 s.
    pub fn problem(&self) -> Option<String> {
        match crate::android_runtime::host_mic_state() {
            2 => Some("microphone not available (permission?)".into()),
            0 if self.opened.elapsed().as_secs() > 6 => Some("microphone did not start".into()),
            _ => None,
        }
    }
}
#[cfg(target_os = "android")]
impl Drop for Mic {
    fn drop(&mut self) {
        crate::android_runtime::host_mic_want(false);
    }
}
#[cfg(all(feature = "audio", not(target_os = "android")))]
fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    buf: std::sync::Arc<std::sync::Mutex<MicBuf>>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    let channels = config.channels().max(1) as usize;
    device
        .build_input_stream(
            &config.config(),
            move |data: &[T], _| {
                if let Ok(mut b) = buf.lock() {
                    // Mono: the channels averaged. Capped at a second: a game
                    // that stopped taking must not grow it without end.
                    for frame in data.chunks(channels) {
                        let mut v = 0.0f32;
                        for s in frame {
                            v += <f32 as cpal::FromSample<T>>::from_sample_(*s);
                        }
                        b.samples.push(v / channels as f32);
                    }
                    let cap = b.rate as usize;
                    if b.samples.len() > cap {
                        let extra = b.samples.len() - cap;
                        b.samples.drain(..extra);
                    }
                }
            },
            |e| log::warn!("Masterix voice: microphone error: {e}"),
            None,
        )
        .map_err(|e| format!("microphone open: {e}"))
}
/// The buffer drained into samples at [`RATE`] (linear), the level noted.
#[cfg_attr(any(not(feature = "audio"), target_os = "android"), allow(dead_code))]
fn take_resampled(b: &mut MicBuf, gain: f32) -> Vec<i16> {
    let src: Vec<f32> = std::mem::take(&mut b.samples);
    if src.is_empty() {
        b.level *= 0.8;
        return Vec::new();
    }
    let step = f64::from(b.rate.max(1)) / f64::from(RATE);
    let mut out = Vec::with_capacity((src.len() as f64 / step) as usize + 2);
    let mut peak = 0.0f32;
    // `frac` is the position in `src` of the next output sample, where -1..0
    // means between `prev` (the last sample of the previous take) and src[0].
    let mut pos = b.frac - 1.0;
    loop {
        let i = pos.floor();
        let t = (pos - i) as f32;
        let a = if i < 0.0 { b.prev } else { src[i as usize] };
        let next_i = i as isize + 1;
        if next_i >= src.len() as isize {
            break;
        }
        let c = src[next_i as usize];
        let v = (a + (c - a) * t) * gain;
        peak = peak.max(v.abs());
        out.push((soft_clip(v) * 32767.0) as i16);
        pos += step;
    }
    b.prev = src[src.len() - 1];
    b.frac = pos - (src.len() as f64 - 1.0);
    b.level = peak.min(1.0);
    out
}
/// Without the `audio` feature there is no microphone.
#[cfg(all(not(feature = "audio"), not(target_os = "android")))]
pub struct Mic {
    pub device: String,
}
#[cfg(all(not(feature = "audio"), not(target_os = "android")))]
impl Mic {
    pub fn open() -> Result<Self, String> {
        Err("built without audio".into())
    }
    pub fn take(&self, _gain: f32) -> Vec<i16> {
        Vec::new()
    }
    pub fn level(&self) -> f32 {
        0.0
    }
    pub fn problem(&self) -> Option<String> {
        None
    }
}

/// Gain without the crunch: linear to 0.8, then a soft knee that never
/// reaches 1 (a loud word distorts gently instead of squaring off).
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let over = a - KNEE;
    let soft = KNEE + (1.0 - KNEE) * (over / ((1.0 - KNEE) + over));
    soft.copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoder_state_carries_and_each_frame_still_stands_alone() {
        let mut samples = [0i16; FRAME];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = ((i as f32 * 0.3).sin() * 20000.0) as i16;
        }
        let mut enc = Encoder::default();
        let first = enc.encode(&samples);
        let second = enc.encode(&samples);
        assert_eq!(first[2], 0, "a fresh encoder starts at step 0");
        assert!(second[2] > 0, "the step carries into the next frame");
        let mean_err = |dec: &[i16; FRAME]| {
            samples
                .iter()
                .zip(dec)
                .map(|(a, b)| f64::from(*a - *b).abs())
                .sum::<f64>()
                / FRAME as f64
        };
        // The second frame decodes on its own, closer than a restart would be.
        let err = mean_err(&decode_frame(&second).expect("a frame"));
        let err_restart = mean_err(&decode_frame(&encode_frame(&samples)).expect("a frame"));
        assert!(err < err_restart, "carried {err} vs restart {err_restart}");
        assert!((soft_clip(0.5) - 0.5).abs() < 1e-6);
        assert!(soft_clip(3.0) < 1.0 && soft_clip(3.0) > 0.9);
        assert!(soft_clip(-3.0) > -1.0);
    }

    #[test]
    fn a_group_voice_is_heard_anywhere_and_a_gap_fades_out() {
        let mut m = VoiceMix::default();
        let frame = [8000i16; FRAME];
        for _ in 0..5 {
            m.push(9, [500.0, 0.0, 0.0], GROUP_RANGE, &frame);
        }
        let mut out = vec![0.0f32; 64];
        let id = ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        let dg = |_: Option<f32>, _: f32| 0.0;
        let sg = |_: f32, _: f32, _: f32| [0.0, 0.0];
        m.mix(&mut out, 2, 16_000, [0.0; 3], id, dg, sg);
        assert!(
            out[4..].iter().all(|v| (*v - 8000.0 / 32768.0).abs() < 1e-3),
            "full, centred, 500 blocks away"
        );
        // Drain the queue: the tail eases down instead of cutting.
        let mut big = vec![0.0f32; 2 * (FRAME * 5 - 40)];
        m.mix(&mut big, 2, 16_000, [0.0; 3], id, dg, sg);
        let mut tail = vec![0.0f32; 64];
        m.mix(&mut tail, 2, 16_000, [0.0; 3], id, dg, sg);
        assert!((tail[0] - 8000.0 / 32768.0).abs() < 1e-3, "{}", tail[0]);
        assert!(tail[62] > 0.0 && tail[62] < tail[0] * 0.5, "{}", tail[62]);
    }

    #[test]
    fn adpcm_round_trips_a_tone() {
        let mut samples = [0i16; FRAME];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = ((i as f32 * 0.1).sin() * 12000.0) as i16;
        }
        let enc = encode_frame(&samples);
        let dec = decode_frame(&enc).expect("a frame");
        let err: f64 = samples
            .iter()
            .zip(&dec)
            .map(|(a, b)| f64::from(*a - *b).abs())
            .sum::<f64>()
            / FRAME as f64;
        assert!(err < 400.0, "mean error {err}");
        assert!(decode_frame(&enc[..10]).is_none());
    }

    #[test]
    fn resampler_keeps_the_rate() {
        let mut b = MicBuf {
            samples: (0..4800).map(|i| (i as f32 * 0.01).sin()).collect(),
            rate: 48_000,
            frac: 0.0,
            prev: 0.0,
            level: 0.0,
        };
        let out = take_resampled(&mut b, 1.0);
        assert!((out.len() as i64 - 1600).abs() <= 2, "{}", out.len());
        assert!(b.level > 0.5);
    }

    #[test]
    fn voice_waits_then_plays_and_attenuates() {
        let mut m = VoiceMix::default();
        let frame = [8000i16; FRAME];
        m.push(7, [0.0, 0.0, 0.0], 16.0, &frame);
        let mut out = vec![0.0f32; 64];
        let id = ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        let dg = |max: Option<f32>, d: f32| max.map_or(1.0, |m| (1.0 - (d / m).min(1.0)).max(0.0));
        let sg = |_: f32, _: f32, _: f32| [1.0, 1.0];
        m.mix(&mut out, 2, 16_000, [0.0; 3], id, dg, sg);
        assert!(out.iter().all(|v| *v == 0.0), "fills first");
        for _ in 0..4 {
            m.push(7, [0.0, 0.0, 0.0], 16.0, &frame);
        }
        m.mix(&mut out, 2, 16_000, [0.0; 3], id, dg, sg);
        assert!(out.iter().any(|v| *v > 0.1));
        assert_eq!(m.speaking(), vec![7]);
        let mut far = vec![0.0f32; 64];
        m.mix(&mut far, 2, 16_000, [40.0, 0.0, 0.0], id, dg, sg);
        assert!(far.iter().all(|v| *v == 0.0), "out of range is silent");
    }
}
