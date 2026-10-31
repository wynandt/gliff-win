//! Why a connection failed, in words a person can act on.
//!
//! When ssh fails, the session used to end as "session ended" and the person
//! had to read a log. This module turns ssh's stderr and exit code into one
//! [`SshFailure`], and each failure into one [`Message`] with one primary
//! [`Button`]. It opens no window: the tab's status text carries the message
//! now, and the failure panel draws it.
//!
//! Everything but [`runner`] is pure and is tested on the Linux build host.
//! The client handles no secret here: it reads no key, writes no
//! `known_hosts`, runs no key tool, and passes no option that weakens
//! ssh (section 11.7).

pub mod diag;
pub mod runner;
pub mod version;

pub use diag::Button;
pub use diag::GUIDE_FILE;
pub use diag::Guide;
pub use diag::Message;
pub use diag::Phase;
pub use diag::SshFailure;
pub use diag::classify;
pub use diag::from_spawn_error;
pub use diag::log_line;
pub use diag::message;
pub use version::MIN_SERVER;
pub use version::ServerVersion;
pub use version::check_server;
