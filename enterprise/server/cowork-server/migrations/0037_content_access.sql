-- Infrastructure administration alone never grants access to customer content.
ALTER TABLE management_members ADD COLUMN content_admin BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE management_members ADD CONSTRAINT content_admin_requires_admin CHECK (NOT content_admin OR role='admin');
CREATE TABLE management_instance_features (
    organisation_id UUID NOT NULL,
    instance_id UUID PRIMARY KEY,
    content_access_version INTEGER NOT NULL CHECK (content_access_version=1),
    refreshed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
