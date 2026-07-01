//! Pure, IO-free append planner — the single source of truth for fjall's
//! write path.
//!
//! Both [`RawEventStore::append`](crate::store) and the
//! [`AtomicAppend`](nexus_store::import::AtomicAppend) impl reduce to the same
//! per-stream work: validate that a run's versions are strictly sequential from
//! the stream's current version, then encode each event's primary key, `$all`
//! key, and 16-byte-aligned wire frame while assigning a running
//! [`GlobalSeq`](crate::GlobalSeq). None of that touches fjall — it is a pure
//! function of `(current_version, current_global, id, envelopes)`, so it lives
//! here, unit-tested with no database, exactly as `nexus-postgres` factors its
//! `prepare_inserts` out of `append`.
//!
//! The two public methods differ only in their *error domain* and in the
//! single-stream-vs-cross-run validation that wraps this core: `append` maps a
//! [`PlanError`] into [`AppendError`](nexus_store::error::AppendError); the
//! atomic path owns its own cross-run head/projected-head/non-injective-route
//! check (index-based conflicts) and then calls [`plan_run`] purely to stage.

use bytes::Bytes;
use nexus::{ErrorId, Version};
use nexus_store::PendingEnvelope;
use nexus_store::StreamKey;
use nexus_store::wire;

use crate::error::reason_label;
use crate::wire_key::{encode_event_key, encode_global_key};

/// A validated, encoded event row ready to `tx.insert` into the `events` and
/// `events_global` partitions.
///
/// Holding a `StagedRow` is proof the event passed the strict-sequential check
/// and encoded cleanly — it owns its key bytes and frame and borrows nothing,
/// so the IO shell that consumes it is a mechanical insert loop.
#[derive(Debug)]
pub struct StagedRow {
    /// `events` partition key: `[u16 BE id_len][id_bytes][u64 BE version]`.
    pub event_key: Vec<u8>,
    /// `events_global` partition key: `[u64 BE global_seq][u64 BE version]`.
    pub global_key: [u8; 16],
    /// The 16-byte-aligned V2 wire frame (the value written to both partitions).
    pub frame: Bytes,
}

/// The result of planning one stream's append run.
#[derive(Debug)]
pub struct PlannedRun {
    /// The staged rows in version order (empty iff `envelopes` was empty).
    pub rows: Vec<StagedRow>,
    /// The stream's new version counter (== `current_version` if empty).
    pub new_version: u64,
    /// The store-global counter after this run (== `current_global` if empty).
    pub ending_global: u64,
}

/// Neutral planner failure — each caller maps it into its own error domain
/// (rule 3: one variant = one failure domain; overflow is never a conflict).
#[derive(Debug)]
pub enum PlanError {
    /// A run version was not the strict successor of the prior version.
    /// `expected` is the version the run should have carried at that position;
    /// `actual` is the version it did carry.
    Conflict {
        expected: Option<Version>,
        actual: Option<Version>,
    },
    /// The stream version sequence would advance past `u64::MAX`.
    VersionOverflow,
    /// The store-global sequence would advance past `u64::MAX`.
    GlobalSeqOverflow,
    /// An event failed to encode (over-long id, or a wire-frame build failure).
    InvalidInput { version: u64, reason: ErrorId<128> },
}

