/// Schema evolution via raw byte transformation.
///
/// Operates on raw bytes BEFORE codec deserialization. Transforms old
/// event schemas to new ones without needing old Rust types.
///
/// Upcasters are chained: V1 → V2 → V3. Applied in order during reads.
/// Writes always use the current schema version.
pub trait EventUpcaster: Send + Sync {
    /// Check if this upcaster handles the given event type and version.
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool;

    /// Transform the event payload.
    ///
    /// Returns `(new_event_type, new_schema_version, new_payload)`.
    /// Only called when `can_upcast` returned true.
    fn upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>);
}
