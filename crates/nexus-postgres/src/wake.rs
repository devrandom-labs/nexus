//! `WakeSource` for [`PostgresStore`] via `LISTEN/NOTIFY` → [`StreamNotifiers`].
//!
//! The generic subscription loop in `nexus_store::subscription` parks on a
//! [`WakeRegistration`] until new events may exist. Postgres drives that loop by
//! reusing the audited in-process [`StreamNotifiers`] registry as the wake
//! machinery: a single background `PgListener` task (spawned in
//! [`crate::store`]) turns each `LISTEN/NOTIFY` message into a
//! `notifiers.wake(id)`, and the trait methods here delegate straight to that
//! registry. So `LISTEN/NOTIFY` is a thin transport whose only job is to *hint*
//! "scan sooner"; correctness rests on the registry's arm-before-confirm-rescan
//! discipline, not on `NOTIFY` delivery (see the listener task's correctness
//! note). `Registration` and `Error` are therefore the registry's own
//! [`WakeReg`] / [`NotifyError`] — no postgres-specific wake types are needed.

use nexus_store::notify::{NotifyError, WakeReg};
use nexus_store::wake::WakeSource;

use crate::store::PostgresStore;

/// Delegates wake-routing to the store's [`StreamNotifiers`], which the shared
/// `PgListener` task drives from `LISTEN/NOTIFY`. Mirrors the trait mechanics of
/// `nexus-fjall`'s `impl WakeSource` (delegate to an `Arc<StreamNotifiers>`),
/// the difference being the wake *source*: fjall's `append` calls `wake`
/// in-process, whereas postgres routes it through `NOTIFY` so a writer in
/// *another* process (or connection) still rouses this process's subscribers.
impl WakeSource for PostgresStore {
    type Registration = WakeReg;
    type Error = NotifyError;

    fn register(&self, stream: Option<&[u8]>) -> Result<Self::Registration, Self::Error> {
        self.wake_registry().register(stream)
    }

    fn wake(&self, stream: &[u8]) {
        // `StreamNotifiers::wake` bumps BOTH the per-stream and the `$all` wake
        // paths, so one call rouses per-stream and `$all` registrations alike.
        self.wake_registry().wake(stream);
    }
}
