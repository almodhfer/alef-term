//! The kitty graphics protocol.
//!
//! Images arrive as APC sequences: `ESC _ G <key=value,...> ; <base64> ESC \`.
//! A payload may be split across several such sequences, chained with `m=1` on
//! every chunk but the last.

use crate::sixel::Image;
use std::collections::HashMap;

/// What a decoded command asks the terminal to do.
pub enum Command {
    /// Display this image at the cursor.
    Display(Image),
    /// Forget stored images. `None` clears everything.
    Delete(Option<u32>),
    /// Nothing to do yet — a chunk was stored, or the command was ignored.
    None,
}

#[derive(Default)]
pub struct Decoder {
    /// Payload accumulated from `m=1` chunks, with the keys of the first chunk.
    pending: Vec<u8>,
    pending_keys: HashMap<String, String>,
    /// Images transmitted but not yet displayed, by id.
    stored: HashMap<u32, Image>,
}

/// Payload and pixel-count ceilings, so a malformed stream cannot exhaust memory.
const MAX_PAYLOAD: usize = 32 << 20;
const MAX_PIXELS: usize = 16 << 20;

fn parse_keys(s: &str) -> HashMap<String, String> {
    s.split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

fn num(keys: &HashMap<String, String>, k: &str) -> Option<u32> {
    keys.get(k)?.parse().ok()
}

impl Decoder {
    /// Feed one APC payload (everything between `ESC _` and `ESC \`).
    pub fn feed(&mut self, apc: &[u8]) -> Command {
        // Only `G` is ours; other APC users are left alone.
        let Some(body) = apc.strip_prefix(b"G") else { return Command::None };
        let body = std::str::from_utf8(body).unwrap_or("");
        let (control, payload) = match body.split_once(';') {
            Some((c, p)) => (c, p),
            None => (body, ""),
        };
        let keys = parse_keys(control);

        if keys.get("a").map(|s| s.as_str()) == Some("d") {
            let id = num(&keys, "i");
            if id.is_none() {
                self.stored.clear();
            } else if let Some(i) = id {
                self.stored.remove(&i);
            }
            return Command::Delete(id);
        }

        // Accumulate chunks. `m=1` means more is coming.
        let more = num(&keys, "m").unwrap_or(0) == 1;
        if self.pending.is_empty() {
            self.pending_keys = keys.clone();
        }
        if self.pending.len() + payload.len() > MAX_PAYLOAD {
            self.pending.clear();
            self.pending_keys.clear();
            return Command::None;
        }
        self.pending.extend_from_slice(payload.as_bytes());
        if more {
            return Command::None;
        }

        // The action comes from the *first* chunk: a continuation carries only
        // `m=0`, so reading it from the chunk in hand would silently turn a
        // display into a transmit.
        let keys = std::mem::take(&mut self.pending_keys);
        let action = keys.get("a").map(|s| s.as_str()).unwrap_or("t");
        let encoded = std::mem::take(&mut self.pending);
        let Some(bytes) = base64_decode(&encoded) else { return Command::None };

        let format = num(&keys, "f").unwrap_or(32);
        let img = match format {
            100 => decode_png(&bytes),
            24 => raw_to_rgba(&bytes, &keys, 3),
            32 => raw_to_rgba(&bytes, &keys, 4),
            _ => None,
        };
        let Some(img) = img else { return Command::None };

        match action {
            // Transmit only: keep it for a later placement.
            "t" => {
                if let Some(id) = num(&keys, "i") {
                    if self.stored.len() < 64 {
                        self.stored.insert(id, img);
                    }
                }
                Command::None
            }
            // Transmit and display, or place a stored image.
            _ => Command::Display(img),
        }
    }

    /// Look up an image transmitted earlier.
    pub fn take_stored(&mut self, id: u32) -> Option<Image> {
        self.stored.remove(&id)
    }
}

fn raw_to_rgba(bytes: &[u8], keys: &HashMap<String, String>, channels: usize) -> Option<Image> {
    let w = num(keys, "s")?;
    let h = num(keys, "v")?;
    if w == 0 || h == 0 || (w as usize).checked_mul(h as usize)? > MAX_PIXELS {
        return None;
    }
    let need = w as usize * h as usize * channels;
    if bytes.len() < need {
        return None;
    }
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for px in bytes[..need].chunks_exact(channels) {
        rgba.extend_from_slice(&[px[0], px[1], px[2], if channels == 4 { px[3] } else { 255 }]);
    }
    Some(Image { width: w, height: h, rgba })
}

fn decode_png(bytes: &[u8]) -> Option<Image> {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = dec.read_info().ok()?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    if (w as usize).checked_mul(h as usize)? > MAX_PIXELS {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let data = &buf[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => {
            data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        }
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => {
            data.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        _ => return None,
    };
    Some(Image { width: w, height: h, rgba })
}

/// Standard base64, tolerating padding and whitespace.
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    #[inline]
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in input {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}
