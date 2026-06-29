//! `StreamKey` — the store's stream-key type: validated raw bytes with a
//! human-readable label.

use core::fmt;

use bytes::Bytes;

/// The identifier of an event stream, as the store keys it: raw bytes.
///
/// The store is byte-level — at append/read it needs only a stream's **key
/// bytes** and a **label** for diagnostics, never a domain aggregate identity.
/// So the byte-level operations ([`RawEventStore::read_stream`](crate::RawEventStore::read_stream)
/// / [`append`](crate::RawEventStore::append),
/// [`export_stream`](crate::export::EventExporter::export_stream),
/// [`list_streams`](crate::export::StreamLister::list_streams), and the
/// [`import`](crate::import::EventImporter::import) target) speak `StreamKey`,
/// not the kernel's [`Id`](nexus::Id) — honestly separating "a stream key" from
/// "a domain identity" (issue #245). A consumer's typed domain id lives at the
/// repository layer; the repository translates it to a `StreamKey` (its job).
///
/// Like the wire-field newtypes in [`value`](crate::value) (`EventType`,
/// `Payload`, `Metadata`), `StreamKey` is a [`Bytes`]-backed newtype — so a
/// stream key can never be confused with a payload or metadata buffer, and it
/// carries a [`Display`](fmt::Display) the raw bytes cannot. A **non-UTF-8** id
/// round-trips losslessly (unlike the former `from_utf8_lossy` reconstruction).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct StreamKey(Bytes);

impl StreamKey {
    /// Build a `StreamKey` from any owned byte source (zero-copy for `Bytes` /
    /// `Vec<u8>` / `String` / `&'static` slices and strs).
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        Self(bytes.into())
    }

    /// Build a `StreamKey` by copying a borrowed byte slice — the convenient
    /// shape for an `import` route (`Fn(&[u8]) -> StreamKey`), for translating a
    /// domain `id.as_ref()` at the repository boundary, or for any `&[u8]` key.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self(Bytes::copy_from_slice(bytes))
    }

    /// The raw id bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consume into the backing `Bytes` (zero-copy).
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.0
    }
}

impl From<Bytes> for StreamKey {
    fn from(bytes: Bytes) -> Self {
        Self(bytes)
    }
}

impl AsRef<[u8]> for StreamKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// A `StreamKey` is itself a valid [`Id`](nexus::Id): it already carries every
/// supertrait the trait requires (clone, send/sync, debug, hash, eq, display,
/// `AsRef<[u8]>`, `'static`). This is purely additive — the byte-level store
/// API stays concrete (`&StreamKey`), but a raw key can now flow through the
/// typed convenience layers (`Subscription::subscribe`, a repository whose
/// `Aggregate::Id` is `StreamKey`) without a domain-id newtype. `BYTE_LEN` is
/// `0` by the workspace convention for variable-length ids.
impl nexus::Id for StreamKey {
    const BYTE_LEN: usize = 0;
}

/// The string when the bytes are valid UTF-8, else `0x…` lowercase hex —
/// faithful for binary ids, readable for the common string id, always
/// unambiguous.
impl fmt::Display for StreamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = core::str::from_utf8(&self.0) {
            return f.write_str(s);
        }
        f.write_str("0x")?;
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::StreamKey;
    use bytes::Bytes;

    #[test]
    fn round_trips_bytes_including_non_utf8() {
        let raw = Bytes::from_static(&[0x00, 0xff, 0x42]);
        let key = StreamKey::from_bytes(raw.clone());
        assert_eq!(key.as_bytes(), &[0x00, 0xff, 0x42]);
        assert_eq!(key.clone().into_bytes(), raw);
        assert_eq!(StreamKey::from_slice(&[0x00, 0xff, 0x42]), key);
    }

    #[test]
    fn display_is_the_string_for_utf8_ids() {
        assert_eq!(
            StreamKey::from_slice(b"account-123").to_string(),
            "account-123"
        );
        assert_eq!(StreamKey::from_slice(b"").to_string(), "");
    }

    #[test]
    fn display_is_hex_for_non_utf8_ids() {
        assert_eq!(
            StreamKey::from_slice(&[0x00, 0xff, 0x42]).to_string(),
            "0x00ff42"
        );
    }
}
