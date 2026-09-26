//! Colour model: ANSI 16, xterm-256 palette, and 24-bit truecolour; and the
//! built-in themes the palette is made from.

/// A resolved 24-bit RGB colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    #[inline]
    pub const fn pack(self) -> u32 {
        // 0RGB in the low 24 bits of a u32, the software canvas layout.
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | self.2 as u32
    }

    /// Faint text moves toward the surface it is drawn on, in either theme.
    pub fn dimmed(self, bg: Rgb) -> Self {
        mix(self, bg, 128)
    }

    fn luminance(self) -> f32 {
        let linear = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * linear(self.0) + 0.7152 * linear(self.1) + 0.0722 * linear(self.2)
    }

    fn contrast(self, bg: Rgb) -> f32 {
        let (a, b) = (self.luminance(), bg.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Move only unreadable ink toward black or white, keeping its colour.
    /// Equal foreground/background is intentional concealment or artwork.
    pub fn ensure_contrast(self, bg: Rgb, minimum: f32) -> Self {
        if minimum <= 1.0 || !minimum.is_finite() || self == bg || self.contrast(bg) >= minimum {
            return self;
        }
        let black = Rgb(0, 0, 0);
        let white = Rgb(255, 255, 255);
        let mut best: Option<(u32, Rgb)> = None;
        for end in [black, white] {
            if end.contrast(bg) < minimum { continue; }
            let (mut lo, mut hi) = (0, 256);
            while lo + 1 < hi {
                let mid = (lo + hi) / 2;
                if mix(self, end, mid).contrast(bg) >= minimum { hi = mid; }
                else { lo = mid; }
            }
            if best.is_none_or(|(distance, _)| hi < distance) {
                best = Some((hi, mix(self, end, hi)));
            }
        }
        best.map(|(_, color)| color).unwrap_or_else(|| {
            if black.contrast(bg) >= white.contrast(bg) { black } else { white }
        })
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

/// A named colour scheme: the 16 ANSI colours and the terminal's own colours.
#[derive(Debug)]
pub struct Theme {
    /// What the configuration file calls it.
    pub name: &'static str,
    /// What the theme picker shows.
    pub title: &'static str,
    pub ansi: [Rgb; 16],
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub selection_bg: Rgb,
    pub search_bg: Rgb,
    pub search_current_bg: Rgb,
}

const fn hex(v: u32) -> Rgb {
    Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// `a` moved `t`/256 of the way to `b`.
pub const fn mix(a: Rgb, b: Rgb, t: u32) -> Rgb {
    Rgb(
        ((a.0 as u32 * (256 - t) + b.0 as u32 * t) / 256) as u8,
        ((a.1 as u32 * (256 - t) + b.1 as u32 * t) / 256) as u8,
        ((a.2 as u32 * (256 - t) + b.2 as u32 * t) / 256) as u8,
    )
}

/// A theme from its published colours. Search highlights are the theme's
/// yellow washed into its background, so hits stay readable in any scheme.
const fn theme(
    name: &'static str,
    title: &'static str,
    fg: u32,
    bg: u32,
    cursor: u32,
    selection_bg: u32,
    ansi: [u32; 16],
) -> Theme {
    let mut rgb = [Rgb(0, 0, 0); 16];
    let mut i = 0;
    while i < 16 {
        rgb[i] = hex(ansi[i]);
        i += 1;
    }
    let bg = hex(bg);
    Theme {
        name,
        title,
        ansi: rgb,
        fg: hex(fg),
        bg,
        cursor: hex(cursor),
        selection_bg: hex(selection_bg),
        search_bg: mix(bg, rgb[3], 72),
        search_current_bg: mix(bg, rgb[3], 128),
    }
}

/// Every built-in theme, in the order the picker lists them.
pub static THEMES: &[Theme] = &[
    // Base16-ish dark scheme; readable against Arabic naskh strokes, which
    // are thinner than Latin mono stems and wash out on low-contrast themes.
    Theme {
        name: "dark",
        title: "Arabic Term Dark",
        ansi: [
            hex(0x1c1f26), hex(0xe0606a), hex(0x7cc370), hex(0xe0b25f),
            hex(0x62a0ea), hex(0xb482dc), hex(0x5cc0c0), hex(0xc5cbd6),
            hex(0x4a515e), hex(0xf27d86), hex(0x98db8c), hex(0xf5cb7c),
            hex(0x84baf5), hex(0xcb9eef), hex(0x79d6d6), hex(0xe8edf5),
        ],
        fg: hex(0xd4d9e0),
        bg: hex(0x14161b),
        cursor: hex(0x62a0ea),
        selection_bg: hex(0x2d415a),
        search_bg: hex(0x5a4a14),
        search_current_bg: hex(0xa87a10),
    },
    // A light scheme with the same contrast care as the dark one: the
    // yellows and cyans are darkened so they stay legible on white.
    Theme {
        name: "light",
        title: "Arabic Term Light",
        ansi: [
            hex(0x2e333d), hex(0xc02834), hex(0x2a7f2e), hex(0x9a6b00),
            hex(0x1f5fc4), hex(0x8a3fb8), hex(0x0e7c86), hex(0x6b7280),
            hex(0x8b929e), hex(0xd93f4a), hex(0x3a9e40), hex(0xb07d00),
            hex(0x2f7ae0), hex(0xa255d6), hex(0x1a96a0), hex(0x1c1f26),
        ],
        fg: hex(0x242830),
        bg: hex(0xfafaf7),
        cursor: hex(0x1f5fc4),
        selection_bg: hex(0xc8dcf5),
        search_bg: hex(0xffe98a),
        search_current_bg: hex(0xffb340),
    },
    theme("dracula", "Dracula", 0xf8f8f2, 0x282a36, 0xf8f8f2, 0x44475a, [
        0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
        0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
    ]),
    theme("nord", "Nord", 0xd8dee9, 0x2e3440, 0xd8dee9, 0x434c5e, [
        0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
        0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
    ]),
    theme("gruvbox-dark", "Gruvbox Dark", 0xebdbb2, 0x282828, 0xebdbb2, 0x504945, [
        0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
        0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
    ]),
    theme("gruvbox-light", "Gruvbox Light", 0x3c3836, 0xfbf1c7, 0x3c3836, 0xd5c4a1, [
        0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
        0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
    ]),
    // Solarized's own bright colours are its greys, which leaves bright black
    // the same as the background and zsh's suggestions invisible. The brights
    // here are the accents instead, with readable greys at either end.
    theme("solarized-dark", "Solarized Dark", 0x839496, 0x002b36, 0x93a1a1, 0x073642, [
        0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
        0x586e75, 0xcb4b16, 0x859900, 0xb58900, 0x268bd2, 0x6c71c4, 0x2aa198, 0xfdf6e3,
    ]),
    theme("solarized-light", "Solarized Light", 0x657b83, 0xfdf6e3, 0x586e75, 0xeee8d5, [
        0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0x93a1a1,
        0x657b83, 0xcb4b16, 0x859900, 0xb58900, 0x268bd2, 0x6c71c4, 0x2aa198, 0x002b36,
    ]),
    theme("tokyo-night", "Tokyo Night", 0xc0caf5, 0x1a1b26, 0xc0caf5, 0x33467c, [
        0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
        0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
    ]),
    theme("catppuccin-mocha", "Catppuccin Mocha", 0xcdd6f4, 0x1e1e2e, 0xf5e0dc, 0x45475a, [
        0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
        0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
    ]),
    theme("catppuccin-latte", "Catppuccin Latte", 0x4c4f69, 0xeff1f5, 0xdc8a78, 0xccd0da, [
        0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
        0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
    ]),
    theme("one-dark", "One Dark", 0xabb2bf, 0x282c34, 0x528bff, 0x3e4451, [
        0x1e2127, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
        0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
    ]),
    theme("monokai", "Monokai", 0xf8f8f2, 0x272822, 0xf8f8f0, 0x49483e, [
        0x272822, 0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xf8f8f2,
        0x75715e, 0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xf9f8f5,
    ]),
    theme("github-dark", "GitHub Dark", 0xe6edf3, 0x0d1117, 0x2f81f7, 0x264f78, [
        0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4,
        0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
    ]),
    theme("github-light", "GitHub Light", 0x1f2328, 0xffffff, 0x0969da, 0xc8e1ff, [
        0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
        0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
    ]),
    theme("rose-pine", "Rosé Pine", 0xe0def4, 0x191724, 0xebbcba, 0x403d52, [
        0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
        0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
    ]),
    theme("kanagawa", "Kanagawa", 0xdcd7ba, 0x1f1f28, 0xc8c093, 0x2d4f67, [
        0x16161d, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093,
        0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
    ]),
    theme("everforest-dark", "Everforest Dark", 0xd3c6aa, 0x2d353b, 0xd3c6aa, 0x475258, [
        0x475258, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
        0x859289, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
    ]),
];

/// The theme a name refers to. Case, spaces and underscores are forgiven, so
/// `Tokyo Night`, `tokyo_night` and `tokyo-night` are the same theme.
pub fn find_theme(name: &str) -> Option<&'static Theme> {
    let key: String = name
        .trim()
        .chars()
        .map(|c| if c == ' ' || c == '_' { '-' } else { c.to_ascii_lowercase() })
        .collect();
    THEMES.iter().find(|t| t.name == key || t.title.to_lowercase() == name.trim().to_lowercase())
}

/// The theme used when nothing says otherwise.
pub fn default_theme() -> &'static Theme {
    &THEMES[0]
}

/// Colours the configuration file sets on top of whichever theme is in use.
#[derive(Clone, Default, Debug)]
pub struct Overrides {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub cursor: Option<Rgb>,
    pub selection_bg: Option<Rgb>,
    pub selection_fg: Option<Rgb>,
    /// The first few ANSI colours, in order.
    pub ansi: Vec<Rgb>,
}

impl Overrides {
    pub fn apply(&self, p: &mut Palette) {
        let set = |into: &mut Rgb, v: Option<Rgb>| {
            if let Some(v) = v {
                *into = v;
            }
        };
        set(&mut p.fg, self.fg);
        set(&mut p.bg, self.bg);
        set(&mut p.cursor, self.cursor);
        set(&mut p.selection_bg, self.selection_bg);
        if self.selection_fg.is_some() {
            p.selection_fg = self.selection_fg;
        }
        for (i, c) in self.ansi.iter().take(16).enumerate() {
            p.colors[i] = *c;
        }
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
    /// Search hits, and the one the view is on.
    pub search_bg: Rgb,
    pub search_current_bg: Rgb,
}

impl Palette {
    /// Preserve coloured text over a terminal highlight when it is readable.
    /// Inverse text and application colours may need the theme foreground,
    /// or black/white on a particularly strong highlight or custom cursor.
    pub fn highlight_fg(&self, fg: Rgb, bg: Rgb) -> Rgb {
        if fg.contrast(bg) >= 4.5 {
            fg
        } else if self.fg.contrast(bg) >= 4.5 {
            self.fg
        } else {
            let black = Rgb(0, 0, 0);
            let white = Rgb(255, 255, 255);
            if black.contrast(bg) >= white.contrast(bg) { black } else { white }
        }
    }

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

    pub fn from_theme(t: &Theme) -> Self {
        Palette {
            colors: Self::table(t.ansi),
            fg: t.fg,
            bg: t.bg,
            cursor: t.cursor,
            selection_bg: t.selection_bg,
            selection_fg: None,
            search_bg: t.search_bg,
            search_current_bg: t.search_current_bg,
        }
    }

    pub fn light() -> Self {
        Self::from_theme(find_theme("light").expect("built in"))
    }

    pub fn dark() -> Self {
        Self::from_theme(default_theme())
    }

    /// Whether the background reads as dark, which is what a program asking
    /// for the colour scheme (`CSI ?996n`) is told.
    pub fn is_dark(&self) -> bool {
        let Rgb(r, g, b) = self.bg;
        0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32 <= 128.0
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
