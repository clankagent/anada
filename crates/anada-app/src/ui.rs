use crate::applications::group_applications;
use crate::model::{Command, Config, HostState};
use crate::ui_client::{Client, RequestId, Tag};
use crate::ui_eq::Editor;
use anada_dsp::EqSettings;
use anada_windows::{Endpoint, WindowsSnapshot};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Margin, RichText, Sense, Stroke,
};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};
#[path = "ui_demo.rs"]
mod demo;
#[path = "ui_mcp.rs"]
mod mcp_ui;
#[path = "ui_meter.rs"]
mod meter;
#[path = "ui_mixer.rs"]
mod mixer;
#[path = "ui_outputs.rs"]
mod outputs;
#[path = "ui_settings.rs"]
mod settings;
#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
const BG: Color32 = Color32::from_rgb(21, 23, 24);
const PANEL: Color32 = Color32::from_rgb(29, 32, 34);
const RAISED: Color32 = Color32::from_rgb(38, 42, 45);
const FIELD: Color32 = Color32::from_rgb(18, 20, 21);
const EDGE: Color32 = Color32::from_rgb(65, 72, 77);
const TEXT: Color32 = Color32::from_rgb(232, 234, 234);
const MUTED: Color32 = Color32::from_rgb(163, 171, 175);
const ACCENT: Color32 = Color32::from_rgb(86, 184, 198);
const GOOD: Color32 = Color32::from_rgb(108, 195, 138);
const WARN: Color32 = Color32::from_rgb(227, 179, 90);
const BAD: Color32 = Color32::from_rgb(239, 123, 110);

