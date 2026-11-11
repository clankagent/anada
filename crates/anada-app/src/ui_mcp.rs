use super::*;
use crate::mcp_session::Session;
use crate::mcp_types::{Permission, Preferences, PublicSessionStatus};

pub(super) struct McpPanel {
    preferences: Preferences,
    session: Session,
    demo: bool,
    demo_until: Option<Instant>,
    error: Option<String>,
    ended: Option<&'static str>,
    was_active: bool,
    copied_at: Option<Instant>,
}

impl McpPanel {
    pub(super) fn new(demo: bool) -> Self {
        let loaded = if demo {
            Ok(Preferences::default())
        } else {
            Preferences::load()
        };
        let (preferences, error) = match loaded {
            Ok(value) => (value, None),
            Err(error) => (
                Preferences::default(),
                Some(format!("Could not load saved access choices: {error}")),
            ),
        };
        Self {
            preferences,
            session: Session::new(),
            demo,
            demo_until: None,
            error,
            ended: None,
            was_active: false,
            copied_at: None,
        }
    }

    fn status(&mut self) -> PublicSessionStatus {
        let status = if self.demo {
            let remaining = self
                .demo_until
                .and_then(|until| until.checked_duration_since(Instant::now()));
            PublicSessionStatus {
                active: remaining.is_some(),
                remaining_seconds: remaining.map_or(0, |remaining| remaining.as_secs()),
                granted: self.preferences.granted.clone(),
                activity: Vec::new(),
            }
        } else {
            self.session.status()
        };
        if self.was_active && !status.active && self.ended.is_none() {
            self.ended = Some("Session expired. Access is off.");
        }
        self.was_active = status.active;
        status
    }

    fn stop(&mut self) {
        self.session.stop();
        self.demo_until = None;
        self.was_active = false;
        self.ended = Some("Session stopped. Access is off.");
    }

