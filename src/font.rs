//! Font loading, script-based fallback, and the rasterised glyph cache.
//!
//! Font bytes are intentionally leaked into `&'static [u8]`: fonts are loaded
//! once at startup and live for the whole process, and a `'static` slice lets
//! us cache the parsed `rustybuzz::Face` instead of re-parsing tables on every
//! shaped run.

use anyhow::{anyhow, Result};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
}

/// Which font in the stack a character should be drawn with.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Slot {
    /// Latin / box drawing / everything the monospace face covers.
    Mono,
    /// Arabic and the Arabic-Indic digits.
    Arabic,
    /// Colour emoji.
    Emoji,
    /// Nerd Font icons: the private-use glyphs status bars and file trees use.
    Symbols,
    /// Last-resort coverage for anything else.
    Fallback,
}

pub struct Face {
    pub data: &'static [u8],
    pub index: u32,
    pub hb: rustybuzz::Face<'static>,
    swash: swash::FontRef<'static>,
    pub upem: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Advance of U+0020, in em units — the basis for the cell width.
    pub space_advance: f32,
}

impl Face {
    fn load(path: &str, index: u32) -> Result<Face> {
        let bytes = std::fs::read(path).map_err(|e| anyhow!("{path}: {e}"))?;
        let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let hb = rustybuzz::Face::from_slice(data, index)
            .ok_or_else(|| anyhow!("{path}#{index}: not a usable face"))?;
        let swash = swash::FontRef::from_index(data, index as usize)
            .ok_or_else(|| anyhow!("{path}#{index}: swash rejected the face"))?;
        let upem = hb.units_per_em() as f32;
        let space_advance = hb
            .glyph_index(' ')
            .and_then(|g| hb.glyph_hor_advance(g))
            .map(|a| a as f32 / upem)
            .unwrap_or(0.6);
        Ok(Face {
            data,
            index,
            swash,
            upem,
            ascent: hb.ascender() as f32 / upem,
            descent: hb.descender() as f32 / upem,
            space_advance,
            hb,
        })
    }

    #[inline]
    pub fn swash(&self) -> swash::FontRef<'static> {
        self.swash
    }

    #[inline]
    pub fn covers(&self, c: char) -> bool {
        self.hb.glyph_index(c).is_some()
    }
}

/// A rasterised glyph: 8-bit coverage, or straight BGRA for colour emoji.
pub struct RasterGlyph {
    pub width: u32,
    pub height: u32,
    /// Offset of the bitmap's top-left from the pen origin on the baseline.
    pub left: i32,
    pub top: i32,
    pub data: Vec<u8>,
    pub color: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: u16,
    pub glyph: u16,
    /// Pixel size in 1/4-px steps, so size changes do not collide.
    pub size_q: u16,
    /// Horizontal subpixel phase, quantised to 4 buckets.
    pub subpx: u8,
}

pub struct Fonts {
    pub faces: Vec<Face>,
    /// faces index by (slot, style).
    map: HashMap<(Slot, Style), usize>,
    cache: HashMap<GlyphKey, Option<RasterGlyph>>,
    scalers: Vec<swash::scale::ScaleContext>,
    /// Extra scale applied to Arabic faces so their visual weight matches the
    /// Latin monospace face; naskh designs sit much smaller on the em.
    pub arabic_scale: f32,
    /// Whether `arabic_scale` is derived from metrics rather than user-set.
    auto_arabic_scale: bool,
    /// Bumped whenever metrics change, so cached layouts can be invalidated.
    generation: u64,
    pub size_px: f32,
    /// Cell metrics derived from the monospace face.
    pub cell_w: f32,
    pub cell_h: f32,
    pub baseline: f32,
}

