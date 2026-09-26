//! arabic-term: a terminal emulator that renders Arabic correctly.
//!
//! The grid stores text in logical order exactly as the client program writes
//! it; bidi reordering and Arabic shaping happen at render time, so column
//! arithmetic in full-screen programs keeps working while Arabic still joins,
//! ligates and carries its diacritics.

pub mod app;
pub mod boxdraw;
pub mod color;
pub mod config;
pub mod commands;
pub mod cli;
mod clipboard;
pub mod input;
pub mod font;
mod frame;
pub mod gpu;
pub mod hash;
pub mod frame_hold;
pub mod grid;
pub mod keys;
pub mod kitty_graphics;
pub mod layout;
pub mod mouse;
pub mod notify;
pub mod picker;
pub mod pty;
pub mod render;
pub mod sixel;
pub mod scroll;
pub mod search;
pub mod selection;
pub mod shortcuts;
pub mod term;
