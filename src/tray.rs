//! Tray icon: the procedural mic glyph and the state -> color mapping.

use tray_icon::Icon;

/// Worker -> UI-thread events (drives the tray icon state).
/// Listening = capturing mic, Processing = uploading/transcribing, Idle = done.
pub enum UiEvent {
    Listening,
    Processing,
    Idle,
}

/// Tray mic-glyph color: blue = idle, red = listening, amber = processing.
const IDLE_RGBA: [u8; 4] = [0x2e, 0x7d, 0xff, 0xff];
const LISTENING_RGBA: [u8; 4] = [0xff, 0x3b, 0x30, 0xff];
const PROCESSING_RGBA: [u8; 4] = [0xff, 0xa5, 0x00, 0xff];

/// The idle (blue) icon, for the initial tray build.
pub fn idle_icon() -> Icon {
    make_icon(IDLE_RGBA)
}

/// The icon for a given UI state (called on the UI thread when a worker event arrives).
pub fn icon_for_event(ui: &UiEvent) -> Icon {
    let rgba = match ui {
        UiEvent::Listening => LISTENING_RGBA,
        UiEvent::Processing => PROCESSING_RGBA,
        UiEvent::Idle => IDLE_RGBA,
    };
    make_icon(rgba)
}

/// Draw a 32x32 microphone glyph in `rgba` on a transparent background.
/// Color encodes state (IDLE_RGBA = idle, LISTENING_RGBA = listening).
///
///   ▟▙     capsule (rounded mic body)
///  (   )   cradle  (U-ring around the lower body)
///    |     stem
///   ───    base
fn make_icon(rgba: [u8; 4]) -> Icon {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4]; // transparent background
    let cx = 16.0_f32;
    for y in 0..S {
        for x in 0..S {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let dx = fx - cx;
            // Mic capsule: rounded vertical body (segment (16,9)-(16,15), r=5).
            let cyy = fy.clamp(9.0, 15.0);
            let head = (dx * dx + (fy - cyy) * (fy - cyy)).sqrt() <= 5.0;
            // Cradle: lower half of a ring (r≈8) around (16,15).
            let dr = (dx * dx + (fy - 15.0) * (fy - 15.0)).sqrt();
            let cradle = (15.0..=23.0).contains(&fy) && (dr - 8.0).abs() <= 1.5;
            // Stand: stem + base.
            let stem = dx.abs() <= 1.5 && (22.0..=27.0).contains(&fy);
            let base = dx.abs() <= 6.0 && (26.0..=28.0).contains(&fy);
            if head || cradle || stem || base {
                let i = (y * S + x) * 4;
                px[i..i + 4].copy_from_slice(&rgba);
            }
        }
    }
    Icon::from_rgba(px, S as u32, S as u32).expect("icon from_rgba")
}
