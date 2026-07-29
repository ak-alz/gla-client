//! Real brand-mark tray icons, replacing the earlier programmatically
//! generated solid-color square. Follows gla-server's
//! `docs/04_DESIGN/DESIGN_GUIDE.md` §9.3/§9.7/§9.9 — the same rules that
//! govern every other consumer of this mark:
//!
//! - 16 px is compact-only, monochrome only, never the gradient mark.
//! - Which monochrome variant (black or white) is chosen by contrast with
//!   the system tray/menu-bar background, not by app status.
//! - No hue/gradient is ever used to encode agent status (paused vs
//!   active) — that would mean inventing a color the brand pack doesn't
//!   define. Status is instead shown by dimming alpha on the SAME mark
//!   (`dim_alpha` below): still monochrome, still the one approved
//!   geometry, just muted — and, as a side effect, exactly how macOS's
//!   own template-image mechanism expects a "de-emphasized" status item
//!   to look (see `tray.rs`'s macOS branch).
//!
//! Source PNGs are the pack's own pre-rendered tray exports
//! (`assets/tray/tray-on-{light,dark}-16.png`, byte-identical to
//! `Growth-Layer-Brand-Assets-v1.0/tray/png/`) — decoded once at process
//! start, not regenerated or hand-edited.

use png::ColorType;

const TRAY_LIGHT_BG_PNG: &[u8] = include_bytes!("../assets/tray/tray-on-light-16.png");
const TRAY_DARK_BG_PNG: &[u8] = include_bytes!("../assets/tray/tray-on-dark-16.png");

pub const SIZE: u32 = 16;

fn decode_rgba(png_bytes: &[u8]) -> Vec<u8> {
    let decoder = png::Decoder::new(png_bytes);
    let mut reader = decoder
        .read_info()
        .expect("bundled tray PNG is well-formed — verified at build time, not user input");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .expect("bundled tray PNG decodes — verified at build time, not user input");
    assert_eq!(
        info.color_type,
        ColorType::Rgba,
        "tray PNG must be RGBA (re-export from the brand pack, don't hand-edit — got {:?})",
        info.color_type
    );
    buf.truncate(info.buffer_size());
    buf
}

/// Halves alpha (integer division, not rounded) — muted, not invisible;
/// still reads as "the agent icon, just paused", not as a rendering glitch.
fn dim_alpha(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|px| [px[0], px[1], px[2], px[3] / 2]).collect()
}

// Real feedback: an available update had no visual signal at all on the
// tray icon itself — only the menu (which requires a click to see) showed
// it. A small corner dot (same idea as Telegram/Discord's unread-badge
// convention) fixes that at a glance. Deliberately a SCOPED exception to
// this module's "never hue-code the mark itself" rule above — that rule
// protects the brand mark's own geometry/color from being reinterpreted
// per status; a small overlay badge next to it (not on top of it, not
// recoloring it) is the same category of UI as the web dashboard's own
// notification dots (see NavigationRail.tsx's `IconWithBadge`), not a
// second attempt at the same thing that rule forbids.
const NOTIFICATION_DOT_COLOR: [u8; 4] = [255, 59, 48, 255]; // opaque red, iOS/Telegram-style notification red
const NOTIFICATION_DOT_RADIUS: f32 = 2.6;
// Center of the dot, in pixel coordinates — top-right corner, matching
// where OS-native badges conventionally sit (bottom-right is already
// visually busy on this mark, see the pulse line's own end point there).
const NOTIFICATION_DOT_CENTER: (f32, f32) = (12.5, 3.5);

/// Overlays a small solid circle in the icon's top-right corner — see the
/// module comment above for why this is a deliberate, narrow exception to
/// the "never hue-code the mark" rule, not a violation of it. Operates on
/// a 16x16 RGBA buffer (this crate's only icon size, see `SIZE`) —
/// pixel-distance math is hardcoded to that, not parameterized, since
/// there is currently no second size that needs this.
fn draw_notification_dot(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - NOTIFICATION_DOT_CENTER.0;
            let dy = y as f32 - NOTIFICATION_DOT_CENTER.1;
            if dx * dx + dy * dy <= NOTIFICATION_DOT_RADIUS * NOTIFICATION_DOT_RADIUS {
                let idx = ((y * SIZE + x) * 4) as usize;
                out[idx..idx + 4].copy_from_slice(&NOTIFICATION_DOT_COLOR);
            }
        }
    }
    out
}

/// Both monochrome variants, decoded once at startup — 16x16 is cheap
/// enough that re-decoding on every refresh tick would also be fine, but
/// there is no reason to when the source bytes never change at runtime.
pub struct TrayIcons {
    light_bg: Vec<u8>,
    dark_bg: Vec<u8>,
}

impl TrayIcons {
    pub fn load() -> Self {
        Self { light_bg: decode_rgba(TRAY_LIGHT_BG_PNG), dark_bg: decode_rgba(TRAY_DARK_BG_PNG) }
    }

    /// `dark_background`: true when the tray/menu-bar/status-area this
    /// icon sits on is dark (pick the white mark for contrast), false
    /// when light (pick the black mark) — see `tray.rs` for how each
    /// platform determines this.
    ///
    /// `dim`: true while the agent is paused or unpaired.
    ///
    /// `notify`: true when a real, verified update is available — draws
    /// the corner dot (see `draw_notification_dot`'s doc comment). Applied
    /// AFTER dimming, not before — the dot must stay fully opaque/legible
    /// even while the rest of the mark is faded from a paused agent (an
    /// available update is exactly as worth noticing whether the agent
    /// happens to be paused or not).
    pub fn rgba_for(&self, dark_background: bool, dim: bool, notify: bool) -> Vec<u8> {
        let base = if dark_background { &self.dark_bg } else { &self.light_bg };
        let rgba = if dim { dim_alpha(base) } else { base.clone() };
        if notify {
            draw_notification_dot(&rgba)
        } else {
            rgba
        }
    }
}
