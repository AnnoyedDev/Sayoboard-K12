macro_rules! t {
    ($fr:expr, $en:expr, $ja:expr $(,)?) => {
        match $crate::LANG.load(std::sync::atomic::Ordering::Relaxed) {
            0 => $fr,
            1 => $en,
            _ => $ja,
        }
    };
}

mod proto;
mod sound;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use eframe::egui;
use proto::{Device, Key, Led, MODS, SILENT, key_name};
use sound::{Players, Sound, Sounds};

static LANG: AtomicU8 = AtomicU8::new(0);
const LANGS: [(&str, &str); 3] = [("fr", "Français"), ("en", "English"), ("ja", "日本語")];

fn lang_of(code: &str) -> u8 {
    LANGS
        .iter()
        .position(|(c, _)| code.starts_with(c))
        .unwrap_or(1) as u8
}

fn detect_lang() -> u8 {
    let code = std::fs::read_to_string(sound::config_file("lang"))
        .ok()
        .or_else(|| {
            ["LC_ALL", "LC_MESSAGES", "LANG"]
                .iter()
                .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        });
    lang_of(code.as_deref().unwrap_or(""))
}

fn setup(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size.y = 26.0;
    });
    let Ok(out) = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}", "sans:lang=ja"])
        .output()
    else {
        return;
    };
    let out = String::from_utf8_lossy(&out.stdout);
    let Some((file, index)) = out.split_once('\n') else {
        return;
    };
    let Ok(bytes) = std::fs::read(file) else {
        return;
    };
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index.trim().parse().unwrap_or(0);
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".into(), data);
    for family in fonts.families.values_mut() {
        family.push("cjk".into());
    }
    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--fond") {
        let _ = sound::listen(Default::default(), Default::default()).join();
        return Ok(());
    }
    LANG.store(detect_lang(), Relaxed);
    eframe::run_native(
        "Sayoboard-K12",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            setup(&cc.egui_ctx);
            let app = App {
                sinks: sound::sinks(),
                ..Default::default()
            };
            *app.sounds.lock().unwrap() = sound::load();
            sound::listen(app.sounds.clone(), app.players.clone());
            Ok(Box::new(app))
        }),
    )
}

#[derive(Default)]
struct App {
    dev: Option<Device>,
    status: String,
    keys: Vec<Key>,
    leds: Vec<Led>,
    selected: usize,
    layer: usize,
    all_color: [u8; 3],
    sounds: Sounds,
    players: Players,
    sinks: Vec<(String, String)>,
}

fn banner(d: &Device) -> Result<String, String> {
    let i = d.info()?;
    Ok(t!(
        format!(
            "{} — firmware {:#06x}, modèle {:#06x}",
            d.name, i.version, i.model
        ),
        format!(
            "{} — firmware {:#06x}, model {:#06x}",
            d.name, i.version, i.model
        ),
        format!(
            "{} — ファームウェア {:#06x}、モデル {:#06x}",
            d.name, i.version, i.model
        ),
    ))
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).strong().size(16.0));
            body(ui);
        });
}

fn form(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, rows);
}

