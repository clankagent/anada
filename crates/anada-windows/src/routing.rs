#![cfg(windows)]

use std::ffi::c_void;
use windows::Win32::Media::Audio::{
    EDataFlow, ERole, eCommunications, eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows::core::{
    GUID, HRESULT, HSTRING, IInspectable_Vtbl, IUnknown, IUnknown_Vtbl, Interface, PCWSTR,
};

const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const DEVICE_PREFIX: &str = r"\\?\SWD#MMDEVAPI#";
const RENDER_SUFFIX: &str = "#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const ROLES: [ERole; 3] = [eConsole, eMultimedia, eCommunications];

use super::PersistedPlaybackPreference;

pub fn set_default_playback(endpoint_id: &str) -> Result<(), String> {
    set_default_roles(endpoint_id, &[eConsole, eMultimedia])
}

pub fn set_default_communications_playback(endpoint_id: &str) -> Result<(), String> {
    set_default_roles(endpoint_id, &[eCommunications])
}

pub fn set_process_playback_preference(process_id: u32, endpoint_id: &str) -> Result<(), String> {
    set_process_playback_preferences(
        process_id,
        &PersistedPlaybackPreference {
            console: Some(endpoint_id.into()),
            multimedia: Some(endpoint_id.into()),
            communications: Some(endpoint_id.into()),
        },
    )
}

pub fn set_process_playback_preferences(
    process_id: u32,
    preferences: &PersistedPlaybackPreference,
) -> Result<(), String> {
    validate_process_id(process_id)?;
    let policy = PersistedPolicy::activate()?;
    let (object, vtable) = policy.parts();
    dispatch_preferences(preferences, |role, device_id| {
        let result = unsafe {
            (vtable.set_persisted_endpoint)(
                object,
                process_id,
                eRender,
                role,
                persisted_endpoint_argument(device_id),
            )
        };
        result.ok().map_err(|error| {
            format!(
                "Set persisted playback preference for PID {process_id}, {}: {error}; \
                 Setter success does not prove an existing audio stream moved",
                role_name(role)
            )
        })
    })
}

fn persisted_endpoint_argument(device_id: Option<&HSTRING>) -> *mut c_void {
    match device_id {
        Some(device_id) => unsafe { std::mem::transmute_copy(device_id) },
        None => std::ptr::null_mut(),
    }
}

fn dispatch_preferences(
    preferences: &PersistedPlaybackPreference,
    mut write_role: impl FnMut(ERole, Option<&HSTRING>) -> Result<(), String>,
) -> Result<(), String> {
    let device_ids = [
        preferences.console.as_deref(),
        preferences.multimedia.as_deref(),
        preferences.communications.as_deref(),
    ]
    .map(|endpoint| {
        endpoint
            .map(pack_render_id)
            .transpose()
            .map(|id| id.map(HSTRING::from))
    });
    let [console, multimedia, communications] = device_ids;
    let device_ids = [console?, multimedia?, communications?];
    for (completed, (role, device_id)) in ROLES.into_iter().zip(&device_ids).enumerate() {
        write_role(role, device_id.as_ref()).map_err(|error| {
            format!("{error}; {completed} earlier role(s) changed, no rollback performed")
        })?;
    }
    Ok(())
}

pub fn get_process_playback_preference(
    process_id: u32,
) -> Result<PersistedPlaybackPreference, String> {
    validate_process_id(process_id)?;
    let policy = PersistedPolicy::activate()?;
    let (object, vtable) = policy.parts();
    let mut preferences = [None, None, None];
    for (index, role) in ROLES.into_iter().enumerate() {
        let mut device_id = HSTRING::new();
        let result = unsafe {
            (vtable.get_persisted_endpoint)(object, process_id, eRender, role, &mut device_id)
        };
        result.ok().map_err(|error| {
            format!(
                "Read persisted playback preference for PID {process_id}, {}: {error}. \
                 The requested preference is unavailable; this is not a default-device result",
                role_name(role)
            )
        })?;
        let device_id = String::from_utf16(&device_id)
            .map_err(|error| format!("Persisted playback preference is invalid UTF-16: {error}"))?;
        preferences[index] = unpack_render_id(&device_id)?;
    }
    let [console, multimedia, communications] = preferences;
    Ok(PersistedPlaybackPreference {
        console,
        multimedia,
        communications,
    })
}

fn set_default_roles(endpoint_id: &str, roles: &[ERole]) -> Result<(), String> {
    validate_raw_id(endpoint_id)?;
    let endpoint_id: Vec<u16> = endpoint_id.encode_utf16().chain(Some(0)).collect();
    let policy: DefaultPolicy =
        unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }.map_err(|error| {
            format!(
                "Windows playback default policy (IPolicyConfig F8679F50) is unavailable: {error}. \
             This undocumented interface is not supported on every Windows build; \
             call on the initialized COM worker"
            )
        })?;
    for (completed, role) in roles.iter().copied().enumerate() {
        unsafe {
            (policy.vtable().set_default_endpoint)(
                policy.as_raw(),
                PCWSTR(endpoint_id.as_ptr()),
                role,
            )
        }
        .ok()
        .map_err(|error| {
            format!(
                "Set Windows playback default for {}: {error}; \
                 {completed} earlier role(s) changed, no rollback performed",
                role_name(role)
            )
        })?;
    }
    Ok(())
}

