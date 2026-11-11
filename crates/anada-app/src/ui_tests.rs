use super::*;
fn frame(app: &mut AnadaApp, ctx: &egui::Context, size: [f32; 2], events: Vec<egui::Event>) {
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(size[0], size[1]),
        )),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(raw, |ui| app.show(ui));
    output.textures_delta.clear();
}
fn fixture(ctx: &egui::Context) -> AnadaApp {
    let mut app = AnadaApp::new(ctx.clone(), true);
    // Consume the real asynchronous Start result before tests alter their
    // fixture state. A late Start acknowledgement otherwise restores the
    // original demo sessions during the first interaction frame.
    let deadline = Instant::now() + Duration::from_secs(2);
    while app.host.is_none() || app.starting {
        app.poll();
        assert!(Instant::now() < deadline, "demo Start did not acknowledge");
        std::thread::yield_now();
    }
    assert!(app.connected);
    app
}
fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    }
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }
}

#[test]
fn height_resizing_keeps_equal_columns_and_inline_eq_with_fractional_values() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    demo::stress(app.host.as_mut().unwrap());
    app.draft = Some(app.host.as_ref().unwrap().config.clone());
    let original = app.draft.clone().unwrap();
    for height in [1180.0, 1057.0, 755.0, 540.0, 1180.0] {
        frame(&mut app, &ctx, [1236.0, height], vec![]);
        let gaming = app.strip_rects["gaming"];
        let output = app.strip_rects["output"];
        let unassigned = app.strip_rects["not-in-anada"];
        assert!(
            output.top() > gaming.bottom() && unassigned.top() > gaming.bottom(),
            "height {height}: output and unassigned apps must sit below the channel strips"
        );
        assert!((output.top() - unassigned.top()).abs() < 1.0);
        assert!((output.height() - unassigned.height()).abs() < 1.0);
        assert!(output.right() <= unassigned.left());
        assert!(unassigned.right() <= 1236.0 - 16.0);
        let mut right = gaming.right();
        for id in ["chat", "media", "aux"] {
            let rect = app.strip_rects[id];
            assert!(
                (rect.top() - gaming.top()).abs() < 1.0,
                "height {height} changed strip orientation"
            );
            assert!(
                (rect.width() - gaming.width()).abs() < 1.0,
                "unequal card widths: {id}"
            );
            assert!(
                (rect.height() - gaming.height()).abs() < 1.0,
                "unequal card heights: {id}"
            );
            assert!(rect.left() >= right, "overlapping strips: {id}");
            right = rect.right();
        }
        assert!(
            right <= 1236.0 - 16.0,
            "the last strip must fit, including its frame border: {right}"
        );
        assert!(app.editor.graph_rect.unwrap().height() >= 300.0);
        assert!(app.editor.graph_rect.unwrap().right() < 1236.0);
        assert!(
            app.percentage_rects
                .values()
                .all(|rect| rect.width() >= 72.0)
        );
        assert!(same_config(app.draft.as_ref().unwrap(), &original));
        assert!(
            app.volumes
                .values()
                .all(|edit| !edit.dirty && edit.request.is_none())
        );
    }
}

#[test]
fn output_panel_holds_its_device_selector_and_stacks_above_unassigned_apps_when_narrow() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    demo::stress(app.host.as_mut().unwrap());
    for (width, side_by_side) in [
        (1236.0, true),
        (1000.0, true),
        (800.0, false),
        (640.0, false),
    ] {
        for _ in 0..3 {
            frame(&mut app, &ctx, [width, 860.0], vec![]);
        }
        let output = app.strip_rects["output"];
        let unassigned = app.strip_rects["not-in-anada"];
        let device = app.header_rects["output-device"];
        assert!(
            output.contains_rect(device),
            "width {width}: the device selector must sit inside the output panel"
        );
        assert!(
            output.contains_rect(
                app.fader_rects[app
                    .host
                    .as_ref()
                    .unwrap()
                    .selected_output_id
                    .as_ref()
                    .unwrap()]
            )
        );
        if side_by_side {
            assert!(
                (output.top() - unassigned.top()).abs() < 1.0,
                "width {width}"
            );
            assert!(output.right() <= unassigned.left(), "width {width}");
        } else {
            assert!(output.bottom() <= unassigned.top(), "width {width}");
            assert!(
                (output.width() - unassigned.width()).abs() < 1.0,
                "width {width}"
            );
        }
        assert!(
            unassigned.right() <= width - 16.0 + 0.5,
            "width {width}: {output:?} {unassigned:?}"
        );
        for key in app.pill_rects.keys().filter(|k| k.contains("steam")) {
            assert!(
                unassigned.contains_rect(app.pill_rects[key]),
                "width {width}: {key} {:?} outside {unassigned:?}",
                app.pill_rects[key]
            );
        }
    }
}

fn click(app: &mut AnadaApp, ctx: &egui::Context, size: [f32; 2], point: egui::Pos2) {
    frame(
        app,
        ctx,
        size,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    frame(app, ctx, size, vec![button(point, false)]);
}

fn settle(
    app: &mut AnadaApp,
    ctx: &egui::Context,
    size: [f32; 2],
    done: impl Fn(&HostState) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        frame(app, ctx, size, vec![]);
        if app.host.as_ref().is_some_and(&done) && app.quiets.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "host state did not settle");
        std::thread::yield_now();
    }
}

