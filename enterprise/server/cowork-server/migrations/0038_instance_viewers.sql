-- Version 2 adds viewer sessions. Older instance APIs keep their original
-- launch purpose and cannot accept the new read-only ticket.
ALTER TABLE management_instance_features DROP CONSTRAINT management_instance_features_content_access_version_check;
ALTER TABLE management_instance_features ADD CONSTRAINT management_instance_features_content_access_version_check CHECK (content_access_version IN (1,2));
