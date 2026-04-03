use crate::upcaster::EventUpcaster;

/// A link in the upcaster chain. Fully monomorphized, zero-size when
/// upcasters are stateless (unit structs).
pub struct Chain<H, T>(pub H, pub T);

/// Compile-time upcaster chain. Dispatches to the first matching
/// upcaster in the chain. The compiler inlines the entire traversal.
pub trait UpcasterChain: Send + Sync {
    /// Try each upcaster in order. Returns `Some(...)` from the first
    /// match, or `None` if no upcaster handles this event type + version.
    fn try_upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Option<(String, u32, Vec<u8>)>;

    /// Check if any upcaster in the chain handles this event type + version.
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool;
}

/// Base case: empty chain. Always returns `None`.
impl UpcasterChain for () {
    #[inline]
    fn try_upcast(&self, _: &str, _: u32, _: &[u8]) -> Option<(String, u32, Vec<u8>)> {
        None
    }

    #[inline]
    fn can_upcast(&self, _: &str, _: u32) -> bool {
        false
    }
}

/// Recursive case: try head, fall through to tail.
impl<H: EventUpcaster, T: UpcasterChain> UpcasterChain for Chain<H, T> {
    #[inline]
    fn try_upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Option<(String, u32, Vec<u8>)> {
        if self.0.can_upcast(event_type, schema_version) {
            Some(self.0.upcast(event_type, schema_version, payload))
        } else {
            self.1.try_upcast(event_type, schema_version, payload)
        }
    }

    #[inline]
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool {
        self.0.can_upcast(event_type, schema_version)
            || self.1.can_upcast(event_type, schema_version)
    }
}
