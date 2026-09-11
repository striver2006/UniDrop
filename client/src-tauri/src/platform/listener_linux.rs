use tokio::sync::mpsc;

pub struct ClipboardChangeEvent {
    pub text_content: Option<String>,
}

pub fn start_clipboard_listener(_tx: mpsc::Sender<ClipboardChangeEvent>) {
    // Linux implementation listens to X11 XFixes or Wayland data control protocols
}