fn volume(host: &HostState, id: &str) -> f32 {
    host.windows
        .outputs
        .iter()
        .find(|e| e.id == id)
        .unwrap()
        .volume
}

#[test]
fn quiet_toggles_to_the_configured_level_and_back_and_ends_on_output_switch() {
    let size = [1180.0, 900.0];
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    for _ in 0..3 {
        frame(&mut app, &ctx, size, vec![]);
    }
    let output = "demo-headphones";
    let before = volume(app.host.as_ref().unwrap(), output);
    assert!(before > 0.2);
    // On: lowered to the default 20% and the button shows the on state.
    let point = app.quiet_rects[output].center();
    click(&mut app, &ctx, size, point);
    settle(&mut app, &ctx, size, |h| h.quiet.contains_key(output));
    assert!((volume(app.host.as_ref().unwrap(), output) - 0.2).abs() < 0.0001);
    // Off: the earlier volume returns.
    click(&mut app, &ctx, size, point);
    settle(&mut app, &ctx, size, |h| !h.quiet.contains_key(output));
    assert!((volume(app.host.as_ref().unwrap(), output) - before).abs() < 0.0001);
    // On again, then switch the output: Quiet ends and the old device is restored.
    click(&mut app, &ctx, size, point);
    settle(&mut app, &ctx, size, |h| h.quiet.contains_key(output));
    app.submit(
        Command::SelectOutput {
            id: "demo-speakers".into(),
        },
        Tag::Other,
    );
    settle(&mut app, &ctx, size, |h| {
        h.selected_output_id.as_deref() == Some("demo-speakers")
    });
    let host = app.host.as_ref().unwrap();
    assert!(host.quiet.is_empty());
    assert!((volume(host, output) - before).abs() < 0.0001);
    // A channel: a manual fader level replaces Quiet without a later restore.
    let channel = "demo-gaming";
    let point = app.quiet_rects[channel].center();
    click(&mut app, &ctx, size, point);
    settle(&mut app, &ctx, size, |h| h.quiet.contains_key(channel));
    app.submit(
        Command::SetVolume {
            endpoint_id: channel.into(),
            volume: 0.33,
        },
        Tag::Other,
    );
    settle(&mut app, &ctx, size, |h| !h.quiet.contains_key(channel));
    assert!((volume(app.host.as_ref().unwrap(), channel) - 0.33).abs() < 0.0001);
}

#[test]
fn pills_size_to_their_names_and_flow_several_to_a_row() {
    let size = [1180.0, 900.0];
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    demo::stress(app.host.as_mut().unwrap());
    for _ in 0..3 {
        frame(&mut app, &ctx, size, vec![]);
    }
    let panel = app.strip_rects["not-in-anada"];
    let unassigned: Vec<_> = app
        .pill_rects
        .values()
        .filter(|rect| panel.contains_rect(**rect))
        .collect();
    assert!(unassigned.len() >= 4);
    assert!(app.pill_rects.values().all(|rect| rect.width() <= 240.0));
    let edge = app
        .pill_rects
        .iter()
        .find(|(key, _)| key.to_lowercase().contains("edge"))
        .unwrap()
        .1;
    assert!(
        edge.width() < 120.0,
        "a short name must give a short pill: {edge:?}"
    );
    let first_row = unassigned
        .iter()
        .filter(|rect| (rect.top() - edge.top()).abs() < 1.0)
        .count();
    assert!(first_row >= 3, "pills must share a row before wrapping");
    for (a, b) in unassigned.iter().zip(unassigned.iter().skip(1)) {
        assert!(!a.intersects(**b) || a == b, "pills overlap: {a:?} {b:?}");
    }
}

#[test]
fn eq_panel_ends_inside_the_window_when_everything_fits() {
    // The panel fills the remaining height. Its band controls are measured, not
    // estimated, so the page ends inside the window instead of always scrolling.
    for size in [[1200.0, 1390.0], [640.0, 1800.0]] {
        let ctx = egui::Context::default();
        // The real theme: its 28 px controls make the two band rows taller than
        // the old fixed 48 px estimate, which is what overflowed.
        theme(&ctx);
        let mut app = fixture(&ctx);
        for _ in 0..4 {
            frame(&mut app, &ctx, size, vec![]);
        }
        let plot = app.editor.graph_rect.unwrap();
        // 25 px below the plot is the graph frame; then the controls and the
        // panel's 12 px margin plus its 1 px border.
        let panel_bottom = plot.bottom() + 25.0 + app.editor.controls_height + 13.0;
        assert!(
            panel_bottom <= size[1],
            "EQ panel overflows at {size:?}: {panel_bottom}"
        );
        assert!(
            app.editor.controls_height >= 56.0,
            "both control rows count: {}",
            app.editor.controls_height
        );
        assert!(plot.height() >= 300.0);
    }
}

#[test]
fn header_and_strip_clicks_select_the_inline_eq_without_navigation_or_edits() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let size = [1236.0, 900.0];
    frame(&mut app, &ctx, size, vec![]);
    let original = app.draft.clone().unwrap();
    for id in ["chat", "media", "aux", "output", "gaming"] {
        let point = app.header_rects[id].center();
        frame(
            &mut app,
            &ctx,
            size,
            vec![egui::Event::PointerMoved(point), button(point, true)],
        );
        frame(&mut app, &ctx, size, vec![button(point, false)]);
        assert_eq!(app.selected, id);
        assert!(
            app.strip_rects.contains_key("gaming"),
            "selecting EQ must retain the mixer"
        );
        assert!(app.editor.graph_rect.is_some());
        assert!(same_config(app.draft.as_ref().unwrap(), &original));
    }
    let point = app.strip_rects["chat"].right_bottom() - egui::vec2(16.0, 16.0);
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    frame(&mut app, &ctx, size, vec![button(point, false)]);
    assert_eq!(app.selected, "chat");
}

