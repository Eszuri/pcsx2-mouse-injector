use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RID_INPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassExW,
    TranslateMessage, MSG, WNDCLASSEXW, WS_POPUP,
};

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

unsafe extern "system" fn raw_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    const WM_INPUT: u32 = 0x00FF;

    if msg == WM_INPUT {
        let mut buf = [0u8; 128];
        let mut size = buf.len() as u32;
        let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;

        unsafe {
            let res = GetRawInputData(
                lparam as HRAWINPUT,
                RID_INPUT,
                buf.as_mut_ptr() as *mut c_void,
                &mut size,
                header_size,
            );

            if res != u32::MAX && res > 0 {
                let raw = &*(buf.as_ptr() as *const RAWINPUT);
                if raw.header.dwType == 0 {
                    let mouse = &raw.data.mouse;
                    let last_x = mouse.lLastX;
                    let last_y = mouse.lLastY;
                    if (last_x != 0 || last_y != 0)
                        && let Some(acc) = ACCUMULATOR.get()
                    {
                        acc.dx.fetch_add(last_x, Ordering::Relaxed);
                        acc.dy.fetch_add(last_y, Ordering::Relaxed);
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

        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(raw_wnd_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class_name.as_ptr();

        let atom = RegisterClassExW(&wc);
        crate::log(&format!("TopLevel RegisterClassExW atom: {}", atom));

        // Create top-level popup window (NULL parent) so RIDEV_INPUTSINK works properly
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(), // Top-level window!
            null_mut(),
            instance,
            null_mut(),
        );

        crate::log(&format!("TopLevel CreateWindowExW hwnd: {:?}", hwnd));
        if hwnd.is_null() {
            crate::log(&format!("Failed CreateWindowExW: {}", windows_sys::Win32::Foundation::GetLastError()));
            return;
        }

        let rid = RAWINPUTDEVICE {
            usUsagePage: 0x01, // Generic Desktop
            usUsage: 0x02,     // Mouse
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };

        let reg = RegisterRawInputDevices(
            &rid,
            1,
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        );
        crate::log(&format!("RegisterRawInputDevices on top-level hwnd: {}", reg));

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    })
}
