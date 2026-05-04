use std::num::NonZeroU32;

use nexus::Id;

use crate::codec::Codec;

use super::pending::PendingState;
use super::persisted::PersistedState;
use super::store::StateStore;

/// Adapter that bridges a byte-level [`StateStore<Vec<u8>>`] to a typed
/// [`StateStore<S>`] by encoding/decoding through a [`Codec<S>`].
///
/// Use this when your storage backend works with raw bytes (e.g., fjall)
/// but consumers need typed state.
///
/// # Example
///
/// ```ignore
/// let byte_store: impl StateStore<Vec<u8>> = fjall_store;
/// let typed_store = CodecStateStore::new(byte_store, JsonCodec::default());
/// // typed_store: impl StateStore<MyState>
/// ```
pub struct CodecStateStore<SS, C> {
    store: SS,
    codec: C,
}

impl<SS, C> CodecStateStore<SS, C> {
    /// Create a new codec-bridged state store.
    #[must_use]
    pub const fn new(store: SS, codec: C) -> Self {
        Self { store, codec }
    }
}

impl<S, SS, C> StateStore<S> for CodecStateStore<SS, C>
where
    S: Send + Sync + 'static,
    SS: StateStore<Vec<u8>>,
    C: Codec<S>,
{
    type Error = CodecStateStoreError<SS::Error, C::Error>;

    async fn load(
        &self,
        id: &impl Id,
        schema_version: NonZeroU32,
    ) -> Result<Option<PersistedState<S>>, Self::Error> {
        let Some(raw) = self
            .store
            .load(id, schema_version)
            .await
            .map_err(CodecStateStoreError::Store)?
        else {
            return Ok(None);
        };

        let (version, schema_ver, bytes) = raw.into_parts();
        let label = id.to_label();
        let state = self
            .codec
            .decode(&label, &bytes)
            .map_err(CodecStateStoreError::Codec)?;

        Ok(Some(PersistedState::new(version, schema_ver, state)))
    }

    async fn save(&self, id: &impl Id, state: &PendingState<S>) -> Result<(), Self::Error> {
        let bytes = self
            .codec
            .encode(state.state())
            .map_err(CodecStateStoreError::Codec)?;

        let raw = PendingState::new(state.version(), state.schema_version(), bytes);
        self.store
            .save(id, &raw)
            .await
            .map_err(CodecStateStoreError::Store)
    }

    async fn delete(&self, id: &impl Id) -> Result<(), Self::Error> {
        self.store
            .delete(id)
            .await
            .map_err(CodecStateStoreError::Store)
    }
}

/// Error from [`CodecStateStore`] — either the underlying store or the codec.
#[derive(Debug, thiserror::Error)]
pub enum CodecStateStoreError<S, C> {
    /// The underlying byte-level store failed.
    #[error(transparent)]
    Store(S),
    /// Encoding or decoding failed.
    #[error(transparent)]
    Codec(C),
}
