# arabic-term

A terminal emulator, written in Rust, that renders Arabic correctly.

Arabic breaks in most terminals for a structural reason: a terminal is a grid of
fixed cells, but Arabic needs the opposite of fixed cells. Letters change shape
depending on their neighbours, some pairs must fuse into a single glyph, vowel
marks stack on top of the letter they belong to without taking any width, and
the whole line runs right-to-left while any numbers or Latin words inside it
still run left-to-right. A terminal that stamps one glyph per cell cannot do
any of that, so Arabic comes out as disconnected letters in the wrong order.

`arabic-term` keeps the grid — every column stays exactly one cell wide, so
`vim`, `htop` and `git log --graph` keep working — and does the Arabic work at
draw time instead.

![showcase](docs/showcase.png)

## How it works

**The grid stores logical order.** Cells hold exactly the characters the program
wrote, in the order it wrote them. Nothing is reordered or pre-shaped in
storage, so the column arithmetic every full-screen program relies on stays
true, and copying text gives back what the program actually emitted.

**Reordering and shaping happen at render time**, per row:

1. **Segment.** The row is cut at *reorder barriers* — box-drawing characters
   and any run of two or more blanks. Text reorders freely inside a field, but
   never across one. This is what stops a single Arabic word from dragging a
   table's column separator, or a whole `ls` column, across the screen.
