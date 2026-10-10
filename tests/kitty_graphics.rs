//! The kitty graphics protocol, and APC extraction from the byte stream.

use alef_term::term::{Terminal, Vt};

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn term() -> (Terminal, Vt) {
    let mut t = Terminal::new(10, 40, 100);
    t.set_cell_px(8.0, 16.0);
    (t, Vt::new())
}

#[test]
fn transmits_and_displays_raw_rgba() {
    let (mut t, mut vt) = term();
    // 2x2 red, fully opaque.
    let px: Vec<u8> = std::iter::repeat([255u8, 0, 0, 255]).take(4).flatten().collect();
    let seq = format!("\x1b_Ga=T,f=32,s=2,v=2;{}\x1b\\", b64(&px));
    vt.feed(&mut t, seq.as_bytes());

    assert_eq!(t.images.len(), 1);
    let img = &t.images[0].image;
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(&img.rgba[..4], &[255, 0, 0, 255]);
}

#[test]
fn rgb_without_alpha_becomes_opaque() {
    let (mut t, mut vt) = term();
    let px: Vec<u8> = std::iter::repeat([0u8, 0, 255]).take(4).flatten().collect();
    let seq = format!("\x1b_Ga=T,f=24,s=2,v=2;{}\x1b\\", b64(&px));
    vt.feed(&mut t, seq.as_bytes());
    let img = &t.images[0].image;
    assert_eq!(&img.rgba[..4], &[0, 0, 255, 255]);
}

#[test]
fn a_chunked_payload_is_reassembled() {
    let (mut t, mut vt) = term();
    let px: Vec<u8> = std::iter::repeat([0u8, 255, 0, 255]).take(4).flatten().collect();
    let all = b64(&px);
    let (a, b) = all.split_at(8);
    // m=1 on every chunk but the last.
    vt.feed(&mut t, format!("\x1b_Ga=T,f=32,s=2,v=2,m=1;{a}\x1b\\").as_bytes());
    assert!(t.images.is_empty(), "nothing placed until the last chunk");
    vt.feed(&mut t, format!("\x1b_Gm=0;{b}\x1b\\").as_bytes());
    assert_eq!(t.images.len(), 1);
    assert_eq!(&t.images[0].image.rgba[..4], &[0, 255, 0, 255]);
}

#[test]
fn apc_is_stripped_and_surrounding_text_still_prints() {
    // vte swallows APC without a callback, so the sequence is pulled out of the
    // stream first — the text either side must be unaffected.
    let (mut t, mut vt) = term();
    let px: Vec<u8> = std::iter::repeat([1u8, 2, 3, 255]).take(1).flatten().collect();
    let seq = format!("before\x1b_Ga=T,f=32,s=1,v=1;{}\x1b\\after", b64(&px));
    vt.feed(&mut t, seq.as_bytes());

    let text: String = (0..3)
        .map(|r| {
            t.grid.screen.rows[r].cells.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("|");
    assert!(text.starts_with("before"), "text before the image: {text}");
    assert!(text.contains("after"), "text after the image: {text}");
    assert_eq!(t.images.len(), 1);
}

#[test]
fn an_apc_split_across_feeds_still_works() {
    // The PTY hands us arbitrary chunks, so a sequence can be cut anywhere.
    let (mut t, mut vt) = term();
    let px: Vec<u8> = std::iter::repeat([9u8, 9, 9, 255]).take(1).flatten().collect();
    let seq = format!("\x1b_Ga=T,f=32,s=1,v=1;{}\x1b\\", b64(&px));
    let bytes = seq.as_bytes();
    for cut in [1usize, 2, 5, bytes.len() - 1] {
        let (mut t, mut vt) = term();
        vt.feed(&mut t, &bytes[..cut]);
        vt.feed(&mut t, &bytes[cut..]);
        assert_eq!(t.images.len(), 1, "split at {cut} lost the image");
    }
    let _ = (&mut t, &mut vt);
}

#[test]
fn delete_clears_placed_images() {
    let (mut t, mut vt) = term();
    let px: Vec<u8> = std::iter::repeat([1u8, 1, 1, 255]).take(1).flatten().collect();
    vt.feed(&mut t, format!("\x1b_Ga=T,f=32,s=1,v=1;{}\x1b\\", b64(&px)).as_bytes());
    assert_eq!(t.images.len(), 1);
    vt.feed(&mut t, b"\x1b_Ga=d\x1b\\");
    assert!(t.images.is_empty());
}

#[test]
fn a_png_payload_is_decoded() {
    // A minimal 1x1 opaque red PNG.
    let png: Vec<u8> = {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 1, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[255, 0, 0, 255]).unwrap();
        }
        out
    };
    let (mut t, mut vt) = term();
    vt.feed(&mut t, format!("\x1b_Ga=T,f=100;{}\x1b\\", b64(&png)).as_bytes());
    assert_eq!(t.images.len(), 1);
    assert_eq!((t.images[0].image.width, t.images[0].image.height), (1, 1));
    assert_eq!(&t.images[0].image.rgba[..4], &[255, 0, 0, 255]);
}

#[test]
fn malformed_graphics_commands_are_ignored() {
    let (mut t, mut vt) = term();
    vt.feed(&mut t, b"\x1b_Ga=T,f=32,s=99999,v=99999;!!!\x1b\\");
    vt.feed(&mut t, b"\x1b_G\x1b\\");
    vt.feed(&mut t, b"\x1b_Gnonsense\x1b\\");
    vt.feed(&mut t, b"\x1b_Xnot ours\x1b\\");
    assert!(t.images.is_empty());
    // And the terminal still works.
    vt.feed(&mut t, b"ok");
    let l: String = t.grid.screen.rows[0].cells.iter().map(|c| c.ch).collect();
    assert!(l.starts_with("ok"));
}
