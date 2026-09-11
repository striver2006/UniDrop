use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSPasteboard;
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::PathBuf;

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
