//! Read-only audio spectrum display. This module never changes audio settings.
use crate::model::Spectrum;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke};

const BINS: usize = 128;
const FLOOR_DBFS: f32 = -90.0;
const RELEASE_DB_PER_SECOND: f32 = 18.0;
const SPECTRUM_COLOR: Color32 = Color32::from_rgb(112, 142, 154);

pub(crate) struct SpectrumDisplay {
    levels: [f32; BINS],
    last_time: Option<f64>,
    visible: bool,
}

impl Default for SpectrumDisplay {
    fn default() -> Self {
        Self {
            levels: [FLOOR_DBFS; BINS],
            last_time: None,
            visible: false,
        }
    }
}

impl SpectrumDisplay {
    /// Clear on target changes or when live data is unavailable.
    pub(crate) fn clear(&mut self) {
        self.levels.fill(FLOOR_DBFS);
        self.last_time = None;
        self.visible = false;
    }

    /// Paint behind the EQ curve and grid. Reserve 42 points to the right of
    /// `plot` for this independent dBFS axis; EQ gain belongs on the left axis.
    pub(crate) fn paint(&mut self, ui: &egui::Ui, plot: Rect, spectrum: Option<&Spectrum>) {
        let Some(target) = spectrum.and_then(resample) else {
            self.clear();
            paint_axis(ui, plot);
            return;
        };
        let now = ui.input(|input| input.time);
        self.update(target, now);
        if !plot.is_positive() {
            return;
        }
        let painter = ui.painter().with_clip_rect(plot.intersect(ui.clip_rect()));
        let mut mesh = egui::epaint::Mesh::default();
        let mut points = Vec::with_capacity(BINS);
        for (index, value) in self.levels.iter().enumerate() {
            let point = spectrum_position(plot, frequency_at(index), *value);
            points.push(point);
            mesh.colored_vertex(point, SPECTRUM_COLOR.gamma_multiply(0.20));
            mesh.colored_vertex(
                Pos2::new(point.x, plot.bottom()),
                SPECTRUM_COLOR.gamma_multiply(0.20),
            );
            if index > 0 {
                let vertex = (index * 2) as u32;
                mesh.add_triangle(vertex - 2, vertex - 1, vertex);
                mesh.add_triangle(vertex, vertex - 1, vertex + 1);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::line(
            points,
            Stroke::new(1.0, SPECTRUM_COLOR.gamma_multiply(0.60)),
        ));
        paint_axis(ui, plot);
        if self
            .levels
            .iter()
            .zip(target)
            .any(|(shown, target)| *shown > target + 0.01)
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
    }

    fn update(&mut self, target: [f32; BINS], now: f64) {
        let elapsed = self
            .last_time
            .map(|previous| (now - previous).max(0.0) as f32)
            .unwrap_or(0.0);
        for (level, target) in self.levels.iter_mut().zip(target) {
            *level = if self.visible {
                target.max(*level - elapsed * RELEASE_DB_PER_SECOND)
            } else {
                target
            };
        }
        self.last_time = Some(now);
        self.visible = true;
    }
}

fn paint_axis(ui: &egui::Ui, plot: Rect) {
    if !plot.is_positive() {
        return;
    }
    let painter = ui.painter();
    let muted = Color32::from_rgb(163, 171, 175);
    for dbfs in [0.0, -20.0, -40.0, -60.0, -80.0, -90.0] {
        let y = spectrum_position(plot, 20_000.0, dbfs).y;
        painter.line_segment(
            [
                Pos2::new(plot.right() + 2.0, y),
                Pos2::new(plot.right() + 5.0, y),
            ],
            Stroke::new(1.0, muted),
        );
        painter.text(
            Pos2::new(plot.right() + 8.0, y),
            egui::Align2::LEFT_CENTER,
            format!("{dbfs:.0}"),
            egui::FontId::monospace(10.0),
            muted,
        );
    }
    painter.text(
        Pos2::new(plot.right() + 8.0, plot.top() - 13.0),
        egui::Align2::LEFT_TOP,
        "dBFS",
        egui::FontId::monospace(10.0),
        muted,
    );
}

fn frequency_at(index: usize) -> f32 {
    20.0 * 1000.0_f32.powf(index as f32 / (BINS - 1) as f32)
}

fn spectrum_position(plot: Rect, frequency: f32, dbfs: f32) -> Pos2 {
    let frequency = if frequency.is_finite() {
        frequency.clamp(20.0, 20_000.0)
    } else {
        20.0
    };
    let level = finite_level(dbfs);
    Pos2::new(
        plot.left() + (frequency / 20.0).log10() / 3.0 * plot.width(),
        plot.bottom() - (level - FLOOR_DBFS) / -FLOOR_DBFS * plot.height(),
    )
}

fn finite_level(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(FLOOR_DBFS, 0.0)
    } else {
        FLOOR_DBFS
    }
}

