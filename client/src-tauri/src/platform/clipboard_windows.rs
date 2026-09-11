use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};

const CF_HDROP: u32 = 15;
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;

use super::ClipboardContent;

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

/// Copies the bytes behind a clipboard HGLOBAL into a Vec.
unsafe fn read_global_bytes(handle: HANDLE) -> Result<Vec<u8>, String> {
    let h_global = HGLOBAL(handle.0);
    let ptr = GlobalLock(h_global) as *const u8;
    if ptr.is_null() {
        return Err("GlobalLock returned null".into());
    }
    // GlobalSize gives the allocated byte length of the clipboard object.
    let size = windows::Win32::System::Memory::GlobalSize(h_global);
    let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
    let _ = GlobalUnlock(h_global);
    Ok(bytes)
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

/// Allocates a global buffer, copies bytes into it and calls SetClipboardData.
/// The clipboard takes ownership of the memory on success.
unsafe fn set_clipboard_bytes(format: u32, bytes: &[u8]) -> Result<(), String> {
    let h_global = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))
        .map_err(|e| format!("GlobalAlloc failed: {:?}", e))?;
    let p_mem = GlobalLock(h_global) as *mut u8;
    if p_mem.is_null() {
        let _ = GlobalFree(h_global);
        return Err("GlobalLock returned null".into());
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p_mem, bytes.len());
    let _ = GlobalUnlock(h_global);

    if SetClipboardData(format, HANDLE(h_global.0)).is_err() {
        let _ = GlobalFree(h_global);
        return Err(format!("SetClipboardData for format {} failed", format));
    }
    Ok(())
}

fn register_format(name: &str) -> u32 {
    let wide: Vec<u16> = OsStr::new(name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(wide.as_ptr())) }
}

/// Reads the Windows clipboard: file list > image (PNG format, then CF_DIB) > text.
pub fn read_clipboard() -> ClipboardContent {
    unsafe {
        if open_clipboard_with_retry(HWND::default(), 5).is_err() {
            return ClipboardContent::Empty;
        }
        let result = read_clipboard_inner();
        let _ = CloseClipboard();
        result
    }
}

unsafe fn read_clipboard_inner() -> ClipboardContent {
    // 1. File list via CF_HDROP
    if IsClipboardFormatAvailable(CF_HDROP).is_ok() {
        if let Ok(handle) = GetClipboardData(CF_HDROP) {
            let hdrop = HDROP(handle.0 as *mut core::ffi::c_void);
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            let mut paths = Vec::new();
            for i in 0..count {
                // First pass with None: path length excluding the NUL terminator
                let len = DragQueryFileW(hdrop, i, None);
                if len == 0 {
                    continue;
                }
                let mut buf = vec![0u16; (len + 1) as usize];
                let copied = DragQueryFileW(hdrop, i, Some(&mut buf));
                let end = buf.iter().position(|&c| c == 0).unwrap_or(copied as usize);
                let path = String::from_utf16_lossy(&buf[..end]);
                if !path.is_empty() {
                    paths.push(PathBuf::from(path));
                }
            }
            if !paths.is_empty() {
                return ClipboardContent::Files(paths);
            }
        }
    }

    // 2. Image: applications often expose raw "PNG" alongside CF_DIB
    let png_format = register_format("PNG");
    if png_format != 0 && IsClipboardFormatAvailable(png_format).is_ok() {
        if let Ok(handle) = GetClipboardData(png_format) {
            if let Ok(bytes) = read_global_bytes(handle) {
                if bytes.len() > 8 && bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
                    return ClipboardContent::Image(bytes);
                }
            }
        }
    }
    if IsClipboardFormatAvailable(CF_DIB).is_ok() {
        if let Ok(handle) = GetClipboardData(CF_DIB) {
            if let Ok(dib) = read_global_bytes(handle) {
                if let Some(png) = dib_to_png(&dib) {
                    return ClipboardContent::Image(png);
                }
            }
        }
    }

    // 3. Text via CF_UNICODETEXT
    if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
        if let Ok(handle) = GetClipboardData(CF_UNICODETEXT) {
            if let Ok(bytes) = read_global_bytes(handle) {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
                let text = String::from_utf16_lossy(&units[..end]);
                if !text.is_empty() {
                    return ClipboardContent::Text(text);
                }
            }
        }
    }

    ClipboardContent::Empty
}

/// Converts a bare DIB (BITMAPINFOHEADER + palette + pixels) to PNG bytes by
/// prepending a BMP file header and decoding via the image crate.
fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let read_u32 = |off: usize| u32::from_le_bytes([dib[off], dib[off + 1], dib[off + 2], dib[off + 3]]);
    let read_u16 = |off: usize| u16::from_le_bytes([dib[off], dib[off + 1]]);
    let bi_size = read_u32(0);
    let bpp = read_u16(14);
    let compression = read_u32(16);
    let clr_used = read_u32(32);
    if compression != 0 || (bpp != 24 && bpp != 32) {
        return None; // BI_RGB 24/32bpp only
    }
    let palette = if clr_used != 0 {
        clr_used
    } else if bpp <= 8 {
        1u32 << bpp
    } else {
        0
    };
    let pixel_offset = bi_size + palette * 4;
    if (pixel_offset as usize) >= dib.len() {
        return None;
    }

    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(14u32 + dib.len() as u32).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&(14u32 + pixel_offset).to_le_bytes());
    bmp.extend_from_slice(dib);

    let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).ok()?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

pub fn write_text_to_clipboard(text: &str) -> Result<(), String> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for u in &units {
        bytes.extend_from_slice(&u.to_le_bytes());
    }

    unsafe {
        open_clipboard_with_retry(HWND::default(), 5)?;
        if let Err(e) = EmptyClipboard() {
            let _ = CloseClipboard();
            return Err(format!("EmptyClipboard failed: {:?}", e));
        }
        let res = set_clipboard_bytes(CF_UNICODETEXT, &bytes);
        let _ = CloseClipboard();
        res?;
    }
    Ok(())
}

pub fn write_image_to_clipboard(png: &[u8]) -> Result<(), String> {
    let img = image::load_from_memory(png)
        .map_err(|e| format!("invalid PNG data: {}", e))?
        .to_rgba8();
    let (w, h) = img.dimensions();

    // Build a bottom-up 32bpp BI_RGB DIB
    let mut dib = Vec::with_capacity(40 + (w as usize) * (h as usize) * 4);
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(w as i32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&(h as i32).to_le_bytes()); // biHeight (positive = bottom-up)
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&0u32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    for y in (0..h).rev() {
        for x in 0..w {
            let p = img.get_pixel(x, y);
            dib.extend_from_slice(&[p[2], p[1], p[0], 255]); // BGRA
        }
    }

    let png_format = register_format("PNG");

    unsafe {
        open_clipboard_with_retry(HWND::default(), 5)?;
        if let Err(e) = EmptyClipboard() {
            let _ = CloseClipboard();
            return Err(format!("EmptyClipboard failed: {:?}", e));
        }
        let dib_res = set_clipboard_bytes(CF_DIB, &dib);
        // Also expose raw PNG for apps that prefer it; failure is non-fatal.
        if png_format != 0 {
            let _ = set_clipboard_bytes(png_format, png);
        }
        let _ = CloseClipboard();
        dib_res?;
    }
    Ok(())
}
