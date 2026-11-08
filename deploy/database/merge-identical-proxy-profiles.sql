-- Run with the platform stopped, or involved profiles paused and no check in flight.
-- psql -v ON_ERROR_STOP=1 -v keep_id=UUID -v drop_id=UUID -v keep_name=NAME -f this.sql
-- Only byte-identical current configurations are accepted. Keep a private backup first.
BEGIN;
SET LOCAL lock_timeout='2s';
SET LOCAL statement_timeout='30s';
SELECT pg_advisory_xact_lock(874033023);
SELECT set_config('easymanage.merge_keep',:'keep_id',TRUE);
SELECT set_config('easymanage.merge_drop',:'drop_id',TRUE);
SELECT set_config('easymanage.merge_name',:'keep_name',TRUE);
CREATE TEMP TABLE proxy_merge_map(old_id UUID PRIMARY KEY,new_id UUID NOT NULL,old_revision BIGINT,new_revision BIGINT,same_current BOOLEAN) ON COMMIT DROP;
DO $$
DECLARE
  kept proxy_lab_profiles%ROWTYPE;
  removed proxy_lab_profiles%ROWTYPE;
  old_node proxy_lab_nodes%ROWTYPE;
  new_node proxy_lab_nodes%ROWTYPE;
BEGIN
  IF current_setting('easymanage.merge_keep')=current_setting('easymanage.merge_drop') THEN RAISE EXCEPTION 'Cannot merge a profile into itself'; END IF;
  SELECT * INTO STRICT kept FROM proxy_lab_profiles WHERE id=current_setting('easymanage.merge_keep')::UUID FOR UPDATE;
  SELECT * INTO STRICT removed FROM proxy_lab_profiles WHERE id=current_setting('easymanage.merge_drop')::UUID FOR UPDATE;
  IF kept.content_hash<>removed.content_hash THEN RAISE EXCEPTION 'Profiles are not byte-identical'; END IF;
  IF (SELECT COUNT(*) FROM proxy_lab_nodes WHERE profile_id=kept.id AND active)<>(SELECT COUNT(*) FROM proxy_lab_nodes WHERE profile_id=removed.id AND active) THEN RAISE EXCEPTION 'Active node sets differ'; END IF;
  FOR old_node IN SELECT * FROM proxy_lab_nodes WHERE profile_id=removed.id ORDER BY id LOOP
    SELECT * INTO new_node FROM proxy_lab_nodes WHERE profile_id=kept.id AND identity_hash=old_node.identity_hash;
    IF NOT FOUND THEN
      IF old_node.active THEN RAISE EXCEPTION 'Active node identities differ'; END IF;
      -- AAD binds the node UUID and revision, not the profile: retain both and the ciphertext.
      UPDATE proxy_lab_nodes SET profile_id=kept.id,active=FALSE WHERE id=old_node.id;
      new_node:=old_node;
    ELSIF old_node.active AND NOT new_node.active THEN
      RAISE EXCEPTION 'Active node identities differ';
    END IF;
    INSERT INTO proxy_merge_map VALUES(old_node.id,new_node.id,old_node.revision,new_node.revision,old_node.active AND new_node.active);
  END LOOP;
  IF EXISTS(SELECT 1 FROM proxy_lab_rounds r CROSS JOIN LATERAL jsonb_array_elements(r.results) item LEFT JOIN proxy_merge_map m ON m.old_id=(item->>'id')::UUID WHERE r.profile_id=removed.id AND m.old_id IS NULL) THEN RAISE EXCEPTION 'History references an unknown node'; END IF;
