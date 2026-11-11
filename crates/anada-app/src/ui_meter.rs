use super::*;

fn peak_db(peak: f32) -> Option<f32> {
    (peak.is_finite() && peak > 0.0000316).then(|| 20.0 * peak.log10())
}

const FLOOR_DB: f32 = -90.0;
const RELEASE_DB_PER_SECOND: f32 = 18.0;

#[derive(Clone)]
struct MeterDisplay {
    levels: [f32; 2],
    hold_until: [f64; 2],
    clip_until: [f64; 2],
    last_time: f64,
}

impl Default for MeterDisplay {
    fn default() -> Self {
        Self {
            levels: [FLOOR_DB; 2],
            hold_until: [0.0; 2],
            clip_until: [0.0; 2],
            last_time: 0.0,
        }
    }
}

impl MeterDisplay {
    fn update(&mut self, peaks: [f32; 2], now: f64, fresh: bool) {
        let previous = self.last_time;
        self.last_time = now;
        if !fresh {
            // A disconnected/stopped stream must never look like a live measurement.
            *self = Self {
                last_time: now,
                ..Self::default()
            };
            return;
        }
        for (index, peak) in peaks.into_iter().enumerate() {
            let target = peak_db(peak).unwrap_or(FLOOR_DB).clamp(FLOOR_DB, 0.0);
            if target >= -0.1 {
                self.clip_until[index] = now + 0.75;
            }
            if target >= self.levels[index] {
                self.levels[index] = target;
                self.hold_until[index] = now + 0.12;
            } else {
                let release_time = (now - previous.max(self.hold_until[index])).max(0.0) as f32;
                self.levels[index] =
                    target.max(self.levels[index] - release_time * RELEASE_DB_PER_SECOND);
            }
        }
    }
}

impl AnadaApp {
    fn signal_peak(&self, endpoint: &Endpoint) -> [f32; 2] {
        if !self.meters.running
            || !self
                .meter_received
                .is_some_and(|at| at.elapsed() < Duration::from_millis(300))
        {
            return [0.0; 2];
        }
        if !endpoint.is_virtual {
            return self.meters.output_peak;
        }
        let channel = self.host.as_ref().and_then(|host| {
            host.config
                .channels
                .iter()
                .find(|channel| channel.endpoint_id.as_deref() == Some(endpoint.id.as_str()))
        });
        channel
            .and_then(|channel| {
                self.meters
                    .channels
                    .iter()
                    .find(|meter| meter.channel_id == channel.id)
            })
            .map(|meter| meter.peak)
            .unwrap_or_default()
    }

    pub(super) fn paint_signal_meter(
        &self,
        ui: &egui::Ui,
        endpoint: &Endpoint,
        rect: egui::Rect,
        vertical: bool,
    ) {
        let now = ui.input(|input| input.time);
        let fresh = self.meters.running
            && self
                .meter_received
                .is_some_and(|at| at.elapsed() < Duration::from_millis(300));
        let id = egui::Id::unique(("signal-meter-display", &endpoint.id));
        let mut display = ui
            .ctx()
            .data_mut(|data| data.get_temp::<MeterDisplay>(id).unwrap_or_default());
        display.update(self.signal_peak(endpoint), now, fresh);
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, display.clone()));
        if fresh && display.levels.iter().any(|level| *level > FLOOR_DB) {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        let painter = ui.painter_at(rect);
        for (index, db) in display.levels.into_iter().enumerate() {
            let fraction = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            let color = if db >= -0.1 {
                BAD
            } else if db >= -6.0 {
                WARN
            } else {
                GOOD
            };
            let lane = if vertical {
                egui::Rect::from_min_max(
                    rect.left_top() + egui::vec2(3.0 + index as f32 * 5.0, 8.0),
                    egui::pos2(rect.left() + 6.0 + index as f32 * 5.0, rect.bottom() - 24.0),
                )
            } else {
                // Matches the horizontal fader track, which ends 72 px from the right.
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 8.0, rect.bottom() - 11.0 + index as f32 * 5.0),
                    egui::pos2(
                        rect.right() - 72.0,
                        rect.bottom() - 8.0 + index as f32 * 5.0,
                    ),
                )
            };
            painter.rect_filled(lane, 1.0, FIELD);
            let fill = if vertical {
                egui::Rect::from_min_max(
                    egui::pos2(lane.left(), lane.bottom() - lane.height() * fraction),
                    lane.right_bottom(),
                )
            } else {
                egui::Rect::from_min_max(
                    lane.min,
                    egui::pos2(lane.left() + lane.width() * fraction, lane.bottom()),
                )
            };
            painter.rect_filled(fill, 1.0, color);
            if display.clip_until[index] > now {
                let tick = if vertical {
                    egui::Rect::from_min_size(lane.min, egui::vec2(lane.width(), 2.0))
                } else {
                    egui::Rect::from_min_size(
                        lane.right_top() - egui::vec2(2.0, 0.0),
                        egui::vec2(2.0, lane.height()),
                    )
                };
                painter.rect_filled(tick, 0.0, BAD);
            }
        }
        let peak = display.levels[0].max(display.levels[1]);
        let text = if peak > FLOOR_DB {
            format!("{peak:.0} dBFS")
        } else {
            "-inf dBFS".into()
        };
        painter.text(
            if vertical {
                egui::pos2(rect.center().x, rect.bottom() - 4.0)
            } else {
                egui::pos2(rect.right() - 2.0, rect.bottom() - 7.0)
            },
            if vertical {
                Align2::CENTER_BOTTOM
            } else {
                Align2::RIGHT_CENTER
            },
            text,
            FontId::monospace(10.0),
            MUTED,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peak_attack_is_immediate_then_holds_and_releases_by_elapsed_time() {
        let mut display = MeterDisplay::default();
        display.update([1.0, 0.1], 1.0, true);
        assert_eq!(display.levels[0], 0.0);
        assert!((display.levels[1] + 20.0).abs() < 0.001);
        display.update([0.0; 2], 1.10, true);
        assert_eq!(display.levels[0], 0.0, "brief dips should not flicker");
        display.update([0.0; 2], 1.62, true);
        assert!((display.levels[0] + 9.0).abs() < 0.001);
        assert!(
            display.clip_until[0] > 1.62,
            "clipping must stay visible after the transient"
        );
        let mut frequent = MeterDisplay::default();
        frequent.update([1.0, 0.1], 1.0, true);
        for step in 1..=62 {
            frequent.update([0.0; 2], 1.0 + step as f64 * 0.01, true);
        }
        assert!(
            (frequent.levels[0] - display.levels[0]).abs() < 0.001,
            "frame rate must not change the decay"
        );
        display.update([0.0; 2], 8.0, true);
        assert_eq!(display.levels, [FLOOR_DB; 2]);
    }

    #[test]
    fn stale_or_stopped_meters_clear_including_clip_marks() {
        let mut display = MeterDisplay::default();
        display.update([1.0; 2], 1.0, true);
        display.update([1.0; 2], 1.1, false);
        assert_eq!(display.levels, [FLOOR_DB; 2]);
        assert_eq!(display.clip_until, [0.0; 2]);
        display.update([f32::NAN, f32::INFINITY], 2.0, true);
        assert_eq!(display.levels, [FLOOR_DB; 2]);
    }
    #[test]
    fn digital_peak_is_dbfs_and_silence_is_not_a_fake_zero_db_level() {
        assert_eq!(peak_db(1.0), Some(0.0));
        assert!((peak_db(0.1).unwrap() + 20.0).abs() < 0.0001);
        for peak in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(peak_db(peak).is_none());
        }
    }
}
