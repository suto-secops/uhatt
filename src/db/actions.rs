//! The "Recent actions" revert log.
//!
//! Every user-triggered task mutation writes one row here right after it
//! succeeds, with enough before-state (as JSON) to reverse it. `id` is a
//! plain SQLite rowid, so it orders entries newest-last and "reset from
//! here" is just "every row with `id >= this one`, newest first".
//!
//! Reverting never itself writes a new row - it applies the inverse, then
//! deletes the row(s) it just undid, both inside one transaction so a
//! mid-revert failure leaves neither the data nor the log half-changed.
//!
//! Time entries and the timer are deliberately not logged here: they carry
//! their own invariant (at most one open entry across the whole app), and
//! folding that into a generic revert log is a separate piece of work.

use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::{ENTRY_COLUMNS, NOW, TASK_COLUMNS};
use crate::domain::{Task, TaskStatus};

/// One row of the log, as shown to the user.
pub struct ActionRow {
    pub id: i64,
    pub summary: String,
    pub created_at: String,
    /// The task's current project name, or "Tasks w/o project" when it has
    /// none - or, for a delete, no longer exists to ask.
    pub project_label: String,
}

const NO_PROJECT_LABEL: &str = "Tasks w/o project";

/// The task id an action's payload is about, per its own shape (see the
/// `log_*` functions below) - `None` for a shape that doesn't carry one.
fn task_id_of(kind: &str, value: &serde_json::Value) -> Option<String> {
    let id = match kind {
        "delete_task" => value.get("tasks")?.get(0)?.get("id")?,
        _ => value.get("id")?,
    };
    id.as_str().map(str::to_owned)
}

/// The project label for one action row: the task's *current* project if it
/// still exists, else (for a delete) the project it was in when deleted,
/// else [`NO_PROJECT_LABEL`].
fn project_label_of(conn: &Connection, kind: &str, payload: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
    let project_id =
        match task_id_of(kind, &value).and_then(|id| super::get_task(conn, &id).ok()?) {
            Some(task) => task.project_id,
            None if kind == "delete_task" => value
                .get("tasks")
                .and_then(|t| t.get(0))
                .and_then(|t| t.get("project_id"))
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            None => None,
        };
    match project_id.and_then(|id| super::get_project(conn, &id).ok()?) {
        Some(p) => p.name,
        None => NO_PROJECT_LABEL.to_owned(),
    }
}

/// Newest first.
pub fn list_actions(conn: &Connection) -> rusqlite::Result<Vec<ActionRow>> {
    let mut stmt = conn
        .prepare("SELECT id, kind, summary, payload, created_at FROM actions ORDER BY id DESC")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, kind, summary, payload, created_at) = row?;
        let project_label = project_label_of(conn, &kind, &payload);
        out.push(ActionRow {
            id,
            summary,
            created_at,
            project_label,
        });
    }
    Ok(out)
}

fn insert_action(
    conn: &Connection,
    kind: &str,
    summary: &str,
    payload: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO actions (kind, summary, payload, created_at) VALUES (?1, ?2, ?3, {NOW})"
        ),
        params![kind, summary, payload],
    )?;
    Ok(())
}

// --- Per-task before-state, captured by the caller before it mutates -------

