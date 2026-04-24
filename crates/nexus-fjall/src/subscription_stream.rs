use crate::encoding::{decode_event_key, decode_event_value};
use crate::error::FjallError;
use crate::store::FjallStore;
use crate::stream::FjallStream;
use arrayvec::ArrayString;
use nexus::{Id, Version};
use nexus_store::PersistedEnvelope;
use nexus_store::store::{EventStream, RawEventStore};

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

/// Subscription stream backed by fjall.
///
/// Holds an eagerly-loaded batch of event rows (via [`FjallStream`]).
/// When the inner batch is exhausted, it waits on [`FjallStore`]'s
/// `Notify` and re-reads from the last yielded version. The stream
/// **never returns `None`** — it blocks until new events arrive.
pub struct FjallSubscriptionStream<'a> {
    store: &'a FjallStore,
    /// Owned byte key for re-reading from the store on refill.
    stream_key: OwnedStreamId,
    /// Human-readable label for error messages.
    label: ArrayString<64>,
    /// The inner batch of eagerly-loaded events from the current read.
    inner: FjallStream,
    /// The last version yielded (tracks position for re-reads).
    last_version: Option<Version>,
}

impl<'a> FjallSubscriptionStream<'a> {
    /// Create a new subscription stream.
    ///
    /// `inner` should already contain the initial catch-up batch.
    pub(crate) const fn new(
        store: &'a FjallStore,
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

    /// Compute the version to start reading from next.
    ///
    /// Returns `last_version + 1`, or `Version::INITIAL` if no events
    /// have been yielded yet. Returns an error on overflow instead of
    /// silently wrapping back to `Version::INITIAL`.
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

impl EventStream for FjallSubscriptionStream<'_> {
    type Error = FjallError;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        loop {
            // Yield from the current inner batch if available.
            if self.inner.poisoned || self.inner.pos >= self.inner.events.len() {
                // Either poisoned or exhausted — fall through to refill.
                // If poisoned in a previous iteration, we try to refill
                // with a fresh stream (the corruption may have been in one
                // batch only).
            } else {
                let (key, value) = &self.inner.events[self.inner.pos];
                self.inner.pos += 1;

                let Ok((_id_bytes, version_raw)) = decode_event_key(key) else {
                    self.inner.poisoned = true;
                    return Some(Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: None,
                    }));
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

                let Ok((schema_version, event_type, payload)) = decode_event_value(value) else {
                    self.inner.poisoned = true;
                    return Some(Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    }));
                };

                let Some(version) = Version::new(version_raw) else {
                    self.inner.poisoned = true;
                    return Some(Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    }));
                };

                let Ok(envelope) =
                    PersistedEnvelope::try_new(version, event_type, schema_version, payload, ())
                else {
                    self.inner.poisoned = true;
                    return Some(Err(FjallError::CorruptValue {
                        stream_id: self.label,
                        version: Some(version_raw),
                    }));
                };

                self.last_version = Some(version);
                return Some(Ok(envelope));
            }

            // Buffer exhausted (or recovering from poison). Register for
            // notification BEFORE reading to avoid the race where an
            // append happens between our read and our wait.
            let notified = self.store.notify.notified();

            let from = match self.next_read_version() {
                Ok(v) => v,
                Err(err) => return Some(Err(err)),
            };
            if let Err(err) = self.refill(from).await {
                return Some(Err(err));
            }

            // If refill found new events, loop back to yield them.
            if !self.inner.events.is_empty() {
                continue;
            }

            // No new events — wait for notification, then retry.
            notified.await;
        }
    }
}
