//! Lower Windows endpoint levels before opening audio. This is not a DSP gain.
use anada_windows::Endpoint;
use std::collections::HashSet;

pub const CHANNEL_CEILING_DB: f32 = -12.0;
pub const OUTPUT_CEILING_DB: f32 = -24.0;
pub const FALLBACK_CEILING: f32 = 0.20;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QuietTarget {
    Decibels(f32),
    Scalar(f32),
}

/// Plan only downward changes during initial setup. The Quiet toggle is separate
/// (see `model::quiet_target`).
/// Existing endpoints and all later automatic transitions keep Windows levels.
/// Callers must apply and verify the plan before audio opens, and block audio
/// when this function or a Windows write/readback fails.
pub fn plan_endpoint(
    endpoint: &Endpoint,
    initialized: bool,
    initial_setup: bool,
) -> Result<Option<QuietTarget>, String> {
    if endpoint.id.is_empty() || endpoint.id.contains('\0') {
        return Err("Quiet start requires a valid Windows endpoint ID".into());
    }
    if initialized || !initial_setup {
        return Ok(None);
    }
    let db = endpoint.level_db.as_ref().filter(|db| {
        db.current.is_finite()
            && db.min.is_finite()
            && db.max.is_finite()
            && db.min <= db.max
            && (db.min..=db.max).contains(&db.current)
            && db.step.is_finite()
            && db.step > 0.0
    });
    let scalar_valid = endpoint.volume.is_finite() && (0.0..=1.0).contains(&endpoint.volume);
    let near_max = db.is_some_and(|db| db.current >= db.max - 1.0)
        || (scalar_valid && endpoint.volume >= 0.95);
    if let Some(db) = db {
        let ceiling = if endpoint.is_virtual {
            CHANNEL_CEILING_DB
        } else {
            OUTPUT_CEILING_DB
        };
        let bounded = ceiling.clamp(db.min, db.max);
        // Round toward quieter supported steps, never toward a louder level.
        let steps = ((bounded - db.min) / db.step).floor();
        let target = (db.min + steps * db.step).clamp(db.min, db.max);
        if !target.is_finite() {
            return Err(format!(
                "Cannot determine a quiet level for {}",
                endpoint.name
            ));
        }
        if target < db.current {
            return Ok(Some(QuietTarget::Decibels(target)));
        }
        // A device with no usable attenuation must not quietly pass the guard.
        if db.min == db.max || (near_max && db.current > ceiling) {
            return Err(format!(
                "{} cannot be lowered to a quiet Windows level; audio remains stopped",
                endpoint.name
            ));
        }
        return Ok(None);
    }
    if !scalar_valid {
        return Err(format!(
            "Cannot read a valid Windows volume for {}; audio remains stopped",
            endpoint.name
        ));
    }
    Ok((endpoint.volume > FALLBACK_CEILING).then_some(QuietTarget::Scalar(FALLBACK_CEILING)))
}

