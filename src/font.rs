//! Font loading, script-based fallback, and the rasterised glyph cache.
//!
//! Font bytes are cached once per file into `&'static [u8]`: fonts live for
//! the whole process, and a `'static` slice lets
//! us cache the parsed `rustybuzz::Face` instead of re-parsing tables on every
//! shaped run.

use anyhow::{anyhow, Result};
use std::collections::HashMap;

/// Curated Arabic choices exposed in the command menu; fonts remain optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArabicFont { Naskh, Amiri, Scheherazade, Geeza }

impl ArabicFont {
    pub fn family(self) -> &'static str {
        match self {
            Self::Naskh => "Noto Naskh Arabic",
            Self::Amiri => "Amiri",
            Self::Scheherazade => "Scheherazade New",
            Self::Geeza => "Geeza Pro",
        }
    }
}

fn font_data(path: &str) -> Result<&'static [u8]> {
    use std::sync::{Mutex, OnceLock};
    static FILES: OnceLock<Mutex<HashMap<String, &'static [u8]>>> = OnceLock::new();
    let mut files = FILES.get_or_init(Default::default).lock().map_err(|_| anyhow!("font cache unavailable"))?;
    if let Some(data) = files.get(path) { return Ok(data); }
    let bytes = std::fs::read(path).map_err(|e| anyhow!("{path}: {e}"))?;
    let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    files.insert(path.to_string(), data);
    Ok(data)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
}

/// Which font in the stack a character should be drawn with.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[repr(usize)]
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
    /// Variable weight applied identically by shaping and rasterization.
    pub weight: Option<f32>,
    // Shaped sample extents/advance in em units, plus terminal column count.
    ink_samples: Vec<(f32, f32, f32, usize)>,
}

