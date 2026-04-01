//! Architecture tests for the kernel.
//!
//! These verify that the kernel module has no dependencies on outer layers.
//! The kernel is the innermost layer — it must not know about commands,
//! queries, stores, infrastructure, or serialization.

#[test]
fn kernel_does_not_import_outer_layers() {
    let kernel_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/kernel");
    let forbidden_imports = [
        "use crate::domain",
        "use crate::command",
        "use crate::query",
        "use crate::store",
        "use crate::infra",
        "use crate::serde",
        "use crate::event",
    ];

    let mut violations = Vec::new();

    for entry in std::fs::read_dir(kernel_dir).expect("kernel dir should exist") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            let content = std::fs::read_to_string(&path).unwrap();
            for (line_num, line) in content.lines().enumerate() {
                for import in &forbidden_imports {
                    if line.contains(import) {
                        violations.push(format!(
                            "{}:{}: {}",
                            path.file_name().unwrap().to_str().unwrap(),
                            line_num + 1,
                            line.trim(),
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Kernel imports from outer layers!\n{}",
        violations.join("\n"),
    );
}