/// Plan each endpoint ID once. Duplicate observations must agree so inventory
/// ambiguity cannot bypass protection. No Windows writes occur in this function.
pub fn plan_endpoints<'a>(
    endpoints: impl IntoIterator<Item = &'a Endpoint>,
    initialized_ids: &[String],
    initial_setup: bool,
) -> Result<Vec<(String, QuietTarget)>, String> {
    if !initial_setup || !initialized_ids.is_empty() {
        return Ok(Vec::new());
    }
    let initialized: HashSet<_> = initialized_ids.iter().map(String::as_str).collect();
    let mut observed = std::collections::HashMap::new();
    let mut result = Vec::new();
    for endpoint in endpoints {
        let target = plan_endpoint(
            endpoint,
            initialized.contains(endpoint.id.as_str()),
            initial_setup,
        )?;
        if let Some(previous) = observed.insert(endpoint.id.as_str(), target) {
            if previous != target {
                return Err(format!(
                    "Conflicting Windows volume observations for {}",
                    endpoint.name
                ));
            }
            continue;
        }
        if let Some(target) = target {
            result.push((endpoint.id.clone(), target));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anada_windows::EndpointLevelDb;

    fn endpoint(current: f32, is_virtual: bool) -> Endpoint {
        Endpoint {
            id: "endpoint".into(),
            name: "Test endpoint".into(),
            volume: if current == 0.0 { 1.0 } else { 0.4 },
            muted: false,
            is_virtual,
            level_db: Some(EndpointLevelDb {
                current,
                min: -96.0,
                max: 0.0,
                step: 0.5,
            }),
        }
    }

    #[test]
    fn new_full_endpoints_get_role_specific_ceilings() {
        assert_eq!(
            plan_endpoint(&endpoint(0.0, false), false, true).unwrap(),
            Some(QuietTarget::Decibels(-24.0))
        );
        assert_eq!(
            plan_endpoint(&endpoint(0.0, true), false, true).unwrap(),
            Some(QuietTarget::Decibels(-12.0))
        );
    }

    #[test]
    fn existing_quiet_levels_are_never_raised() {
        for current in [-96.0, -60.0, -24.0] {
            assert_eq!(
                plan_endpoint(&endpoint(current, false), false, true).unwrap(),
                None
            );
        }
    }

    #[test]
    fn restarts_and_device_changes_preserve_windows_levels() {
        for is_virtual in [false, true] {
            for current in [-60.0, -6.0, -0.5, 0.0] {
                let value = endpoint(current, is_virtual);
                assert_eq!(plan_endpoint(&value, true, true).unwrap(), None);
                assert_eq!(plan_endpoint(&value, true, false).unwrap(), None);
                // A newly connected device must also keep its Windows level.
                assert_eq!(plan_endpoint(&value, false, false).unwrap(), None);
                let scalar_only = Endpoint {
                    level_db: None,
                    ..value
                };
                assert_eq!(plan_endpoint(&scalar_only, true, true).unwrap(), None);
                assert_eq!(plan_endpoint(&scalar_only, false, false).unwrap(), None);
            }
        }
    }

    #[test]
    fn absent_or_malformed_db_uses_downward_scalar_fallback() {
        for malformed in [
            None,
            Some(EndpointLevelDb {
                current: 0.0,
                min: 1.0,
                max: 0.0,
                step: 0.5,
            }),
            Some(EndpointLevelDb {
                current: f32::NAN,
                min: -96.0,
                max: 0.0,
                step: 0.5,
            }),
        ] {
            let mut value = endpoint(0.0, false);
            value.level_db = malformed;
            assert_eq!(
                plan_endpoint(&value, false, true).unwrap(),
                Some(QuietTarget::Scalar(0.20))
            );
            value.volume = 0.1;
            assert_eq!(plan_endpoint(&value, false, true).unwrap(), None);
            value.volume = f32::NAN;
            assert!(plan_endpoint(&value, false, true).is_err());
        }
    }

    #[test]
    fn bounded_levels_and_step_rounding_never_raise_volume() {
        let mut value = endpoint(-6.0, false);
        let db = value.level_db.as_mut().unwrap();
        db.min = -10.0;
        assert_eq!(
            plan_endpoint(&value, false, true).unwrap(),
            Some(QuietTarget::Decibels(-10.0))
        );
        let db = value.level_db.as_mut().unwrap();
        db.min = -95.0;
        db.step = 2.0;
        assert_eq!(
            plan_endpoint(&value, false, true).unwrap(),
            Some(QuietTarget::Decibels(-25.0))
        );
        let db = value.level_db.as_mut().unwrap();
        db.min = 0.0;
        db.max = 0.0;
        db.current = 0.0;
        assert!(plan_endpoint(&value, false, true).is_err());
    }

    #[test]
    fn duplicate_ids_produce_only_one_write_and_conflicts_fail_closed() {
        let value = endpoint(0.0, false);
        assert_eq!(
            plan_endpoints([&value, &value], &[], true).unwrap().len(),
            1
        );
        let quiet = endpoint(-60.0, false);
        assert!(plan_endpoints([&value, &quiet], &[], true).is_err());
        assert!(
            plan_endpoints([&value], &[value.id.clone(), value.id.clone()], false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn initial_setup_history_survives_switches_reconnects_and_restarts() {
        let a = endpoint(0.0, false);
        let b = Endpoint {
            id: "other".into(),
            level_db: None,
            ..a.clone()
        };
        assert_eq!(plan_endpoints([&a], &[], true).unwrap().len(), 1);
        let history = vec![a.id.clone()];
        // Switch to a new device, back, disconnect, reconnect, then restart.
        for endpoints in [vec![&b], vec![&a], vec![], vec![&b], vec![&a, &b]] {
            for startup in [false, true] {
                assert!(
                    plan_endpoints(endpoints.clone(), &history, startup)
                        .unwrap()
                        .is_empty()
                );
            }
        }
        assert!(plan_endpoints([&b], &[], false).unwrap().is_empty());
        // A device seen for the first time during initial setup is still planned.
        assert_eq!(
            plan_endpoint(&b, false, true).unwrap(),
            Some(QuietTarget::Scalar(0.2))
        );
    }
}
