use alef_term::boxdraw;
use alef_term::color::Rgb;
use alef_term::render::{Canvas, Target};

#[test]
fn extra_leading_does_not_make_light_borders_heavy() {
    for (width, stroke) in [(9, 1), (18, 2), (27, 3)] {
        for height in [width * 2, width * 3, width * 4] {
            let mut pixels = vec![0; width * height];
            let mut canvas = Canvas { buf: &mut pixels, width, height };
            canvas.clear(Rgb(0, 0, 0));
            boxdraw::draw(&mut canvas, '─', 0.0, 0.0, width as f32, height as f32, Rgb(255, 255, 255));
            let ink_rows = pixels.chunks(width).filter(|row| row.iter().any(|&p| p != 0)).count();
            assert_eq!(ink_rows, stroke, "{width}x{height}");
            for row in pixels.chunks(width).filter(|row| row.iter().any(|&p| p != 0)) {
                assert!(row.iter().all(|&p| p != 0), "horizontal stroke must tile without gaps");
            }
        }
    }
}

#[test]
fn quadrant_blocks_tile_with_the_half_blocks() {
    let (width, height) = (9usize, 21usize);
    let ink = |chars: &[char]| {
        let mut pixels = vec![0; width * height];
        let mut canvas = Canvas { buf: &mut pixels, width, height };
        canvas.clear(Rgb(0, 0, 0));
        for &c in chars {
            assert!(boxdraw::is_procedural(c), "{c} must not come from the font");
            boxdraw::draw(&mut canvas, c, 0.0, 0.0, width as f32, height as f32, Rgb(255, 255, 255));
        }
        pixels.iter().map(|&p| p != 0).collect::<Vec<bool>>()
    };
    assert_eq!(ink(&['▘', '▝']), ink(&['▀']));
    assert_eq!(ink(&['▖', '▗']), ink(&['▄']));
    assert_eq!(ink(&['▘', '▖']), ink(&['▌']));
    assert_eq!(ink(&['▝', '▗']), ink(&['▐']));
    assert_eq!(ink(&['▛', '▗']), ink(&['█']));
    assert_eq!(ink(&['▜', '▖']), ink(&['█']));
    assert_eq!(ink(&['▚', '▞']), ink(&['█']));
    assert_eq!(ink(&['▙']), ink(&['▌', '▄']));
    assert_eq!(ink(&['▟']), ink(&['▐', '▄']));
}
