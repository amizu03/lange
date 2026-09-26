use std::collections::VecDeque;

pub const WHISPER_SAMPLE_RATE: u32 = 16000;

// VAD tuning constants
const VAD_FRAME_SECS: f32 = 0.100; // 20ms analysis frames
const NOISE_EMA_ALPHA: f32 = 0.45; // how fast noise floor tracks down (silence only)
const THRESHOLD_RATIO: f32 = 2.0; // threshold = noise_floor * ratio
const PRE_ROLL_SECS: f32 = 0.08; // audio to prepend before speech onset
pub const SILENCE_HOLD_SECS: f32 = 0.5; // silence duration that ends a chunk
pub const MAX_CHUNK_SECS: f32 = 15.0; // hard cap on chunk length
pub const MIN_SPEECH_SECS: f32 = 0.5; // discard chunks shorter than this

pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: u32,
}

pub fn to_mono(samples: &[f32], channels: u32) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }

    samples
        .chunks(channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

// Linear interpolation resample — sufficient quality for speech recognition.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    (0..output_len)
        .map(|i| {
            let src_pos = i as f64 * ratio;
            let src_idx = src_pos as usize;
            let frac = (src_pos - src_idx as f64) as f32;
            let s0 = samples[src_idx];
            let s1 = samples.get(src_idx + 1).copied().unwrap_or(s0);
            s0 + (s1 - s0) * frac
        })
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Voice-activity-detection buffer. Feed raw interleaved f32 samples;
/// get back speech chunks whenever a pause is detected.
pub struct VadBuffer {
    frame_size: usize,
    noise_floor: f32,
    is_speaking: bool,
    silence_frames: usize,
    speech_frames: usize,
    silence_hold_frames: usize,
    max_chunk_frames: usize,
    min_speech_frames: usize,
    buffer: Vec<f32>,
    pre_roll: VecDeque<f32>,
    pre_roll_size: usize,
    frame_acc: Vec<f32>,
}

impl VadBuffer {
    pub fn new(sample_rate: u32, channels: u32) -> Self {
        let sps = sample_rate as f32 * channels as f32;
        let frame_size = ((sps * VAD_FRAME_SECS) as usize).max(1);

        VadBuffer {
            frame_size,
            noise_floor: 0.01,
            is_speaking: false,
            silence_frames: 0,
            speech_frames: 0,
            silence_hold_frames: (SILENCE_HOLD_SECS / VAD_FRAME_SECS) as usize,
            max_chunk_frames: (MAX_CHUNK_SECS / VAD_FRAME_SECS) as usize,
            min_speech_frames: (MIN_SPEECH_SECS / VAD_FRAME_SECS) as usize,
            buffer: Vec::new(),
            pre_roll: VecDeque::new(),
            pre_roll_size: (sps * PRE_ROLL_SECS) as usize,
            frame_acc: Vec::new(),
        }
    }

    /// Push raw samples. Returns any complete speech chunks detected.
    pub fn push_samples(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
        let mut chunks = Vec::new();
        self.frame_acc.extend_from_slice(samples);

        while self.frame_acc.len() >= self.frame_size {
            let frame: Vec<f32> = self.frame_acc.drain(..self.frame_size).collect();

            if let Some(chunk) = self.process_frame(&frame) {
                chunks.push(chunk);
            }
        }
        chunks
    }

    fn process_frame(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
        let energy = rms(frame);
        let threshold = self.noise_floor * THRESHOLD_RATIO;

        if energy > threshold {
            // println!("E: {energy} {}", self.noise_floor);

            if !self.is_speaking {
                // println!("SPEAK END");
                // Speech onset: prepend buffered pre-roll
                self.is_speaking = true;
                self.speech_frames = 0;
                self.silence_frames = 0;
                let pre: Vec<f32> = self.pre_roll.drain(..).collect();
                self.buffer.extend(pre);
            }

            self.buffer.extend_from_slice(frame);
            self.speech_frames += 1;

            // Hard cap: flush mid-speech if chunk is getting too long
            if self.speech_frames + self.silence_frames >= self.max_chunk_frames {
                self.is_speaking = false;

                let speech_f = std::mem::replace(&mut self.speech_frames, 0);

                self.silence_frames = 0;

                if speech_f >= self.min_speech_frames {
                    return Some(std::mem::take(&mut self.buffer));
                } else {
                    self.buffer.clear();
                }
            }
        } else {
            // Silence: update adaptive noise floor via EMA
            self.noise_floor =
                (self.noise_floor * (1.0 - NOISE_EMA_ALPHA) + energy * NOISE_EMA_ALPHA).max(1e-2);

            // println!("NoE: {energy} {}", self.noise_floor * THRESHOLD_RATIO);

            if self.is_speaking {
                self.buffer.extend_from_slice(frame);
                self.silence_frames += 1;

                // println!(
                //     "SI: {} {}",
                //     self.silence_frames as f32 * VAD_FRAME_SECS,
                //     self.silence_hold_frames as f32 * VAD_FRAME_SECS
                // );

                if self.silence_frames >= self.silence_hold_frames {
                    // println!("SPEAK START");
                    self.is_speaking = false;
                    self.silence_frames = 0;

                    let speech_f = std::mem::replace(&mut self.speech_frames, 0);

                    if speech_f >= self.min_speech_frames {
                        return Some(std::mem::take(&mut self.buffer));
                    } else {
                        self.buffer.clear();
                    }
                }
            } else {
                // Maintain pre-roll ring buffer during silence
                self.pre_roll.extend(frame.iter().copied());

                while self.pre_roll.len() > self.pre_roll_size {
                    self.pre_roll.pop_front();
                }
            }

            // if self.noise_floor < 0.025 {
            //     self.noise_floor = 0.025;
            // }
        }

        None
    }
}