END $$;
INSERT INTO proxy_lab_scores(node_id,provider,revision,ip,region,raw_score,purity,direction,state,detail,method,checked_at,observed_at)
SELECT m.new_id,s.provider,CASE WHEN m.same_current AND s.revision=m.old_revision THEN m.new_revision ELSE -1 END,s.ip,s.region,s.raw_score,s.purity,s.direction,s.state,s.detail,s.method,s.checked_at,s.observed_at
FROM proxy_lab_scores s JOIN proxy_merge_map m ON m.old_id=s.node_id WHERE m.old_id<>m.new_id
ON CONFLICT(node_id,provider) DO UPDATE SET revision=EXCLUDED.revision,ip=EXCLUDED.ip,region=EXCLUDED.region,raw_score=EXCLUDED.raw_score,purity=EXCLUDED.purity,direction=EXCLUDED.direction,state=EXCLUDED.state,detail=EXCLUDED.detail,method=EXCLUDED.method,checked_at=EXCLUDED.checked_at,observed_at=EXCLUDED.observed_at
WHERE ((EXCLUDED.revision=(SELECT revision FROM proxy_lab_nodes WHERE id=EXCLUDED.node_id))::INT,EXCLUDED.checked_at) > ((proxy_lab_scores.revision=(SELECT revision FROM proxy_lab_nodes WHERE id=EXCLUDED.node_id))::INT,proxy_lab_scores.checked_at);
CREATE TEMP TABLE proxy_merge_rounds ON COMMIT DROP AS
SELECT * FROM proxy_lab_rounds WHERE profile_id IN(current_setting('easymanage.merge_keep')::UUID,current_setting('easymanage.merge_drop')::UUID);
WITH expanded AS (
  SELECT r.slot,r.checked_at,r.revision,r.profile_id,
    item||jsonb_build_object('id',COALESCE(m.new_id,(item->>'id')::UUID)) AS observation,
    COALESCE((item->>'sampled_at')::TIMESTAMPTZ,r.checked_at) AS sampled_at
  FROM proxy_merge_rounds r CROSS JOIN LATERAL jsonb_array_elements(r.results) item
  LEFT JOIN proxy_merge_map m ON r.profile_id=current_setting('easymanage.merge_drop')::UUID AND m.old_id=(item->>'id')::UUID
), ranked AS (
  SELECT *,ROW_NUMBER() OVER(PARTITION BY slot,observation->>'id' ORDER BY (observation->>'state' IN('up','slow','down')) DESC,sampled_at DESC,(profile_id=current_setting('easymanage.merge_keep')::UUID) DESC) AS chosen
  FROM expanded
), merged AS (
  SELECT slot,MAX(checked_at) AS checked_at,
    COALESCE(MAX(revision) FILTER(WHERE profile_id=current_setting('easymanage.merge_keep')::UUID),(SELECT revision FROM proxy_lab_profiles WHERE id=current_setting('easymanage.merge_keep')::UUID)) AS revision,
    jsonb_agg(observation||jsonb_build_object('sampled_at',sampled_at) ORDER BY observation->>'id') FILTER(WHERE chosen=1) AS results
  FROM ranked GROUP BY slot
)
INSERT INTO proxy_lab_rounds(profile_id,slot,checked_at,revision,results)
SELECT current_setting('easymanage.merge_keep')::UUID,slot,checked_at,revision,results FROM merged
ON CONFLICT(profile_id,slot) DO UPDATE SET checked_at=EXCLUDED.checked_at,revision=EXCLUDED.revision,results=EXCLUDED.results;
DO $$
DECLARE keep_id UUID:=current_setting('easymanage.merge_keep')::UUID; drop_id UUID:=current_setting('easymanage.merge_drop')::UUID; was_enabled BOOLEAN;
BEGIN
  IF (SELECT COUNT(DISTINCT slot) FROM proxy_merge_rounds)<>(SELECT COUNT(*) FROM proxy_lab_rounds WHERE profile_id=keep_id) THEN RAISE EXCEPTION 'Merged history slot count changed'; END IF;
  SELECT enabled INTO was_enabled FROM proxy_lab_profiles WHERE id=drop_id;
  DELETE FROM proxy_lab_profiles WHERE id=drop_id;
  UPDATE proxy_lab_profiles SET enabled=(enabled OR was_enabled),name=COALESCE(NULLIF(current_setting('easymanage.merge_name'),''),name),updated_at=NOW() WHERE id=keep_id;
END $$;
COMMIT;
