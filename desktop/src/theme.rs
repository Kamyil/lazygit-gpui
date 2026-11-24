//! Colors. lazygit's content carries terminal colors (palette indices or RGB);
//! the palette entries are mapped to colors that suit the native window.

use gpui::{Hsla, Rgba, rgb};

pub const PAPER: u32 = 0x161b1f;
pub const SIDEBAR: u32 = 0x1c2227;
pub const PANEL: u32 = 0x20272d;
pub const RULE: u32 = 0x313b43;
pub const INK: u32 = 0xdfe5e8;
pub const MUTED: u32 = 0x8c9da6;
pub const ACCENT: u32 = 0xe4bd79;
pub const SELECTION: u32 = 0x2f4b5c;
pub const SELECTION_INACTIVE: u32 = 0x29343c;
pub const SELECTION_EDGE: u32 = 0x6cb6e0;
pub const ADDED_TINT: u32 = 0x1d3326;
pub const REMOVED_TINT: u32 = 0x3a2528;
pub const BACKDROP: u32 = 0x0b0e10;
pub const INPUT: u32 = 0x12171a;

/// The 16 basic terminal colors, tuned for the dark window background.
const BASIC: [u32; 16] = [
    0x1b2126, // black
    0xf07f7a, // red
    0x9ed492, // green
    0xe6c27d, // yellow
    0x7db5f0, // blue
    0xd49cf0, // magenta
    0x7fd3d1, // cyan
    0xd7dde1, // white
    0x6e7d87, // bright black
    0xff9a95, // bright red
    0xb5e8a8, // bright green
    0xf3d898, // bright yellow
    0x9fcbff, // bright blue
    0xe5b8ff, // bright magenta
    0x9de9e6, // bright cyan
    0xf4f7f8, // bright white
];

/// Decodes a color from the protocol: 0 is the default color (None), 1 to 256
/// are terminal palette colors 0 to 255, and 0x1000000 + RGB is an RGB color.
pub fn terminal_color(code: u32) -> Option<u32> {
    match code {
        0 => None,
        1..=16 => Some(BASIC[(code - 1) as usize]),
        17..=232 => {
            let index = code - 17;
            let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
            Some(level(index / 36) << 16 | level(index / 6 % 6) << 8 | level(index % 6))
        }
        233..=256 => {
            let gray = 8 + (code - 233) * 10;
            Some(gray << 16 | gray << 8 | gray)
        }
        _ if code & 0x1000000 != 0 => Some(code & 0xffffff),
        _ => None,
    }
}

pub fn hsla(color: u32) -> Hsla {
    Hsla::from(Rgba::from(rgb(color)))
}

/// Whether a protocol color is the terminal's green or red, which lazygit's
/// diffs use for added and removed lines.
pub fn is_green(code: u32) -> bool {
    matches!(code, 3 | 11)
}

pub fn is_red(code: u32) -> bool {
    matches!(code, 2 | 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_palette_and_rgb_colors() {
        assert_eq!(terminal_color(0), None);
        assert_eq!(terminal_color(2), Some(BASIC[1]));
        assert_eq!(terminal_color(17), Some(0x000000));
        assert_eq!(terminal_color(17 + 215), Some(0xffffff));
        assert_eq!(terminal_color(233), Some(0x080808));
        assert_eq!(terminal_color(0x1000000 | 0x123456), Some(0x123456));
    }
}
