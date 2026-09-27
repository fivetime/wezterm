#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use self::windows::*;

/// A window's own edge (shadow, border, round top corners), X11 and Wayland alike
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) mod edge;
#[cfg(feature = "wayland")]
pub mod wayland;
pub mod x11;
pub mod x_and_wayland;
pub mod xdg_desktop_portal;
pub mod xkeysyms;

#[cfg(all(unix, not(target_os = "macos")))]
pub use x_and_wayland::*;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

pub mod parameters;

/// Where a popup whose body is `size` goes for the point `at` of the
/// `screen` (x, y, width, height): its top left at the point, or on the
/// point's other side where the screen ends, and within the screen.
#[cfg(not(target_os = "macos"))]
pub(crate) fn place_popup(
    at: (i32, i32),
    size: (i32, i32),
    screen: (i32, i32, i32, i32),
) -> (i32, i32) {
    let one = |at: i32, size: i32, start: i32, length: i32| {
        let end = start + length;
        let pos = if at + size > end { at - size } else { at };
        pos.min(end - size).max(start)
    };
    (
        one(at.0, size.0, screen.0, screen.2),
        one(at.1, size.1, screen.1, screen.3),
    )
}

#[cfg(all(test, not(target_os = "macos")))]
mod test {
    use super::place_popup;

    #[test]
    fn popup_placement() {
        let screen = (0, 0, 1920, 1080);
        // at the point
        assert_eq!(place_popup((100, 50), (300, 400), screen), (100, 50));
        // to the point's other side where the screen ends
        assert_eq!(place_popup((1800, 50), (300, 400), screen), (1500, 50));
        assert_eq!(place_popup((100, 900), (300, 400), screen), (100, 500));
        // within the screen, where neither side has the room
        assert_eq!(place_popup((100, 300), (300, 900), screen), (100, 0));
        // a second monitor, to the left of the first
        let left = (-1920, 0, 1920, 1080);
        assert_eq!(place_popup((-100, 50), (300, 400), left), (-400, 50));
    }
}
