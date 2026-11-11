//! Permission-filtered MCP tools. Host responses are projected, never passed through.
use crate::{
    mcp_types::Permission,
    model::{Command, HostState},
    presets::{EqPreset, PresetStore},
};
use anada_dsp::EqSettings;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub trait Backend {
    fn request(&mut self, command: Command) -> Result<HostState, String>;
    fn presets(&mut self) -> Result<Vec<EqPreset>, String>;
    fn save_preset(&mut self, name: &str, eq: &EqSettings, replace: bool) -> Result<(), String>;
    fn delete_preset(&mut self, name: &str) -> Result<(), String>;
    fn startup(&mut self, enabled: Option<bool>) -> Result<bool, String>;
}
struct Local;
impl Backend for Local {
    fn request(&mut self, command: Command) -> Result<HostState, String> {
        crate::transport::request(command)
    }
    fn presets(&mut self) -> Result<Vec<EqPreset>, String> {
        PresetStore::default_store()?.list()
    }
    fn save_preset(&mut self, n: &str, e: &EqSettings, r: bool) -> Result<(), String> {
        PresetStore::default_store()?.save(n, e, r).map(|_| ())
    }
    fn delete_preset(&mut self, n: &str) -> Result<(), String> {
        PresetStore::default_store()?.remove(n).map(|_| ())
    }
    fn startup(&mut self, e: Option<bool>) -> Result<bool, String> {
        match e {
            Some(e) => crate::startup::set_enabled(e),
            None => crate::startup::enabled(),
        }
    }
}
// Each entry's fields are also the complete argument allowlist.
fn definitions() -> Vec<(&'static str, Permission, &'static str, Value)> {
    use Permission::*;
    let string = json!({"type":"string","minLength":1,"maxLength":2048});
    let boolean = json!({"type":"boolean"});
    let target = json!({"type":"object","additionalProperties":false,"properties":{"channel_id":string,"output_id":string},"oneOf":[{"required":["channel_id"]},{"required":["output_id"]}]});
    let eq = json!({"type":"object","additionalProperties":false,"required":["bypass","preamp_db","bands"],"properties":{"bypass":boolean,"preamp_db":{"type":"number","minimum":-24,"maximum":12},"bands":{"type":"array","maxItems":10,"items":{"type":"object","additionalProperties":false,"required":["enabled","kind","frequency_hz","gain_db","q"],"properties":{"enabled":boolean,"kind":{"enum":["Bell","LowShelf","HighShelf"]},"frequency_hz":{"type":"number","minimum":20,"maximum":20000},"gain_db":{"type":"number","minimum":-24,"maximum":24},"q":{"type":"number","minimum":0.1,"maximum":20}}}}}});
    let mut band_patch = eq["properties"]["bands"]["items"].clone();
    band_patch.as_object_mut().unwrap().remove("required");
    band_patch["minProperties"] = json!(1);
    vec![
        (
            "read_status",
            ReadStatus,
            "Read audio running state and diagnostics; does not start audio.",
            json!({}),
        ),
        (
            "read_devices",
            ReadDevices,
            "List endpoint IDs and names. Levels require read_levels.",
            json!({}),
        ),
        (
            "read_applications",
            ReadApplications,
            "List running audio applications and endpoint assignments. Executable paths are omitted.",
            json!({}),
        ),
        (
            "read_eq",
            ReadEq,
            "Read EQ for an explicit channel_id or physical output_id.",
            json!({"target":target}),
        ),
        (
            "read_levels",
            ReadLevels,
            "Read Windows endpoint scalar, actual dB range and mute, plus digital sample peak meters (not true peak).",
            json!({}),
        ),
        (
            "read_spectrum",
            ReadSpectrum,
            "Read post-EQ channel and post-clamp output spectra in dBFS, before Windows volume.",
            json!({}),
        ),
        (
            "read_presets",
            ReadPresets,
            "Read named local EQ presets.",
            json!({}),
        ),
        (
            "read_channel_settings",
            ReadChannelSettings,
            "Read channel identity, name, visibility and endpoint binding; excludes EQ and levels.",
            json!({}),
        ),
        (
            "read_output_settings",
            ReadOutputSettings,
            "Read output priorities and selected output; excludes EQ and levels.",
            json!({}),
        ),
        (
            "set_channel_eq",
            WriteChannelEq,
            "Replace a channel EQ. target must contain channel_id only. Does not change bindings or levels.",
            json!({"target":target,"eq":eq}),
        ),
        (
            "set_output_eq",
            WriteOutputEq,
            "Replace physical output EQ. target must contain explicit output_id only.",
            json!({"target":target,"eq":eq}),
        ),
        (
            "patch_channel_eq",
            WriteChannelEq,
            "Edit channel EQ preamp, bypass, or one band; band_index is zero-based. Unspecified values stay unchanged.",
            json!({"target":target,"preamp_db":{"type":"number","minimum":-24,"maximum":12},"bypass":boolean,"band_index":{"type":"integer","minimum":0,"maximum":9},"band":band_patch}),
        ),
        (
            "patch_output_eq",
            WriteOutputEq,
            "Edit explicit physical output EQ preamp, bypass, or one band; band_index is zero-based.",
            json!({"target":target,"preamp_db":{"type":"number","minimum":-24,"maximum":12},"bypass":boolean,"band_index":{"type":"integer","minimum":0,"maximum":9},"band":band_patch}),
        ),
        (
            "reset_channel_eq",
            WriteChannelEq,
            "Reset channel EQ to factory flat settings.",
            json!({"target":target}),
        ),
        (
            "reset_output_eq",
            WriteOutputEq,
            "Reset explicit physical output EQ to factory flat settings.",
            json!({"target":target}),
        ),
        (
            "save_preset",
            SavePresets,
            "Save supplied complete EQ as a named local preset. Existing names require replace=true.",
            json!({"name":string,"eq":eq,"replace":boolean}),
        ),
        (
            "delete_preset",
            DeletePresets,
            "Delete exactly one named local EQ preset.",
            json!({"name":string}),
        ),
        (
            "recall_channel_preset",
            WriteChannelEq,
            "Apply named preset to channel EQ; also requires read_presets.",
            json!({"target":target,"name":string}),
        ),
        (
            "recall_output_preset",
            WriteOutputEq,
            "Apply named preset to explicit output EQ; also requires read_presets.",
            json!({"target":target,"name":string}),
        ),
        (
            "set_channel_volume",
            ChannelVolume,
            "Set Windows volume on bound channel endpoint; provide exactly one scalar (0..1) or level_db within device range.",
            json!({"channel_id":string,"scalar":{"type":"number","minimum":0,"maximum":1},"level_db":{"type":"number"}}),
        ),
        (
            "set_output_volume",
            OutputVolume,
            "Set Windows volume on explicit physical output; provide exactly one scalar (0..1) or level_db within device range.",
            json!({"output_id":string,"scalar":{"type":"number","minimum":0,"maximum":1},"level_db":{"type":"number"}}),
        ),
        (
            "set_mute",
            Mute,
            "Set Windows mute on explicit channel or physical output.",
            json!({"target":target,"muted":boolean}),
        ),
        (
            "assign_applications",
            RouteApplications,
            "Assign nonzero process IDs to a persistent channel using Windows routing preferences.",
            json!({"channel_id":string,"process_ids":{"type":"array","minItems":1,"maxItems":256,"uniqueItems":true,"items":{"type":"integer","minimum":1,"maximum":4294967295u64}}}),
        ),
        (
            "select_output",
            SelectOutput,
            "Select a connected physical output by exact endpoint ID.",
            json!({"output_id":string}),
        ),
        (
            "set_output_priorities",
            OutputPriorities,
            "Replace ordered physical-output priorities; retains all other settings.",
            json!({"output_ids":{"type":"array","maxItems":64,"uniqueItems":true,"items":string}}),
        ),
        (
            "rename_channel",
            ChannelNames,
            "Rename a persistent channel without changing identity or binding.",
            json!({"channel_id":string,"name":{"type":"string","minLength":1,"maxLength":48}}),
        ),
        (
            "set_channel_visibility",
            ChannelVisibility,
            "Show or hide a channel strip; audio identity and binding stay unchanged.",
            json!({"channel_id":string,"visible":boolean}),
        ),
        (
            "set_channel_binding",
            ChannelBindings,
            "Change a channel playback/capture pair. Both IDs must be supplied (or null to clear both).",
            json!({"channel_id":string,"endpoint_id":{"type":["string","null"]},"capture_id":{"type":["string","null"]}}),
        ),
        (
            "repair_windows_defaults",
            RepairWindowsDefaults,
            "Explicitly restore Anada's Windows default playback assignments.",
            json!({}),
        ),
        (
            "read_startup",
            Startup,
            "Read sign-in startup registration without changing it.",
            json!({}),
        ),
        (
            "set_startup",
            Startup,
            "Explicitly enable or disable Anada sign-in startup; does not start audio now.",
            json!({"enabled":boolean}),
        ),
        (
            "stop_host",
            StopHost,
            "Stop the running Anada audio host. Does not start it.",
            json!({}),
        ),
    ]
}
fn required(name: &str) -> Vec<&'static str> {
    match name {
        "read_eq" | "reset_channel_eq" | "reset_output_eq" => vec!["target"],
        "set_channel_eq" | "set_output_eq" => vec!["target", "eq"],
        "patch_channel_eq" | "patch_output_eq" => vec!["target"],
        "save_preset" => vec!["name", "eq"],
        "delete_preset" => vec!["name"],
        "recall_channel_preset" | "recall_output_preset" => vec!["target", "name"],
        "set_channel_volume" => vec!["channel_id"],
        "set_output_volume" => vec!["output_id"],
        "set_mute" => vec!["target", "muted"],
        "assign_applications" => vec!["channel_id", "process_ids"],
        "select_output" => vec!["output_id"],
        "set_output_priorities" => vec!["output_ids"],
        "rename_channel" => vec!["channel_id", "name"],
        "set_channel_visibility" => vec!["channel_id", "visible"],
        "set_channel_binding" => vec!["channel_id", "endpoint_id", "capture_id"],
        "set_startup" => vec!["enabled"],
        _ => vec![],
    }
}
pub fn catalog(grants: &BTreeSet<Permission>) -> Vec<Value> {
    definitions().into_iter().filter(|(n,p,_,_)|grants.contains(p)&&(!n.starts_with("recall_")||grants.contains(&Permission::ReadPresets))).map(|(n,_,d,properties)|json!({"name":n,"description":d,"inputSchema":{"type":"object","additionalProperties":false,"properties":properties,"required":required(n)}})).collect()
}
fn check_schema(v: &Value, s: &Value) -> Result<(), String> {
    if let Some(options) = s["oneOf"].as_array()
        && options
            .iter()
            .filter(|o| {
                o["required"]
                    .as_array()
                    .is_some_and(|r| r.iter().all(|k| v.get(k.as_str().unwrap_or("")).is_some()))
            })
            .count()
            != 1
    {
        return Err("Specify exactly one target identity".into());
    }
    if let Some(t) = s.get("type") {
        let types: Vec<&str> = if let Some(a) = t.as_array() {
            a.iter().filter_map(Value::as_str).collect()
        } else {
            vec![t.as_str().unwrap_or("")]
        };
        if !types.iter().any(|t| match *t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "number" => v.as_f64().is_some_and(f64::is_finite),
            "integer" => v.as_u64().is_some(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => false,
        }) {
            return Err("Invalid argument type".into());
        }
    }
    if let Some(a) = s["enum"].as_array()
        && !a.contains(v)
    {
        return Err("Unsupported argument value".into());
    }
    if let Some(o) = v.as_object() {
        if s["minProperties"]
            .as_u64()
            .is_some_and(|n| o.len() < n as usize)
        {
            return Err("Empty object is not a patch".into());
        }
        let p = s["properties"].as_object();
        if s["additionalProperties"] == false
            && o.keys().any(|k| !p.is_some_and(|p| p.contains_key(k)))
        {
            return Err("Unknown argument field".into());
        }
        if let Some(r) = s["required"].as_array()
            && r.iter().any(|k| !o.contains_key(k.as_str().unwrap_or("")))
        {
            return Err("Missing required argument".into());
        }
        if let Some(p) = p {
            for (k, v) in o {
                if let Some(s) = p.get(k) {
                    check_schema(v, s)?;
                }
            }
        }
    }
    if let Some(a) = v.as_array() {
        let n = a.len() as u64;
        if s["maxItems"].as_u64().is_some_and(|m| n > m)
            || s["minItems"].as_u64().is_some_and(|m| n < m)
        {
            return Err("Invalid array length".into());
        }
        for (i, v) in a.iter().enumerate() {
            if s["uniqueItems"] == true && a[..i].contains(v) {
                return Err("Duplicate array value".into());
            }
            check_schema(v, &s["items"])?;
        }
    }
    if let Some(n) = v.as_f64()
        && (!n.is_finite()
            || s["minimum"].as_f64().is_some_and(|m| n < m)
            || s["maximum"].as_f64().is_some_and(|m| n > m))
    {
        return Err("Numeric argument outside allowed range".into());
    }
    if let Some(t) = v.as_str() {
        let n = t.chars().count() as u64;
        if s["minLength"].as_u64().is_some_and(|m| n < m)
            || s["maxLength"].as_u64().is_some_and(|m| n > m)
            || t.contains('\0')
        {
            return Err("Invalid string length or NUL".into());
        }
    }
    Ok(())
}
fn str_arg<'a>(v: &'a Value, k: &str) -> Result<&'a str, String> {
    v[k].as_str().ok_or_else(|| format!("Expected {k}"))
}
fn target(state: &HostState, v: &Value) -> Result<(bool, String), String> {
    if let Some(id) = v["channel_id"].as_str() {
        if state.config.channels.iter().any(|c| c.id == id) {
            return Ok((true, id.into()));
        }
        return Err("Unknown channel_id".into());
    }
    let id = str_arg(v, "output_id")?;
    if state
        .windows
        .outputs
        .iter()
        .any(|e| e.id == id && !e.is_virtual)
        || state.config.output_eq.contains_key(id)
        || state.config.output_priorities.iter().any(|x| x == id)
    {
        Ok((false, id.into()))
    } else {
        Err("Unknown physical output_id".into())
    }
}
fn endpoint(state: &HostState, t: (bool, String)) -> Result<String, String> {
    if t.0 {
        state
            .config
            .channels
            .iter()
            .find(|c| c.id == t.1)
            .and_then(|c| c.endpoint_id.clone())
            .ok_or("Channel is not bound".into())
    } else if state
        .windows
        .outputs
        .iter()
        .any(|e| e.id == t.1 && !e.is_virtual)
    {
        Ok(t.1)
    } else {
        Err("Physical output is disconnected".into())
    }
}
fn eq_at(state: &HostState, t: &(bool, String)) -> EqSettings {
    if t.0 {
        state
            .config
            .channels
            .iter()
            .find(|c| c.id == t.1)
            .unwrap()
            .eq
            .clone()
    } else {
        state
            .config
            .output_eq
            .get(&t.1)
            .cloned()
            .unwrap_or_default()
    }
}
pub fn execute(name: &str, args: Value, grants: &BTreeSet<Permission>) -> Result<Value, String> {
    execute_with_backend(name, args, grants, &mut Local)
}
pub fn execute_with_backend(
    name: &str,
    args: Value,
    grants: &BTreeSet<Permission>,
    backend: &mut impl Backend,
) -> Result<Value, String> {
    let definition = definitions()
        .into_iter()
        .find(|(n, _, _, _)| *n == name)
        .ok_or("Unknown tool")?;
    if !grants.contains(&definition.1)
        || name.starts_with("recall_") && !grants.contains(&Permission::ReadPresets)
    {
        return Err("Permission not granted for this tool".into());
    }
    check_schema(
        &args,
        &json!({"type":"object","properties":definition.3,"required":required(name),"additionalProperties":false}),
    )?;
    match name {
        "read_presets" => return Ok(json!(backend.presets()?)),
        "save_preset" => {
            let eq: EqSettings =
                serde_json::from_value(args["eq"].clone()).map_err(|e| e.to_string())?;
            eq.validate(48000)?;
            backend.save_preset(
                str_arg(&args, "name")?,
                &eq,
                args["replace"].as_bool().unwrap_or(false),
            )?;
            return Ok(json!({"ok":true}));
        }
        "delete_preset" => {
            backend.delete_preset(str_arg(&args, "name")?)?;
            return Ok(json!({"ok":true}));
        }
        "read_startup" => return Ok(json!({"enabled":backend.startup(None)?})),
        "set_startup" => {
            backend.startup(args["enabled"].as_bool())?;
            return Ok(json!({"ok":true}));
        }
        _ => {}
    }
    // Every host operation uses an existing host; this module never launches one.
    // Refresh Windows values before requesting the lightweight digital meters.
    if name == "read_levels" {
        backend.request(Command::Snapshot)?;
    }
    let state = backend.request(if matches!(name, "read_levels" | "read_spectrum") {
        Command::SnapshotMeters
    } else {
        Command::Snapshot
    })?;
    match name {
        "read_status"=>return Ok(json!({"revision":state.revision,"audio_running":state.audio_running,"audio_status":if state.audio_running {"Running"} else {"Stopped"},"sample_rate":state.audio_diagnostics.sample_rate,"stream_faults":state.audio_diagnostics.stream_faults,"underruns":state.audio_diagnostics.underruns})),
        "read_devices"=>return Ok(json!({"outputs":state.windows.outputs.iter().map(|e|json!({"id":e.id,"name":e.name,"is_virtual":e.is_virtual})).collect::<Vec<_>>(),"inputs":state.windows.inputs.iter().map(|e|json!({"id":e.id,"name":e.name,"is_virtual":e.is_virtual})).collect::<Vec<_>>()})),
        "read_applications"=>return Ok(json!(state.windows.sessions.iter().map(|s|json!({"process_id":s.process_id,"name":s.name,"endpoint_id":s.endpoint_id,"active":s.active})).collect::<Vec<_>>())),
        "read_channel_settings"=>return Ok(json!(state.config.channels.iter().map(|c|json!({"id":c.id,"name":c.name,"visible":c.visible,"endpoint_id":c.endpoint_id,"capture_id":c.capture_id})).collect::<Vec<_>>())),
        "read_output_settings"=>return Ok(json!({"output_priorities":state.config.output_priorities,"selected_output_id":state.selected_output_id,"manual_output":state.manual_output})),
        "read_levels"=>return Ok(json!({"windows":state.windows.outputs.iter().map(|e|json!({"id":e.id,"scalar":e.volume,"muted":e.muted,"level_db":e.level_db})).collect::<Vec<_>>(),"running":state.audio_diagnostics.meters.running,"channels":state.audio_diagnostics.meters.channels.iter().map(|c|json!({"channel_id":c.channel_id,"peak":c.peak})).collect::<Vec<_>>(),"output_peak":state.audio_diagnostics.meters.output_peak})),
        "read_spectrum"=>return Ok(json!({"channels":state.audio_diagnostics.meters.channels.iter().map(|c|json!({"channel_id":c.channel_id,"spectrum":c.spectrum})).collect::<Vec<_>>(),"output":state.audio_diagnostics.meters.output_spectrum})),
        "read_eq"=>return Ok(json!(eq_at(&state,&target(&state,&args["target"])?))),
        _=>{}
    }
    let mut config = state.config.clone();
    let command = if name.ends_with("_eq") || name.starts_with("recall_") {
        let t = target(&state, &args["target"])?;
        let channel = name.contains("channel");
        if t.0 != channel {
            return Err("Tool permission does not match EQ target type".into());
        }
        let mut eq = eq_at(&state, &t);
        if name.starts_with("set_") {
            eq = serde_json::from_value(args["eq"].clone()).map_err(|e| e.to_string())?;
        } else if name.starts_with("reset_") {
            eq = EqSettings::default();
        } else if name.starts_with("recall_") {
            eq = backend
                .presets()?
                .into_iter()
                .find(|p| Some(p.name.as_str()) == args["name"].as_str())
                .ok_or("Unknown preset")?
                .eq;
        } else {
            if args.get("preamp_db").is_none()
                && args.get("bypass").is_none()
                && args.get("band").is_none()
            {
                return Err("An EQ patch needs a value".into());
            }
            if let Some(n) = args["preamp_db"].as_f64() {
                eq.preamp_db = n as f32;
            }
            if let Some(b) = args["bypass"].as_bool() {
                eq.bypass = b;
            }
            if args.get("band").is_some() != args.get("band_index").is_some() {
                return Err("band and band_index must be supplied together".into());
            }
            if let Some(i) = args["band_index"].as_u64() {
                let band = eq
                    .bands
                    .get_mut(i as usize)
                    .ok_or("Band index is outside this EQ")?;
                let mut merged = serde_json::to_value(&*band).map_err(|e| e.to_string())?;
                for (key, value) in args["band"].as_object().unwrap() {
                    merged[key] = value.clone();
                }
                *band = serde_json::from_value(merged).map_err(|e| e.to_string())?;
            }
        }
        eq.validate(48000)?;
        if t.0 {
            config.channels.iter_mut().find(|c| c.id == t.1).unwrap().eq = eq;
        } else {
            config.output_eq.insert(t.1, eq);
        }
        config.validate()?;
        Command::SaveConfig {
            config,
            expected_revision: state.revision,
        }
    } else {
        match name {
            "set_channel_volume" | "set_output_volume" => {
                let t = target(&state, &args)?;
                let id = endpoint(&state, t)?;
                match (args["scalar"].as_f64(), args["level_db"].as_f64()) {
                    (Some(v), None) => Command::SetVolume {
                        endpoint_id: id,
                        volume: v as f32,
                    },
                    (None, Some(v)) => {
                        let range = state
                            .windows
                            .outputs
                            .iter()
                            .find(|e| e.id == id)
                            .and_then(|e| e.level_db.as_ref())
                            .ok_or("Endpoint dB range is unavailable")?;
                        if v < f64::from(range.min) || v > f64::from(range.max) {
                            return Err("dB level outside endpoint range".into());
                        }
                        Command::SetVolumeDb {
                            endpoint_id: id,
                            level_db: v as f32,
                        }
                    }
                    _ => return Err("Specify exactly one scalar or level_db".into()),
                }
            }
            "set_mute" => Command::SetMute {
                endpoint_id: endpoint(&state, target(&state, &args["target"])?)?,
                muted: args["muted"].as_bool().unwrap(),
            },
            "assign_applications" => {
                target(&state, &json!({"channel_id":args["channel_id"]}))?;
                Command::AssignApplications {
                    channel_id: str_arg(&args, "channel_id")?.into(),
                    process_ids: args["process_ids"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_u64().unwrap() as u32)
                        .collect(),
                }
            }
            "select_output" => Command::SelectOutput {
                id: endpoint(&state, target(&state, &args)?)?,
            },
            "repair_windows_defaults" => Command::RepairDefaults,
            "stop_host" => Command::StopHost,
            "rename_channel"
            | "set_channel_visibility"
            | "set_channel_binding"
            | "set_output_priorities" => {
                if name == "set_output_priorities" {
                    let ids: Vec<String> = serde_json::from_value(args["output_ids"].clone())
                        .map_err(|e| e.to_string())?;
                    for id in &ids {
                        target(&state, &json!({"output_id":id}))?;
                    }
                    config.output_priorities = ids;
                } else {
                    let c = config
                        .channels
                        .iter_mut()
                        .find(|c| Some(c.id.as_str()) == args["channel_id"].as_str())
                        .ok_or("Unknown channel_id")?;
                    match name {
                        "rename_channel" => c.name = str_arg(&args, "name")?.into(),
                        "set_channel_visibility" => c.visible = args["visible"].as_bool().unwrap(),
                        _ => {
                            let ep = args["endpoint_id"].as_str();
                            let cp = args["capture_id"].as_str();
                            if ep.is_some() != cp.is_some() {
                                return Err("Supply both binding IDs or clear both".into());
                            }
                            if let (Some(ep), Some(cp)) = (ep, cp)
                                && (!state
                                    .windows
                                    .outputs
                                    .iter()
                                    .any(|e| e.id == ep && e.is_virtual)
                                    || !state.windows.inputs.iter().any(|e| e.id == cp))
                            {
                                return Err("Binding requires a connected virtual playback and capture pair".into());
                            }
                            c.endpoint_id = ep.map(str::to_owned);
                            c.capture_id = cp.map(str::to_owned);
                        }
                    }
                }
                config.validate()?;
                Command::SaveConfig {
                    config,
                    expected_revision: state.revision,
                }
            }
            _ => return Err("Unknown tool".into()),
        }
    };
    let updated = backend.request(command)?;
    Ok(json!({"ok":true,"revision":updated.revision}))
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) struct Fake {
        state: HostState,
        writes: usize,
        conflict: bool,
    }
    impl Fake {
        pub(super) fn new() -> Self {
            let mut config = crate::model::Config::default();
            config.channels[0].endpoint_id = Some("virtual".into());
            config.channels[0].capture_id = Some("capture".into());
            config.output_priorities = vec!["speaker".into()];
            Self {
                state: HostState {
                    audio_diagnostics: Default::default(),
                    protocol_version: 1,
                    revision: 7,
                    config,
                    windows: anada_windows::WindowsSnapshot {
                        outputs: vec![
                            anada_windows::Endpoint {
                                id: "speaker".into(),
                                name: "Speakers".into(),
                                volume: 0.2,
                                muted: false,
                                is_virtual: false,
                                level_db: Some(anada_windows::EndpointLevelDb {
                                    current: -24.0,
                                    min: -60.0,
                                    max: 0.0,
                                    step: 1.0,
                                }),
                            },
                            anada_windows::Endpoint {
                                id: "virtual".into(),
                                name: "Gaming".into(),
                                volume: 0.3,
                                muted: false,
                                is_virtual: true,
                                level_db: None,
                            },
                        ],
                        inputs: vec![],
                        sessions: vec![anada_windows::AudioSession {
                            executable_path: Some("PRIVATE_PATH".into()),
                            process_id: 42,
                            name: "Music".into(),
                            endpoint_id: "virtual".into(),
                            active: true,
                        }],
                        default_output_id: Some("virtual".into()),
                        communications_output_id: None,
                    },
                    selected_output_id: Some("speaker".into()),
                    manual_output: false,
                    audio_running: false,
                    audio_status: "PRIVATE_AUDIO_ERROR".into(),
                    warnings: vec!["PRIVATE_WARNING".into()],
                    host_pid: 99,
                    quiet: Default::default(),
                },
                writes: 0,
                conflict: false,
            }
        }
    }
    impl Backend for Fake {
        fn request(&mut self, c: Command) -> Result<HostState, String> {
            match c {
                Command::Snapshot | Command::SnapshotMeters => {}
                Command::SaveConfig {
                    config,
                    expected_revision,
                } => {
                    self.writes += 1;
                    if self.conflict || expected_revision != self.state.revision {
                        return Err("Revision conflict".into());
                    }
                    self.state.config = config;
                    self.state.revision += 1;
                }
                _ => self.writes += 1,
            }
            Ok(self.state.clone())
        }
        fn presets(&mut self) -> Result<Vec<EqPreset>, String> {
            Ok(vec![EqPreset {
                name: "Flat".into(),
                eq: EqSettings::default(),
            }])
        }
        fn save_preset(&mut self, _: &str, _: &EqSettings, _: bool) -> Result<(), String> {
            self.writes += 1;
            Ok(())
        }
        fn delete_preset(&mut self, _: &str) -> Result<(), String> {
            self.writes += 1;
            Ok(())
        }
        fn startup(&mut self, e: Option<bool>) -> Result<bool, String> {
            if e.is_some() {
                self.writes += 1;
            }
            Ok(e.unwrap_or(false))
        }
    }
    fn only(p: Permission) -> BTreeSet<Permission> {
        [p].into_iter().collect()
    }
    #[test]
    fn all_25_scopes_have_tools_and_deny_without_grants() {
        let defs = definitions();
        let scopes: BTreeSet<_> = defs.iter().map(|(_, p, _, _)| *p).collect();
        assert_eq!(scopes.len(), 25);
        for (name, p, _, _) in defs {
            assert!(
                catalog(&only(p)).iter().any(|t| t["name"] == name) || name.starts_with("recall_")
            );
            let mut b = Fake::new();
            assert!(
                execute_with_backend(name, json!({}), &BTreeSet::new(), &mut b)
                    .unwrap_err()
                    .contains("Permission")
            );
            assert_eq!(b.writes, 0);
        }
    }
    #[test]
    fn channel_eq_cannot_write_output_binding_or_volume() {
        let grants = only(Permission::WriteChannelEq);
        let mut b = Fake::new();
        let original = serde_json::to_value(&b.state.config).unwrap();
        for (name, args) in [
            (
                "set_output_eq",
                json!({"target":{"output_id":"speaker"},"eq":EqSettings::default()}),
            ),
            (
                "set_channel_binding",
                json!({"channel_id":"gaming","endpoint_id":null,"capture_id":null}),
            ),
            (
                "set_output_volume",
                json!({"output_id":"speaker","scalar":1.0}),
            ),
            (
                "set_channel_eq",
                json!({"target":{"output_id":"speaker"},"eq":EqSettings::default()}),
            ),
        ] {
            assert!(execute_with_backend(name, args, &grants, &mut b).is_err());
        }
        assert_eq!(b.writes, 0);
        assert_eq!(original, serde_json::to_value(&b.state.config).unwrap());
    }
    #[test]
    fn eq_mutates_only_target_and_returns_ack() {
        let mut b = Fake::new();
        let eq = EqSettings {
            preamp_db: -6.0,
            ..Default::default()
        };
        let before = serde_json::to_value(&b.state.config).unwrap();
        let result = execute_with_backend(
            "set_channel_eq",
            json!({"target":{"channel_id":"gaming"},"eq":eq}),
            &only(Permission::WriteChannelEq),
            &mut b,
        )
        .unwrap();
        assert_eq!(result, json!({"ok":true,"revision":8}));
        let mut after = serde_json::to_value(&b.state.config).unwrap();
        after["channels"][0]["eq"] = before["channels"][0]["eq"].clone();
        assert_eq!(before, after);
    }
    #[test]
    fn unknown_nested_fields_and_invalid_eq_never_save() {
        for args in [
            json!({"target":{"channel_id":"gaming"},"eq":{"bypass":false,"preamp_db":999,"bands":[]}}),
            json!({"target":{"channel_id":"gaming","extra":true},"eq":EqSettings::default()}),
            json!({"target":{"channel_id":"gaming"},"eq":EqSettings::default(),"config":{}}),
        ] {
            let mut b = Fake::new();
            assert!(
                execute_with_backend(
                    "set_channel_eq",
                    args,
                    &only(Permission::WriteChannelEq),
                    &mut b
                )
                .is_err()
            );
            assert_eq!(b.writes, 0);
        }
    }
    #[test]
    fn projections_do_not_leak_paths_or_ungranted_fields() {
        for (name, p, forbidden) in [
            (
                "read_devices",
                Permission::ReadDevices,
                vec!["volume", "muted", "level_db", "preamp_db", "PRIVATE"],
            ),
            (
                "read_applications",
                Permission::ReadApplications,
                vec!["executable_path", "PRIVATE", "preamp_db"],
            ),
            (
                "read_channel_settings",
                Permission::ReadChannelSettings,
                vec!["preamp_db", "volume", "PRIVATE"],
            ),
            (
                "read_output_settings",
                Permission::ReadOutputSettings,
                vec!["preamp_db", "volume", "PRIVATE"],
            ),
            (
                "read_status",
                Permission::ReadStatus,
                vec!["PRIVATE", "endpoint_id", "preamp_db", "volume"],
            ),
        ] {
            let mut b = Fake::new();
            let v = execute_with_backend(name, json!({}), &only(p), &mut b)
                .unwrap()
                .to_string();
            for word in forbidden {
                assert!(!v.contains(word), "{name} leaked {word}");
            }
            assert_eq!(b.writes, 0);
        }
    }
    #[test]
    fn revision_conflict_is_visible_without_retry() {
        let mut b = Fake::new();
        b.conflict = true;
        assert!(
            execute_with_backend(
                "rename_channel",
                json!({"channel_id":"gaming","name":"Games"}),
                &only(Permission::ChannelNames),
                &mut b
            )
            .unwrap_err()
            .contains("Revision conflict")
        );
        assert_eq!(b.writes, 1);
        assert_eq!(b.state.config.channels[0].name, "Gaming");
    }
    #[test]
    fn volume_permission_is_target_specific_and_db_is_bounded() {
        let mut b = Fake::new();
        for (name, args, p) in [
            (
                "set_output_volume",
                json!({"output_id":"speaker","scalar":1.1}),
                Permission::OutputVolume,
            ),
            (
                "set_output_volume",
                json!({"output_id":"virtual","scalar":0.1}),
                Permission::OutputVolume,
            ),
            (
                "set_output_volume",
                json!({"output_id":"speaker","level_db":1}),
                Permission::OutputVolume,
            ),
            (
                "set_channel_volume",
                json!({"channel_id":"gaming","scalar":0.5}),
                Permission::OutputVolume,
            ),
        ] {
            assert!(execute_with_backend(name, args, &only(p), &mut b).is_err());
        }
        assert_eq!(b.writes, 0);
        execute_with_backend(
            "set_output_volume",
            json!({"output_id":"speaker","scalar":0.1}),
            &only(Permission::OutputVolume),
            &mut b,
        )
        .unwrap();
        assert_eq!(b.writes, 1);
    }
    #[test]
    fn recall_requires_both_scopes_and_preserves_target_identity() {
        let mut b = Fake::new();
        let args = json!({"target":{"channel_id":"gaming"},"name":"Flat"});
        assert!(
            execute_with_backend(
                "recall_channel_preset",
                args.clone(),
                &only(Permission::WriteChannelEq),
                &mut b
            )
            .is_err()
        );
        assert_eq!(b.writes, 0);
        execute_with_backend(
            "recall_channel_preset",
            args,
            &[Permission::ReadPresets, Permission::WriteChannelEq]
                .into_iter()
                .collect(),
            &mut b,
        )
        .unwrap();
        assert_eq!(b.writes, 1);
    }
    #[test]
    fn every_catalog_schema_rejects_unknown_arguments_before_writes() {
        for (name, p, _, _) in definitions() {
            let mut b = Fake::new();
            let mut grants = only(p);
            grants.insert(Permission::ReadPresets);
            assert!(execute_with_backend(name, json!({"unknown":true}), &grants, &mut b).is_err());
            assert_eq!(b.writes, 0);
        }
    }
}