    fn start(&mut self) {
        self.error = None;
        let result = if self.demo {
            self.demo_until = Some(
                Instant::now()
                    + Duration::from_secs(u64::from(self.preferences.duration_minutes) * 60),
            );
            Ok(())
        } else {
            self.preferences.save().and_then(|()| {
                self.session
                    .start(self.preferences.clone(), crate::mcp_tools::execute)
            })
        };
        match result {
            Ok(()) => {
                self.ended = None;
                self.was_active = true;
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// The stop control remains reachable even outside Settings.
    pub(super) fn banner(&mut self, ui: &mut egui::Ui) -> bool {
        let status = self.status();
        if !status.active {
            return false;
        }
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        let mut manage = false;
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                WARN,
                format!(
                    "MCP active · {}:{:02} left",
                    status.remaining_seconds / 60,
                    status.remaining_seconds % 60
                ),
            );
            if ui.button("Permissions").clicked() {
                manage = true;
            }
            if ui.button(RichText::new("Stop MCP").color(BAD)).clicked() {
                self.stop();
            }
        });
        manage
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        let status = self.status();
        ui.set_max_width(ui.available_width().min(1100.0));
        ui.heading("Agent access (MCP)");
        ui.label(
            "Choose what a local agent may read or change. Only the checked permissions apply.",
        );
        ui.add_space(10.0);
        Frame::new().fill(PANEL).stroke(Stroke::new(1.0, if status.active { WARN } else { EDGE }))
            .inner_margin(12).corner_radius(6).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if status.active {
                        ui.label(RichText::new(format!("● Session active · {}:{:02} left", status.remaining_seconds / 60, status.remaining_seconds % 60)).color(GOOD).strong());
                        if ui.add(egui::Button::new(RichText::new("Stop MCP").color(Color32::BLACK)).fill(BAD).min_size(egui::vec2(112.0, 32.0))).clicked() { self.stop(); }
                    } else {
                        ui.label(RichText::new("● Agent access is off").strong());
                        egui::ComboBox::from_id_salt("mcp-session-duration").selected_text(format!("{} minutes", self.preferences.duration_minutes)).show_ui(ui, |ui| {
                            for minutes in [15, 30, 60, 120] { ui.selectable_value(&mut self.preferences.duration_minutes, minutes, format!("{minutes} minutes")); }
                        });
                        if ui.add_enabled(!self.preferences.granted.is_empty(), egui::Button::new(format!("Enable session · {} permissions", self.preferences.granted.len())).min_size(egui::vec2(190.0, 32.0))).clicked() { self.start(); }
                    }
                });
                ui.label(RichText::new("Local only. Stops when the timer ends, you press Stop MCP, or you close this window.").small().color(MUTED));
                if !status.active { ui.label(RichText::new("Your choices are remembered. Access always starts off.").small().color(MUTED)); }
                if self.demo { ui.label(RichText::new("Demo · no MCP server is opened").small().color(WARN)); }
                if let Some(message) = self.ended { ui.label(RichText::new(message).small().color(MUTED)); }
                if let Some(error) = &self.error { ui.label(RichText::new(error).color(BAD)); }
            });
        ui.add_space(10.0);
        let status = self.status();
        egui::ScrollArea::vertical().id_salt("mcp-permissions-scroll").auto_shrink([false, false]).show(ui, |ui| {
            if status.active {
                ui.label("Permissions are fixed for this session. Stop MCP to edit them.");
                egui::CollapsingHeader::new(format!("Granted permissions · {} of {}", status.granted.len(), Permission::ALL.len())).id_salt("mcp-active-permissions").show(ui, |ui| {
                    ui.add_enabled_ui(false, |ui| self.permission_grid(ui));
                });
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            } else {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("EQ assistant").clicked() { self.eq_assistant(); }
                    if ui.button("Read-only").clicked() { self.read_only(); }
                    if ui.button("Select all").clicked() { self.preferences.granted = Permission::ALL.into_iter().collect(); }
                    if ui.button("Clear").clicked() { self.preferences.granted.clear(); }
                    ui.label(RichText::new(format!("{} of {} selected", self.preferences.granted.len(), Permission::ALL.len())).small().color(MUTED));
                });
                ui.add_space(8.0);
                self.permission_grid(ui);
            }
            ui.add_space(14.0);
            ui.heading("Connect your agent");
            ui.label("Enable a session, then add this stdio server in your agent's MCP settings. Reconnect the agent after enabling a new session.");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Copy MCP configuration").clicked() { ui.ctx().copy_text(connection_json()); self.copied_at = Some(Instant::now()); }
                if self.copied_at.is_some_and(|at| at.elapsed() < Duration::from_secs(2)) {
                    ui.label(RichText::new("Copied").color(GOOD));
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                }
                ui.label(RichText::new("No password or token is stored in this configuration.").small().color(MUTED));
            });
            egui::CollapsingHeader::new("Show connection configuration").id_salt("mcp-connection-config").show(ui, |ui| { ui.code(connection_json()); });
            ui.add_space(14.0);
            ui.heading("Recent agent activity");
            if status.activity.is_empty() { ui.label(RichText::new("No agent activity in this session.").color(MUTED)); }
            else {
                for activity in status.activity.iter().rev().take(30) {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(if activity.success { GOOD } else { BAD }, if activity.success { "OK" } else { "Denied / error" });
                        ui.monospace(&activity.tool);
                    });
                }
            }
        });
    }

    fn read_only(&mut self) {
        self.preferences.granted = [
            Permission::ReadStatus,
            Permission::ReadDevices,
            Permission::ReadApplications,
            Permission::ReadEq,
            Permission::ReadLevels,
            Permission::ReadSpectrum,
            Permission::ReadPresets,
            Permission::ReadChannelSettings,
            Permission::ReadOutputSettings,
        ]
        .into_iter()
        .collect();
    }

    fn eq_assistant(&mut self) {
        self.preferences.granted = [
            Permission::ReadStatus,
            Permission::ReadDevices,
            Permission::ReadEq,
            Permission::ReadLevels,
            Permission::ReadSpectrum,
            Permission::ReadPresets,
            Permission::WriteChannelEq,
            Permission::WriteOutputEq,
        ]
        .into_iter()
        .collect();
    }

    fn permission_grid(&mut self, ui: &mut egui::Ui) {
        if ui.available_width() >= 900.0 {
            ui.columns(2, |columns| {
                for group in ["Read", "Equalizers and presets"] {
                    self.permission_group(&mut columns[0], group);
                }
                for group in ["Volume and mute", "Routing", "Channels", "System"] {
                    self.permission_group(&mut columns[1], group);
                }
            });
        } else {
            for group in [
                "Read",
                "Equalizers and presets",
                "Volume and mute",
                "Routing",
                "Channels",
                "System",
            ] {
                self.permission_group(ui, group);
            }
        }
    }

    fn permission_group(&mut self, ui: &mut egui::Ui, group: &str) {
        Frame::new()
            .fill(PANEL)
            .inner_margin(10)
            .corner_radius(6)
            .show(ui, |ui| {
                ui.label(RichText::new(group).strong());
                for permission in Permission::ALL
                    .into_iter()
                    .filter(|permission| permission.group() == group)
                {
                    let mut checked = self.preferences.granted.contains(&permission);
                    if ui
                        .checkbox(&mut checked, permission.label())
                        .on_hover_text(permission.description())
                        .changed()
                    {
                        if checked {
                            self.preferences.granted.insert(permission);
                        } else {
                            self.preferences.granted.remove(&permission);
                        }
                    }
                }
            });
        ui.add_space(8.0);
    }
}

fn connection_json() -> String {
    let executable = std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "anada.exe".into());
    serde_json::to_string_pretty(
        &serde_json::json!({"mcpServers":{"anada":{"command":executable,"args":["--mcp"]}}}),
    )
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn choices_never_enable_access_and_demo_stop_is_immediate() {
        let mut panel = McpPanel::new(true);
        assert!(panel.preferences.granted.is_empty());
        panel.eq_assistant();
        assert!(!panel.status().active);
        assert!(
            panel
                .preferences
                .granted
                .contains(&Permission::WriteChannelEq)
        );
        assert!(
            !panel
                .preferences
                .granted
                .contains(&Permission::OutputVolume)
        );
        panel.start();
        assert!(panel.status().active);
        panel.stop();
        assert!(!panel.status().active);
        panel.read_only();
        assert!(
            panel
                .preferences
                .granted
                .iter()
                .all(|permission| permission.group() == "Read")
        );
    }
}