/// Families to look for in each slot, best first.
///
/// A Nerd Font is preferred for the monospace slot: it is a good terminal face
/// and it carries the private-use icons that status bars, file trees and
/// workspace managers draw with. The "Mono" cut keeps those icons one cell wide,
/// which is what the grid needs.
const FAMILIES: &[(Slot, &[&str])] = &[
    (
        Slot::Mono,
        &[
            "JetBrainsMono Nerd Font Mono",
            "JetBrainsMonoNL Nerd Font Mono",
            "FiraCode Nerd Font Mono",
            "Hack Nerd Font Mono",
            "MesloLGS Nerd Font Mono",
            "CaskaydiaCove Nerd Font Mono",
            "JetBrains Mono",
            "Menlo",
            "SF Mono",
            "DejaVu Sans Mono",
            "Liberation Mono",
            "Consolas",
            "Ubuntu Mono",
            "Noto Sans Mono",
            "Monaco",
            "Courier New",
        ],
    ),
    (
        Slot::Arabic,
        &[
            "Geeza Pro",
            "Noto Naskh Arabic",
            "Noto Sans Arabic",
            "Amiri",
            "Scheherazade New",
            "Al Bayan",
            "Baghdad",
            "Damascus",
            "Al Nile",
            "DejaVu Sans",
        ],
    ),
    (Slot::Symbols, &["Symbols Nerd Font Mono", "Symbols Nerd Font"]),
    (
        Slot::Emoji,
        &["Apple Color Emoji", "Noto Color Emoji", "Segoe UI Emoji", "Twemoji Mozilla"],
    ),
    (
        Slot::Fallback,
        &["Apple Symbols", "Arial Unicode MS", "Noto Sans Symbols 2", "DejaVu Sans"],
    ),
];

/// Last-resort absolute paths, for the case where no system font database is
/// available (a stripped container, say) but the files are still on disk.
const FALLBACK_PATHS: &[(Slot, &str, u32)] = &[
    (Slot::Mono, "/System/Library/Fonts/Menlo.ttc", 0),
    (Slot::Mono, "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", 0),
    (Slot::Arabic, "/System/Library/Fonts/GeezaPro.ttc", 0),
    (Slot::Arabic, "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
];

/// Expand a leading `~/` against the home directory.
fn expand(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => std::path::Path::new(&home).join(rest),
            None => std::path::PathBuf::from(path),
        },
        None => std::path::PathBuf::from(path),
    }
}

fn style_of(s: Style) -> (fontdb::Weight, fontdb::Style) {
    (
        if s.bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
        if s.italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
    )
}

/// Find the best face for `family` in `style`, as a (path, face index) pair.
///
/// Matching is done by walking the database rather than through `Query`, so an
/// exactly-named family always wins and we can fall back within the family when
/// a particular style is missing.
fn find_face(db: &fontdb::Database, family: &str, style: Style) -> Option<(String, u32)> {
    let (want_weight, want_style) = style_of(style);
    let mut best: Option<(&fontdb::FaceInfo, i32)> = None;
    for face in db.faces() {
        if !face.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(family)) {
            continue;
        }
        // Prefer an exact style match; otherwise take the closest weight, so a
        // family shipped without an italic still yields something usable.
        let weight_gap = (face.weight.0 as i32 - want_weight.0 as i32).abs();
        let style_gap = if face.style == want_style { 0 } else { 1000 };
        let score = style_gap + weight_gap;
        if best.map_or(true, |(_, b)| score < b) {
            best = Some((face, score));
        }
    }
    let (face, _) = best?;
    match &face.source {
        fontdb::Source::File(p) => Some((p.to_str()?.to_string(), face.index)),
        _ => None,
    }
}

pub const STYLES: [Style; 4] = [
    Style { bold: false, italic: false },
    Style { bold: true, italic: false },
    Style { bold: false, italic: true },
    Style { bold: true, italic: true },
];

