//! Frame-time benchmark for the render path.
//!
//! Measures the cases a full-screen program actually produces, not just a
//! static screen: a warm frame where nothing changed, a frame where one row
//! changed, and a frame where every row changed, at a QHD-sized grid.
use arabic_term::font::Fonts;
use arabic_term::gpu::Gpu;
use arabic_term::render::{Canvas, DrawOpts, Renderer};
use arabic_term::term::{Terminal, Vt};
use std::time::Instant;

struct Rig {
    fonts: Fonts,
    term: Terminal,
    vt: Vt,
    renderer: Renderer,
    buf: Vec<u32>,
    w: usize,
    h: usize,
}

impl Rig {
    fn new(size: f32, rows: usize, cols: usize) -> Rig {
        let fonts = Fonts::new(size, None).unwrap();
        let w = (cols as f32 * fonts.cell_w) as usize + 8;
        let h = (rows as f32 * fonts.cell_h) as usize + 4;
        Rig {
            fonts,
            term: Terminal::new(rows, cols, 1000),
            vt: Vt::new(),
            renderer: Renderer::new(),
            buf: vec![0u32; w * h],
            w,
            h,
        }
    }

    fn write_row(&mut self, r: usize, s: &str) {
        self.vt.feed(&mut self.term, format!("\x1b[{};1H\x1b[2K", r + 1).as_bytes());
        self.vt.feed(&mut self.term, s.as_bytes());
    }

    fn fill(&mut self, gen: &dyn Fn(usize) -> String) {
        for r in 0..self.term.grid.rows {
            self.write_row(r, &gen(r));
        }
    }

    fn draw(&mut self) -> f64 {
        let t0 = Instant::now();
        let mut c = Canvas { buf: &mut self.buf, width: self.w, height: self.h };
        self.renderer.draw(&mut c, &self.term, &mut self.fonts, &DrawOpts::default());
        t0.elapsed().as_secs_f64() * 1000.0
    }

    /// Draw through the GPU compositor: the CPU time to build and submit the
    /// frame, which is what the app pays, and separately the wait for the GPU
    /// to finish, which the app overlaps with the next frame.
    fn draw_gpu(&mut self, gpu: &mut Gpu) -> (f64, f64) {
        let t0 = Instant::now();
        gpu.begin();
        self.renderer.draw(gpu, &self.term, &mut self.fonts, &DrawOpts::default());
        let t1 = Instant::now();
        gpu.render_headless().unwrap();
        let t2 = Instant::now();
        ((t1 - t0).as_secs_f64() * 1000.0, (t2 - t1).as_secs_f64() * 1000.0)
    }
}

fn report(name: &str, rig: &Rig, ms: f64) {
    println!(
        "{name:36} {}x{} @ {}x{}px  {ms:7.2} ms/frame",
        rig.term.grid.rows, rig.term.grid.cols, rig.w, rig.h
    );
}

fn report_gpu(name: &str, rig: &Rig, cpu: f64, wait: f64) {
    println!(
        "{name:36} {}x{} @ {}x{}px  {cpu:7.2} ms/frame cpu, +{wait:.2} ms gpu wait",
        rig.term.grid.rows, rig.term.grid.cols, rig.w, rig.h
    );
}

fn scenario(label: &str, size: f32, rows: usize, cols: usize, gen: &dyn Fn(usize, usize) -> String) {
    let mut rig = Rig::new(size, rows, cols);
    rig.fill(&|r| gen(r, 0));
    for _ in 0..3 {
        rig.draw();
    }

    // Static: every row's layout is cached, every glyph is cached.
    let n = 30;
    let mut total = 0.0;
    for _ in 0..n {
        total += rig.draw();
    }
    report(&format!("{label}: static"), &rig, total / n as f64);

    // One row changed per frame, the common interactive case.
    let mut total = 0.0;
    for f in 1..=n {
        rig.write_row(rows / 2, &gen(rows / 2, f));
        total += rig.draw();
    }
    report(&format!("{label}: one row changed"), &rig, total / n as f64);

    // Every row changed: a full repaint with fresh content, as when a
    // multiplexer redraws a pane or a program prints a new screen.
    let mut total = 0.0;
    for f in 1..=n {
        rig.fill(&|r| gen(r, f));
        total += rig.draw();
    }
    report(&format!("{label}: all rows changed"), &rig, total / n as f64);
}