#[test]
fn top_bar_sound_button_sits_beside_settings_at_every_width() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    for (width, settings) in [
        (1236.0, false),
        (640.0, false),
        (640.0, true),
        (1600.0, true),
    ] {
        app.settings = settings;
        frame(&mut app, &ctx, [width, 700.0], vec![]);
        let sound = app.top_bar_rects["sound"];
        let gear = app.top_bar_rects["settings"];
        assert!(
            sound.right() <= gear.left(),
            "width {width}: buttons overlap"
        );
        assert!((sound.center().y - gear.center().y).abs() < 1.0);
        assert!(
            gear.right() <= width - 16.0 + 0.5,
            "width {width}: Settings clipped"
        );
        assert!(
            sound.left() > 300.0,
            "width {width}: collides with the status text"
        );
    }
}

#[test]
fn scrolling_reveals_the_eq_without_growing_or_changing_it() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let size = [1236.0, 755.0];
    frame(&mut app, &ctx, size, vec![]);
    let original = app.draft.clone().unwrap();
    let before = app.editor.graph_rect.unwrap();
    let position = app.strip_rects["output"].right_center() - egui::vec2(20.0, 0.0);
    frame(
        &mut app,
        &ctx,
        size,
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                delta: egui::vec2(0.0, -400.0),
                modifiers: Default::default(),
            },
        ],
    );
    for _ in 0..30 {
        frame(&mut app, &ctx, size, vec![]);
    }
    let after = app.editor.graph_rect.unwrap();
    assert!(
        after.top() < before.top() - 50.0,
        "scroll must reveal EQ controls: {before:?} -> {after:?}"
    );
    assert!(
        (after.height() - before.height()).abs() < 1.0,
        "scroll must not resize the graph"
    );
    assert!(same_config(app.draft.as_ref().unwrap(), &original));
}

#[test]
fn hidden_channels_keep_equal_widths_and_the_eq_available() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    for visible in [4, 3, 2, 1, 0] {
        for (index, channel) in app.draft.as_mut().unwrap().channels.iter_mut().enumerate() {
            channel.visible = index < visible;
        }
        app.strip_rects.clear();
        frame(&mut app, &ctx, [1236.0, 900.0], vec![]);
        let output = app.strip_rects["output"];
        let visible: Vec<_> = app
            .draft
            .as_ref()
            .unwrap()
            .channels
            .iter()
            .filter(|c| c.visible)
            .map(|c| app.strip_rects[&c.id])
            .collect();
        for rect in &visible {
            assert!((rect.width() - visible[0].width()).abs() < 1.0);
            assert!((rect.top() - visible[0].top()).abs() < 1.0);
            assert!(rect.bottom() < output.top());
            assert!(rect.right() <= 1236.0 - 16.0);
        }
        assert!(app.editor.graph_rect.unwrap().height() >= 300.0);
        // Avoid the save worker replacing this intentionally edited fixture.
        app.dirty_since = None;
    }
}

#[test]
fn typed_percentages_commit_only_on_confirmation_for_channels_and_output_at_both_sizes() {
    for size in [[1180.0, 860.0], [1180.0, 540.0]] {
        for endpoint in ["demo-gaming", "demo-headphones"] {
            let ctx = egui::Context::default();
            let mut app = fixture(&ctx);
            frame(&mut app, &ctx, size, vec![]);
            // On short windows the output panel sits below the fold; scroll to it first.
            if app.percentage_rects[endpoint].bottom() > size[1] {
                let page = egui::pos2(size[0] / 2.0, 100.0);
                frame(
                    &mut app,
                    &ctx,
                    size,
                    vec![
                        egui::Event::PointerMoved(page),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            phase: egui::TouchPhase::Move,
                            source: egui::MouseWheelSource::Unknown,
                            delta: egui::vec2(0.0, -300.0),
                            modifiers: Default::default(),
                        },
                    ],
                );
                for _ in 0..30 {
                    frame(&mut app, &ctx, size, vec![]);
                }
            }
            let point = app.percentage_rects[endpoint].center();
            assert!(point.y < size[1], "{endpoint} field off-screen at {size:?}");
            frame(
                &mut app,
                &ctx,
                size,
                vec![egui::Event::PointerMoved(point), button(point, true)],
            );
            frame(&mut app, &ctx, size, vec![button(point, false)]);
            frame(&mut app, &ctx, size, vec![egui::Event::Text("37.5".into())]);
            assert!(
                !app.volumes
                    .get(endpoint)
                    .is_some_and(|edit| edit.dirty || edit.request.is_some()),
                "incomplete text must not set Windows volume"
            );
            frame(&mut app, &ctx, size, vec![key(egui::Key::Enter)]);
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                frame(&mut app, &ctx, size, vec![]);
                if app
                    .host
                    .as_ref()
                    .unwrap()
                    .windows
                    .outputs
                    .iter()
                    .any(|e| e.id == endpoint && (e.volume - 0.375).abs() < 0.0001)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "typed volume did not reach {endpoint} at {size:?}"
                );
                std::thread::yield_now();
            }
        }
    }
}

