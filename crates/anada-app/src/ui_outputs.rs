use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct OutputRow {
    id: String,
    name: String,
    priority: Option<usize>,
    connected: bool,
    unidentified: bool,
}

fn output_rows(config: &Config, windows: &WindowsSnapshot) -> Vec<OutputRow> {
    let mut rows: Vec<_> = windows
        .outputs
        .iter()
        .filter(|endpoint| !endpoint.is_virtual)
        .map(|endpoint| OutputRow {
            id: endpoint.id.clone(),
            name: endpoint.name.clone(),
            priority: config
                .output_priorities
                .iter()
                .position(|id| *id == endpoint.id),
            connected: true,
            unidentified: false,
        })
        .collect();
    for id in config
        .output_priorities
        .iter()
        .chain(&config.known_output_ids)
    {
        if rows.iter().any(|row| row.id == *id) {
            continue;
        }
        // A virtual device must never become a physical listening target.
        if windows
            .outputs
            .iter()
            .any(|endpoint| endpoint.id == *id && endpoint.is_virtual)
        {
            continue;
        }
        let remembered = config
            .output_names
            .get(id)
            .filter(|name| !name.trim().is_empty());
        rows.push(OutputRow {
            id: id.clone(),
            name: remembered
                .cloned()
                .unwrap_or_else(|| "Unidentified saved output".into()),
            priority: config
                .output_priorities
                .iter()
                .position(|saved| saved == id),
            connected: false,
            unidentified: remembered.is_none(),
        });
    }
    rows.sort_by(|a, b| {
        (
            a.priority.unwrap_or(usize::MAX),
            a.name.to_lowercase(),
            &a.id,
        )
            .cmp(&(
                b.priority.unwrap_or(usize::MAX),
                b.name.to_lowercase(),
                &b.id,
            ))
    });
    rows
}

#[derive(Clone)]
struct OutputPriorityDrag {
    id: String,
    origin: egui::Pos2,
}

// Move by stable endpoint identity. Every other saved entry keeps its relative order.
fn insert_priority(priorities: &mut Vec<String>, source: &str, target: &str, after: bool) {
    if source == target {
        return;
    }
    let Some(from) = priorities.iter().position(|id| id == source) else {
        return;
    };
    if !priorities.iter().any(|id| id == target) {
        return;
    }
    let id = priorities.remove(from);
    let to = priorities.iter().position(|id| id == target).unwrap() + usize::from(after);
    priorities.insert(to, id);
}

fn priority_shortcut(ui: &egui::Ui, expected: egui::Key) -> bool {
    // A complete native chord can arrive in one frame. Inspect the key event's
    // modifiers, since the final frame state may already have Alt released.
    ui.input(|i| {
        i.events.iter().any(|event| {
            matches!(event,
        egui::Event::Key {key, pressed:true, modifiers, ..} if *key==expected && modifiers.alt)
        })
    })
}

enum PriorityEdit {
    Insert(String, String, bool),
    Remove(String),
    Add(String),
    ClearUnidentified,
}

