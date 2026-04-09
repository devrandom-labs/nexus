use nexus::Version;
use std::num::NonZeroU32;

// =============================================================================
// PendingEnvelope — typestate builder ensures compile-time valid construction
// =============================================================================

/// Event envelope for the write path — fully owned, going to the database.
///
/// Cannot be constructed directly. Use the typestate builder:
/// ```ignore
/// pending_envelope(version)
///     .event_type("UserCreated")
///     .payload(bytes)
///     .build(metadata)
/// ```
///
/// Each step returns a different type — the compiler enforces that all
/// required fields are set before `build()` is callable.
#[derive(Debug)]
pub struct PendingEnvelope<M = ()> {
    version: Version,
    event_type: &'static str,
    schema_version: u32,
    payload: Vec<u8>,
    metadata: M,
}

impl<M> PendingEnvelope<M> {
    /// The event version in the stream.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The event type name (from `DomainEvent::name()`).
    #[must_use]
    pub const fn event_type(&self) -> &'static str {
        self.event_type
    }

    /// The serialized event payload.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// User-defined metadata.
    #[must_use]
    pub const fn metadata(&self) -> &M {
        &self.metadata
    }

    /// The schema version of the event.
    ///
    /// Defaults to 1 when not explicitly set via the builder.
    /// The `EventStore` save path sets this based on the upcaster chain
    /// to prevent upcasters from double-transforming new events.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

// =============================================================================
// Typestate builder — compile-time enforced construction
// =============================================================================

/// Step 1: has `version`, needs `event_type`.
pub struct WithVersion {
    version: Version,
}

/// Step 2: has `version` + `event_type`, needs payload.
pub struct WithEventType {
    version: Version,
    event_type: &'static str,
}

/// Step 3: has all core fields, needs metadata.
pub struct WithPayload {
    version: Version,
    event_type: &'static str,
    payload: Vec<u8>,
    schema_version: u32,
}

impl WithVersion {
    /// Set the event type name (from `DomainEvent::name()`).
    #[must_use]
    pub const fn event_type(self, event_type: &'static str) -> WithEventType {
        WithEventType {
            version: self.version,
            event_type,
        }
    }
}

impl WithEventType {
    /// Set the serialized event payload.
    #[must_use]
    pub const fn payload(self, payload: Vec<u8>) -> WithPayload {
        WithPayload {
            version: self.version,
            event_type: self.event_type,
            payload,
            schema_version: 1,
        }
    }
}

impl WithPayload {
    /// Override the schema version (default: 1).
    ///
    /// Uses [`NonZeroU32`] to make `schema_version = 0` a compile-time
    /// error. This matches the invariant enforced by
    /// `PersistedEnvelope::try_new` on the read path — symmetric by
    /// construction, not by runtime checks.
    ///
    /// The `EventStore` save path uses this to stamp new events at the
    /// current schema version, preventing upcasters from re-transforming
    /// events that are already in the latest format.
    #[must_use]
    pub const fn schema_version(mut self, schema_version: NonZeroU32) -> Self {
        self.schema_version = schema_version.get();
        self
    }

    /// Build with user-defined metadata.
    ///
    /// # Validation contract
    ///
    /// The builder deliberately accepts empty `event_type`.
    /// Content validation (e.g. rejecting empty strings) is the responsibility
    /// of the `EventStore` facade, not the envelope builder. This keeps the
    /// builder simple and composable while deferring policy to the layer that
    /// owns it. See `security_tests.rs` C2 tests for the documented contract.
    #[must_use]
    pub fn build<M>(self, metadata: M) -> PendingEnvelope<M> {
        PendingEnvelope {
            version: self.version,
            event_type: self.event_type,
            schema_version: self.schema_version,
            payload: self.payload,
            metadata,
        }
    }

    /// Build with no metadata (`M = ()`).
    ///
    /// See [`Self::build`] for the validation contract — the same rules apply.
    #[must_use]
    pub fn build_without_metadata(self) -> PendingEnvelope<()> {
        self.build(())
    }
}

/// Start building a `PendingEnvelope`.
///
/// ```ignore
/// pending_envelope(version)
///     .event_type("UserCreated")
///     .payload(bytes)
///     .build(metadata)
/// ```
#[must_use]
pub const fn pending_envelope(version: Version) -> WithVersion {
    WithVersion { version }
}
