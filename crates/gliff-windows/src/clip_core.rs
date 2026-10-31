//! The clipboard rules that need no Windows and no network: what a local
//! change turns into, what a remote offer turns into, and how the client
//! tells its own clipboard writes from another program's.
//!
//! The Win32 thread (`clip_win`) gathers the facts and acts on the answers;
//! the transfer bookkeeping (serials, revocation) is in `clip_engine`.

use gliff_proto::ClipboardFile;

use crate::clip_formats;
use crate::clip_formats::RemotePlan;
use crate::clip_formats::WinFormat;
use crate::clip_log::Own;

/// What a change of the Windows clipboard becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalChange {
    /// Nothing to forward: an empty clipboard, formats with no mapping, or a
    /// copy that asked to stay private. The previous offer is withdrawn.
    Withdraw,
    /// Offer these flavours, and the files behind `CF_HDROP` when `files`.
    Offer { mimes: Vec<String>, files: bool },
}

/// The offer for a clipboard holding `formats`. `private` is
/// [`clip_formats::is_private`] for them.
pub fn local_change(formats: &[WinFormat], private: bool) -> LocalChange {
    if private {
        return LocalChange::Withdraw;
    }
    let mimes = clip_formats::offer_mimes(formats);
    let files = formats.contains(&WinFormat::Hdrop);
    if mimes.is_empty() && !files {
        LocalChange::Withdraw
    } else {
        LocalChange::Offer { mimes, files }
    }
}

/// What to put on the Windows clipboard for an offer from the remote side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteWrite {
    /// Formats to promise, each rendered on demand.
    pub plans: Vec<RemotePlan>,
    /// Promise `CF_HDROP` and `Preferred DropEffect` for the files.
    pub files: bool,
    /// Keep the copy out of history, sync and monitors: the privacy formats.
    pub private: bool,
}

/// The write for a remote offer, or `None` for an offer with nothing
/// Windows can use (the clipboard is cleared, if it is still ours).
/// `allow_history` is `--clipboard-history`. A secret (the remote offer
/// carries the password manager hint) is private either way.
pub fn remote_write(
    mimes: &[String],
    files: &[ClipboardFile],
    allow_history: bool,
) -> Option<RemoteWrite> {
    let plans = clip_formats::plan_remote(mimes);
    let files = !files.is_empty();
    if plans.is_empty() && !files {
        return None;
    }
    Some(RemoteWrite {
        plans,
        files,
        private: !allow_history || clip_formats::remote_is_sensitive(mimes),
    })
}

/// Tells the client's own clipboard writes from other programs'.
///
/// An update is ours when its sequence number is the one recorded after our
/// last write, when our window owns the clipboard, or when it carries our
/// marker format. Anything else is the user's, or another program's.
#[derive(Debug, Default)]
pub struct EchoGuard {
    own_sequence: Option<u32>,
}

impl EchoGuard {
    /// Record the sequence number the clipboard has after one of our writes.
    pub fn wrote(&mut self, sequence: u32) {
        self.own_sequence = Some(sequence);
    }

    /// Whether, and by which sign, a change is our own write. Owner and
    /// marker are checked first: they hold even when the sequence does not.
    pub fn verdict(&self, sequence: u32, owner_is_us: bool, has_marker: bool) -> Own {
        if owner_is_us {
            Own::Owner
        } else if has_marker {
            Own::Marker
        } else if self.own_sequence == Some(sequence) {
            Own::Sequence
        } else {
            Own::No
        }
    }

    pub fn is_echo(&self, sequence: u32, owner_is_us: bool, has_marker: bool) -> bool {
        self.verdict(sequence, owner_is_us, has_marker) != Own::No
    }
}

/// Collapses a burst of clipboard updates into one reaction. Programs write
/// a clipboard in several steps; the client reads it after a quiet period.
#[derive(Debug, Default)]
pub struct Debounce {
    pending: bool,
}

impl Debounce {
    /// An update arrived. Returns true when this one starts the quiet period
    /// (the caller arms its timer); later ones inside it only extend it.
    pub fn update(&mut self) -> bool {
        !std::mem::replace(&mut self.pending, true)
    }

    /// The timer fired: the burst is over. Returns whether to react.
    pub fn settle(&mut self) -> bool {
        std::mem::replace(&mut self.pending, false)
    }
}

#[cfg(test)]
mod tests;
