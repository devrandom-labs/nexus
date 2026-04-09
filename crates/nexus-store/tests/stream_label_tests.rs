use nexus_store::stream_label::ToStreamLabel;

// =============================================================================
// I1 — std variant: lossless representation
// =============================================================================

#[test]
fn std_label_preserves_full_content() {
    let label = "order-42".to_stream_label();
    assert_eq!(label.to_string(), "order-42");
}

#[test]
fn std_label_preserves_empty_string() {
    let label = "".to_stream_label();
    assert_eq!(label.to_string(), "");
}

#[test]
fn std_label_preserves_unicode() {
    let label = "café-naïve-日本語".to_stream_label();
    assert_eq!(label.to_string(), "café-naïve-日本語");
}

#[cfg(not(feature = "bounded-labels"))]
#[test]
fn std_label_preserves_long_content() {
    let long = "a".repeat(1000);
    let label = long.as_str().to_stream_label();
    assert_eq!(label.to_string(), long);
}

// =============================================================================
// I2 — std variant: Display round-trips
// =============================================================================

#[test]
fn std_display_round_trips() {
    let input = "stream/user/12345";
    let label = input.to_stream_label();
    assert_eq!(format!("{label}"), input);
}

// =============================================================================
// I3 — std variant: Debug wraps in quotes
// =============================================================================

#[test]
fn std_debug_wraps_in_quotes() {
    let label = "abc".to_stream_label();
    assert_eq!(format!("{label:?}"), "\"abc\"");
}

// =============================================================================
// I4 — std variant: PartialEq works correctly
// =============================================================================

#[test]
fn std_eq_same_content() {
    let a = "stream-1".to_stream_label();
    let b = "stream-1".to_stream_label();
    assert_eq!(a, b);
}

#[test]
fn std_ne_different_content() {
    let a = "stream-1".to_stream_label();
    let b = "stream-2".to_stream_label();
    assert_ne!(a, b);
}

// =============================================================================
// I5 — std variant: Clone produces equal value
// =============================================================================

#[test]
fn std_clone_equals_original() {
    let a = "order-99".to_stream_label();
    let b = a.clone();
    assert_eq!(a, b);
}

// =============================================================================
// I6 — blanket ToStreamLabel works for various Display types
// =============================================================================

#[test]
fn blanket_impl_works_for_integers() {
    let label = 42u64.to_stream_label();
    assert_eq!(label.to_string(), "42");
}

#[test]
fn blanket_impl_works_for_str_ref() {
    let s = "hello";
    let label = s.to_stream_label();
    assert_eq!(label.to_string(), "hello");
}

#[test]
fn blanket_impl_works_for_string() {
    let s = String::from("world");
    let label = s.to_stream_label();
    assert_eq!(label.to_string(), "world");
}

#[test]
fn blanket_impl_works_for_custom_display() {
    struct MyId(u32);
    impl std::fmt::Display for MyId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "my-{}", self.0)
        }
    }
    let label = MyId(7).to_stream_label();
    assert_eq!(label.to_string(), "my-7");
}

// =============================================================================
// I7 — from_display is pub(crate), not constructable externally
//      (cannot be tested from integration tests — this is a compile_fail)
// =============================================================================

// =============================================================================
// Bounded-labels invariant tests
// =============================================================================
// These test the bounded (IoT) variant. They only compile + run when the
// `bounded-labels` feature is enabled. Run with:
//   cargo test -p nexus-store --features bounded-labels -- stream_label

#[cfg(feature = "bounded-labels")]
mod bounded {
    use super::*;

    // I3 — len <= 64 always
    #[test]
    fn bounded_short_ascii_preserved() {
        let label = "order-42".to_stream_label();
        assert_eq!(label.to_string(), "order-42");
    }

    #[test]
    fn bounded_empty_string() {
        let label = "".to_stream_label();
        assert_eq!(label.to_string(), "");
    }

    // I4 — buf[0..len] is always valid UTF-8
    #[test]
    fn bounded_multibyte_at_boundary_does_not_split() {
        // 'é' is 2 bytes (0xC3 0xA9). Fill buffer so 'é' straddles byte 63-64.
        let prefix = "a".repeat(63);
        let input = format!("{prefix}é");
        let label = input.as_str().to_stream_label();
        let output = label.to_string();
        // Must be valid UTF-8 (this assert would fail if we split the codepoint)
        assert!(output.is_ascii() || std::str::from_utf8(output.as_bytes()).is_ok());
        // The 'é' doesn't fit — it should be excluded, and "..." appended
        // because the full input (65 bytes) exceeds 64.
        assert!(
            output.ends_with("..."),
            "expected truncation, got: {output}"
        );
    }