#[test]
fn percentage_escape_invalid_text_tab_and_clamping() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let size = [640.0, 700.0];
    frame(&mut app, &ctx, size, vec![]);
    for (text, finish, expected) in [
        ("75", egui::Key::Escape, 0.5),
        ("NaN", egui::Key::Enter, 0.5),
        ("invalid", egui::Key::Tab, 0.5),
        ("125", egui::Key::Tab, 1.0),
        ("-25", egui::Key::Enter, 0.0),
    ] {
        let point = app.percentage_rects["demo-gaming"].center();
        frame(
            &mut app,
            &ctx,
            size,
            vec![egui::Event::PointerMoved(point), button(point, true)],
        );
        frame(&mut app, &ctx, size, vec![button(point, false)]);
        frame(&mut app, &ctx, size, vec![egui::Event::Text(text.into())]);
        frame(&mut app, &ctx, size, vec![key(finish)]);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            frame(&mut app, &ctx, size, vec![]);
            if !app
                .volumes
                .get("demo-gaming")
                .is_some_and(|edit| edit.dirty || edit.request.is_some())
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let volume = app
            .host
            .as_ref()
            .unwrap()
            .windows
            .outputs
            .iter()
            .find(|e| e.id == "demo-gaming")
            .unwrap()
            .volume;
        assert!(
            (volume - expected).abs() < 0.0001,
            "text {text} with {finish:?}: {volume}"
        );
    }
}

#[test]
fn percentage_focus_does_not_round_or_restore_external_windows_volume() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.host
        .as_mut()
        .unwrap()
        .windows
        .outputs
        .iter_mut()
        .find(|e| e.id == "demo-gaming")
        .unwrap()
        .volume = 0.333333;
    let size = [1180.0, 860.0];
    frame(&mut app, &ctx, size, vec![]);
    let point = app.percentage_rects["demo-gaming"].center();
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    frame(&mut app, &ctx, size, vec![button(point, false)]);
    // A Windows change while the field has focus remains authoritative unless
    // the owner actually types a new value.
    app.host
        .as_mut()
        .unwrap()
        .windows
        .outputs
        .iter_mut()
        .find(|e| e.id == "demo-gaming")
        .unwrap()
        .volume = 0.456789;
    frame(&mut app, &ctx, size, vec![key(egui::Key::Tab)]);
    assert!(
        !app.volumes
            .get("demo-gaming")
            .is_some_and(|edit| edit.dirty || edit.request.is_some())
    );
    assert_eq!(
        app.host
            .as_ref()
            .unwrap()
            .windows
            .outputs
            .iter()
            .find(|e| e.id == "demo-gaming")
            .unwrap()
            .volume,
        0.456789
    );
}
#[test]
fn physical_fader_same_frame_gesture_sends_scalar_once_and_idle_meters_do_not_write() {
    use std::sync::{Arc, Mutex};
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let writes = Arc::new(Mutex::new(Vec::new()));
    let recorded = writes.clone();
    let mut state = app.host.clone().unwrap();
    app.client = Client::with_backend(ctx.clone(), Duration::from_secs(30), move |command| {
        if let Some(command) = command {
            if !matches!(command, Command::Snapshot | Command::SnapshotMeters) {
                recorded.lock().unwrap().push(command.clone());
            }
            demo::command(&mut state, command)?;
        }
        Ok(state.clone())
    });
    // Let the page scrollbar reserve its width first; it narrows the horizontal fader.
    for _ in 0..4 {
        frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
    }
    // The output fader is horizontal: its track runs from 8 px to 72 px inside the rect.
    let rect = app.fader_rects["demo-headphones"];
    let left = rect.left() + 8.0;
    let right = rect.right() - 72.0;
    let y = rect.top() + 13.0;
    let start = egui::pos2(left + (right - left) * 0.5, y);
    let target = egui::pos2(left + (right - left) * 0.75, y);
    frame(
        &mut app,
        &ctx,
        [1180.0, 860.0],
        vec![
            egui::Event::PointerMoved(start),
            button(start, true),
            egui::Event::PointerMoved(target),
            button(target, false),
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
        if app
            .host
            .as_ref()
            .unwrap()
            .windows
            .outputs
            .iter()
            .any(|endpoint| {
                endpoint.id == "demo-headphones" && (endpoint.volume - 0.75).abs() < 0.0001
            })
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "same-frame physical fader did not reach the Windows scalar backend"
        );
        std::thread::yield_now();
    }
    // Holding the stationary fader and changing meter samples must not repeat
    // the acknowledged Windows write, including after the throttle interval.
    frame(
        &mut app,
        &ctx,
        [1180.0, 860.0],
        vec![egui::Event::PointerMoved(target), button(target, true)],
    );
    for _ in 0..25 {
        frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
    frame(&mut app, &ctx, [1180.0, 860.0], vec![button(target, false)]);
    for _ in 0..5 {
        frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
    }
    assert!(app.meter_received.is_some());
    let writes = writes.lock().unwrap();
    assert_eq!(
        writes.len(),
        1,
        "meter/idle/held frames generated extra writes: {writes:?}"
    );
    assert!(
        matches!(&writes[0], Command::SetVolume { endpoint_id, volume } if endpoint_id == "demo-headphones" && (*volume - 0.75).abs() < 0.0001)
    );
}

#[test]
fn meter_reply_cannot_replace_configuration_revision_or_pending_edits() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let original_host = app.host.clone().unwrap();
    let mut meter_state = original_host.clone();
    meter_state.revision = 999;
    meter_state.config.channels[0].name = "Unrelated meter payload config".into();
    meter_state.audio_diagnostics.meters = crate::model::AudioMeters {
        channels: vec![crate::model::ChannelMeter {
            channel_id: "gaming".into(),
            peak: [0.25, 0.5],
            spectrum: Default::default(),
        }],
        output_peak: [0.6, 0.7],
        running: true,
        output_spectrum: Default::default(),
    };
    app.client = Client::with_backend(ctx, Duration::from_secs(30), move |command| {
        assert!(
            matches!(command, Some(Command::SnapshotMeters)),
            "isolated meter test must not request inventory or writes"
        );
        Ok(meter_state.clone())
    });
    app.draft.as_mut().unwrap().channels[0].name = "My pending local edit".into();
    app.changed();
    app.submitted = Some(1234);
    app.volumes.insert(
        "demo-headphones".into(),
        VolumeEdit {
            value: 75.0,
            db: false,
            request: Some(5678),
            sent: Some(75.0),
            last_sent: Instant::now(),
            dirty: true,
        },
    );
    app.mutes.insert("demo-gaming".into(), (true, 9876));
    let base_revision = app.base_revision;
    app.client.set_metering(true);
    let deadline = Instant::now() + Duration::from_secs(2);
    while app.meter_received.is_none() {
        app.poll();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(app.meters.output_peak, [0.6, 0.7]);
    assert_eq!(app.meters.channels[0].peak, [0.25, 0.5]);
    assert_eq!(app.base_revision, base_revision);
    assert_eq!(app.host.as_ref().unwrap().revision, original_host.revision);
    assert!(same_config(
        &app.host.as_ref().unwrap().config,
        &original_host.config
    ));
    assert_eq!(
        app.draft.as_ref().unwrap().channels[0].name,
        "My pending local edit"
    );
    assert!(app.dirty_since.is_some());
    assert_eq!(app.submitted, Some(1234));
    assert!(app.volumes["demo-headphones"].dirty);
    assert_eq!(app.volumes["demo-headphones"].request, Some(5678));
    assert_eq!(app.mutes["demo-gaming"], (true, 9876));
    assert!(!app.conflict);
}
fn close_frame(
    app: &mut AnadaApp,
    ctx: &egui::Context,
    requested: bool,
) -> Vec<egui::ViewportCommand> {
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1180.0, 860.0),
        )),
        ..Default::default()
    };
    if requested {
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
    }
    let mut output = ctx.run_ui(raw, |ui| app.show(ui));
    output.textures_delta.clear();
    output
        .viewport_output
        .remove(&egui::ViewportId::ROOT)
        .map(|viewport| viewport.commands)
        .unwrap_or_default()
}

