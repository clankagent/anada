//! Interactive, stable-index parametric EQ editor.
use anada_dsp::{EqBand, EqSettings, FilterKind, response_db};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

const MUTED: Color32 = Color32::from_rgb(163, 171, 175);
const TEXT: Color32 = Color32::from_rgb(232, 234, 234);

pub struct Editor {
    target: String,
    pub(crate) graph_rect: Option<Rect>,
    /// Height of the band controls below the graph, measured on the last frame.
    pub(crate) controls_height: f32,
    selected: usize,
    drag: Option<(usize, EqBand, Pos2)>,
    spectrum: crate::ui_spectrum::SpectrumDisplay,
    analysis: Option<crate::model::Spectrum>,
    spectrum_enabled: bool,
    sample_rate: u32,
    range_db: u8,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            target: String::new(),
            graph_rect: None,
            controls_height: 0.0,
            selected: 0,
            drag: None,
            spectrum: Default::default(),
            analysis: None,
            spectrum_enabled: false,
            sample_rate: 0,
            range_db: 16,
        }
    }
}

pub(crate) fn band_position(plot: Rect, band: &EqBand, range_db: f32) -> Pos2 {
    Pos2::new(
        plot.left() + (band.frequency_hz / 20.0).log10() / 3.0 * plot.width(),
        plot.center().y - band.gain_db / (2.0 * range_db) * plot.height(),
    )
}
fn handle_position(plot: Rect, band: &EqBand, range_db: f32) -> Pos2 {
    let mut position = band_position(plot, band, range_db);
    position.y = position.y.clamp(plot.top() + 8.0, plot.bottom() - 8.0);
    position
}

fn values_at(plot: Rect, pos: Pos2, range_db: f32) -> (f32, f32) {
    (
        20.0 * 1000.0_f32.powf(((pos.x - plot.left()) / plot.width()).clamp(0.0, 1.0)),
        ((plot.center().y - pos.y) / plot.height() * (2.0 * range_db)).clamp(-24.0, 24.0),
    )
}
fn default_band(frequency_hz: f32, gain_db: f32) -> EqBand {
    EqBand {
        enabled: true,
        kind: FilterKind::Bell,
        frequency_hz,
        gain_db,
        q: 1.0,
    }
}

impl Editor {
    pub(crate) fn set_range_db(&mut self, range_db: u8) {
        if self.range_db != range_db {
            self.range_db = range_db;
            self.drag = None;
        }
    }
    pub(crate) fn analysis(
        &mut self,
        spectrum: Option<&crate::model::Spectrum>,
        enabled: bool,
        sample_rate: u32,
    ) {
        if !enabled || spectrum.is_none() {
            self.spectrum.clear();
        }
        self.analysis = spectrum.filter(|_| enabled).cloned();
        self.spectrum_enabled = enabled;
        self.sample_rate = sample_rate.max(8_000);
    }

