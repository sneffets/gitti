//! Windows reopen where they were: position, size and maximized / fullscreen state are
//! saved per window kind and display setup (`gitti_core::store::WindowPlacement`).
//!
//! gpui reports and takes bounds relative to a display (macOS) or in global coordinates
//! with the display only picking the scale factor (Windows), so a placement is restored on
//! the display it was saved for, found by its UUID. Scripted runs neither read nor write
//! placements (their windows live off-screen).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gitti_core::store::{self, WindowPlacement, WindowState};
use gpui_kit::*;

/// Wait this long after the last move / resize before writing (a drag fires many events).
const SAVE_DELAY: Duration = Duration::from_millis(500);

/// Bounds (and the display to open on) for a new window of `kind`: the saved placement
/// for the current display setup, else `default_size` centred on the main display.
pub fn initial(kind: &str, default_size: Size<Pixels>, cx: &App) -> (WindowBounds, Option<DisplayId>) {
    let fallback = (
        WindowBounds::Windowed(Bounds::centered(None, default_size, cx)),
        None,
    );
    let Some(p) = store::load_window_placement(kind, &setup_key(cx)) else {
        return fallback;
    };
    let Some(display) = cx
        .displays()
        .into_iter()
        .find(|d| display_key(d.as_ref()) == p.display)
    else {
        return fallback;
    };
    let bounds = Bounds::new(point(px(p.x), px(p.y)), size(px(p.width), px(p.height)));
    // A window that would come up entirely off its display (resolution changed under the
    // same display) starts centred instead.
    if !bounds.intersects(&display.bounds()) || p.width < 100.0 || p.height < 100.0 {
        return fallback;
    }
    let bounds = match p.state {
        WindowState::Windowed => WindowBounds::Windowed(bounds),
        WindowState::Maximized => WindowBounds::Maximized(bounds),
        WindowState::Fullscreen => WindowBounds::Fullscreen(bounds),
    };
    (bounds, Some(display.id()))
}

/// Saves the placement of `window` whenever it was moved or resized (debounced).
pub fn track<V: 'static>(kind: &'static str, window: &mut Window, cx: &mut Context<V>) {
    let pending: Rc<RefCell<Option<Task<()>>>> = Rc::default();
    cx.observe_window_bounds(window, move |_, window, cx| {
        let Some(placement) = current(window, cx) else {
            return;
        };
        let setup = setup_key(cx);
        // Replacing the task cancels the previous, not yet written one.
        *pending.borrow_mut() = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            cx.background_executor()
                .spawn(async move { store::save_window_placement(kind, &setup, &placement) })
                .await;
        }));
    })
    .detach();
}

pub fn current(window: &Window, cx: &App) -> Option<WindowPlacement> {
    let display = display_key(window.display(cx)?.as_ref());
    let (bounds, state) = match window.window_bounds() {
        WindowBounds::Windowed(b) => (b, WindowState::Windowed),
        WindowBounds::Maximized(b) => (b, WindowState::Maximized),
        WindowBounds::Fullscreen(b) => (b, WindowState::Fullscreen),
    };
    let mut size = bounds.size;
    // macOS reports the frame (incl. the system title bar) but opens with that size as the
    // content size — store the content size, or the window grows with every restore.
    if cfg!(target_os = "macos") && state == WindowState::Windowed {
        size = window.viewport_size();
    }
    Some(WindowPlacement {
        display,
        x: bounds.origin.x.as_f32(),
        y: bounds.origin.y.as_f32(),
        width: size.width.as_f32(),
        height: size.height.as_f32(),
        state,
    })
}

/// A display's identity: its UUID, else (platforms without one) its bounds.
fn display_key(d: &dyn PlatformDisplay) -> String {
    match d.uuid() {
        Ok(uuid) => uuid.to_string(),
        Err(_) => {
            let b = d.bounds();
            format!(
                "{}x{}@{},{}",
                b.size.width.as_f32(),
                b.size.height.as_f32(),
                b.origin.x.as_f32(),
                b.origin.y.as_f32()
            )
        }
    }
}

/// Fingerprint of the connected displays (identity and size of each, sorted).
pub fn setup_key(cx: &App) -> String {
    let mut keys: Vec<String> = cx
        .displays()
        .iter()
        .map(|d| {
            let s = d.bounds().size;
            format!(
                "{} {}x{}",
                display_key(d.as_ref()),
                s.width.as_f32(),
                s.height.as_f32()
            )
        })
        .collect();
    keys.sort();
    keys.join("; ")
}