impl Face {
    fn load(path: &str, index: u32, style: Style) -> Result<Face> {
        let data = font_data(path)?;
        let mut hb = rustybuzz::Face::from_slice(data, index)
            .ok_or_else(|| anyhow!("{path}#{index}: not a usable face"))?;
        let weight = hb.variation_axes().into_iter().find(|axis| axis.tag.to_bytes() == *b"wght")
            .map(|axis| (if style.bold { 700.0f32 } else { 400.0f32 }).clamp(axis.min_value, axis.max_value));
        if let Some(value) = weight {
            hb.set_variations(&[rustybuzz::Variation { tag: rustybuzz::ttf_parser::Tag::from_bytes(b"wght"), value }]);
        }
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
            weight,
            ink_samples: Vec::new(),
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
    /// Unique for the life of the process, so a GPU atlas can key on it.
    pub id: u32,
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

/// Shaping plans, one per face, direction and script.
///
/// Building a plan is by far the most expensive part of shaping a short run:
/// rustybuzz walks the GSUB and GPOS tables collecting the script's features
/// and lookups. `rustybuzz::shape` does that on every call, which made a
/// full-screen repaint spend most of its time re-deriving the same plan once
/// per word. Built once here, it is reused for every run that shares its
/// properties.
#[derive(Default)]
pub struct PlanCache {
    plans: HashMap<(usize, rustybuzz::Direction, Option<rustybuzz::Script>), rustybuzz::ShapePlan>,
}

impl PlanCache {
    /// The plan for shaping `buf` with `face`, built on first use.
    ///
    /// The buffer's script and direction are settled here, before the key is
    /// read, so the plan and the buffer cannot disagree about them.
    pub fn get(
        &mut self,
        face: &rustybuzz::Face<'_>,
        font: usize,
        buf: &mut rustybuzz::UnicodeBuffer,
    ) -> &rustybuzz::ShapePlan {
        buf.guess_segment_properties();
        let direction = buf.direction();
        let script = Some(buf.script()).filter(|s| *s != rustybuzz::script::UNKNOWN);
        self.plans
            .entry((font, direction, script))
            .or_insert_with(|| {
                rustybuzz::ShapePlan::new(face, direction, script, buf.language().as_ref(), &[])
            })
    }
}

pub struct Fonts {
    pub faces: Vec<Face>,
    /// Actual Arabic family, so a missing requested font is never mislabeled.
    pub arabic_family: String,
    pub plans: PlanCache,
    /// faces index by (slot, style).
    map: HashMap<(Slot, Style), usize>,
    /// `map` with every fallback already resolved, indexed by slot and style.
    /// Looked up once per character, so it must not cost a hash.
    table: [[usize; 4]; SLOTS],
    /// Whether a slot has any face at all.
    has_slot: [bool; SLOTS],
    /// True when every monospace style covers printable ASCII, which lets the
    /// commonest characters skip the cmap walk entirely.
    ascii_in_mono: bool,
    cache: crate::hash::FastMap<GlyphKey, Option<RasterGlyph>>,
    next_glyph_id: u32,
    scalers: Vec<swash::scale::ScaleContext>,
    /// Extra scale applied to Arabic faces so their visual weight matches the
    /// Latin monospace face; naskh designs sit much smaller on the em.
    pub arabic_scale: f32,
    /// Whether `arabic_scale` is derived from metrics rather than user-set.
    auto_arabic_scale: bool,
    /// Bumped whenever metrics change, so cached layouts can be invalidated.
    generation: u64,
    /// Every installed face, searched for characters the stack does not cover.
    db: fontdb::Database,
    /// The face found for each such character, or `None` when no installed
    /// font has it. Each character is searched for once.
    fallback: crate::hash::FastMap<char, Option<usize>>,
    /// Faces loaded for `fallback`, tried before searching the system again.
    extras: Vec<usize>,
    pub size_px: f32,
    /// Multiplier on the line box shared by Latin and Arabic, at least 1.
    line_height: f32,
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

/// Faces tried first for a character none of the stack's faces cover, before
/// any other installed font, so a script gets a broad, legible design rather
/// than whichever file happens to be found first.
const FALLBACK_FAMILIES: &[&str] = &[
    // Chinese, Japanese, Korean. The monospace CJK cut matches the grid.
    "Noto Sans Mono CJK SC",
    "Noto Sans CJK SC",
    "Source Han Sans SC",
    "PingFang SC",
    "Hiragino Sans GB",
    "Hiragino Sans",
    "Heiti SC",
    "Apple SD Gothic Neo",
    "Microsoft YaHei",
    "Malgun Gothic",
    "WenQuanYi Micro Hei",
    "Droid Sans Fallback",
    // Hebrew, the other right-to-left script in common use.
    "Noto Sans Hebrew",
    "Arial Hebrew",
    // Broad coverage for everything else.
    "Noto Sans",
    "Arial Unicode MS",
    "DejaVu Sans",
];

/// Families that claim every character but draw only placeholder boxes;
/// choosing one would hide a real font further down the list.
const PLACEHOLDER_FAMILIES: &[&str] = &["LastResort", "Adobe Blank"];

/// Last-resort absolute paths, for the case where no system font database is
/// available (a stripped container, say) but the files are still on disk.
const FALLBACK_PATHS: &[(Slot, &str, u32)] = &[
    (Slot::Mono, "/System/Library/Fonts/Menlo.ttc", 0),
    (Slot::Mono, "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", 0),
    (Slot::Arabic, "/System/Library/Fonts/GeezaPro.ttc", 0),
    (Slot::Arabic, "/usr/share/fonts/truetype/noto/NotoNaskhArabic-Regular.ttf", 0),
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
    let face = best_face(db, family, style)?;
    match &face.source {
        fontdb::Source::File(p) => Some((p.to_str()?.to_string(), face.index)),
        _ => None,
    }
}

fn best_face<'a>(db: &'a fontdb::Database, family: &str, style: Style) -> Option<&'a fontdb::FaceInfo> {
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
    best.map(|(face, _)| face)
}

const SLOTS: usize = 5;

impl Style {
    #[inline]
    fn index(self) -> usize {
        self.bold as usize | (self.italic as usize) << 1
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
        Self::with_families(size_px, arabic_scale, None, None)
    }

