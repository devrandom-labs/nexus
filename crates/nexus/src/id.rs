use std::fmt::{Debug, Display};
use std::hash::Hash;

use crate::ErrorId;

pub trait Id: Clone + Send + Sync + Debug + Hash + Eq + Display + AsRef<[u8]> + 'static {
    /// Fixed byte length of this ID's storage representation.
    /// All instances must return exactly this many bytes from `as_ref()`.
    const BYTE_LEN: usize;

    /// Stack-allocated diagnostic label for error messages.
    ///
    /// If the display representation exceeds 64 bytes it is truncated on a
    /// char boundary and the tail is replaced with `…`, so truncation is
    /// always visually signalled (never silent). See [`ErrorId`].
    fn to_label(&self) -> ErrorId {
        ErrorId::from_display(self)
    }
}
