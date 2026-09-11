use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeString,
    NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};
use std::path::PathBuf;

use super::{path_from_file_uri, ClipboardContent};

/// Injects files into macOS NSPasteboard as NSURL fileURLWithPath objects.
pub fn inject_files_to_clipboard(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    autoreleasepool(|_| {
        let pboard = unsafe { NSPasteboard::generalPasteboard() };
        unsafe { pboard.clearContents() };

        let mut url_vec = Vec::with_capacity(paths.len());
        for p in paths {
            let path_str = p.to_str().ok_or_else(|| "Invalid UTF-8 in file path".to_string())?;

            // Safe NSString conversion with internal null termination
            let ns_path = NSString::from_str(path_str);
            let ns_url = unsafe { NSURL::fileURLWithPath(&ns_path) };
            let proto_obj = ProtocolObject::from_retained(ns_url);
            url_vec.push(proto_obj);
        }

        let ns_array = NSArray::from_vec(url_vec);

        // Modern macOS Finder recognizes NSPasteboardTypeFileURL for Cmd+V copying
        let success = unsafe { pboard.writeObjects(&ns_array) };
        if !success {
            return Err("NSPasteboard writeObjects returned NO".into());
        }

        Ok(())
    })
}

/// Reads the macOS system pasteboard: file URLs > PNG/TIFF image > plain text.
pub fn read_clipboard() -> ClipboardContent {
    autoreleasepool(|_| {
        let pboard = unsafe { NSPasteboard::generalPasteboard() };

        // 1. File list: scan pasteboard items for file-url flavors (Finder copy)
        if let Some(items) = unsafe { pboard.pasteboardItems() } {
            let mut paths = Vec::new();
            for i in 0..items.len() {
                let item = unsafe { items.objectAtIndex(i) };
                if let Some(url_str) = unsafe { item.stringForType(&NSPasteboardTypeFileURL) } {
                    if let Some(path) = path_from_file_uri(&url_str.to_string()) {
                        paths.push(PathBuf::from(path));
                    }
                }
            }
            if !paths.is_empty() {
                return ClipboardContent::Files(paths);
            }
        }

        // 2. Image: native PNG first, TIFF (screenshots) decoded via image crate
        if let Some(data) = unsafe { pboard.dataForType(&NSPasteboardTypePNG) } {
            if !data.is_empty() {
                return ClipboardContent::Image(data.bytes().to_vec());
            }
        }
        if let Some(data) = unsafe { pboard.dataForType(&NSPasteboardTypeTIFF) } {
            if !data.is_empty() {
                if let Some(png) = tiff_to_png(data.bytes()) {
                    return ClipboardContent::Image(png);
                }
            }
        }

        // 3. Plain text
        if let Some(s) = unsafe { pboard.stringForType(&NSPasteboardTypeString) } {
            let text = s.to_string();
            if !text.is_empty() {
                return ClipboardContent::Text(text);
            }
        }

        ClipboardContent::Empty
    })
}

fn tiff_to_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(tiff, image::ImageFormat::Tiff).ok()?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

pub fn write_text_to_clipboard(text: &str) -> Result<(), String> {
    autoreleasepool(|_| {
        let pboard = unsafe { NSPasteboard::generalPasteboard() };
        unsafe { pboard.clearContents() };
        let ns_text = NSString::from_str(text);
        let ok = unsafe { pboard.setString_forType(&ns_text, &NSPasteboardTypeString) };
        if ok {
            Ok(())
        } else {
            Err("NSPasteboard setString:forType: returned NO".into())
        }
    })
}

pub fn write_image_to_clipboard(png: &[u8]) -> Result<(), String> {
    autoreleasepool(|_| {
        let pboard = unsafe { NSPasteboard::generalPasteboard() };
        unsafe { pboard.clearContents() };
        let ns_data = NSData::with_bytes(png);
        let ok = unsafe { pboard.setData_forType(Some(&ns_data), &NSPasteboardTypePNG) };
        if ok {
            Ok(())
        } else {
            Err("NSPasteboard setData:forType: returned NO".into())
        }
    })
}
