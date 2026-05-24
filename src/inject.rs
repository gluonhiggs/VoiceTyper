//! Inject text into the focused window via clipboard paste + synthetic Ctrl+V.

use std::thread;
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

/// Paste `text` into the focused window: snapshot the clipboard, set our text,
/// send Ctrl+V, then restore the old clipboard. Text-only restore for now.
pub fn inject(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    if text.is_empty() {
        return Ok(());
    }
    let mut clipboard = arboard::Clipboard::new()?;
    let previous = clipboard.get_text().ok();
    clipboard.set_text(text.to_string())?;
    thread::sleep(Duration::from_millis(30)); // let the clipboard settle
    wait_modifiers_released(); // avoid a held Win key turning Ctrl+V into Win+V
    send_ctrl_v();
    thread::sleep(Duration::from_millis(150)); // let the target app paste
    if let Some(prev) = previous {
        let _ = clipboard.set_text(prev);
    }
    Ok(())
}

/// Spin (briefly) until Ctrl/Win/Alt/Shift are all up, so the synthetic Ctrl+V
/// isn't poisoned by a still-held modifier (e.g. Win+V opens clipboard history).
fn wait_modifiers_released() {
    let down = |vk: VIRTUAL_KEY| unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 };
    for _ in 0..50 {
        if !down(VK_CONTROL)
            && !down(VK_LWIN)
            && !down(VK_RWIN)
            && !down(VK_MENU)
            && !down(VK_SHIFT)
        {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Synthesize a Ctrl+V keystroke via SendInput.
fn send_ctrl_v() {
    const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
    let inputs = [
        key_event(VK_CONTROL, false),
        key_event(VK_V, false),
        key_event(VK_V, true),
        key_event(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn key_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
