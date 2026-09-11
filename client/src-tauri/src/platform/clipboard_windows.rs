use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::UI::Shell::DROPFILES;

const CF_HDROP: u32 = 15;

fn open_clipboard_with_retry(hwnd: HWND, max_retries: u32) -> Result<(), String> {
    for attempt in 0..max_retries {
        unsafe {
            if OpenClipboard(hwnd).is_ok() {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(30 * (attempt + 1) as u64));
    }
    Err("OpenClipboard locked by other application after retries".into())
}

pub fn inject_files_to_clipboard(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    // 1. Serialize paths to UTF-16 separated by \0, terminated by double \0\0
    let mut buffer_u16: Vec<u16> = Vec::new();
    for p in paths {
        let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        buffer_u16.extend(wide);
    }
    buffer_u16.push(0); // Final terminating null

    let dropfiles_size = std::mem::size_of::<DROPFILES>();
    let total_size = dropfiles_size + buffer_u16.len() * 2;

    unsafe {
        // 2. Allocate global movable memory
        let h_global: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, total_size)
            .map_err(|e| format!("GlobalAlloc failed: {:?}", e))?;

        let p_mem = GlobalLock(h_global) as *mut u8;
        if p_mem.is_null() {
            let _ = GlobalFree(h_global);
            return Err("GlobalLock returned null".into());
        }

        // 3. Populate DROPFILES header
        let dropfiles = p_mem as *mut DROPFILES;
        (*dropfiles).pFiles = dropfiles_size as u32;
        (*dropfiles).pt.x = 0;
        (*dropfiles).pt.y = 0;
        (*dropfiles).fNC = false.into();
        (*dropfiles).fWide = true.into(); // Unicode UTF-16

        std::ptr::copy_nonoverlapping(
            buffer_u16.as_ptr() as *const u8,
            p_mem.add(dropfiles_size),
            buffer_u16.len() * 2,
        );

        let _ = GlobalUnlock(h_global);

        // 4. Open and empty clipboard
        if let Err(e) = open_clipboard_with_retry(HWND::default(), 5) {
            let _ = GlobalFree(h_global);
            return Err(e);
        }

        if let Err(e) = EmptyClipboard() {
            let _ = CloseClipboard();
            let _ = GlobalFree(h_global);
            return Err(format!("EmptyClipboard failed: {:?}", e));
        }

        // 5. Set CF_HDROP data
        if SetClipboardData(CF_HDROP, HANDLE(h_global.0)).is_err() {
            let _ = CloseClipboard();
            let _ = GlobalFree(h_global);
            return Err("SetClipboardData for CF_HDROP failed".into());
        }

        // 6. Set Preferred DropEffect: DROPEFFECT_COPY (0x01)
        let _ = inject_drop_effect(1);

        let _ = CloseClipboard();
    }

    Ok(())
}

unsafe fn inject_drop_effect(effect: u32) -> Result<(), String> {
    let name: Vec<u16> = OsStr::new("Preferred DropEffect")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let format_id = RegisterClipboardFormatW(windows::core::PCWSTR(name.as_ptr()));
    if format_id == 0 {
        return Err("Register format failed".into());
    }

    let h_global = GlobalAlloc(GMEM_MOVEABLE, 4).map_err(|e| format!("{:?}", e))?;
    let p_mem = GlobalLock(h_global) as *mut u32;
    if p_mem.is_null() {
        let _ = GlobalFree(h_global);
        return Err("Lock failed".into());
    }
    *p_mem = effect;
    let _ = GlobalUnlock(h_global);

    if SetClipboardData(format_id, HANDLE(h_global.0)).is_err() {
        let _ = GlobalFree(h_global);
        return Err("Set DropEffect failed".into());
    }
    Ok(())
}
