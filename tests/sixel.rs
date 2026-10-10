//! Sixel decoding.

use alef_term::sixel::decode;

fn px(img: &alef_term::sixel::Image, x: u32, y: u32) -> [u8; 4] {
    let i = (y * img.width + x) as usize * 4;
    [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]]
}

#[test]
fn decodes_a_single_sixel_column() {
    // '@' is 0x40, so bits = 1: only the top pixel of the band is set.
    let img = decode(b"#1;2;100;0;0@").expect("decoded");
    assert_eq!(img.width, 1);
    assert!(img.height >= 6);
    assert_eq!(px(&img, 0, 0), [255, 0, 0, 255], "top pixel is red");
    assert_eq!(px(&img, 0, 1)[3], 0, "the rest of the band is transparent");
}

#[test]
fn a_full_band_sets_all_six_pixels() {
    // '~' is 0x7e, bits = 0x3f: every pixel in the band.
    let img = decode(b"#1;2;0;100;0~").expect("decoded");
    for y in 0..6 {
        assert_eq!(px(&img, 0, y), [0, 255, 0, 255], "row {y}");
    }
}

#[test]
fn repeat_expands_horizontally() {
    let img = decode(b"#1;2;0;0;100!10~").expect("decoded");
    assert_eq!(img.width, 10);
    for x in 0..10 {
        assert_eq!(px(&img, x, 0), [0, 0, 255, 255], "column {x}");
    }
}

#[test]
fn newline_starts_a_new_band_six_rows_down() {
    let img = decode(b"#1;2;100;0;0~-#2;2;0;0;100~").expect("decoded");
    assert!(img.height >= 12);
    assert_eq!(px(&img, 0, 0), [255, 0, 0, 255], "first band red");
    assert_eq!(px(&img, 0, 6), [0, 0, 255, 255], "second band blue");
}

#[test]
fn carriage_return_returns_to_the_left_edge() {
    // Draw red across, then return and overwrite the first column blue.
    let img = decode(b"#1;2;100;0;0!5~$#2;2;0;0;100~").expect("decoded");
    assert_eq!(img.width, 5);
    assert_eq!(px(&img, 0, 0), [0, 0, 255, 255], "overwritten");
    assert_eq!(px(&img, 1, 0), [255, 0, 0, 255], "untouched");
}

#[test]
fn raster_attributes_set_the_canvas_size() {
    let img = decode(b"\"1;1;20;12#1;2;100;100;100@").expect("decoded");
    assert_eq!(img.width, 20);
    assert_eq!(img.height, 12);
}

#[test]
fn hls_colours_are_converted() {
    // Pu=1 selects HLS: hue 0, lightness 50, saturation 100 is sixel blue.
    let img = decode(b"#1;1;0;50;100~").expect("decoded");
    let c = px(&img, 0, 0);
    assert_eq!(c[3], 255);
    assert!(c[2] > c[0] && c[2] > c[1], "expected a blue-dominant colour, got {c:?}");
}

#[test]
fn the_default_palette_is_used_when_no_colour_is_defined() {
    let img = decode(b"#3~").expect("decoded");
    let c = px(&img, 0, 0);
    assert_eq!(c[3], 255);
    assert!(c[1] > c[0] && c[1] > c[2], "palette entry 3 is green, got {c:?}");
}

#[test]
fn malformed_input_does_not_panic_or_allocate_wildly() {
    assert!(decode(b"").is_none());
    // Note that ASCII letters are *valid* sixel data (0x3f..=0x7e), so
    // "garbage" would legitimately decode. Use bytes outside the range.
    assert!(decode(b"\x01\x02\x03   ").is_none());
    // Absurd repeat counts and sizes must be refused, not honoured.
    assert!(decode(b"\"1;1;99999999;99999999@").is_none());
    let _ = decode(b"!999999999~");
    let _ = decode(b"#####;;;;;");
}

#[test]
fn a_small_picture_round_trips() {
    // Two bands of a 3-wide checker.
    let img = decode(b"#1;2;100;0;0~?~-#1?~?").expect("decoded");
    assert_eq!(img.width, 3);
    assert!(img.height >= 12);
    assert_eq!(px(&img, 0, 0)[3], 255);
    assert_eq!(px(&img, 1, 0)[3], 0);
    assert_eq!(px(&img, 2, 0)[3], 255);
    assert_eq!(px(&img, 0, 6)[3], 0);
    assert_eq!(px(&img, 1, 6)[3], 255);
}
