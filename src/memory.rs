use std::ffi::c_void;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{
    CloseHandle, FreeLibrary, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExA, DONT_RESOLVE_DLL_REFERENCES,
};
use windows_sys::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_MAPPED, MEM_PRIVATE,
};
use windows_sys::Win32::System::ProcessStatus::{EnumProcessModules, GetModuleFileNameExA};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION,
    PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, GA_ROOTOWNER,
};

const MEM_COMMIT: u32 = 0x1000;
const PS2_MAX_MEM_ADDR: u64 = 0x0800_0000;

pub struct Pcsx2Process {
    pub pid: u32,
    pub name: String,
    pub handle: HANDLE,
    pub ram_base: u64,
    pub eemem_ptr_remote: u64,
    pub last_cpu_sample: std::time::Instant,
    pub last_cpu_time: u64,
    pub consecutive_idle_samples: u32,
    pub is_paused: bool,
}

unsafe impl Send for Pcsx2Process {}
unsafe impl Sync for Pcsx2Process {}

impl Drop for Pcsx2Process {
    fn drop(&mut self) {
        if !self.handle.is_null() && self.handle != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.handle);
            }
            self.handle = null_mut();
        }
    }
}

pub struct Pcsx2Memory<'a> {
    pub handle: HANDLE,
    pub ram_base: u64,
    _marker: std::marker::PhantomData<&'a ()>,
}

pub fn is_pcsx2_exe_name(name: &str) -> bool {
    let clean = name
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' && c != '_')
        .to_lowercase();
    let is_not_injector = !clean.contains("injector");
    let is_pcsx2 = clean == "pcsx2.exe"
        || clean == "pcsx2-qt.exe"
        || clean == "pcsx2-wx.exe"
        || clean == "pcsx2x64.exe"
        || clean == "pcsx2-avx2.exe"
        || clean == "pcsx2-sse4.exe"
        || (clean.contains("pcsx2") && clean.contains(".exe"));
    is_not_injector && is_pcsx2
}

pub fn get_process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        let handle = if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            OpenProcess(0x1000, 0, pid)
        } else {
            handle
        };

        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let mut buf = [0u8; 512];
        let len = GetModuleFileNameExA(handle, null_mut(), buf.as_mut_ptr(), buf.len() as u32);
        CloseHandle(handle);

        if len > 0 {
            let path_str = String::from_utf8_lossy(&buf[..len as usize]);
            let name = std::path::Path::new(path_str.as_ref())
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let clean = name
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' && c != '_')
                .to_string();
            if !clean.is_empty() {
                Some(clean)
            } else {
                None
            }
        } else {
            None
        }
    }
}

fn get_process_cpu_time(handle: HANDLE) -> u64 {
    unsafe {
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();

        if GetProcessTimes(
            handle,
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        ) != 0 {
            let k = ((kernel.dwHighDateTime as u64) << 32) | (kernel.dwLowDateTime as u64);
            let u = ((user.dwHighDateTime as u64) << 32) | (user.dwLowDateTime as u64);
            k.saturating_add(u)
        } else {
            0
        }
    }
}

