use std::iter;

use nexus::{DomainEvent, Version};
use nexus_store::projection::{ProjectionTrigger, Projector};

/// FSM state for the projection event loop.
///
/// Three write-centric states tracking the relationship between
/// in-memory folded state and persisted state:
///
/// - [`Idle`](ProjectionStatus::Idle) — no events processed yet
/// - [`Pending`](ProjectionStatus::Pending) — events folded, write pending
/// - [`Committed`](ProjectionStatus::Committed) — trigger fired, state persisted
pub(crate) enum ProjectionStatus<S> {
    /// No events processed yet.
    Idle {
        state: S,
        checkpoint: Option<Version>,
    },
    /// Events folded, write pending.
    Pending {
        state: S,
        version: Version,
        checkpoint: Option<Version>,
    },
    /// Trigger fired, state persisted at version.
    Committed { state: S, version: Version },
}

/// Process a single decoded event through the FSM.
///
/// Folds the event through the projector, evaluates the trigger against
/// the checkpoint carried in the current status, and returns the new
/// status. Pure — no IO, no async.
pub(crate) fn apply_event<P, E, Trig>(
    projector: &P,
    trigger: &Trig,
    status: ProjectionStatus<P::State>,
    event: &E,
    version: Version,
) -> Result<ProjectionStatus<P::State>, P::Error>
where
    P: Projector<Event = E>,
    E: DomainEvent,
    Trig: ProjectionTrigger,
{
    let (state, checkpoint) = match status {
        ProjectionStatus::Idle { state, checkpoint } => (state, checkpoint),
        ProjectionStatus::Pending {
            state, checkpoint, ..
        } => (state, checkpoint),
        ProjectionStatus::Committed { state, version } => (state, Some(version)),
    };

    let new_state = projector.apply(state, event)?;
    let should_persist = trigger.should_project(checkpoint, version, iter::once(event.name()));

    if should_persist {
        Ok(ProjectionStatus::Committed {
            state: new_state,
            version,
        })
    } else {
        Ok(ProjectionStatus::Pending {
            state: new_state,
            version,
            checkpoint,
        })
    }
}

impl<S> ProjectionStatus<S> {
    /// The checkpoint (last persisted version), if any.
    ///
    /// Used by `Projection::run()` to determine the subscribe-from position.
    pub(crate) fn checkpoint(&self) -> Option<Version> {
        match self {
            ProjectionStatus::Idle { checkpoint, .. } => *checkpoint,
            ProjectionStatus::Pending { checkpoint, .. } => *checkpoint,
            ProjectionStatus::Committed { version, .. } => Some(*version),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code — relaxed lints"
)]
mod tests {
    use std::convert::Infallible;
    use std::num::NonZeroU64;

    use nexus::{DomainEvent, Message, Version};
    use nexus_store::projection::{EveryNEvents, ProjectionTrigger, Projector};

    use super::*;

    // ── Fixtures (same as prepared.rs, shared domain) ─────────────────

    #[derive(Debug)]
    struct Evt;
    impl Message for Evt {}
    impl DomainEvent for Evt {
        fn name(&self) -> &'static str {
            "Evt"
        }
    }

    struct SumProjector;
    impl Projector for SumProjector {
        type Event = Evt;
        type State = u64;
        type Error = Infallible;

        fn initial(&self) -> u64 {
            0
        }

        fn apply(&self, state: u64, _event: &Evt) -> Result<u64, Infallible> {
            Ok(state.wrapping_add(1))
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("apply failed")]
    struct ApplyError;

    struct FailingProjector;
    impl Projector for FailingProjector {
        type Event = Evt;
        type State = u64;
        type Error = ApplyError;

        fn initial(&self) -> u64 {
            0
        }

        fn apply(&self, _state: u64, _event: &Evt) -> Result<u64, ApplyError> {
            Err(ApplyError)
        }
    }

    struct AlwaysTrigger;
    impl ProjectionTrigger for AlwaysTrigger {
        fn should_project(
            &self,
            _old: Option<Version>,
            _new: Version,
            _names: impl Iterator<Item: AsRef<str>>,
        ) -> bool {
            true
        }
    }

    struct NeverTrigger;
    impl ProjectionTrigger for NeverTrigger {
        fn should_project(
            &self,
            _old: Option<Version>,
            _new: Version,
            _names: impl Iterator<Item: AsRef<str>>,
        ) -> bool {
            false
        }
    }

    fn v(n: u64) -> Version {
        Version::new(n).unwrap()
    }

    // ── 1. Sequence/Protocol: all 6 event transitions ─────────────────

    #[test]
    fn idle_to_pending_when_trigger_does_not_fire() {
        let status = ProjectionStatus::Idle {
            state: 0_u64,
            checkpoint: None,
        };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(1)).unwrap();
        let ProjectionStatus::Pending {
            state,
            version,
            checkpoint,
        } = result
        else {
            panic!("expected Pending");
        };
        assert_eq!(state, 1);
        assert_eq!(version, v(1));
        assert!(checkpoint.is_none());
    }

    #[test]
    fn idle_to_committed_when_trigger_fires() {
        let status = ProjectionStatus::Idle {
            state: 0_u64,
            checkpoint: None,
        };
        let result = apply_event(&SumProjector, &AlwaysTrigger, status, &Evt, v(1)).unwrap();
        let ProjectionStatus::Committed { state, version } = result else {
            panic!("expected Committed");
        };
        assert_eq!(state, 1);
        assert_eq!(version, v(1));
    }

