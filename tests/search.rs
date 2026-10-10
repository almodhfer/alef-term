//! Scrollback search: what is found, where, and how fast.
use alef_term::search::{Index, Search};
use alef_term::selection::Span;
use alef_term::term::{Terminal, Vt};

fn term(rows: usize, cols: usize, input: &str) -> Terminal {
    let mut t = Terminal::new(rows, cols, 100_000);
    Vt::new().feed(&mut t, input.as_bytes());
    t
}

fn find(t: &Terminal, q: &str) -> Vec<Span> {
    let mut ix = Index::default();
    ix.build(&t.grid);
    ix.find(q).1
}

#[test]
fn finds_text_on_screen_and_in_scrollback() {
    let t = term(2, 20, "alpha beta\r\ngamma\r\nbeta again\r\nlast");
    // Two lines retired to scrollback, two on screen.
    assert_eq!(t.grid.scrollback.len(), 2);
    let spans = find(&t, "beta");
    assert_eq!(spans, vec![Span { line: 0, start: 6, end: 10 }, Span { line: 2, start: 0, end: 4 }]);
}

#[test]
fn case_is_ignored_unless_the_query_has_capitals() {
    let t = term(3, 20, "Hello hello HELLO");
    assert_eq!(find(&t, "hello").len(), 3);
    assert_eq!(find(&t, "Hello").len(), 1);
    assert_eq!(find(&t, "HELLO").len(), 1);
}

#[test]
fn arabic_is_found_with_or_without_harakat() {
    let t = term(3, 40, "قال: بِسْمِ اللَّهِ");
    let spans = find(&t, "بسم");
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].start, spans[0].end), (5, 8));
    assert_eq!(find(&t, "بِسْمِ").len(), 1, "marks in the query are ignored");
    assert_eq!(find(&t, "الله").len(), 1);
}

#[test]
fn arabic_spelling_variants_find_each_other() {
    // Hamza seats on alef, alef maqsura and ta marbuta are written
    // interchangeably, so each spelling finds the others.
    let t = term(3, 40, "أحمد في المدرسة\r\nإلى آخر مستشفى");
    let spans = find(&t, "احمد");
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].start, spans[0].end), (0, 4), "the hit covers the original letters");
    assert_eq!(find(&t, "أحمد").len(), 1);
    assert_eq!(find(&t, "المدرسه").len(), 1);
    assert_eq!(find(&t, "الى").len(), 1);
    assert_eq!(find(&t, "اخر").len(), 1);
    assert_eq!(find(&t, "مستشفي").len(), 1);
    assert_eq!(find(&t, "إلي").len(), 1, "variants in the query fold too");
}

#[test]
fn a_hit_across_a_soft_wrap_has_one_span_per_row() {
    let t = term(4, 10, "xxxxxxxxneedlexxxx");
    // "xxxxxxxxne" | "edlexxxx"
    let (matches, spans) = {
        let mut ix = Index::default();
        ix.build(&t.grid);
        ix.find("needle")
    };
    assert_eq!(matches.len(), 1);
    assert_eq!(spans, vec![Span { line: 0, start: 8, end: 10 }, Span { line: 1, start: 0, end: 4 }]);
    assert_eq!(matches[0].count, 2);
}

#[test]
fn hits_never_cross_an_explicit_newline() {
    let t = term(4, 20, "need\r\nle");
    assert!(find(&t, "needle").is_empty());
}

#[test]
fn wide_characters_take_two_cells_in_spans() {
    let t = term(3, 20, "ab日本語cd");
    let spans = find(&t, "語cd");
    assert_eq!(spans, vec![Span { line: 0, start: 6, end: 10 }]);
}

#[test]
fn the_index_is_rebuilt_only_when_the_content_changes() {
    let mut t = term(3, 20, "one");
    let mut ix = Index::default();
    assert!(ix.update(&t.grid, 1));
    assert!(!ix.update(&t.grid, 1), "same epoch, no rebuild");
    Vt::new().feed(&mut t, b"\r\ntwo");
    assert!(ix.update(&t.grid, 2));
    assert_eq!(ix.find("two").0.len(), 1);
}

