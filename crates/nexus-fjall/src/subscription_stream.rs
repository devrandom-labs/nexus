use std::sync::Arc;

use crate::encoding::{decode_event_key, decode_event_value};
use crate::error::FjallError;
use crate::store::FjallStore;
use crate::stream::FjallStream;
use arrayvec::ArrayString;
use bytes::Bytes;
use nexus::{Id, Version};
use nexus_store::GlobalSeq;
use nexus_store::PersistedEnvelope;
use nexus_store::store::RawEventStore;
use nexus_store::stream::EventStream;

/// Owned byte-key wrapper to satisfy the [`Id`] trait's `'static` bound
/// when re-reading from the store during subscription refills.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct OwnedStreamId(Vec<u8>);

impl OwnedStreamId {
    /// Create from any [`Id`] by capturing its byte representation.
    pub(crate) fn from_id(id: &impl Id) -> Self {
        Self(id.as_ref().to_vec())
    }
}

impl std::fmt::Display for OwnedStreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match std::str::from_utf8(&self.0) {
            Ok(s) => f.write_str(s),
            Err(_) => write!(f, "<{} bytes>", self.0.len()),
        }
    }
}

impl AsRef<[u8]> for OwnedStreamId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Id for OwnedStreamId {
    const BYTE_LEN: usize = 0;
}

// ═══════════════════════════════════════════════════════════════════════════
// FjallSubscriptionStream — Arc-owning subscription cursor
// ═══════════════════════════════════════════════════════════════════════════

/// Subscription cursor that owns an `Arc<FjallStore>`.
///
/// `'static` (no lifetime parameter). Spawnable across async boundaries.
/// Holds an eagerly-loaded batch of event rows (via [`FjallStream`]).
/// When the inner batch is exhausted, it waits on [`FjallStore`]'s
/// `Notify` and re-reads from the last yielded version. The stream
/// **never returns `None`** — it blocks until new events arrive.
///
/// The per-record [`EventStream::Item<'a>`](nexus_store::stream::EventStream::Item)
/// GAT is `PersistedEnvelope` — `next()` yields each envelope from
/// the inner [`FjallStream`]'s row buffer.
pub struct FjallSubscriptionStream {
    store: Arc<FjallStore>,
    /// Owned byte key for re-reading from the store on refill.
    stream_key: OwnedStreamId,
    /// Human-readable label for error messages.
    label: ArrayString<64>,
    /// Inner batch of eagerly-loaded events from the current read.
    inner: FjallStream,
    /// Last version yielded (tracks position for re-reads).
    last_version: Option<Version>,
}

impl FjallSubscriptionStream {
    /// Create a new subscription stream.
    ///
    /// `inner` should already contain the initial catch-up batch.
    pub(crate) const fn new(
        store: Arc<FjallStore>,
        stream_key: OwnedStreamId,
        label: ArrayString<64>,
        inner: FjallStream,
        last_version: Option<Version>,
    ) -> Self {
        Self {
            store,
            stream_key,
            label,
            inner,
            last_version,
        }
    }

    /// Compute the version to read from next: `last_version` + 1, or `INITIAL`.
    ///
    /// Returns an error on overflow instead of silently wrapping back
    /// to `Version::INITIAL`.
    fn next_read_version(&self) -> Result<Version, FjallError> {
        self.last_version.map_or(Ok(Version::INITIAL), |v| {
            v.next().ok_or(FjallError::VersionOverflow)
        })
    }

    /// Replace the inner stream with a fresh read starting at `from`.
    async fn refill(&mut self, from: Version) -> Result<(), FjallError> {
        let fresh = self.store.read_stream(&self.stream_key, from).await?;
        self.inner = fresh;
        Ok(())
    }
}

impl EventStream for FjallSubscriptionStream {
    type Item<'a>
        = PersistedEnvelope
    where
        Self: 'a;
    type Error = FjallError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope>, Self::Error> {
        loop {
            // Yield from the current inner batch if available.
            if self.inner.poisoned || self.inner.pos >= self.inner.events.len() {
                // Either poisoned or exhausted — fall through to refill.
            } else {
                let (key, value) = &self.inner.events[self.inner.pos];
                self.inner.pos += 1;

                let Ok((_id_bytes, version_raw)) = decode_event_key(key) else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: None,
                    });
                };

                #[cfg(debug_assertions)]
                {
                    if let Some(prev) = self.inner.prev_version {
                        debug_assert!(
                            version_raw > prev,
                            "Subscription monotonicity violated: version {version_raw} \
                             is not greater than previous {prev}",
                        );
                    }
                    self.inner.prev_version = Some(version_raw);
                }

                let bytes_value: Bytes = value.clone().into();
                let Ok(decoded) = decode_event_value(&bytes_value) else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    });
                };

                let Some(version) = Version::new(version_raw) else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    });
                };

                let Some(global_seq) = GlobalSeq::new(decoded.global_seq) else {
                    self.inner.poisoned = true;
                    return Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    });
                };

                let envelope = match PersistedEnvelope::try_new(
                    version,
                    global_seq,
                    bytes_value,
                    decoded.schema_version,
                    decoded.event_type_range,
                    decoded.payload_range,
                    decoded.metadata_range,
                ) {
                    Ok(env) => env,
                    Err(source) => {
                        self.inner.poisoned = true;
                        return Err(FjallError::EnvelopeCorrupt {
                            stream_id: self.label,
                            version: version.as_u64(),
                            source,
                        });
                    }
                };

                self.last_version = Some(version);
                return Ok(Some(envelope));
            }

            // Buffer exhausted (or recovering from poison). Register for
            // notification BEFORE reading to avoid the race where an
            // append happens between our read and our wait.
            //
            // Clone the `Arc<FjallStore>` so `notified` does not borrow
            // `self`. `store: Arc<FjallStore>` lives inside `self`, so a
            // `&mut self` call to `refill` would otherwise conflict with
            // the borrow held by `notified`.
            let store = Arc::clone(&self.store);
            let notified = store.notify.notified();

            let from = self.next_read_version()?;
            self.refill(from).await?;

            // If refill found new events, loop back to yield them.
            if !self.inner.events.is_empty() {
                continue;
            }

            // No new events — wait for notification, then retry.
            notified.await;
        }
    }
}