    #[test]
    fn pending_to_pending_when_trigger_does_not_fire() {
        let status = ProjectionStatus::Pending {
            state: 1_u64,
            version: v(1),
            checkpoint: None,
        };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(2)).unwrap();
        let ProjectionStatus::Pending {
            state,
            version,
            checkpoint,
        } = result
        else {
            panic!("expected Pending");
        };
        assert_eq!(state, 2);
        assert_eq!(version, v(2));
        assert!(checkpoint.is_none());
    }

    #[test]
    fn pending_to_committed_when_trigger_fires() {
        let status = ProjectionStatus::Pending {
            state: 1_u64,
            version: v(1),
            checkpoint: None,
        };
        let result = apply_event(&SumProjector, &AlwaysTrigger, status, &Evt, v(2)).unwrap();
        let ProjectionStatus::Committed { state, version } = result else {
            panic!("expected Committed");
        };
        assert_eq!(state, 2);
        assert_eq!(version, v(2));
    }

    #[test]
    fn committed_to_pending_when_trigger_does_not_fire() {
        let status = ProjectionStatus::Committed {
            state: 3_u64,
            version: v(3),
        };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(4)).unwrap();
        let ProjectionStatus::Pending {
            state,
            version,
            checkpoint,
        } = result
        else {
            panic!("expected Pending");
        };
        assert_eq!(state, 4);
        assert_eq!(version, v(4));
        assert_eq!(checkpoint, Some(v(3)));
    }

    #[test]
    fn committed_to_committed_when_trigger_fires() {
        let status = ProjectionStatus::Committed {
            state: 3_u64,
            version: v(3),
        };
        let result = apply_event(&SumProjector, &AlwaysTrigger, status, &Evt, v(4)).unwrap();
        let ProjectionStatus::Committed { state, version } = result else {
            panic!("expected Committed");
        };
        assert_eq!(state, 4);
        assert_eq!(version, v(4));
    }

    // ── Multi-step sequence ───────────────────────────────────────────

    #[test]
    fn multi_step_idle_pending_pending_committed_pending_committed() {
        let every_3 = EveryNEvents(NonZeroU64::new(3).unwrap());

        // Idle → Pending (v1, no trigger at bucket 0)
        let s = ProjectionStatus::Idle {
            state: 0_u64,
            checkpoint: None,
        };
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(1)).unwrap();
        assert!(matches!(s, ProjectionStatus::Pending { .. }));

        // Pending → Pending (v2, still bucket 0)
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(2)).unwrap();
        assert!(matches!(s, ProjectionStatus::Pending { .. }));

        // Pending → Committed (v3, crosses to bucket 1)
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(3)).unwrap();
        assert!(matches!(s, ProjectionStatus::Committed { .. }));

        // Committed → Pending (v4, bucket 1 still)
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(4)).unwrap();
        assert!(matches!(s, ProjectionStatus::Pending { .. }));

        // Pending → Committed (v6, crosses to bucket 2)
        let s = apply_event(&SumProjector, &every_3, s, &Evt, v(6)).unwrap();
        let ProjectionStatus::Committed { state, version } = s else {
            panic!("expected Committed");
        };
        assert_eq!(state, 5);
        assert_eq!(version, v(6));
    }

    // ── 3. Defensive: error propagation from each variant ─────────────

    #[test]
    fn error_propagated_from_idle() {
        let status = ProjectionStatus::Idle {
            state: 0_u64,
            checkpoint: None,
        };
        assert!(apply_event(&FailingProjector, &AlwaysTrigger, status, &Evt, v(1)).is_err());
    }

    #[test]
    fn error_propagated_from_pending() {
        let status = ProjectionStatus::Pending {
            state: 0_u64,
            version: v(1),
            checkpoint: None,
        };
        assert!(apply_event(&FailingProjector, &AlwaysTrigger, status, &Evt, v(2)).is_err());
    }

    #[test]
    fn error_propagated_from_committed() {
        let status = ProjectionStatus::Committed {
            state: 0_u64,
            version: v(1),
        };
        assert!(apply_event(&FailingProjector, &AlwaysTrigger, status, &Evt, v(2)).is_err());
    }

    // ── Checkpoint correctness ────────────────────────────────────────

    #[test]
    fn checkpoint_preserved_through_pending_transitions() {
        let status = ProjectionStatus::Idle {
            state: 0_u64,
            checkpoint: Some(v(5)),
        };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(6)).unwrap();
        let ProjectionStatus::Pending { checkpoint, .. } = result else {
            panic!("expected Pending");
        };
        assert_eq!(checkpoint, Some(v(5)));
    }

    #[test]
    fn committed_version_becomes_checkpoint_on_next_pending() {
        // Commit at v(3), then no-trigger at v(4) → Pending with checkpoint = v(3)
        let status = ProjectionStatus::Committed {
            state: 3_u64,
            version: v(3),
        };
        let result = apply_event(&SumProjector, &NeverTrigger, status, &Evt, v(4)).unwrap();
        let ProjectionStatus::Pending { checkpoint, .. } = result else {
            panic!("expected Pending");
        };
        assert_eq!(checkpoint, Some(v(3)));
    }
}
