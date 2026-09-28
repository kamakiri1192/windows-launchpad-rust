CREATE TABLE icons (
    app_id                    TEXT PRIMARY KEY,
    link_path                 TEXT NOT NULL,
    display_name              TEXT NOT NULL,
    source_modified_at        INTEGER NOT NULL,
    target_path               TEXT NOT NULL,
    target_modified_at        INTEGER NOT NULL,
    icon_location             TEXT NOT NULL,
    icon_index                INTEGER NOT NULL,
    image_w                   INTEGER NOT NULL,
    image_h                   INTEGER NOT NULL,
    image_rgba                BLOB NOT NULL,
    extraction_version        INTEGER NOT NULL,
    category                  TEXT NOT NULL DEFAULT 'fullbleed',
    scale                     REAL NOT NULL DEFAULT 1.0,
    last_seen_at              INTEGER NOT NULL,
    deleted_at                INTEGER,
    provider_icon_path        TEXT,
    provider_icon_modified_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX icons_last_seen ON icons(last_seen_at);

-- User data only: settings, launcher_state (including initial-layout undo),
-- and Windows identity_aliases. Icon resets never touch this table.
CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
