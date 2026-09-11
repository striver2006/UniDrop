use std::time::Duration;
use tokio::sync::mpsc;
use objc2_app_kit::NSPasteboard;
use objc2_foundation::NSString;

pub struct ClipboardChangeEvent {
    pub text_content: Option<String>,
}

pub fn start_clipboard_listener(tx: mpsc::Sender<ClipboardChangeEvent>) {
    std::thread::spawn(move || {
        let mut last_change_count: isize = -1;

        loop {
            std::thread::sleep(Duration::from_millis(500));

            let pboard = unsafe { NSPasteboard::generalPasteboard() };
            let current_count = unsafe { pboard.changeCount() };

            if current_count != last_change_count {
                last_change_count = current_count;

                // 1. Check for password manager concealed type (1Password, Bitwarden, etc.)
                let concealed_tag = NSString::from_str("org.nspasteboard.ConcealedType");
                let onepass_tag = NSString::from_str("com.agilebits.onepassword");

                let types = unsafe { pboard.types() };
                let is_concealed = types.map_or(false, |arr| {
                    unsafe { arr.containsObject(&concealed_tag) || arr.containsObject(&onepass_tag) }
                });

                if is_concealed {
                    continue; // Skip sensitive clipboard data
                }

                // 2. Read plain text if available
                let ns_str = unsafe { pboard.stringForType(&objc2_app_kit::NSPasteboardTypeString) };
                let text = ns_str.map(|s| s.to_string());

                if text.is_some() {
                    let _ = tx.blocking_send(ClipboardChangeEvent { text_content: text });
                }
            }
        }
    });
}