impl Fonts {
    /// `arabic_scale` of `None` derives the scale from font metrics so that a
    /// typical Arabic advance matches the monospace cell width.
    pub fn new(size_px: f32, arabic_scale: Option<f32>) -> Result<Fonts> {
        let mut faces = Vec::new();
        let mut map = HashMap::new();

        // Ask the platform what it has, rather than guessing at paths. This is
        // also what makes the font stack work off macOS.
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        for dir in ["~/Library/Fonts", "~/.local/share/fonts", "~/.fonts"] {
            let p = expand(dir);
            if p.is_dir() {
                db.load_fonts_dir(p);
            }
        }

        for (slot, families) in FAMILIES {
            for family in *families {
                for style in STYLES {
                    if map.contains_key(&(*slot, style)) {
                        continue;
                    }
                    let Some((path, idx)) = find_face(&db, family, style) else { continue };
                    match Face::load(&path, idx) {
                        Ok(f) => {
                            faces.push(f);
                            map.insert((*slot, style), faces.len() - 1);
                        }
                        Err(e) => log::debug!("skipping {path}#{idx}: {e}"),
                    }
                }
                // Once a family has supplied the regular face, stay with it for
                // the remaining styles rather than mixing two designs.
                if map.contains_key(&(*slot, Style::default())) {
                    break;
                }
            }
        }

        for (slot, path, idx) in FALLBACK_PATHS {
            if map.contains_key(&(*slot, Style::default())) {
                continue;
            }
            if !std::path::Path::new(path).exists() {
                continue;
            }
            if let Ok(f) = Face::load(path, *idx) {
                faces.push(f);
                map.insert((*slot, Style::default()), faces.len() - 1);
            }
        }

        if !map.contains_key(&(Slot::Mono, Style::default())) {
            return Err(anyhow!("no usable monospace font found"));
        }

        let scalers = (0..faces.len()).map(|_| swash::scale::ScaleContext::new()).collect();
        let auto = arabic_scale.is_none();
        let mut f = Fonts {
            faces,
            map,
            cache: HashMap::new(),
            scalers,
            arabic_scale: arabic_scale.unwrap_or(1.0),
            auto_arabic_scale: auto,
            generation: 0,
            size_px,
            cell_w: 0.0,
            cell_h: 0.0,
            baseline: 0.0,
        };
        f.recompute_metrics();
        if auto {
            f.arabic_scale = f.derive_arabic_scale();
            f.recompute_metrics();
        }
        Ok(f)
    }

    /// Choose the Arabic scale so a typical Arabic word fills the columns it
    /// occupies, without overflowing them.
    ///
    /// Naskh faces set far narrower on the em than a monospace Latin face, so
    /// Arabic drawn at the Latin size looks thin and detached from its columns.
    /// Scaling it up fixes that, but overshooting is worse than undershooting:
    /// once a word's ink is wider than the columns it owns it spills into its
    /// neighbour and swallows the space between words.
    ///
    /// The measurement has to be made on *shaped* words rather than isolated
    /// glyph advances, because cursive medial forms are markedly narrower than
    /// the isolated forms — averaging isolated advances underestimates how much
    /// room a real word leaves and lands the scale far too low.
    fn derive_arabic_scale(&self) -> f32 {
        // Ordinary words, chosen to mix joining and non-joining letters.
        const SAMPLES: &[&str] =
            &["مرحبا", "بالعالم", "الطرفية", "العربية", "الملف", "إصلاح", "المستخدم", "كتابة"];
        let Some(&id) = self.map.get(&(Slot::Arabic, Style::default())) else { return 1.0 };
        let face = &self.faces[id];

        let mut ratios = Vec::new();
        for w in SAMPLES {
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(w);
            buf.set_direction(rustybuzz::Direction::RightToLeft);
            buf.set_script(rustybuzz::script::ARABIC);
            let out = rustybuzz::shape(&face.hb, &[], buf);
            let em: f32 =
                out.glyph_positions().iter().map(|p| p.x_advance as f32).sum::<f32>() / face.upem;
            if em <= 0.0 {
                continue;
            }
            let want = w.chars().count() as f32 * self.cell_w;
            ratios.push(want / (em * self.size_px));
        }
        if ratios.is_empty() {
            return 1.0;
        }
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
        // Take the smallest ratio, not the mean: it is the word that would
        // overflow first, and staying under it keeps every sample inside its
        // columns. A small margin absorbs words denser than any sample.
        (ratios[0] * 0.97).clamp(0.8, 2.0)
    }

    fn recompute_metrics(&mut self) {
        let mono = &self.faces[self.map[&(Slot::Mono, Style::default())]];
        // Cell width from the space advance keeps columns aligned with what
        // the shell assumes; round so glyph rasterisation stays on-pixel.
        self.cell_w = (mono.space_advance * self.size_px).round().max(1.0);
        let mut ascent = mono.ascent * self.size_px;
        let mut descent = -mono.descent * self.size_px;

        // The Arabic face is drawn larger to match the Latin weight, so the
        // line box has to accommodate *its* extent too — otherwise tashkeel
        // above the letters clips into the row above.
        if let Some(&ar) = self.map.get(&(Slot::Arabic, Style::default())) {
            let apx = self.size_px * self.arabic_scale;
            ascent = ascent.max(self.faces[ar].ascent * apx);
            descent = descent.max(-self.faces[ar].descent * apx);
        }

        let line = ascent + descent;
        self.cell_h = (line * 1.06).round().max(1.0);
        self.baseline = (ascent + (self.cell_h - line) * 0.5).round();
    }

    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn set_size(&mut self, size_px: f32) {
        self.generation += 1;
        self.size_px = size_px.clamp(6.0, 200.0);
        self.cache.clear();
        self.recompute_metrics();
        if self.auto_arabic_scale {
            self.arabic_scale = self.derive_arabic_scale();
            self.recompute_metrics();
        }
    }