impl AnadaApp {
    pub(super) fn output_settings(
        &mut self,
        ui: &mut egui::Ui,
        host: &HostState,
        draft: &mut Config,
    ) {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.heading("Output priority");
        ui.label(RichText::new("Drag a device to change priority. The first connected device is used automatically.").color(MUTED));
        ui.add_space(10.0);
        let rows = output_rows(draft, &host.windows);
        let automatic = rows
            .iter()
            .find(|row| row.connected && row.priority.is_some())
            .map(|row| row.id.as_str());
        // Register from previous-frame hit regions before visiting rows, so an upward
        // drop also works when press, motion and release arrive in one input batch.
        let hit_id = egui::Id::unique("output-priority-hit-regions");
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            egui::DragAndDrop::clear_payload(ui.ctx());
        } else if let Some(origin) = ui.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => Some(*pos),
                _ => None,
            })
        }) {
            let hits = ui
                .ctx()
                .data(|data| data.get_temp::<Vec<(String, egui::Rect)>>(hit_id))
                .unwrap_or_default();
            if let Some((id, _)) = hits.iter().find(|(_, rect)| rect.contains(origin)) {
                egui::DragAndDrop::set_payload(
                    ui.ctx(),
                    OutputPriorityDrag {
                        id: id.clone(),
                        origin,
                    },
                );
            }
        }
        let mut hits = Vec::new();
        let mut edit = None;
        if host.manual_output {
            ui.label(RichText::new("Manual output in use").small().color(ACCENT))
                .on_hover_text(
                    "Automatic selection resumes when a device connects or disconnects.",
                );
        }
        if !rows.iter().any(|row| row.priority.is_some()) {
            ui.label("No priority outputs. Expand Not in priority list to add a device.");
        }
        for row in rows.iter().filter(|row| row.priority.is_some()) {
            ui.push_id(&row.id, |ui| {
                self.output_row(
                    ui,
                    row,
                    host,
                    &draft.output_priorities,
                    automatic,
                    &mut edit,
                    &mut hits,
                )
            });
        }
        if rows
            .iter()
            .any(|row| row.priority.is_some() && !row.connected && row.unidentified)
            && ui.button("Remove unidentified offline outputs").clicked()
        {
            edit = Some(PriorityEdit::ClearUnidentified);
        }
        let unlisted: Vec<_> = rows.iter().filter(|row| row.priority.is_none()).collect();
        if !unlisted.is_empty() {
            ui.add_space(10.0);
            let section =
                egui::CollapsingHeader::new(format!("Not in priority list ({})", unlisted.len()))
                    .id_salt("unlisted-listening-outputs")
                    .default_open(false)
                    .show(ui, |ui| {
                        #[cfg(test)]
                        {
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(egui::Id::unique("unlisted-outputs-visible"), true)
                            });
                        }
                        for row in &unlisted {
                            ui.push_id(&row.id, |ui| {
                                self.output_row(
                                    ui,
                                    row,
                                    host,
                                    &draft.output_priorities,
                                    automatic,
                                    &mut edit,
                                    &mut hits,
                                )
                            });
                        }
                    });
            #[cfg(test)]
            {
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        egui::Id::unique("unlisted-output-header"),
                        section.header_response.rect,
                    )
                });
            }
            let _ = section;
        }
        ui.ctx().data_mut(|data| data.insert_temp(hit_id, hits));
        if let Some(payload) = egui::DragAndDrop::payload::<OutputPriorityDrag>(ui.ctx()) {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos())
                && pos.distance(payload.origin) > 4.0
                && ui.input(|i| i.pointer.primary_down())
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                let name = rows
                    .iter()
                    .find(|row| row.id == payload.id)
                    .map(|row| row.name.as_str())
                    .unwrap_or("Output");
                egui::Area::new(egui::Id::unique("output-priority-drag-preview"))
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .fixed_pos(pos + egui::vec2(16.0, 16.0))
                    .show(ui.ctx(), |ui| {
                        Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(name);
                        });
                    });
                let clip = ui.clip_rect();
                let delta = if pos.y < clip.top() + 28.0 {
                    6.0
                } else if pos.y > clip.bottom() - 28.0 {
                    -6.0
                } else {
                    0.0
                };
                if delta != 0.0 {
                    ui.scroll_with_delta(egui::vec2(0.0, delta));
                }
                ui.ctx().request_repaint();
            }
            if ui.input(|i| i.pointer.any_released()) {
                egui::DragAndDrop::clear_payload(ui.ctx());
            }
        }
        match edit {
            Some(PriorityEdit::Insert(source, target, after)) => {
                insert_priority(&mut draft.output_priorities, &source, &target, after)
            }
            Some(PriorityEdit::Remove(id)) => draft.output_priorities.retain(|saved| saved != &id),
            Some(PriorityEdit::Add(id)) if draft.output_priorities.len() < 64 => {
                draft.output_priorities.push(id)
            }
            Some(PriorityEdit::ClearUnidentified) => draft.output_priorities.retain(|id| {
                !rows
                    .iter()
                    .any(|row| row.id == *id && !row.connected && row.unidentified)
            }),
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn output_row(
        &mut self,
        ui: &mut egui::Ui,
        row: &OutputRow,
        host: &HostState,
        priorities: &[String],
        automatic: Option<&str>,
        edit: &mut Option<PriorityEdit>,
        hits: &mut Vec<(String, egui::Rect)>,
    ) {
        let count = priorities.len();
        let visible: Vec<_> = priorities
            .iter()
            .filter(|id| {
                !host
                    .windows
                    .outputs
                    .iter()
                    .any(|endpoint| endpoint.id == **id && endpoint.is_virtual)
            })
            .collect();
        let position = visible.iter().position(|id| **id == row.id);
        let previous = position.and_then(|i| i.checked_sub(1)).map(|i| visible[i]);
        let next = position.and_then(|i| visible.get(i + 1).copied());
        let frame = Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(8,4)).corner_radius(4).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.spacing_mut().button_padding = egui::vec2(8.0,3.0);
                let handle = ui.add_enabled(row.priority.is_some(), egui::Button::new(RichText::new("≡").size(16.0)).min_size(egui::vec2(28.0,28.0)).sense(egui::Sense::click_and_drag())).on_hover_text("Drag to reorder. Focus here and press Alt+Up or Alt+Down; right-click for more actions.");
                if handle.clicked() {handle.request_focus();}
                if row.priority.is_some() {
                    if handle.has_focus() {
                        if let Some(previous) = previous && priority_shortcut(ui, egui::Key::ArrowUp) {*edit = Some(PriorityEdit::Insert(row.id.clone(), previous.clone(), false));}
                        if let Some(next) = next && priority_shortcut(ui, egui::Key::ArrowDown) {*edit = Some(PriorityEdit::Insert(row.id.clone(), next.clone(), true));}
                    }
                    handle.context_menu(|ui| {
                        if ui.add_enabled(previous.is_some(), egui::Button::new("Move up")).clicked() {*edit=Some(PriorityEdit::Insert(row.id.clone(), previous.unwrap().clone(), false)); ui.close();}
                        if ui.add_enabled(next.is_some(), egui::Button::new("Move down")).clicked() {*edit=Some(PriorityEdit::Insert(row.id.clone(), next.unwrap().clone(), true)); ui.close();}
                    });
                }
                ui.add_sized([25.0,28.0], egui::Label::new(RichText::new(row.priority.map(|n| (n+1).to_string()).unwrap_or_else(|| "—".into())).color(MUTED))).on_hover_text("Position in the saved list, including offline devices");
                let wide = ui.available_width() > 460.0;
                let name_width = (ui.available_width()-if wide {244.0} else {138.0}).max(80.0);
                let name = ui.allocate_ui_with_layout(egui::vec2(name_width,28.0),egui::Layout::left_to_right(egui::Align::Center),|ui| {ui.set_min_width(name_width); ui.add(egui::Label::new(RichText::new(&row.name).color(if row.connected {TEXT} else {MUTED})).truncate().sense(egui::Sense::click_and_drag()))}).inner.on_hover_text(&row.name);
                if row.priority.is_some() {
                    let rect = handle.rect.union(name.rect).intersect(ui.clip_rect());
                    hits.push((row.id.clone(),rect));
                    handle.clone().on_hover_cursor(egui::CursorIcon::Grab);
                    name.on_hover_cursor(egui::CursorIcon::Grab);
                    #[cfg(test)] { ui.ctx().data_mut(|data| {data.insert_temp(egui::Id::unique(("output-row",&row.id)),rect); data.insert_temp(egui::Id::unique(("output-handle",&row.id)),handle.rect);}); }
                }
                let selected = host.selected_output_id.as_ref() == Some(&row.id);
                let status = if !row.connected {"Offline"} else if selected && host.audio_running {"Playing"} else if selected {"Selected"} else if automatic==Some(row.id.as_str()) {"Automatic"} else {""};
                if wide {ui.add_sized([98.0,28.0], egui::Label::new(RichText::new(status).small().color(if selected {GOOD} else {MUTED})));}
                #[cfg(test)] {ui.ctx().data_mut(|data|data.insert_temp(egui::Id::unique(("output-actions",&row.id)),ui.next_widget_position()));}
                if row.connected && ui.add_enabled(self.connected && !selected,egui::Button::new("Use now").min_size(egui::vec2(80.0,28.0))).clicked() {self.submit(Command::SelectOutput {id:row.id.clone()},Tag::Other);}
                if !row.connected {ui.allocate_space(egui::vec2(80.0,28.0));}
                let membership=if row.priority.is_some() {
                    ui.add_sized([38.0,28.0],egui::Button::new("×")).on_hover_text("Remove from priority list")
                } else {
                    ui.add_enabled(count<64,egui::Button::new("Add").min_size(egui::vec2(38.0,28.0)))
                };
                #[cfg(test)] {ui.ctx().data_mut(|data|data.insert_temp(egui::Id::unique(("output-membership",&row.id)),membership.rect));}
                if membership.clicked() {*edit=Some(if row.priority.is_some() {PriorityEdit::Remove(row.id.clone())} else {PriorityEdit::Add(row.id.clone())});}
            });
        });
        if row.priority.is_some()
            && let Some(payload) = egui::DragAndDrop::payload::<OutputPriorityDrag>(ui.ctx())
            && payload.id != row.id
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            && frame.response.rect.intersect(ui.clip_rect()).contains(pos)
            && pos.distance(payload.origin) > 4.0
        {
            let after = pos.y >= frame.response.rect.center().y;
            let y = if after {
                frame.response.rect.bottom()
            } else {
                frame.response.rect.top()
            };
            ui.painter()
                .hline(frame.response.rect.x_range(), y, Stroke::new(2.0, ACCENT));
            if ui.input(|i| i.pointer.primary_released()) {
                *edit = Some(PriorityEdit::Insert(
                    payload.id.clone(),
                    row.id.clone(),
                    after,
                ));
            }
        }
        ui.add_space(2.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insertion_keeps_every_other_saved_identity_in_order_and_ignores_stale_drops() {
        let mut ids: Vec<String> = ["offline-a", "a", "demo-virtual", "offline-b", "b", "c"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let original = ids.clone();
        insert_priority(&mut ids, "c", "a", false);
        assert_eq!(
            ids,
            ["offline-a", "c", "a", "demo-virtual", "offline-b", "b"]
        );
        insert_priority(&mut ids, "c", "b", true);
        assert_eq!(ids, original);
        insert_priority(&mut ids, "a", "a", true);
        insert_priority(&mut ids, "missing", "a", false);
        insert_priority(&mut ids, "a", "missing", false);
        assert_eq!(ids, original);
    }
    #[test]
    fn saved_priority_order_includes_offline_names_and_excludes_virtual_outputs() {
        let mut host = demo::state();
        host.config.output_priorities = vec![
            "offline-known".into(),
            "demo-headphones".into(),
            "offline-unknown".into(),
            "demo-speakers".into(),
        ];
        host.config
            .output_names
            .insert("offline-known".into(), "Studio DAC".into());
        let rows = output_rows(&host.config, &host.windows);
        assert_eq!(
            rows.iter()
                .filter(|row| row.connected)
                .map(|row| (&*row.id, row.priority))
                .collect::<Vec<_>>(),
            [("demo-headphones", Some(1)), ("demo-speakers", Some(3))]
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.priority.is_some())
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            [
                "offline-known",
                "demo-headphones",
                "offline-unknown",
                "demo-speakers"
            ]
        );
        let remembered = rows.iter().find(|row| row.id == "offline-known").unwrap();
        assert_eq!(remembered.name, "Studio DAC");
        assert_eq!(remembered.priority, Some(0));
        assert!(!remembered.unidentified);
        assert!(
            rows.iter()
                .find(|row| row.id == "offline-unknown")
                .unwrap()
                .unidentified
        );
        assert!(!rows.iter().any(|row| row.id == "demo-gaming"));
        host.config
            .output_priorities
            .retain(|id| id != "demo-headphones");
        assert_eq!(
            output_rows(&host.config, &host.windows)
                .iter()
                .find(|row| row.id == "demo-headphones")
                .unwrap()
                .priority,
            None
        );
    }
}
