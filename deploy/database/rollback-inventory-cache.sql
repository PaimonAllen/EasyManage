-- Stop the platform first. Agents keep their durable queues; no history is removed.
BEGIN;
SET LOCAL lock_timeout = '2s';
UPDATE metrics_latest m SET metrics=i.payload||m.metrics FROM node_snapshot_inventory i WHERE i.node_id=m.node_id;
DROP VIEW metrics_snapshot;
DROP TABLE node_snapshot_inventory;
DELETE FROM _sqlx_migrations WHERE version=22;
COMMIT;