    pub fn set_arabic_scale(&mut self, s: f32) {
        self.generation += 1;
        self.auto_arabic_scale = false;
        self.arabic_scale = s.clamp(0.5, 2.5);
        self.cache.clear();
        self.recompute_metrics();
    }

    /// Font index for a slot+style, falling back through style then slot.
    pub fn face_id(&self, slot: Slot, style: Style) -> usize {
        self.map
            .get(&(slot, style))
            .or_else(|| self.map.get(&(slot, Style { bold: style.bold, italic: false })))
            .or_else(|| self.map.get(&(slot, Style::default())))
            .or_else(|| self.map.get(&(Slot::Mono, style)))
            .copied()
            .unwrap_or(self.map[&(Slot::Mono, Style::default())])
    }

    /// Pick the slot that should render `c`.
    pub fn slot_for(&self, c: char, style: Style) -> Slot {
        // Arabic script blocks (plus Arabic-Indic digits) go to the Arabic face
        // even when the mono face nominally has a glyph, because mono coverage
        // of Arabic is unshaped and ugly.
        if is_arabic(c) {
            if self.map.contains_key(&(Slot::Arabic, Style::default())) {
                return Slot::Arabic;
            }
        }
        let mono = &self.faces[self.face_id(Slot::Mono, style)];
        if mono.covers(c) {
            return Slot::Mono;
        }
        for slot in [Slot::Arabic, Slot::Symbols, Slot::Emoji, Slot::Fallback] {
            if let Some(&i) = self.map.get(&(slot, Style::default())) {
                if self.faces[i].covers(c) {
                    return slot;
                }
            }
        }
        Slot::Mono
    }

    /// Pixel size used for a given face — Arabic gets its own multiplier.
    #[inline]
    pub fn px_for(&self, slot: Slot) -> f32 {
        if slot == Slot::Arabic {
            self.size_px * self.arabic_scale
        } else {
            self.size_px
        }
    }

    pub fn rasterize(&mut self, font: usize, glyph: u16, px: f32, subpx: f32) -> Option<&RasterGlyph> {
        let bucket = ((subpx.fract().abs() * 4.0) as u8).min(3);
        let key = GlyphKey {
            font: font as u16,
            glyph,
            size_q: (px * 4.0).round().clamp(0.0, 65535.0) as u16,
            subpx: bucket,
        };
        if !self.cache.contains_key(&key) {
            let g = self.render_uncached(font, glyph, px, bucket as f32 * 0.25);
            self.cache.insert(key, g);
        }
        self.cache.get(&key).and_then(|o| o.as_ref())
    }

    fn render_uncached(&mut self, font: usize, glyph: u16, px: f32, subpx: f32) -> Option<RasterGlyph> {
        let swf = self.faces[font].swash();
        let mut scaler = self.scalers[font].builder(swf).size(px).hint(true).build();
        let img = swash::scale::Render::new(&[
            swash::scale::Source::ColorOutline(0),
            swash::scale::Source::ColorBitmap(swash::scale::StrikeWith::BestFit),
            swash::scale::Source::Outline,
        ])
        .format(swash::zeno::Format::Alpha)
        .offset(swash::zeno::Vector::new(subpx, 0.0))
        .render(&mut scaler, glyph)?;

        let color = matches!(img.content, swash::scale::image::Content::Color);
        if img.placement.width == 0 || img.placement.height == 0 {
            return None;
        }
        Some(RasterGlyph {
            width: img.placement.width,
            height: img.placement.height,
            left: img.placement.left,
            top: img.placement.top,
            data: img.data,
            color,
        })
    }
}

/// Arabic script, its supplements, presentation forms, and Arabic-Indic digits.
#[inline]
pub fn is_arabic(c: char) -> bool {
    matches!(c as u32,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF |
        0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
}
