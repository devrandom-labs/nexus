use std::borrow::Cow;

use nexus::Version;

// ═══════════════════════════════════════════════════════════════════════════
// EventMorsel — data unit flowing through the transform pipeline
// ═══════════════════════════════════════════════════════════════════════════

/// A single event as it flows through the transform pipeline.
///
/// Borrows from the cursor buffer when no transform has fired (zero-copy).
/// Becomes owned after the first transform allocates.
///
/// Inspired by Polars' Morsel pattern: data + metadata wrapped in a
/// single type that flows through each pipeline step.
pub struct EventMorsel<'a> {
    event_type: Cow<'a, str>,
    schema_version: Version,
    payload: Cow<'a, [u8]>,
}

impl<'a> EventMorsel<'a> {
    /// Build with owned payload (from a transform).
    #[must_use]
    pub fn new(event_type: &str, schema_version: Version, payload: Vec<u8>) -> Self {
        Self {
            event_type: Cow::Owned(event_type.to_owned()),
            schema_version,
            payload: Cow::Owned(payload),
        }
    }

    /// Create a morsel borrowing from existing data (zero allocation).
    #[must_use]
    pub const fn borrowed(event_type: &'a str, schema_version: Version, payload: &'a [u8]) -> Self {
        Self {
            event_type: Cow::Borrowed(event_type),
            schema_version,
            payload: Cow::Borrowed(payload),
        }
    }

    /// The event type name.
    #[must_use]
    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    /// The schema version.
    #[must_use]
    pub const fn schema_version(&self) -> Version {
        self.schema_version
    }

    /// The serialized payload bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// True if all fields still borrow from the original source.
    #[must_use]
    pub const fn is_borrowed(&self) -> bool {
        matches!(
            (&self.event_type, &self.payload),
            (Cow::Borrowed(_), Cow::Borrowed(_))
        )
    }

    /// Return a new morsel with a different payload.
    #[must_use]
    pub fn with_payload(self, payload: Cow<'a, [u8]>) -> Self {
        Self { payload, ..self }
    }

    /// Return a new morsel with a different schema version.
    #[must_use]
    pub fn with_schema_version(self, schema_version: Version) -> Self {
        Self {
            schema_version,
            ..self
        }
    }

    /// Return a new morsel with a different event type.
    #[must_use]
    pub fn with_event_type(self, event_type: Cow<'a, str>) -> Self {
        Self { event_type, ..self }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Upcaster — schema migration trait
// ═══════════════════════════════════════════════════════════════════════════

/// Upcast old events to the current schema version.
///
/// `EventStore` calls [`upcast`](Upcaster::upcast) on the read path and
/// [`current_version`](Upcaster::current_version) on the write path. The proc
/// macro `#[nexus::transforms]` generates implementations; `()` is the no-op
/// passthrough.
///
/// # Compile-time guarantees (macro-generated impls)
///
/// The `#[nexus::transforms]` macro verifies, at compile time:
/// - `from >= 1` on every transform.
/// - `to == from + 1` on every transform (contiguity per step).
/// - No duplicate `(event_type, from)` pairs.
/// - **Chain coverage**: for each event type, every schema version in
///   `[1, current_version]` is reachable via the declared chain (no gaps).
///
/// These guarantees make a number of runtime errors unrepresentable for
/// macro-generated upcasters: the chain always terminates, every step
/// advances the schema version, and event types are non-empty literals.
/// Hand-rolled `Upcaster` implementations should preserve these properties
/// themselves; the trait does not check them at runtime.
pub trait Upcaster: Send + Sync {
    /// The error type produced by transform functions.
    ///
    /// Each upcaster implementation specifies its own concrete error type.
    /// The no-op `()` impl uses [`Infallible`](std::convert::Infallible).
    type Error: std::error::Error + Send + Sync + 'static;

    /// Run all matching transforms until the morsel is at the current schema version.
    ///
    /// # Errors
    ///
    /// Returns the implementation's `Self::Error` when a user-provided
    /// transform function fails. Macro-generated implementations propagate
    /// the transform's error verbatim; the wrapper context (event type,
    /// schema version) is the user's responsibility to encode in their own
    /// error type if needed.
    fn upcast<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, Self::Error>;

    /// Current schema version for an event type (stamped on new events).
    /// Returns `None` if the event type has no transforms.
    fn current_version(&self, event_type: &str) -> Option<Version>;
}

/// No-op upcaster — passthrough, no transforms.
impl Upcaster for () {
    type Error = std::convert::Infallible;

    #[inline]
    fn upcast<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, Self::Error> {
        Ok(morsel)
    }

    #[inline]
    fn current_version(&self, _event_type: &str) -> Option<Version> {
        None
    }
}
