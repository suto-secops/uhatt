//! Forward-only schema migrations keyed on `PRAGMA user_version`.
//!
//! Each entry in `MIGRATIONS` is one version step, applied in its own
//! transaction. To evolve the schema, append a new `&str` - never edit an
//! existing one.

use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[V1, V2, V3, V4];

/// Number of migration steps the code expects the database to be at.
pub const COUNT: usize = MIGRATIONS.len();

/// Bring `conn` up to `COUNT`. A no-op if it is already current.
pub fn run(conn: &mut Connection) -> rusqlite::Result<()> {
    let mut version: usize =
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;

    while version < COUNT {
        let sql = MIGRATIONS[version];
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        version += 1;
        tx.pragma_update(None, "user_version", version as i64)?;
        tx.commit()?;
    }
    Ok(())
}

const V1: &str = r#"
CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT    NOT NULL,
    tracked     INTEGER NOT NULL DEFAULT 0,   -- display flag: show the time graph
    archived    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL
);

CREATE TABLE tasks (
    id              TEXT PRIMARY KEY,
    project_id      TEXT REFERENCES projects(id) ON DELETE SET NULL,
    parent_task_id  TEXT REFERENCES tasks(id)    ON DELETE CASCADE,
    title           TEXT    NOT NULL,
    notes           TEXT    NOT NULL DEFAULT '',
    deadline        TEXT,
    tracked         INTEGER NOT NULL DEFAULT 0,   -- display flag only
    status          TEXT    NOT NULL DEFAULT 'todo',
    completed_at    TEXT,
    sort_order      REAL    NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL
);
CREATE INDEX idx_tasks_parent  ON tasks(parent_task_id);
CREATE INDEX idx_tasks_project ON tasks(project_id);

CREATE TABLE time_entries (
    id          TEXT PRIMARY KEY,
    task_id     TEXT    NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    start_ts    TEXT    NOT NULL,
    end_ts      TEXT,                            -- NULL = currently running
    source      TEXT    NOT NULL DEFAULT 'timer',-- 'timer' | 'manual'
    note        TEXT    NOT NULL DEFAULT '',
    created_at  TEXT    NOT NULL
);
CREATE INDEX idx_time_entries_task ON time_entries(task_id);

-- At most one running timer (end_ts IS NULL) across the whole app. A partial
-- UNIQUE index can't express this (SQLite treats NULLs as distinct and rejects
-- constant index expressions), so enforce it with triggers.
CREATE TRIGGER trg_one_running_timer_insert
BEFORE INSERT ON time_entries
WHEN NEW.end_ts IS NULL
BEGIN
    SELECT RAISE(ABORT, 'a timer is already running')
    WHERE EXISTS (SELECT 1 FROM time_entries WHERE end_ts IS NULL);
END;

CREATE TRIGGER trg_one_running_timer_update
BEFORE UPDATE ON time_entries
WHEN NEW.end_ts IS NULL AND OLD.end_ts IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'a timer is already running')
    WHERE EXISTS (SELECT 1 FROM time_entries WHERE end_ts IS NULL AND id <> NEW.id);
END;
"#;

// A tiny key/value store for app-level state that isn't domain data. Currently
// holds only `timer_heartbeat`: the last local time the running timer was seen
// alive, used on startup to close an entry a crash left open.
const V2: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

// The "Recent actions" revert log. `id` orders entries (newest = highest) and
// is what "reset from here" ranges over; `payload` is kind-specific JSON with
// whatever before-state a revert needs (see `db::actions`).
const V3: &str = r#"
CREATE TABLE actions (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL,
    summary     TEXT NOT NULL,
    payload     TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
"#;

// A task created through the "quick creation" bulk text import can carry a
// one-off starting duration ("I've already put 3h into this"). It is display
// only - never summed into time totals or the heatmap, never backed by a
// time_entries row - so a plain nullable column is all it needs.
const V4: &str = r#"
ALTER TABLE tasks ADD COLUMN initial_time_seconds INTEGER;
"#;
