//! Numeric controls with explicit, discoverable resets and input-only changes.
use eframe::egui::{self, Response, Ui};
use std::ops::RangeInclusive;

#[derive(Clone, Default)]
struct PercentageDraft {
    text: String,
    editing: bool,
    edited: bool,
}

/// Keep incomplete text local. Only Enter or leaving the field writes a volume.
pub(crate) fn percentage_input(
    ui: &mut Ui,
    endpoint_id: &str,
    current: f32,
    enabled: bool,
) -> (Option<f32>, Response) {
    let id = egui::Id::unique(("volume-percentage", endpoint_id));
    let mut draft = ui
        .ctx()
        .data_mut(|data| data.get_temp::<PercentageDraft>(id).unwrap_or_default());
    if !draft.editing {
        draft.text = format!("{current:.0}");
    }
    let response = ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(&mut draft.text)
            .id(id)
            .font(egui::TextStyle::Monospace)
            .align(egui::Align2::CENTER_CENTER)
            .desired_width(72.0)
            .min_size(egui::vec2(72.0, 28.0))
            .char_limit(12),
    );
    if response.gained_focus() {
        draft.editing = true;
        draft.edited = false;
        draft.text = format!("{current:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned();
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(draft.text.chars().count()),
                )));
            state.store(ui.ctx(), id);
        }
    }
    draft.edited |= response.changed();
    let escape = (response.has_focus() || response.lost_focus())
        && ui.input(|i| i.key_pressed(egui::Key::Escape));
    let enter = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut committed = None;
    if draft.editing && (escape || enter || response.lost_focus() || !enabled) {
        if !escape && enabled && draft.edited {
            committed = draft
                .text
                .trim()
                .trim_end_matches('%')
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite())
                .map(|value| value.clamp(0.0, 100.0))
                .filter(|value| (*value - current).abs() > 0.0001);
        }
        draft.editing = false;
        response.surrender_focus();
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, draft));
    (
        committed,
        response.on_hover_text(
            "Windows volume, 0–100%. Click to type. Enter or Tab applies; Esc cancels.",
        ),
    )
}

pub(crate) fn resettable_numeric(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    speed: f64,
    default: f32,
) -> bool {
    numeric_control(ui, label, value, range, speed, default).0
}

fn numeric_control(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    speed: f64,
    default: f32,
) -> (bool, egui::Id) {
    ui.label(label);
    let before = *value;
    let response = ui.add(
        egui::DragValue::new(value)
            .range(range.clone())
            .speed(speed)
            .max_decimals(2),
    );
    // DragValue can round during layout. Paint and focus alone are not edits.
    if !actual_numeric_input(ui, &response) {
        *value = before;
    }
    let reset_value = default.clamp(*range.start(), *range.end());
    let reset_shortcut = response.has_focus()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Num0));
    let mut reset_menu = false;
    response.context_menu(|ui| {
        if ui
            .button(format!("Reset to {reset_value:.2} {label}"))
            .clicked()
        {
            reset_menu = true;
            ui.close();
        }
    });
    if reset_shortcut || reset_menu {
        *value = reset_value;
        // Discard a pending text buffer instead of recommitting it after reset.
        response.surrender_focus();
    }
    let id = response.id;
    response.on_hover_text(format!(
        "Drag to adjust. Click to type.\nRight-click: reset to {reset_value:.2} {label}\nCtrl+0: reset the focused field."
    ));
    (*value != before, id)
}

