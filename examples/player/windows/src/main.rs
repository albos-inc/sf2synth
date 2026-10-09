//! SF2 Player for Windows: choose an SF2 and a MIDI file and play it.
//!
//! The UI is egui, the audio cpal (WASAPI), the player `sf2player`. It runs
//! on macOS and Linux too, which is handy while working on it.
//!
//! ```text
//! cargo run --release -p sf2player-windows -- [song.mid] [font.sf2]
//! ```

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};
use egui_phosphor::regular as icon;
use sf2player::{LoadedSong, LoadedSoundFont, State, format_size, format_time};

use audio::Output;

const SOUND_FONT_EXTENSIONS: &[&str] = &["sf2", "dls"];
const MIDI_EXTENSIONS: &[&str] = &["mid", "midi", "smf", "kar", "rmi"];
const LAST_SOUND_FONT: &str = "last_sound_font";

/// The General MIDI set that comes with the system, played until an SF2 is
/// chosen.
fn built_in_sounds() -> Option<PathBuf> {
    let path = if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join("drivers").join("gm.dls")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/System/Library/Components/CoreAudio.component/Contents/Resources/gs_instruments.dls")
    } else {
        return None;
    };
    path.exists().then_some(path)
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SF2 Player")
            .with_inner_size([560.0, 720.0])
            .with_min_inner_size([420.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(app_icon())),
        ..Default::default()
    };
    eframe::run_native("SF2 Player", options, Box::new(|cc| Ok(Box::new(PlayerApp::new(cc)))))
}

/// A file parsed on a loading thread.
enum Loaded {
    SoundFont(PathBuf, Result<Box<LoadedSoundFont>, String>),
    Song(PathBuf, Result<LoadedSong, String>),
}

struct PlayerApp {
    output: Result<Output, String>,
    loaded_tx: Sender<Loaded>,
    loaded_rx: Receiver<Loaded>,
    /// Files being read.
    loading: usize,
    sound_font_path: Option<PathBuf>,
    error: Option<String>,
    /// Where the progress bar is dragged to, until it is let go.
    scrub: Option<f64>,
    /// Play the song once the SF2 it waits for is loaded.
    autoplay: bool,
}

impl PlayerApp {
    fn new(cc: &eframe::CreationContext) -> PlayerApp {
        install_fonts(&cc.egui_ctx);
        let (loaded_tx, loaded_rx) = channel();
        let mut app = PlayerApp {
            output: Output::open(),
            loaded_tx,
            loaded_rx,
            loading: 0,
            sound_font_path: None,
            error: None,
            scrub: None,
            autoplay: false,
        };
        // Files from the command line (or "Open with"), else the last SF2,
        // else the system's own sounds.
        let mut sound_font = cc.storage.and_then(|s| s.get_string(LAST_SOUND_FONT)).map(PathBuf::from);
        for arg in std::env::args_os().skip(1) {
            let path = PathBuf::from(arg);
            if has_extension(&path, SOUND_FONT_EXTENSIONS) {
                sound_font = Some(path);
            } else {
                app.open_song(path);
            }
        }
        if let Some(path) = sound_font.filter(|p| p.exists()).or_else(built_in_sounds) {
            app.open_sound_font(path);
        }
        app
    }

    fn sample_rate(&self) -> u32 {
        self.output.as_ref().map_or(48_000, |o| o.info.sample_rate)
    }

    fn open_sound_font(&mut self, path: PathBuf) {
        let (tx, sample_rate) = (self.loaded_tx.clone(), self.sample_rate());
        self.loading += 1;
        std::thread::spawn(move || {
            let result = std::fs::File::open(&path)
                .map_err(|e| e.to_string())
                .and_then(|file| LoadedSoundFont::open(&file, &file_name(&path), sample_rate).map(Box::new));
            let _ = tx.send(Loaded::SoundFont(path, result));
        });
    }

