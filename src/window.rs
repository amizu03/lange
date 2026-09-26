use std::{
    process::Command,
    thread,
    thread::sleep,
    time::Duration,
};

use eframe::egui::Pos2;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowInfo {
    pub class: String,
    pub title: String,
    pub pid: u64,
}

impl WindowInfo {
    pub fn pipewire_node(&self) -> Option<u64> {
        let s = String::from_utf8(Command::new("pw-dump").output().ok()?.stdout).ok()?;
        let j = serde_json::from_str::<Value>(&s).ok()?;

        for obj in j.as_array().unwrap() {
            if obj["type"] != "PipeWire:Interface:Node" {
                continue;
            }

            let props = &obj["info"]["props"];

            if let Some(pid) = props["application.process.id"].as_u64() {
                if pid == self.pid {
                    return obj["id"].as_u64();
                }
            }
        }

        None
    }

    fn from_hyprland() -> Option<Self> {
        let x = Command::new("hyprctl")
            .args(["-j", "activewindow"])
            .output()
            .ok()?;

        if !x.status.success() {
            return None;
        }

        let s = String::from_utf8(x.stdout).ok()?;
        serde_json::from_str(&s).ok()
    }

    fn from_niri() -> Option<Self> {
        let x = Command::new("niri")
            .args(["msg", "--json", "focused-window"])
            .output()
            .ok()?;

        if !x.status.success() {
            return None;
        }

        let s = String::from_utf8(x.stdout).ok()?;
        let v: Value = serde_json::from_str(&s).ok()?;

        Some(Self {
            class: v.get("app_id")?.as_str()?.to_string(),
            title: v.get("title")?.as_str()?.to_string(),
            pid: v.get("pid")?.as_u64()?,
        })
    }

    fn from_sway() -> Option<Self> {
        let x = Command::new("swaymsg")
            .args(["-t", "get_focused"])
            .output()
            .ok()?;
        if !x.status.success() {
            return None;
        }
        let s = String::from_utf8(x.stdout).ok()?;
        let v: Value = serde_json::from_str(&s).ok()?;
        let class = v
            .get("app_id")
            .and_then(|v| v.as_str())
            .or_else(|| {
                v.pointer("/window_properties/class")
                    .and_then(|v| v.as_str())
            })
            .map(str::to_string)?;
        Some(Self {
            class,
            title: v.get("name")?.as_str()?.to_string(),
            pid: v.get("pid")?.as_u64()?,
        })
    }

    pub fn current() -> Option<Self> {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase();

        if desktop.contains("hyprland") {
            return Self::from_hyprland();
        }

        if desktop.contains("niri") {
            return Self::from_niri();
        }

        if desktop.contains("sway") || desktop.contains("i3") {
            return Self::from_sway();
        }

        Self::from_hyprland()
            .or_else(Self::from_niri)
            .or_else(Self::from_sway)
    }
}

pub fn focused_monitor_rect() -> Option<(f32, f32, f32, f32)> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    if desktop.contains("hyprland") {
        return focused_monitor_rect_hyprland();
    }
    if desktop.contains("niri") {
        return focused_monitor_rect_niri();
    }
    focused_monitor_rect_hyprland().or_else(focused_monitor_rect_niri)
}

pub fn apply_compositor_position(pos: Pos2) {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    if desktop.contains("hyprland") {
        // Wayland disallows apps from setting their own position; use window rules instead.
        let _ = Command::new("hyprctl")
            .args(["keyword", "windowrulev2", "float, title:^(lange)$"])
            .output();
        let move_rule = format!("move {} {}, title:^(lange)$", pos.x as i32, pos.y as i32);
        let _ = Command::new("hyprctl")
            .args(["keyword", "windowrulev2", &move_rule])
            .output();
    } else if desktop.contains("niri") {
        // eframe::run_native() blocks, so we position the window from a background thread.
        thread::spawn(move || {
            // Poll until our window appears.
            let window_id = loop {
                sleep(Duration::from_millis(200));
                let Ok(out) = Command::new("niri")
                    .args(["msg", "--json", "windows"])
                    .output()
                else {
                    continue;
                };
                let Ok(s) = String::from_utf8(out.stdout) else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(&s) else {
                    continue;
                };
                if let Some(id) = v
                    .as_array()
                    .and_then(|arr| arr.iter().find(|w| w["title"].as_str() == Some("lange")))
                    .and_then(|w| w["id"].as_u64())
                {
                    break id;
                }
            };

            let id = window_id.to_string();
            let _ = Command::new("niri")
                .args(["msg", "action", "move-window-to-floating", "--id", &id])
                .output();

            sleep(Duration::from_millis(100));

            let _ = Command::new("niri")
                .args([
                    "msg",
                    "action",
                    "move-floating-window",
                    "--id",
                    &id,
                    "--x",
                    &(pos.x as i32).to_string(),
                    "--y",
                    &(pos.y as i32).to_string(),
                ])
                .output();
        });
    }
}

fn focused_monitor_rect_hyprland() -> Option<(f32, f32, f32, f32)> {
    let out = Command::new("hyprctl")
        .args(["-j", "monitors"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let arr = serde_json::from_str::<Value>(&s).ok()?;
    let arr = arr.as_array()?;
    let mon = arr
        .iter()
        .find(|m| m["focused"].as_bool() == Some(true))
        .or_else(|| arr.first())?;
    let scale = mon["scale"].as_f64().unwrap_or(1.0) as f32;
    let w = mon["width"].as_f64()? as f32 / scale;
    let h = mon["height"].as_f64()? as f32 / scale;
    let x = mon["x"].as_f64().unwrap_or(0.0) as f32;
    let y = mon["y"].as_f64().unwrap_or(0.0) as f32;
    Some((x, y, w, h))
}

fn focused_monitor_rect_niri() -> Option<(f32, f32, f32, f32)> {
    let out = Command::new("niri")
        .args(["msg", "--json", "outputs"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let map = serde_json::from_str::<Value>(&s).ok()?;
    let outputs = map.as_object()?;
    let mon = outputs
        .values()
        .find(|o| o["focused"].as_bool() == Some(true))
        .or_else(|| outputs.values().next())?;
    let logical = &mon["logical"];
    let w = logical["width"].as_f64()? as f32;
    let h = logical["height"].as_f64()? as f32;
    let x = logical["x"].as_f64().unwrap_or(0.0) as f32;
    let y = logical["y"].as_f64().unwrap_or(0.0) as f32;
    Some((x, y, w, h))
}
