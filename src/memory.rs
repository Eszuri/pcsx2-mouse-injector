use std::ffi::c_void;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{CloseHandle, FreeLibrary, HANDLE, INVALID_HANDLE_VALUE};
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
    GetCurrentProcessId, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION,
    PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const MEM_COMMIT: u32 = 0x1000;
const PS2_MAX_MEM_ADDR: u64 = 0x0800_0000; // 128 MB maximum supported RAM range

pub struct Pcsx2Process {
    pub pid: u32,
    pub name: String,
    pub handle: HANDLE,
    pub ram_base: u64,
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

impl Pcsx2Process {
    pub fn find_and_attach() -> Option<Self> {
        unsafe {
            let my_pid = GetCurrentProcessId();

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

            let mut target_pid = 0u32;
            let mut target_name = String::new();

            loop {
                // Must not match our own process!
                if entry.th32ProcessID != my_pid {
                    let null_pos = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let exe_name = String::from_utf16_lossy(&entry.szExeFile[..null_pos]);
                    let clean_name = exe_name.trim().to_string();
                    let lower = clean_name.to_lowercase();

                    // Filter out any mouse-injector executable and match actual PCSX2 emulator
                    let is_not_injector = !lower.contains("injector");
                    let is_pcsx2 = lower == "pcsx2.exe"
                        || lower == "pcsx2-qt.exe"
                        || lower == "pcsx2-wx.exe"
                        || lower == "pcsx2x64.exe"
                        || lower == "pcsx2-avx2.exe"
                        || lower == "pcsx2-sse4.exe"
                        || (lower.contains("pcsx2") && lower.contains(".exe"));

                    if is_not_injector && is_pcsx2 {
                        target_pid = entry.th32ProcessID;
                        target_name = clean_name;
                        break;
                    }
                }

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }

            CloseHandle(snapshot);

            if target_pid == 0 {
                return None;
            }

            let desired_access = PROCESS_VM_OPERATION
                | PROCESS_VM_READ
                | PROCESS_VM_WRITE
                | PROCESS_QUERY_INFORMATION;

            let handle = OpenProcess(desired_access, 0, target_pid);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return None;
            }

            let mut proc = Self {
                pid: target_pid,
                name: target_name,
                handle,
                ram_base: 0,
            };

            proc.find_eemem_base();
            Some(proc)
        }
    }

    pub fn is_still_alive(&self) -> bool {
        if self.handle.is_null() || self.handle == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut exit_code = 0u32;
        unsafe {
            windows_sys::Win32::System::Threading::GetExitCodeProcess(self.handle, &mut exit_code) != 0
                && exit_code == 259 // STILL_ACTIVE = 259
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

    pub fn find_eemem_base(&mut self) -> bool {
        if self.handle.is_null() || self.handle == INVALID_HANDLE_VALUE {
            return false;
        }

        unsafe {
            // =========================================================================
            // Strategy 1: Dynamic lookup of exported "EEmem" symbol (PCSX2 2.0+ / 64-bit Qt)
            // =========================================================================
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
                            let mut target_val: u64 = 0;
                            let mut bytes_read = 0usize;
                            if ReadProcessMemory(
                                self.handle,
                                ((h_mods[0] as usize) + rva) as *const c_void,
                                &mut target_val as *mut _ as *mut c_void,
                                std::mem::size_of::<u64>(),
                                &mut bytes_read,
                            ) != 0
                                && target_val != 0
                            {
                                FreeLibrary(local_hmod);
                                self.ram_base = target_val;
                                crate::log(&format!("PCSX2 >= 2.0 (EEmem export) attached at 0x{:X}", self.ram_base));
                                return true;
                            }
                        }
                        FreeLibrary(local_hmod);
                    }
                }
            }

            // =========================================================================
            // Strategy 2: Direct 0x20000000 check for PCSX2 1.x (1.6.0, 1.4.0, etc. 32-bit)
            // In PCSX2 1.6.0, EE RAM is permanently mapped at virtual address 0x20000000.
            // =========================================================================
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

            // =========================================================================
            // Strategy 3: Heuristic scan for 32MB / 64MB / 128MB mapped region or 0x20000000 alloc
            // =========================================================================
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
