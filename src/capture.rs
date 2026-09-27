use crate::audio::{AudioChunk, VadBuffer};
use std::{sync::mpsc, thread};

// ── Unix / PipeWire ──────────────────────────────────────────────────────────

#[cfg(unix)]
use std::{io::Cursor, mem};

#[cfg(unix)]
use pipewire::{
    self as pw,
    spa::{
        self,
        param::{
            format::{MediaSubtype, MediaType},
            format_utils,
        },
        pod::Pod,
    },
};

#[cfg(unix)]
use pw::{context::ContextBox, main_loop::MainLoopBox, properties::properties};

#[cfg(unix)]
struct UserData {
    format: pipewire::spa::param::audio::AudioInfoRaw,
    vad: Option<VadBuffer>,
    tx: mpsc::SyncSender<AudioChunk>,
}

#[cfg(unix)]
pub fn spawn_capture_thread(tx: mpsc::SyncSender<AudioChunk>, node_id: u64, pid: u64) {
    thread::spawn(move || {
        let mainloop = MainLoopBox::new(None).expect("failed to create PipeWire main loop");
        let context = ContextBox::new(mainloop.loop_(), None).unwrap();
        let core = context.connect(None).unwrap();

        let mut props = properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::APP_PROCESS_ID => pid.to_string(),
        };
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");

        let stream = pw::stream::StreamBox::new(&core, "audio-capture", props).unwrap();

        let data = UserData {
            format: Default::default(),
            vad: None,
            tx,
        };

        let _listener = stream
            .add_local_listener_with_user_data(data)
            .param_changed(|_, user_data, id, param| {
                let Some(param) = param else { return };
                if id != pw::spa::param::ParamType::Format.as_raw() {
                    return;
                }

                let (media_type, media_subtype) = match format_utils::parse_format(param) {
                    Ok(v) => v,
                    Err(_) => return,
                };

                if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                    return;
                }

                user_data
                    .format
                    .parse(param)
                    .expect("Failed to parse param changed to AudioInfoRaw");

                let rate = user_data.format.rate();
                let channels = user_data.format.channels();
                user_data.vad = Some(VadBuffer::new(rate, channels));
            })
            .process(|stream, user_data| match stream.dequeue_buffer() {
                None => {}
                Some(mut buffer) => {
                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        return;
                    }

                    let data = &mut datas[0];
                    let n_samples = data.chunk().size() / (mem::size_of::<f32>() as u32);

                    if let Some(samples) = data.data() {
                        let mut new_samples = Vec::with_capacity(n_samples as usize);
                        for i in 0..n_samples as usize {
                            let start = i * mem::size_of::<f32>();
                            let end = start + mem::size_of::<f32>();
                            if end <= samples.len() {
                                let f =
                                    f32::from_le_bytes(samples[start..end].try_into().unwrap());
                                new_samples.push(f);
                            }
                        }

                        if let Some(vad) = &mut user_data.vad {
                            for chunk in vad.push_samples(&new_samples) {
                                let _ = user_data.tx.try_send(AudioChunk {
                                    samples: chunk,
                                    rate: user_data.format.rate(),
                                    channels: user_data.format.channels(),
                                });
                            }
                        }
                    }
                }
            })
            .register()
            .unwrap();

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
        let obj = pw::spa::pod::Object {
            type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: pw::spa::param::ParamType::EnumFormat.as_raw(),
            properties: audio_info.into(),
        };
        let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(obj),
        )
        .unwrap()
        .0
        .into_inner();

        let mut params = [Pod::from_bytes(&values).unwrap()];

        stream
            .connect(
                spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT
                    | pw::stream::StreamFlags::MAP_BUFFERS
                    | pw::stream::StreamFlags::RT_PROCESS,
                &mut params,
            )
            .unwrap();

        mainloop.run();
    });
}

// ── Windows / WASAPI loopback ────────────────────────────────────────────────

#[cfg(windows)]
use std::{mem::offset_of, time::Duration};