    /// Returns true only when settings actually change as a result of user input.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        target: &str,
        eq: &mut EqSettings,
        height: f32,
        accent: Color32,
    ) -> bool {
        if self.target != target {
            self.target = target.into();
            self.selected = 0;
            self.drag = None;
            self.spectrum.clear();
        }
        self.selected = self.selected.min(eq.bands.len().saturating_sub(1));
        let before = eq.clone();
        let range_db = f32::from(self.range_db);
        let shown = ui.push_id(target, |ui| {
            let (outer, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height.max(180.0)), Sense::click_and_drag());
            let plot = Rect::from_min_max(outer.min + Vec2::new(40.0, 27.0), outer.max - Vec2::new(if self.spectrum_enabled {48.0} else {18.0}, 25.0));
            self.graph_rect = Some(plot);
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let nearest = pointer.and_then(|p| eq.bands.iter().enumerate().filter_map(|(i,b)| {
                let distance = handle_position(plot,b,range_db).distance(p);
                (distance <= 14.0).then_some((i,distance))
            }).min_by(|a,b| a.1.total_cmp(&b.1)).map(|(i,_)|i));
            if response.clicked() || response.drag_started() || response.secondary_clicked() {
                response.request_focus();
                if let Some(index) = self.drag.as_ref().map(|(index, _, _)| *index).or(nearest) { self.selected = index; }
            }
            if response.clicked() && ui.input(|i| i.modifiers.alt) && let Some(index) = nearest {
                eq.bands[index].enabled = !eq.bands[index].enabled;
            }
            if response.double_clicked() && nearest.is_none() && let Some(p) = pointer.filter(|p| plot.contains(*p)) {
                    let (frequency,gain) = values_at(plot,p,range_db);
                    let mut band = default_band(frequency,gain);
                    if p.x < plot.left()+plot.width()*0.08 { band.kind = FilterKind::LowShelf; }
                    if p.x > plot.right()-plot.width()*0.08 { band.kind = FilterKind::HighShelf; }
                    if let Some(index) = eq.bands.iter().position(|b| !b.enabled) { eq.bands[index] = band; self.selected = index; }
                    else if eq.bands.len() < 10 { self.selected = eq.bands.len(); eq.bands.push(band); } else if let Some((index, _)) = eq.bands.iter().enumerate().min_by(|(_, a), (_, b)| handle_position(plot,a,range_db).distance(p).total_cmp(&handle_position(plot,b,range_db).distance(p))) { eq.bands[index] = band; self.selected = index; }
            }
            // Native input can deliver the entire gesture before a repaint. Hit-test
            // the actual press event, rather than the pointer's final position.
            let pressed_at = ui.input(|input| input.events.iter().find_map(|event| match event {
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } => Some(*pos),
                _ => None,
            }));
            if ui.is_enabled() && ui.rect_contains_pointer(outer) && let Some(origin) = pressed_at.filter(|p| outer.contains(*p) && ui.clip_rect().contains(*p))
                && let Some((index, _)) = eq.bands.iter().enumerate()
                    .map(|(index, band)| (index, handle_position(plot, band,range_db).distance(origin)))
                    .filter(|(_, distance)| *distance <= 14.0)
                    .min_by(|a,b| a.1.total_cmp(&b.1)) {
                self.selected = index;
                self.drag = Some((index,eq.bands[index].clone(),origin));
                response.request_focus();
            }
            let released = ui.input(|i| i.pointer.primary_released());
            if (response.dragged() || response.drag_stopped() || released)
                && let (Some((index, initial, origin)),Some(p)) = (&self.drag,pointer)
                && origin.distance(p) > 0.0 {
                    let modifiers = ui.input(|i|i.modifiers);
                    let delta = (p-*origin) * if modifiers.shift {0.25} else {1.0};
                    let band = &mut eq.bands[*index];
                    if modifiers.ctrl { band.q = (initial.q * (-delta.y*0.015).exp()).clamp(0.1,20.0); }
                    else {
                        let delta = if modifiers.alt { if delta.x.abs()>delta.y.abs() {Vec2::new(delta.x,0.0)} else {Vec2::new(0.0,delta.y)} } else {delta};
                        let (freq,gain) = values_at(plot,band_position(plot,initial,range_db)+delta,range_db);
                        band.frequency_hz = freq; band.gain_db = gain;
                    }
            }
            if response.drag_stopped() || released { self.drag = None; }
            if response.hovered() && let Some(index) = nearest {
                let wheel: f32 = ui.input(|i| i.events.iter().filter_map(|event| match event { egui::Event::MouseWheel { delta, unit, .. } => Some(delta.y * match unit { egui::MouseWheelUnit::Point => 1.0, egui::MouseWheelUnit::Line => 40.0, egui::MouseWheelUnit::Page => 200.0 }), _ => None }).sum());
                if wheel != 0.0 { let band = &mut eq.bands[index]; band.q=(band.q*(wheel*0.004).exp()).clamp(0.1,20.0); }
            }
            if response.has_focus() {
                ui.input(|input| {
                    if input.key_pressed(egui::Key::PageUp) { self.selected = self.selected.saturating_sub(1); }
                    if input.key_pressed(egui::Key::PageDown) { self.selected = (self.selected+1).min(eq.bands.len().saturating_sub(1)); }
                    if input.key_pressed(egui::Key::B) {eq.bypass = !eq.bypass;}
                    if let Some(band) = eq.bands.get_mut(self.selected) {
                        let fine = if input.modifiers.shift {0.25} else {1.0};
                        let horizontal = i32::from(input.key_pressed(egui::Key::ArrowRight))-i32::from(input.key_pressed(egui::Key::ArrowLeft));
                        let vertical = i32::from(input.key_pressed(egui::Key::ArrowUp))-i32::from(input.key_pressed(egui::Key::ArrowDown));
                        if horizontal != 0 {band.frequency_hz=(band.frequency_hz*2.0_f32.powf(horizontal as f32*fine/12.0)).clamp(20.0,20000.0);}
                        if vertical != 0 {if input.modifiers.ctrl {band.q=(band.q*2.0_f32.powf(vertical as f32*fine/12.0)).clamp(0.1,20.0);} else {band.gain_db=(band.gain_db+vertical as f32*0.5*fine).clamp(-24.0,24.0);}}
                        if input.key_pressed(egui::Key::Delete) {band.enabled=false;}
                        if input.key_pressed(egui::Key::E) {band.enabled = !band.enabled;}
                    }
                });
            }
            response.context_menu(|ui| {
                if let Some(band) = eq.bands.get_mut(self.selected) {
                    ui.label(format!("Band {}",self.selected+1));
                    ui.checkbox(&mut band.enabled,"Enabled");
                    types(ui, &mut band.kind);
                    if ui.button("Reset band").clicked() {band.gain_db=0.0;band.q=1.0;ui.close();}
                    if ui.button("Disable band").clicked() {band.enabled = false;ui.close();}
                }
            });
            let painter = ui.painter_at(outer);
            painter.rect_filled(outer,4.0,Color32::from_rgb(18,20,21));
            if self.spectrum_enabled {
                self.spectrum.paint(ui, plot, self.analysis.as_ref());
            }
            painter.text(Pos2::new(plot.left(),outer.top()+8.0),egui::Align2::LEFT_TOP,"EQ dB",egui::FontId::monospace(10.0),accent);
            for (f,label) in [(20.0,"20"),(50.0,"50"),(100.0,"100"),(200.0,"200"),(500.0,"500"),(1000.0,"1k"),(2000.0,"2k"),(5000.0,"5k"),(10000.0,"10k"),(20000.0,"20k")] {
                let x=band_position(plot,&default_band(f,0.0),range_db).x;
                painter.line_segment([Pos2::new(x,plot.top()),Pos2::new(x,plot.bottom())],Stroke::new(1.0,Color32::from_rgb(52,57,60)));
                painter.text(Pos2::new(x,plot.bottom()+5.0),egui::Align2::CENTER_TOP,label,egui::FontId::monospace(10.0),MUTED);
            }
            for index in -4..=4 {
                let gain = range_db * index as f32 / 4.0;
                let y=band_position(plot,&default_band(1000.0,gain),range_db).y;
                painter.line_segment([Pos2::new(plot.left(),y),Pos2::new(plot.right(),y)],Stroke::new(1.0,if gain==0.0 {Color32::from_rgb(107,116,122)} else {Color32::from_rgb(52,57,60)}));
                painter.text(Pos2::new(plot.left()-5.0,y),egui::Align2::RIGHT_CENTER,format!("{gain}"),egui::FontId::monospace(10.0),MUTED);
            }
            let curve = |settings:&EqSettings| -> Vec<Pos2> {(0..=240).map(|i| {
                let frequency=20.0*1000.0_f32.powf(i as f32/240.0);
                let gain=response_db(settings,frequency,if self.sample_rate >= 8_000 { self.sample_rate } else {48_000}).unwrap_or(0.0);
                band_position(plot,&default_band(frequency,gain.clamp(-range_db,range_db)),range_db)
            }).collect()};
            if let Some(band)=eq.bands.get(self.selected) {
                let individual=EqSettings{bypass:false,preamp_db:0.0,bands:vec![band.clone()]};
                painter.add(egui::Shape::line(curve(&individual),Stroke::new(1.0,accent.gamma_multiply(0.35))));
            }
            let points=curve(eq);
            if eq.bypass {for (i,pair) in points.windows(2).enumerate() {if i%4<2 {painter.line_segment([pair[0],pair[1]],Stroke::new(2.0,MUTED));}}}
            else {painter.add(egui::Shape::line(points,Stroke::new(2.0,accent)));}
            for (index,band) in eq.bands.iter().enumerate() {
                let p=handle_position(plot,band,range_db); let selected=index==self.selected;
                let color=if band.enabled {accent} else {MUTED};
                painter.circle_filled(p,if selected {10.0} else {8.0},Color32::from_rgb(18,20,21));
                painter.circle(p,if selected {7.0} else {5.0},if selected {color} else {Color32::from_rgb(18,20,21)},Stroke::new(1.5,color));
                painter.text(p-Vec2::new(0.0,10.0),egui::Align2::CENTER_BOTTOM,format!("{}",index+1),egui::FontId::monospace(11.0),TEXT);
                if band.gain_db.abs() > range_db {
                    painter.text(p+Vec2::new(10.0,0.0),egui::Align2::LEFT_CENTER,if band.gain_db > 0.0 {"↑"} else {"↓"},egui::FontId::monospace(11.0),color);
                }
            }
            if eq.bypass {painter.text(plot.left_top()+Vec2::splat(8.0),egui::Align2::LEFT_TOP,"Bypassed  -  edits are kept",egui::FontId::proportional(12.0),MUTED);}
            if response.has_focus() {painter.rect_stroke(outer,4.0,Stroke::new(2.0,accent),egui::StrokeKind::Inside);}
            if let Some(index)=nearest {let b=&eq.bands[index];response.on_hover_text(format!("Band {}  -  {:.0} Hz  -  {:+.1} dB  -  Q {:.2}\nDrag: frequency/gain  -  Wheel or Ctrl-drag: Q  -  Shift: fine\nRight-click: band options",index+1,b.frequency_hz,b.gain_db,b.q));}
            ui.horizontal_wrapped(|ui| {
                for index in 0..eq.bands.len() {
                    let b=&eq.bands[index];
                    if ui.selectable_label(self.selected==index,format!("{}{}",index+1,if b.enabled {""} else {" off"})).clicked() {self.selected=index;}
                }
            });
            if let Some(band)=eq.bands.get_mut(self.selected) {
                ui.horizontal_wrapped(|ui| {
                    types(ui,&mut band.kind);
                    crate::ui_numeric::resettable_numeric(ui,"Hz",&mut band.frequency_hz,20.0..=20000.0,1.0, [31.25,62.5,125.0,250.0,500.0,1000.0,2000.0,4000.0,8000.0,16000.0][self.selected.min(9)]);
                    crate::ui_numeric::resettable_numeric(ui,"dB",&mut band.gain_db,-24.0..=24.0,0.1,0.0);
                    crate::ui_numeric::resettable_numeric(ui,"Q",&mut band.q,0.1..=20.0,0.01,std::f32::consts::FRAC_1_SQRT_2);
                    ui.checkbox(&mut band.enabled,"On");
                    if ui.small_button("Reset").clicked() {band.gain_db=0.0;band.q=1.0;}
                });
            }
            outer.bottom()
        });
        self.controls_height = shown.response.rect.bottom() - shown.inner;
        *eq != before
    }
}
fn types(ui: &mut egui::Ui, kind: &mut FilterKind) {
    ui.selectable_value(kind, FilterKind::Bell, "Bell");
    ui.selectable_value(kind, FilterKind::LowShelf, "Low shelf");
    ui.selectable_value(kind, FilterKind::HighShelf, "High shelf");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logarithmic_coordinates_round_trip_and_clamp() {
        let rect = Rect::from_min_size(Pos2::new(30.0, 20.0), Vec2::new(600.0, 240.0));
        for range in crate::model::EQ_GRAPH_RANGES_DB {
            let range = f32::from(range);
            for f in [20.0, 31.25, 1000.0, 16000.0, 20000.0] {
                for gain in [-24.0, 0.0, 24.0] {
                    let (actual, g) = values_at(
                        rect,
                        band_position(rect, &default_band(f, gain), range),
                        range,
                    );
                    assert!((actual / f - 1.0).abs() < 0.00001);
                    assert!((g - gain).abs() < 0.0001);
                    assert!(rect.contains(handle_position(rect, &default_band(f, gain), range)));
                }
            }
        }
        assert_eq!(
            values_at(rect, Pos2::new(-999.0, -999.0), 16.0),
            (20.0, 24.0)
        );
        assert_eq!(
            values_at(rect, Pos2::new(9999.0, 9999.0), 16.0),
            (20000.0, -24.0)
        );
        assert_eq!(Editor::default().range_db, 16);
        assert!(
            (band_position(rect, &default_band(1000.0, 8.0), 16.0).y
                - (rect.top() + rect.height() / 4.0))
                .abs()
                < 0.001
        );
    }
    #[test]
    fn idle_frames_preserve_exact_default_values() {
        let context = egui::Context::default();
        let mut editor = Editor::default();
        let mut eq = EqSettings::default();
        let original = eq.clone();
        for _ in 0..5 {
            let mut output = context.run_ui(egui::RawInput::default(), |root_ui| {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    assert!(!editor.show(ui, "test", &mut eq, 300.0, Color32::WHITE));
                });
            });
            output.textures_delta.clear();
        }
        assert_eq!(eq, original);
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    fn frame(
        ctx: &egui::Context,
        editor: &mut Editor,
        eq: &mut EqSettings,
        events: Vec<egui::Event>,
    ) -> bool {
        frame_target(ctx, editor, "target", eq, events)
    }
    fn frame_target(
        ctx: &egui::Context,
        editor: &mut Editor,
        target: &str,
        eq: &mut EqSettings,
        events: Vec<egui::Event>,
    ) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 500.0))),
            events,
            ..Default::default()
        };
        let mut changed = false;
        let mut output = ctx.run_ui(input, |root_ui| {
            egui::CentralPanel::default().show(root_ui, |ui| {
                changed = editor.show(ui, target, eq, 300.0, Color32::WHITE);
            });
        });
        output.textures_delta.clear();
        changed
    }
    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }
    #[test]
    fn changing_output_target_during_drag_preserves_new_output_eq() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let mut first = EqSettings::default();
        let mut second = EqSettings::default();
        second.bands[4].gain_db = -9.0;
        second.bands[4].q = 2.0;
        let original_second = second.clone();
        frame_target(&ctx, &mut editor, "output:first", &mut first, vec![]);
        let start = band_position(editor.graph_rect.unwrap(), &first.bands[4], 16.0);
        frame_target(
            &ctx,
            &mut editor,
            "output:first",
            &mut first,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        let moved = start + Vec2::new(40.0, -30.0);
        assert!(frame_target(
            &ctx,
            &mut editor,
            "output:first",
            &mut first,
            vec![egui::Event::PointerMoved(moved)],
        ));
        assert!(editor.drag.is_some());
        let end = moved + Vec2::new(30.0, -20.0);
        assert!(!frame_target(
            &ctx,
            &mut editor,
            "output:second",
            &mut second,
            vec![egui::Event::PointerMoved(end)],
        ));
        assert!(editor.drag.is_none());
        assert_eq!(second, original_second);
        assert!(!frame_target(
            &ctx,
            &mut editor,
            "output:second",
            &mut second,
            vec![button(end, false)],
        ));
        assert_eq!(second, original_second);
    }
    #[test]
    fn pointer_drag_and_keyboard_change_only_selected_band() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let mut eq = EqSettings::default();
        let original = eq.clone();
        frame(&ctx, &mut editor, &mut eq, vec![]);
        let start = band_position(editor.graph_rect.unwrap(), &eq.bands[5], 16.0);
        frame(
            &ctx,
            &mut editor,
            &mut eq,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        let end = start + Vec2::new(40.0, -30.0);
        assert!(frame(
            &ctx,
            &mut editor,
            &mut eq,
            vec![egui::Event::PointerMoved(end)]
        ));
        frame(&ctx, &mut editor, &mut eq, vec![button(end, false)]);
        assert!(eq.bands[5].frequency_hz > original.bands[5].frequency_hz);
        assert!(eq.bands[5].gain_db > 0.0);
        for i in 0..10 {
            if i != 5 {
                assert_eq!(eq.bands[i], original.bands[i]);
            }
        }
        let gain = eq.bands[5].gain_db;
        assert!(frame(
            &ctx,
            &mut editor,
            &mut eq,
            vec![egui::Event::Key {
                key: egui::Key::ArrowUp,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default()
            }]
        ));
        assert_eq!(eq.bands[5].gain_db, gain + 0.5);
        assert!(!frame(&ctx, &mut editor, &mut eq, vec![]));
    }
    #[test]
    fn wheel_changes_q_once_without_idle_drift() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let mut eq = EqSettings::default();
        frame(&ctx, &mut editor, &mut eq, vec![]);
        let pos = band_position(editor.graph_rect.unwrap(), &eq.bands[4], 16.0);
        let before = eq.clone();
        assert!(frame(
            &ctx,
            &mut editor,
            &mut eq,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    delta: Vec2::new(0.0, 1.0),
                    modifiers: egui::Modifiers::default()
                },
            ]
        ));
        assert!(eq.bands[4].q > before.bands[4].q);
        for i in 0..10 {
            if i != 4 {
                assert_eq!(eq.bands[i], before.bands[i]);
            }
        }
        let after = eq.clone();
        for _ in 0..4 {
            assert!(!frame(&ctx, &mut editor, &mut eq, vec![]));
        }
        assert_eq!(eq, after);
    }
    #[test]
    fn double_click_full_eq_repositions_nearest_band_without_changing_slots() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let mut eq = EqSettings::default();
        frame(&ctx, &mut editor, &mut eq, vec![]);
        let plot = editor.graph_rect.unwrap();
        let pos = band_position(plot, &default_band(1200.0, 9.0), 16.0);
        let original = eq.clone();
        let expected = original
            .bands
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                band_position(plot, a, 16.0)
                    .distance(pos)
                    .total_cmp(&band_position(plot, b, 16.0).distance(pos))
            })
            .unwrap()
            .0;
        for _ in 0..2 {
            frame(
                &ctx,
                &mut editor,
                &mut eq,
                vec![egui::Event::PointerMoved(pos), button(pos, true)],
            );
            frame(&ctx, &mut editor, &mut eq, vec![button(pos, false)]);
        }
        assert_eq!(eq.bands.len(), 10);
        assert_eq!(editor.selected, expected);
        assert!((eq.bands[expected].frequency_hz - 1200.0).abs() < 0.01);
        assert!((eq.bands[expected].gain_db - 9.0).abs() < 0.01);
        for i in 0..10 {
            if i != expected {
                assert_eq!(eq.bands[i], original.bands[i]);
            }
        }
    }
    #[test]
    fn disabling_selected_band_preserves_slot_identity_and_parameters() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let mut eq = EqSettings::default();
        frame(&ctx, &mut editor, &mut eq, vec![]);
        let pos = band_position(editor.graph_rect.unwrap(), &eq.bands[3], 16.0);
        frame(
            &ctx,
            &mut editor,
            &mut eq,
            vec![egui::Event::PointerMoved(pos), button(pos, true)],
        );
        frame(&ctx, &mut editor, &mut eq, vec![button(pos, false)]);
        let original = eq.clone();
        assert!(frame(
            &ctx,
            &mut editor,
            &mut eq,
            vec![egui::Event::Key {
                key: egui::Key::Delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default()
            }]
        ));
        let mut expected = original;
        expected.bands[3].enabled = false;
        assert_eq!(eq, expected);
        assert_eq!(editor.selected, 3);
    }
    #[test]
    fn same_frame_press_move_release_changes_pressed_band_only() {
        let ctx = egui::Context::default();
        let mut editor = Editor::default();
        let mut eq = EqSettings::default();
        frame(&ctx, &mut editor, &mut eq, vec![]);
        let plot = editor.graph_rect.unwrap();
        let start = band_position(plot, &eq.bands[4], 16.0);
        let end = start + Vec2::new(45.0, -68.0);
        let original = eq.clone();
        assert!(frame(
            &ctx,
            &mut editor,
            &mut eq,
            vec![
                egui::Event::PointerMoved(start),
                button(start, true),
                egui::Event::PointerMoved(end),
                button(end, false),
            ]
        ));
        assert_eq!(editor.selected, 4);
        let (frequency, gain) = values_at(plot, end, 16.0);
        assert!((eq.bands[4].frequency_hz - frequency).abs() < 0.01);
        assert!((eq.bands[4].gain_db - gain).abs() < 0.001);
        for i in 0..10 {
            if i != 4 {
                assert_eq!(eq.bands[i], original.bands[i]);
            }
        }
        let after = eq.clone();
        assert!(!frame(&ctx, &mut editor, &mut eq, vec![]));
        assert_eq!(eq, after);
        assert!(editor.drag.is_none());
    }
}
