use nexus::Version;

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn initial_version_is_one() {
    assert_eq!(Version::INITIAL.as_u64(), 1);
}

#[test]
fn new_returns_some_for_nonzero() {
    let v = Version::new(42).unwrap();
    assert_eq!(v.as_u64(), 42);
}

#[test]
fn new_returns_none_for_zero() {
    assert!(Version::new(0).is_none());
}

#[test]
fn new_returns_some_for_one() {
    let v = Version::new(1).unwrap();
    assert_eq!(v, Version::INITIAL);
}

#[test]
fn new_returns_some_for_u64_max() {
    let v = Version::new(u64::MAX).unwrap();
    assert_eq!(v.as_u64(), u64::MAX);
}

// ---------------------------------------------------------------------------
// next()
// ---------------------------------------------------------------------------

#[test]
fn next_increments_by_one() {
    let v = Version::INITIAL;
    let v2 = v.next().unwrap();
    assert_eq!(v2.as_u64(), 2);

    let v3 = v2.next().unwrap();
    assert_eq!(v3.as_u64(), 3);
}

#[test]
fn next_overflows_at_u64_max() {
    let v = Version::new(u64::MAX).unwrap();
    assert!(v.next().is_none());
}

#[test]
fn next_succeeds_at_u64_max_minus_one() {
    let v = Version::new(u64::MAX - 1).unwrap();
    let next = v.next().unwrap();
    assert_eq!(next.as_u64(), u64::MAX);
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

#[test]
fn display_shows_inner_value() {
    let v = Version::new(7).unwrap();
    assert_eq!(format!("{v}"), "7");
}

#[test]
fn display_initial() {
    assert_eq!(format!("{}", Version::INITIAL), "1");
}

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

#[test]
fn ordering_less_than() {
    let v1 = Version::new(1).unwrap();
    let v2 = Version::new(2).unwrap();
    assert!(v1 < v2);
}

#[test]
fn ordering_greater_than() {
    let v2 = Version::new(2).unwrap();
    let v1 = Version::new(1).unwrap();
    assert!(v2 > v1);
}

// ---------------------------------------------------------------------------
// Equality
// ---------------------------------------------------------------------------

#[test]
fn equality_same_value() {
    let v1 = Version::new(5).unwrap();
    let v2 = Version::new(5).unwrap();
    assert_eq!(v1, v2);
}

#[test]
fn inequality_different_values() {
    let v1 = Version::new(5).unwrap();
    let v2 = Version::new(6).unwrap();
    assert_ne!(v1, v2);
}

// ---------------------------------------------------------------------------
// Copy semantics
// ---------------------------------------------------------------------------

#[test]
fn copy_semantics() {
    let v1 = Version::new(3).unwrap();
    let v2 = v1;
    assert_eq!(v1, v2);
    // v1 is still usable after copy
    assert_eq!(v1.as_u64(), 3);
}

// ---------------------------------------------------------------------------
// Hash
// ---------------------------------------------------------------------------

#[test]
fn equal_versions_have_equal_hashes() {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut h1 = DefaultHasher::new();
    let mut h2 = DefaultHasher::new();

    Version::new(42).unwrap().hash(&mut h1);
    Version::new(42).unwrap().hash(&mut h2);

    assert_eq!(h1.finish(), h2.finish());
}

// ---------------------------------------------------------------------------
// VersionedEvent
// ---------------------------------------------------------------------------

#[test]
fn versioned_event_round_trip() {
    use nexus::VersionedEvent;

    let v = Version::new(10).unwrap();
    let ve = VersionedEvent::new(v, "payload");

    assert_eq!(ve.version(), v);
    assert_eq!(*ve.event(), "payload");
}

#[test]
fn versioned_event_into_parts() {
    use nexus::VersionedEvent;

    let v = Version::new(5).unwrap();
    let ve = VersionedEvent::new(v, 42_u32);
    let (version, event) = ve.into_parts();

    assert_eq!(version, v);
    assert_eq!(event, 42_u32);
}

#[test]
fn versioned_event_clone() {
    use nexus::VersionedEvent;

    let ve = VersionedEvent::new(Version::INITIAL, String::from("hello"));
    #[allow(clippy::redundant_clone, reason = "testing Clone impl")]
    let ve2 = ve.clone();

    assert_eq!(ve, ve2);
}

#[test]
fn versioned_event_equality() {
    use nexus::VersionedEvent;

    let a = VersionedEvent::new(Version::new(1).unwrap(), 100_i32);
    let b = VersionedEvent::new(Version::new(1).unwrap(), 100_i32);
    let c = VersionedEvent::new(Version::new(2).unwrap(), 100_i32);
    let d = VersionedEvent::new(Version::new(1).unwrap(), 200_i32);

    assert_eq!(a, b);
    assert_ne!(a, c); // different version
    assert_ne!(a, d); // different event
}