/// The same static and all-rows-changed cases, composited on the GPU.
fn gpu_scenario(label: &str, size: f32, rows: usize, cols: usize, gen: &dyn Fn(usize, usize) -> String) {
    let mut rig = Rig::new(size, rows, cols);
    let Ok(mut gpu) = Gpu::headless(rig.w as u32, rig.h as u32) else {
        println!("{label}: no GPU available");
        return;
    };
    rig.fill(&|r| gen(r, 0));
    for _ in 0..3 {
        rig.draw_gpu(&mut gpu);
    }
    let n = 30;
    let (mut cpu, mut wait) = (0.0, 0.0);
    for _ in 0..n {
        let (c, g) = rig.draw_gpu(&mut gpu);
        cpu += c;
        wait += g;
    }
    report_gpu(&format!("{label}: static (gpu)"), &rig, cpu / n as f64, wait / n as f64);
    let (mut cpu, mut wait) = (0.0, 0.0);
    for f in 1..=n {
        rig.fill(&|r| gen(r, f));
        let (c, g) = rig.draw_gpu(&mut gpu);
        cpu += c;
        wait += g;
    }
    report_gpu(&format!("{label}: all rows changed (gpu)"), &rig, cpu / n as f64, wait / n as f64);
}

fn main() {
    // BENCH_LOOP=1 runs the all-rows-changed TUI case forever, for `sample`.
    if std::env::var_os("BENCH_LOOP").is_some() {
        let tui = |r: usize, f: usize| {
            let mut s = format!("│ workspace {r:>3} │ ");
            for w in 0..18 {
                s.push_str(&format!("word{} ", (r * 7 + w + f) % 97));
            }
            s.push_str("│ ");
            for w in 0..14 {
                s.push_str(&format!("token{} ", (r * 3 + w * 5 + f) % 89));
            }
            s.push_str("│");
            s
        };
        let mut rig = Rig::new(15.0, 75, 284);
        let mut f = 0;
        loop {
            f += 1;
            rig.fill(&|r| tui(r, f));
            rig.draw();
        }
    }
    let ascii = |r: usize, f: usize| {
        format!("row {r} frame {f} the quick brown fox jumps over the lazy dog 0123456789 and again the quick brown fox jumps over the lazy dog")
    };
    // A multiplexer-style row: a sidebar, pane borders, and prose in the panes.
    let tui = |r: usize, f: usize| {
        let mut s = format!("│ workspace {r:>3} │ ");
        for w in 0..18 {
            s.push_str(&format!("word{} ", (r * 7 + w + f) % 97));
        }
        s.push_str("│ ");
        for w in 0..14 {
            s.push_str(&format!("token{} ", (r * 3 + w * 5 + f) % 89));
        }
        s.push_str("│");
        s
    };
    let arabic = |r: usize, f: usize| {
        format!("{r} {f} مرحبا بالعالم من الطرفية العربية التي تدعم النص العربي ومعه نص لاتيني latin text")
    };

    println!("--- 50x200 (the historical numbers) ---");
    scenario("ascii", 15.0, 50, 200, &ascii);
    println!("--- QHD full screen, 1x scale: 75x284 ---");
    scenario("ascii", 15.0, 75, 284, &ascii);
    scenario("tui", 15.0, 75, 284, &tui);
    scenario("arabic", 15.0, 75, 284, &arabic);
    println!("--- retina full screen, 2x scale: 75x284 at 30px ---");
    scenario("tui @2x", 30.0, 75, 284, &tui);
    println!("--- the same, composited on the GPU ---");
    gpu_scenario("tui", 15.0, 75, 284, &tui);
    gpu_scenario("tui @2x", 30.0, 75, 284, &tui);
}