#[cfg(test)]
mod allowed_scope_tests {
    use super::*;
    #[test]
    fn each_permission_allows_its_intended_operation() {
        for (name, permission, _, _) in definitions() {
            let mut backend = super::tests::Fake::new();
            let target = if name.contains("output") {
                json!({"output_id":"speaker"})
            } else {
                json!({"channel_id":"gaming"})
            };
            let args = match name {
                "read_eq" | "reset_channel_eq" | "reset_output_eq" => json!({"target":target}),
                "set_channel_eq" | "set_output_eq" => {
                    json!({"target":target,"eq":EqSettings::default()})
                }
                "patch_channel_eq" | "patch_output_eq" => json!({"target":target,"preamp_db":-3}),
                "recall_channel_preset" | "recall_output_preset" => {
                    json!({"target":target,"name":"Flat"})
                }
                "save_preset" => json!({"name":"Test","eq":EqSettings::default()}),
                "delete_preset" => json!({"name":"Flat"}),
                "set_channel_volume" => json!({"channel_id":"gaming","scalar":0.1}),
                "set_output_volume" => json!({"output_id":"speaker","scalar":0.1}),
                "set_mute" => json!({"target":{"channel_id":"gaming"},"muted":true}),
                "assign_applications" => json!({"channel_id":"gaming","process_ids":[42]}),
                "select_output" => json!({"output_id":"speaker"}),
                "set_output_priorities" => json!({"output_ids":["speaker"]}),
                "rename_channel" => json!({"channel_id":"gaming","name":"Games"}),
                "set_channel_visibility" => json!({"channel_id":"gaming","visible":false}),
                "set_channel_binding" => {
                    json!({"channel_id":"gaming","endpoint_id":null,"capture_id":null})
                }
                "set_startup" => json!({"enabled":false}),
                _ => json!({}),
            };
            let mut grants: BTreeSet<_> = [permission].into_iter().collect();
            if name.starts_with("recall_") {
                grants.insert(Permission::ReadPresets);
            }
            assert!(
                execute_with_backend(name, args, &grants, &mut backend).is_ok(),
                "allowed tool failed: {name}"
            );
        }
    }
}

#[cfg(test)]
mod patch_tests {
    use super::*;
    #[test]
    fn partial_band_patch_preserves_other_band_values() {
        let mut backend = super::tests::Fake::new();
        execute_with_backend(
            "patch_channel_eq",
            json!({"target":{"channel_id":"gaming"},"band_index":0,"band":{"gain_db":-4}}),
            &[Permission::WriteChannelEq].into_iter().collect(),
            &mut backend,
        )
        .unwrap();
        let result = execute_with_backend(
            "read_eq",
            json!({"target":{"channel_id":"gaming"}}),
            &[Permission::ReadEq].into_iter().collect(),
            &mut backend,
        )
        .unwrap();
        assert_eq!(result["bands"][0]["gain_db"].as_f64(), Some(-4.0));
        assert_eq!(result["bands"][0]["frequency_hz"], 31.25);
        assert_eq!(result["bands"][1]["gain_db"].as_f64(), Some(0.0));
    }
}