    /// As [`Fonts::new`], trying the given monospace and Arabic families
    /// before the built-in preference lists.
    pub fn with_families(
        size_px: f32,
        arabic_scale: Option<f32>,
        mono: Option<&str>,
        arabic: Option<&str>,
    ) -> Result<Fonts> {
        let mut faces = Vec::new();
        let mut map = HashMap::new();
        let mut arabic_family = "Fallback".to_string();

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
            let preferred = match slot {
                Slot::Mono => mono,
                Slot::Arabic => arabic,
                _ => None,
            };
            if let Some(p) = preferred {
                if find_face(&db, p, Style::default()).is_none() {
                    log::warn!("configured font {p:?} not found; using the built-in list");
                }
            }
            for family in preferred.into_iter().chain(families.iter().copied()) {
                for style in STYLES {
                    if map.contains_key(&(*slot, style)) {
                        continue;
                    }
                    let Some((path, idx)) = find_face(&db, family, style) else { continue };
                    match Face::load(&path, idx, style) {
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
                    if *slot == Slot::Arabic { arabic_family = family.to_string(); }
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
            if let Ok(f) = Face::load(path, *idx, Style::default()) {
                faces.push(f);
                map.insert((*slot, Style::default()), faces.len() - 1);
            }
        }

        if !map.contains_key(&(Slot::Mono, Style::default())) {
            return Err(anyhow!("no usable monospace font found"));
        }

        // Some book faces position stacked marks outside their advertised
        // ascender/descender. Measure shaped ink in every Arabic style so those
        // marks stay inside a row, including at the bottom of the viewport.
        for style in STYLES {
            if let Some(&id) = map.get(&(Slot::Arabic, style)) {
                let face = &mut faces[id];
                for word in "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ مُحَمَّدٌ سُبْحَانَ ضُوءٌ قُرْآنٌ إِنَّا أَعْطَيْنَاكَ فَصَلِّ".split_whitespace() {
                    let mut buf = rustybuzz::UnicodeBuffer::new();
                    buf.push_str(word);
                    buf.set_direction(rustybuzz::Direction::RightToLeft);
                    buf.set_script(rustybuzz::script::ARABIC);
                    let shaped = rustybuzz::shape(&face.hb, &[], buf);
                    let (mut up, mut down) = (0.0f32, 0.0f32);
                    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                        if let Some(bounds) = face.hb.glyph_bounding_box(rustybuzz::ttf_parser::GlyphId(info.glyph_id as u16)) {
                            up = up.max((bounds.y_max as f32 + pos.y_offset as f32) / face.upem);
                            down = down.max(-(bounds.y_min as f32 + pos.y_offset as f32) / face.upem);
                        }
                    }
                    let advance = shaped.glyph_positions().iter().map(|p| p.x_advance as f32).sum::<f32>() / face.upem;
                    let cols = word.chars().map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)).sum();
                    face.ink_samples.push((up, down, advance, cols));
                }
            }
        }

        let scalers = (0..faces.len()).map(|_| swash::scale::ScaleContext::new()).collect();
        let auto = arabic_scale.is_none();
        let mut f = Fonts {
            faces,
            arabic_family,
            plans: PlanCache::default(),
            map,
            table: [[0; 4]; SLOTS],
            has_slot: [false; SLOTS],
            ascii_in_mono: false,
            cache: Default::default(),
            next_glyph_id: 0,
            scalers,
            arabic_scale: arabic_scale.unwrap_or(1.0),
            auto_arabic_scale: auto,
            generation: 0,
            db,
            fallback: Default::default(),
            extras: Vec::new(),
            size_px,
            line_height: 1.0,
            cell_w: 0.0,
            cell_h: 0.0,
            baseline: 0.0,
        };
        for slot in [Slot::Mono, Slot::Arabic, Slot::Emoji, Slot::Symbols, Slot::Fallback] {
            f.has_slot[slot as usize] = f.map.contains_key(&(slot, Style::default()));
            for style in STYLES {
                f.table[slot as usize][style.index()] = f.resolve_face(slot, style);
            }
        }
        f.ascii_in_mono = STYLES
            .iter()
            .all(|&st| (' '..='~').all(|c| f.faces[f.face_id(Slot::Mono, st)].covers(c)));
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
        for style in STYLES {
            if let Some(&ar) = self.map.get(&(Slot::Arabic, style)) {
                let apx = self.size_px * self.arabic_scale;
                ascent = ascent.max(self.faces[ar].ascent * apx);
                descent = descent.max(-self.faces[ar].descent * apx);
                for &(up, down, advance, cols) in &self.faces[ar].ink_samples {
                    // Match the layout's isotropic fit for each word instead of
                    // reserving full-size ink for words that render smaller.
                    let fit = if advance > 0.0 { (cols as f32 * self.cell_w / (advance * apx)).min(1.0) } else { 1.0 };
                    ascent = ascent.max(up * apx * fit);
                    descent = descent.max(down * apx * fit);
                }
            }
        }

        let line = ascent + descent;
        self.cell_h = (line * self.line_height).ceil().max(1.0);
        self.baseline = (ascent + (self.cell_h - line) * 0.5).round();
    }

