//! SQLite persistence and the task repository.
//!
//! Everything here is plain Rust with no Qt types, so it is exercised directly
//! by `cargo test` against an in-memory database. The Qt bridge
//! (`crate::tasks_model`) is a thin adapter on top.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::{new_id, Project, ProjectFilter, Task, TaskStatus};

mod migrations;

/// Failure opening a database: either creating its directory or SQLite itself.
#[derive(Debug)]
pub enum OpenError {
    Io(std::io::Error),
    Db(rusqlite::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Io(e) => write!(f, "database directory: {e}"),
            OpenError::Db(e) => write!(f, "sqlite: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<std::io::Error> for OpenError {
    fn from(e: std::io::Error) -> Self {
        OpenError::Io(e)
    }
}

impl From<rusqlite::Error> for OpenError {
    fn from(e: rusqlite::Error) -> Self {
        OpenError::Db(e)
    }
}

/// `$XDG_DATA_HOME/uhatt/uhatt.db`, falling back to `~/.local/share/uhatt/uhatt.db`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(".local").join("share")
        });
    base.join("uhatt").join("uhatt.db")
}

/// Open (creating file and parent directory if needed) and migrate the schema.
pub fn open(path: &Path) -> Result<Connection, OpenError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    migrations::run(&mut conn)?;
    Ok(conn)
}

/// In-memory database with the schema applied - for tests and as a fallback.
pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrations::run(&mut conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
}

const TASK_COLUMNS: &str = "id, parent_task_id, project_id, title, notes, \
                            deadline, tracked, status, sort_order, created_at";

fn row_to_task(r: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        parent_id: r.get(1)?,
        project_id: r.get(2)?,
        title: r.get(3)?,
        notes: r.get(4)?,
        deadline: r.get(5)?,
        tracked: r.get::<_, i64>(6)? != 0,
        status: TaskStatus::from_db(&r.get::<_, String>(7)?),
        sort_order: r.get(8)?,
        created_at: r.get(9)?,
    })
}

/// Insert a task and return it. `title` is trimmed; the caller rejects blanks.
///
/// A subtask always inherits its parent's project, so `project_id` is only
/// consulted for root tasks (`parent_id == None`).
pub fn create_task(
    conn: &Connection,
    title: &str,
    parent_id: Option<&str>,
    project_id: Option<&str>,
) -> rusqlite::Result<Task> {
    let id = new_id();
    let project_id: Option<String> = match parent_id {
        Some(parent) => conn.query_row(
            "SELECT project_id FROM tasks WHERE id = ?1",
            params![parent],
            |r| r.get(0),
        )?,
        None => project_id.map(str::to_owned),
    };
    // Place the new task after its current last sibling.
    let sort_order: f64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), 0) + 1.0 FROM tasks WHERE parent_task_id IS ?1",
        params![parent_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO tasks (id, parent_task_id, project_id, title, sort_order, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        params![id, parent_id, project_id, title.trim(), sort_order],
    )?;
    Ok(get_task(conn, &id)?.expect("row just inserted"))
}

/// Fetch one task by id.
pub fn get_task(conn: &Connection, id: &str) -> rusqlite::Result<Option<Task>> {
    conn.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
        params![id],
        row_to_task,
    )
    .optional()
}

/// A task plus its nesting depth and whether it has any subtasks, in tree
/// (pre-order) display order. Siblings are ordered by `sort_order`.
#[derive(Debug, Clone)]
pub struct TaskNode {
    pub task: Task,
    pub depth: u32,
    pub has_children: bool,
}

/// All tasks matching `filter` as a depth-annotated, pre-ordered list for the
/// flattened tree view.
///
/// The filter is applied only to root tasks: subtasks inherit their parent's
/// project, so a whole subtree always belongs to one project.
pub fn list_task_tree(
    conn: &Connection,
    filter: &ProjectFilter,
) -> rusqlite::Result<Vec<TaskNode>> {
    let (root_predicate, bind): (&str, Option<&str>) = match filter {
        ProjectFilter::All => ("1", None),
        ProjectFilter::Unfiled => ("project_id IS NULL", None),
        ProjectFilter::Only(id) => ("project_id = ?1", Some(id.as_str())),
    };
    // `sort_path` concatenates each ancestor's zero-padded sort_order so that
    // ordering the flat result by it yields a correct pre-order traversal.
    let sql = format!(
        "WITH RECURSIVE subtree(task_id, depth, sort_path) AS (
             SELECT id, 0, printf('%020.6f', sort_order)
             FROM tasks WHERE parent_task_id IS NULL AND {root_predicate}
           UNION ALL
             SELECT t.id, s.depth + 1,
                    s.sort_path || '/' || printf('%020.6f', t.sort_order)
             FROM tasks t JOIN subtree s ON t.parent_task_id = s.task_id
         )
         SELECT {TASK_COLUMNS},
                subtree.depth,
                EXISTS(SELECT 1 FROM tasks c WHERE c.parent_task_id = tasks.id)
         FROM tasks JOIN subtree ON tasks.id = subtree.task_id
         ORDER BY subtree.sort_path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<TaskNode> {
        Ok(TaskNode {
            task: row_to_task(r)?,
            depth: r.get::<_, i64>(10)? as u32,
            has_children: r.get::<_, i64>(11)? != 0,
        })
    };
    let rows = match bind {
        Some(id) => stmt.query_map(params![id], map_row)?,
        None => stmt.query_map([], map_row)?,
    };
    rows.collect()
}