    #[test]
    fn bounded_3byte_char_at_boundary_does_not_split() {
        // '€' is 3 bytes (0xE2 0x82 0xAC). Fill so it straddles boundary.
        let prefix = "a".repeat(62);
        let input = format!("{prefix}€");
        let label = input.as_str().to_stream_label();
        let output = label.to_string();
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
        assert!(
            output.ends_with("..."),
            "expected truncation, got: {output}"
        );
    }

    #[test]
    fn bounded_4byte_char_at_boundary_does_not_split() {
        // '𝄞' is 4 bytes. Fill so it straddles boundary.
        let prefix = "a".repeat(61);
        let input = format!("{prefix}𝄞");
        let label = input.as_str().to_stream_label();
        let output = label.to_string();
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
        assert!(
            output.ends_with("..."),
            "expected truncation, got: {output}"
        );
    }

    #[test]
    fn bounded_pure_multibyte_string_stays_valid() {
        // All 3-byte chars, way over 64 bytes.
        let input = "日".repeat(30); // 90 bytes
        let label = input.as_str().to_stream_label();
        let output = label.to_string();
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
        assert!(output.ends_with("..."));
    }

    // I5 — truncation is signaled with "..." only when content was lost
    #[test]
    fn bounded_truncation_signaled_when_over_64() {
        let input = "x".repeat(100);
        let label = input.as_str().to_stream_label();
        let output = label.to_string();
        assert!(output.ends_with("..."));
        assert!(output.len() <= 64);
    }

    // I6 — no false truncation for exactly-fitting content
    #[test]
    fn bounded_exact_64_bytes_no_truncation() {
        let input = "a".repeat(64);
        let label = input.as_str().to_stream_label();
        let output = label.to_string();
        // Exactly 64 bytes fits — no data lost, no "..." needed.
        assert_eq!(output, input);
        assert!(!output.ends_with("..."));
    }

    #[test]
    fn bounded_under_64_bytes_no_truncation() {
        let input = "a".repeat(63);
        let label = input.as_str().to_stream_label();
        assert_eq!(label.to_string(), input);
    }

    #[test]
    fn bounded_65_bytes_triggers_truncation() {
        let input = "a".repeat(65);
        let label = input.as_str().to_stream_label();
        let output = label.to_string();
        assert!(output.ends_with("..."));
        assert!(output.len() <= 64);
        // First 61 chars should be 'a', then "..."
        assert_eq!(&output[..61], &"a".repeat(61));
    }

    // I7 — API parity: ?Sized works (str references)
    #[test]
    fn bounded_str_ref_works() {
        let s: &str = "hello";
        let label = s.to_stream_label();
        assert_eq!(label.to_string(), "hello");
    }

    // Eq / Clone
    #[test]
    fn bounded_eq_same_content() {
        let a = "stream-1".to_stream_label();
        let b = "stream-1".to_stream_label();
        assert_eq!(a, b);
    }

    #[test]
    fn bounded_ne_different_content() {
        let a = "stream-1".to_stream_label();
        let b = "stream-2".to_stream_label();
        assert_ne!(a, b);
    }

    #[test]
    fn bounded_clone_equals_original() {
        let a = "order-99".to_stream_label();
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn bounded_debug_wraps_in_quotes() {
        let label = "abc".to_stream_label();
        assert_eq!(format!("{label:?}"), "\"abc\"");
    }

    // Edge case: unicode that fits exactly at 64 bytes
    #[test]
    fn bounded_unicode_exact_fit() {
        // 'é' is 2 bytes. 32 * 2 = 64 bytes exactly.
        let input = "é".repeat(32);
        assert_eq!(input.len(), 64);
        let label = input.as_str().to_stream_label();
        assert_eq!(label.to_string(), input);
    }

    // Edge case: single char over 64 scenario (impossible in practice but tests boundary)
    #[test]
    fn bounded_all_4byte_chars_truncation() {
        // '🎵' is 4 bytes. 17 * 4 = 68 bytes > 64.
        let input = "🎵".repeat(17);
        let label = input.as_str().to_stream_label();
        let output = label.to_string();
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
        assert!(output.ends_with("..."));
        assert!(output.len() <= 64);
    }
}
