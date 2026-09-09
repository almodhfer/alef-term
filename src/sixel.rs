//! Sixel image decoding.
//!
//! Sixel encodes an image as columns of six vertical pixels, one character per
//! column, in bands six pixels tall. It is what `img2sixel`, gnuplot and a
//! number of TUI file managers use to draw pictures in a terminal.

/// A decoded image: tightly packed RGBA, top-left origin.
pub struct Image {
    /// Unique for the life of the process, so a GPU texture cache can key on it.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Image {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Image { id, width, height, rgba }
    }
}

/// Upper bounds, so a malformed or hostile stream cannot exhaust memory.
const MAX_DIM: u32 = 10_000;
const MAX_PIXELS: usize = 16 << 20;

/// The default sixel palette: 16 colours, as specified by VT340.
fn default_palette() -> Vec<[u8; 4]> {
    const P: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (20, 20, 80),
        (80, 13, 13),
        (20, 80, 20),
        (80, 20, 80),
        (20, 80, 80),
        (80, 80, 20),
        (53, 53, 53),
        (26, 26, 26),
        (33, 33, 60),
        (60, 26, 26),
        (33, 60, 33),
        (60, 33, 60),
        (33, 60, 60),
        (60, 60, 33),
        (80, 80, 80),
    ];
    let mut v: Vec<[u8; 4]> = P
        .iter()
        .map(|&(r, g, b)| {
            [(r as u32 * 255 / 100) as u8, (g as u32 * 255 / 100) as u8, (b as u32 * 255 / 100) as u8, 255]
        })
        .collect();
    v.resize(256, [0, 0, 0, 255]);
    v
}

/// Convert HLS (as sixel specifies it: hue 0-360, lightness and saturation
/// 0-100) to RGB.
fn hls_to_rgb(h: u32, l: u32, s: u32) -> [u8; 4] {
    let l = l as f32 / 100.0;
    let s = s as f32 / 100.0;
    // Sixel's hue origin is offset by 120 degrees from the usual convention.
    let h = ((h as f32 + 240.0) % 360.0) / 360.0;
    if s <= 0.0 {
        let v = (l * 255.0).round().clamp(0.0, 255.0) as u8;
        return [v, v, v, 255];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| -> u8 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0), 255]
}

/// Decode a sixel data string (everything after the `q` of the DCS header).
pub fn decode(data: &[u8]) -> Option<Image> {
    let mut palette = default_palette();
    let mut color = 0usize;

    // The canvas grows as the image is drawn, since raster attributes are
    // optional and often absent.
    let mut width = 0u32;
    let mut pixels: Vec<[u8; 4]> = Vec::new();
    let mut stride = 0u32;
    let mut height = 0u32;

    let mut x = 0u32;
    let mut band_top = 0u32;

    let mut i = 0usize;
    // Reads a `;`-separated parameter list starting at `i`.
    let read_params = |i: &mut usize, data: &[u8]| -> Vec<u32> {
        let mut out = vec![0u32];
        while *i < data.len() {
            match data[*i] {
                b'0'..=b'9' => {
                    let v = out.last_mut().unwrap();
                    *v = v.saturating_mul(10).saturating_add((data[*i] - b'0') as u32);
                }
                b';' => out.push(0),
                _ => break,
            }
            *i += 1;
        }
        out
    };

    // Grows the canvas so that (px, py) is addressable.
    macro_rules! ensure {
        ($px:expr, $py:expr) => {{
            let need_w = ($px + 1).max(width);
            let need_h = ($py + 1).max(height);
            if need_w > MAX_DIM || need_h > MAX_DIM {
                return None;
            }
            if need_w as usize * need_h as usize > MAX_PIXELS {
                return None;
            }
            if need_w > stride || need_h > height {
                let new_stride = need_w.max(stride).max(1);
                let new_height = need_h.max(height);
                let mut next = vec![[0u8, 0, 0, 0]; new_stride as usize * new_height as usize];
                for row in 0..height {
                    let src = row as usize * stride as usize;
                    let dst = row as usize * new_stride as usize;
                    next[dst..dst + width as usize]
                        .copy_from_slice(&pixels[src..src + width as usize]);
                }
                pixels = next;
                stride = new_stride;
                height = new_height;
            }
            width = need_w.max(width);
        }};
    }

    while i < data.len() {
        match data[i] {
            b'#' => {
                i += 1;
                let p = read_params(&mut i, data);
                let idx = p[0] as usize % 256;
                if p.len() >= 5 {
                    palette[idx] = match p[1] {
                        1 => hls_to_rgb(p[2], p[3], p[4]),
                        _ => [
                            (p[2].min(100) * 255 / 100) as u8,
                            (p[3].min(100) * 255 / 100) as u8,
                            (p[4].min(100) * 255 / 100) as u8,
                            255,
                        ],
                    };
                }
                color = idx;
            }
            b'"' => {
                i += 1;
                let p = read_params(&mut i, data);
                // Pan;Pad;Ph;Pv — only the declared size is useful here.
                if p.len() >= 4 && p[2] > 0 && p[3] > 0 {
                    let (w, h) = (p[2].min(MAX_DIM), p[3].min(MAX_DIM));
                    ensure!(w - 1, h - 1);
                }
            }
            b'!' => {
                i += 1;
                let p = read_params(&mut i, data);
                let count = p[0].min(MAX_DIM);
                if i < data.len() {
                    let c = data[i];
                    i += 1;
                    if (0x3f..=0x7e).contains(&c) {
                        let bits = c - 0x3f;
                        if count > 0 {
                            ensure!(x + count - 1, band_top + 5);
                            for n in 0..count {
                                for b in 0..6u32 {
                                    if bits & (1 << b) != 0 {
                                        let px = x + n;
                                        let py = band_top + b;
                                        pixels[py as usize * stride as usize + px as usize] =
                                            palette[color];
                                    }
                                }
                            }
                        }
                        x += count;
                    }
                }
                continue;
            }
            b'-' => {
                band_top += 6;
                x = 0;
                i += 1;
            }
            b'$' => {
                x = 0;
                i += 1;
            }
            0x3f..=0x7e => {
                let bits = data[i] - 0x3f;
                ensure!(x, band_top + 5);
                for b in 0..6u32 {
                    if bits & (1 << b) != 0 {
                        let py = band_top + b;
                        pixels[py as usize * stride as usize + x as usize] = palette[color];
                    }
                }
                x += 1;
                i += 1;
            }
            _ => i += 1, // whitespace and anything unrecognised
        }
    }

    if width == 0 || height == 0 {
        return None;
    }
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height {
        let base = row as usize * stride as usize;
        for col in 0..width {
            rgba.extend_from_slice(&pixels[base + col as usize]);
        }
    }
    Some(Image::new(width, height, rgba))
}
