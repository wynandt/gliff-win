//! When the window asks the server for a new output size.
//!
//! Upstream (`install_resize_handler` in the gliff client) polls the widget every 100 ms and sends once the size has held for 200 ms, gated
//! against the server's view and the last request, never under 64 px. This
//! gate instead deduplicates against the latest request only: a server view
//! may predate an in-flight resize. On
//! Windows the window tells us instead of being polled: a drag ends with
//! `WM_EXITSIZEMOVE` (send once, at once), other size changes (maximise,
//! snap, F11) settle for the same 200 ms, and `WM_DPICHANGED` sends after
//! the window takes the suggested rect. A Scale pick on a shown Own screen
//! session sends at once the same way; a hidden one stores the scale for its
//! next resize. Sizes are client pixels `& !1` at the
//! session's [`Scale`](crate::machine_choices::Scale)
//! ([`protocol_scale`](crate::layout::protocol_scale) for [`Scale::Auto`]).

use std::time::Duration;
use std::time::Instant;

use crate::cli::Screen;
use crate::machine_choices::Running;
use crate::machine_choices::Scale;
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
    /// Nothing pending, or the request matches the last request or is too
    /// small: stop the timer.
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
    /// The scale choice active for this session until a Scale pick, a screen
    /// move, or a reconnect.
    scale: Scale,
}

impl ResizeGate {
    /// Upstream's settle delay.
    pub const SETTLE: Duration = Duration::from_millis(200);
    /// Upstream skips sizes under this on either side.
    pub const MIN_SIDE: u32 = 64;

    /// A gate whose last request is `initial`, the size the session
    /// started with, and whose active scale choice is `scale`.
    pub fn new(initial: ResizeReq, scale: Scale) -> Self {
        Self {
            pending: None,
            seen: None,
            requested: initial,
            scale,
        }
    }

    /// The scale choice active for this session.
    pub fn scale(&self) -> Scale {
        self.scale
    }

    /// Stores `scale` for a hidden session. The next resize uses it. Nothing
    /// is sent now.
    pub fn set_scale(&mut self, scale: Scale) {
        self.scale = scale;
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
        let req = request_for_scale(client, dpi, self.scale);
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

    /// Adopts `scale` and sends the client size at once. A move to another
    /// screen and a Scale pick on the shown session both use this.
    pub fn on_dpi_changed(
        &mut self,
        client: (u32, u32),
        dpi: u32,
        scale: Scale,
    ) -> Option<ResizeReq> {
        self.scale = scale;
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

    /// Sends `client` at once, superseding any pending settle.
    fn now(&mut self, client: (u32, u32), dpi: u32) -> Option<ResizeReq> {
        let req = request_for_scale(client, dpi, self.scale);
        self.seen = Some(req);
        self.pending = None;
        self.gate(req)
    }

    /// Latest requested size wins, even if an older server view matches it.
    fn gate(&mut self, req: ResizeReq) -> Option<ResizeReq> {
        if req.width < Self::MIN_SIDE || req.height < Self::MIN_SIDE {
            return None;
        }
        if self.requested == req {
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

/// Client pixels to a request: even physical sides at `scale`.
pub fn request_for_scale(client: (u32, u32), dpi: u32, scale: Scale) -> ResizeReq {
    ResizeReq {
        width: client.0 & !1,
        height: client.1 & !1,
        scale: scale.factor(dpi),
    }
}

/// Client pixels to a request: even sides, the protocol scale (`Scale::Auto`).
pub fn request_for(client: (u32, u32), dpi: u32) -> ResizeReq {
    request_for_scale(client, dpi, Scale::Auto)
}

/// Applies a Scale menu pick to one live session.
///
/// `is_tab_session` is false for a session that is not the tab's. Nothing
/// changes in that case. An Own screen session whose scale differs adopts
/// `scale`. When `shown`, the result is the window size at that scale, sent
/// at once, the same request a move to another screen sends. A hidden
/// session returns nothing; the gate keeps `scale` for its next resize. A
/// Mirror session and a scale the session already runs return nothing and
/// change nothing.
///
/// Upstream: `Session::resize` calls `set_monitor_mode` on the existing
/// headless output at 58c7424, so the output is not recreated. The output
/// is removed only when the session ends (`remove_output`).
pub fn apply_live_scale(
    is_tab_session: bool,
    running: &mut Running,
    gate: &mut ResizeGate,
    shown: bool,
    client: (u32, u32),
    dpi: u32,
    scale: Scale,
) -> Option<ResizeReq> {
    if !is_tab_session || running.screen != Screen::Own || running.scale == scale {
        return None;
    }
    running.scale = scale;
    if shown {
        gate.on_dpi_changed(client, dpi, scale)
    } else {
        gate.set_scale(scale);
        None
    }
}

#[cfg(test)]
mod tests;
