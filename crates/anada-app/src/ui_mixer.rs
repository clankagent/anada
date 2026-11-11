use super::*;
use crate::applications::{GroupedApplication, group_applications};

impl AnadaApp {
    pub(super) fn defaults_notice(&mut self, ui: &mut egui::Ui, host: &HostState) {
        let gaming = host
            .config
            .channels
            .iter()
            .find(|c| c.id == "gaming")
            .and_then(|c| c.endpoint_id.as_deref());
        let chat = host
            .config
            .channels
            .iter()
            .find(|c| c.id == "chat")
            .and_then(|c| c.endpoint_id.as_deref());
        if gaming.is_some()
            && (host.windows.default_output_id.as_deref() != gaming
                || host.windows.communications_output_id.as_deref() != chat)
        {
            Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0, WARN))
                .inner_margin(10)
                .corner_radius(6)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Windows defaults differ. New audio may bypass Anada.")
                                .color(WARN),
                        );
                        if ui
                            .add_enabled(
                                self.connected
                                    && binding_ready(host, "gaming")
                                    && binding_ready(host, "chat"),
                                egui::Button::new("Repair defaults"),
                            )
                            .clicked()
                        {
                            self.submit(Command::RepairDefaults, Tag::Defaults);
                        }
                    });
                });
            ui.add_space(8.0);
        }
    }

    pub(super) fn mixer_view(&mut self, ui: &mut egui::Ui, host: &HostState, draft: &mut Config) {
        ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                egui::Id::unique("application-drop-rects"),
                Vec::<(egui::Rect, String)>::new(),
            )
        });
        let apps = group_applications(host);
        let assigned = |app: &GroupedApplication| {
            self.routes
                .get(&app.key)
                .map(|route| route.target.clone())
                .or_else(|| app.channel_id.clone())
        };
        let other: Vec<_> = apps
            .iter()
            .filter(|app| assigned(app).is_none())
            .cloned()
            .collect();
        let viewport_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt("mixer-workspace")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let content_top = ui.cursor().min.y;

                let channels: Vec<_> = draft
                    .channels
                    .iter()
                    .filter(|c| c.visible)
                    .cloned()
                    .collect();
                let count = channels.len().max(1);
                let gap = 8.0;
                let strip_width = ((ui.available_width() - gap * (count - 1) as f32)
                    / count as f32)
                    .floor()
                    .max(220.0);
                egui::ScrollArea::horizontal()
                    .id_salt("mixer-strips")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        ui.horizontal_top(|ui| {
                            for channel in &channels {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(strip_width, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(strip_width);
                                        self.channel_strip(ui, host, channel, &apps);
                                    },
                                );
                            }
                        });
                    });
                ui.add_space(10.0);
                self.output_and_apps_row(ui, host, &other);
                ui.add_space(12.0);
                // Keep a useful editor. Scroll instead of reorienting strips or hiding EQ.
                self.eq_panel(
                    ui,
                    host,
                    draft,
                    (viewport_height - (ui.cursor().min.y - content_top)).max(460.0),
                );
            });
        if self.connected && ui.input(|i| i.pointer.primary_released()) {
            let target = ui
                .ctx()
                .data(|data| {
                    data.get_temp::<Vec<(egui::Rect, String)>>(egui::Id::unique(
                        "application-drop-rects",
                    ))
                })
                .unwrap_or_default()
                .into_iter()
                .find(|(rect, _)| {
                    ui.input(|i| {
                        i.pointer
                            .interact_pos()
                            .is_some_and(|point| rect.contains(point))
                    })
                });
            if let Some((_, target)) = target
                && binding_ready(host, &target)
                && let Some(payload) = egui::DragAndDrop::take_payload::<ApplicationDrag>(ui.ctx())
            {
                self.route(&payload.key, &target, host);
            }
        }
    }

    fn app_pill(&mut self, ui: &mut egui::Ui, app: &GroupedApplication, host: &HostState) {
        let intent = self.routes.get(&app.key);
        let failed = intent.and_then(|r| r.error.as_ref()).is_some();
        let waiting = intent.is_some_and(|r| !r.applied && r.error.is_none());
        let mismatch =
            intent.is_some_and(|r| app.channel_id.as_ref() != Some(&r.target) || app.mixed);
        let border = if failed {
            BAD
        } else if mismatch || waiting {
            WARN
        } else {
            EDGE
        };
        let dot = if failed {
            BAD
        } else if mismatch || waiting {
            WARN
        } else if app.active {
            GOOD
        } else {
            MUTED
        };
        let name = app.name.strip_suffix(".exe").unwrap_or(&app.name);
        let suffix = if app.process_ids.len() > 1 {
            format!(" x{}", app.process_ids.len())
        } else {
            String::new()
        };
        let caption = if failed {
            "Routing failed - click to retry"
        } else if waiting {
            "Setting Windows preference..."
        } else if intent.is_some_and(|r| r.since.elapsed() < Duration::from_secs(5)) {
            "Waiting for app to switch..."
        } else {
            "Restart app to move"
        };
        // Size to the content, up to 240 px, so pills flow several to a row.
        let measure = |text: &str, size: f32| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), FontId::proportional(size), TEXT)
                .size()
                .x
        };
        let mut natural = 20.0 + measure(name, 13.0) + 10.0;
        if !suffix.is_empty() {
            natural += 6.0 + measure(&suffix, 13.0);
        }
        if mismatch || failed {
            natural = natural.max(20.0 + measure(caption, 11.0));
        }
        let width = natural
            .ceil()
            .clamp(60.0, 240.0)
            .min(ui.available_width().max(60.0));
        let height = if mismatch || failed { 46.0 } else { 28.0 };
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
        let dragging = response.dragged()
            || egui::DragAndDrop::payload::<ApplicationDrag>(ui.ctx())
                .is_some_and(|drag| drag.key == app.key);
        let alpha = if dragging { 0.4 } else { 1.0 };
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(
            rect,
            12.0,
            (if response.hovered() { RAISED } else { FIELD }).gamma_multiply(alpha),
        );
        painter.rect_stroke(
            rect,
            12.0,
            Stroke::new(
                if response.has_focus() { 2.0 } else { 1.0 },
                border.gamma_multiply(alpha),
            ),
            egui::StrokeKind::Inside,
        );
        painter.circle_filled(
            egui::pos2(rect.left() + 11.0, rect.top() + 14.0),
            3.0,
            dot.gamma_multiply(alpha),
        );
        let font = FontId::proportional(13.0);
        let count = ui
            .painter()
            .layout_no_wrap(suffix.clone(), font.clone(), TEXT);
        let available = (width - 30.0 - count.size().x).max(0.0);
        let mut short = name.to_owned();
        while ui
            .painter()
            .layout_no_wrap(short.clone(), font.clone(), TEXT)
            .size()
            .x
            > available
            && !short.is_empty()
        {
            short.pop();
            if short.ends_with("...") {
                short.truncate(short.len() - 3);
            }
            if short.is_empty() {
                break;
            }
            short.push_str("...");
            // Remove one original character on the next pass, rather than dots.
            if ui
                .painter()
                .layout_no_wrap(short.clone(), font.clone(), TEXT)
                .size()
                .x
                > available
            {
                short.truncate(short.len() - 3);
            } else {
                break;
            }
        }
        painter.text(
            egui::pos2(rect.left() + 20.0, rect.top() + 14.0),
            Align2::LEFT_CENTER,
            &short,
            font.clone(),
            TEXT.gamma_multiply(alpha),
        );
        painter.text(
            egui::pos2(rect.right() - 8.0, rect.top() + 14.0),
            Align2::RIGHT_CENTER,
            &suffix,
            font.clone(),
            TEXT.gamma_multiply(alpha),
        );
        if self.connected && response.hovered() {
            ui.ctx().set_cursor_icon(if dragging {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        if dragging && let Some(pointer) = ui.ctx().pointer_latest_pos() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            let ghost = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::unique("application-drag-ghost"),
            ));
            let ghost_rect = egui::Rect::from_min_size(
                pointer + egui::vec2(14.0, 16.0),
                egui::vec2(width, 28.0),
            );
            ghost.rect_filled(ghost_rect, 12.0, RAISED.gamma_multiply(0.8));
            ghost.rect_stroke(
                ghost_rect,
                12.0,
                Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Inside,
            );
            ghost.with_clip_rect(ghost_rect).text(
                ghost_rect.left_center() + egui::vec2(10.0, 0.0),
                Align2::LEFT_CENTER,
                format!("{short}{suffix}"),
                font,
                TEXT,
            );
        }
        if mismatch || failed {
            painter.text(
                egui::pos2(rect.left() + 10.0, rect.top() + 33.0),
                Align2::LEFT_CENTER,
                caption,
                FontId::proportional(11.0),
                border,
            );
        }
        if self.connected
            && ui.input(|i| {
                i.pointer.primary_pressed()
                    && i.pointer
                        .press_origin()
                        .or_else(|| {
                            i.events.iter().find_map(|event| {
                                if let egui::Event::PointerButton {
                                    pos,
                                    button: egui::PointerButton::Primary,
                                    pressed: true,
                                    ..
                                } = event
                                {
                                    Some(*pos)
                                } else {
                                    None
                                }
                            })
                        })
                        .is_some_and(|origin| rect.contains(origin))
            })
        {
            egui::DragAndDrop::set_payload(
                ui.ctx(),
                ApplicationDrag {
                    key: app.key.clone(),
                },
            );
        }
        if self.connected {
            response.dnd_set_drag_payload(ApplicationDrag {
                key: app.key.clone(),
            });
        }
        if response.clicked()
            || response.secondary_clicked()
            || (response.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)))
        {
            self.route_menu = Some(app.key.clone());
        }
        if response.has_focus() {
            for (key, target) in [
                (egui::Key::Num1, "gaming"),
                (egui::Key::Num2, "chat"),
                (egui::Key::Num3, "media"),
                (egui::Key::Num4, "aux"),
            ] {
                if ui.input(|i| i.key_pressed(key)) && self.connected && binding_ready(host, target)
                {
                    self.route(&app.key, target, host);
                }
            }
        }
        #[cfg(test)]
        {
            self.pill_rects.insert(app.key.clone(), rect);
        }
        let observed = app
            .endpoint_ids
            .iter()
            .map(|id| {
                host.config
                    .channels
                    .iter()
                    .find(|c| c.endpoint_id.as_ref() == Some(id))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| name_of(&host.windows, Some(id)))
            })
            .collect::<Vec<_>>()
            .join(", ");
        response.on_hover_text(format!("{}\n{} audio process(es)\nPlaying through: {}\nClick to assign. Drag to a channel. Keyboard: 1-4.",app.name,app.process_ids.len(),observed));
    }

    fn channel_strip(
        &mut self,
        ui: &mut egui::Ui,
        host: &HostState,
        channel: &crate::model::Channel,
        apps: &[GroupedApplication],
    ) {
        let ready = binding_ready(host, &channel.id);
        let width = ui.available_width();
        let selected = self.selected == channel.id;
        let response = Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(
                1.0,
                if selected {
                    channel_color(&channel.id)
                } else {
                    EDGE
                },
            ))
            .inner_margin(12)
            .corner_radius(6)
            .show(ui, |ui| {
                ui.set_width(width - 26.0);
                ui.set_min_height(320.0);
                self.strip_header(ui, &channel.id, &channel.name, &channel.eq);
                ui.add_space(8.0);
                if !ready {
                    ui.colored_label(WARN, "Not set up");
                    if ui.button("Set up channels").clicked() {
                        self.settings = true;
                        self.settings_tab = 2;
                    }
                } else if let Some(endpoint) = host
                    .windows
                    .outputs
                    .iter()
                    .find(|e| channel.endpoint_id.as_ref() == Some(&e.id))
                {
                    ui.horizontal_top(|ui| {
                        self.endpoint_fader(ui, endpoint, true);
                        let width = ui.available_width();
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 236.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(width);
                                egui::ScrollArea::vertical()
                                    .id_salt(("apps", &channel.id))
                                    .max_height(236.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| self.strip_apps(ui, apps, channel, host));
                            },
                        );
                    });
                }
            })
            .response;
        self.select_strip_at_click(ui, response.rect, &channel.id);
        ui.ctx().data_mut(|data| {
            let key = egui::Id::unique("application-drop-rects");
            let mut targets = data
                .get_temp::<Vec<(egui::Rect, String)>>(key)
                .unwrap_or_default();
            targets.push((response.rect, channel.id.clone()));
            data.insert_temp(key, targets);
        });
        #[cfg(test)]
        {
            self.strip_rects.insert(channel.id.clone(), response.rect);
        }
        if ready
            && self.connected
            && ui.input(|i| {
                i.pointer.primary_released()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|point| response.rect.contains(point))
            })
            && let Some(payload) = egui::DragAndDrop::take_payload::<ApplicationDrag>(ui.ctx())
        {
            self.route(&payload.key, &channel.id, host);
        }
        if ui.input(|i| {
            i.pointer
                .hover_pos()
                .is_some_and(|point| response.rect.contains(point))
        }) && egui::DragAndDrop::has_payload_of_type::<ApplicationDrag>(ui.ctx())
        {
            let caption = if ready {
                format!("Move to {}", channel.name)
            } else {
                "Channel not set up".into()
            };
            ui.painter().text(
                response.rect.right_top() + egui::vec2(-8.0, 8.0),
                Align2::RIGHT_TOP,
                caption,
                FontId::proportional(11.0),
                if ready { ACCENT } else { WARN },
            );
            ui.painter().rect_stroke(
                response.rect,
                6.0,
                Stroke::new(2.0, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn strip_apps(
        &mut self,
        ui: &mut egui::Ui,
        apps: &[GroupedApplication],
        channel: &crate::model::Channel,
        host: &HostState,
    ) {
        let placed: Vec<_> = apps
            .iter()
            .filter(|app| {
                self.routes
                    .get(&app.key)
                    .map(|r| r.target.as_str())
                    .or(app.channel_id.as_deref())
                    == Some(channel.id.as_str())
            })
            .collect();
        if placed.is_empty() {
            ui.label(RichText::new("Drop apps here").color(MUTED).size(12.0));
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for app in placed {
                self.app_pill(ui, app, host);
            }
        });
    }

    /// Below the channel strips: output and unassigned apps share one row on wide
    /// windows and stack on narrow ones.
    fn output_and_apps_row(
        &mut self,
        ui: &mut egui::Ui,
        host: &HostState,
        other: &[GroupedApplication],
    ) {
        let gap = 8.0;
        let width = ui.available_width();
        if width >= 860.0 {
            let half = ((width - gap) / 2.0).floor();
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let output = ui
                    .allocate_ui_with_layout(
                        egui::vec2(half, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(half);
                            self.output_panel(ui, host)
                        },
                    )
                    .inner;
                let rest = width - half - gap;
                ui.allocate_ui_with_layout(
                    egui::vec2(rest, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(rest);
                        self.unassigned_panel(ui, host, other, Some(output.height()));
                    },
                );
            });
        } else {
            self.output_panel(ui, host);
            ui.add_space(gap);
            self.unassigned_panel(ui, host, other, None);
        }
    }

    fn output_panel(&mut self, ui: &mut egui::Ui, host: &HostState) -> egui::Rect {
        let width = ui.available_width();
        let eq = host
            .selected_output_id
            .as_ref()
            .and_then(|id| host.config.output_eq.get(id))
            .cloned()
            .unwrap_or_default();
        let response = Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(
                1.0,
                if self.selected == "output" {
                    ACCENT
                } else {
                    EDGE
                },
            ))
            .inner_margin(12)
            .corner_radius(6)
            .show(ui, |ui| {
                ui.set_width(width - 26.0);
                self.strip_header(ui, "output", "Output", &eq);
                ui.horizontal(|ui| {
                    ui.label("Device");
                    let priority_width = 76.0;
                    let combo_width =
                        (ui.available_width() - priority_width - 2.0 * ui.spacing().item_spacing.x)
                            .max(120.0);
                    // ComboBox width is only a minimum; the slot caps it so long names truncate.
                    let combo = ui.allocate_ui_with_layout(
                        egui::vec2(combo_width, ui.spacing().interact_size.y),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                    ui.set_max_width(combo_width);
                    egui::ComboBox::from_id_salt("listening-output")
                        .selected_text(name_of(&host.windows, host.selected_output_id.as_deref()))
                        .width(combo_width)
                        .truncate()
                        .show_ui(ui, |ui| {
                            for endpoint in host.windows.outputs.iter().filter(|e| !e.is_virtual) {
                                if ui
                                    .add_enabled(
                                        self.connected,
                                        egui::Button::new(&endpoint.name).selected(
                                            host.selected_output_id.as_ref() == Some(&endpoint.id),
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.submit(
                                        Command::SelectOutput {
                                            id: endpoint.id.clone(),
                                        },
                                        Tag::Other,
                                    );
                                }
                            }
                        })
                        .response
                        .on_hover_text(name_of(
                            &host.windows,
                            host.selected_output_id.as_deref(),
                        ))
                        },
                    )
                    .inner;
                    #[cfg(test)]
                    {
                        self.header_rects.insert("output-device".into(), combo.rect);
                    }
                    #[cfg(not(test))]
                    let _ = combo;
                    if ui
                        .add_sized(
                            [priority_width, ui.spacing().interact_size.y],
                            egui::Button::new("Priority"),
                        )
                        .on_hover_text("Choose the order Anada uses to pick an output automatically")
                        .clicked()
                    {
                        self.settings = true;
                        self.settings_tab = 1;
                    }
                });
                let note = |text: &str| RichText::new(text).small().color(MUTED);
                ui.label(note(if host.manual_output {
                    "Chosen manually. Your priorities apply again when a device connects or disconnects."
                } else {
                    "Chosen automatically from your output priorities."
                }));
                ui.label(note(
                    "EQ is saved separately for each output device and follows the device you choose.",
                ));
                ui.add_space(4.0);
                if let Some(endpoint) = host
                    .windows
                    .outputs
                    .iter()
                    .find(|e| host.selected_output_id.as_ref() == Some(&e.id))
                {
                    self.endpoint_fader(ui, endpoint, false);
                } else {
                    ui.colored_label(WARN, "No listening output");
                }
            })
            .response;
        self.select_strip_at_click(ui, response.rect, "output");
        #[cfg(test)]
        {
            self.strip_rects.insert("output".into(), response.rect);
        }
        response.rect
    }

    fn unassigned_panel(
        &mut self,
        ui: &mut egui::Ui,
        host: &HostState,
        other: &[GroupedApplication],
        match_height: Option<f32>,
    ) {
        let width = ui.available_width();
        let top = ui.cursor().min.y;
        let response = Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, EDGE))
            .inner_margin(12)
            .corner_radius(6)
            .show(ui, |ui| {
                ui.set_width(width - 26.0);
                if let Some(height) = match_height {
                    ui.set_min_height(height - 26.0);
                }
                ui.label(RichText::new("NOT IN ANADA").size(11.0).color(MUTED));
                ui.label(
                    RichText::new(
                        "These apps play straight to Windows. Drag one onto a channel, or click it to choose.",
                    )
                    .small()
                    .color(MUTED),
                );
                if other.is_empty() {
                    ui.label(RichText::new("No other apps are playing.").color(MUTED));
                    return;
                }
                let max_height = match match_height {
                    Some(height) => (top + height - 13.0 - ui.cursor().min.y).max(40.0),
                    None => 160.0,
                };
                egui::ScrollArea::vertical()
                    .id_salt("not-in-anada")
                    .max_height(max_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            for app in other {
                                self.app_pill(ui, app, host);
                            }
                        });
                    });
            })
            .response;
        #[cfg(test)]
        {
            self.strip_rects
                .insert("not-in-anada".into(), response.rect);
        }
        #[cfg(not(test))]
        let _ = response;
    }

    fn strip_header(&mut self, ui: &mut egui::Ui, id: &str, name: &str, eq: &EqSettings) {
        let status = if eq.bypass {
            "EQ off"
        } else if eq.preamp_db != 0.0
            || eq
                .bands
                .iter()
                .any(|band| band.enabled && band.gain_db != 0.0)
        {
            "EQ *"
        } else {
            "EQ"
        };
        let response = ui
            .add_sized(
                egui::vec2(ui.available_width(), 28.0),
                egui::Button::new(RichText::new(format!("{name} · {status}")).size(14.0))
                    .truncate()
                    .selected(self.selected == id),
            )
            .on_hover_text(format!("Edit {name} equalizer below"));
        #[cfg(test)]
        {
            self.header_rects.insert(id.into(), response.rect);
        }
        if response.clicked() {
            self.selected = id.into();
        }
    }

    fn select_strip_at_click(&mut self, ui: &egui::Ui, rect: egui::Rect, id: &str) {
        if ui.rect_contains_pointer(rect)
            && ui.input(|input| {
                input.pointer.primary_clicked()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|pos| rect.contains(pos) && ui.clip_rect().contains(pos))
            })
        {
            self.selected = id.into();
        }
    }

    fn endpoint_fader(&mut self, ui: &mut egui::Ui, endpoint: &Endpoint, vertical: bool) {
        let db = endpoint.level_db.as_ref();
        // Windows' scalar API supplies the device's audio-tapered fader curve.
        let (current, min, max, step) = (
            endpoint.volume * 100.0,
            0.0,
            100.0,
            if ui.input(|i| i.modifiers.shift) {
                0.1
            } else {
                1.0
            },
        );
        let edit = self
            .volumes
            .entry(endpoint.id.clone())
            .or_insert_with(|| VolumeEdit {
                value: current,
                db: false,
                request: None,
                sent: None,
                last_sent: Instant::now() - Duration::from_secs(1),
                dirty: false,
            });
        if !edit.dirty && edit.request.is_none() {
            edit.value = current;
            edit.db = false;
        }
        let value = if edit.db { current } else { edit.value };
        let level = RichText::new(
            db.map(|d| format!("{:+.1} dB", d.current))
                .unwrap_or_else(|| "Windows level".into()),
        )
        .size(11.0)
        .color(if db.is_some_and(|level| level.current > 0.1) {
            BAD
        } else {
            MUTED
        });
        ui.vertical(|ui| {
            if vertical {
                ui.set_width(100.0);
                self.percentage_control(ui, endpoint);
                ui.label(level);
            } else {
                ui.horizontal(|ui| {
                    self.percentage_control(ui, endpoint);
                    ui.label(level);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Right-to-left: add Quiet first so the order reads Mute, Quiet.
                        self.quiet_button(ui, endpoint);
                        self.mute_button(ui, endpoint);
                    });
                });
            }
            let size = if vertical {
                egui::vec2(88.0, 160.0)
            } else {
                egui::vec2(ui.available_width(), 40.0)
            };
            let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
            #[cfg(test)] { self.fader_rects.insert(endpoint.id.clone(),rect); }
            // Horizontal: the track runs above the meter lanes and leaves room
            // on the right for the dBFS readout.
            let start = if vertical {
                egui::pos2(rect.center().x, rect.bottom() - 24.0)
            } else {
                egui::pos2(rect.left() + 8.0, rect.top() + 13.0)
            };
            let end = if vertical {
                egui::pos2(rect.center().x, rect.top() + 8.0)
            } else {
                egui::pos2(rect.right() - 72.0, rect.top() + 13.0)
            };
            ui.painter()
                .line_segment([start, end], Stroke::new(5.0, FIELD));
            for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let center = start + (end - start) * fraction;
                let tick = if vertical {
                    [center + egui::vec2(8.0, 0.0), center + egui::vec2(12.0, 0.0)]
                } else {
                    [center - egui::vec2(0.0, 12.0), center - egui::vec2(0.0, 8.0)]
                };
                ui.painter().line_segment(tick, Stroke::new(1.0, EDGE));
            }
            self.paint_signal_meter(ui, endpoint, rect, vertical);
            let fraction = ((value - min) / (max - min)).clamp(0.0, 1.0);
            let point = start + (end - start) * fraction;
            ui.painter()
                .line_segment([start, point], Stroke::new(4.0, ACCENT));
            let thumb = egui::Rect::from_center_size(
                point,
                if vertical {
                    egui::vec2(27.0, 13.0)
                } else {
                    egui::vec2(13.0, 25.0)
                },
            );
            ui.painter().rect_filled(thumb, 3.0, TEXT);
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    Stroke::new(1.5, ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
            let mut next = None;
            if self.fader_drag.as_deref() == Some(endpoint.id.as_str()) && ui.input(|i| !i.pointer.primary_down() && !i.pointer.primary_released()) {
                self.fader_drag = None;
            }
            let press = ui.input(|i| i.events.iter().find_map(|event| match event {
                egui::Event::PointerButton { pos,button:egui::PointerButton::Primary,pressed:true,.. } => Some(*pos), _=>None,
            }));
            if self.connected && ui.is_enabled() && press.is_some_and(|pos| rect.contains(pos) && ui.clip_rect().contains(pos)) {
                self.fader_drag = Some(endpoint.id.clone());
                response.request_focus();
            }
            if self.connected
                && (self.fader_drag.as_deref() == Some(endpoint.id.as_str()) || response.dragged() || response.clicked())
                && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            {
                let t = if vertical {
                    (start.y - pos.y) / (start.y - end.y)
                } else {
                    (pos.x - start.x) / (end.x - start.x)
                };
                next = Some(min + t.clamp(0.0, 1.0) * (max - min));
                response.request_focus();
            }
            if self.fader_drag.as_deref() == Some(endpoint.id.as_str()) && ui.input(|i|i.pointer.primary_released()) {
                self.fader_drag = None;
            }
            if self.connected && response.has_focus() {
                ui.input(|i| {
                    let mut delta = 0.0;
                    if i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::ArrowRight) {
                        delta += step;
                    }
                    if i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::ArrowLeft) {
                        delta -= step;
                    }
                    if i.key_pressed(egui::Key::PageUp) {
                        delta += step * 10.0;
                    }
                    if i.key_pressed(egui::Key::PageDown) {
                        delta -= step * 10.0;
                    }
                    if delta != 0.0 {
                        next = Some(value + delta);
                    }
                    if i.key_pressed(egui::Key::Home) {
                        next = Some(min);
                    }
                    if i.key_pressed(egui::Key::End) {
                        next = Some(max);
                    }
                });
            }
            if self.connected
                && response.hovered()
                && (response.has_focus() || ui.input(|i| i.modifiers.ctrl))
            {
                let wheel = ui.input_mut(|i| {
                    let mut wheel = 0.0;
                    i.events.retain(|event| {
                        if let egui::Event::MouseWheel { delta, .. } = event {
                            wheel += delta.y;
                            false
                        } else {
                            true
                        }
                    });
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                    wheel
                });
                if wheel.abs() > 0.0 {
                    next = Some(
                        value
                            + wheel.signum()
                                * if db.is_some() {
                                    if ui.input(|i| i.modifiers.shift) {
                                        0.5
                                    } else {
                                        1.0
                                    }
                                } else {
                                    1.0
                                },
                    );
                    ui.ctx().request_repaint();
                }
            }
            if let Some(next) = next {
                let edit = self.volumes.get_mut(&endpoint.id).unwrap();
                let rounded = ((next - min) / step)
                    .round()
                    .mul_add(step, min)
                    .clamp(min, max);
                if edit.db || rounded != edit.value {
                    edit.value = rounded;
                    edit.db = false;
                    edit.dirty = true;
                }
            }
            if vertical {
                ui.horizontal(|ui| self.endpoint_buttons(ui, endpoint));
            }
            response.on_hover_text("Windows volume uses its audio-tapered curve. Shift: fine adjustment. The stereo bars show digital signal with a fast rise and a slower fall. dBFS is measured before the output device's volume and mute. A red mark holds clipping for 0.75 seconds.");
        });
    }

    fn endpoint_buttons(&mut self, ui: &mut egui::Ui, endpoint: &Endpoint) {
        self.mute_button(ui, endpoint);
        self.quiet_button(ui, endpoint);
    }

    fn mute_button(&mut self, ui: &mut egui::Ui, endpoint: &Endpoint) {
        let muted = self
            .mutes
            .get(&endpoint.id)
            .map(|p| p.0)
            .unwrap_or(endpoint.muted);
        if ui
            .add_enabled(
                self.connected,
                egui::Button::new(if muted { "Unmute" } else { "Mute" }).selected(muted),
            )
            .clicked()
            && let Some(id) = self.submit(
                Command::SetMute {
                    endpoint_id: endpoint.id.clone(),
                    muted: !muted,
                },
                Tag::Other,
            )
        {
            self.mutes.insert(endpoint.id.clone(), (!muted, id));
        }
    }

    fn quiet_button(&mut self, ui: &mut egui::Ui, endpoint: &Endpoint) {
        let host = self.host.as_ref();
        let level = host.map_or(20.0, |host| host.config.quiet_volume_percent);
        let on = self.quiets.get(&endpoint.id).map_or_else(
            || host.is_some_and(|host| host.quiet.contains_key(&endpoint.id)),
            |pending| pending.0,
        );
        let hover = if on {
            "Quiet is on. Click to return to the volume from before Quiet. Moving the fader also ends Quiet.".to_owned()
        } else {
            format!(
                "Lower to the quiet level ({level:.0}%) until you click again. Never raises the volume. Change the level in Settings → Channels."
            )
        };
        let response = ui
            .add_enabled(self.connected, egui::Button::new("Quiet").selected(on))
            .on_hover_text(hover);
        #[cfg(test)]
        {
            self.quiet_rects.insert(endpoint.id.clone(), response.rect);
        }
        if response.clicked()
            && let Some(id) = self.submit(
                Command::SetQuiet {
                    endpoint_id: endpoint.id.clone(),
                    on: !on,
                },
                Tag::Other,
            )
        {
            self.quiets.insert(endpoint.id.clone(), (!on, id));
        }
    }

    fn percentage_control(&mut self, ui: &mut egui::Ui, endpoint: &Endpoint) {
        let current = self
            .volumes
            .get(&endpoint.id)
            .filter(|edit| !edit.db)
            .map(|edit| edit.value)
            .unwrap_or(endpoint.volume * 100.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (next, response) =
                crate::ui_numeric::percentage_input(ui, &endpoint.id, current, self.connected);
            #[cfg(test)]
            {
                self.percentage_rects
                    .insert(endpoint.id.clone(), response.rect);
            }
            #[cfg(not(test))]
            let _ = response;
            ui.label(RichText::new("%").color(MUTED));
            if let Some(value) = next {
                let edit = self
                    .volumes
                    .entry(endpoint.id.clone())
                    .or_insert_with(|| VolumeEdit {
                        value: current,
                        db: false,
                        request: None,
                        sent: None,
                        last_sent: Instant::now() - Duration::from_secs(1),
                        dirty: false,
                    });
                edit.value = value;
                edit.db = false;
                edit.dirty = true;
            }
        });
    }

    pub(super) fn application_menu(&mut self, ctx: &egui::Context, host: &HostState) {
        let Some(key) = self.route_menu.clone() else {
            return;
        };
        let apps = group_applications(host);
        let Some(app) = apps.iter().find(|app| app.key == key) else {
            self.route_menu = None;
            return;
        };
        let mut open = true;
        egui::Window::new(format!(
            "Move {}",
            app.name.strip_suffix(".exe").unwrap_or(&app.name)
        ))
        .id(egui::Id::unique("application-route-menu"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            if let Some(error) = self.routes.get(&key).and_then(|r| r.error.as_ref()) {
                ui.colored_label(BAD, error);
            }
            ui.label(
                RichText::new("Set the Windows playback preference for every audio process.")
                    .color(MUTED),
            );
            for channel in &host.config.channels {
                if ui
                    .add_enabled(
                        self.connected && binding_ready(host, &channel.id),
                        egui::Button::new(&channel.name),
                    )
                    .clicked()
                {
                    self.route(&key, &channel.id, host);
                    self.route_menu = None;
                }
            }
            ui.label(
                RichText::new("Existing audio may need the app to restart playback.")
                    .small()
                    .color(MUTED),
            );
        });
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.route_menu = None;
        }
    }
}
