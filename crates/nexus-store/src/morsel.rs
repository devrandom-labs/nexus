use std::borrow::Cow;

use nexus::Version;

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
