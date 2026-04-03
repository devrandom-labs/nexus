use crate::error::UpcastError;

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
    ///
    /// # Implementor contract
    ///
    /// The returned `new_schema_version` **must** be strictly greater than
    /// the input `schema_version`. Upcasting is a forward migration — it
    /// must never produce the same or a lower version. The `EventStore`
    /// facade will validate this at runtime, but implementations should
    /// uphold this invariant to avoid silent data corruption.
    fn upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>);
}

const DEFAULT_CHAIN_LIMIT: u32 = 100;

/// Apply upcasters safely with validation and loop detection.
///
/// Chains upcasters until none match, validating after each step:
/// - Output `schema_version` must be strictly greater than input
/// - Output `event_type` must not be empty
/// - Total iterations bounded by 100 to prevent infinite loops
///
/// Returns the final `(event_type, schema_version, payload)`.
///
/// # Errors
///
/// Returns [`UpcastError`] if any validation fails or the chain limit
/// is exceeded.
pub fn apply_upcasters(
    upcasters: &[&dyn EventUpcaster],
    event_type: &str,
    schema_version: u32,
    payload: &[u8],
) -> Result<(String, u32, Vec<u8>), UpcastError> {
    let mut current_type = event_type.to_owned();
    let mut current_version = schema_version;
    let mut current_payload = payload.to_vec();
    let mut iterations = 0u32;

    loop {
        let matched = upcasters
            .iter()
            .find(|u| u.can_upcast(&current_type, current_version));

        let Some(upcaster) = matched else {
            break;
        };

        iterations += 1;
        if iterations > DEFAULT_CHAIN_LIMIT {
            return Err(UpcastError::ChainLimitExceeded {
                event_type: current_type,
                schema_version: current_version,
                limit: DEFAULT_CHAIN_LIMIT,
            });
        }

        let input_version = current_version;
        let input_type = current_type.clone();
        let (new_type, new_version, new_payload) =
            upcaster.upcast(&current_type, current_version, &current_payload);

        if new_version <= input_version {
            return Err(UpcastError::VersionNotAdvanced {
                event_type: input_type,
                input_version,
                output_version: new_version,
            });
        }

        if new_type.is_empty() {
            return Err(UpcastError::EmptyEventType {
                input_event_type: input_type,
                schema_version: new_version,
            });
        }

        current_type = new_type;
        current_version = new_version;
        current_payload = new_payload;
    }

    Ok((current_type, current_version, current_payload))
}