#[cfg(windows)]
use windows::{
    core::GUID,
    Win32::{
        Media::Audio::{
            eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
            MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
            WAVEFORMATEXTENSIBLE,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
        },
    },
};

// WAVEFORMATEX format tags
#[cfg(windows)]
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
#[cfg(windows)]
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

// {00000003-0000-0010-8000-00aa00389b71}
#[cfg(windows)]
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID = GUID {
    data1: 0x0000_0003,
    data2: 0x0000,
    data3: 0x0010,
    data4: [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
};

/// Capture the system render device in loopback mode (all audio playing on the
/// default output). `_node_id` and `_pid` are accepted for API compatibility
/// but not used on Windows.
#[cfg(windows)]
pub fn spawn_capture_thread(tx: mpsc::SyncSender<AudioChunk>, _node_id: u64, _pid: u64) {
    thread::spawn(move || unsafe {
        // COM must be initialised on this thread before calling any WASAPI API.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .expect("CoCreateInstance IMMDeviceEnumerator");

        // Loopback capture uses the *render* endpoint, not a capture endpoint.
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .expect("GetDefaultAudioEndpoint");

        let audio_client: IAudioClient =
            device.Activate(CLSCTX_ALL, None).expect("Activate IAudioClient");

        // Use the device's own mix format so we always get a supported layout.
        let mix_fmt_ptr = audio_client.GetMixFormat().expect("GetMixFormat");

        let rate = (*mix_fmt_ptr).nSamplesPerSec;
        let channels = (*mix_fmt_ptr).nChannels as u32;
        let bits = (*mix_fmt_ptr).wBitsPerSample;
        let tag = (*mix_fmt_ptr).wFormatTag;

        let is_float = tag == WAVE_FORMAT_IEEE_FLOAT
            || (tag == WAVE_FORMAT_EXTENSIBLE && {
                let ext = mix_fmt_ptr as *const WAVEFORMATEXTENSIBLE;
                let guid = ((ext.addr() + offset_of!(WAVEFORMATEXTENSIBLE, SubFormat)) as *const GUID).read_unaligned();
                guid == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            });

        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                2_000_000, // 200 ms buffer in 100-ns units
                0,
                mix_fmt_ptr,
                None,
            )
            .expect("IAudioClient::Initialize");

        CoTaskMemFree(Some(mix_fmt_ptr as *const _));

        let capture_client: IAudioCaptureClient =
            audio_client.GetService().expect("GetService IAudioCaptureClient");

        audio_client.Start().expect("IAudioClient::Start");

        let mut vad = VadBuffer::new(rate, channels);

        loop {
            let packet_size = match capture_client.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => break,
            };

            if packet_size == 0 {
                thread::sleep(Duration::from_millis(10));
                continue;
            }

            let mut data_ptr = std::ptr::null_mut::<u8>();
            let mut num_frames = 0u32;
            let mut flags = 0u32;

            if capture_client
                .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                .is_err()
            {
                break;
            }

            const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
            let n_samples = num_frames as usize * channels as usize;

            let samples: Vec<f32> = if flags & AUDCLNT_BUFFERFLAGS_SILENT != 0 {
                vec![0.0; n_samples]
            } else if is_float && bits == 32 {
                let ptr = data_ptr as *const f32;
                std::slice::from_raw_parts(ptr, n_samples).to_vec()
            } else if bits == 16 {
                let ptr = data_ptr as *const i16;
                std::slice::from_raw_parts(ptr, n_samples)
                    .iter()
                    .map(|&s| s as f32 / 32_768.0)
                    .collect()
            } else if bits == 32 {
                // PCM 32-bit integer
                let ptr = data_ptr as *const i32;
                std::slice::from_raw_parts(ptr, n_samples)
                    .iter()
                    .map(|&s| s as f32 / 2_147_483_648.0)
                    .collect()
            } else {
                vec![0.0; n_samples]
            };

            let _ = capture_client.ReleaseBuffer(num_frames);

            for chunk in vad.push_samples(&samples) {
                let _ = tx.try_send(AudioChunk {
                    samples: chunk,
                    rate,
                    channels,
                });
            }
        }
    });
}
