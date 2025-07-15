use std::{mem, process::Command, thread::sleep, time::Duration};

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

use pw::{
    context::Context,
    main_loop::MainLoop,
    properties::{Properties, properties},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

struct UserData {
    format: spa::param::audio::AudioInfoRaw,
    cursor_move: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowInfo {
    pub class: String,
    pub title: String,
    pub pid: usize,
}

impl WindowInfo {
    pub fn current() -> Option<Self> {
        let xdg_current_desktop = std::env::var("XDG_CURRENT_DESKTOP").ok()?;

        if xdg_current_desktop.eq_ignore_ascii_case("hyprland") {
            let x = Command::new("hyprctl")
                .arg("-j")
                .arg("activewindow")
                .output()
                .ok()?;

            if x.status.success() {
                let s = String::from_utf8(x.stdout).ok()?;
                serde_json::from_str(&s).ok()
            } else {
                None
            }
        } else {
            None
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pid = 0;

    loop {
        sleep(Duration::from_secs(1));

        if let Some(w) = WindowInfo::current() {
            if w.class == "firefox" {
                pid = w.pid;
                break;
            }
        }
    }

    let mainloop = MainLoop::new(None)?;
    let context = Context::new(&mainloop)?;
    let core = context.connect(None)?;

    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        // *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::APP_PROCESS_ID => pid.to_string(),
    };

    props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    // props.insert(*pw::keys::APP_PROCESS_ID, "true");

    let stream = pw::stream::Stream::new(&core, "audio-capture", props)?;

    let data = UserData {
        format: Default::default(),
        cursor_move: false,
    };

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, user_data, id, param| {
            // NULL means to clear the format
            let Some(param) = param else {
                return;
            };

            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }

            let (media_type, media_subtype) = match format_utils::parse_format(param) {
                Ok(v) => v,
                Err(_) => return,
            };

            // only accept raw audio
            if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                return;
            }

            // call a helper function to parse the format for us.
            user_data
                .format
                .parse(param)
                .expect("Failed to parse param changed to AudioInfoRaw");

            println!(
                "capturing rate:{} channels:{}",
                user_data.format.rate(),
                user_data.format.channels()
            );
        })
        .process(|stream, user_data| match stream.dequeue_buffer() {
            None => println!("out of buffers"),
            Some(mut buffer) => {
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let data = &mut datas[0];
                let n_channels = user_data.format.channels();
                let n_samples = data.chunk().size() / (mem::size_of::<f32>() as u32);

                if let Some(samples) = data.data() {
                    if user_data.cursor_move {
                        print!("\x1B[{}A", n_channels + 1);
                    }
                    println!("captured {} samples", n_samples / n_channels);
                    for c in 0..n_channels {
                        let mut max: f32 = 0.0;
                        for n in (c..n_samples).step_by(n_channels as usize) {
                            let start = n as usize * mem::size_of::<f32>();
                            let end = start + mem::size_of::<f32>();
                            let chan = &samples[start..end];
                            let f = f32::from_le_bytes(chan.try_into().unwrap());
                            max = max.max(f.abs());
                        }

                        let peak = ((max * 30.0) as usize).clamp(0, 39);

                        println!(
                            "channel {}: |{:>w1$}{:w2$}| peak:{}",
                            c,
                            "*",
                            "",
                            max,
                            w1 = peak + 1,
                            w2 = 40 - peak
                        );
                    }
                    user_data.cursor_move = true;
                }
            }
        })
        .register()?;

    /* Make one parameter with the supported formats. The SPA_PARAM_EnumFormat
     * id means that this is a format enumeration (of 1 value).
     * We leave the channels and rate empty to accept the native graph
     * rate and channels. */
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .unwrap()
    .0
    .into_inner();

    let mut params = [Pod::from_bytes(&values).unwrap()];

    /* Now connect this stream. We ask that our process function is
     * called in a realtime thread. */
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // and wait while we let things run
    mainloop.run();

    // let registry = core.get_registry()?;

    println!("Hello, world!");

    Ok(())
}