fn volume(ui: &mut egui::Ui, v: &mut f32) {
    ui.add(egui::Slider::new(v, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
}

fn sink_combo(ui: &mut egui::Ui, id: &str, sink: &mut String, sinks: &[(String, String)]) {
    let default = t!("Sortie par défaut", "Default output", "既定の出力");
    let current = match sinks.iter().find(|(n, _)| n == sink) {
        Some((_, d)) => d.as_str(),
        None if sink.is_empty() => default,
        None => sink.as_str(),
    }
    .to_string();
    egui::ComboBox::from_id_salt(id)
        .width(320.0)
        .selected_text(current)
        .show_ui(ui, |ui| {
            ui.selectable_value(sink, String::new(), default);
            for (n, d) in sinks {
                ui.selectable_value(sink, n.clone(), d);
            }
        });
}

fn pick_sound(current: &str) -> Option<String> {
    let mut dialog = rfd::FileDialog::new()
        .set_title(t!("Choisir un son", "Choose a sound", "サウンドを選択"))
        .add_filter(
            t!("Audio", "Audio", "音声"),
            &["wav", "ogg", "oga", "opus", "flac", "mp3", "m4a", "aiff"],
        )
        .add_filter(
            t!("Tous les fichiers", "All files", "すべてのファイル"),
            &["*"],
        );
    if let Some(dir) = std::path::Path::new(current)
        .parent()
        .filter(|p| p.is_dir())
    {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_file().map(|p| p.display().to_string())
}

fn describe(k: &mut Key, layer: usize) -> String {
    if layer >= k.layers() {
        return "—".into();
    }
    let l = k.layer(layer);
    match l[0] {
        0 => {
            let mut parts: Vec<String> = MODS
                .iter()
                .enumerate()
                .filter(|(b, _)| l[2] & (1 << b) != 0)
                .map(|(_, m)| m.to_string())
                .collect();
            parts.extend(l[3..6].iter().filter(|c| **c != 0).map(|c| key_name(*c)));
            if parts.is_empty() {
                "—".into()
            } else {
                parts.join("+")
            }
        }
        m => format!("{} {m}", t!("mode", "mode", "モード")),
    }
}

impl App {
    fn dev(&self) -> Result<&Device, String> {
        self.dev
            .as_ref()
            .ok_or_else(|| t!("Non connecté", "Not connected", "未接続").into())
    }

    fn connect(&mut self) {
        self.dev = None;
        let r = Device::open().and_then(|d| {
            self.status = banner(&d)?;
            self.dev = Some(d);
            self.reload()
        });
        if let Err(e) = r {
            self.status = e;
        }
    }

    fn reload(&mut self) -> Result<(), String> {
        let d = self.dev()?;
        let (keys, leds) = (d.keys()?, d.leds()?);
        self.keys = keys;
        self.leds = leds;
        self.selected = self.selected.min(self.keys.len().saturating_sub(1));
        Ok(())
    }

    fn run(&mut self, f: impl FnOnce(&mut Self) -> Result<String, String>) {
        self.status = f(self).unwrap_or_else(|e| {
            t!(
                format!("Erreur : {e}"),
                format!("Error: {e}"),
                format!("エラー: {e}")
            )
        });
    }

    fn set_lang(&mut self, lang: u8) {
        LANG.store(lang, Relaxed);
        let _ = sound::write_config("lang", LANGS[lang as usize].0);
        if let Some(Ok(s)) = self.dev.as_ref().map(banner) {
            self.status = s;
        }
    }

    fn save_sounds(&mut self, map: &BTreeMap<u8, Sound>) {
        if let Err(e) = sound::save(map) {
            self.status = t!(
                format!("Erreur sauvegarde des sons : {e}"),
                format!("Could not save sounds: {e}"),
                format!("サウンドの保存に失敗しました: {e}"),
            );
        }
    }

    fn top(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(t!("🔌 Connecter", "🔌 Connect", "🔌 接続"))
                .clicked()
            {
                self.connect();
            }
            ui.add_enabled_ui(self.dev.is_some(), |ui| {
                if ui
                    .button(t!("⟳ Relire", "⟳ Reload", "⟳ 再読み込み"))
                    .clicked()
                {
                    self.run(|s| {
                        s.reload()?;
                        Ok(t!(
                            "Configuration relue",
                            "Configuration reloaded",
                            "設定を再読み込みしました"
                        )
                        .into())
                    });
                }
                if ui
                    .button(t!(
                        "💾 Sauvegarder en flash",
                        "💾 Save to flash",
                        "💾 フラッシュに保存"
                    ))
                    .clicked()
                {
                    self.run(|s| {
                        s.dev()?.save()?;
                        Ok(t!(
                            "Sauvegardé dans le périphérique",
                            "Saved to the device",
                            "デバイスに保存しました"
                        )
                        .into())
                    });
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut lang = LANG.load(Relaxed);
                egui::ComboBox::from_id_salt("lang")
                    .selected_text(LANGS[lang as usize].1)
                    .show_ui(ui, |ui| {
                        for (i, (_, name)) in LANGS.iter().enumerate() {
                            ui.selectable_value(&mut lang, i as u8, *name);
                        }
                    });
                if lang != LANG.load(Relaxed) {
                    self.set_lang(lang);
                }
            });
        });
    }

    fn key_grid(&mut self, ui: &mut egui::Ui) {
        let layers = self.keys.iter().map(Key::layers).max().unwrap_or(0);
        ui.horizontal(|ui| {
            ui.label(t!("Couche", "Layer", "レイヤー"));
            for l in 0..layers {
                ui.selectable_value(&mut self.layer, l, format!("{}", l + 1));
            }
        });
        let width = ((ui.available_width() - 5.0 * 6.0) / 6.0).max(100.0);
        egui::Grid::new("keys").spacing([6.0, 6.0]).show(ui, |ui| {
            for i in 0..self.keys.len() {
                let label = describe(&mut self.keys[i], self.layer);
                let n = self.keys[i].number();
                let b = egui::Button::new(format!("#{n}\n{label}"))
                    .min_size(egui::vec2(width, 52.0))
                    .selected(self.selected == i);
                if ui.add(b).clicked() {
                    self.selected = i;
                }
                if i % 6 == 5 {
                    ui.end_row();
                }
            }
        });
    }

    fn key_editor(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.keys.get_mut(self.selected) else {
            return;
        };
        if self.layer >= key.layers() {
            ui.label(t!(
                "Cette touche n'a pas cette couche.",
                "This key has no such layer.",
                "このキーにはこのレイヤーがありません。"
            ));
            return;
        }
        let (x, y) = key.site();
        let (n, layer) = (key.number(), self.layer + 1);
        ui.weak(t!(
            format!("Touche #{n} (position {x},{y}) — couche {layer}"),
            format!("Key #{n} (position {x},{y}) — layer {layer}"),
            format!("キー #{n}(位置 {x},{y})— レイヤー {layer}"),
        ));
        let l = key.layer(self.layer);
        form(ui, "key_form", |ui| {
            let keyboard = t!("Clavier", "Keyboard", "キーボード");
            ui.label(t!("Mode", "Mode", "モード"));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("mode")
                    .selected_text(if l[0] == 0 {
                        keyboard.to_string()
                    } else {
                        format!("{} ({})", t!("Autre", "Other", "その他"), l[0])
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut l[0], 0, keyboard);
                    });
                ui.add(egui::DragValue::new(&mut l[0]).prefix(t!("brut ", "raw ", "生値 ")));
            });
            ui.end_row();
            if l[0] != 0 {
                ui.label(t!("Données", "Data", "データ"));
                ui.horizontal(|ui| {
                    for b in l[1..].iter_mut() {
                        ui.add(egui::DragValue::new(b).hexadecimal(2, false, true));
                    }
                });
                ui.end_row();
                return;
            }
            ui.label(t!("Modificateurs", "Modifiers", "修飾キー"));
            ui.horizontal(|ui| {
                for (b, name) in MODS.iter().enumerate() {
                    let mut on = l[2] & (1 << b) != 0;
                    if ui.checkbox(&mut on, *name).changed() {
                        l[2] ^= 1 << b;
                    }
                }
            });
            ui.end_row();
            ui.label(t!("Touches", "Keys", "キー"));
            ui.horizontal(|ui| {
                for slot in 3..6 {
                    egui::ComboBox::from_id_salt(slot)
                        .width(120.0)
                        .selected_text(key_name(l[slot]))
                        .show_ui(ui, |ui| {
                            for c in std::iter::once(0).chain(4..=0x65).chain(0x68..=0x73) {
                                ui.selectable_value(&mut l[slot], c, key_name(c));
                            }
                        });
                }
            });
            ui.end_row();
        });
        if ui
            .button(t!("✔ Appliquer la touche", "✔ Apply key", "✔ キーを適用"))
            .clicked()
        {
            let k = self.keys[self.selected].clone();
            let i = self.selected;
            self.run(|s| {
                s.keys[i] = s.dev()?.write_key(&k)?;
                let n = k.number();
                Ok(t!(
                    format!("Touche #{n} écrite (pensez à sauvegarder)"),
                    format!("Key #{n} written (remember to save)"),
                    format!("キー #{n} を書き込みました(保存を忘れずに)"),
                ))
            });
        }
    }

    fn led_editor(&mut self, ui: &mut egui::Ui) {
        let mut apply: Vec<usize> = Vec::new();
        ui.horizontal(|ui| {
            ui.label(t!("Toutes les LED", "All LEDs", "すべての LED"));
            ui.color_edit_button_srgb(&mut self.all_color);
            if ui
                .button(t!("Appliquer à toutes", "Apply to all", "すべてに適用"))
                .clicked()
            {
                for l in self.leds.iter_mut() {
                    l.color().copy_from_slice(&self.all_color);
                }
                apply = (0..self.leds.len()).collect();
            }
        });
        egui::Grid::new("leds")
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.weak("LED");
                ui.weak(t!("Couleur", "Color", "色"));
                ui.weak(t!(
                    "Paramètres bruts (octets 2-5, 9-11)",
                    "Raw parameters (bytes 2-5, 9-11)",
                    "生パラメータ(バイト 2-5、9-11)"
                ));
                ui.end_row();
                for (i, led) in self.leds.iter_mut().enumerate() {
                    ui.label(format!("#{}", led.number()));
                    let mut c = [led.raw[6], led.raw[7], led.raw[8]];
                    if ui.color_edit_button_srgb(&mut c).changed() {
                        led.color().copy_from_slice(&c);
                    }
                    ui.horizontal(|ui| {
                        for b in (2..6).chain(9..12) {
                            ui.add(
                                egui::DragValue::new(&mut led.raw[b]).hexadecimal(2, false, true),
                            );
                        }
                    });
                    if ui.button(t!("Appliquer", "Apply", "適用")).clicked() {
                        apply.push(i);
                    }
                    ui.end_row();
                }
            });
        if !apply.is_empty() {
            self.run(|s| {
                for &i in &apply {
                    s.leds[i] = s.dev()?.write_led(&s.leds[i])?;
                }
                let n = apply.len();
                Ok(t!(
                    format!("{n} LED écrite(s) (pensez à sauvegarder)"),
                    format!("{n} LED(s) written (remember to save)"),
                    format!("LED を {n} 個書き込みました(保存を忘れずに)"),
                ))
            });
        }
    }

    fn monitor_editor(&mut self, ui: &mut egui::Ui) {
        let sounds = self.sounds.clone();
        let mut map = sounds.lock().unwrap();
        let before = map.clone();
        let mut on = map.contains_key(&sound::MONITOR);
        ui.horizontal(|ui| {
            if ui
                .checkbox(
                    &mut on,
                    t!("M'entendre aussi sur", "Also hear it on", "自分用にも再生"),
                )
                .changed()
            {
                if on {
                    map.insert(sound::MONITOR, Sound::default());
                } else {
                    map.remove(&sound::MONITOR);
                }
            }
            if let Some(m) = map.get_mut(&sound::MONITOR) {
                sink_combo(ui, "monitor", &mut m.sink, &self.sinks);
                volume(ui, &mut m.volume);
            }
        });
        if *map != before {
            self.save_sounds(&map);
        }
    }

    fn sound_editor(&mut self, ui: &mut egui::Ui) {
        let code = self
            .keys
            .get_mut(self.selected)
            .filter(|k| self.layer < k.layers())
            .and_then(|k| {
                let l = k.layer(self.layer);
                (l[0] == 0)
                    .then(|| l[3..6].iter().copied().find(|c| *c != 0))
                    .flatten()
            });
        let Some(code) = code else {
            ui.label(t!(
                "Sélectionnez une touche en mode clavier avec une touche assignée.",
                "Select a key in keyboard mode with a key assigned.",
                "キーが割り当てられたキーボードモードのキーを選択してください。"
            ));
            return;
        };
        let name = key_name(code);
        ui.weak(t!(
            format!("Son joué quand le Sayo envoie « {name} »"),
            format!("Sound played when the Sayo sends “{name}”"),
            format!("Sayo が「{name}」を送信したときに再生するサウンド"),
        ));
        let dropped = ui
            .ctx()
            .input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        let sounds = self.sounds.clone();
        let mut map = sounds.lock().unwrap();
        let before = map.clone();
        let mut enabled = map.contains_key(&code);
        let mut silence = None;
        if ui
            .checkbox(
                &mut enabled,
                t!(
                    "Activer un son pour cette touche (la touche ne tape plus rien)",
                    "Enable a sound for this key (the key no longer types anything)",
                    "このキーでサウンドを有効にする(キーは何も入力しなくなります)"
                ),
            )
            .changed()
        {
            if !enabled {
                map.remove(&code);
            } else if code >= SILENT {
                map.insert(code, Sound::default());
            } else {
                silence = Some(map.remove(&code).unwrap_or_default());
            }
        }
        let (mut test, mut browse) = (false, None);
        if let Some(s) = map.get_mut(&code) {
            if let Some(p) = dropped {
                s.path = p.display().to_string();
            }
            form(ui, "sound_form", |ui| {
                ui.label(t!("Fichier", "File", "ファイル"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut s.path)
                            .desired_width(420.0)
                            .hint_text(t!(
                                "chemin du fichier audio",
                                "path to the audio file",
                                "音声ファイルのパス"
                            )),
                    );
                    if ui
                        .button(t!("📂 Parcourir...", "📂 Browse...", "📂 参照..."))
                        .clicked()
                    {
                        browse = Some(s.path.clone());
                    }
                });
                ui.end_row();
                ui.label(t!("Sortie", "Output", "出力"));
                ui.horizontal(|ui| {
                    sink_combo(ui, "sink", &mut s.sink, &self.sinks);
                    if ui
                        .button("⟳")
                        .on_hover_text(t!(
                            "Rafraîchir les sorties",
                            "Refresh outputs",
                            "出力を更新"
                        ))
                        .clicked()
                    {
                        self.sinks = sound::sinks();
                    }
                });
                ui.end_row();
                ui.label(t!("Volume", "Volume", "音量"));
                ui.horizontal(|ui| {
                    volume(ui, &mut s.volume);
                    if ui.button(t!("▶ Tester", "▶ Test", "▶ テスト")).clicked() {
                        test = true;
                    }
                    if ui.button(t!("⏹ Stop", "⏹ Stop", "⏹ 停止")).clicked() {
                        sound::stop_all(&self.players);
                    }
                });
                ui.end_row();
            });
        }
        if *map != before {
            self.save_sounds(&map);
        }
        drop(map);
        if let Some(p) = browse.and_then(|current| pick_sound(&current)) {
            let mut map = sounds.lock().unwrap();
            if let Some(s) = map.get_mut(&code) {
                s.path = p;
            }
            self.save_sounds(&map);
        }
        if let Some(s) = silence {
            let (i, layer) = (self.selected, self.layer);
            self.run(|app| {
                let d = app.dev.as_ref().ok_or(t!(
                    "Connectez le Sayo pour attribuer un son",
                    "Connect the Sayo to assign a sound",
                    "サウンドを割り当てるには Sayo を接続してください"
                ))?;
                let mut k = app.keys[i].clone();
                let n = k.number();
                let silent = SILENT + n;
                k.layer(layer).copy_from_slice(&[0, 0, 0, silent, 0, 0]);
                app.keys[i] = d.write_key(&k)?;
                d.save()?;
                let mut map = app.sounds.lock().unwrap();
                map.insert(silent, s);
                sound::save(&map)?;
                Ok(t!(
                    format!("Touche #{n} : son uniquement (enregistré dans le Sayo)"),
                    format!("Key #{n}: sound only (saved to the Sayo)"),
                    format!("キー #{n}: サウンド専用(Sayo に保存しました)"),
                ))
            });
        }
        if test {
            if let Err(e) = sound::trigger(&sounds.lock().unwrap(), code, &self.players) {
                self.status = e;
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.dev.is_none() && self.status.is_empty() {
            self.connect();
        }
        let bar = egui::Frame::side_top_panel(&ctx.style())
            .inner_margin(egui::Margin::symmetric(12.0, 8.0));
        egui::TopBottomPanel::top("top")
            .frame(bar)
            .show(ctx, |ui| self.top(ui));
        egui::TopBottomPanel::bottom("status")
            .frame(bar)
            .show(ctx, |ui| ui.label(&self.status));
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                section(ui, t!("Touches", "Keys", "キー"), |ui| {
                    self.key_grid(ui);
                    ui.separator();
                    self.key_editor(ui);
                });
                section(ui, t!("Son", "Sound", "サウンド"), |ui| {
                    self.monitor_editor(ui);
                    ui.separator();
                    self.sound_editor(ui);
                });
                section(
                    ui,
                    t!("Éclairage", "Lighting", "ライティング"),
                    |ui| self.led_editor(ui),
                );
            });
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lang_codes() {
        assert_eq!(super::lang_of("fr_FR.UTF-8"), 0);
        assert_eq!(super::lang_of("ja"), 2);
        assert_eq!(super::lang_of("de_DE.UTF-8"), 1);
        assert_eq!(super::lang_of(""), 1);
    }
}