pub fn run() -> eframe::Result {
    run_window(false)
}
pub fn run_demo() -> eframe::Result {
    run_window(true)
}
fn run_window(demo: bool) -> eframe::Result {
    let size = if demo {
        [
            std::env::var("ANADA_UI_WIDTH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1180.0),
            std::env::var("ANADA_UI_HEIGHT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(860.0),
        ]
    } else {
        [1180.0, 860.0]
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(if demo {
                "Anada · demo · audio off"
            } else {
                "Anada"
            })
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/anada.png"))
                    .expect("embedded window icon"),
            )
            .with_inner_size(size)
            .with_min_inner_size([640.0, 540.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Anada",
        options,
        Box::new(move |cc| {
            theme(&cc.egui_ctx);
            if demo
                && let Ok(scale) = std::env::var("ANADA_UI_SCALE")
                && let Ok(scale) = scale.parse::<f32>()
                && (0.75..=2.0).contains(&scale)
            {
                cc.egui_ctx.set_pixels_per_point(scale);
            }
            Ok(Box::new(AnadaApp::new(cc.egui_ctx.clone(), demo)))
        }),
    )
}
#[derive(Clone)]
struct ApplicationDrag {
    key: String,
}
struct RouteIntent {
    target: String,
    request: RequestId,
    since: Instant,
    applied: bool,
    error: Option<String>,
}
struct VolumeEdit {
    value: f32,
    db: bool,
    request: Option<RequestId>,
    sent: Option<f32>,
    last_sent: Instant,
    dirty: bool,
}
struct AnadaApp {
    mcp: mcp_ui::McpPanel,
    meters: crate::model::AudioMeters,
    meter_received: Option<Instant>,
    clipped_at: Option<Instant>,
    closing: bool,
    allow_close: bool,
    capture_frames: u8,
    client: Client,
    host: Option<HostState>,
    draft: Option<Config>,
    base_revision: u64,
    submitted: Option<RequestId>,
    dirty_since: Option<Instant>,
    connected: bool,
    starting: bool,
    save_error: Option<String>,
    conflict: bool,
    notice: Option<(String, bool)>,
    selected: String,
    settings: bool,
    settings_tab: usize,
    eq_spectrum: bool,
    editor: Editor,
    volumes: HashMap<String, VolumeEdit>,
    fader_drag: Option<String>,
    mutes: HashMap<String, (bool, RequestId)>,
    /// Quiet toggles sent but not yet answered, so the button shows the new state.
    quiets: HashMap<String, (bool, RequestId)>,
    routes: HashMap<String, RouteIntent>,
    route_menu: Option<String>,
    presets: Vec<crate::presets::EqPreset>,
    preset_name: String,
    preset_editor: bool,
    preset_delete: bool,
    startup: Option<Result<bool, String>>,
    startup_rx: mpsc::Receiver<Result<bool, String>>,
    startup_tx: mpsc::Sender<Result<bool, String>>,
    startup_pending: bool,
    startup_desired: Option<bool>,
    confirm_exit: bool,
    demo: bool,
    #[cfg(test)]
    pill_rects: HashMap<String, egui::Rect>,
    #[cfg(test)]
    strip_rects: HashMap<String, egui::Rect>,
    #[cfg(test)]
    fader_rects: HashMap<String, egui::Rect>,
    #[cfg(test)]
    percentage_rects: HashMap<String, egui::Rect>,
    #[cfg(test)]
    header_rects: HashMap<String, egui::Rect>,
    #[cfg(test)]
    eq_scale_rects: HashMap<String, egui::Rect>,
    #[cfg(test)]
    top_bar_rects: HashMap<String, egui::Rect>,
    #[cfg(test)]
    quiet_rects: HashMap<String, egui::Rect>,
}
impl AnadaApp {
    fn new(context: egui::Context, demo: bool) -> Self {
        let client = if demo {
            let mut host = demo::state();
            if std::env::var_os("ANADA_UI_STRESS").is_some() {
                demo::stress(&mut host);
            }
            if std::env::var_os("ANADA_UI_OUTPUTS").is_some() {
                demo::priority_stress(&mut host);
            }
            Client::with_backend(context.clone(), Duration::from_secs(1), move |command| {
                if let Some(command) = command {
                    demo::command(&mut host, command)?;
                }
                Ok(host.clone())
            })
        } else {
            Client::spawn(context.clone())
        };
        let (startup_tx, startup_rx) = mpsc::channel();
        let mut app = Self {
            mcp: mcp_ui::McpPanel::new(demo),
            meters: Default::default(),
            meter_received: None,
            clipped_at: None,
            closing: false,
            allow_close: false,
            capture_frames: 0,
            client,
            host: None,
            draft: None,
            base_revision: 0,
            submitted: None,
            dirty_since: None,
            connected: false,
            starting: true,
            save_error: None,
            conflict: false,
            notice: None,
            selected: "gaming".into(),
            settings: false,
            settings_tab: 0,
            eq_spectrum: true,
            editor: Editor::default(),
            volumes: HashMap::new(),
            fader_drag: None,
            mutes: HashMap::new(),
            quiets: HashMap::new(),
            routes: HashMap::new(),
            route_menu: None,
            presets: Vec::new(),
            preset_name: String::new(),
            preset_editor: false,
            preset_delete: false,
            startup: None,
            startup_rx,
            startup_tx,
            startup_pending: false,
            startup_desired: None,
            confirm_exit: false,
            demo,
            #[cfg(test)]
            pill_rects: HashMap::new(),
            #[cfg(test)]
            strip_rects: HashMap::new(),
            #[cfg(test)]
            fader_rects: HashMap::new(),
            #[cfg(test)]
            percentage_rects: HashMap::new(),
            #[cfg(test)]
            header_rects: HashMap::new(),
            #[cfg(test)]
            eq_scale_rects: HashMap::new(),
            #[cfg(test)]
            top_bar_rects: HashMap::new(),
            #[cfg(test)]
            quiet_rects: HashMap::new(),
        };
        if demo && std::env::var_os("ANADA_UI_OUTPUTS").is_some() {
            app.settings = true;
            app.settings_tab = 1;
        }
        if !demo {
            match crate::presets::PresetStore::default_store().and_then(|store| store.list()) {
                Ok(presets) => app.presets = presets,
                Err(error) => app.notice = Some((error, true)),
            }
        }
        let _ = app.client.start();
        app.startup_change(None, &context);
        app
    }
    fn submit(&mut self, command: Command, tag: Tag) -> Option<RequestId> {
        match self.client.submit(command, tag) {
            Ok(id) => Some(id),
            Err(error) => {
                self.notice = Some((error, true));
                None
            }
        }
    }
    fn poll(&mut self) {
        while let Ok(result) = self.startup_rx.try_recv() {
            self.startup = Some(result);
            self.startup_pending = false;
            self.startup_desired = None;
        }
        for outcome in self.client.drain() {
            if outcome.tag == Tag::Meters {
                match outcome.result {
                    Ok(state) => {
                        self.meters = state.audio_diagnostics.meters;
                        self.meter_received = Some(Instant::now());
                    }
                    Err(_) => {
                        self.meters = Default::default();
                        self.meter_received = None;
                    }
                }
                continue;
            }
            match outcome.result {
                Ok(state) => {
                    if self.host.as_ref().is_some_and(|previous| {
                        state.audio_diagnostics.clamped_samples
                            > previous.audio_diagnostics.clamped_samples
                    }) {
                        self.clipped_at = Some(Instant::now());
                    }
                    self.starting = false;
                    self.connected = outcome.tag != Tag::Stop;
                    let local = self.dirty_since.is_some()
                        || self.submitted.is_some()
                        || self.save_error.is_some();
                    if outcome.tag == Tag::Save {
                        if let Some(Command::SaveConfig { config, .. }) = &outcome.command
                            && self
                                .draft
                                .as_ref()
                                .is_some_and(|draft| same_config(draft, config))
                        {
                            self.draft = Some(state.config.clone());
                            self.dirty_since = None;
                        }
                        if self.submitted == Some(outcome.id) {
                            self.submitted = None;
                        }
                        self.base_revision = state.revision;
                        self.save_error = None;
                    } else if !local {
                        self.draft = Some(state.config.clone());
                        self.base_revision = state.revision;
                    } else if self.host.is_some()
                        && state.revision != self.base_revision
                        && self.submitted.is_none()
                    {
                        self.conflict = true;
                    }
                    for edit in self.volumes.values_mut() {
                        if edit.request == Some(outcome.id) {
                            edit.request = None;
                        }
                    }
                    self.volumes
                        .retain(|_, edit| edit.dirty || edit.request.is_some());
                    self.mutes.retain(|_, (_, id)| *id != outcome.id);
                    self.quiets.retain(|_, (_, id)| *id != outcome.id);
                    for route in self.routes.values_mut() {
                        if route.request == outcome.id {
                            route.applied = true;
                        }
                    }
                    let apps = group_applications(&state);
                    self.routes.retain(|key, intent| {
                        !intent.applied
                            || apps.iter().find(|app| &app.key == key).is_some_and(|app| {
                                app.channel_id.as_deref() != Some(intent.target.as_str())
                            })
                    });
                    self.host = Some(state);
                }
                Err(error) => {
                    if matches!(outcome.tag, Tag::Poll | Tag::Start) {
                        self.connected = false;
                        self.starting = false;
                    }
                    if outcome.tag == Tag::Save {
                        if self.submitted == Some(outcome.id) {
                            self.submitted = None;
                        }
                        self.save_error = Some(error.clone());
                        if error.contains("changed in another") {
                            self.conflict = true;
                        }
                    }
                    for intent in self.routes.values_mut() {
                        if intent.request == outcome.id {
                            intent.error = Some(error.clone());
                        }
                    }
                    for edit in self.volumes.values_mut() {
                        if edit.request == Some(outcome.id) {
                            edit.request = None;
                        }
                    }
                    self.volumes
                        .retain(|_, edit| edit.dirty || edit.request.is_some());
                    self.mutes.retain(|_, (_, id)| *id != outcome.id);
                    self.quiets.retain(|_, (_, id)| *id != outcome.id);
                    if outcome.tag != Tag::Poll {
                        self.notice = Some((error, true));
                    }
                }
            }
        }
    }
    fn changed(&mut self) {
        self.dirty_since.get_or_insert_with(Instant::now);
        self.save_error = None;
    }
    fn flush(&mut self, pointer_down: bool) {
        if self.connected
            && !self.conflict
            && self.save_error.is_none()
            && self
                .dirty_since
                .is_some_and(|since| since.elapsed() >= Duration::from_millis(150))
            && let Some(config) = self.draft.clone()
        {
            match config.validate() {
                Ok(()) => {
                    if let Some(id) = self.submit(
                        Command::SaveConfig {
                            config,
                            expected_revision: self.base_revision,
                        },
                        Tag::Save,
                    ) {
                        self.submitted = Some(id);
                        self.dirty_since = None;
                    }
                }
                Err(error) => self.save_error = Some(error),
            }
        }
        let controls: Vec<_> = self
            .volumes
            .iter()
            .filter(|(_, e)| {
                e.dirty && (!pointer_down || e.last_sent.elapsed() >= Duration::from_millis(50))
            })
            .map(|(id, e)| (id.clone(), e.value, e.db))
            .collect();
        for (endpoint_id, value, db) in controls {
            let command = if db {
                Command::SetVolumeDb {
                    endpoint_id: endpoint_id.clone(),
                    level_db: value,
                }
            } else {
                Command::SetVolume {
                    endpoint_id: endpoint_id.clone(),
                    volume: value / 100.0,
                }
            };
            if let Some(request) = self.submit(command, Tag::Other)
                && let Some(edit) = self.volumes.get_mut(&endpoint_id)
            {
                edit.request = Some(request);
                edit.sent = Some(value);
                edit.last_sent = Instant::now();
                edit.dirty = false;
            }
        }
    }
    fn route(&mut self, key: &str, target: &str, host: &HostState) {
        if !self.connected || !binding_ready(host, target) {
            return;
        }
        if let Some(app) = group_applications(host)
            .into_iter()
            .find(|app| app.key == key)
            && let Some(request) = self.submit(
                Command::AssignApplications {
                    process_ids: app.process_ids,
                    channel_id: target.into(),
                },
                Tag::Assignment,
            )
        {
            self.routes.insert(
                key.into(),
                RouteIntent {
                    target: target.into(),
                    request,
                    since: Instant::now(),
                    applied: false,
                    error: None,
                },
            );
        }
    }
    fn startup_change(&mut self, change: Option<bool>, context: &egui::Context) {
        if self.startup_pending {
            return;
        }
        if self.demo {
            self.startup = Some(Ok(change.unwrap_or(false)));
            return;
        }
        self.startup_pending = true;
        self.startup_desired = change;
        let sender = self.startup_tx.clone();
        let context = context.clone();
        std::thread::spawn(move || {
            let result = match change {
                Some(v) => crate::startup::set_enabled(v),
                None => crate::startup::enabled(),
            };
            let _ = sender.send(result);
            context.request_repaint();
        });
    }
    fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();
        let context = ui.ctx().clone();
        self.client.set_metering(
            self.connected
                && self.host.is_some()
                && !self.settings
                && !context.input(|i| i.viewport().minimized.unwrap_or(false)),
        );
        if !self.allow_close
            && context.input(|i| i.viewport().close_requested())
            && (self.dirty_since.is_some() || self.submitted.is_some() || self.save_error.is_some())
        {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = true;
            if self.dirty_since.is_some() {
                self.dirty_since = Some(Instant::now() - Duration::from_secs(1));
            }
        }
        let host = self.host.clone();
        let mut draft = self.draft.take();
        egui::Panel::top("anada-header")
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(16, 9)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Anada").size(22.0).strong());
                    ui.add_space(12.0);
                    let (state, color) = if self.demo {
                        ("DEMO · AUDIO OFF", WARN)
                    } else if self.starting {
                        ("Starting host", MUTED)
                    } else if self.connected {
                        ("Host connected", GOOD)
                    } else {
                        ("Disconnected", BAD)
                    };
                    ui.label(RichText::new(format!("● {state}")).size(12.0).color(color))
                        .on_hover_text(
                            host.as_ref()
                                .map(|host| host.audio_status.as_str())
                                .unwrap_or("Connecting to the audio host"),
                        );
                    if self.clipped_at.is_some_and(|at| at.elapsed() < Duration::from_secs(5)) {
                        ui.colored_label(WARN, "Clipping").on_hover_text("Lower the channel volume or EQ preamp. Details are in Settings → Startup & host.");
                        context.request_repaint_after(Duration::from_secs(1));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let settings = ui.add(
                            egui::Button::new(if self.settings {
                                "← Mixer"
                            } else {
                                "Settings"
                            })
                            .selected(self.settings),
                        );
                        if settings.clicked() {
                            self.settings = !self.settings;
                        }
                        let sound = ui
                            .button("Windows Sound")
                            .on_hover_text("Open the Windows Sound control panel (playback devices)");
                        #[cfg(test)]
                        {
                            self.top_bar_rects.insert("settings".into(), settings.rect);
                            self.top_bar_rects.insert("sound".into(), sound.rect);
                        }
                        if sound.clicked()
                            && let Err(error) = open_sound_panel()
                        {
                            self.notice = Some((
                                format!("Could not open the Windows Sound control panel: {error}"),
                                true,
                            ));
                        }
                    });
                });
                if self.mcp.banner(ui) {
                    self.settings = true;
                    self.settings_tab = 4;
                }
            });
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                if let Some(error) = self.save_error.clone() {
                    Frame::new()
                        .fill(PANEL)
                        .inner_margin(10)
                        .corner_radius(6)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(BAD, format!("Changes not saved: {error}"));
                                if ui.button("Retry").clicked() {
                                    self.save_error = None;
                                    self.dirty_since =
                                        Some(Instant::now() - Duration::from_secs(1));
                                }
                            });
                        });
                    ui.add_space(8.0);
                }
                if let Some((text, failure)) = self.notice.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(text).color(if failure { BAD } else { GOOD }));
                        if ui.small_button("Dismiss").clicked() {
                            self.notice = None;
                        }
                    });
                    ui.add_space(6.0);
                }
                if !self.connected {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(if self.starting {
                                "Starting the audio host…"
                            } else {
                                "The audio host is unavailable. Reconnect to use the mixer."
                            })
                            .color(WARN),
                        );
                        if !self.starting && ui.button("Reconnect").clicked() {
                            let _ = self.client.start();
                            self.starting = true;
                        }
                    });
                    ui.add_space(8.0);
                }
                if self.conflict {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Settings changed elsewhere.").color(WARN));
                        if ui.button("Use current").clicked()
                            && let Some(state) = &host
                        {
                            draft = Some(state.config.clone());
                            self.base_revision = state.revision;
                            self.conflict = false;
                            self.dirty_since = None;
                            self.save_error = None;
                        }
                        if ui.button("Keep my edits").clicked()
                            && let Some(state) = &host
                        {
                            self.base_revision = state.revision;
                            self.conflict = false;
                            self.changed();
                        }
                    });
                }
                if let (Some(host), Some(draft)) = (&host, draft.as_mut()) {
                    let before = draft.clone();
                    if self.settings {
                        self.settings_view(ui, host, draft);
                    } else {
                        self.defaults_notice(ui, host);
                        ui.add_enabled_ui(self.connected, |ui| self.mixer_view(ui, host, draft));
                    }
                    if !same_config(&before, draft) {
                        self.changed();
                    }
                } else {
                    ui.add_space(40.0);
                    ui.heading("Connecting to Anada");
                    ui.label("Your channels and listening output will appear here.");
                }
            });
        if let Some(host) = &host {
            self.application_menu(&context, host);
        }
        self.draft = draft;
        self.flush(context.input(|i| i.pointer.any_down()));
        if self.closing {
            if self.dirty_since.is_none() && self.submitted.is_none() && self.save_error.is_none() {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                egui::Window::new("Saving before closing")
                    .collapsible(false)
                    .resizable(false)
                    .show(&context, |ui| {
                        ui.label("Anada will close when your changes are saved.");
                        if let Some(error) = &self.save_error {
                            ui.colored_label(BAD, error);
                        }
                        if !self.connected {
                            ui.colored_label(WARN, "The audio host is unavailable.");
                        }
                        if ui.button("Keep editing").clicked() {
                            self.closing = false;
                        }
                        if ui.button("Discard unsaved edits and close").clicked() {
                            self.closing = false;
                            self.allow_close = true;
                            context.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
            }
        }
        if self.demo
            && self.host.is_some()
            && let Some(path) = std::env::var_os("ANADA_UI_SCREENSHOT")
        {
            self.capture_frames = self.capture_frames.saturating_add(1);
            if self.capture_frames < 8 {
                context.request_repaint_after(Duration::from_millis(60));
            }
            if self.capture_frames == 8 {
                context.request_screenshot(move |image| {
                    let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                    if let Err(error) = image::save_buffer(
                        std::path::Path::new(&path),
                        &rgba,
                        image.size[0] as u32,
                        image.size[1] as u32,
                        image::ColorType::Rgba8,
                    ) {
                        eprintln!("UI screenshot: {error}");
                    }
                });
            }
        }
        if self.client.has_pending() || self.dirty_since.is_some() {
            context.request_repaint_after(Duration::from_millis(30));
        }
    }
}
impl eframe::App for AnadaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}
/// Opens the classic Sound control panel on its Playback tab.
fn open_sound_panel() -> Result<(), String> {
    #[cfg(not(windows))]
    {
        Err("the Sound control panel requires Windows".into())
    }
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        std::process::Command::new(std::path::Path::new(&root).join("System32\\control.exe"))
            .arg("mmsys.cpl,,0")
            .spawn()
            .map(drop)
            .map_err(|error| error.to_string())
    }
}
fn same_config(a: &Config, b: &Config) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}
fn channel_color(id: &str) -> Color32 {
    match id {
        "chat" => GOOD,
        "media" => Color32::from_rgb(211, 168, 112),
        "aux" => Color32::from_rgb(174, 156, 212),
        _ => ACCENT,
    }
}
fn binding_ready(host: &HostState, id: &str) -> bool {
    host.config
        .channels
        .iter()
        .find(|c| c.id == id)
        .is_some_and(|c| {
            c.endpoint_id.as_ref().is_some_and(|id| {
                host.windows
                    .outputs
                    .iter()
                    .any(|e| e.id == *id && e.is_virtual)
            }) && c.capture_id.as_ref().is_some_and(|id| {
                host.windows
                    .inputs
                    .iter()
                    .any(|e| e.id == *id && e.is_virtual)
            })
        })
}
fn name_of(windows: &WindowsSnapshot, id: Option<&str>) -> String {
    id.and_then(|id| {
        windows
            .outputs
            .iter()
            .chain(&windows.inputs)
            .find(|e| e.id == id)
    })
    .map(|e| e.name.clone())
    .unwrap_or_else(|| "No device".into())
}
fn bind_standard_vac(config: &mut Config, windows: &WindowsSnapshot) -> Result<(), String> {
    config.validate()?;
    if config
        .channels
        .iter()
        .any(|c| c.endpoint_id.is_some() || c.capture_id.is_some())
    {
        return Err(
            "Existing bindings were preserved. Use the individual menus to change them.".into(),
        );
    }
    let mut pairs = Vec::new();
    for (id, n) in [("gaming", 1), ("chat", 2), ("media", 3), ("aux", 4)] {
        let name = format!("Line {n} (Virtual Audio Cable)");
        let unique = |list: &[Endpoint]| -> Result<String, String> {
            let found: Vec<_> = list
                .iter()
                .filter(|e| e.is_virtual && e.name.eq_ignore_ascii_case(&name))
                .collect();
            if found.len() != 1 {
                return Err(format!(
                    "Need one playback and recording endpoint named '{name}'. Run Configure-VAC.cmd, or select renamed cables below. No bindings changed."
                ));
            }
            Ok(found[0].id.clone())
        };
        pairs.push((id, unique(&windows.outputs)?, unique(&windows.inputs)?));
    }
    for (id, output, input) in pairs {
        let c = config.channels.iter_mut().find(|c| c.id == id).unwrap();
        c.endpoint_id = Some(output);
        c.capture_id = Some(input);
    }
    Ok(())
}
fn theme(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/segoeui.ttf") {
        fonts
            .font_data
            .insert("Segoe UI".into(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "Segoe UI".into());
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.text_styles = [
        (egui::TextStyle::Heading, FontId::proportional(19.0)),
        (egui::TextStyle::Body, FontId::proportional(13.0)),
        (egui::TextStyle::Button, FontId::proportional(13.0)),
        (egui::TextStyle::Small, FontId::proportional(11.5)),
        (egui::TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();
    let v = &mut style.visuals;
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = FIELD;
    v.faint_bg_color = PANEL;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        w.bg_fill = RAISED;
        w.weak_bg_fill = RAISED;
        w.fg_stroke = Stroke::new(1.0, TEXT);
        w.corner_radius = CornerRadius::same(4);
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    ctx.set_style_of(egui::Theme::Dark, style);
}