    fn open_song(&mut self, path: PathBuf) {
        let tx = self.loaded_tx.clone();
        self.loading += 1;
        std::thread::spawn(move || {
            let result = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| LoadedSong::new(&bytes, &file_name(&path)));
            let _ = tx.send(Loaded::Song(path, result));
        });
    }

    fn open(&mut self, path: PathBuf) {
        if has_extension(&path, SOUND_FONT_EXTENSIONS) {
            self.open_sound_font(path);
        } else if has_extension(&path, MIDI_EXTENSIONS) {
            self.open_song(path);
        } else {
            self.error = Some(format!("Not an SF2, DLS or MIDI file: {}", file_name(&path)));
        }
    }

    /// Hands loaded files to the player.
    fn receive(&mut self) {
        while let Ok(loaded) = self.loaded_rx.try_recv() {
            self.loading -= 1;
            let Ok(output) = &self.output else { continue };
            match loaded {
                Loaded::SoundFont(path, Ok(font)) => {
                    let previous = output.player().with(|p| p.set_sound_font(*font));
                    // The old SF2 is freed here, outside the audio lock.
                    drop(previous);
                    self.sound_font_path = Some(path);
                    self.error = None;
                }
                Loaded::Song(_, Ok(song)) => {
                    let previous = output.player().with(|p| p.set_song(song));
                    drop(previous);
                    self.autoplay = true;
                    self.error = None;
                }
                Loaded::SoundFont(path, Err(e)) | Loaded::Song(path, Err(e)) => {
                    self.error = Some(format!("{}: {e}", file_name(&path)));
                }
            }
            if self.autoplay && self.loading == 0 && output.player().status().can_play {
                output.player().with(|p| p.play());
                self.autoplay = false;
            }
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let (dropped, space) = ctx.input(|i| (i.raw.dropped_files.clone(), i.key_pressed(egui::Key::Space)));
        for file in dropped {
            self.open(file.path().to_path_buf());
        }
        if space && let Ok(output) = &self.output {
            output.player().with(|p| p.toggle());
        }
    }
}

