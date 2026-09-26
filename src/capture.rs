use std::{io::Cursor, mem, sync::mpsc, thread};

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
use pw::{context::ContextBox, main_loop::MainLoopBox, properties::properties};

use crate::audio::{AudioChunk, VadBuffer};

struct UserData {
    format: spa::param::audio::AudioInfoRaw,
    vad: Option<VadBuffer>,
    tx: mpsc::SyncSender<AudioChunk>,
}

pub fn spawn_capture_thread(tx: mpsc::SyncSender<AudioChunk>, node_id: u64, pid: u64) {
    thread::spawn(move || {
        // eprintln!("Attached to Firefox (node {node_id}). Transcribing...");

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
                // eprintln!("capturing rate:{rate} channels:{channels} (VAD mode)");
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
