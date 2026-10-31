//! The `--stats-out <path>` file: what the client did, as JSON, rewritten
//! about once a second while it runs and a last time at exit
//! (`complete: true`). `gwin-verify present` polls it to follow the client
//! and judges the last one.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::present_log::PresentSummary;
use crate::session::EndReason;
use crate::session::ResizeReq;
use crate::session::SessionStats;
use crate::session_set::Report;
use crate::settings::Settings;
use crate::tabs::Status;
use crate::tabs::Tabs;

/// The whole file.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClientStats {
    pub version: String,
    /// False while the client runs; true in the copy written at exit.
    pub complete: bool,
    pub session: SessionCounters,
    pub present: PresentSummary,
    pub adapter: AdapterInfo,
    pub monitor: MonitorInfo,
    pub window: WindowInfo,
    pub swap_chain: SwapChainInfo,
    pub resizes: Vec<ResizeRecord>,
    /// The last StreamConfig's fps cap; 0 before the first.
    pub fps_cap: u32,
    /// `EndReason` in snake case ("bye", "server_error", ...); empty while
    /// running.
    pub end: String,
    pub ssh_exit: Option<i32>,
    /// Keyboard and pointer state; absent in files from older builds.
    #[serde(default)]
    pub input: InputInfo,
    /// Every live session, shown or hidden; absent in files from before
    /// several sessions. `session` above is the shown one.
    #[serde(default)]
    pub sessions: Vec<SessionEntry>,
    /// The chords in force; absent in files from before several sessions.
    #[serde(default)]
    pub chords: ChordsInfo,
}

/// One live session's line: what it has done whether or not it is shown.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionEntry {
    /// The session's number in this run (the id the tab model gave it).
    pub id: u32,
    pub target: String,
    /// The session on screen (the one the keyboard goes to).
    pub shown: bool,
    /// The tab's state: "stopped", "connecting", "streaming" or "gave_up".
    pub status: String,
    /// Reconnect attempts so far (always 0 until reconnect exists).
    pub retries: u32,
    pub frames: u64,
    pub acks: u64,
    /// Keys the server has been sent a Down for and no Up: zero in a session
    /// that is not shown.
    pub keys_down: u32,
    /// Buttons held, likewise.
    pub buttons_down: u32,
}

/// The chords in force, as the settings spell them ("none" for one that is
/// off).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChordsInfo {
    pub release: String,
    pub super_l: String,
    pub switch_next: String,
    pub switch_previous: String,
    /// The modifiers of the jump chords.
    pub switch_jump: String,
    pub local: Vec<String>,
}

impl ChordsInfo {
    pub fn of(settings: &Settings) -> Self {
        Self {
            release: settings.release_chord.to_string(),
            super_l: settings.super_l_chord.to_string(),
            switch_next: settings.switch_next.to_string(),
            switch_previous: settings.switch_previous.to_string(),
            switch_jump: settings.switch_jump.to_string(),
            local: settings
                .local_chords
                .0
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
}

/// The per-session lines: one for each live session, in session order. A
/// session the model does not know (it ended and was forgotten between the
/// two reads) is left out.
pub fn session_entries(reports: &[Report], tabs: &Tabs) -> Vec<SessionEntry> {
    let shown = tabs.shown_session();
    reports
        .iter()
        .filter_map(|report| {
            let tab = tabs.tab_of(report.session)?;
            Some(SessionEntry {
                id: report.session.get(),
                target: report.target.to_string(),
                shown: shown == Some(report.session),
                status: status_name(tab.status()).to_owned(),
                retries: u32::from(tab.retries()),
                frames: report.stats.frames,
                acks: report.stats.acks,
                keys_down: report.stats.keys_down,
                buttons_down: report.stats.buttons_down,
            })
        })
        .collect()
}

/// A tab's [`Status`] as the stats file spells it.
pub fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Stopped => "stopped",
        Status::Connecting => "connecting",
        Status::Reconnecting { .. } => "reconnecting",
        Status::Streaming => "streaming",
        Status::GaveUp(_) => "gave_up",
    }
}

/// What the input side is doing, for `gwin-verify keys` and `pointer`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InputInfo {
    /// The keyboard hook: "off", "super_l_only" or "grab".
    pub hook: String,
    /// Keys are being taken for the remote machine (`hook == "grab"`).
    pub grab: bool,
    /// Times the hook was found silent and installed again.
    pub hook_reinstalls: u32,
    /// Keys sent from the window because the hook was silent.
    pub legacy_keys: u32,
    /// Grabs ended by `--grab-timeout-secs`.
    #[serde(default)]
    pub grab_timeouts: u32,
    /// Modifier Ups injected into Windows when a grab ended.
    #[serde(default)]
    pub modifiers_released: u32,
    /// The same, when the client exited.
    #[serde(default)]
    pub modifiers_released_on_exit: u32,
    /// The Windows lock setting is in force (always false in chord mode).
    pub lock_setting_held: bool,
    /// The picture last shown, for mapping a remote point to a screen one.
    pub geometry: Option<GeometryInfo>,
    /// One line per layout sent, oldest first (at most 16).
    pub keymap_sent: Vec<String>,
    /// Wheel steps sent to the server by axis and sign as Wayland counts
    /// ("vertical+" is down, "horizontal+" is right): what the client did, apart from what
    /// the remote desktop made of it.
    #[serde(default)]
    pub wheel_steps_sent: BTreeMap<String, u64>,
    /// Button presses sent to the server by Linux button code (272 left,
    /// 273 right, 274 middle, 275 and 276 or 277 and 278 for the thumbs).
    #[serde(default)]
    pub button_presses_sent: BTreeMap<u32, u64>,
}

