//! NativeTerm's remote control: a pane held at a device's size (a phone's
//! columns and rows) while the device looks at it, so that the program in
//! it lays itself out for the phone. The pane is drawn at that size at the
//! top left of its area; the window's own size is what it goes back to.
//!
//! `LocalPane::resize` is the one place every resize of a pane ends up
//! (window, font, split, zoom, tab activation): it asks `apply` what to
//! use, and tells `requested` what it was asked for, so that a hold and
//! its release can re-apply the size the window wants.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use wezterm_term::TerminalSize;

use crate::pane::PaneId;

#[derive(Default)]
struct Sizes {
    /// The sizes panes are held at: (cols, rows).
    held: HashMap<PaneId, (usize, usize)>,
    /// What each pane was last asked to be by its window.
    requested: HashMap<PaneId, TerminalSize>,
}

static SIZES: LazyLock<Mutex<Sizes>> = LazyLock::new(Default::default);

/// Hold the pane at `cols` x `rows` (never larger than its window gives).
pub fn hold(pane: PaneId, cols: usize, rows: usize) {
    SIZES
        .lock()
        .unwrap()
        .held
        .insert(pane, (cols.max(2), rows.max(1)));
}

/// The pane follows its window again.
pub fn release(pane: PaneId) {
    SIZES.lock().unwrap().held.remove(&pane);
}

pub fn is_held(pane: PaneId) -> bool {
    SIZES.lock().unwrap().held.contains_key(&pane)
}

/// What the window last asked the pane to be, if it asked.
pub fn requested(pane: PaneId) -> Option<TerminalSize> {
    SIZES.lock().unwrap().requested.get(&pane).copied()
}

/// The size to give the pane its window asked to be `size`: that, or the
/// held size within it (the cell size kept from the request, so pixels
/// stay right after a font or DPI change).
pub fn apply(pane: PaneId, size: TerminalSize) -> TerminalSize {
    let mut sizes = SIZES.lock().unwrap();
    sizes.requested.insert(pane, size);
    let Some(&(cols, rows)) = sizes.held.get(&pane) else {
        return size;
    };
    let cols = cols.min(size.cols.max(1));
    let rows = rows.min(size.rows.max(1));
    let cell_w = size.pixel_width / size.cols.max(1);
    let cell_h = size.pixel_height / size.rows.max(1);
    TerminalSize {
        rows,
        cols,
        pixel_width: cell_w * cols,
        pixel_height: cell_h * rows,
        dpi: size.dpi,
    }
}

/// A pane gone: nothing kept for it.
pub fn forget(pane: PaneId) {
    let mut sizes = SIZES.lock().unwrap();
    sizes.held.remove(&pane);
    sizes.requested.remove(&pane);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_within_the_window_and_released() {
        let window = TerminalSize {
            rows: 40,
            cols: 120,
            pixel_width: 1200,
            pixel_height: 800,
            dpi: 96,
        };
        assert_eq!(apply(9001, window), window, "not held: as asked");
        hold(9001, 45, 30);
        let held = apply(9001, window);
        assert_eq!((held.cols, held.rows), (45, 30));
        assert_eq!(
            (held.pixel_width, held.pixel_height, held.dpi),
            (450, 600, 96)
        );
        // a window smaller than the hold: the window's
        let small = TerminalSize {
            rows: 20,
            cols: 30,
            pixel_width: 300,
            pixel_height: 400,
            dpi: 96,
        };
        assert_eq!((apply(9001, small).cols, apply(9001, small).rows), (30, 20));
        assert_eq!(requested(9001), Some(small));
        release(9001);
        assert_eq!(apply(9001, window), window);
        forget(9001);
        assert_eq!(requested(9001), None);
    }
}