#[test]
fn viewport_close_saves_debounced_draft_and_waits_for_its_acknowledgement() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let mut state = app.host.clone().unwrap();
    let (entered, received) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    app.client = Client::with_backend(ctx.clone(), Duration::from_secs(30), move |command| {
        if let Some(Command::SaveConfig {
            config,
            expected_revision,
        }) = command
        {
            entered.send((config.clone(), expected_revision)).unwrap();
            blocked.recv().unwrap();
            assert_eq!(expected_revision, state.revision);
            state.config = config;
            state.revision += 1;
        }
        Ok(state.clone())
    });
    app.draft.as_mut().unwrap().channels[0].name = "Last edit before close".into();
    app.changed();
    assert!(app.dirty_since.unwrap().elapsed() < Duration::from_millis(150));
    let commands = close_frame(&mut app, &ctx, true);
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Close))
    );
    // Keep the UI consuming its bounded result slot. A meter completion may
    // precede Save, just as it can during an ordinary interactive frame.
    let deadline = Instant::now() + Duration::from_secs(2);
    let (saved, _) = loop {
        if let Ok(saved) = received.try_recv() {
            break saved;
        }
        let commands = close_frame(&mut app, &ctx, false);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Close)),
            "close must wait while Save is blocked"
        );
        assert!(
            Instant::now() < deadline,
            "Save was not dispatched while closing"
        );
        std::thread::yield_now();
    };
    assert_eq!(saved.channels[0].name, "Last edit before close");
    assert!(app.submitted.is_some());
    assert!(app.closing);
    let commands = close_frame(&mut app, &ctx, false);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Close))
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let commands = close_frame(&mut app, &ctx, false);
        if commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Close))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "close was not resumed after Save acknowledgement"
        );
        std::thread::yield_now();
    }
    assert!(app.submitted.is_none());
    assert!(app.dirty_since.is_none());
    assert_eq!(
        app.host.as_ref().unwrap().config.channels[0].name,
        "Last edit before close"
    );
}

