use std::{
    collections::HashMap,
    sync::{mpsc, Arc, Mutex},
    thread,
};

use cccedict::cedict::{Cedict, CedictEntry};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::audio::{resample, to_mono, AudioChunk, WHISPER_SAMPLE_RATE};

// Maximum-munch sliding window: for each position try longest match first, shrink until found.
fn lookup_sentence(trimmed: &str, index: &HashMap<String, usize>) -> Vec<(String, usize)> {
    let chars: Vec<char> = trimmed.chars().collect();
    let mut results = Vec::new();
    let mut pos = 0;

    while pos < chars.len() {
        let mut window_end = chars.len();
        let mut found = false;

        while window_end > pos {
            let candidate: String = chars[pos..window_end].iter().collect();
            if let Some(&idx) = index.get(&candidate) {
                results.push((candidate, idx));
                pos = window_end;
                found = true;
                break;
            }
            window_end -= 1;
        }

        if !found {
            pos += 1;
        }
    }

    results
}

pub fn spawn_transcribe_thread(
    rx: mpsc::Receiver<AudioChunk>,
    model_path: String,
    captions: Arc<Mutex<Vec<Vec<CedictEntry>>>>,
    cedict: Arc<Cedict>,
) {
    thread::spawn(move || {
        let mut index: HashMap<String, usize> = HashMap::with_capacity(cedict.entries.len() * 2);
        for (i, entry) in cedict.entries.iter().enumerate() {
            index.entry(entry.simplified.clone()).or_insert(i);
            index.entry(entry.traditional.clone()).or_insert(i);
        }

        let ctx = WhisperContext::new_with_params(&model_path, WhisperContextParameters::default())
            .expect("failed to load whisper model");
        let mut state = ctx.create_state().expect("failed to create whisper state");

        for chunk in rx {
            let mono = to_mono(&chunk.samples, chunk.channels);
            let resampled = resample(&mono, chunk.rate, WHISPER_SAMPLE_RATE);

            if resampled.is_empty() {
                continue;
            }

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_language(Some("zh"));
            // params.set_n_threads(std::thread::available_parallelism().unwrap().get() as _);

            if state.full(params, &resampled).is_err() {
                continue;
            }

            let n = state.full_n_segments();

            for i in 0..n {
                if let Some(segment) = state.get_segment(i) {
                    if let Ok(text) = segment.to_str() {
                        let trimmed = text.trim();

                        if !trimmed.is_empty() {
                            let matches = lookup_sentence(trimmed, &index);

                            let mut caps = Vec::new();

                            for (word, idx) in &matches {
                                let entry = &cedict.entries[*idx];

                                caps.push(entry.clone());
                            }

                            captions.lock().unwrap().push(caps);
                        }
                    }
                }
            }
        }
    });
}
