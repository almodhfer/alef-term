//! Procedural box-drawing and block glyphs.
//!
//! Terminals expect box-drawing characters to tile seamlessly: a vertical bar
//! must meet the one in the row above with no seam. Font glyphs cannot promise
//! that here, because our line box is taller than the monospace face's natural
//! line — we enlarge it so Arabic tashkeel has room. Drawing these characters
//! ourselves makes them fill the cell exactly and always connect.

use crate::color::Rgb;
use crate::render::Target;

/// Stroke weight of each arm: 0 none, 1 light, 2 heavy, 3 double.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Arms {
    pub up: u8,
    pub down: u8,
    pub left: u8,
    pub right: u8,
}

const fn a(up: u8, down: u8, left: u8, right: u8) -> Arms {
    Arms { up, down, left, right }
}

/// Arms for a box-drawing character, or `None` to fall back to the font.
fn arms(c: char) -> Option<Arms> {
    let x = c as u32;
    Some(match x {
        0x2500 => a(0, 0, 1, 1), // ─
        0x2501 => a(0, 0, 2, 2), // ━
        0x2502 => a(1, 1, 0, 0), // │
        0x2503 => a(2, 2, 0, 0), // ┃
        // Corners: ┌ ┐ └ ┘ families (0x250C..=0x251B), 4 weight variants each.
        0x250C..=0x250F => {
            let v = x - 0x250C;
            a(0, if v & 2 != 0 { 2 } else { 1 }, 0, if v & 1 != 0 { 2 } else { 1 })
        }
        0x2510..=0x2513 => {
            let v = x - 0x2510;
            a(0, if v & 2 != 0 { 2 } else { 1 }, if v & 1 != 0 { 2 } else { 1 }, 0)
        }
        0x2514..=0x2517 => {
            let v = x - 0x2514;
            a(if v & 2 != 0 { 2 } else { 1 }, 0, 0, if v & 1 != 0 { 2 } else { 1 })
        }
        0x2518..=0x251B => {
            let v = x - 0x2518;
            a(if v & 2 != 0 { 2 } else { 1 }, 0, if v & 1 != 0 { 2 } else { 1 }, 0)
        }
        // Tees and cross: only the all-light and all-heavy members are laid out
        // regularly enough to derive; the mixed-weight ones are rare.
        0x251C => a(1, 1, 0, 1),
        0x2523 => a(2, 2, 0, 2),
        0x2524 => a(1, 1, 1, 0),
        0x252B => a(2, 2, 2, 0),
        0x252C => a(0, 1, 1, 1),
        0x2533 => a(0, 2, 2, 2),
        0x2534 => a(1, 0, 1, 1),
        0x253B => a(2, 0, 2, 2),
        0x253C => a(1, 1, 1, 1),
        0x254B => a(2, 2, 2, 2),
        // Double lines.
        0x2550 => a(0, 0, 3, 3), // ═
        0x2551 => a(3, 3, 0, 0), // ║
        0x2554 => a(0, 3, 0, 3), // ╔
        0x2557 => a(0, 3, 3, 0), // ╗
        0x255A => a(3, 0, 0, 3), // ╚
        0x255D => a(3, 0, 3, 0), // ╝
        0x2560 => a(3, 3, 0, 3), // ╠
        0x2563 => a(3, 3, 3, 0), // ╣
        0x2566 => a(0, 3, 3, 3), // ╦
        0x2569 => a(3, 0, 3, 3), // ╩
        0x256C => a(3, 3, 3, 3), // ╬
        // Rounded corners draw as their square equivalents; at terminal sizes
        // the arc is a pixel or two and not worth the complexity.
        0x256D => a(0, 1, 0, 1), // ╭
        0x256E => a(0, 1, 1, 0), // ╮
        0x256F => a(1, 0, 1, 0), // ╯
        0x2570 => a(1, 0, 0, 1), // ╰
        // Half lines.
        0x2574 => a(0, 0, 1, 0),
        0x2575 => a(1, 0, 0, 0),
        0x2576 => a(0, 0, 0, 1),
        0x2577 => a(0, 1, 0, 0),
        0x2578 => a(0, 0, 2, 0),
        0x2579 => a(2, 0, 0, 0),
        0x257A => a(0, 0, 0, 2),
        0x257B => a(0, 2, 0, 0),
        _ => return None,
    })
}

