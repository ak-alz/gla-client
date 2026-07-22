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
    pub fn rgba_for(&self, dark_background: bool, dim: bool) -> Vec<u8> {
        let base = if dark_background { &self.dark_bg } else { &self.light_bg };
        if dim {
            dim_alpha(base)
        } else {
            base.clone()
        }
    }
}
