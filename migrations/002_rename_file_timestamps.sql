-- These timestamps describe source files, not database row updates.
ALTER TABLE icons RENAME COLUMN link_mtime TO source_modified_at;
ALTER TABLE icons RENAME COLUMN target_mtime TO target_modified_at;
ALTER TABLE icons RENAME COLUMN provider_icon_mtime TO provider_icon_modified_at;
