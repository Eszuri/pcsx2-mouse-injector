use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RID_INPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassExW,
    TranslateMessage, MSG, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

static RAW_HWND: AtomicIsize = AtomicIsize::new(0);

pub struct RawMouseAccumulator {
    pub dx: AtomicI32,
    pub dy: AtomicI32,
}

impl RawMouseAccumulator {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            dx: AtomicI32::new(0),
            dy: AtomicI32::new(0),
        })
    }

    pub fn take(&self) -> (i32, i32) {
        (
            self.dx.swap(0, Ordering::Relaxed),
            self.dy.swap(0, Ordering::Relaxed),
        )
    }

    pub fn reset(&self) {
        self.dx.store(0, Ordering::Relaxed);
        self.dy.store(0, Ordering::Relaxed);
    }
}

static ACCUMULATOR: OnceLock<Arc<RawMouseAccumulator>> = OnceLock::new();

pub fn ensure_raw_input_registered() {
    let raw = RAW_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = raw as HWND;
    let rid = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x02,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    unsafe {
        let res = RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
        if res == 0 {
            static LAST_ERR_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if now.saturating_sub(LAST_ERR_LOG.load(Ordering::Relaxed)) >= 5 {
                LAST_ERR_LOG.store(now, Ordering::Relaxed);
                crate::log(&format!(
                    "ensure_raw_input_registered failed: {}",
                    windows_sys::Win32::Foundation::GetLastError()
                ));
            }
        }
    }
}

unsafe extern "system" fn raw_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    const WM_INPUT: u32 = 0x00FF;

    if msg == WM_INPUT {
        #[repr(C, align(8))]
        struct RawBuffer {
            bytes: [u8; 256],
        }
        let mut raw_buf = RawBuffer { bytes: [0u8; 256] };
        let mut size = std::mem::size_of::<RawBuffer>() as u32;
        let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;

        unsafe {
            let res = GetRawInputData(
                lparam as HRAWINPUT,
                RID_INPUT,
                raw_buf.bytes.as_mut_ptr() as *mut c_void,
                &mut size,
                header_size,
            );

            if res != u32::MAX && res > 0 {
                let raw = &*(raw_buf.bytes.as_ptr() as *const RAWINPUT);
                if raw.header.dwType == 0 {
                    let mouse = &raw.data.mouse;
                    let last_x = mouse.lLastX;
                    let last_y = mouse.lLastY;
                    // Ignore absolute coordinates (MOUSE_MOVE_ABSOLUTE = 0x01)
                    if (mouse.usFlags & 0x01) == 0
                        && (last_x != 0 || last_y != 0)
                        && let Some(acc) = ACCUMULATOR.get()
                    {
                        acc.dx.fetch_add(last_x, Ordering::Relaxed);
                        acc.dy.fetch_add(last_y, Ordering::Relaxed);

                        static RAW_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                        let c = RAW_COUNT.fetch_add(1, Ordering::Relaxed);
                        if c < 5 || c % 1000 == 0 {
                            crate::log(&format!(
                                "[RAW_INPUT] #{} dx={}, dy={}, flags=0x{:X}",
                                c + 1, last_x, last_y, mouse.usFlags
                            ));
                        }
                    }
                }
            }
        }
        return 0;
    }

    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

pub fn start_raw_input_listener(acc: Arc<RawMouseAccumulator>) -> std::thread::JoinHandle<()> {
    let _ = ACCUMULATOR.set(acc);

    std::thread::spawn(|| unsafe {
        let instance = GetModuleHandleW(null_mut());
        let class_name: Vec<u16> = "Pcsx2RawMouseListener\0".encode_utf16().collect();
        let win_name: Vec<u16> = "Pcsx2RawMouseTopWindow\0".encode_utf16().collect();

        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(raw_wnd_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class_name.as_ptr();

        let atom = RegisterClassExW(&wc);
        crate::log(&format!("RawListener RegisterClassExW atom: {}", atom));

        // Create top-level popup window with NULL parent (required for RIDEV_INPUTSINK)
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name.as_ptr(),
            win_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null_mut(),
        );

        crate::log(&format!("RawListener CreateWindowExW hwnd: {:?}", hwnd));
        if hwnd.is_null() {
            crate::log(&format!("Failed CreateWindowExW: {}", windows_sys::Win32::Foundation::GetLastError()));
            return;
        }

        RAW_HWND.store(hwnd as isize, Ordering::SeqCst);

        let rid = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };

        let reg = RegisterRawInputDevices(
            &rid,
            1,
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        );
        crate::log(&format!("Initial RegisterRawInputDevices hwnd ({:?}): {}", hwnd, reg));
        if reg == 0 {
            crate::log(&format!("Failed RegisterRawInputDevices: {}", windows_sys::Win32::Foundation::GetLastError()));
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    })
}
