use super::*;
impl AnadaApp {
    pub(super) fn settings_view(
        &mut self,
        ui: &mut egui::Ui,
        host: &HostState,
        draft: &mut Config,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Settings");
            ui.label(RichText::new("Changes apply and save automatically").color(MUTED));
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for (index, title) in [
                "Channels",
                "Outputs",
                "Windows audio",
                "Startup & host",
                "Agent access (MCP)",
            ]
            .iter()
            .enumerate()
            {
                ui.selectable_value(&mut self.settings_tab, index, *title);
            }
        });
        ui.separator();
        if self.settings_tab == 4 {
            self.mcp.show(ui);
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false,false]).show(ui,|ui|{if self.settings_tab != 1 {ui.set_max_width(900.0);}match self.settings_tab {
            0=>{
                ui.heading("Channel strips");ui.label("Names, visibility and order keep the same Windows endpoints.");
                let mut operation=None;
                for(index,channel)in draft.channels.iter_mut().enumerate(){
                    Frame::new().fill(PANEL).inner_margin(12).corner_radius(6).show(ui,|ui|{ui.horizontal_wrapped(|ui|{
                        ui.label(RichText::new("●").color(channel_color(&channel.id)));
                        ui.add(egui::TextEdit::singleline(&mut channel.name).char_limit(48).desired_width(180.0));
                        ui.checkbox(&mut channel.visible,"Show strip");
                        if ui.add_enabled(index>0,egui::Button::new("Move left / up")).clicked(){operation=Some((index,index-1));}
                        if ui.add_enabled(index<3,egui::Button::new("Move right / down")).clicked(){operation=Some((index,index+1));}
                    });});ui.add_space(6.0);
                }
                if let Some((a,b))=operation{draft.channels.swap(a,b);}
                ui.add_space(18.0);
                ui.heading("Quiet button");
                ui.horizontal(|ui| {
                    crate::ui_numeric::resettable_numeric(ui, "Quiet level %", &mut draft.quiet_volume_percent, 1.0..=100.0, 1.0, 20.0);
                });
                ui.label(RichText::new("Quiet lowers a channel or the output to this Windows volume. Click Quiet again to return to the earlier volume. It never raises a volume, and it turns off when you move the fader or change the output device.").small().color(MUTED));
            }
            1=>self.output_settings(ui,host,draft),
            2=>{
                ui.heading("Windows audio");ui.label(format!("Windows default: {}",name_of(&host.windows,host.windows.default_output_id.as_deref())));
                ui.label(format!("Communications default: {}",name_of(&host.windows,host.windows.communications_output_id.as_deref())));
                if ui.add_enabled(self.connected&&binding_ready(host,"gaming")&&binding_ready(host,"chat"),egui::Button::new("Make Anada default")).clicked(){self.submit(Command::RepairDefaults,Tag::Defaults);}
                ui.label(RichText::new("Gaming becomes the ordinary default. Chat becomes the communications default.").small().color(MUTED));ui.add_space(20.0);
                ui.heading("Endpoint bindings");
                if draft.channels.iter().all(|c|c.endpoint_id.is_none()&&c.capture_id.is_none())&&ui.button("Bind standard VAC Lines 1-4").clicked(){
                    match bind_standard_vac(draft,&host.windows){Ok(())=>self.notice=Some(("Four VAC pairs connected. Applying settings…".into(),false)),Err(e)=>self.notice=Some((e,true))}
                }
                ui.label("Each channel uses a matching VAC playback and recording pair. Listening outputs stay physical.");
                for i in 0..draft.channels.len(){
                    let taken_outputs:Vec<_>=draft.channels.iter().enumerate().filter(|(j,_)|*j!=i).filter_map(|(_,c)|c.endpoint_id.clone()).collect();
                    let taken_inputs:Vec<_>=draft.channels.iter().enumerate().filter(|(j,_)|*j!=i).filter_map(|(_,c)|c.capture_id.clone()).collect();
                    let channel=&mut draft.channels[i];ui.add_space(8.0);ui.label(RichText::new(&channel.name).color(channel_color(&channel.id)).strong());
                    ui.horizontal_wrapped(|ui|{endpoint_picker(ui,(&channel.id,"playback"),"Playback",&host.windows.outputs,&taken_outputs,&mut channel.endpoint_id);endpoint_picker(ui,(&channel.id,"recording"),"Recording",&host.windows.inputs,&taken_inputs,&mut channel.capture_id);});
                }
            }
            _=>{
                ui.heading("Startup");let context=ui.ctx().clone();
                match self.startup.clone(){Some(Ok(mut enabled))=>{enabled=self.startup_desired.unwrap_or(enabled);if ui.add_enabled(!self.startup_pending,egui::Checkbox::new(&mut enabled,"Start Anada at sign-in")).changed(){self.startup_change(Some(enabled),&context);}},Some(Err(e))=>{ui.label(RichText::new(e).color(BAD));if ui.button("Read startup setting again").clicked(){self.startup_change(None,&context);}},None=>{ui.label("Reading startup setting…");}}
                if self.startup_pending { ui.spinner(); ui.label("Updating Windows startup..."); }
                ui.label(RichText::new("Starts the audio host and tray. The window stays closed.").small().color(MUTED));ui.add_space(24.0);
                ui.heading("Audio host");ui.label("Closing the window leaves audio running. Exiting the host stops listening.");
                if self.confirm_exit{ui.horizontal_wrapped(|ui|{ui.label(RichText::new("Stop Anada audio?").color(WARN));if ui.button("Exit host").clicked(){self.submit(Command::StopHost,Tag::Stop);self.confirm_exit=false;}
if ui.button("Keep running").clicked(){self.confirm_exit=false;}});}else if ui.button("Exit host…").clicked(){self.confirm_exit=true;}
                ui.add_space(24.0);ui.heading("Diagnostics");ui.label(format!("Anada {}",env!("CARGO_PKG_VERSION")));ui.label(&host.audio_status);ui.label(format!("Clipped samples: {}",host.audio_diagnostics.clamped_samples));
                ui.label(format!("Stream opens {}  ·  faults {}  ·  dropped frames {}  ·  underruns {}",host.audio_diagnostics.stream_opens,host.audio_diagnostics.stream_faults,host.audio_diagnostics.dropped_frames,host.audio_diagnostics.underruns));
                for warning in &host.warnings{ui.label(RichText::new(warning).color(WARN));}
            }
        }});
    }
    pub(super) fn eq_panel(
        &mut self,
        ui: &mut egui::Ui,
        host: &HostState,
        draft: &mut Config,
        height: f32,
    ) {
        let selected = self.selected.clone();
        let fresh = self.connected
            && self.meters.running
            && self
                .meter_received
                .is_some_and(|at| at.elapsed() < Duration::from_millis(300));
        let spectrum = if selected == "output" {
            Some(&self.meters.output_spectrum)
        } else {
            self.meters
                .channels
                .iter()
                .find(|meter| meter.channel_id == selected)
                .map(|meter| &meter.spectrum)
        }
        .filter(|spectrum| fresh && spectrum.sample_rate > 0 && !spectrum.dbfs.is_empty())
        .cloned();
        let spectrum_status = if !self.eq_spectrum {
            "Spectrum hidden"
        } else if self.demo {
            "Demo · no audio"
        } else if !host.audio_running || !self.connected {
            "Audio stopped · spectrum idle"
        } else if spectrum
            .as_ref()
            .is_some_and(|s| s.dbfs.iter().any(|db| *db > -90.0))
        {
            "Live · after EQ"
        } else {
            "No signal · after EQ"
        };
        self.editor.analysis(
            spectrum.as_ref(),
            self.eq_spectrum,
            spectrum
                .as_ref()
                .map(|s| s.sample_rate)
                .unwrap_or(host.audio_diagnostics.sample_rate.max(48_000)),
        );
        let title = if selected == "output" {
            // Output EQ belongs to one device; name it so switching devices is not a surprise.
            let device = name_of(&host.windows, host.selected_output_id.as_deref());
            let device = if device.chars().count() > 40 {
                format!("{}…", device.chars().take(39).collect::<String>())
            } else {
                device
            };
            format!("Output ({device})")
        } else {
            draft
                .channels
                .iter()
                .find(|c| c.id == selected)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "Gaming".into())
        };
        let mut working = if selected == "output" {
            let Some(id) = &host.selected_output_id else {
                ui.label("Choose a listening output to edit its EQ.");
                return;
            };
            draft.output_eq.get(id).cloned().unwrap_or_default()
        } else {
            let Some(channel) = draft.channels.iter().find(|c| c.id == selected) else {
                return;
            };
            channel.eq.clone()
        };
        let before = working.clone();
        let eq = &mut working;
        let editor_target = if selected == "output" {
            format!(
                "output:{}",
                host.selected_output_id.as_deref().unwrap_or_default()
            )
        } else {
            selected.clone()
        };
        let panel_bottom = ui.cursor().min.y + height - 24.0;
        let compact = ui.available_width() < 800.0;
        Frame::new()
            .fill(PANEL)
            .inner_margin(12)
            .corner_radius(6)
            .stroke(Stroke::new(1.0, EDGE))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let heading = ui.label(
                        RichText::new(format!("Equalizer · {title}"))
                            .size(17.0)
                            .strong(),
                    );
                    if selected == "output" {
                        heading.on_hover_text(
                            "Output EQ is saved separately for each output device. Choosing another device loads that device's EQ.",
                        );
                    }
                    ui.add_space(8.0);
                    let mut recalled = None;
                    egui::ComboBox::from_id_salt("eq-presets")
                        .selected_text(if self.preset_name.is_empty() {
                            "Presets…"
                        } else {
                            &self.preset_name
                        })
                        .width(155.0)
                        .show_ui(ui, |ui| {
                            if self.presets.is_empty() {
                                ui.label("No saved presets");
                            }
                            for preset in &self.presets {
                                if ui.selectable_label(false, &preset.name).clicked() {
                                    recalled = Some(preset.clone());
                                }
                            }
                        });
                    if let Some(preset) = recalled {
                        *eq = preset.eq;
                        self.preset_name = preset.name;
                    }
                    if ui.button("Save preset…").clicked() {
                        self.preset_editor = !self.preset_editor;
                    }
                    if ui.button("Reset").clicked() {
                        *eq = EqSettings::default();
                    }
                    ui.checkbox(&mut eq.bypass, "Bypass");
                    if !compact { crate::ui_numeric::resettable_numeric(
                        ui,
                        "Preamp dB",
                        &mut eq.preamp_db,
                        -24.0..=12.0,
                        0.1,
                        0.0,
                    ); }
                });
                if self.preset_editor {
                    self.preset_controls(ui, eq);
                }
                ui.horizontal(|ui| {
                    if compact { crate::ui_numeric::resettable_numeric(
                        ui, "Preamp dB", &mut eq.preamp_db, -24.0..=12.0, 0.1, 0.0,
                    ); }
                    ui.checkbox(&mut self.eq_spectrum, "Spectrum");
                    let status_width = (ui.available_width() - 124.0).max(0.0);
                    ui.add_sized([status_width, ui.spacing().interact_size.y], egui::Label::new(RichText::new(spectrum_status).small().color(MUTED)).truncate())
                        .on_hover_text("The left dB axis shows EQ gain. The right dBFS axis shows the audio spectrum after EQ.");
                    let range = egui::ComboBox::from_id_salt("eq-graph-range")
                        .width(108.0)
                        .selected_text(format!("EQ ±{} dB", draft.eq_graph_range_db))
                        .show_ui(ui, |ui| {
                            for range in crate::model::EQ_GRAPH_RANGES_DB {
                                let choice = ui.selectable_value(&mut draft.eq_graph_range_db, range, format!("±{range} dB"));
                                #[cfg(test)]
                                self.eq_scale_rects.insert(range.to_string(), choice.rect);
                                #[cfg(not(test))]
                                let _ = choice;
                            }
                        }).response.on_hover_text("EQ graph scale. Saved for all channels and outputs; audio gain and the spectrum dBFS scale stay unchanged.");
                    #[cfg(test)]
                    self.eq_scale_rects.insert("control".into(), range.rect);
                    #[cfg(not(test))]
                    let _ = range;
                });
                self.editor.set_range_db(draft.eq_graph_range_db);
                ui.add_space(8.0);
                // Use the band controls' measured height so the panel ends inside the
                // window; the estimate covers only the first frame.
                let controls_height = if self.editor.controls_height > 0.0 {
                    self.editor.controls_height
                } else if ui.available_width() < 680.0 {
                    64.0
                } else {
                    48.0
                };
                self.editor.show(
                    ui,
                    &editor_target,
                    eq,
                    (panel_bottom - ui.cursor().min.y - controls_height).max(360.0),
                    channel_color(&selected),
                );
            });
        if working != before {
            if selected == "output" {
                if let Some(id) = &host.selected_output_id {
                    draft.output_eq.insert(id.clone(), working);
                }
            } else if let Some(channel) = draft.channels.iter_mut().find(|c| c.id == selected) {
                channel.eq = working;
            }
        }
    }
    fn preset_controls(&mut self, ui: &mut egui::Ui, eq: &EqSettings) {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.preset_name)
                    .hint_text("Preset name")
                    .char_limit(64)
                    .desired_width(180.0),
            );
            let exists = self.presets.iter().any(|p| p.name == self.preset_name);
            if ui
                .button(if exists {
                    "Replace preset"
                } else {
                    "Save new preset"
                })
                .clicked()
            {
                if self.demo {
                    self.presets.retain(|p| p.name != self.preset_name);
                    self.presets.push(crate::presets::EqPreset {
                        name: self.preset_name.clone(),
                        eq: eq.clone(),
                    });
                } else {
                    match crate::presets::PresetStore::default_store()
                        .and_then(|s| s.save(&self.preset_name, eq, exists))
                    {
                        Ok(p) => {
                            self.presets = p;
                            self.notice = Some(("EQ preset saved.".into(), false));
                        }
                        Err(e) => self.notice = Some((e, true)),
                    }
                }
            }
            if ui
                .add_enabled(exists, egui::Button::new("Delete…"))
                .clicked()
            {
                self.preset_delete = true;
            }
            if self.preset_delete {
                ui.label("Delete this preset?");
                if ui.button("Delete preset").clicked() {
                    if self.demo {
                        self.presets.retain(|p| p.name != self.preset_name);
                    } else {
                        match crate::presets::PresetStore::default_store()
                            .and_then(|s| s.remove(&self.preset_name))
                        {
                            Ok(p) => self.presets = p,
                            Err(e) => self.notice = Some((e, true)),
                        }
                    }
                    self.preset_delete = false;
                }
                if ui.button("Cancel").clicked() {
                    self.preset_delete = false;
                }
            }
        });
    }
}
fn endpoint_picker(
    ui: &mut egui::Ui,
    salt: (&str, &str),
    label: &str,
    options: &[Endpoint],
    taken: &[String],
    selected: &mut Option<String>,
) {
    ui.label(label);
    let text = selected
        .as_ref()
        .and_then(|id| options.iter().find(|e| &e.id == id))
        .map(|e| e.name.as_str())
        .unwrap_or("Not connected");
    egui::ComboBox::from_id_salt(salt)
        .selected_text(text)
        .width(260.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(selected.is_none(), "Not connected")
                .clicked()
            {
                *selected = None;
            }
            for e in options
                .iter()
                .filter(|e| e.is_virtual && !taken.contains(&e.id))
            {
                if ui
                    .selectable_label(selected.as_deref() == Some(e.id.as_str()), &e.name)
                    .clicked()
                {
                    *selected = Some(e.id.clone());
                }
            }
        });
}
