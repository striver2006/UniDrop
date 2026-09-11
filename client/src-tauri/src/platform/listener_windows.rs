use tokio::sync::mpsc;

pub struct ClipboardChangeEvent {
    pub text_content: Option<String>,
}

pub fn start_clipboard_listener(_tx: mpsc::Sender<ClipboardChangeEvent>) {
    // Windows implementation hooks AddClipboardFormatListener(hwnd) via hidden message window
    // and checks for "Clipboard Viewer Ignore" format.
}
