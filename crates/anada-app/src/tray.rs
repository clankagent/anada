//! The host owns the notification icon and its independent Windows message loop.
#[cfg(windows)]
mod platform {
    use std::cell::RefCell;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::process::Child;
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{BOOL, PCWSTR, PWSTR, w};

    const CALLBACK: u32 = WM_APP + 19;
    const OPEN: usize = 1;
    const EXIT: usize = 2;
    const OUTPUT_FIRST: usize = 100;
    const OUTPUT_LIMIT: usize = 64;

    struct OutputChoice {
        command_id: usize,
        endpoint_id: String,
        label: String,
        checked: bool,
    }

    fn menu_text(text: &str) -> String {
        text.chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>()
            .replace('&', "&&")
    }

    fn output_choices(
        outputs: &[anada_windows::Endpoint],
        selected: Option<&str>,
    ) -> Vec<OutputChoice> {
        let mut seen = std::collections::HashSet::new();
        let physical: Vec<_> = outputs
            .iter()
            .filter(|endpoint| !endpoint.is_virtual && seen.insert(endpoint.id.as_str()))
            .collect();
        let mut counts = std::collections::HashMap::new();
        for endpoint in &physical {
            *counts.entry(menu_text(&endpoint.name)).or_insert(0usize) += 1;
        }
        physical
            .into_iter()
            .take(OUTPUT_LIMIT)
            .enumerate()
            .map(|(index, endpoint)| {
                let name = menu_text(&endpoint.name);
                let label = if counts[&name] > 1 {
                    format!("{name} [{}]", menu_text(&endpoint.id))
                } else {
                    name
                };
                OutputChoice {
                    command_id: OUTPUT_FIRST + index,
                    endpoint_id: endpoint.id.clone(),
                    label,
                    checked: selected == Some(endpoint.id.as_str()),
                }
            })
            .collect()
    }

