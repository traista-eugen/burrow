//! Timing the transcript, logged at `debug` under the `perf` target
//! (`BURROW_LOG=info,perf=debug`):
//! - `transcript frame`: how many rows a frame drew, and how long the list took to lay them out,
//!   prepaint and paint, which is all the transcript costs a frame;
//! - `transcript applied` and `transcript rebuilt`: what applying events cost, in the model and
//!   in all.
//!
//! The numbers in D-033 were measured this way.

use std::cell::Cell;
use std::panic;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};
use tracing::debug;

/// The `tracing` target of the timings.
pub const TARGET: &str = "perf";

/// What one frame of the transcript cost.
#[derive(Default)]
pub struct FrameStats {
    rows: Cell<u32>,
    time: Cell<Duration>,
}

impl FrameStats {
    /// Counts a row drawn.
    pub fn row(&self) {
        self.rows.set(self.rows.get() + 1);
    }

    fn add(&self, started: Instant) {
        self.time.set(self.time.get() + started.elapsed());
    }

    fn report(&self) {
        debug!(
            target: TARGET,
            rows = self.rows.get(),
            us = self.time.get().as_micros() as u64,
            "transcript frame"
        );
        self.rows.set(0);
        self.time.set(Duration::ZERO);
    }
}

/// Wraps an element and times its layout, prepaint and paint into [`FrameStats`].
pub struct Timed {
    child: AnyElement,
    stats: Rc<FrameStats>,
}

impl Timed {
    pub fn new(child: AnyElement, stats: Rc<FrameStats>) -> Self {
        Self { child, stats }
    }
}

impl IntoElement for Timed {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Timed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let started = Instant::now();
        let layout = self.child.request_layout(window, cx);
        self.stats.add(started);
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        self.child.prepaint(window, cx);
        self.stats.add(started);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        self.child.paint(window, cx);
        self.stats.add(started);
        self.stats.report();
    }
}
