//! Global Ctrl+Win hotkey via a WH_KEYBOARD_LL low-level keyboard hook.
//!
//! Ctrl+Win is a modifier-only chord, so RegisterHotKey can't bind it; we watch
//! the raw key stream instead and fire once per chord (on key-down, not repeat).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LCONTROL, VK_LWIN, VK_RCONTROL, VK_RWIN};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, SetWindowsHookExW, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::PCWSTR;

/// Messages from the keyboard hook to the worker thread.
pub enum HookMsg {
    Toggle,
}

// The WH_KEYBOARD_LL callback is a plain `extern "system" fn`, so it can't
// capture state. It reaches the rest of the app through these process-lifetime
// statics. No GC, nothing to keep alive but the HHOOK (see `install`).
static HOOK_TX: OnceLock<Sender<HookMsg>> = OnceLock::new();
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static WIN_DOWN: AtomicBool = AtomicBool::new(false);
// True while Ctrl+Win is held, so we fire once per chord, not on key-repeat.
static COMBO_ACTIVE: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let vk = kb.vkCode;
        let msg = wparam.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        let is_ctrl = vk == VK_LCONTROL.0 as u32 || vk == VK_RCONTROL.0 as u32;
        let is_win = vk == VK_LWIN.0 as u32 || vk == VK_RWIN.0 as u32;

        if is_ctrl && is_down {
            CTRL_DOWN.store(true, Ordering::SeqCst);
        } else if is_ctrl && is_up {
            CTRL_DOWN.store(false, Ordering::SeqCst);
        }
        if is_win && is_down {
            WIN_DOWN.store(true, Ordering::SeqCst);
        } else if is_win && is_up {
            WIN_DOWN.store(false, Ordering::SeqCst);
        }

        let chord = CTRL_DOWN.load(Ordering::SeqCst) && WIN_DOWN.load(Ordering::SeqCst);
        if chord {
            if !COMBO_ACTIVE.swap(true, Ordering::SeqCst)
                && let Some(tx) = HOOK_TX.get()
            {
                let _ = tx.send(HookMsg::Toggle);
            }
        } else {
            COMBO_ACTIVE.store(false, Ordering::SeqCst);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Install the global low-level keyboard hook for the process lifetime, routing
/// each Ctrl+Win chord press to `tx`. Panics if Windows refuses the hook.
pub fn install(tx: Sender<HookMsg>) {
    HOOK_TX.set(tx).ok();
    unsafe {
        let hmod = GetModuleHandleW(PCWSTR::null()).expect("GetModuleHandleW failed");
        let hinstance = HINSTANCE(hmod.0);
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(hinstance), 0)
            .expect("SetWindowsHookExW failed");
    }
}
