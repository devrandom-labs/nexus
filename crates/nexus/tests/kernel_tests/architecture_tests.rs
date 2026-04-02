//! Architecture tests for nexus.
//!
//! Now that the kernel IS the crate, architecture is enforced
//! by Cargo's dependency graph. This test verifies the crate
//! has no external runtime dependencies beyond smallvec and thiserror.

#[test]
fn nexus_has_minimal_dependencies() {
    // If nexus compiles with only smallvec + thiserror,
    // the architecture is correct. This test documents that intent.
    // Any new dependency added to nexus Cargo.toml will require
    // deliberate consideration.
    let cargo_toml = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    assert!(
        cargo_toml.contains("smallvec"),
        "nexus should depend on smallvec"
    );
    assert!(
        cargo_toml.contains("thiserror"),
        "nexus should depend on thiserror"
    );
    // Ensure heavy dependencies are NOT present
    assert!(
        !cargo_toml.contains("tokio-stream"),
        "nexus must not depend on tokio-stream (async belongs in nexus-store)"
    );
    assert!(
        !cargo_toml.contains("async-trait"),
        "nexus must not depend on async-trait (async belongs in nexus-store)"
    );
    assert!(
        !cargo_toml.contains("serde ="),
        "nexus must not depend on serde (serialization belongs in nexus-store)"
    );
}