/// A task's project id together with every descendant's, keyed by task id -
/// what [`super::set_task_project`] and [`super::reparent_task`] (which also
/// moves the subtree's project) are about to overwrite.
pub fn capture_project_ids(
    conn: &Connection,
    task_id: &str,
) -> rusqlite::Result<BTreeMap<String, Option<String>>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE subtree(id) AS (
             SELECT ?1
           UNION ALL
             SELECT t.id FROM tasks t JOIN subtree s ON t.parent_task_id = s.id
         )
         SELECT id, (SELECT project_id FROM tasks WHERE tasks.id = subtree.id) FROM subtree",
    )?;
    let rows = stmt.query_map(params![task_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

// --- Logging, one function per kind of mutation -----------------------------

pub fn log_create_task(conn: &Connection, task: &Task) -> rusqlite::Result<()> {
    let payload = serde_json::json!({ "id": task.id }).to_string();
    let summary = format!("Created task \"{}\"", task.title);
    insert_action(conn, "create_task", &summary, &payload)
}

pub fn log_rename_task(
    conn: &Connection,
    task_id: &str,
    old_title: &str,
    new_title: &str,
) -> rusqlite::Result<()> {
    let payload = serde_json::json!({ "id": task_id, "old_title": old_title }).to_string();
    let summary = format!("Renamed \"{old_title}\" to \"{new_title}\"");
    insert_action(conn, "rename_task", &summary, &payload)
}

pub fn log_set_notes(
    conn: &Connection,
    task_id: &str,
    title: &str,
    old_notes: &str,
) -> rusqlite::Result<()> {
    let payload = serde_json::json!({ "id": task_id, "old_notes": old_notes }).to_string();
    let summary = format!("Edited notes on \"{title}\"");
    insert_action(conn, "set_notes", &summary, &payload)
}

pub fn log_set_deadline(
    conn: &Connection,
    task_id: &str,
    title: &str,
    old_deadline: Option<&str>,
    new_deadline: Option<&str>,
) -> rusqlite::Result<()> {
    let payload = serde_json::json!({ "id": task_id, "old_deadline": old_deadline }).to_string();
    let summary = match new_deadline {
        Some(d) => format!("Set deadline on \"{title}\" to {d}"),
        None => format!("Cleared deadline on \"{title}\""),
    };
    insert_action(conn, "set_deadline", &summary, &payload)
}

pub fn log_set_status(
    conn: &Connection,
    task_id: &str,
    title: &str,
    old_status: TaskStatus,
    done: bool,
) -> rusqlite::Result<()> {
    let payload =
        serde_json::json!({ "id": task_id, "old_status": old_status.as_str() }).to_string();
    let summary = if done {
        format!("Marked \"{title}\" done")
    } else {
        format!("Marked \"{title}\" not done")
    };
    insert_action(conn, "set_status", &summary, &payload)
}

pub fn log_set_project(
    conn: &Connection,
    task_id: &str,
    title: &str,
    old_project_ids: &BTreeMap<String, Option<String>>,
    new_project_name: &str,
) -> rusqlite::Result<()> {
    let payload =
        serde_json::json!({ "id": task_id, "old_project_ids": old_project_ids }).to_string();
    let summary = if new_project_name.is_empty() {
        format!("Moved \"{title}\" to Unfiled")
    } else {
        format!("Moved \"{title}\" to project \"{new_project_name}\"")
    };
    insert_action(conn, "set_project", &summary, &payload)
}

#[derive(Serialize, Deserialize)]
struct ReparentPayload {
    id: String,
    old_parent_id: Option<String>,
    old_sort_order: f64,
    old_project_ids: BTreeMap<String, Option<String>>,
}

#[allow(clippy::too_many_arguments)]
pub fn log_reparent(
    conn: &Connection,
    task_id: &str,
    title: &str,
    old_parent_id: Option<&str>,
    old_sort_order: f64,
    old_project_ids: &BTreeMap<String, Option<String>>,
    new_parent_title: &str,
) -> rusqlite::Result<()> {
    let payload = ReparentPayload {
        id: task_id.to_owned(),
        old_parent_id: old_parent_id.map(str::to_owned),
        old_sort_order,
        old_project_ids: old_project_ids.clone(),
    };
    let payload = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned());
    let summary = format!("Moved \"{title}\" under \"{new_parent_title}\"");
    insert_action(conn, "reparent", &summary, &payload)
}

/// A task row, exactly as stored, for a delete's subtree snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskSnapshot {
    id: String,
    parent_task_id: Option<String>,
    project_id: Option<String>,
    title: String,
    notes: String,
    deadline: Option<String>,
    tracked: bool,
    status: String,
    sort_order: f64,
    created_at: String,
    initial_time_seconds: Option<i64>,
    periodicity: Option<String>,
    completed_at: Option<String>,
}

