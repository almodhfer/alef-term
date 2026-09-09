//! Colour model: ANSI 16, xterm-256 palette, and 24-bit truecolour.

/// A resolved 24-bit RGB colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    #[inline]
    pub const fn pack(self) -> u32 {
        // 0RGB in the low 24 bits of a u32, the software canvas layout.
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | self.2 as u32
    }
}

/// A cell colour before palette resolution. Kept indexed so that live palette
/// changes (OSC 4) and theme switches recolour existing scrollback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    /// The terminal's default foreground/background — meaning depends on slot.
    Default,
    /// Index into the 256-colour palette.
    Indexed(u8),
    /// Literal 24-bit colour from SGR 38;2 / 48;2.
    Rgb(Rgb),
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

/// The 256-entry xterm palette: 16 configurable ANSI colours, a 6x6x6 colour
/// cube, and a 24-step greyscale ramp.
#[derive(Clone)]
pub struct Palette {
    pub colors: [Rgb; 256],
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub selection_bg: Rgb,
    pub selection_fg: Option<Rgb>,
}

impl Palette {
    /// The 256-colour table from 16 ANSI colours: xterm's 6x6x6 cube and
    /// 24-step grey ramp follow.
    fn table(ansi: [Rgb; 16]) -> [Rgb; 256] {
        let mut colors = [Rgb(0, 0, 0); 256];
        colors[..16].copy_from_slice(&ansi);

        // 16..232: 6x6x6 cube. The level ramp is xterm's, not linear.
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let mut i = 16;
        for r in 0..6 {
            for g in 0..6 {
                for b in 0..6 {
                    colors[i] = Rgb(LEVELS[r], LEVELS[g], LEVELS[b]);
                    i += 1;
                }
            }
        }

        // 232..256: 24 greys from 8 to 238 in steps of 10.
        for j in 0..24 {
            let v = 8 + j as u8 * 10;
            colors[232 + j] = Rgb(v, v, v);
        }
        colors
    }

    /// A light scheme with the same contrast care as the dark one: the
    /// yellows and cyans are darkened so they stay legible on white.
    pub fn light() -> Self {
        const ANSI: [Rgb; 16] = [
            Rgb(0x2e, 0x33, 0x3d), // 0 black
            Rgb(0xc0, 0x28, 0x34), // 1 red
            Rgb(0x2a, 0x7f, 0x2e), // 2 green
            Rgb(0x9a, 0x6b, 0x00), // 3 yellow
            Rgb(0x1f, 0x5f, 0xc4), // 4 blue
            Rgb(0x8a, 0x3f, 0xb8), // 5 magenta
            Rgb(0x0e, 0x7c, 0x86), // 6 cyan
            Rgb(0x6b, 0x72, 0x80), // 7 white
            Rgb(0x8b, 0x92, 0x9e), // 8 bright black
            Rgb(0xd9, 0x3f, 0x4a), // 9 bright red
            Rgb(0x3a, 0x9e, 0x40), // 10 bright green
            Rgb(0xb0, 0x7d, 0x00), // 11 bright yellow
            Rgb(0x2f, 0x7a, 0xe0), // 12 bright blue
            Rgb(0xa2, 0x55, 0xd6), // 13 bright magenta
            Rgb(0x1a, 0x96, 0xa0), // 14 bright cyan
            Rgb(0x1c, 0x1f, 0x26), // 15 bright white
        ];
        Palette {
            colors: Self::table(ANSI),
            fg: Rgb(0x24, 0x28, 0x30),
            bg: Rgb(0xfa, 0xfa, 0xf7),
            cursor: Rgb(0x1f, 0x5f, 0xc4),
            selection_bg: Rgb(0xc8, 0xdc, 0xf5),
            selection_fg: None,
        }
    }

    pub fn dark() -> Self {
        // Base16-ish dark scheme; readable against Arabic naskh strokes, which
        // are thinner than Latin mono stems and wash out on low-contrast themes.
        const ANSI: [Rgb; 16] = [
            Rgb(0x1c, 0x1f, 0x26), // 0 black
            Rgb(0xe0, 0x60, 0x6a), // 1 red
            Rgb(0x7c, 0xc3, 0x70), // 2 green
            Rgb(0xe0, 0xb2, 0x5f), // 3 yellow
            Rgb(0x62, 0xa0, 0xea), // 4 blue
            Rgb(0xb4, 0x82, 0xdc), // 5 magenta
            Rgb(0x5c, 0xc0, 0xc0), // 6 cyan
            Rgb(0xc5, 0xcb, 0xd6), // 7 white
            Rgb(0x4a, 0x51, 0x5e), // 8 bright black
            Rgb(0xf2, 0x7d, 0x86), // 9 bright red
            Rgb(0x98, 0xdb, 0x8c), // 10 bright green
            Rgb(0xf5, 0xcb, 0x7c), // 11 bright yellow
            Rgb(0x84, 0xba, 0xf5), // 12 bright blue
            Rgb(0xcb, 0x9e, 0xef), // 13 bright magenta
            Rgb(0x79, 0xd6, 0xd6), // 14 bright cyan
            Rgb(0xe8, 0xed, 0xf5), // 15 bright white
        ];

        Palette {
            colors: Self::table(ANSI),
            fg: Rgb(0xd4, 0xd9, 0xe0),
            bg: Rgb(0x14, 0x16, 0x1b),
            cursor: Rgb(0x62, 0xa0, 0xea),
            selection_bg: Rgb(0x2d, 0x41, 0x5a),
            selection_fg: None,
        }
    }

    /// Resolve a cell colour to RGB. `is_fg` picks which default applies.
    #[inline]
    pub fn resolve(&self, c: Color, is_fg: bool) -> Rgb {
        match c {
            Color::Default => {
                if is_fg {
                    self.fg
                } else {
                    self.bg
                }
            }
            Color::Indexed(i) => self.colors[i as usize],
            Color::Rgb(rgb) => rgb,
        }
    }
}