    /// Change leading without stretching glyphs or clipping Arabic marks.
    pub fn set_line_height(&mut self, height: f32) {
        if height.is_finite() {
            self.line_height = height.clamp(1.0, 2.0);
            self.generation += 1;
            self.recompute_metrics();
        }
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
    #[inline]
    pub fn face_id(&self, slot: Slot, style: Style) -> usize {
        self.table[slot as usize][style.index()]
    }

    fn resolve_face(&self, slot: Slot, style: Style) -> usize {
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
        self.covering_slot(c, style).unwrap_or(Slot::Mono)
    }

    fn covering_slot(&self, c: char, style: Style) -> Option<Slot> {
        // Printable ASCII is most of what a terminal shows; skip the cmap walk.
        if self.ascii_in_mono && (' '..='~').contains(&c) {
            return Some(Slot::Mono);
        }
        // Arabic script blocks (plus Arabic-Indic digits) go to the Arabic face
        // even when the mono face nominally has a glyph, because mono coverage
        // of Arabic is unshaped and ugly.
        if is_arabic(c) && self.has_slot[Slot::Arabic as usize] {
            return Some(Slot::Arabic);
        }
        let mono = &self.faces[self.face_id(Slot::Mono, style)];
        if mono.covers(c) {
            return Some(Slot::Mono);
        }
        [Slot::Arabic, Slot::Symbols, Slot::Emoji, Slot::Fallback].into_iter().find(|&slot| {
            self.has_slot[slot as usize] && self.faces[self.face_id(slot, Style::default())].covers(c)
        })
    }

    /// The slot and face that should render `c`: the stack's own faces first,
    /// then any installed font that has the character, so Chinese, Hebrew or
    /// Devanagari text is drawn rather than left as empty boxes.
    pub fn pick(&mut self, c: char, style: Style) -> (Slot, usize) {
        if let Some(slot) = self.covering_slot(c, style) {
            return (slot, self.face_id(slot, style));
        }
        match self.installed_face(c) {
            Some(id) => (Slot::Fallback, id),
            None => (Slot::Mono, self.face_id(Slot::Mono, style)),
        }
    }

    fn installed_face(&mut self, c: char) -> Option<usize> {
        // Blanks, controls and invisible formatting draw nothing, and a
        // private-use codepoint means something different in every font that
        // has one, so another font's glyph for it would be wrong. Searching
        // for these would cost a full scan each, for nothing.
        if c.is_whitespace() || c.is_control() || invisible(c) || private_use(c) {
            return None;
        }
        if let Some(&hit) = self.fallback.get(&c) {
            return hit;
        }
        let hit = match self.extras.iter().copied().find(|&id| self.faces[id].covers(c)) {
            Some(id) => Some(id),
            None => self.load_covering(c),
        };
        self.fallback.insert(c, hit);
        hit
    }

    /// Load the first installed face that has `c`, preferred families first.
    /// Only each candidate's character map is read, through a memory map, so
    /// passing over a large CJK file costs little.
    fn load_covering(&mut self, c: char) -> Option<usize> {
        let has = |id: fontdb::ID| {
            self.db.with_face_data(id, |data, index| {
                rustybuzz::ttf_parser::Face::parse(data, index).is_ok_and(|f| f.glyph_index(c).is_some())
            }) == Some(true)
        };
        let real = |face: &fontdb::FaceInfo| {
            !face.families.iter().any(|(name, _)| PLACEHOLDER_FAMILIES.contains(&name.as_str()))
        };
        let preferred = FALLBACK_FAMILIES.iter().filter_map(|family| best_face(&self.db, family, Style::default()));
        let others = self.db.faces().filter(|face| face.style == fontdb::Style::Normal);
        let info = preferred.chain(others).find(|&face| real(face) && has(face.id))?;
        let fontdb::Source::File(path) = &info.source else { return None };
        let face = match Face::load(path.to_str()?, info.index, Style::default()) {
            Ok(face) => face,
            Err(e) => {
                log::debug!("fallback for {c:?}: {e}");
                return None;
            }
        };
        log::debug!("fallback for {c:?}: {}", info.families.first().map_or("?", |(name, _)| name.as_str()));
        self.faces.push(face);
        self.scalers.push(swash::scale::ScaleContext::new());
        let id = self.faces.len() - 1;
        self.extras.push(id);
        Some(id)
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
        let variations = self.faces[font].weight.into_iter().map(|w| ("wght", w));
        let mut scaler = self.scalers[font].builder(swf).size(px).hint(true).variations(variations).build();
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
        let id = self.next_glyph_id;
        self.next_glyph_id += 1;
        Some(RasterGlyph {
            id,
            width: img.placement.width,
            height: img.placement.height,
            left: img.placement.left,
            top: img.placement.top,
            data: img.data,
            color,
        })
    }
}

/// Default-ignorable codepoints: joiners, direction marks, variation selectors
/// and tags, which change how neighbours render but have no glyph of their own.
fn invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F..=0x1160 | 0x17B4..=0x17B5 | 0x180B..=0x180F |
        0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F |
        0xFEFF | 0xFFA0 | 0xFFF0..=0xFFF8 | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A |
        0xE0000..=0xE0FFF)
}

fn private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF)
}

/// Arabic script, its supplements, presentation forms, and Arabic-Indic digits.
#[inline]
pub fn is_arabic(c: char) -> bool {
    matches!(c as u32,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF |
        0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
}