    fn menu_wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    pub struct Tray {
        window: isize,
        thread: Option<std::thread::JoinHandle<()>>,
        stopped: mpsc::Receiver<()>,
    }
    impl Tray {
        pub fn start() -> Result<Self, String> {
            let (send, receive) = mpsc::sync_channel(1);
            let (stopped_send, stopped) = mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("anada-tray".into())
                .spawn(move || {
                    let result = unsafe { create() };
                    let tray = match result {
                        Ok(tray) => tray,
                        Err(error) => {
                            let _ = send.send(Err(error));
                            return;
                        }
                    };
                    if send.send(Ok(tray.window.0 as isize)).is_err() {
                        return;
                    }
                    unsafe {
                        let mut message = MSG::default();
                        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                            let _ = TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                    drop(tray);
                    let _ = stopped_send.send(());
                })
                .map_err(|e| e.to_string())?;
            let window = receive
                .recv_timeout(Duration::from_secs(5))
                .map_err(|e| e.to_string())??;
            Ok(Self {
                window,
                thread: Some(thread),
                stopped,
            })
        }
    }
    impl Drop for Tray {
        fn drop(&mut self) {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.window as *mut _)),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            // A shell menu or an unresponsive Windows API must not hold audio shutdown forever.
            if let Some(thread) = self.thread.take()
                && self
                    .stopped
                    .recv_timeout(Duration::from_millis(500))
                    .is_ok()
            {
                let _ = thread.join();
            }
        }
    }

    thread_local! { static PENDING_UI: RefCell<Option<Child>> = const { RefCell::new(None) }; }

    fn same_executable(expected: &Path, observed: &Path) -> bool {
        match (expected.canonicalize(), observed.canonicalize()) {
            (Ok(expected), Ok(observed)) => expected == observed,
            _ => false,
        }
    }

    struct WindowSearch {
        executable: PathBuf,
        found: Option<HWND>,
    }

    unsafe extern "system" fn find_ui(window: HWND, parameter: LPARAM) -> BOOL {
        unsafe {
            let search = &mut *(parameter.0 as *mut WindowSearch);
            // Ignore invisible helper windows and owned popups. Never match by title or process name.
            if !IsWindowVisible(window).as_bool() || GetWindow(window, GW_OWNER).is_ok() {
                return BOOL(1);
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                let mut path = vec![0u16; 32768];
                let mut length = path.len() as u32;
                let queried = QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    PWSTR(path.as_mut_ptr()),
                    &mut length,
                );
                let _ = CloseHandle(process);
                if queried.is_ok() {
                    use std::os::windows::ffi::OsStringExt;
                    let observed =
                        PathBuf::from(std::ffi::OsString::from_wide(&path[..length as usize]));
                    if same_executable(&search.executable, &observed) {
                        search.found = Some(window);
                        return BOOL(0);
                    }
                }
            }
            BOOL(1)
        }
    }

    fn open() {
        let Ok(executable) = std::env::current_exe() else {
            return;
        };
        let executable = executable.with_file_name("anada.exe");
        let mut search = WindowSearch {
            executable: executable.clone(),
            found: None,
        };
        unsafe {
            let _ = EnumWindows(
                Some(find_ui),
                LPARAM((&mut search as *mut WindowSearch) as isize),
            );
            if let Some(window) = search.found
                && IsWindow(Some(window)).as_bool()
                && ShowWindowAsync(window, SW_RESTORE).as_bool()
            {
                let _ = SetForegroundWindow(window);
                return;
            }
        }
        PENDING_UI.with_borrow_mut(|pending| {
            if let Some(child) = pending.as_mut() {
                match child.try_wait() {
                    Ok(None) => return,
                    Ok(Some(_)) | Err(_) => *pending = None,
                }
            }
            *pending = std::process::Command::new(executable).spawn().ok();
        });
    }

    struct NativeTray {
        window: HWND,
        class_name: Vec<u16>,
    }
    impl Drop for NativeTray {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.window);
                let _ = UnregisterClassW(PCWSTR(self.class_name.as_ptr()), None);
            }
        }
    }

    unsafe fn create() -> Result<NativeTray, String> {
        unsafe {
            let class_name: Vec<u16> = format!(
                "AnadaHostTrayV1-{}",
                windows::Win32::System::Threading::GetCurrentThreadId()
            )
            .encode_utf16()
            .chain(Some(0))
            .collect();
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(windows::core::Error::from_thread().to_string());
            }
            let window = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                w!("Anada host"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            );
            let window = match window {
                Ok(window) => window,
                Err(error) => {
                    let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), None);
                    return Err(error.to_string());
                }
            };
            let tray = NativeTray { window, class_name };
            add_icon(window)?;
            Ok(tray)
        }
    }
    /// The icon embedded in this executable, at the size of the notification area.
    unsafe fn app_icon() -> Result<HICON, String> {
        unsafe {
            let module = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let embedded = LoadImageW(
                Some(module.into()),
                // Resource ID 1, passed in place of a name (MAKEINTRESOURCE).
                PCWSTR(std::ptr::without_provenance(1)),
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_SHARED,
            );
            match embedded {
                Ok(icon) => Ok(HICON(icon.0)),
                Err(_) => LoadIconW(None, IDI_APPLICATION).map_err(|e| e.to_string()),
            }
        }
    }
    unsafe fn add_icon(window: HWND) -> Result<(), String> {
        unsafe {
            let mut data = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: window,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: CALLBACK,
                hIcon: app_icon()?,
                ..Default::default()
            };
            let tip: Vec<u16> = "Anada audio host".encode_utf16().collect();
            data.szTip[..tip.len()].copy_from_slice(&tip);
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                return Err("Cannot create the Anada notification icon".into());
            }
            Ok(())
        }
    }
    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> LRESULT {
        unsafe {
            // Explorer can restart while audio continues. Recreate its lost icon.
            if message == RegisterWindowMessageW(w!("TaskbarCreated")) {
                let _ = add_icon(window);
                return LRESULT(0);
            }
            match message {
                WM_QUERYENDSESSION => LRESULT(1),
                WM_ENDSESSION if wp.0 != 0 => {
                    std::thread::spawn(|| {
                        let _ = crate::transport::request(crate::model::Command::StopHost);
                    });
                    LRESULT(0)
                }
                CALLBACK if lp.0 as u32 == WM_LBUTTONDBLCLK => {
                    open();
                    LRESULT(0)
                }
                CALLBACK if matches!(lp.0 as u32, WM_RBUTTONUP | WM_CONTEXTMENU) => {
                    if let Ok(menu) = CreatePopupMenu() {
                        // IPC runs on the tray thread, independently of host control and audio.
                        // The transport enforces its existing request deadline.
                        let snapshot = crate::transport::request(crate::model::Command::Snapshot);
                        let choices = match &snapshot {
                            Ok(state) => output_choices(
                                &state.windows.outputs,
                                state.selected_output_id.as_deref(),
                            ),
                            Err(_) => Vec::new(),
                        };
                        let _ = AppendMenuW(menu, MF_STRING, OPEN, w!("Open Anada"));
                        match &snapshot {
                            Err(_) => {
                                let _ = AppendMenuW(
                                    menu,
                                    MF_STRING | MF_GRAYED,
                                    0,
                                    w!("Listening output (host unavailable)"),
                                );
                            }
                            Ok(_) if choices.is_empty() => {
                                let _ = AppendMenuW(
                                    menu,
                                    MF_STRING | MF_GRAYED,
                                    0,
                                    w!("Listening output (no physical outputs)"),
                                );
                            }
                            Ok(_) => {
                                if let Ok(outputs) = CreatePopupMenu() {
                                    for choice in &choices {
                                        let label = menu_wide(&choice.label);
                                        let flags = if choice.checked {
                                            MF_STRING | MF_CHECKED
                                        } else {
                                            MF_STRING
                                        };
                                        let _ = AppendMenuW(
                                            outputs,
                                            flags,
                                            choice.command_id,
                                            PCWSTR(label.as_ptr()),
                                        );
                                    }
                                    // The parent menu owns this submenu after successful insertion.
                                    if AppendMenuW(
                                        menu,
                                        MF_POPUP,
                                        outputs.0 as usize,
                                        w!("Listening output"),
                                    )
                                    .is_err()
                                    {
                                        let _ = DestroyMenu(outputs);
                                    }
                                } else {
                                    let _ = AppendMenuW(
                                        menu,
                                        MF_STRING | MF_GRAYED,
                                        0,
                                        w!("Listening output (menu unavailable)"),
                                    );
                                }
                            }
                        }
                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                        let _ = AppendMenuW(
                            menu,
                            MF_STRING,
                            EXIT,
                            w!("Exit host (stops the listening mix)"),
                        );
                        let mut point = POINT::default();
                        let _ = GetCursorPos(&mut point);
                        let _ = SetForegroundWindow(window);
                        let selected = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_RIGHTBUTTON,
                            point.x,
                            point.y,
                            None,
                            window,
                            None,
                        )
                        .0 as usize;
                        let _ = DestroyMenu(menu);
                        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
                        match selected {
                            OPEN => open(),
                            EXIT => {
                                std::thread::spawn(|| {
                                    let _ =
                                        crate::transport::request(crate::model::Command::StopHost);
                                });
                            }
                            _ => {
                                if let Some(choice) = choices
                                    .into_iter()
                                    .find(|choice| choice.command_id == selected)
                                {
                                    let result = crate::transport::request(
                                        crate::model::Command::SelectOutput {
                                            id: choice.endpoint_id,
                                        },
                                    );
                                    if let Err(error) = result {
                                        let text = menu_wide(&format!(
                                            "Could not select the listening output.\n\n{error}\n\nOpen Anada to inspect the current output."
                                        ));
                                        let _ = MessageBoxW(
                                            Some(window),
                                            PCWSTR(text.as_ptr()),
                                            w!("Anada output selection"),
                                            MB_OK | MB_ICONERROR,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    LRESULT(0)
                }
                WM_CLOSE => {
                    let _ = DestroyWindow(window);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    let data = NOTIFYICONDATAW {
                        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                        hWnd: window,
                        uID: 1,
                        ..Default::default()
                    };
                    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(window, message, wp, lp),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn endpoint(id: &str, name: &str, is_virtual: bool) -> anada_windows::Endpoint {
            anada_windows::Endpoint {
                level_db: None,
                id: id.into(),
                name: name.into(),
                is_virtual,
                volume: 0.5,
                muted: false,
            }
        }

        #[test]
        fn output_menu_uses_physical_ids_and_escapes_and_disambiguates_labels() {
            let choices = output_choices(
                &[
                    endpoint("virtual", "Cable", true),
                    endpoint("stable-a", "Headphones & DAC", false),
                    endpoint("stable-b", "Headphones & DAC", false),
                    endpoint("stable-a", "Repeated device", false),
                ],
                Some("stable-b"),
            );
            assert_eq!(choices.len(), 2);
            assert_eq!(choices[0].label, "Headphones && DAC [stable-a]");
            assert_eq!(choices[1].label, "Headphones && DAC [stable-b]");
            assert_eq!(choices[1].endpoint_id, "stable-b");
            assert!(!choices[0].checked);
            assert!(choices[1].checked);
            assert_eq!(menu_text("A\0B\t&C"), "A B &&C");
        }

        #[test]
        fn output_menu_command_range_is_bounded_and_separate_from_host_actions() {
            let outputs: Vec<_> = (0..OUTPUT_LIMIT + 10)
                .map(|index| endpoint(&format!("id-{index}"), "Output", false))
                .collect();
            let choices = output_choices(&outputs, None);
            assert_eq!(choices.len(), OUTPUT_LIMIT);
            assert_eq!(choices.first().unwrap().command_id, OUTPUT_FIRST);
            assert_eq!(
                choices.last().unwrap().command_id,
                OUTPUT_FIRST + OUTPUT_LIMIT - 1
            );
            assert!(
                choices
                    .iter()
                    .all(|choice| choice.command_id != OPEN && choice.command_id != EXIT)
            );
            assert!(output_choices(&[endpoint("v", "Virtual", true)], None).is_empty());
        }

        #[test]
        fn ui_match_requires_the_exact_existing_file() {
            let executable = std::env::current_exe().unwrap();
            assert!(same_executable(&executable, &executable));
            let equivalent = executable
                .parent()
                .unwrap()
                .join(".")
                .join(executable.file_name().unwrap());
            assert!(same_executable(&executable, &equivalent));
            assert!(!same_executable(
                &executable,
                &executable.with_file_name("another-anada.exe")
            ));
            assert!(!same_executable(
                &executable.with_file_name("missing-anada.exe"),
                &executable.with_file_name("missing-anada.exe")
            ));
        }
    }
}
#[cfg(windows)]
pub use platform::Tray;
#[cfg(not(windows))]
pub struct Tray;
#[cfg(not(windows))]
impl Tray {
    pub fn start() -> Result<Self, String> {
        Err("The Anada tray requires Windows".into())
    }
}