/// [`crate::session::Geometry`] as numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GeometryInfo {
    pub stream: (u32, u32),
    pub view: (u32, u32),
    pub scale_milli: u32,
}

/// [`SessionStats`] plus the frame slot's drops, as numbers.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionCounters {
    pub frames: u64,
    pub acks: u64,
    pub decode_errors: u64,
    pub drain_errors: u64,
    pub keyframe_requests: u64,
    /// Pictures handed to the frame slot.
    pub to_slot: u64,
    pub slot_dropped: u64,
    pub configs: u64,
    pub resets: u64,
    pub resizes_sent: u64,
    pub decode_p50_ms: f32,
    pub decode_p95_ms: f32,
    pub decode_max_ms: f32,
}

impl SessionCounters {
    pub fn new(stats: &SessionStats, slot_dropped: u64) -> Self {
        Self {
            frames: stats.frames,
            acks: stats.acks,
            decode_errors: stats.decode_errors,
            drain_errors: stats.drain_errors,
            keyframe_requests: stats.keyframe_requests,
            to_slot: stats.presented,
            slot_dropped,
            configs: stats.configs,
            resets: stats.resets,
            resizes_sent: stats.resizes_sent,
            decode_p50_ms: stats.decode_ms.quantile(0.5),
            decode_p95_ms: stats.decode_ms.quantile(0.95),
            decode_max_ms: stats.decode_ms.max(),
        }
    }
}

/// The adapter the device was created on (`DXGI_ADAPTER_DESC1`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdapterInfo {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    /// `--adapter` as spelled there: "monitor", "high-performance" or
    /// "minimum-power".
    pub choice: String,
    /// Whether one of its outputs is the window's monitor.
    pub drives_monitor: bool,
}

/// The window's monitor.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MonitorInfo {
    /// `rcMonitor`: left, top, width, height in physical px.
    pub rect: (i32, i32, u32, u32),
    pub dpi: u32,
    pub refresh_hz: f64,
}

/// The window now.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WindowInfo {
    /// Client area in physical px.
    pub client: (u32, u32),
    /// Window rect on screen: left, top, width, height.
    pub rect: (i32, i32, u32, u32),
    pub fullscreen: bool,
    pub dpi: u32,
}

/// The swap chain as created, for the tear-free assertion.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SwapChainInfo {
    pub size: (u32, u32),
    /// "flip_discard".
    pub swap_effect: String,
    pub buffers: u32,
    /// `DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING` set.
    pub allow_tearing: bool,
    /// The `SyncInterval` every `Present` uses.
    pub sync_interval: u32,
    pub max_frame_latency: u32,
}

/// One Resize the window sent, with times since the client started.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResizeRecord {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub sent_ms: u64,
    /// The first StreamConfig whose view is this size.
    pub applied_ms: Option<u64>,
    /// The first picture shown at this view.
    pub first_picture_ms: Option<u64>,
}

impl ResizeRecord {
    pub fn sent(req: ResizeReq, sent_ms: u64) -> Self {
        Self {
            width: req.width,
            height: req.height,
            scale: req.scale,
            sent_ms,
            applied_ms: None,
            first_picture_ms: None,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// The Resizes sent, matched to the StreamConfig that applied each and
/// the first picture shown at its view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResizeLog {
    records: Vec<ResizeRecord>,
}

impl ResizeLog {
    pub fn sent(&mut self, req: ResizeReq, now_ms: u64) {
        self.records.push(ResizeRecord::sent(req, now_ms));
    }

    /// A StreamConfig: the oldest unapplied Resize for this view and
    /// scale is applied now.
    pub fn on_config(&mut self, view: (u32, u32), scale_milli: u32, now_ms: u64) {
        let pending = self.records.iter_mut().find(|r| {
            r.applied_ms.is_none() && r.size() == view && scale_milli_of(r.scale) == scale_milli
        });
        if let Some(record) = pending {
            record.applied_ms = Some(now_ms);
        }
    }

    /// A picture shown at `view`: the first for each applied record.
    pub fn on_picture(&mut self, view: (u32, u32), now_ms: u64) {
        for record in &mut self.records {
            if record.applied_ms.is_some()
                && record.first_picture_ms.is_none()
                && record.size() == view
            {
                record.first_picture_ms = Some(now_ms);
            }
        }
    }

    pub fn records(&self) -> &[ResizeRecord] {
        &self.records
    }
}

/// [`EndReason`] as the stats file spells it.
pub fn end_name(reason: &EndReason) -> &'static str {
    match reason {
        EndReason::Bye => "bye",
        EndReason::Incompatible(_) => "incompatible",
        EndReason::ServerError { .. } => "server_error",
        EndReason::Link(_) => "link",
        EndReason::Ssh(_) => "ssh",
    }
}

/// The protocol's `scale_milli` for a Resize scale.
fn scale_milli_of(scale: f32) -> u32 {
    (scale * 1000.0).round() as u32
}

impl ClientStats {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("ClientStats has only plain fields")
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Writes `path` through a temporary sibling and a rename, so a reader
    /// never sees half a file.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests;