/// True when we render this character ourselves rather than via the font.
pub fn is_procedural(c: char) -> bool {
    let x = c as u32;
    arms(c).is_some() || matches!(x, 0x2580..=0x2593)
}

/// Draw `c` filling the cell rect exactly.
pub fn draw(canvas: &mut impl Target, c: char, x: f32, y: f32, w: f32, h: f32, fg: Rgb) {
    let x = x.round();
    let y = y.round();
    let cp = c as u32;

    // Block elements and shades.
    if (0x2580..=0x2593).contains(&cp) {
        match cp {
            0x2580 => canvas.fill_rect(x, y, w, (h / 2.0).round(), fg), // ▀
            0x2584 => {
                let hh = (h / 2.0).round();
                canvas.fill_rect(x, y + h - hh, w, hh, fg) // ▄
            }
            0x2588 => canvas.fill_rect(x, y, w, h, fg), // █
            0x258C => canvas.fill_rect(x, y, (w / 2.0).round(), h, fg), // ▌
            0x2590 => {
                let hw = (w / 2.0).round();
                canvas.fill_rect(x + w - hw, y, hw, h, fg) // ▐
            }
            // Eighth blocks from the bottom (2581..2587) and left (2589..258F).
            0x2581..=0x2587 => {
                let n = (cp - 0x2580) as f32;
                let bh = (h * n / 8.0).round();
                canvas.fill_rect(x, y + h - bh, w, bh, fg);
            }
            0x2589..=0x258F => {
                let n = 8.0 - (cp - 0x2588) as f32;
                canvas.fill_rect(x, y, (w * n / 8.0).round(), h, fg);
            }
            // Shades: stipple at 25% / 50% / 75%.
            0x2591 | 0x2592 | 0x2593 => {
                let step = match cp {
                    0x2591 => 4,
                    0x2592 => 2,
                    _ => 1,
                };
                let (x0, y0) = (x as i32, y as i32);
                for py in 0..h as i32 {
                    for px in 0..w as i32 {
                        let on = if step == 1 {
                            (px + py) % 4 != 0
                        } else {
                            (px + py * 2) % (step * 2) == 0
                        };
                        if on {
                            canvas.fill_rect(
                                (x0 + px) as f32,
                                (y0 + py) as f32,
                                1.0,
                                1.0,
                                fg,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
        return;
    }

    let Some(arm) = arms(c) else { return };

    // Match the monospace pitch, not the enlarged Arabic line box. Deriving
    // this from height made light borders look bold and made extra leading
    // change the stroke weight of every TUI on screen.
    let light = (w / 9.0).round().max(1.0);
    let heavy = (light * 2.0).max(2.0);
    let thick = |weight: u8| -> f32 {
        match weight {
            2 => heavy,
            _ => light,
        }
    };

    // Centre of the cell, snapped so opposing arms in adjacent cells align.
    let cx = x + (w / 2.0).floor();
    let cy = y + (h / 2.0).floor();

    // A double line is two light strokes straddling the centre.
    let gap = (light * 2.0).max(2.0);

    let mut hline = |from: f32, to: f32, at: f32, t: f32| {
        canvas.fill_rect(from, at - (t / 2.0).floor(), to - from, t, fg);
    };
    // Horizontal arms.
    for (weight, from, to) in [(arm.left, x, cx + light), (arm.right, cx, x + w)] {
        if weight == 0 {
            continue;
        }
        if weight == 3 {
            hline(from, to, cy - gap / 2.0, light);
            hline(from, to, cy + gap / 2.0, light);
        } else {
            hline(from, to, cy, thick(weight));
        }
    }
    // Vertical arms.
    let mut vline = |from: f32, to: f32, at: f32, t: f32| {
        canvas.fill_rect(at - (t / 2.0).floor(), from, t, to - from, fg);
    };
    for (weight, from, to) in [(arm.up, y, cy + light), (arm.down, cy, y + h)] {
        if weight == 0 {
            continue;
        }
        if weight == 3 {
            vline(from, to, cx - gap / 2.0, light);
            vline(from, to, cx + gap / 2.0, light);
        } else {
            vline(from, to, cx, thick(weight));
        }
    }
}