#[test]
fn failed_older_volume_write_keeps_newer_unsent_value() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let (entered, received) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    app.client = Client::with_backend(ctx, Duration::from_secs(30), move |_| {
        entered.send(()).unwrap();
        blocked.recv().unwrap();
        Err("test endpoint temporarily unavailable".into())
    });
    let id = app
        .client
        .submit(
            Command::SetVolume {
                endpoint_id: "demo-gaming".into(),
                volume: 0.2,
            },
            Tag::Other,
        )
        .unwrap();
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    app.volumes.insert(
        "demo-gaming".into(),
        VolumeEdit {
            value: 80.0,
            db: false,
            request: Some(id),
            sent: Some(20.0),
            last_sent: Instant::now(),
            dirty: true,
        },
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        app.poll();
        if app.notice.is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let edit = &app.volumes["demo-gaming"];
    assert!(edit.dirty);
    assert_eq!(edit.value, 80.0);
    assert!(edit.request.is_none());
}
#[test]
fn complete_mixer_idle_never_creates_volume_or_config_writes() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let original = app.draft.clone().unwrap();
    for _ in 0..8 {
        frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(same_config(app.draft.as_ref().unwrap(), &original));
    assert!(app.dirty_since.is_none());
    assert!(app.submitted.is_none());
    assert!(
        app.volumes
            .values()
            .all(|edit| !edit.dirty && edit.sent.is_none())
    );
    frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
    assert!(
        app.editor.graph_rect.unwrap().right() <= 1180.0,
        "EQ graph must fit the window"
    );
}

#[test]
fn inline_equalizer_always_exists_and_display_changes_do_not_edit_audio() {
    for size in [
        [1440.0, 900.0],
        [1236.0, 1180.0],
        [1236.0, 1057.0],
        [1236.0, 755.0],
        [1180.0, 540.0],
        [640.0, 700.0],
    ] {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        let before = app.draft.clone().unwrap();
        for (selected, enabled) in [
            ("gaming", true),
            ("gaming", false),
            ("output", true),
            ("output", false),
            ("gaming", true),
        ] {
            app.selected = selected.into();
            app.eq_spectrum = enabled;
            frame(&mut app, &ctx, size, vec![]);
            let plot = app.editor.graph_rect.unwrap();
            assert!(
                plot.height() >= 300.0,
                "plot at {size:?} was {}",
                plot.height()
            );
            assert!(
                plot.right() <= size[0],
                "EQ must fit the viewport width, independently of scrolling strips: {size:?}: {plot:?}"
            );
            assert!(same_config(app.draft.as_ref().unwrap(), &before));
            assert!(app.dirty_since.is_none());
        }
    }
}

#[test]
fn graph_range_pointer_choice_changes_only_saved_view_and_adds_no_toolbar_row() {
    for size in [[1180.0, 860.0], [640.0, 700.0]] {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        app.draft.as_mut().unwrap().channels[0].eq.bands[4].gain_db = 24.0;
        // Let the existing page scrollbar finish reserving its width before
        // comparing toolbar and graph bounds across the range interaction.
        for _ in 0..4 {
            frame(&mut app, &ctx, size, vec![]);
        }
        // On short windows the EQ toolbar starts below the fold; scroll it into view.
        let page = egui::pos2(size[0] / 2.0, 120.0);
        frame(
            &mut app,
            &ctx,
            size,
            vec![
                egui::Event::PointerMoved(page),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    delta: egui::vec2(0.0, -300.0),
                    modifiers: Default::default(),
                },
            ],
        );
        for _ in 0..30 {
            frame(&mut app, &ctx, size, vec![]);
        }
        let original = app.draft.clone().unwrap();
        let plot = app.editor.graph_rect.unwrap();
        let control = app.eq_scale_rects["control"];
        assert!(control.right() <= size[0]);
        assert!(
            control.bottom() <= size[1],
            "range control is off-screen: {control:?}"
        );
        let point = control.center();
        frame(
            &mut app,
            &ctx,
            size,
            vec![egui::Event::PointerMoved(point), button(point, true)],
        );
        frame(&mut app, &ctx, size, vec![button(point, false)]);
        frame(&mut app, &ctx, size, vec![]);
        let option = app.eq_scale_rects["12"].center();
        frame(
            &mut app,
            &ctx,
            size,
            vec![egui::Event::PointerMoved(option), button(option, true)],
        );
        frame(&mut app, &ctx, size, vec![button(option, false)]);
        let actual = app.draft.as_ref().unwrap();
        assert_eq!(actual.eq_graph_range_db, 12);
        let mut expected = original;
        expected.eq_graph_range_db = 12;
        assert!(
            same_config(actual, &expected),
            "Changing the view scale edited audio settings"
        );
        assert_eq!(
            app.selected, "gaming",
            "A popup choice selected the strip underneath it"
        );
        let actual_plot = app.editor.graph_rect.unwrap();
        let actual_control = app.eq_scale_rects["control"];
        // Scroll-to-focus can move the whole page. Its toolbar-to-graph gap
        // and graph height must remain stable rather than adding another row.
        assert!(
            ((actual_plot.top() - actual_control.bottom()) - (plot.top() - control.bottom())).abs()
                < 1.0
        );
        assert!((actual_plot.height() - plot.height()).abs() < 1.0);
        assert!(app.eq_scale_rects["control"].right() <= size[0]);
    }
}
#[test]
fn repeated_sessions_render_one_draggable_pill_and_route_all_processes() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
    let chrome = app
        .pill_rects
        .keys()
        .find(|key| key.to_lowercase().contains("chrome"))
        .unwrap()
        .clone();
    assert_eq!(
        app.pill_rects
            .keys()
            .filter(|key| key.to_lowercase().contains("chrome"))
            .count(),
        1
    );
    let start = app.pill_rects[&chrome].center();
    let target = app.strip_rects["chat"];
    let end = egui::pos2(target.center().x, target.bottom() - 14.0);
    frame(
        &mut app,
        &ctx,
        [1180.0, 860.0],
        vec![egui::Event::PointerMoved(start), button(start, true)],
    );
    frame(
        &mut app,
        &ctx,
        [1180.0, 860.0],
        vec![egui::Event::PointerMoved(start + egui::vec2(8.0, 0.0))],
    );
    frame(
        &mut app,
        &ctx,
        [1180.0, 860.0],
        vec![egui::Event::PointerMoved(end)],
    );
    frame(&mut app, &ctx, [1180.0, 860.0], vec![button(end, false)]);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
        if app
            .host
            .as_ref()
            .unwrap()
            .windows
            .sessions
            .iter()
            .filter(|s| s.process_id == 42 || s.process_id == 43)
            .all(|s| s.endpoint_id == "demo-chat")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Actual drag did not route both Chrome processes"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn standard_binding_failure_is_atomic_and_reordering_keeps_identity() {
    let mut state = demo::state();
    for c in &mut state.config.channels {
        c.endpoint_id = None;
        c.capture_id = None;
    }
    state.config.channels.reverse();
    let mut absent = state.windows.clone();
    absent.inputs.pop();
    let before = state.config.clone();
    assert!(bind_standard_vac(&mut state.config, &absent).is_err());
    assert!(same_config(&state.config, &before));
    bind_standard_vac(&mut state.config, &state.windows).unwrap();
    assert_eq!(
        state
            .config
            .channels
            .iter()
            .find(|c| c.id == "gaming")
            .unwrap()
            .endpoint_id
            .as_deref(),
        Some("demo-gaming")
    );
}