// `SELECT {TASK_COLUMNS}, completed_at` - TASK_COLUMNS' own order (see
// `super::TASK_COLUMNS`) with completed_at tacked on at the end.
fn row_to_task_snapshot(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskSnapshot> {
    Ok(TaskSnapshot {
        id: r.get(0)?,
        parent_task_id: r.get(1)?,
        project_id: r.get(2)?,
        title: r.get(3)?,
        notes: r.get(4)?,
        deadline: r.get(5)?,
        tracked: r.get::<_, i64>(6)? != 0,
        status: r.get(7)?,
        sort_order: r.get(8)?,
        created_at: r.get(9)?,
        initial_time_seconds: r.get(10)?,
        periodicity: r.get(11)?,
        completed_at: r.get(12)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EntrySnapshot {
    id: String,
    task_id: String,
    start_ts: String,
    end_ts: Option<String>,
    source: String,
    note: String,
    created_at: String,
}

fn row_to_entry_snapshot(r: &rusqlite::Row<'_>) -> rusqlite::Result<EntrySnapshot> {
    Ok(EntrySnapshot {
        id: r.get(0)?,
        task_id: r.get(1)?,
        start_ts: r.get(2)?,
        end_ts: r.get(3)?,
        source: r.get(4)?,
        note: r.get(5)?,
        created_at: r.get(6)?,
    })
}

/// Snapshot `task_id` and its whole subtree (pre-order, so a parent always
/// precedes its children) plus every time entry against any of them - the
/// state [`super::delete_task`] is about to cascade away.
fn subtree_snapshot(
    conn: &Connection,
    task_id: &str,
) -> rusqlite::Result<(Vec<TaskSnapshot>, Vec<EntrySnapshot>)> {
    let sql = format!(
        "WITH RECURSIVE subtree(task_id, sort_path) AS (
             SELECT ?1, printf('%020.6f', (SELECT sort_order FROM tasks WHERE id = ?1))
           UNION ALL
             SELECT t.id, s.sort_path || '/' || printf('%020.6f', t.sort_order)
             FROM tasks t JOIN subtree s ON t.parent_task_id = s.task_id
         )
         SELECT {TASK_COLUMNS}, completed_at FROM tasks JOIN subtree ON tasks.id = subtree.task_id
         ORDER BY subtree.sort_path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let tasks: Vec<TaskSnapshot> = stmt
        .query_map(params![task_id], row_to_task_snapshot)?
        .collect::<rusqlite::Result<_>>()?;

    let entries = if tasks.is_empty() {
        Vec::new()
    } else {
        let placeholders = tasks.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql =
            format!("SELECT {ENTRY_COLUMNS} FROM time_entries WHERE task_id IN ({placeholders})");
        let mut stmt = conn.prepare(&sql)?;
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        let rows: Vec<EntrySnapshot> = stmt
            .query_map(rusqlite::params_from_iter(ids), row_to_entry_snapshot)?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };
    Ok((tasks, entries))
}

/// Snapshot `task_id`'s subtree and log the delete about to happen to it.
/// Call this *before* [`super::delete_task`] - the cascade destroys exactly
/// what this captures. A no-op (nothing logged) if `task_id` doesn't exist.
pub fn capture_and_log_delete(conn: &Connection, task_id: &str) -> rusqlite::Result<()> {
    let (tasks, entries) = subtree_snapshot(conn, task_id)?;
    let Some(root) = tasks.first() else {
        return Ok(());
    };
    let summary = if tasks.len() > 1 {
        format!(
            "Deleted task \"{}\" and {} more",
            root.title,
            tasks.len() - 1
        )
    } else {
        format!("Deleted task \"{}\"", root.title)
    };
    let payload = serde_json::json!({ "tasks": tasks, "entries": entries }).to_string();
    insert_action(conn, "delete_task", &summary, &payload)
}

/// What [`import_quick_creation`] created.
pub struct ImportOutcome {
    pub project_id: String,
    pub task_count: usize,
}

/// Run a whole quick-creation import atomically: create the project, then
/// each task in `tasks`' order (already depth-validated by
/// [`crate::domain::parse_quick_creation`]) - a task's parent is the most
/// recently created task at `depth - 1`, the same stack the parser itself
/// used to assign depths - then log the whole batch as one undoable action.
/// All-or-nothing: a failure partway rolls back the whole import.
pub fn import_quick_creation(
    conn: &Connection,
    project_name: &str,
    tasks: &[crate::domain::QuickCreateTask],
) -> rusqlite::Result<ImportOutcome> {
    let tx = conn.unchecked_transaction()?;
    let project = super::create_project(&tx, project_name)?;

    let mut stack: Vec<String> = Vec::new();
    let mut created_ids: Vec<String> = Vec::new();
    for t in tasks {
        let parent_id = if t.depth == 0 {
            None
        } else {
            stack.get(t.depth - 1).map(String::as_str)
        };
        let root_project = (t.depth == 0).then_some(project.id.as_str());
        let task = super::create_task(&tx, &t.title, parent_id, root_project)?;
        if let Some(d) = &t.deadline {
            super::set_task_deadline(&tx, &task.id, Some(d.as_str()))?;
        }
        if let Some(secs) = t.initial_time_seconds {
            super::set_initial_time(&tx, &task.id, Some(secs))?;
        }
        if !t.description.is_empty() {
            super::set_task_notes(&tx, &task.id, &t.description)?;
        }
        stack.truncate(t.depth);
        stack.push(task.id.clone());
        created_ids.push(task.id);
    }

    let summary = format!(
        "Created project \"{project_name}\" with {} task{}",
        created_ids.len(),
        if created_ids.len() == 1 { "" } else { "s" }
    );
    let payload = serde_json::json!({
        "project_id": project.id,
        "task_ids": created_ids,
    })
    .to_string();
    insert_action(&tx, "import_project", &summary, &payload)?;

    let outcome = ImportOutcome {
        project_id: project.id,
        task_count: created_ids.len(),
    };
    tx.commit()?;
    Ok(outcome)
}

// --- Revert ------------------------------------------------------------

fn exists(conn: &Connection, table: &str, id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
        params![id],
        |r| r.get::<_, i64>(0).map(|n| n != 0),
    )
}

fn apply_one(conn: &Connection, action_id: i64) -> rusqlite::Result<()> {
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT kind, payload FROM actions WHERE id = ?1",
            params![action_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((kind, payload)) = found else {
        return Ok(());
    };
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();

    match kind.as_str() {
        "create_task" => {
            if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
                super::delete_task(conn, id)?;
            }
        }
        "rename_task" => {
            if let (Some(id), Some(old)) = (
                value.get("id").and_then(|v| v.as_str()),
                value.get("old_title").and_then(|v| v.as_str()),
            ) {
                super::rename_task(conn, id, old)?;
            }
        }
        "set_notes" => {
            if let (Some(id), Some(old)) = (
                value.get("id").and_then(|v| v.as_str()),
                value.get("old_notes").and_then(|v| v.as_str()),
            ) {
                super::set_task_notes(conn, id, old)?;
            }
        }
        "set_deadline" => {
            if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
                let old = value.get("old_deadline").and_then(|v| v.as_str());
                super::set_task_deadline(conn, id, old)?;
            }
        }
        "set_status" => {
            if let (Some(id), Some(old)) = (
                value.get("id").and_then(|v| v.as_str()),
                value.get("old_status").and_then(|v| v.as_str()),
            ) {
                super::set_task_status(conn, id, TaskStatus::from_db(old))?;
            }
        }
        "set_project" => {
            let map: BTreeMap<String, Option<String>> = value
                .get("old_project_ids")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            for (id, project_id) in &map {
                conn.execute(
                    "UPDATE tasks SET project_id = ?2 WHERE id = ?1",
                    params![id, project_id],
                )?;
            }
        }
        "reparent" => {
            let p: ReparentPayload = match serde_json::from_value(value) {
                Ok(p) => p,
                Err(_) => return Ok(()),
            };
            let parent = match &p.old_parent_id {
                Some(pid) if exists(conn, "tasks", pid)? => Some(pid.as_str()),
                _ => None,
            };
            conn.execute(
                "UPDATE tasks SET parent_task_id = ?2, sort_order = ?3 WHERE id = ?1",
                params![p.id, parent, p.old_sort_order],
            )?;
            for (id, project_id) in &p.old_project_ids {
                conn.execute(
                    "UPDATE tasks SET project_id = ?2 WHERE id = ?1",
                    params![id, project_id],
                )?;
            }
        }
        "delete_task" => {
            #[derive(Deserialize)]
            struct Payload {
                tasks: Vec<TaskSnapshot>,
                entries: Vec<EntrySnapshot>,
            }
            let Ok(p) = serde_json::from_value::<Payload>(value) else {
                return Ok(());
            };
            for t in &p.tasks {
                let parent = match &t.parent_task_id {
                    Some(pid) if exists(conn, "tasks", pid)? => Some(pid.as_str()),
                    Some(_) => None,
                    None => None,
                };
                let project = match &t.project_id {
                    Some(pjid) if exists(conn, "projects", pjid)? => Some(pjid.as_str()),
                    Some(_) => None,
                    None => None,
                };
                conn.execute(
                    "INSERT INTO tasks (id, parent_task_id, project_id, title, notes, deadline,
                                         tracked, status, completed_at, sort_order, created_at,
                                         initial_time_seconds, periodicity)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        t.id,
                        parent,
                        project,
                        t.title,
                        t.notes,
                        t.deadline,
                        t.tracked as i64,
                        t.status,
                        t.completed_at,
                        t.sort_order,
                        t.created_at,
                        t.initial_time_seconds,
                        t.periodicity,
                    ],
                )?;
            }
            for e in &p.entries {
                conn.execute(
                    "INSERT INTO time_entries (id, task_id, start_ts, end_ts, source, note, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![e.id, e.task_id, e.start_ts, e.end_ts, e.source, e.note, e.created_at],
                )?;
            }
        }
        "import_project" => {
            #[derive(Deserialize)]
            struct Payload {
                project_id: String,
                task_ids: Vec<String>,
            }
            let Ok(p) = serde_json::from_value::<Payload>(value) else {
                return Ok(());
            };
            // `project_id` is `ON DELETE SET NULL` on tasks, not CASCADE, so
            // the tasks must be deleted explicitly too - deleting a root
            // first cascades its children, making their own deletes no-ops.
            for id in &p.task_ids {
                super::delete_task(conn, id)?;
            }
            super::delete_project(conn, &p.project_id)?;
        }
        _ => {}
    }
    Ok(())
}

