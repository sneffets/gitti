//! Layout probes: named elements record their bounds and clip rectangle while a frame is
//! laid out, so tests (`tests/ui`) and the `layout` / `check` script steps can check
//! geometry without screenshots. Off unless [`enable`]d: then `.probe(name)` adds nothing.
//!
//! Names are `kind` or `kind:index[:detail]`, e.g. `commit-box`, `diff-row:12:line`,
//! `msg-line:0`. Rows of virtualised lists are only recorded while they are rendered.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::*;

static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The frame being drawn.
    static PROBES: RefCell<Vec<Probe>> = const { RefCell::new(Vec::new()) };
    /// Finished frames since the last [`clear`] (bounded), for checking every frame.
    static DONE: RefCell<Vec<Vec<Probe>>> = const { RefCell::new(Vec::new()) };
}

/// Most finished frames kept by [`begin_frame`].
const MAX_DONE: usize = 256;

/// One element of the last frame: its layout bounds and the content mask it was drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub name: String,
    pub bounds: Bounds<Pixels>,
    pub clip: Bounds<Pixels>,
}

impl Probe {
    /// The part of the element that can show anything (bounds ∩ clip); empty if hidden.
    pub fn visible(&self) -> Option<Bounds<Pixels>> {
        let v = self.bounds.intersect(&self.clip);
        (v.size.width > px(0.) && v.size.height > px(0.)).then_some(v)
    }

    /// The name's first part (`diff-row` of `diff-row:12:line`).
    pub fn kind(&self) -> &str {
        self.name.split(':').next().unwrap_or_default()
    }

    /// The name's second part as a number (`12` of `diff-row:12:line`).
    pub fn index(&self) -> Option<usize> {
        self.name.split(':').nth(1)?.parse().ok()
    }

    /// The name's third part (`line` of `diff-row:12:line`).
    pub fn detail(&self) -> &str {
        self.name.split(':').nth(2).unwrap_or_default()
    }

    /// `{"name":…,"x":…,"y":…,"w":…,"h":…,"vis":[x,y,w,h]|null}` in logical px.
    pub fn to_json(&self) -> String {
        let r = |b: &Bounds<Pixels>| {
            [
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            ]
        };
        let [x, y, w, h] = r(&self.bounds);
        serde_json::json!({
            "name": self.name,
            "x": x, "y": y, "w": w, "h": h,
            "vis": self.visible().map(|v| r(&v)),
        })
        .to_string()
    }
}

/// Turns recording on (tests, scripted runs); the next frames record their probes.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Forgets the recorded probes and frames; call before drawing the frame to look at.
pub fn clear() {
    PROBES.with(|p| p.borrow_mut().clear());
    DONE.with(|d| d.borrow_mut().clear());
}

/// A new frame starts (the main window's root renders): the probes recorded so far become a
/// finished frame. gpui redraws within one update (test mode draws on every effect flush),
/// so a test that only looked at the last frame would miss a wrong one in between.
pub fn begin_frame() {
    if !enabled() {
        return;
    }
    let prev = PROBES.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if !prev.is_empty() {
        DONE.with(|d| {
            let mut d = d.borrow_mut();
            if d.len() >= MAX_DONE {
                d.remove(0);
            }
            d.push(prev);
        });
    }
}

/// The probes of the frame drawn last (or being drawn).
pub fn snapshot() -> Vec<Probe> {
    PROBES.with(|p| p.borrow().clone())
}

/// Every frame drawn since the last call (or [`clear`]), oldest first, the current one last.
pub fn take_frames() -> Vec<Vec<Probe>> {
    let mut frames = DONE.with(|d| std::mem::take(&mut *d.borrow_mut()));
    let current = snapshot();
    if !current.is_empty() {
        frames.push(current);
    }
    frames
}

pub(crate) fn record(name: String, bounds: Bounds<Pixels>, clip: Bounds<Pixels>) {
    PROBES.with(|p| p.borrow_mut().push(Probe { name, bounds, clip }));
}

/// `.probe(name)`: records the element's bounds (an absolutely placed, invisible child
/// filling it) when probes are enabled.
pub trait ProbeExt: ParentElement + Sized {
    fn probe(self, name: impl Into<String>) -> Self {
        if !enabled() {
            return self;
        }
        let name = name.into();
        self.child(
            canvas(
                move |bounds, window, _| record(name, bounds, window.content_mask().bounds),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
    }
}

impl<E: ParentElement + Sized> ProbeExt for E {}
