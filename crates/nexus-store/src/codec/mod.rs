mod borrowing;
mod owning;
#[cfg(feature = "serde")]
pub mod serde;

#[cfg(feature = "serde")]
pub use self::serde::{SerdeCodec, SerdeFormat};
pub use borrowing::BorrowingCodec;
pub use owning::Codec;
