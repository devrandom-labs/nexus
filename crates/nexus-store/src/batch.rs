//! Bounded batch size for stream reads and subscription refills.
//!
//! Read paths never materialize more than `batch_size` event rows at once.
//! [`BatchSize`] carries the `1..=MAX_BATCH` invariant by construction, so an
//! out-of-range value is unrepresentable past [`BatchSize::new`] — the builder
//! that accepts a `BatchSize` cannot be handed an invalid one.

use thiserror::Error;

/// Largest permitted batch size — the compile-time ceiling on resident rows.
pub const MAX_BATCH: usize = 4096;

/// Default batch size when none is configured.
pub const DEFAULT_BATCH: usize = 256;

/// A validated read / refill batch size in `1..=MAX_BATCH`.
///
/// Construct via [`BatchSize::new`]; the invalid range is rejected there, so
/// every `BatchSize` value in the program is in range by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchSize(usize);

impl BatchSize {
    /// The default batch size ([`DEFAULT_BATCH`]).
    pub const DEFAULT: Self = Self(DEFAULT_BATCH);

    /// Construct a `BatchSize`, rejecting `0` and values above [`MAX_BATCH`].
    ///
    /// # Errors
    ///
    /// Returns [`BatchSizeError`] if `n == 0` or `n > MAX_BATCH`.
    pub const fn new(n: usize) -> Result<Self, BatchSizeError> {
        if n == 0 || n > MAX_BATCH {
            return Err(BatchSizeError {
                requested: n,
                max: MAX_BATCH,
            });
        }
        Ok(Self(n))
    }

    /// The validated value, always in `1..=MAX_BATCH`.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for BatchSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Error returned when a configured batch size is out of range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("batch size must be in 1..={max}, got {requested}")]
pub struct BatchSizeError {
    /// The rejected value.
    pub requested: usize,
    /// The compile-time maximum ([`MAX_BATCH`]).
    pub max: usize,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::expect_used, reason = "test code")]
mod tests {
    use super::{BatchSize, BatchSizeError, DEFAULT_BATCH, MAX_BATCH};

    #[test]
    fn rejects_zero() {
        let err = BatchSize::new(0).unwrap_err();
        assert_eq!(
            err,
            BatchSizeError {
                requested: 0,
                max: MAX_BATCH
            }
        );
    }

    #[test]
    fn rejects_above_max() {
        let err = BatchSize::new(MAX_BATCH + 1).unwrap_err();
        assert_eq!(err.requested, MAX_BATCH + 1);
        assert_eq!(err.max, MAX_BATCH);
    }

    #[test]
    fn accepts_lower_and_upper_bound() {
        assert_eq!(BatchSize::new(1).expect("1 is valid").get(), 1);
        assert_eq!(
            BatchSize::new(MAX_BATCH).expect("MAX is valid").get(),
            MAX_BATCH
        );
    }

    #[test]
    fn default_is_default_batch() {
        assert_eq!(BatchSize::DEFAULT.get(), DEFAULT_BATCH);
        assert_eq!(BatchSize::default().get(), DEFAULT_BATCH);
    }
}