/// Delete a task; subtasks cascade.
pub fn delete_task(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
    Ok(())
}

/// Change a task's title. `title` is trimmed.
pub fn rename_task(conn: &Connection, id: &str, title: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tasks SET title = ?2 WHERE id = ?1",
        params![id, title.trim()],
    )?;
    Ok(())
}

/// Set a task's status, stamping or clearing `completed_at` to match.
pub fn set_task_status(conn: &Connection, id: &str, status: TaskStatus) -> rusqlite::Result<()> {
    let done = matches!(status, TaskStatus::Done);
    conn.execute(
        "UPDATE tasks
         SET status = ?2,
             completed_at = CASE WHEN ?3
                 THEN strftime('%Y-%m-%dT%H:%M:%SZ', 'now') ELSE NULL END
         WHERE id = ?1",
        params![id, status.as_str(), done],
    )?;
    Ok(())
}

/// Move a task - and its whole subtree, so a subtree never spans projects -
/// to `project_id` (or to unfiled when `None`).
pub fn set_task_project(
    conn: &Connection,
    task_id: &str,
    project_id: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "WITH RECURSIVE subtree(id) AS (
             SELECT ?1
           UNION ALL
             SELECT t.id FROM tasks t JOIN subtree s ON t.parent_task_id = s.id
         )
         UPDATE tasks SET project_id = ?2 WHERE id IN (SELECT id FROM subtree)",
        params![task_id, project_id],
    )?;
    Ok(())
}

// --- Projects -------------------------------------------------------------

const PROJECT_COLUMNS: &str = "id, name, tracked, archived, created_at";

fn row_to_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        tracked: r.get::<_, i64>(2)? != 0,
        archived: r.get::<_, i64>(3)? != 0,
        created_at: r.get(4)?,
    })
}

/// Create a project. `name` is trimmed; the caller rejects blanks.
pub fn create_project(conn: &Connection, name: &str) -> rusqlite::Result<Project> {
    let id = new_id();
    conn.execute(
        "INSERT INTO projects (id, name, created_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        params![id, name.trim()],
    )?;
    Ok(get_project(conn, &id)?.expect("row just inserted"))
}

/// Fetch one project by id.
pub fn get_project(conn: &Connection, id: &str) -> rusqlite::Result<Option<Project>> {
    conn.query_row(
        &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1"),
        params![id],
        row_to_project,
    )
    .optional()
}

/// All non-archived projects, ordered by name.
pub fn list_projects(conn: &Connection) -> rusqlite::Result<Vec<Project>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PROJECT_COLUMNS} FROM projects
         WHERE archived = 0
         ORDER BY name COLLATE NOCASE, created_at"
    ))?;
    let rows = stmt.query_map([], row_to_project)?;
    rows.collect()
}

/// Rename a project. `name` is trimmed.
pub fn rename_project(conn: &Connection, id: &str, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE projects SET name = ?2 WHERE id = ?1",
        params![id, name.trim()],
    )?;
    Ok(())
}

