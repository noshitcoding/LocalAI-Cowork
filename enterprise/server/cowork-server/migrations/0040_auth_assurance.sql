ALTER TABLE auth_sessions ADD COLUMN second_factor_verified_at TIMESTAMPTZ;
ALTER TABLE native_authorization_codes ADD COLUMN second_factor_verified_at TIMESTAMPTZ;
-- Existing sessions/codes intentionally have no evidence of a second factor.
ALTER TABLE user_totp ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempts>=0);
ALTER TABLE user_totp ADD COLUMN failure_window_started_at TIMESTAMPTZ;
ALTER TABLE user_totp ADD COLUMN blocked_until TIMESTAMPTZ;
