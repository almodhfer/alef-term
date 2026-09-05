//! arabic-term: a terminal emulator that renders Arabic correctly.
//!
//! The grid stores text in logical order exactly as the client program writes
//! it; bidi reordering and Arabic shaping happen at render time, so column
//! arithmetic in full-screen programs keeps working while Arabic still joins,
//! ligates and carries its diacritics.

pub mod app;
pub mod boxdraw;
pub mod color;
pub mod font;
pub mod grid;
pub mod keys;
pub mod kitty_graphics;
pub mod layout;
pub mod pty;
pub mod render;
pub mod sixel;
pub mod scroll;
pub mod selection;
pub mod term;
