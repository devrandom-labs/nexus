use nexus::kernel::Version;

#[test]
fn initial_version_is_zero() {
    assert_eq!(Version::INITIAL.as_u64(), 0);
}

#[test]
fn next_increments_by_one() {
    let v = Version::INITIAL;
    assert_eq!(v.next().as_u64(), 1);
    assert_eq!(v.next().next().as_u64(), 2);
}

#[test]
fn version_from_u64() {
    let v = Version::from_persisted(42);
    assert_eq!(v.as_u64(), 42);
}

#[test]
fn version_display() {
    let v = Version::from_persisted(7);
    assert_eq!(format!("{v}"), "7");
}

#[test]
fn version_ordering() {
    let v1 = Version::from_persisted(1);
    let v2 = Version::from_persisted(2);
    assert!(v1 < v2);
}

#[test]
fn version_equality() {
    let v1 = Version::from_persisted(5);
    let v2 = Version::from_persisted(5);
    assert_eq!(v1, v2);
}

#[test]
fn version_copy_semantics() {
    let v1 = Version::from_persisted(3);
    let v2 = v1;
    assert_eq!(v1, v2);
}