#[test]
fn narrow_columns_drag_press_move_release_in_one_frame_routes_both_directions() {
    for (source, target) in [("gaming", "chat"), ("chat", "gaming")] {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        let host = app.host.as_mut().unwrap();
        let endpoint = host
            .config
            .channels
            .iter()
            .find(|channel| channel.id == source)
            .unwrap()
            .endpoint_id
            .clone()
            .unwrap();
        for session in &mut host.windows.sessions {
            if session
                .executable_path
                .as_ref()
                .is_some_and(|path| path.to_lowercase().contains("chrome"))
            {
                session.endpoint_id = endpoint.clone();
            }
        }
        frame(&mut app, &ctx, [640.0, 700.0], vec![]);
        let key = app
            .pill_rects
            .keys()
            .find(|key| key.to_lowercase().contains("chrome"))
            .unwrap()
            .clone();
        let start = app.pill_rects[&key].center();
        let end = app.strip_rects[target].center();
        frame(
            &mut app,
            &ctx,
            [640.0, 700.0],
            vec![
                egui::Event::PointerMoved(start),
                button(start, true),
                egui::Event::PointerMoved(end),
                button(end, false),
            ],
        );
        assert_eq!(
            app.routes.get(&key).map(|route| route.target.as_str()),
            Some(target),
            "one-frame {source} to {target}"
        );
    }
}