fn validate_process_id(process_id: u32) -> Result<(), String> {
    if process_id == 0 {
        Err("A nonzero application process ID is required".into())
    } else {
        Ok(())
    }
}

fn validate_raw_id(endpoint_id: &str) -> Result<(), String> {
    if endpoint_id.trim().is_empty() || endpoint_id.contains('\0') {
        Err("Playback endpoint ID must be nonempty and contain no NUL characters".into())
    } else if endpoint_id.starts_with(r"\\?\") {
        Err("Expected a raw Windows playback endpoint ID, not a device-interface path".into())
    } else {
        Ok(())
    }
}

fn pack_render_id(endpoint_id: &str) -> Result<String, String> {
    validate_raw_id(endpoint_id)?;
    Ok(format!("{DEVICE_PREFIX}{endpoint_id}{RENDER_SUFFIX}"))
}

fn unpack_render_id(device_id: &str) -> Result<Option<String>, String> {
    if device_id.is_empty() {
        return Ok(None);
    }
    let endpoint_id = device_id
        .strip_prefix(DEVICE_PREFIX)
        .and_then(|value| value.strip_suffix(RENDER_SUFFIX))
        .ok_or_else(|| {
            "Windows returned an unrecognized persisted playback device-interface ID; \
             this policy ABI may be unsupported on this Windows build"
                .to_owned()
        })?;
    validate_raw_id(endpoint_id)?;
    Ok(Some(endpoint_id.to_owned()))
}

fn role_name(role: ERole) -> &'static str {
    match role {
        role if role == eConsole => "console",
        role if role == eMultimedia => "multimedia",
        role if role == eCommunications => "communications",
        _ => "unknown role",
    }
}

#[repr(C)]
struct DefaultPolicyVtable {
    base: IUnknown_Vtbl,
    unused: [usize; 10],
    set_default_endpoint: unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT,
}

#[repr(C)]
struct PersistedPolicyVtable {
    base: IInspectable_Vtbl,
    unused: [usize; 19],
    set_persisted_endpoint:
        unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut c_void) -> HRESULT,
    get_persisted_endpoint:
        unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut HSTRING) -> HRESULT,
}

#[repr(transparent)]
#[derive(Clone)]
struct DefaultPolicy(IUnknown);

unsafe impl Interface for DefaultPolicy {
    type Vtable = DefaultPolicyVtable;
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
}

#[repr(transparent)]
#[derive(Clone)]
struct PersistedPolicy21H2(IUnknown);

unsafe impl Interface for PersistedPolicy21H2 {
    type Vtable = PersistedPolicyVtable;
    const IID: GUID = GUID::from_u128(0xab3d4648_e242_459f_b02f_541c70306324);
}

#[repr(transparent)]
#[derive(Clone)]
struct PersistedPolicyDownlevel(IUnknown);

unsafe impl Interface for PersistedPolicyDownlevel {
    type Vtable = PersistedPolicyVtable;
    const IID: GUID = GUID::from_u128(0x2a59116d_6c4f_45e0_a74f_707e3fef9258);
}

enum PersistedPolicy {
    Current(PersistedPolicy21H2),
    Downlevel(PersistedPolicyDownlevel),
}

impl PersistedPolicy {
    fn activate() -> Result<Self, String> {
        let class = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
        match unsafe { RoGetActivationFactory::<PersistedPolicy21H2>(&class) } {
            Ok(policy) => Ok(Self::Current(policy)),
            Err(current_error) if current_error.code() == HRESULT(0x80004002u32 as i32) => {
                unsafe { RoGetActivationFactory::<PersistedPolicyDownlevel>(&class) }
                    .map(Self::Downlevel)
                    .map_err(|downlevel_error| {
                        format!(
                            "Windows persisted playback policy is unavailable. \
                             21H2 IID AB3D4648: {current_error}; \
                             downlevel IID 2A59116D: {downlevel_error}. \
                             No supported undocumented policy ABI was activated"
                        )
                    })
            }
            Err(error) => Err(format!(
                "Activate Windows.Media.Internal.AudioPolicyConfig: {error}. \
                 Call on the initialized COM MTA worker; this undocumented policy \
                 may be unavailable on this Windows build"
            )),
        }
    }