/// Delete a project. Its tasks are kept and become unfiled
/// (`tasks.project_id` is `ON DELETE SET NULL`).
pub fn delete_project(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(conn: &Connection, title: &str) -> Task {
        create_task(conn, title, None, None).unwrap()
    }

    fn child(conn: &Connection, title: &str, parent: &str) -> Task {
        create_task(conn, title, Some(parent), None).unwrap()
    }

    fn completed_at(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row(
            "SELECT completed_at FROM tasks WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Task titles in tree display order.
    fn titles(conn: &Connection) -> Vec<String> {
        list_task_tree(conn, &ProjectFilter::All)
            .unwrap()
            .into_iter()
            .map(|n| n.task.title)
            .collect()
    }

    #[test]
    fn migrations_reach_head_and_are_idempotent() {
        let conn = open_in_memory().unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, migrations::COUNT);

        // Running again must not error or change the version.
        let mut conn = conn;
        migrations::run(&mut conn).unwrap();
        let v2: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn create_trims_and_orders_by_insertion() {
        let conn = open_in_memory().unwrap();
        let a = root(&conn, "  write plan  ");
        let b = root(&conn, "ship it");

        assert_eq!(a.title, "write plan");
        assert!(a.sort_order < b.sort_order);

        assert_eq!(titles(&conn), ["write plan", "ship it"]);
    }

    #[test]
    fn toggle_status_stamps_and_clears_completed_at() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "task");
        assert!(completed_at(&conn, &t.id).is_none());

        set_task_status(&conn, &t.id, TaskStatus::Done).unwrap();
        assert_eq!(
            get_task(&conn, &t.id).unwrap().unwrap().status,
            TaskStatus::Done
        );
        assert!(completed_at(&conn, &t.id).is_some());

        set_task_status(&conn, &t.id, TaskStatus::Todo).unwrap();
        assert!(completed_at(&conn, &t.id).is_none());
    }

    #[test]
    fn delete_cascades_to_subtasks() {
        let conn = open_in_memory().unwrap();
        let parent = root(&conn, "parent");
        child(&conn, "child", &parent.id);
        assert_eq!(titles(&conn).len(), 2);

        delete_task(&conn, &parent.id).unwrap();
        assert!(titles(&conn).is_empty());
    }

    #[test]
    fn task_tree_is_preordered_with_depth_and_child_flags() {
        let conn = open_in_memory().unwrap();
        let a = root(&conn, "A");
        let a1 = child(&conn, "A1", &a.id);
        child(&conn, "A1a", &a1.id);
        child(&conn, "A2", &a.id);
        root(&conn, "B");

        let tree = list_task_tree(&conn, &ProjectFilter::All).unwrap();
        let shape: Vec<_> = tree
            .iter()
            .map(|n| (n.task.title.as_str(), n.depth, n.has_children))
            .collect();
        assert_eq!(
            shape,
            [
                ("A", 0, true),
                ("A1", 1, true),
                ("A1a", 2, false),
                ("A2", 1, false),
                ("B", 0, false),
            ]
        );
    }

    #[test]
    fn siblings_keep_insertion_order_within_a_parent() {
        let conn = open_in_memory().unwrap();
        let a = root(&conn, "A");
        let first = child(&conn, "first", &a.id);
        let second = child(&conn, "second", &a.id);
        assert!(first.sort_order < second.sort_order);
        assert_eq!(titles(&conn), ["A", "first", "second"]);
    }

    #[test]
    fn rename_trims_and_persists() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "old");
        rename_task(&conn, &t.id, "  new  ").unwrap();
        assert_eq!(get_task(&conn, &t.id).unwrap().unwrap().title, "new");
    }

    #[test]
    fn only_one_running_timer_allowed() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "task");
        let insert_open = |suffix: &str| {
            conn.execute(
                "INSERT INTO time_entries (id, task_id, start_ts, created_at)
                 VALUES (?1, ?2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                params![format!("e{suffix}"), t.id],
            )
        };
        assert!(insert_open("1").is_ok());
        assert!(
            insert_open("2").is_err(),
            "second open entry must be rejected"
        );
    }

    #[test]
    fn project_filter_scopes_the_tree_and_subtasks_inherit() {
        let conn = open_in_memory().unwrap();
        let work = create_project(&conn, "Work").unwrap();

        let filed = create_task(&conn, "filed", None, Some(&work.id)).unwrap();
        let sub = create_task(&conn, "sub", Some(&filed.id), None).unwrap();
        root(&conn, "loose");

        // Subtask inherited the parent's project despite passing None.
        assert_eq!(
            get_task(&conn, &sub.id).unwrap().unwrap().project_id,
            Some(work.id.clone())
        );

        let in_work: Vec<_> = list_task_tree(&conn, &ProjectFilter::Only(work.id.clone()))
            .unwrap()
            .into_iter()
            .map(|n| n.task.title)
            .collect();
        assert_eq!(in_work, ["filed", "sub"]);

        let unfiled: Vec<_> = list_task_tree(&conn, &ProjectFilter::Unfiled)
            .unwrap()
            .into_iter()
            .map(|n| n.task.title)
            .collect();
        assert_eq!(unfiled, ["loose"]);
    }

    #[test]
    fn deleting_a_project_unfiles_its_tasks() {
        let conn = open_in_memory().unwrap();
        let p = create_project(&conn, "Temp").unwrap();
        let t = create_task(&conn, "t", None, Some(&p.id)).unwrap();

        delete_project(&conn, &p.id).unwrap();

        assert!(list_projects(&conn).unwrap().is_empty());
        assert_eq!(get_task(&conn, &t.id).unwrap().unwrap().project_id, None);
    }

    #[test]
    fn move_task_carries_its_subtree() {
        let conn = open_in_memory().unwrap();
        let p = create_project(&conn, "P").unwrap();
        let parent = root(&conn, "parent");
        let kid = child(&conn, "kid", &parent.id);

        set_task_project(&conn, &parent.id, Some(&p.id)).unwrap();

        assert_eq!(
            get_task(&conn, &parent.id).unwrap().unwrap().project_id,
            Some(p.id.clone())
        );
        assert_eq!(
            get_task(&conn, &kid.id).unwrap().unwrap().project_id,
            Some(p.id)
        );
    }

    #[test]
    fn projects_listed_by_name_case_insensitive() {
        let conn = open_in_memory().unwrap();
        create_project(&conn, "  banana ").unwrap();
        create_project(&conn, "Apple").unwrap();
        let names: Vec<_> = list_projects(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Apple", "banana"]);
    }
}