/// Plan one stream's append run: validate strict-sequential versions from
/// `current_version`, then encode + stage each event assigning a running
/// `GlobalSeq` from `current_global`. Pure — no fjall, no `tx`.
///
/// `current_version` is the stream's current max (0 = fresh stream); the run's
/// first event must be version `current_version + 1`. `current_global` is the
/// store-wide counter; the first staged event is stamped `current_global + 1`.
pub fn plan_run(
    current_version: u64,
    current_global: u64,
    id: &StreamKey,
    envelopes: &[PendingEnvelope],
) -> Result<PlannedRun, PlanError> {
    let id_bytes = id.as_ref();
    let mut expected = current_version;
    let mut global_seq = current_global;
    let mut rows = Vec::with_capacity(envelopes.len());

    for env in envelopes {
        // Strict-sequential check via a running checked_add counter — no
        // index→u64 cast, overflow-safe near u64::MAX (rule 2).
        expected = expected.checked_add(1).ok_or(PlanError::VersionOverflow)?;
        let version = env.version().as_u64();
        if version != expected {
            return Err(PlanError::Conflict {
                expected: Version::new(expected),
                actual: Some(env.version()),
            });
        }
        global_seq = global_seq
            .checked_add(1)
            .ok_or(PlanError::GlobalSeqOverflow)?;

        let event_key =
            encode_event_key(id_bytes, version).map_err(|e| PlanError::InvalidInput {
                version,
                reason: reason_label(&e),
            })?;
        let frame = wire::encode_frame(
            env.schema_version_value(),
            &env.event_type_value(),
            &env.payload_value(),
            env.metadata_value().as_ref(),
        )
        .map_err(|e| PlanError::InvalidInput {
            version,
            reason: reason_label(&e),
        })?;
        let global_key = encode_global_key(global_seq, version);

        rows.push(StagedRow {
            event_key,
            global_key,
            frame: frame.value,
        });
    }

    let new_version = envelopes
        .last()
        .map_or(current_version, |last| last.version().as_u64());
    Ok(PlannedRun {
        rows,
        new_version,
        ending_global: global_seq,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
#[allow(clippy::panic, reason = "test code")]
mod tests {
    use super::*;
    use nexus_store::envelope::pending_envelope;

    fn sk() -> StreamKey {
        StreamKey::from_slice(b"s")
    }

    /// A minimal valid envelope at `version` (>= 1).
    fn env(version: u64) -> PendingEnvelope {
        pending_envelope(Version::new(version).unwrap())
            .event_type("E")
            .payload(b"p".as_slice())
            .unwrap()
            .build()
    }

    // 1. Sequence/protocol — happy paths ------------------------------------

    #[test]
    fn fresh_stream_three_events_stamps_versions_and_global() {
        let evs = [env(1), env(2), env(3)];
        let p = plan_run(0, 0, &sk(), &evs).unwrap();
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.new_version, 3);
        assert_eq!(p.ending_global, 3);
    }

    #[test]
    fn existing_stream_continues_version_and_global() {
        // current_version = 5, current_global = 40, run [6, 7]
        let evs = [env(6), env(7)];
        let p = plan_run(5, 40, &sk(), &evs).unwrap();
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.new_version, 7);
        assert_eq!(p.ending_global, 42);
    }

    #[test]
    fn empty_batch_stages_nothing_and_leaves_counters() {
        let p = plan_run(5, 40, &sk(), &[]).unwrap();
        assert!(p.rows.is_empty());
        assert_eq!(p.new_version, 5);
        assert_eq!(p.ending_global, 40);
    }

    // 2. Conflict / defensive boundary — failure paths ----------------------

    #[test]
    fn gapped_run_is_conflict_with_expected_and_actual() {
        // [1, 3] skips version 2 → Conflict at the second event.
        let evs = [env(1), env(3)];
        match plan_run(0, 0, &sk(), &evs).unwrap_err() {
            PlanError::Conflict { expected, actual } => {
                assert_eq!(expected, Version::new(2));
                assert_eq!(actual, Version::new(3));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[test]
    fn out_of_order_run_is_conflict_at_first_event() {
        // [2, 1] on a fresh stream → first event should be version 1.
        let evs = [env(2), env(1)];
        match plan_run(0, 0, &sk(), &evs).unwrap_err() {
            PlanError::Conflict { expected, actual } => {
                assert_eq!(expected, Version::new(1));
                assert_eq!(actual, Version::new(2));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[test]
    fn wrong_start_version_is_conflict() {
        // current_version = 5, run starts at 7 (should be 6).
        let evs = [env(7)];
        assert!(matches!(
            plan_run(5, 40, &sk(), &evs).unwrap_err(),
            PlanError::Conflict { .. }
        ));
    }

    #[test]
    fn version_overflow_at_ceiling_is_version_overflow_not_conflict() {
        // current_version = u64::MAX → the first successor overflows. This must
        // be VersionOverflow, never Conflict (rule 3: overflow is not a retry-
        // eligible conflict). The envelope version is irrelevant — overflow is
        // detected before the version is compared.
        let evs = [env(1)];
        assert!(matches!(
            plan_run(u64::MAX, 0, &sk(), &evs).unwrap_err(),
            PlanError::VersionOverflow
        ));
    }

    #[test]
    fn global_seq_overflow_at_ceiling_is_global_seq_overflow() {
        // current_global = u64::MAX → stamping the first event overflows.
        let evs = [env(1)];
        assert!(matches!(
            plan_run(0, u64::MAX, &sk(), &evs).unwrap_err(),
            PlanError::GlobalSeqOverflow
        ));
    }

    // 3. Staged bytes are the real encoders --------------------------------

    #[test]
    fn staged_keys_match_the_wire_key_codecs() {
        let evs = [env(1)];
        let p = plan_run(0, 0, &sk(), &evs).unwrap();
        // event_key = [u16 id_len][id][u64 version]; global_key = [gseq][ver].
        assert_eq!(p.rows[0].event_key, encode_event_key(b"s", 1).unwrap());
        assert_eq!(p.rows[0].global_key, encode_global_key(1, 1));
        assert!(!p.rows[0].frame.is_empty());
    }
}