fn actual_numeric_input(ui: &Ui, response: &Response) -> bool {
    response.changed()
        && (ui.input(|input| input.events.iter().any(|event| {
                matches!(event, egui::Event::AccessKitActionRequest(request)
                    if request.target_node == response.id.accesskit_id() && request.target_tree == egui::accesskit::TreeId::ROOT
                        && matches!(request.action, egui::accesskit::Action::SetValue | egui::accesskit::Action::Increment | egui::accesskit::Action::Decrement))
            }))
            || response.dragged()
            || ((response.has_focus() || response.lost_focus())
                && ui.input(|input| {
                    input.events.iter().any(|event| {
                        matches!(
                            event,
                            egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_)
                        )
                    })
                })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, RawInput, Rect, Vec2};
    fn frame(
        ctx: &egui::Context,
        value: &mut f32,
        events: Vec<Event>,
    ) -> (bool, Rect, Option<Rect>) {
        let mut changed = false;
        let mut field = Rect::NOTHING;
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 400.0))),
                events,
                ..Default::default()
            },
            |ui| {
                let response = ui.horizontal(|ui| {
                    changed = resettable_numeric(ui, "dB", value, -24.0..=24.0, 0.1, 0.0);
                });
                field = response.response.rect;
            },
        );
        let reset = output.shapes.iter().find_map(|clipped| {
            if let egui::Shape::Text(text) = &clipped.shape
                && text.galley.text().starts_with("Reset to ")
            {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            } else {
                None
            }
        });
        output.textures_delta.clear();
        (changed, field, reset)
    }
    fn pointer(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }
    #[test]
    fn context_reset_changes_value_and_idle_preserves_it() {
        let ctx = egui::Context::default();
        let mut value = 5.0;
        let (_, rect, _) = frame(&ctx, &mut value, vec![]);
        let pos = rect.right_center() - Vec2::new(10.0, 0.0);
        frame(
            &ctx,
            &mut value,
            vec![
                Event::PointerMoved(pos),
                pointer(pos, PointerButton::Secondary, true),
            ],
        );
        let (_, _, mut reset) = frame(
            &ctx,
            &mut value,
            vec![pointer(pos, PointerButton::Secondary, false)],
        );
        if reset.is_none() {
            reset = frame(&ctx, &mut value, vec![]).2;
        }
        let reset = reset
            .expect("right-click must offer an explicit reset")
            .center();
        frame(
            &ctx,
            &mut value,
            vec![
                Event::PointerMoved(reset),
                pointer(reset, PointerButton::Primary, true),
            ],
        );
        assert!(
            frame(
                &ctx,
                &mut value,
                vec![pointer(reset, PointerButton::Primary, false)]
            )
            .0
        );
        assert_eq!(value, 0.0);
        assert!(!frame(&ctx, &mut value, vec![]).0);
    }
    #[test]
    fn focused_control_accepts_text_and_control_zero_resets() {
        let ctx = egui::Context::default();
        let mut value = 5.0;
        let (_, rect, _) = frame(&ctx, &mut value, vec![]);
        let pos = rect.right_center() - Vec2::new(10.0, 0.0);
        frame(
            &ctx,
            &mut value,
            vec![
                Event::PointerMoved(pos),
                pointer(pos, PointerButton::Primary, true),
            ],
        );
        frame(
            &ctx,
            &mut value,
            vec![pointer(pos, PointerButton::Primary, false)],
        );
        let key = |key, modifiers| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        frame(
            &ctx,
            &mut value,
            vec![
                key(egui::Key::A, egui::Modifiers::CTRL),
                Event::Text("7.5".into()),
            ],
        );
        frame(
            &ctx,
            &mut value,
            vec![key(egui::Key::Enter, egui::Modifiers::default())],
        );
        assert_eq!(value, 7.5, "click-to-type must remain available");
        frame(
            &ctx,
            &mut value,
            vec![
                Event::PointerMoved(pos),
                pointer(pos, PointerButton::Primary, true),
            ],
        );
        frame(
            &ctx,
            &mut value,
            vec![pointer(pos, PointerButton::Primary, false)],
        );
        assert!(
            frame(
                &ctx,
                &mut value,
                vec![key(egui::Key::Num0, egui::Modifiers::CTRL)]
            )
            .0
        );
        assert_eq!(value, 0.0);
        assert!(!frame(&ctx, &mut value, vec![]).0);
    }
    #[test]
    fn accessibility_value_actions_target_only_the_requested_numeric_field() {
        let ctx = egui::Context::default();
        let mut values = [0.0, 0.0];
        let mut ids = [egui::Id::NULL; 2];
        let mut run = |events: Vec<Event>| {
            let mut changed = [false; 2];
            let mut output = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.horizontal(|ui| {
                        for i in 0..2 {
                            let result =
                                numeric_control(ui, "dB", &mut values[i], -24.0..=24.0, 0.1, 0.0);
                            changed[i] = result.0;
                            ids[i] = result.1;
                        }
                    });
                },
            );
            output.textures_delta.clear();
            (changed, values, ids)
        };
        let (_, _, ids) = run(vec![]);
        let request = |target, action, data| {
            Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
                action,
                target_tree: egui::accesskit::TreeId::ROOT,
                target_node: target,
                data,
            })
        };
        let (changed, values, _) = run(vec![request(
            ids[0].accesskit_id(),
            egui::accesskit::Action::SetValue,
            Some(egui::accesskit::ActionData::NumericValue(6.0)),
        )]);
        assert_eq!(changed, [true, false]);
        assert_eq!(values, [6.0, 0.0]);
        let (changed, values, _) = run(vec![request(
            ids[1].accesskit_id(),
            egui::accesskit::Action::Increment,
            None,
        )]);
        assert_eq!(changed, [false, true]);
        assert!((values[1] - 0.1).abs() < 0.0001);
        let (changed, values, _) = run(vec![request(
            ids[1].accesskit_id(),
            egui::accesskit::Action::Decrement,
            None,
        )]);
        assert_eq!(changed, [false, true]);
        assert!(values[1].abs() < 0.0001);
        let (changed, values, _) = run(vec![]);
        assert_eq!(changed, [false, false]);
        assert_eq!(values, [6.0, 0.0]);
    }
}
