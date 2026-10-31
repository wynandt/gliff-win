//! When the window asks the server for a new output size.
//!
//! Upstream (gliff 089662fd `crates/gliff/src/main.rs:1859-1945`) polls the
//! widget every 100 ms and sends once the size has held for 200 ms, gated
//! against the server's view and the last request, never under 64 px. On
//! Windows the window tells us instead of being polled: a drag ends with
//! `WM_EXITSIZEMOVE` (send once, at once), other size changes (maximise,
//! snap, F11) settle for the same 200 ms, and `WM_DPICHANGED` sends after
//! the window takes the suggested rect. Sizes are client pixels `& !1` at
//! [`protocol_scale`](crate::layout::protocol_scale).

use std::time::Duration;
use std::time::Instant;

use crate::layout::protocol_scale;
use crate::session::ResizeReq;

/// Whether `WM_SIZE` arrived inside the modal size-move loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moving {
    /// Between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`: a drag.
    InSizeMove,
    Free,
}

/// What the settle timer should do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Settled {
    /// The size held for [`ResizeGate::SETTLE`]: send this.
    Send(ResizeReq),
    /// The size changed since the timer was armed: fire again after this.
    Wait(Duration),
    /// Nothing pending, or the request is gated (same as the view or the
    /// last request, or too small): stop the timer.
    Nothing,
}

/// The gate. Pure: the caller owns the clock and the timer.
#[derive(Debug, Clone)]
pub struct ResizeGate {
    /// The size seen outside a drag, and when it must have held until.
    pending: Option<(ResizeReq, Instant)>,
    /// The latest size `WM_SIZE` reported, so repeats do not re-arm.
    seen: Option<ResizeReq>,
    /// The last request sent (the Hello-time one to begin with).
    requested: ResizeReq,
    /// The server's view from the latest `StreamConfig`.
    view: Option<(u32, u32)>,
}

impl ResizeGate {
    /// Upstream's settle delay (`main.rs:1878`).
    pub const SETTLE: Duration = Duration::from_millis(200);
    /// Upstream skips sizes under this on either side (`main.rs:1928`).
    pub const MIN_SIDE: u32 = 64;

    /// A gate whose last request is `initial`, the size the session
    /// started with.
    pub fn new(initial: ResizeReq) -> Self {
        Self {
            pending: None,
            seen: None,
            requested: initial,
            view: None,
        }
    }

    /// `WM_SIZE`. Never sends: inside a drag it waits for
    /// `WM_EXITSIZEMOVE`; outside one a new size (re)arms the settle
    /// timer, and the returned delay is when to call
    /// [`ResizeGate::on_settle`].
    pub fn on_size(
        &mut self,
        client: (u32, u32),
        dpi: u32,
        moving: Moving,
        now: Instant,
    ) -> Option<Duration> {
        let req = request_for(client, dpi);
        let repeat = self.seen == Some(req);
        self.seen = Some(req);
        match moving {
            Moving::InSizeMove => {
                self.pending = None;
                None
            }
            Moving::Free if repeat => None,
            Moving::Free => {
                self.pending = Some((req, now + Self::SETTLE));
                Some(Self::SETTLE)
            }
        }
    }

    /// `WM_EXITSIZEMOVE`: the drag is over; send the final size at once.
    pub fn on_exit_size_move(&mut self, client: (u32, u32), dpi: u32) -> Option<ResizeReq> {
        self.now(client, dpi)
    }

    /// `WM_DPICHANGED`, after `SetWindowPos` to the suggested rect.
    pub fn on_dpi_changed(&mut self, client: (u32, u32), dpi: u32) -> Option<ResizeReq> {
        self.now(client, dpi)
    }

    /// The settle timer fired.
    pub fn on_settle(&mut self, now: Instant) -> Settled {
        let Some((req, due)) = self.pending else {
            return Settled::Nothing;
        };
        if now < due {
            return Settled::Wait(due - now);
        }
        self.pending = None;
        match self.gate(req) {
            Some(req) => Settled::Send(req),
            None => Settled::Nothing,
        }
    }

    /// `UiEvent::StreamConfig`: the server's view. A request matching it is
    /// not sent (upstream compares the view, not the stream, so a reduced
    /// stream is not a mismatch).
    pub fn on_view(&mut self, view: (u32, u32)) {
        self.view = Some(view);
    }

    /// Sends `client` at once, superseding any pending settle.
    fn now(&mut self, client: (u32, u32), dpi: u32) -> Option<ResizeReq> {
        let req = request_for(client, dpi);
        self.seen = Some(req);
        self.pending = None;
        self.gate(req)
    }

    /// Upstream's `request_resize` gate (`main.rs:1926-1945`).
    fn gate(&mut self, req: ResizeReq) -> Option<ResizeReq> {
        if req.width < Self::MIN_SIDE || req.height < Self::MIN_SIDE {
            return None;
        }
        let size = (req.width, req.height);
        let last = (self.requested.width, self.requested.height);
        if self.requested.scale == req.scale && (Some(size) == self.view || size == last) {
            return None;
        }
        self.requested = req;
        Some(req)
    }

    /// The last request sent.
    pub fn requested(&self) -> ResizeReq {
        self.requested
    }
}

/// Client pixels to a request: even sides, the protocol scale.
pub fn request_for(client: (u32, u32), dpi: u32) -> ResizeReq {
    ResizeReq {
        width: client.0 & !1,
        height: client.1 & !1,
        scale: protocol_scale(dpi),
    }
}

#[cfg(test)]
mod tests;