    fn parts(&self) -> (*mut c_void, &PersistedPolicyVtable) {
        match self {
            Self::Current(policy) => (policy.as_raw(), policy.vtable()),
            Self::Downlevel(policy) => (policy.as_raw(), policy.vtable()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn mixed_preferences_dispatch_exact_roles_and_null_clear_handle() {
        let preferences = PersistedPlaybackPreference {
            console: Some("console-render".into()),
            multimedia: None,
            communications: Some("communications-render".into()),
        };
        let mut observed = Vec::new();
        dispatch_preferences(&preferences, |role, device_id| {
            let argument = persisted_endpoint_argument(device_id);
            assert_eq!(argument.is_null(), device_id.is_none());
            let raw = device_id.map(|id| String::from_utf16(id).unwrap());
            observed.push((role.0, raw));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            observed,
            vec![
                (eConsole.0, Some(pack_render_id("console-render").unwrap())),
                (eMultimedia.0, None),
                (
                    eCommunications.0,
                    Some(pack_render_id("communications-render").unwrap())
                ),
            ]
        );
    }

    #[test]
    fn absent_preferences_clear_each_role_without_global_reset() {
        let preferences = PersistedPlaybackPreference {
            console: None,
            multimedia: None,
            communications: None,
        };
        let mut observed = Vec::new();
        dispatch_preferences(&preferences, |role, device_id| {
            assert!(persisted_endpoint_argument(device_id).is_null());
            observed.push(role.0);
            Ok(())
        })
        .unwrap();
        assert_eq!(observed, [eConsole.0, eMultimedia.0, eCommunications.0]);
    }

    #[test]
    fn invalid_last_role_prevents_all_writes() {
        let preferences = PersistedPlaybackPreference {
            console: Some("valid-render".into()),
            multimedia: None,
            communications: Some("".into()),
        };
        let mut writes = 0;
        assert!(
            dispatch_preferences(&preferences, |_, _| {
                writes += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(writes, 0);
    }

    #[test]
    fn dispatch_failure_reports_partial_change_and_stops() {
        let preferences = PersistedPlaybackPreference {
            console: None,
            multimedia: None,
            communications: None,
        };
        let mut observed = Vec::new();
        let error = dispatch_preferences(&preferences, |role, _| {
            observed.push(role.0);
            if role == eMultimedia {
                Err("fake failure".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(observed, [eConsole.0, eMultimedia.0]);
        assert!(error.contains("1 earlier role(s) changed"));
    }

    #[test]
    fn render_id_round_trip_preserves_raw_endpoint() {
        let endpoint = "{0.0.0.00000000}.{12345678-1234-1234-1234-123456789abc}";
        let packed = pack_render_id(endpoint).unwrap();
        assert_eq!(
            packed,
            format!(r"\\?\SWD#MMDEVAPI#{endpoint}#{{e6327cad-dcec-4949-ae8a-991e976a79d2}}")
        );
        assert_eq!(unpack_render_id(&packed).unwrap(), Some(endpoint.into()));
        assert_eq!(unpack_render_id("").unwrap(), None);
    }

    #[test]
    fn malformed_ids_do_not_become_other_preferences() {
        for endpoint in [
            "",
            " ",
            "endpoint\0suffix",
            r"\\?\SWD#MMDEVAPI#already-packed",
        ] {
            assert!(pack_render_id(endpoint).is_err());
        }
        for device in [
            "raw-unexpected-id",
            r"\\?\SWD#MMDEVAPI#endpoint#{2eef81be-33fa-4800-9670-1cd474972c3f}",
        ] {
            assert!(unpack_render_id(device).is_err());
        }
        assert!(unpack_render_id(&format!("{DEVICE_PREFIX}{RENDER_SUFFIX}")).is_err());
        assert!(validate_process_id(0).is_err());
        assert!(validate_process_id(1).is_ok());
    }

    #[test]
    fn vtable_prefixes_match_verified_slot_numbers() {
        let pointer = size_of::<usize>();
        assert_eq!(size_of::<IUnknown_Vtbl>(), 3 * pointer);
        assert_eq!(size_of::<IInspectable_Vtbl>(), 6 * pointer);
        assert_eq!(
            offset_of!(DefaultPolicyVtable, set_default_endpoint),
            13 * pointer
        );
        assert_eq!(
            offset_of!(PersistedPolicyVtable, set_persisted_endpoint),
            25 * pointer
        );
        assert_eq!(
            offset_of!(PersistedPolicyVtable, get_persisted_endpoint),
            26 * pointer
        );
        assert_eq!(size_of::<HSTRING>(), pointer);
        assert_eq!(size_of::<ERole>(), 4);
        assert_eq!(size_of::<EDataFlow>(), 4);
        assert_eq!(eRender.0, 0);
        assert_eq!(ROLES.map(|role| role.0), [0, 1, 2]);
    }
}