/// Undo one action, then remove it from the log. Atomic: a failure partway
/// through leaves both the data and the log untouched.
pub fn revert_action(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    apply_one(&tx, id)?;
    tx.execute("DELETE FROM actions WHERE id = ?1", params![id])?;
    tx.commit()
}

/// Undo `id` and every action newer than it, newest first, then remove all of
/// them from the log. One transaction for the whole range.
pub fn revert_from(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    let ids: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM actions WHERE id >= ?1 ORDER BY id DESC")?;
        let rows: Vec<i64> = stmt
            .query_map(params![id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };
    for action_id in &ids {
        apply_one(&tx, *action_id)?;
    }
    tx.execute("DELETE FROM actions WHERE id >= ?1", params![id])?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Periodicity;

    fn root(conn: &Connection, title: &str) -> Task {
        super::super::create_task(conn, title, None, None).unwrap()
    }

    fn child(conn: &Connection, title: &str, parent: &str) -> Task {
        super::super::create_task(conn, title, Some(parent), None).unwrap()
    }

    fn only_action_id(conn: &Connection) -> i64 {
        let ids = list_actions(conn).unwrap();
        assert_eq!(ids.len(), 1, "expected exactly one logged action");
        ids[0].id
    }

    #[test]
    fn project_label_reflects_current_project_or_unfiled() {
        let conn = super::super::open_in_memory().unwrap();
        let p = super::super::create_project(&conn, "Work").unwrap();

        let in_project =
            super::super::create_task(&conn, "quarterly report", None, Some(&p.id)).unwrap();
        log_create_task(&conn, &in_project).unwrap();

        let no_project = root(&conn, "buy groceries");
        log_create_task(&conn, &no_project).unwrap();

        let rows = list_actions(&conn).unwrap();
        let in_project_row = rows
            .iter()
            .find(|r| r.summary.contains("quarterly"))
            .unwrap();
        let no_project_row = rows
            .iter()
            .find(|r| r.summary.contains("groceries"))
            .unwrap();
        assert_eq!(in_project_row.project_label, "Work");
        assert_eq!(no_project_row.project_label, NO_PROJECT_LABEL);
    }

    #[test]
    fn project_label_falls_back_to_snapshot_for_a_deleted_task() {
        let conn = super::super::open_in_memory().unwrap();
        let p = super::super::create_project(&conn, "Work").unwrap();
        let t = super::super::create_task(&conn, "gone", None, Some(&p.id)).unwrap();

        capture_and_log_delete(&conn, &t.id).unwrap();
        super::super::delete_task(&conn, &t.id).unwrap();

        let rows = list_actions(&conn).unwrap();
        assert_eq!(rows[0].project_label, "Work");
    }

    #[test]
    fn create_task_reverts_to_no_task() {
        let conn = super::super::open_in_memory().unwrap();
        let t = root(&conn, "write plan");
        log_create_task(&conn, &t).unwrap();

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        assert!(super::super::get_task(&conn, &t.id).unwrap().is_none());
        assert!(list_actions(&conn).unwrap().is_empty());
    }

    #[test]
    fn rename_reverts_title() {
        let conn = super::super::open_in_memory().unwrap();
        let t = root(&conn, "old title");
        super::super::rename_task(&conn, &t.id, "new title").unwrap();
        log_rename_task(&conn, &t.id, "old title", "new title").unwrap();

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        assert_eq!(
            super::super::get_task(&conn, &t.id).unwrap().unwrap().title,
            "old title"
        );
    }

    #[test]
    fn set_notes_reverts_notes() {
        let conn = super::super::open_in_memory().unwrap();
        let t = root(&conn, "task");
        super::super::set_task_notes(&conn, &t.id, "new notes").unwrap();
        log_set_notes(&conn, &t.id, &t.title, "").unwrap();

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        assert_eq!(
            super::super::get_task(&conn, &t.id).unwrap().unwrap().notes,
            ""
        );
    }

    #[test]
    fn set_deadline_reverts_to_previous_deadline() {
        let conn = super::super::open_in_memory().unwrap();
        let t = root(&conn, "task");
        super::super::set_task_deadline(&conn, &t.id, Some("2026-01-01")).unwrap();
        log_set_deadline(&conn, &t.id, &t.title, None, Some("2026-01-01")).unwrap();

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        assert_eq!(
            super::super::get_task(&conn, &t.id)
                .unwrap()
                .unwrap()
                .deadline,
            None
        );
    }

    #[test]
    fn set_status_revert_clears_completed_at() {
        let conn = super::super::open_in_memory().unwrap();
        let t = root(&conn, "task");
        super::super::set_task_status(&conn, &t.id, TaskStatus::Done).unwrap();
        log_set_status(&conn, &t.id, &t.title, TaskStatus::Todo, true).unwrap();

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        let after = super::super::get_task(&conn, &t.id).unwrap().unwrap();
        assert_eq!(after.status, TaskStatus::Todo);
    }

    #[test]
    fn set_project_reverts_whole_subtree() {
        let conn = super::super::open_in_memory().unwrap();
        let p = super::super::create_project(&conn, "Work").unwrap();
        let parent = root(&conn, "parent");
        let kid = child(&conn, "kid", &parent.id);

        let old_map = capture_project_ids(&conn, &parent.id).unwrap();
        super::super::set_task_project(&conn, &parent.id, Some(&p.id)).unwrap();
        log_set_project(&conn, &parent.id, &parent.title, &old_map, "Work").unwrap();

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        assert_eq!(
            super::super::get_task(&conn, &parent.id)
                .unwrap()
                .unwrap()
                .project_id,
            None
        );
        assert_eq!(
            super::super::get_task(&conn, &kid.id)
                .unwrap()
                .unwrap()
                .project_id,
            None
        );
    }

    #[test]
    fn reparent_revert_restores_parent_and_project() {
        let conn = super::super::open_in_memory().unwrap();
        let a = root(&conn, "a");
        let b = root(&conn, "b");
        let c = child(&conn, "c", &a.id);

        let before = super::super::get_task(&conn, &c.id).unwrap().unwrap();
        let old_map = capture_project_ids(&conn, &c.id).unwrap();
        super::super::reparent_task(&conn, &c.id, &b.id).unwrap();
        log_reparent(
            &conn,
            &c.id,
            &before.title,
            before.parent_id.as_deref(),
            before.sort_order,
            &old_map,
            "b",
        )
        .unwrap();

        assert_eq!(
            super::super::get_task(&conn, &c.id)
                .unwrap()
                .unwrap()
                .parent_id,
            Some(b.id.clone())
        );

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        let after = super::super::get_task(&conn, &c.id).unwrap().unwrap();
        assert_eq!(after.parent_id, Some(a.id.clone()));
    }

    #[test]
    fn reparent_revert_falls_back_to_root_when_old_parent_gone() {
        let conn = super::super::open_in_memory().unwrap();
        let r1 = root(&conn, "r1");
        let r2 = root(&conn, "r2");
        let c = child(&conn, "c", &r1.id);

        let before = super::super::get_task(&conn, &c.id).unwrap().unwrap();
        let old_map = capture_project_ids(&conn, &c.id).unwrap();
        super::super::reparent_task(&conn, &c.id, &r2.id).unwrap();
        log_reparent(
            &conn,
            &c.id,
            &before.title,
            before.parent_id.as_deref(),
            before.sort_order,
            &old_map,
            "r2",
        )
        .unwrap();

        // r1 (c's original parent) is gone by the time we revert.
        super::super::delete_task(&conn, &r1.id).unwrap();

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        let after = super::super::get_task(&conn, &c.id).unwrap().unwrap();
        assert_eq!(after.parent_id, None, "should fall back to root, not error");
    }

    #[test]
    fn delete_revert_restores_subtree_and_time_entries() {
        let conn = super::super::open_in_memory().unwrap();
        let parent = root(&conn, "parent");
        let kid = child(&conn, "kid", &parent.id);
        super::super::add_manual_entry(
            &conn,
            &kid.id,
            "2026-01-01T09:00",
            "2026-01-01T10:00",
            "worked",
        )
        .unwrap();
        let weekly = Periodicity::EveryWeeks { n: 1 };
        super::super::set_task_periodicity(&conn, &parent.id, Some(&weekly)).unwrap();

        capture_and_log_delete(&conn, &parent.id).unwrap();
        super::super::delete_task(&conn, &parent.id).unwrap();

        assert!(super::super::get_task(&conn, &parent.id).unwrap().is_none());
        assert!(super::super::get_task(&conn, &kid.id).unwrap().is_none());

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        let restored_parent = super::super::get_task(&conn, &parent.id).unwrap().unwrap();
        assert_eq!(restored_parent.title, "parent");
        assert_eq!(
            restored_parent.periodicity.as_deref(),
            Some(weekly.to_stored().as_str())
        );
        let restored_kid = super::super::get_task(&conn, &kid.id).unwrap().unwrap();
        assert_eq!(restored_kid.parent_id, Some(parent.id.clone()));
        let entries = super::super::list_entries_for_task(&conn, &kid.id).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seconds, 3600);
    }

    #[test]
    fn delete_revert_falls_back_to_root_when_original_parent_gone() {
        let conn = super::super::open_in_memory().unwrap();
        let parent = root(&conn, "parent");
        let kid = child(&conn, "kid", &parent.id);

        capture_and_log_delete(&conn, &kid.id).unwrap();
        super::super::delete_task(&conn, &kid.id).unwrap();
        // Now the parent is gone too, before the kid's delete is reverted.
        super::super::delete_task(&conn, &parent.id).unwrap();

        let id = only_action_id(&conn);
        revert_action(&conn, id).unwrap();

        let restored_kid = super::super::get_task(&conn, &kid.id).unwrap().unwrap();
        assert_eq!(
            restored_kid.parent_id, None,
            "should fall back to root, not error"
        );
    }

    #[test]
    fn reset_from_here_undoes_a_whole_range_newest_first() {
        let conn = super::super::open_in_memory().unwrap();
        let t = root(&conn, "v1");

        super::super::rename_task(&conn, &t.id, "v2").unwrap();
        log_rename_task(&conn, &t.id, "v1", "v2").unwrap();
        let first_id = list_actions(&conn).unwrap()[0].id;

        super::super::rename_task(&conn, &t.id, "v3").unwrap();
        log_rename_task(&conn, &t.id, "v2", "v3").unwrap();

        super::super::set_task_status(&conn, &t.id, TaskStatus::Done).unwrap();
        log_set_status(&conn, &t.id, "v3", TaskStatus::Todo, true).unwrap();

        assert_eq!(list_actions(&conn).unwrap().len(), 3);

        revert_from(&conn, first_id).unwrap();

        let after = super::super::get_task(&conn, &t.id).unwrap().unwrap();
        assert_eq!(after.title, "v1");
        assert_eq!(after.status, TaskStatus::Todo);
        assert!(list_actions(&conn).unwrap().is_empty());
    }

    #[test]
    fn reset_single_action_leaves_newer_ones_in_place() {
        let conn = super::super::open_in_memory().unwrap();
        let t = root(&conn, "v1");

        super::super::rename_task(&conn, &t.id, "v2").unwrap();
        log_rename_task(&conn, &t.id, "v1", "v2").unwrap();
        let first_id = list_actions(&conn).unwrap()[0].id;

        super::super::rename_task(&conn, &t.id, "v3").unwrap();
        log_rename_task(&conn, &t.id, "v2", "v3").unwrap();

        // "Reset" only undoes the one action picked - it restores exactly
        // what that action's own before-state was ("v1"), independent of
        // the still-standing newer rename's log entry.
        revert_action(&conn, first_id).unwrap();

        let after = super::super::get_task(&conn, &t.id).unwrap().unwrap();
        assert_eq!(after.title, "v1");
        assert_eq!(list_actions(&conn).unwrap().len(), 1);
    }
}