fn priority_fixture(ctx: &egui::Context) -> AnadaApp {
    let mut app = fixture(ctx);
    demo::priority_stress(app.host.as_mut().unwrap());
    app.draft = Some(app.host.as_ref().unwrap().config.clone());
    app.settings = true;
    app.settings_tab = 1;
    app
}
fn priority_rect(ctx: &egui::Context, kind: &str, id: &str) -> egui::Rect {
    ctx.data(|data| data.get_temp(egui::Id::unique((kind, id))))
        .unwrap()
}
#[test]
fn output_priority_pointer_drags_insert_both_directions_and_preserve_offline_entries() {
    let ctx = egui::Context::default();
    let mut app = priority_fixture(&ctx);
    let size = [1180.0, 860.0];
    frame(&mut app, &ctx, size, vec![]);
    frame(&mut app, &ctx, size, vec![]);
    let mut action_x: Option<f32> = None;
    for index in 0..6 {
        let id = format!("priority-{index}");
        let point: egui::Pos2 = ctx
            .data(|data| data.get_temp(egui::Id::unique(("output-actions", &id))))
            .unwrap();
        if let Some(x) = action_x {
            assert!(
                (point.x - x).abs() < 1.0,
                "device actions must align regardless of name length"
            );
        } else {
            action_x = Some(point.x);
        }
    }
    let original = app.draft.as_ref().unwrap().output_priorities.clone();
    let from = priority_rect(&ctx, "output-handle", "priority-5").center();
    let target = priority_rect(&ctx, "output-row", "priority-0");
    let to = egui::pos2(from.x, target.top() + 2.0);
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(from), button(from, true)],
    );
    frame(&mut app, &ctx, size, vec![egui::Event::PointerMoved(to)]);
    assert_eq!(
        app.draft.as_ref().unwrap().output_priorities,
        original,
        "hover must not save a reorder"
    );
    frame(&mut app, &ctx, size, vec![button(to, false)]);
    let mut expected = original.clone();
    expected.retain(|id| id != "priority-5");
    expected.insert(0, "priority-5".into());
    assert_eq!(app.draft.as_ref().unwrap().output_priorities, expected);
    assert!(
        app.dirty_since.is_some(),
        "drop must use the normal autosave path"
    );
    frame(&mut app, &ctx, size, vec![]);
    let from = priority_rect(&ctx, "output-row", "priority-5").center();
    let target = priority_rect(&ctx, "output-row", "priority-4");
    let to = egui::pos2(from.x, target.bottom() - 2.0);
    // Press, motion and release in one frame also works (fast pointer input).
    frame(
        &mut app,
        &ctx,
        size,
        vec![
            egui::Event::PointerMoved(from),
            button(from, true),
            egui::Event::PointerMoved(to),
            button(to, false),
        ],
    );
    let mut expected = original.clone();
    expected.retain(|id| id != "priority-5");
    let index = expected.iter().position(|id| id == "priority-4").unwrap() + 1;
    expected.insert(index, "priority-5".into());
    assert_eq!(app.draft.as_ref().unwrap().output_priorities, expected);
    assert_eq!(
        app.draft.as_ref().unwrap().output_names,
        app.host.as_ref().unwrap().config.output_names
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        frame(&mut app, &ctx, size, vec![]);
        if app.dirty_since.is_none() && app.submitted.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "reorder did not autosave: {:?}",
            app.save_error
        );
        std::thread::yield_now();
    }
    assert_eq!(
        app.host.as_ref().unwrap().config.output_priorities,
        expected
    );
}
#[test]
fn output_priority_cancel_and_keyboard_move_to_adjacent_offline_entry() {
    let ctx = egui::Context::default();
    let mut app = priority_fixture(&ctx);
    let size = [640.0, 700.0];
    frame(&mut app, &ctx, size, vec![]);
    frame(&mut app, &ctx, size, vec![]);
    let original = app.draft.as_ref().unwrap().output_priorities.clone();
    let from = priority_rect(&ctx, "output-handle", "priority-2").center();
    let to = priority_rect(&ctx, "output-handle", "priority-0").center();
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(from), button(from, true)],
    );
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(to), key(egui::Key::Escape)],
    );
    frame(&mut app, &ctx, size, vec![button(to, false)]);
    assert_eq!(app.draft.as_ref().unwrap().output_priorities, original);
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(from), button(from, true)],
    );
    frame(&mut app, &ctx, size, vec![button(from, false)]);
    let mut up = key(egui::Key::ArrowUp);
    if let egui::Event::Key { modifiers, .. } = &mut up {
        modifiers.alt = true;
    }
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(size[0], size[1]),
        )),
        events: vec![
            egui::Event::ModifiersChanged(egui::Modifiers::ALT),
            up,
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
        ],
        ..Default::default()
    };
    let _ = ctx.run_ui(raw, |ui| app.show(ui));
    let result = &app.draft.as_ref().unwrap().output_priorities;
    assert!(
        result.iter().position(|id| id == "priority-2")
            < result.iter().position(|id| id == "offline-1"),
        "keyboard should move to the preceding visible offline entry"
    );
    assert_eq!(
        result
            .iter()
            .filter(|id| id.starts_with("offline"))
            .collect::<Vec<_>>(),
        original
            .iter()
            .filter(|id| id.starts_with("offline"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn output_priority_keeps_offline_rows_in_place_and_unlisted_devices_collapsed() {
    let ctx = egui::Context::default();
    let mut app = priority_fixture(&ctx);
    let before = app.draft.clone().unwrap();
    frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
    frame(&mut app, &ctx, [1180.0, 860.0], vec![]);
    let first = priority_rect(&ctx, "output-row", "priority-0");
    let offline = priority_rect(&ctx, "output-row", "offline-0");
    let next = priority_rect(&ctx, "output-row", "priority-1");
    let connected_action = priority_rect(&ctx, "output-membership", "priority-0");
    let offline_action = priority_rect(&ctx, "output-membership", "offline-0");
    assert!(
        (connected_action.right() - offline_action.right()).abs() < 1.0,
        "offline remove controls must align with connected rows"
    );
    assert!(
        first.top() < offline.top() && offline.top() < next.top(),
        "offline rows must remain between their saved neighbours"
    );
    assert!(
        ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("unlisted-outputs-visible")))
            .is_none(),
        "unlisted outputs must start collapsed"
    );
    assert!(
        same_config(&before, app.draft.as_ref().unwrap()),
        "rendering must not add or remove priorities"
    );
}

#[test]
fn output_priority_remove_and_add_move_device_between_the_two_groups() {
    let ctx = egui::Context::default();
    let mut app = priority_fixture(&ctx);
    let size = [1180.0, 860.0];
    app.draft.as_mut().unwrap().output_priorities =
        vec!["priority-0".into(), "offline-0".into(), "priority-1".into()];
    ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
    ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.0);
    frame(&mut app, &ctx, size, vec![]);
    frame(&mut app, &ctx, size, vec![]);
    let point = priority_rect(&ctx, "output-membership", "priority-0").center();
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    frame(&mut app, &ctx, size, vec![button(point, false)]);
    assert_eq!(
        app.draft.as_ref().unwrap().output_priorities,
        ["offline-0", "priority-1"]
    );
    frame(&mut app, &ctx, size, vec![]);
    let header: egui::Rect = ctx
        .data(|data| data.get_temp(egui::Id::unique("unlisted-output-header")))
        .unwrap();
    let point = header.center();
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    frame(&mut app, &ctx, size, vec![button(point, false)]);
    frame(&mut app, &ctx, size, vec![]);
    let point = priority_rect(&ctx, "output-membership", "priority-0").center();
    frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    frame(&mut app, &ctx, size, vec![button(point, false)]);
    assert_eq!(
        app.draft.as_ref().unwrap().output_priorities,
        ["offline-0", "priority-1", "priority-0"]
    );
}
