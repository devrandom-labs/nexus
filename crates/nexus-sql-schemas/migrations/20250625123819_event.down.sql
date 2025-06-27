DROP INDEX IF EXISTS metadata_correlation_id_idx ON event_metadata (correlation_id);
DROP TABLE IF EXISTS event_metadata;
DROP TABLE IF EXISTS event;
