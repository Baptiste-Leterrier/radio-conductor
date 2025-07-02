use eframe::egui;
use egui::{Color32, Vec2, Pos2, Stroke, FontId, Align2};
use rfd::FileDialog;
use rodio::{Decoder, OutputStream, Sink, Source};
use serde::{Serialize, Deserialize};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::default::get_probe;

mod vec2_serde {
    use egui::Vec2;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(vec: &Vec2, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde::Serialize::serialize(&[vec.x, vec.y], serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec2, D::Error>
    where
        D: Deserializer<'de>,
    {
        let arr = <[f32; 2]>::deserialize(deserializer)?;
        Ok(Vec2::new(arr[0], arr[1]))
    }
}

mod color32_serde {
    use egui::Color32;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(color: &Color32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        let rgba = (a as u32) << 24 | (b as u32) << 16 | (g as u32) << 8 | (r as u32);
        serializer.serialize_u32(rgba)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Color32, D::Error>
    where
        D: Deserializer<'de>,
    {
        let rgba = u32::deserialize(deserializer)?;
        Ok(Color32::from_rgba_premultiplied(
            (rgba & 0xFF) as u8,
            ((rgba >> 8) & 0xFF) as u8,
            ((rgba >> 16) & 0xFF) as u8,
            ((rgba >> 24) & 0xFF) as u8,
        ))
    }
}

#[derive(Serialize, Deserialize)]
struct MusicButton {
    name: String,
    path: PathBuf,
    #[serde(with = "vec2_serde")]
    position: Vec2,
    #[serde(with = "color32_serde")]
    color: Color32,
    waveform: Vec<f32>,
    duration: f32, // seconds
}

#[derive(Serialize, Deserialize)]
struct MusicTab {
    name: String,
    buttons: Vec<MusicButton>,
}

#[derive(Serialize, Deserialize)]
struct EditState {
    editing: Option<usize>, // index in tab.buttons
    name_buf: String,
    #[serde(with = "color32_serde")]
    color_buf: Color32,
    pending_music_slot: Option<usize>, // slot to add music to
    pending_change_music: Option<usize>, // button index to change music
}

#[derive(Serialize, Deserialize)]
struct MusicInterface {
    tabs: Vec<MusicTab>,
    current_tab: usize,
    #[serde(skip)]
    audio_player: AudioPlayer,
    edit_mode: bool,
    current_playing: Option<(usize, usize)>, // (tab, index)
    edit_state: EditState,
    renaming_tab: Option<usize>, // index of tab being renamed
    tab_rename_buf: String,      // buffer for renaming
}

struct AudioPlayer {
    sink: Option<Arc<Sink>>,
    _stream: OutputStream,
    _stream_handle: rodio::OutputStreamHandle,
    is_fading: Arc<AtomicBool>,
    start_time: Option<Instant>,
    duration: f32,
}

impl AudioPlayer {
    fn new() -> Self {
        let (_stream, _stream_handle) = OutputStream::try_default().unwrap();
        Self {
            sink: None,
            _stream,
            _stream_handle,
            is_fading: Arc::new(AtomicBool::new(false)),
            start_time: None,
            duration: 0.0,
        }
    }

    fn play(&mut self, path: &PathBuf, duration: f32) {
        if let Some(current_sink) = &self.sink {
            current_sink.stop();
        }
        let file = BufReader::new(File::open(path).unwrap());
        let sink = Arc::new(Sink::try_new(&self._stream_handle).unwrap());
        let source = Decoder::new(file).unwrap();
        sink.append(source);
        self.sink = Some(sink);
        self.start_time = Some(Instant::now());
        self.duration = duration;
    }

    fn stop(&mut self) {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        self.sink = None;
        self.start_time = None;
    }

    fn fade_out(&mut self) {
        if let Some(sink) = &self.sink {
            let is_fading = self.is_fading.clone();
            if !is_fading.load(Ordering::SeqCst) {
                is_fading.store(true, Ordering::SeqCst);
                let sink_clone = sink.clone();
                thread::spawn(move || {
                    let start = Instant::now();
                    let duration = Duration::from_secs(1);
                    while start.elapsed() < duration {
                        let progress = start.elapsed().as_secs_f32() / duration.as_secs_f32();
                        let volume = 1.0 - progress;
                        sink_clone.set_volume(volume);
                        thread::sleep(Duration::from_millis(16));
                    }
                    sink_clone.stop();
                    is_fading.store(false, Ordering::SeqCst);
                });
            }
        }
    }

    fn elapsed(&self) -> f32 {
        if let Some(start) = self.start_time {
            start.elapsed().as_secs_f32()
        } else {
            0.0
        }
    }
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for MusicInterface {
    fn default() -> Self {
        Self {
            tabs: vec![MusicTab { name: "Tab 1".to_string(), buttons: Vec::new() }],
            current_tab: 0,
            audio_player: AudioPlayer::new(),
            edit_mode: false,
            current_playing: None,
            edit_state: EditState {
                editing: None,
                name_buf: String::new(),
                color_buf: Color32::WHITE,
                pending_music_slot: None,
                pending_change_music: None,
            },
            renaming_tab: None,
            tab_rename_buf: String::new(),
        }
    }
}

impl MusicInterface {
    fn get_duration_with_symphonia(path: &PathBuf) -> Option<f32> {
        let file = std::fs::File::open(path).ok()?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let probed = get_probe().format(
            &Hint::new(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        ).ok()?;
        let format = probed.format;
        let track = format.default_track()?;
        let codec_params = &track.codec_params;
        let duration = codec_params.n_frames?;
        let sample_rate = codec_params.sample_rate?;
        Some(duration as f32 / sample_rate as f32)
    }

    fn generate_waveform_and_duration(path: &PathBuf) -> (Vec<f32>, f32) {
        let file = BufReader::new(File::open(path).unwrap());
        let decoder = Decoder::new(file).unwrap();
        let samples: Vec<f32> = decoder
            .convert_samples::<f32>()
            .collect::<Vec<f32>>()
            .chunks(1024)
            .map(|chunk| chunk.iter().map(|s| s.abs()).max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0))
            .collect();
        let duration = Self::get_duration_with_symphonia(path).unwrap_or(0.0);
        (samples, duration)
    }

    fn add_music_at(&mut self, slot: usize) {
        if let Some(path) = FileDialog::new()
            .add_filter("Audio", &["mp3", "wav"])
            .pick_file()
        {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let (waveform, duration) = Self::generate_waveform_and_duration(&path);
            let tab = &mut self.tabs[self.current_tab];
            if tab.buttons.len() <= slot {
                tab.buttons.resize_with(slot + 1, || MusicButton {
                    name: String::new(),
                    path: PathBuf::new(),
                    position: Vec2::ZERO,
                    color: Color32::from_rgb(100, 100, 255),
                    waveform: vec![],
                    duration: 0.0,
                });
            }
            tab.buttons[slot] = MusicButton {
                name,
                path,
                position: Vec2::ZERO,
                color: Color32::from_rgb(100, 100, 255),
                waveform,
                duration,
            };
        }
    }

    fn format_time(secs: f32) -> String {
        let secs = secs.max(0.0) as u64;
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        if h > 0 {
            format!("{:02}:{:02}:{:02}", h, m, s)
        } else {
            format!("{:02}:{:02}", m, s)
        }
    }

    fn save_to_file(&self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let data = bincode::serialize(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    fn load_from_file(&mut self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let data = std::fs::read(path)?;
        let mut loaded: MusicInterface = bincode::deserialize(&data)?;
        loaded.audio_player = AudioPlayer::new();
        *self = loaded;
        Ok(())
    }
}

impl eframe::App for MusicInterface {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // Save/Import buttons
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    if let Some(path) = FileDialog::new().set_file_name("radio_conductor_save.bin").save_file() {
                        let _ = self.save_to_file(&path);
                    }
                }
                if ui.button("Import").clicked() {
                    if let Some(path) = FileDialog::new().pick_file() {
                        let _ = self.load_from_file(&path);
                    }
                }
            });
            // Edit mode banner
            if self.edit_mode {
                ui.colored_label(
                    Color32::from_rgb(255, 200, 0),
                    egui::RichText::new("YOU ARE IN EDIT MODE: You can edit music buttons. Click 'Exit Edit Mode' to return to normal mode.")
                        .strong()
                        .size(20.0),
                );
                ui.add_space(8.0);
            }
            // Tabs
            ui.horizontal(|ui| {
                for (i, tab) in self.tabs.iter_mut().enumerate() {
                    let tab_id = ui.make_persistent_id(("tab", i));
                    if self.renaming_tab == Some(i) {
                        let resp = ui.text_edit_singleline(&mut self.tab_rename_buf);
                        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (resp.lost_focus() && ui.input(|i| !i.pointer.any_pressed())) || resp.clicked_elsewhere() || enter_pressed {
                            if !self.tab_rename_buf.trim().is_empty() {
                                tab.name = self.tab_rename_buf.trim().to_string();
                            }
                            self.renaming_tab = None;
                        }
                    } else {
                        let label = ui.selectable_label(self.current_tab == i, &tab.name);
                        if label.clicked() {
                            self.current_tab = i;
                        }
                        if label.double_clicked() {
                            self.renaming_tab = Some(i);
                            self.tab_rename_buf = tab.name.clone();
                        }
                    }
                }
                if ui.button("+").clicked() {
                    let idx = self.tabs.len() + 1;
                    self.tabs.push(MusicTab { name: format!("Tab {}", idx), buttons: Vec::new() });
                    self.current_tab = self.tabs.len() - 1;
                }
            });
            ui.separator();
            // Edit mode toggle only
            ui.horizontal(|ui| {
                if ui.button(if self.edit_mode { "Exit Edit Mode" } else { "Enter Edit Mode" }).clicked() {
                    self.edit_mode = !self.edit_mode;
                }
            });
            ui.separator();
            // Responsive grid with 20 slots
            let tab = &mut self.tabs[self.current_tab];
            let available_size = ui.available_size();
            let cols = 5;
            let rows = 4;
            let hpad = 12.0; // horizontal padding on each side
            let vpad = 12.0; // vertical padding on top and bottom
            let col_spacing = 8.0;
            let row_spacing = 8.0;
            let btn_w = (available_size.x - 2.0 * hpad - (cols as f32 - 1.0) * col_spacing) / cols as f32;
            let btn_h = (available_size.y - 2.0 * vpad - (rows as f32 - 1.0) * row_spacing) / rows as f32;
            let idx = 0;
            let add_requests = Vec::new();
            ui.add_space(vpad);
            egui::Grid::new("button_grid").spacing(Vec2::new(col_spacing, row_spacing)).show(ui, |ui| {
                for row in 0..rows {
                    for col in 0..cols {
                        let idx = row * cols + col;
                        let button_opt = tab.buttons.get_mut(idx);
                        if let Some(button) = button_opt {
                            if !button.name.is_empty() {
                                let (id, rect) = ui.allocate_space(Vec2::new(btn_w, btn_h));
                                let painter = ui.painter_at(rect);
                                // Draw waveform background
                                let wf = &button.waveform;
                                let wf_len = wf.len().max(1);
                                let step = wf_len as f32 / btn_w.max(1.0);
                                let base_y = rect.bottom();
                                let top_y = rect.top();
                                let color = button.color.gamma_multiply(0.3);
                                for x in 0..btn_w as usize {
                                    let idx_wf = (x as f32 * step) as usize;
                                    let h = wf.get(idx_wf).copied().unwrap_or(0.0);
                                    let y = base_y - h * (btn_h * 0.8);
                                    painter.line_segment([
                                        Pos2::new(rect.left() + x as f32, base_y),
                                        Pos2::new(rect.left() + x as f32, y.max(top_y))
                                    ], Stroke::new(1.0, color));
                                }
                                // Draw button overlay
                                painter.rect_filled(rect, 8.0, button.color.gamma_multiply(0.7));
                                // Draw name
                                painter.text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    &button.name,
                                    FontId::proportional(22.0),
                                    Color32::WHITE,
                                );
                                // Draw duration/remaining
                                let (time_str, time_color) = if Some((self.current_tab, idx)) == self.current_playing {
                                    let elapsed = self.audio_player.elapsed();
                                    let remaining = (button.duration - elapsed).max(0.0);
                                    (Self::format_time(remaining), Color32::YELLOW)
                                } else {
                                    (Self::format_time(button.duration), Color32::WHITE)
                                };
                                painter.text(
                                    Pos2::new(rect.right() - 10.0, rect.bottom() - 10.0),
                                    Align2::RIGHT_BOTTOM,
                                    time_str,
                                    FontId::proportional(16.0),
                                    time_color,
                                );
                                // Draw progress slider if playing
                                if Some((self.current_tab, idx)) == self.current_playing {
                                    let elapsed = self.audio_player.elapsed();
                                    let progress = (elapsed / button.duration).min(1.0);
                                    let x = rect.left() + progress * rect.width();
                                    painter.line_segment([
                                        Pos2::new(x, rect.top()),
                                        Pos2::new(x, rect.bottom())
                                    ], Stroke::new(2.0, Color32::RED));
                                }
                                // Interactivity
                                let resp = ui.interact(rect, ui.make_persistent_id((row, col)), egui::Sense::click());
                                if self.edit_mode {
                                    if resp.clicked() {
                                        self.edit_state.editing = Some(idx);
                                        self.edit_state.name_buf = button.name.clone();
                                        self.edit_state.color_buf = button.color;
                                    }
                                } else if resp.clicked() {
                                    if Some((self.current_tab, idx)) == self.current_playing {
                                        self.audio_player.fade_out();
                                        self.current_playing = None;
                                    } else {
                                        if self.current_playing.is_some() {
                                            self.audio_player.fade_out();
                                        }
                                        self.audio_player.play(&button.path, button.duration);
                                        self.current_playing = Some((self.current_tab, idx));
                                    }
                                }
                            } else {
                                // Empty slot
                                let (id, rect) = ui.allocate_space(Vec2::new(btn_w, btn_h));
                                let resp = ui.interact(rect, ui.make_persistent_id((row, col)), egui::Sense::click());
                                ui.painter_at(rect).rect_filled(rect, 8.0, Color32::DARK_GRAY.gamma_multiply(0.5));
                                ui.painter_at(rect).text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    "Click to add...",
                                    FontId::proportional(20.0),
                                    Color32::WHITE,
                                );
                                if resp.clicked() {
                                    self.edit_state.pending_music_slot = Some(idx);
                                }
                            }
                        } else {
                            // Slot not yet created
                            let (id, rect) = ui.allocate_space(Vec2::new(btn_w, btn_h));
                            let resp = ui.interact(rect, ui.make_persistent_id((row, col)), egui::Sense::click());
                            ui.painter_at(rect).rect_filled(rect, 8.0, Color32::DARK_GRAY.gamma_multiply(0.5));
                            ui.painter_at(rect).text(
                                rect.center(),
                                Align2::CENTER_CENTER,
                                "Click to add...",
                                FontId::proportional(20.0),
                                Color32::WHITE,
                            );
                            if resp.clicked() {
                                self.edit_state.pending_music_slot = Some(idx);
                            }
                        }
                    }
                    ui.end_row();
                }
            });
            // Edit popup
            if let Some(edit_idx) = self.edit_state.editing {
                egui::Window::new("Edit Music Button")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut self.edit_state.name_buf);
                        ui.label("Color:");
                        ui.color_edit_button_srgba(&mut self.edit_state.color_buf);
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                if let Some(button) = tab.buttons.get_mut(edit_idx) {
                                    button.name = self.edit_state.name_buf.clone();
                                    button.color = self.edit_state.color_buf;
                                }
                                self.edit_state.editing = None;
                            }
                            if ui.button("Cancel").clicked() {
                                self.edit_state.editing = None;
                            }
                            if ui.button("Change Music").clicked() {
                                self.edit_state.pending_change_music = Some(edit_idx);
                            }
                        });
                    });
            }

            // After UI: process add requests
            if let Some(slot) = self.edit_state.pending_music_slot.take() {
                if let Some(path) = FileDialog::new()
                    .add_filter("Audio", &["mp3", "wav"])
                    .pick_file()
                {
                    let name = path.file_name().unwrap().to_string_lossy().to_string();
                    let (waveform, duration) = Self::generate_waveform_and_duration(&path);
                    let tab = &mut self.tabs[self.current_tab];
                    if tab.buttons.len() <= slot {
                        tab.buttons.resize_with(slot + 1, || MusicButton {
                            name: String::new(),
                            path: PathBuf::new(),
                            position: Vec2::ZERO,
                            color: Color32::from_rgb(100, 100, 255),
                            waveform: vec![],
                            duration: 0.0,
                        });
                    }
                    tab.buttons[slot] = MusicButton {
                        name,
                        path,
                        position: Vec2::ZERO,
                        color: Color32::from_rgb(100, 100, 255),
                        waveform,
                        duration,
                    };
                }
            }
            if let Some(edit_idx) = self.edit_state.pending_change_music.take() {
                if let Some(path) = FileDialog::new()
                    .add_filter("Audio", &["mp3", "wav"])
                    .pick_file()
                {
                    let name = path.file_name().unwrap().to_string_lossy().to_string();
                    let (waveform, duration) = Self::generate_waveform_and_duration(&path);
                    let tab = &mut self.tabs[self.current_tab];
                    if let Some(button) = tab.buttons.get_mut(edit_idx) {
                        button.name = name.clone();
                        button.path = path;
                        button.waveform = waveform;
                        button.duration = duration;
                    }
                    self.edit_state.name_buf = name;
                }
            }
            for idx in add_requests {
                self.add_music_at(idx);
            }
        });
        ctx.request_repaint();
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Radio Conductor",
        options,
        Box::new(|_cc| Box::new(MusicInterface::default())),
    )
}
