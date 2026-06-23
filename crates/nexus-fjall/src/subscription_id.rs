use nexus::Id;

/// Owned byte-key wrapper to satisfy the [`Id`] trait's `'static` bound
/// when re-reading from the store during subscription refills.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct OwnedStreamId(Vec<u8>);

impl OwnedStreamId {
    /// Create from any [`Id`] by capturing its byte representation.
    pub fn from_id(id: &impl Id) -> Self {
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
