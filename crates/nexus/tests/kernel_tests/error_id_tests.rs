//! Tests for `nexus::ErrorId` — the bounded, truncation-signalling
//! diagnostic label that seals `arrayvec` out of the public API.

use nexus::ErrorId;
use proptest::prelude::*;

const ELLIPSIS: char = '…';

// ── Sequence/protocol: construct → read → compare ──────────────────────────

#[test]
fn short_value_is_stored_verbatim() {
    let id = ErrorId::<64>::from_display(&"order-42");
    assert_eq!(id.as_str(), "order-42");
    assert_eq!(id.len(), 8);
    assert!(!id.as_str().ends_with(ELLIPSIS));
}

#[test]
fn empty_value_is_empty() {
    let id = ErrorId::<64>::from_display(&"");
    assert!(id.is_empty());
    assert_eq!(id.as_str(), "");
}

#[test]
fn display_matches_as_str() {
    let id = ErrorId::<64>::from_display(&"stream-x");
    assert_eq!(format!("{id}"), "stream-x");
}

#[test]
fn equal_inputs_are_equal_and_hash_equal() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let a = ErrorId::<64>::from_display(&"same");
    let b = ErrorId::<64>::from_display(&"same");
    assert_eq!(a, b);

    let mut ha = DefaultHasher::new();
    let mut hb = DefaultHasher::new();
    a.hash(&mut ha);
    b.hash(&mut hb);
    assert_eq!(ha.finish(), hb.finish());
}

#[test]
fn default_is_empty() {
    let id = ErrorId::<64>::default();
    assert!(id.is_empty());
}

#[test]
fn error_id_is_copy() {
    // `ErrorId` must be `Copy`: the fjall adapter's error paths stamp the
    // same label into several variants in one expression (e.g. `scan.rs`
    // builds `CorruptValue` then reuses the id for `EnvelopeCorrupt`), which
    // only compiles move-free when the label is `Copy`. The wrapped
    // `ArrayString<N>` is `Copy`, so this is sound and additive.
    fn assert_copy<T: Copy>() {}
    assert_copy::<ErrorId<64>>();
    assert_copy::<ErrorId<128>>();

    let a = ErrorId::<64>::from_display(&"copy-me");
    let b = a; // copies, not moves
    assert_eq!(a, b);
    assert_eq!(a.as_str(), "copy-me");
}

// ── Defensive boundary: lengths {0, 1, N-1, N, N+1} ────────────────────────

#[test]
fn exactly_capacity_is_not_truncated() {
    let input = "a".repeat(64);
    let id = ErrorId::<64>::from_display(&input);
    assert_eq!(id.as_str(), input);
    assert_eq!(id.len(), 64);
    assert!(!id.as_str().ends_with(ELLIPSIS));
}

#[test]
fn one_below_capacity_is_not_truncated() {
    let input = "a".repeat(63);
    let id = ErrorId::<64>::from_display(&input);
    assert_eq!(id.as_str(), input);
}

#[test]
fn one_above_capacity_is_truncated_with_ellipsis() {
    let input = "a".repeat(65);
    let id = ErrorId::<64>::from_display(&input);
    assert!(id.len() <= 64);
    assert!(id.as_str().ends_with(ELLIPSIS));
    assert!(id.as_str().starts_with("aaa"));
}

#[test]
fn far_above_capacity_is_truncated_with_ellipsis() {
    let input = "z".repeat(1000);
    let id = ErrorId::<64>::from_display(&input);
    assert!(id.len() <= 64);
    assert!(id.as_str().ends_with(ELLIPSIS));
}

// ── Defensive boundary: multi-byte char straddling the cap ─────────────────

#[test]
fn multibyte_char_at_boundary_does_not_panic_and_stays_valid() {
    // "€" is 3 bytes (U+20AC). 30 of them = 90 bytes; cap 64 falls
    // mid-character. Must not panic, must be valid UTF-8 (guaranteed by
    // &str), and must end with the ellipsis marker.
    let input = "€".repeat(30);
    let id = ErrorId::<64>::from_display(&input);
    assert!(id.len() <= 64);
    assert!(id.as_str().ends_with(ELLIPSIS));
    // No assertion on exact bytes — only that it is a valid, bounded label.
}

// ── Custom width (the fjall reason case uses N = 128) ──────────────────────

#[test]
fn custom_width_truncates_at_its_own_cap() {
    let input = "r".repeat(200);
    let id = ErrorId::<128>::from_display(&input);
    assert!(id.len() <= 128);
    assert!(id.as_str().ends_with(ELLIPSIS));
}

// ── Property: bounded, valid, verbatim-when-fits ───────────────────────────

proptest! {
    // Strategy includes the boundary lengths explicitly.
    #[test]
    fn never_panics_and_stays_within_cap(
        input in prop_oneof![
            Just(String::new()),
            "[a-z]{1}",
            "[a-z]{31}",
            "[a-z]{32}",
            "[a-z]{33}",
            "\\PC{0,500}",   // arbitrary non-control unicode, any length
        ]
    ) {
        let id = ErrorId::<32>::from_display(&input);
        prop_assert!(id.as_str().len() <= 32);
        if input.len() <= 32 {
            prop_assert_eq!(id.as_str(), input.as_str());
        }
    }
}
