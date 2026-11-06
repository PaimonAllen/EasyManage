-- Separate cold inventory from the frequently rewritten heartbeat JSON.
-- Existing node identities, history, timestamps and public API values are retained.
SET LOCAL lock_timeout = '2s';
CREATE TABLE node_snapshot_inventory (
    node_id UUID PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    digest TEXT CHECK (digest IS NULL OR digest ~ '^[a-f0-9]{64}$'),
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload)='object')
);
INSERT INTO node_snapshot_inventory(node_id,payload)
SELECT m.node_id, COALESCE((SELECT jsonb_object_agg(key,value) FROM jsonb_each(m.metrics)
    WHERE key=ANY(ARRAY['block_devices','hardware_details','users','interfaces','overlay_networks','routes','proxy_services','inventory_collected_at','running_services','host_security','permission_audit','ai_tools','programming_tools']::TEXT[])), '{}'::JSONB)
FROM metrics_latest m;
UPDATE metrics_latest SET metrics=metrics-ARRAY['block_devices','hardware_details','users','interfaces','overlay_networks','routes','proxy_services','inventory_collected_at','running_services','host_security','permission_audit','ai_tools','programming_tools']::TEXT[];
-- Dynamic fields win: old Agents and a restored legacy snapshot remain readable.
CREATE VIEW metrics_snapshot AS
SELECT m.node_id,m.collected_at,COALESCE(i.payload,'{}'::JSONB)||m.metrics AS metrics,
       m.updated_at,m.agent_control
FROM metrics_latest m LEFT JOIN node_snapshot_inventory i ON i.node_id=m.node_id;