#[test]
fn stepping_wraps_and_starts_from_the_bottom() {
    let t = term(3, 20, "a\r\na\r\na\r\nzz");
    let mut ix = Index::default();
    ix.build(&t.grid);
    let mut s = Search { query: "a".into(), ..Default::default() };
    s.run(&ix, None);
    assert_eq!(s.matches.len(), 3);
    assert_eq!(s.current, Some(2), "the newest hit first");
    s.step(-1);
    assert_eq!(s.current, Some(1));
    s.step(-1);
    s.step(-1);
    assert_eq!(s.current, Some(2), "wrapped around");
    // Re-running near a line keeps the hit at or above it.
    s.run(&ix, Some(1));
    assert_eq!(s.current, Some(1));
    s.run(&ix, Some(0));
    assert_eq!(s.current, Some(0));
}

#[test]
fn indexing_and_searching_a_large_history_is_fast() {
    let mut t = Terminal::new(50, 160, 100_000);
    let mut vt = Vt::new();
    let mut input = String::new();
    for i in 0..40_000 {
        input.push_str(&format!("line {i}: the quick brown fox jumps over the lazy dog مرحبا بالعالم {}\r\n", i % 7));
    }
    vt.feed(&mut t, input.as_bytes());
    assert!(t.grid.scrollback.len() > 39_000);
    let mut ix = Index::default();
    let t0 = std::time::Instant::now();
    ix.build(&t.grid);
    let build = t0.elapsed();
    let t1 = std::time::Instant::now();
    let (m, _) = ix.find("lazy dog");
    let (m2, _) = ix.find("بالعالم");
    let search = t1.elapsed();
    eprintln!("index of 40k lines: build {:.1} ms, two searches {:.2} ms", build.as_secs_f64() * 1e3, search.as_secs_f64() * 1e3);
    assert!(m.len() >= 20_000 && m2.len() >= 20_000);
    assert!(build.as_millis() < 150, "build took {build:?}");
    assert!(search.as_millis() < 60, "search took {search:?}");
}

#[test]
fn hits_and_the_bar_are_painted() {
    use alef_term::font::Fonts;
    use alef_term::render::{Canvas, DrawOpts, Renderer, SearchBar};
    let t = term(6, 30, "find the needle here\r\nand another needle");
    let mut ix = Index::default();
    ix.build(&t.grid);
    let mut s = Search { query: "needle".into(), ..Default::default() };
    s.run(&ix, None);
    assert_eq!(s.matches.len(), 2);

    let mut fonts = Fonts::new(15.0, None).unwrap();
    let mut renderer = Renderer::new();
    let w = (30.0 * fonts.cell_w) as usize + 8;
    let h = (6.0 * fonts.cell_h) as usize + 4;
    let mut buf = vec![0u32; w * h];
    let mut canvas = Canvas { buf: &mut buf, width: w, height: h };
    let opts = DrawOpts {
        highlights: &s.spans,
        current: s.current_spans(),
        search: Some(SearchBar { query: &s.query, total: s.matches.len(), current: s.current }),
        ..Default::default()
    };
    renderer.draw(&mut canvas, &t, &mut fonts, &opts);

    let pal = &t.palette;
    let count = |c: alef_term::color::Rgb| buf.iter().filter(|&&p| p == c.pack()).count();
    assert!(count(pal.search_bg) > 50, "the other hit is highlighted");
    assert!(count(pal.search_current_bg) > 50, "the current hit is highlighted more strongly");
    // The bar's border is in the cursor colour, at the top right.
    let x = w - 20;
    let border = (0..12).any(|y| buf[y * w + x] == pal.cursor.pack());
    assert!(border, "search bar drawn at the top right");
}
