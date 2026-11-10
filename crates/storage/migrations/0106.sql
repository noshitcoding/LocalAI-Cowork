ALTER TABLE team_effect_intents ADD COLUMN request_id TEXT;
CREATE UNIQUE INDEX team_effect_request_scope ON team_effect_intents(run_id,request_id);
