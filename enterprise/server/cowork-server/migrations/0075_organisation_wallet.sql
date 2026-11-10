-- Internal member budgets for the central managed-model proxy. No payment or
-- invoice data is inferred. Historical ledger rows survive membership removal.
CREATE TABLE management_wallet_settings (
    organisation_id UUID PRIMARY KEY REFERENCES management_organisations(id),
    currency TEXT NOT NULL DEFAULT 'EUR' CHECK(currency ~ '^[A-Z]{3}$'),
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    updated_by UUID REFERENCES users(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
INSERT INTO management_wallet_settings(organisation_id) SELECT id FROM management_organisations;
CREATE TABLE management_member_wallets (
    organisation_id UUID NOT NULL REFERENCES management_wallet_settings(organisation_id),
    user_id UUID NOT NULL REFERENCES users(id),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    balance_micros BIGINT NOT NULL DEFAULT 0 CHECK(balance_micros BETWEEN 0 AND 9000000000000),
    monthly_limit_micros BIGINT CHECK(monthly_limit_micros BETWEEN 0 AND 9000000000000),
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    updated_by UUID REFERENCES users(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(organisation_id,user_id)
);
CREATE TABLE management_wallet_reservations (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    user_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    model_resource_id UUID NOT NULL REFERENCES management_resource_catalog(id),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL CHECK(request_sha256 ~ '^[a-f0-9]{64}$'),
    currency TEXT NOT NULL CHECK(currency ~ '^[A-Z]{3}$'),
    maximum_micros BIGINT NOT NULL CHECK(maximum_micros BETWEEN 0 AND 9000000000000),
    state TEXT NOT NULL DEFAULT 'reserved' CHECK(state IN ('reserved','held','settled')),
    charged_micros BIGINT CHECK(charged_micros BETWEEN 0 AND maximum_micros),
    period_start DATE NOT NULL DEFAULT (date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC'))::DATE,
    receipt_sha256 TEXT CHECK(receipt_sha256 ~ '^[a-f0-9]{64}$'),
    metadata JSONB NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(metadata)='object' AND octet_length(metadata::TEXT)<=8192),
    held_reason TEXT CHECK(length(held_reason)<=120),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY(organisation_id,user_id) REFERENCES management_member_wallets(organisation_id,user_id),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id),
    UNIQUE(organisation_id,instance_id,user_id,request_id),
    CHECK((state='settled')=(charged_micros IS NOT NULL AND receipt_sha256 IS NOT NULL))
);
CREATE INDEX management_wallet_reservations_open ON management_wallet_reservations(organisation_id,user_id,period_start) WHERE state IN ('reserved','held');
CREATE TABLE management_wallet_ledger (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    user_id UUID NOT NULL,
    request_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('credit','debit','usage_charge')),
    amount_micros BIGINT NOT NULL CHECK(amount_micros BETWEEN 0 AND 9000000000000),
    balance_after_micros BIGINT NOT NULL CHECK(balance_after_micros BETWEEN 0 AND 9000000000000),
    currency TEXT NOT NULL CHECK(currency ~ '^[A-Z]{3}$'),
    wallet_revision BIGINT NOT NULL CHECK(wallet_revision>1),
    reason TEXT NOT NULL CHECK(length(reason) BETWEEN 1 AND 500),
    actor_user_id UUID REFERENCES users(id),
    reservation_id UUID UNIQUE REFERENCES management_wallet_reservations(id),
    request_sha256 TEXT NOT NULL CHECK(request_sha256 ~ '^[a-f0-9]{64}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY(organisation_id,user_id) REFERENCES management_member_wallets(organisation_id,user_id),
    UNIQUE(organisation_id,user_id,request_id),
    UNIQUE(organisation_id,user_id,wallet_revision),
    CHECK((kind='usage_charge')=(reservation_id IS NOT NULL)),
    CHECK(kind='usage_charge' OR amount_micros>0)
);
CREATE INDEX management_wallet_ledger_history ON management_wallet_ledger(organisation_id,user_id,created_at DESC,id);
CREATE FUNCTION reject_management_wallet_ledger_change() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'wallet ledger is append only';
END;
$$;
CREATE TRIGGER management_wallet_ledger_immutable BEFORE UPDATE OR DELETE ON management_wallet_ledger
    FOR EACH ROW EXECUTE FUNCTION reject_management_wallet_ledger_change();
CREATE FUNCTION create_management_wallet_settings() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO management_wallet_settings(organisation_id) VALUES(NEW.id);
    RETURN NEW;
END;
$$;
CREATE TRIGGER management_wallet_default_settings AFTER INSERT ON management_organisations
    FOR EACH ROW EXECUTE FUNCTION create_management_wallet_settings();
CREATE TRIGGER management_wallet_settings_active_org BEFORE INSERT OR UPDATE ON management_wallet_settings
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_member_wallet_active_org BEFORE INSERT OR UPDATE ON management_member_wallets
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_wallet_reservations_active_org BEFORE INSERT OR UPDATE ON management_wallet_reservations
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_wallet_ledger_active_org BEFORE INSERT ON management_wallet_ledger
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