impl Pcsx2Process {
    pub fn attach_pid(target_pid: u32, target_name: String) -> Option<Self> {
        unsafe {
            let desired_access = PROCESS_VM_OPERATION
                | PROCESS_VM_READ
                | PROCESS_VM_WRITE
                | PROCESS_QUERY_INFORMATION;

            let handle = OpenProcess(desired_access, 0, target_pid);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return None;
            }

            let initial_cpu = get_process_cpu_time(handle);
            let mut proc = Self {
                pid: target_pid,
                name: target_name,
                handle,
                ram_base: 0,
                eemem_ptr_remote: 0,
                last_cpu_sample: std::time::Instant::now(),
                last_cpu_time: initial_cpu,
                consecutive_idle_samples: 0,
                is_paused: false,
            };

            proc.find_eemem_base();
            Some(proc)
        }
    }

    pub fn find_and_attach() -> Option<Self> {
        let my_pid = unsafe { GetCurrentProcessId() };

        unsafe {
            let fg = GetForegroundWindow();
            if !fg.is_null() {
                let mut fg_pid = 0u32;
                GetWindowThreadProcessId(fg, &mut fg_pid);
                if fg_pid != 0
                    && fg_pid != my_pid
                    && let Some(name) = get_process_name(fg_pid)
                    && is_pcsx2_exe_name(&name)
                    && let Some(proc) = Self::attach_pid(fg_pid, name)
                {
                    return Some(proc);
                }
            }
        }

        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return None;
            }

            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snapshot, &mut entry) == 0 {
                CloseHandle(snapshot);
                return None;
            }

            let mut candidates: Vec<(u32, String)> = Vec::new();

            loop {
                if entry.th32ProcessID != my_pid {
                    let null_pos = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let exe_name = String::from_utf16_lossy(&entry.szExeFile[..null_pos]);
                    let clean_name = exe_name.trim().to_string();

                    if is_pcsx2_exe_name(&clean_name) {
                        candidates.push((entry.th32ProcessID, clean_name));
                    }
                }

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
            CloseHandle(snapshot);

            candidates.sort_by(|a, b| {
                let a_is_qt = a.1.to_lowercase().contains("-qt");
                let b_is_qt = b.1.to_lowercase().contains("-qt");
                b_is_qt.cmp(&a_is_qt).then_with(|| b.0.cmp(&a.0))
            });

            for (pid, name) in candidates {
                if let Some(proc) = Self::attach_pid(pid, name) {
                    return Some(proc);
                }
            }
        }

        None
    }

    pub fn is_still_alive(&self) -> bool {
        if self.handle.is_null() || self.handle == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut exit_code = 0u32;
        unsafe {
            windows_sys::Win32::System::Threading::GetExitCodeProcess(self.handle, &mut exit_code) != 0
                && exit_code == 259
        }
    }

    pub fn is_focused(&self) -> bool {
        if self.pid == 0 {
            return false;
        }
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() {
                return false;
            }
            let mut fg_pid = 0u32;
            GetWindowThreadProcessId(fg, &mut fg_pid);
            fg_pid == self.pid
        }
    }

    pub fn update_pause_state(&mut self) -> bool {
        if self.pid == 0 || self.handle.is_null() || self.handle == INVALID_HANDLE_VALUE {
            self.is_paused = false;
            return false;
        }

        let title_paused = unsafe {
            let fg = GetForegroundWindow();
            if !fg.is_null() {
                let mut fg_pid = 0u32;
                GetWindowThreadProcessId(fg, &mut fg_pid);
                if fg_pid == self.pid {
                    let check_title = |hwnd: windows_sys::Win32::Foundation::HWND| -> bool {
                        let mut buf = [0u16; 512];
                        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
                        if len > 0 {
                            let title = String::from_utf16_lossy(&buf[..len as usize]);
                            let lower = title.to_lowercase();
                            if lower.contains("paused")
                                || lower.contains("speed: 0%")
                                || lower.contains("fps: 0.00")
                                || lower.contains("fps: 0.0")
                            {
                                return true;
                            }
                        }
                        false
                    };

                    if check_title(fg) {
                        true
                    } else {
                        let root = GetAncestor(fg, GA_ROOTOWNER);
                        !root.is_null() && root != fg && check_title(root)
                    }
                } else {
                    false
                }
            } else {
                false
            }
        };

        if title_paused {
            self.is_paused = true;
            self.consecutive_idle_samples = 2;
            return true;
        }

        let elapsed = self.last_cpu_sample.elapsed();
        if elapsed >= std::time::Duration::from_millis(250) {
            self.last_cpu_sample = std::time::Instant::now();
            let cur_cpu = get_process_cpu_time(self.handle);
            let cpu_delta = cur_cpu.saturating_sub(self.last_cpu_time);
            self.last_cpu_time = cur_cpu;

            let elapsed_units = (elapsed.as_nanos() / 100) as u64;

            const IDLE_CPU_THRESHOLD: u64 = 500_000;
            let is_idle = cpu_delta < IDLE_CPU_THRESHOLD;

            let was_paused = self.is_paused;
            if is_idle {
                self.consecutive_idle_samples = self.consecutive_idle_samples.saturating_add(1);
                if self.consecutive_idle_samples >= 2 {
                    self.is_paused = true;
                }
            } else {
                self.consecutive_idle_samples = 0;
                self.is_paused = false;
            }

            if self.is_paused != was_paused {
                let cpu_pct = if elapsed_units > 0 {
                    (cpu_delta as f64 / elapsed_units as f64) * 100.0
                } else {
                    0.0
                };
                crate::log(&format!(
                    "PCSX2 pause state changed: is_paused = {} (CPU load: {:.1}%, delta: {} units / ~{:.1}ms)",
                    self.is_paused,
                    cpu_pct,
                    cpu_delta,
                    (cpu_delta as f64) / 10_000.0
                ));
            }
        }

        self.is_paused
    }

    pub fn find_eemem_base(&mut self) -> bool {
        if self.handle.is_null() || self.handle == INVALID_HANDLE_VALUE {
            return false;
        }

        unsafe {

            let mut h_mods = [null_mut(); 1024];
            let mut cb_needed = 0u32;
            let ptr_size = std::mem::size_of::<*mut c_void>() as u32;

            if EnumProcessModules(
                self.handle,
                h_mods.as_mut_ptr() as *mut _,
                (h_mods.len() * ptr_size as usize) as u32,
                &mut cb_needed,
            ) != 0
                && cb_needed >= ptr_size
            {
                let mut path_buf = [0u8; 260];
                let path_len =
                    GetModuleFileNameExA(self.handle, h_mods[0], path_buf.as_mut_ptr(), 260);
                if path_len > 0 {
                    let local_hmod = LoadLibraryExA(
                        path_buf.as_ptr(),
                        null_mut(),
                        DONT_RESOLVE_DLL_REFERENCES,
                    );
                    if !local_hmod.is_null() {
                        let sym = b"EEmem\0";
                        if let Some(proc_addr) = GetProcAddress(local_hmod, sym.as_ptr()) {
                            let rva = (proc_addr as usize) - (local_hmod as usize);
                            let remote_ptr = ((h_mods[0] as usize) + rva) as u64;
                            let mut target_val: u64 = 0;
                            let mut bytes_read = 0usize;
                            if ReadProcessMemory(
                                self.handle,
                                remote_ptr as *const c_void,
                                &mut target_val as *mut _ as *mut c_void,
                                std::mem::size_of::<u64>(),
                                &mut bytes_read,
                            ) != 0
                                && target_val != 0
                            {
                                FreeLibrary(local_hmod);
                                self.eemem_ptr_remote = remote_ptr;
                                self.ram_base = target_val;
                                crate::log(&format!("PCSX2 >= 2.0 (EEmem export) attached at 0x{:X}", self.ram_base));
                                return true;
                            }
                        }
                        FreeLibrary(local_hmod);
                    }
                }
            }

            let test_addr: usize = 0x2000_0000;
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            if VirtualQueryEx(
                self.handle,
                test_addr as *const c_void,
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            ) != 0
            {
                let alloc_base = info.AllocationBase as usize;
                let base_addr = info.BaseAddress as usize;
                if (alloc_base == 0x2000_0000 || base_addr == 0x2000_0000)
                    && (info.State == MEM_COMMIT || info.State == 0x1000)
                {
                    let mut test_bytes = [0u8; 4];
                    let mut read = 0usize;
                    if ReadProcessMemory(
                        self.handle,
                        test_addr as *const c_void,
                        test_bytes.as_mut_ptr() as *mut c_void,
                        4,
                        &mut read,
                    ) != 0
                    {
                        self.ram_base = 0x2000_0000;
                        crate::log(&format!("PCSX2 <= 2.0 (1.6.0 32-bit) attached at 0x{:X}", self.ram_base));
                        return true;
                    }
                }
            }

            let mut scan_ptr: usize = 0;
            while VirtualQueryEx(
                self.handle,
                scan_ptr as *const c_void,
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            ) != 0
            {
                let region_size = info.RegionSize;
                let region_type = info.Type;
                let alloc_base = info.AllocationBase as u64;
                scan_ptr = (info.BaseAddress as usize).saturating_add(region_size);

                if alloc_base == 0x2000_0000 && (info.State == MEM_COMMIT || info.State == 0x1000) {
                    self.ram_base = 0x2000_0000;
                    crate::log(&format!("PCSX2 <= 2.0 (AllocBase 0x20000000) attached at 0x{:X}", self.ram_base));
                    return true;
                }

                if (region_size == 0x0200_0000
                    || region_size == 0x0400_0000
                    || region_size == 0x0800_0000)
                    && (region_type == MEM_MAPPED || region_type == MEM_PRIVATE)
                {
                    self.ram_base = info.BaseAddress as u64;
                    crate::log(&format!("PCSX2 memory region attached at 0x{:X}", self.ram_base));
                    return true;
                }
            }
        }

        false
    }

    pub fn is_eemem_valid(&mut self) -> bool {
        if self.handle.is_null() || self.handle == INVALID_HANDLE_VALUE || self.ram_base == 0 {
            return false;
        }

        if self.eemem_ptr_remote != 0 {
            let mut target_val: u64 = 0;
            let mut bytes_read = 0usize;
            unsafe {
                if ReadProcessMemory(
                    self.handle,
                    self.eemem_ptr_remote as *const c_void,
                    &mut target_val as *mut _ as *mut c_void,
                    std::mem::size_of::<u64>(),
                    &mut bytes_read,
                ) != 0
                {
                    if target_val == 0 {
                        self.ram_base = 0;
                        return false;
                    }
                    if target_val != self.ram_base {
                        self.ram_base = target_val;
                    }
                } else {
                    self.ram_base = 0;
                    return false;
                }
            }
        }

        unsafe {
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            if VirtualQueryEx(
                self.handle,
                self.ram_base as *const c_void,
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            ) != 0
            {
                if info.State != MEM_COMMIT {
                    self.ram_base = 0;
                    return false;
                }
            } else {
                self.ram_base = 0;
                return false;
            }
        }

        true
    }

    pub fn is_emulation_running(&self) -> bool {
        if self.pid == 0 || self.handle.is_null() || self.handle == INVALID_HANDLE_VALUE {
            return false;
        }

        struct WindowScanContext {
            target_pid: u32,
            has_running_game: bool,
        }

        unsafe extern "system" fn enum_windows_callback(
            hwnd: windows_sys::Win32::Foundation::HWND,
            lparam: windows_sys::Win32::Foundation::LPARAM,
        ) -> windows_sys::Win32::Foundation::BOOL {
            unsafe {
                let ctx = &mut *(lparam as *mut WindowScanContext);
                let mut win_pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut win_pid);
                if win_pid == ctx.target_pid {
                    let is_vis = IsWindowVisible(hwnd) != 0 || IsIconic(hwnd) != 0;
                    if is_vis {
                        let mut buf = [0u16; 512];
                        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
                        if len > 0 {
                            let title = String::from_utf16_lossy(&buf[..len as usize]);
                            let trimmed = title.trim();
                            let lower = trimmed.to_lowercase();
                            if !lower.is_empty()
                                && lower != "_q_titlebar"
                                && lower != "msctfime ui"
                                && lower != "default ime"
                            {
                                if !lower.starts_with("pcsx2") {
                                    ctx.has_running_game = true;
                                    return 0;
                                }
                                if lower.contains("speed:")
                                    || lower.contains("fps:")
                                    || lower.contains("slus")
                                    || lower.contains("sles")
                                {
                                    ctx.has_running_game = true;
                                    return 0;
                                }
                                if lower.contains(" | ") && !lower.contains("game list") {
                                    ctx.has_running_game = true;
                                    return 0;
                                }
                            }
                        }
                    }
                }
                1
            }
        }

        let mut ctx = WindowScanContext {
            target_pid: self.pid,
            has_running_game: false,
        };

        unsafe {
            EnumWindows(
                Some(enum_windows_callback),
                &mut ctx as *mut WindowScanContext as windows_sys::Win32::Foundation::LPARAM,
            );
        }

        ctx.has_running_game
    }

    pub fn memory(&self) -> Pcsx2Memory<'_> {
        Pcsx2Memory {
            handle: self.handle,
            ram_base: self.ram_base,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<'a> Pcsx2Memory<'a> {
    pub fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> bool {
        if self.ram_base == 0 || (addr as u64) + (buf.len() as u64) > PS2_MAX_MEM_ADDR {
            return false;
        }
        let target = (self.ram_base + (addr as u64)) as *const c_void;
        let mut read = 0usize;
        unsafe {
            ReadProcessMemory(
                self.handle,
                target,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                &mut read,
            ) != 0
        }
    }

    pub fn write_bytes(&self, addr: u32, buf: &[u8]) -> bool {
        if self.ram_base == 0 || (addr as u64) + (buf.len() as u64) > PS2_MAX_MEM_ADDR {
            return false;
        }
        let target = (self.ram_base + (addr as u64)) as *mut c_void;
        let mut written = 0usize;
        unsafe {
            WriteProcessMemory(
                self.handle,
                target,
                buf.as_ptr() as *const c_void,
                buf.len(),
                &mut written,
            ) != 0
        }
    }

    pub fn read_word_swapped(&self, addr: u32) -> u32 {
        let mut buf = [0u8; 4];
        if self.read_bytes(addr, &mut buf) {
            u32::from_be_bytes(buf)
        } else {
            0
        }
    }

    pub fn read_f32(&self, addr: u32) -> f32 {
        let mut buf = [0u8; 4];
        if self.read_bytes(addr, &mut buf) {
            f32::from_le_bytes(buf)
        } else {
            0.0
        }
    }

    pub fn write_f32(&self, addr: u32, val: f32) -> bool {
        self.write_bytes(addr, &val.to_le_bytes())
    }
}