2. **Resolve.** Each segment goes through the Unicode Bidirectional Algorithm
   (UAX #9) to get runs of uniform direction, in visual order.
3. **Assign columns.** Each run's cells are laid onto the columns it owns —
   reversed for a right-to-left run. This is a *permutation of cells onto
   columns*, never a change of column widths, so backgrounds, selection, the
   cursor and mouse hit-testing stay exact.
4. **Shape.** Each run is shaped whole by [rustybuzz], so letters join across
   cell boundaries, لا becomes one ligature, and diacritics get their proper
   GPOS anchors. Runs are split by font, **never by colour** — a colour change
   in the middle of a word must not break the joining.
5. **Place.** Cursive Arabic is placed as one piece, with the font's own
   advances, inside the columns its word occupies. Everything else is snapped
   to its cell — `cell_w` is a whole number of pixels but a glyph advance is
   not, so a long Latin run laid out by advances drifts about a third of a pixel
   per character and collides within one status bar's width. Snapping is safe
   there precisely because Latin has no joins to protect.

**Pre-shaped Arabic is undone on the way in.** Editors that predate shaping
terminals convert Arabic to Arabic Presentation Forms before writing it out —
vim and neovim do this by default, via `'arabicshape'`. Those codepoints are
frozen letter shapes, so passing them to a real shaper yields disconnected
letters. Each one that maps back to a single base letter is restored, letting
the shaper join properly; multi-character ligature forms are left alone, since
they are already one correct glyph in one cell.

### Two decisions worth explaining

**Base direction is pinned left-to-right by default.** Auto-detecting a row's
direction from its content sounds right and behaves badly: the prompt jumps
between edges as you type, `git log --graph` loses its gutter, and any table
whose first strong character happens to be Arabic flips whole. Pinning the base
keeps the frame still while Arabic runs reorder inside it, which is what
actually reads correctly. Press `Cmd R` to cycle to forced-RTL or auto, and
programs can still request RTL inline with `U+200F`/`U+202B`/`U+2067`.

**The Arabic font is scaled to match the cell.** Naskh faces set much narrower
on the em than a monospace Latin face, so Arabic drawn at the Latin size looks
thin and detached from its columns. The scale is derived at startup by shaping
sample words and comparing their width to the columns they would occupy, then
backing off slightly — overshooting is worse than undershooting, because a word
wider than its columns spills into its neighbour and swallows the space between
words. Override with `--arabic-scale` if you prefer.

Box-drawing characters are drawn procedurally rather than from the font, because
the line box is taller than the monospace face's natural line — it has to be, to
leave room for stacked diacritics — and font glyphs would no longer tile.

### Stated policies

These are deliberate choices, not omissions:

- **No contextual digit substitution.** `123` is displayed as `123`, never as
  `١٢٣`. The terminal shows the codepoints the program sent; anything else makes
  `grep`, column arithmetic and copy/paste disagree with the screen.
- **No kashida justification.** Slack goes to the gaps between words, not into
  synthetically elongated letters.
- **Copy yields logical order**, which is what a shell expects back.
- **Paste is sanitised**: control characters are stripped, so a pasted
  right-to-left override cannot make a command line display as something other
  than what runs.

## Build and run

```sh
cargo build --release
./target/release/arabic-term
```

Requires Rust 1.85+ and macOS. Fonts are picked in this order:

| Slot | Preferred | Falls back to |
| --- | --- | --- |
| monospace | JetBrains Mono Nerd Font Mono (`~/Library/Fonts`) | Menlo, Monaco, Courier |
| Arabic | Geeza Pro | Al Nile |
| icons | whatever the monospace face covers | Symbols Nerd Font Mono |
| emoji | Apple Color Emoji | |

A Nerd Font is preferred for monospace because it carries the private-use icons
that status bars, file trees and workspace managers draw with; without one they
render as empty boxes. The "Mono" cut keeps those icons one cell wide.

```
--font-size <px>        base font size (default 15)
--arabic-scale <f>      Arabic face scale (default: derived from metrics)
--exec <cmd>            run a command at startup
```

| Key | Action |
| --- | --- |
| `Cmd` `+` / `-` / `0` | font size |
| `Cmd R` | cycle base direction: LTR → RTL → auto |
| `Cmd C` / `Cmd V` | copy / paste |
| scroll wheel | scrollback (arrow keys on the alt screen) |

## Known limitations

- **Built and tested on macOS.** Fonts are found through the platform font
  database, so the stack is not macOS-specific, but nothing else has been run on
  Linux or Windows and the clipboard is whatever `arboard` provides.
- **No kitty graphics protocol.** Sixel is supported; kitty's newer image
  protocol is not.
- **Little hands-on interactive use.** Everything is covered by tests or by
  driving real programs over a PTY, but the window itself has had few hours of
  real use; IME-based Arabic input is the least exercised path.

## What's implemented

Colours (16 / 256 / truecolour, both `;` and `:` SGR forms), bold/dim/italic/
underline/strikeout/inverse, cursor addressing, erase and insert/delete of
characters and lines, `DECSTBM` scroll regions, the alternate screen, scrollback,
autowrap with deferred wrap at the margin, wide (CJK) characters with spacers,
combining marks, DEC line-drawing charset, mouse reporting (normal and SGR),
bracketed paste, focus events, window title (OSC 0/2), palette changes (OSC 4),
hyperlinks (OSC 8), clipboard writes (OSC 52), device status reports, the kitty
keyboard protocol's disambiguation flag, sixel images, selection and clipboard,
and resize propagated to the child via `TIOCSWINSZ`.

Sixel images are pinned to the line they were drawn on, so they scroll with
their text:

![sixel](docs/sixel.png)

## Testing

```sh
cargo test
```

96 tests, covering: VT and grid behaviour; font coverage and fallback; the
Arabic layout itself (reordering, ligatures, diacritics, bracket mirroring,
column pinning, cell-grid alignment, joining across a soft wrap); key encoding
including the kitty protocol; selection and copy; sixel decoding; and end-to-end
tests that drive a real shell — and a real editor — over a PTY, typing Arabic
through the key encoder and interrupting a running command with Ctrl+C.

`cargo run --release --example tui_probe -- /bin/sh "htop\n<<PAUSE>>"` drives a
full-screen program through the terminal and dumps both the grid and a render,
which is how the editor and htop cases above were verified.

`cargo run --release --example render_test` renders a showcase screen to a file
without opening a window, which is handy for eyeballing layout changes.
`cargo run --release --example bench` reports frame times; a 50×200 grid at
Retina resolution renders in about 3 ms, re-laying out every row every frame.

## Layout

| | |
| --- | --- |
| `src/layout.rs` | bidi, segmentation, shaping, glyph placement — the core |
| `src/term.rs` | VT/ANSI interpretation |
| `src/grid.rs` | cells, rows, screens, scrollback |
| `src/render.rs` | software rasterisation and blending |
| `src/font.rs` | font loading, script fallback, glyph cache |
| `src/boxdraw.rs` | procedural box-drawing glyphs |
| `src/keys.rs` | key presses to terminal byte sequences |
| `src/selection.rs` | selection and copy |
| `src/sixel.rs` | sixel image decoding |
| `src/pty.rs` | PTY and the shell process |
| `src/app.rs` | window, input, event loop |

Built on [rustybuzz] (shaping), [swash] (rasterisation), [unicode-bidi] (UAX #9),
[vte] (ANSI parsing), [portable-pty], [winit] and [softbuffer]. No `unsafe`.

[rustybuzz]: https://github.com/harfbuzz/rustybuzz
[swash]: https://github.com/dfrg/swash
[unicode-bidi]: https://github.com/servo/unicode-bidi
[vte]: https://github.com/alacritty/vte
[portable-pty]: https://github.com/wez/wezterm
[winit]: https://github.com/rust-windowing/winit
[softbuffer]: https://github.com/rust-windowing/softbuffer