impl eframe::App for PlayerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.receive();
        self.handle_input(ui.ctx());

        egui::Panel::top("files").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(format!("{} Open SF2…", icon::WAVEFORM)).clicked()
                    && let Some(path) =
                        rfd::FileDialog::new().add_filter("SoundFont / DLS", SOUND_FONT_EXTENSIONS).pick_file()
                {
                    self.open_sound_font(path);
                }
                if ui.button(format!("{} Open MIDI…", icon::MUSIC_NOTES)).clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter("MIDI", MIDI_EXTENSIONS).pick_file()
                {
                    self.open_song(path);
                }
                if self.loading > 0 {
                    ui.spinner();
                    ui.label("Loading…");
                }
            });
            ui.add_space(6.0);
        });

        let output = match &self.output {
            Ok(output) => output,
            Err(e) => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.colored_label(Color32::LIGHT_RED, format!("No audio output: {e}"));
                });
                return;
            }
        };

        // Read without the player's lock: drawing never holds up the audio.
        let status = output.player().status();
        let (state, can_play, position, duration, voices) =
            (status.state, status.can_play, status.position, status.duration, status.active_voices);
        let (song, font) = (output.player().song(), output.player().sound_font());

        let mut command = None;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                let title = song.as_ref().map_or("No song", |s| s.title.as_str());
                ui.label(RichText::new(title).size(24.0).strong());
                let subtitle = match (&song, &font) {
                    (Some(song), Some(font)) => format!("{} · {}", song.file_name, font.name),
                    (Some(song), None) => format!("{} · choose an SF2", song.file_name),
                    (None, Some(font)) => format!("{} · open or drop a MIDI file", font.name),
                    (None, None) => "Open or drop an SF2 and a MIDI file".to_string(),
                };
                ui.label(RichText::new(subtitle).weak());
            });
            ui.add_space(16.0);

            // Progress: drag to seek, and the time.
            let mut shown = self.scrub.unwrap_or(position);
            ui.spacing_mut().slider_width = ui.available_width();
            let slider = egui::Slider::new(&mut shown, 0.0..=duration.max(0.001)).show_value(false);
            let response = ui.add_enabled(song.is_some(), slider);
            if response.dragged() || response.changed() {
                self.scrub = Some(shown);
            }
            if (response.drag_stopped() || (response.changed() && !response.dragged()))
                && let Some(to) = self.scrub.take()
            {
                command = Some(Command::Seek(to));
            }
            ui.horizontal(|ui| {
                ui.monospace(format_time(shown));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.monospace(format_time(duration));
                });
            });
            ui.add_space(8.0);

            // Transport.
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    let size = egui::vec2(56.0, 40.0);
                    ui.add_space((ui.available_width() - size.x * 2.0 - ui.spacing().item_spacing.x) / 2.0);
                    let (glyph, label) =
                        if state == State::Playing { (icon::PAUSE, "Pause") } else { (icon::PLAY, "Play") };
                    let play = egui::Button::new(RichText::new(glyph).size(22.0)).min_size(size);
                    if ui.add_enabled(can_play, play).on_hover_text(format!("{label} (Space)")).clicked() {
                        command = Some(Command::Toggle);
                    }
                    let stop = egui::Button::new(RichText::new(icon::STOP).size(22.0)).min_size(size);
                    if ui.add_enabled(state != State::Stopped, stop).on_hover_text("Stop").clicked() {
                        command = Some(Command::Stop);
                    }
                });
                let state_label = match state {
                    State::Stopped => "Stopped",
                    State::Playing => "Playing",
                    State::Paused => "Paused",
                };
                ui.label(RichText::new(state_label).weak());
            });

            if let Some(error) = &self.error {
                ui.add_space(8.0);
                ui.colored_label(Color32::LIGHT_RED, error);
            }
            ui.add_space(12.0);
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                section(ui, "Song", |ui| match &song {
                    Some(s) => {
                        row(ui, "Title", &s.title);
                        row(ui, "File", &s.file_name);
                        row(ui, "Length", &format_time(s.duration));
                        let standard =
                            if s.standard.is_empty() { String::new() } else { format!(" · {}", s.standard) };
                        row(ui, "Format", &format!("SMF {} · {} tracks{standard}", s.format, s.tracks));
                        let tempo = if s.tempo_changes > 1 {
                            format!("{:.1} BPM ({} tempo changes)", s.tempo_bpm, s.tempo_changes)
                        } else {
                            format!("{:.1} BPM", s.tempo_bpm)
                        };
                        row(ui, "Tempo", &tempo);
                        if s.ticks_per_quarter > 0 {
                            row(ui, "Resolution", &format!("{} ticks / quarter", s.ticks_per_quarter));
                        }
                        row(ui, "Notes", &format!("{} on {} channels", s.notes, s.channels));
                    }
                    None => row(ui, "", "—"),
                });
                section(ui, "SoundFont", |ui| match &font {
                    Some(f) => {
                        row(ui, "Name", &f.name);
                        row(ui, "File", &f.file_name);
                        row(ui, "Format", &format!("{} · {}", f.format, format_size(f.file_size)));
                        row(
                            ui,
                            "Contents",
                            &format!("{} presets · {} instruments · {} samples", f.presets, f.instruments, f.samples),
                        );
                        for (label, value) in
                            [("Copyright", &f.copyright), ("Engineers", &f.engineers), ("Comment", &f.comment)]
                        {
                            if !value.is_empty() {
                                row(ui, label, value);
                            }
                        }
                        if let Some(path) = &self.sound_font_path {
                            row(ui, "Path", &path.display().to_string());
                        }
                    }
                    None => row(ui, "", "—"),
                });
                section(ui, "Audio", |ui| {
                    let info = &output.info;
                    row(ui, "Output", &format!("{} ({})", info.device, info.host));
                    row(
                        ui,
                        "Format",
                        &format!(
                            "{:.1} kHz · {} ch · {}",
                            info.sample_rate as f64 / 1000.0,
                            info.channels,
                            info.sample_format
                        ),
                    );
                    let frames = output.callback_frames();
                    if frames > 0 {
                        let ms = frames as f64 * 1000.0 / info.sample_rate as f64;
                        row(ui, "Buffer", &format!("{frames} frames ({ms:.1} ms)"));
                    }
                    row(ui, "Voices", &format!("{voices} playing"));
                    row(ui, "Engine", "sf2synth");
                });
            });
        });

        match command {
            Some(Command::Toggle) => output.player().with(|p| p.toggle()),
            Some(Command::Stop) => output.player().with(|p| p.stop()),
            Some(Command::Seek(to)) => output.player().with(|p| p.seek(to)),
            None => {}
        }

        let ctx = ui.ctx();
        if state == State::Playing || voices > 0 || self.loading > 0 {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(path) = &self.sound_font_path
            && Some(path) != built_in_sounds().as_ref()
        {
            storage.set_string(LAST_SOUND_FONT, path.display().to_string());
        }
    }
}

enum Command {
    Toggle,
    Stop,
    Seek(f64),
}

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong());
    egui::Grid::new(title).num_columns(2).min_col_width(96.0).spacing([16.0, 4.0]).show(ui, add);
}

fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).weak());
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// The window icon (`assets/icon-256.png`, drawn by `icon/make-icons.swift`).
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).unwrap_or_default()
}

/// Puts a system font with Japanese glyphs first, so Japanese song titles
/// show, and adds the Phosphor icons.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let candidates: &[(&str, u32)] = &[
        // Meiryo UI is face 2 of the Meiryo collection.
        ("C:\\Windows\\Fonts\\meiryo.ttc", 2),
        ("C:\\Windows\\Fonts\\YuGothM.ttc", 0),
        ("C:\\Windows\\Fonts\\msgothic.ttc", 0),
        ("/System/Library/Fonts/ヒラギノ角ゴシック W4.ttc", 0),
        ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    ];
    if let Some((bytes, index)) = candidates.iter().find_map(|&(path, index)| Some((std::fs::read(path).ok()?, index)))
    {
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert("system".into(), data.into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.push("system".into());
            }
        }
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}