/// Resample measured frequencies onto fixed log display bins. Invalid levels
/// become silence; invalid frequencies are ignored. No spectrum is fabricated.
fn resample(spectrum: &Spectrum) -> Option<[f32; BINS]> {
    if spectrum.sample_rate < 41 || spectrum.frequencies_hz.len() != spectrum.dbfs.len() {
        return None;
    }
    let nyquist = spectrum.sample_rate as f32 * 0.5;
    let valid = |frequency: f32| {
        frequency.is_finite() && (20.0..=20_000.0).contains(&frequency) && frequency <= nyquist
    };
    if !spectrum
        .frequencies_hz
        .iter()
        .take(BINS)
        .any(|frequency| valid(*frequency))
    {
        return None;
    }
    let mut result = [FLOOR_DBFS; BINS];
    for (index, level) in result.iter_mut().enumerate() {
        let target = frequency_at(index);
        if target > nyquist {
            continue;
        }
        let mut lower: Option<(f32, f32)> = None;
        let mut upper: Option<(f32, f32)> = None;
        for (&frequency, &value) in spectrum
            .frequencies_hz
            .iter()
            .zip(&spectrum.dbfs)
            .take(BINS)
        {
            if !valid(frequency) {
                continue;
            }
            let value = finite_level(value);
            if frequency <= target && lower.is_none_or(|(f, _)| frequency > f) {
                lower = Some((frequency, value));
            }
            if frequency >= target && upper.is_none_or(|(f, _)| frequency < f) {
                upper = Some((frequency, value));
            }
        }
        *level = match (lower, upper) {
            (Some((a, level_a)), Some((b, level_b))) if b > a => {
                let t = (target / a).ln() / (b / a).ln();
                finite_level(level_a + (level_b - level_a) * t)
            }
            (Some((_, value)), _) | (_, Some((_, value))) => value,
            _ => FLOOR_DBFS,
        };
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinates_use_log_frequency_and_independent_dbfs_scale() {
        let plot = Rect::from_min_max(Pos2::new(40.0, 20.0), Pos2::new(640.0, 320.0));
        assert_eq!(spectrum_position(plot, 20.0, 0.0), plot.left_top());
        assert_eq!(
            spectrum_position(plot, 20_000.0, -90.0),
            plot.right_bottom()
        );
        assert!((spectrum_position(plot, 632.4555, -45.0).x - plot.center().x).abs() < 0.01);
        assert_eq!(spectrum_position(plot, 1000.0, -45.0).y, plot.center().y);
        assert_eq!(spectrum_position(plot, 0.0, 10.0), plot.left_top());
        assert_eq!(
            spectrum_position(plot, f32::NAN, f32::INFINITY),
            plot.left_bottom()
        );
    }
    #[test]
    fn invalid_data_cannot_draw_nonfinite_or_false_full_scale_levels() {
        let spectrum = Spectrum {
            sample_rate: 48_000,
            frequencies_hz: vec![20.0, 1000.0, 20_000.0, f32::NAN],
            dbfs: vec![f32::NAN, f32::INFINITY, -200.0, 0.0],
        };
        assert_eq!(resample(&spectrum), Some([FLOOR_DBFS; BINS]));
        assert!(resample(&Spectrum::default()).is_none());
        assert!(
            resample(&Spectrum {
                sample_rate: 48_000,
                frequencies_hz: vec![1000.0],
                dbfs: vec![]
            })
            .is_none()
        );
    }
    #[test]
    fn attack_is_immediate_release_is_time_based_and_clear_discards_history() {
        let mut display = SpectrumDisplay::default();
        display.update([-50.0; BINS], 1.0);
        display.update([-10.0; BINS], 1.1);
        assert_eq!(display.levels, [-10.0; BINS]);
        display.update([-70.0; BINS], 1.6);
        assert_eq!(display.levels, [-19.0; BINS]);
        display.clear();
        assert_eq!(display.levels, [FLOOR_DBFS; BINS]);
        assert!(!display.visible);
        display.update([-70.0; BINS], 1.7);
        assert_eq!(display.levels, [-70.0; BINS]);
    }
    #[test]
    fn source_values_and_nyquist_are_respected() {
        let spectrum = Spectrum {
            sample_rate: 32_000,
            frequencies_hz: vec![20.0, 1000.0, 16_000.0],
            dbfs: vec![-80.0, -20.0, -40.0],
        };
        let values = resample(&spectrum).unwrap();
        assert_eq!(values[0], -80.0);
        assert_eq!(values[BINS - 1], FLOOR_DBFS);
        assert!(
            values
                .iter()
                .all(|value| value.is_finite() && (FLOOR_DBFS..=0.0).contains(value))
        );
    }
    #[test]
    fn missing_live_frame_clears_instead_of_releasing_stale_data() {
        let ctx = egui::Context::default();
        let mut display = SpectrumDisplay::default();
        display.update([-6.0; BINS], 1.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            display.paint(
                ui,
                Rect::from_min_max(Pos2::new(20.0, 20.0), Pos2::new(500.0, 250.0)),
                None,
            );
        });
        output.textures_delta.clear();
        assert_eq!(display.levels, [FLOOR_DBFS; BINS]);
        assert!(!display.visible);
        assert!(display.last_time.is_none());
    }
}
