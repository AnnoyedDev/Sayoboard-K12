use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hidapi::HidApi;

#[derive(Clone, PartialEq)]
pub struct Sound {
    pub path: String,
    pub sink: String,
    pub volume: f32,
}

impl Default for Sound {
    fn default() -> Self {
        Self {
            path: String::new(),
            sink: String::new(),
            volume: 1.0,
        }
    }
}

pub const MONITOR: u8 = 0;
const SOUNDS: &str = "sons.tsv";

pub type Sounds = Arc<Mutex<BTreeMap<u8, Sound>>>;
pub type Players = Arc<Mutex<Vec<Child>>>;

pub fn config_file(name: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("sayo").join(name)
}

pub fn parse(text: &str) -> BTreeMap<u8, Sound> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(4, '\t');
            let code = f.next()?.parse().ok()?;
            let volume = f.next()?.parse().ok()?;
            let sink = f.next()?.to_string();
            let path = f.next()?.to_string();
            Some((code, Sound { path, sink, volume }))
        })
        .collect()
}

pub fn serialize(map: &BTreeMap<u8, Sound>) -> String {
    map.iter()
        .map(|(c, s)| format!("{c}\t{}\t{}\t{}\n", s.volume, s.sink, s.path))
        .collect()
}

pub fn load() -> BTreeMap<u8, Sound> {
    std::fs::read_to_string(config_file(SOUNDS))
        .map(|t| parse(&t))
        .unwrap_or_default()
}

pub fn write_config(name: &str, content: &str) -> Result<(), String> {
    let p = config_file(name);
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&p, content).map_err(|e| e.to_string())
}

pub fn save(map: &BTreeMap<u8, Sound>) -> Result<(), String> {
    write_config(SOUNDS, &serialize(map))
}

pub fn sinks() -> Vec<(String, String)> {
    let Ok(out) = Command::new("pactl")
        .args(["list", "sinks"])
        .env("LC_ALL", "C")
        .output()
    else {
        return Vec::new();
    };
    let mut list = Vec::new();
    let mut name = None;
    for line in String::from_utf8_lossy(&out.stdout).lines().map(str::trim) {
        if let Some(n) = line.strip_prefix("Name: ") {
            name = Some(n.to_string());
        } else if let (Some(d), Some(n)) = (line.strip_prefix("Description: "), name.take()) {
            list.push((n, d.to_string()));
        }
    }
    list
}

pub fn play(s: &Sound, players: &Players) -> Result<(), String> {
    let mut cmd = Command::new("pw-play");
    cmd.arg(format!("--volume={}", s.volume));
    if !s.sink.is_empty() {
        cmd.arg(format!("--target={}", s.sink));
    }
    let child = cmd
        .arg(&s.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("pw-play: {e}"))?;
    let mut p = players.lock().unwrap();
    p.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    p.push(child);
    Ok(())
}

pub fn trigger(map: &BTreeMap<u8, Sound>, code: u8, players: &Players) -> Result<(), String> {
    let Some(s) = map.get(&code) else {
        return Ok(());
    };
    play(s, players)?;
    match map.get(&MONITOR) {
        Some(m) if m.sink != s.sink => play(
            &Sound {
                sink: m.sink.clone(),
                volume: s.volume * m.volume,
                ..s.clone()
            },
            players,
        ),
        _ => Ok(()),
    }
}

pub fn stop_all(players: &Players) {
    for mut c in players.lock().unwrap().drain(..) {
        let _ = c.kill();
        let _ = c.wait();
    }
}

pub fn pressed(report: &[u8]) -> HashSet<u8> {
    report.iter().skip(2).copied().filter(|c| *c > 3).collect()
}

fn stamp() -> Option<std::time::SystemTime> {
    std::fs::metadata(config_file(SOUNDS))
        .and_then(|m| m.modified())
        .ok()
}

fn acquire_lock() -> std::fs::File {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    loop {
        if let Ok(f) = std::fs::File::create(dir.join("sayo-ecoute.lock")) {
            if f.try_lock().is_ok() {
                return f;
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

pub fn listen(sounds: Sounds, players: Players) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _lock = acquire_lock();
        let mut seen = stamp();
        *sounds.lock().unwrap() = load();
        loop {
            let dev = HidApi::new().ok().and_then(|api| {
                let vendor: Vec<_> = api
                    .device_list()
                    .filter(|d| d.vendor_id() == crate::proto::VID && d.usage_page() == 0xFF00)
                    .map(|d| d.path().to_owned())
                    .collect();
                api.device_list()
                    .find(|d| {
                        d.vendor_id() == crate::proto::VID
                            && d.usage_page() == 1
                            && d.usage() == 6
                            && !vendor.contains(&d.path().to_owned())
                    })
                    .and_then(|d| d.open_device(&api).ok())
            });
            let Some(dev) = dev else {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            };
            let mut before = HashSet::new();
            let mut buf = [0u8; 64];
            while let Ok(n) = dev.read(&mut buf) {
                let now = pressed(&buf[..n]);
                if now.difference(&before).next().is_some() {
                    let mut map = sounds.lock().unwrap();
                    let stamp = stamp();
                    if stamp != seen {
                        seen = stamp;
                        *map = load();
                    }
                    for code in now.difference(&before) {
                        let _ = trigger(&map, *code, &players);
                    }
                }
                before = now;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let mut m = BTreeMap::new();
        m.insert(
            0x1e,
            Sound {
                path: "/tmp/a b.ogg".into(),
                sink: "soundboard".into(),
                volume: 0.5,
            },
        );
        m.insert(
            0x04,
            Sound {
                path: "/x.wav".into(),
                sink: String::new(),
                volume: 1.25,
            },
        );
        m.insert(
            MONITOR,
            Sound {
                path: String::new(),
                sink: String::new(),
                volume: 0.8,
            },
        );
        assert!(parse(&serialize(&m)) == m);
    }

    #[test]
    fn boot_report_keys() {
        assert_eq!(
            pressed(&[0x02, 0, 0x1e, 0x1f, 0, 0, 0, 0]),
            HashSet::from([0x1e, 0x1f])
        );
        assert!(pressed(&[0, 0, 0, 0, 0, 0, 0, 0]).is_empty());
    }
}
