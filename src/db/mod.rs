//! SQLite persistence and the task repository.
//!
//! Everything here is plain Rust with no Qt types, so it is exercised directly
//! by `cargo test` against an in-memory database. The Qt bridge
//! (`crate::tasks_model`) is a thin adapter on top.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::{new_id, EntrySource, Project, ProjectFilter, Task, TaskStatus, TimeEntry};

pub mod actions;
mod migrations;

/// SQLite expression for the current wall-clock time, `YYYY-MM-DDTHH:MM:SS`.
///
/// Timestamps are stored in **local naive time** (no zone) so that timer
/// entries and hand-entered entries share one clock and their durations add up.
/// The tradeoff is minor skew around DST changes / travelling between zones,
/// acceptable for a personal tracker.
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%S', 'now', 'localtime')";

/// SQLite expression for today's local date, `YYYY-MM-DD`.
const TODAY: &str = "date('now', 'localtime')";

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
                            deadline, tracked, status, sort_order, created_at, \
                            initial_time_seconds";

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
        initial_time_seconds: r.get(10)?,
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
        &format!(
            "INSERT INTO tasks (id, parent_task_id, project_id, title, sort_order, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, {NOW})"
        ),
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

/// A task plus view-only annotations, in tree (pre-order) display order.
/// Siblings are ordered by `sort_order`.
#[derive(Debug, Clone)]
pub struct TaskNode {
    pub task: Task,
    pub depth: u32,
    pub has_children: bool,
    /// Deadline is in the past and the task isn't done.
    pub overdue: bool,
    /// Last of its parent's (visible) children - its connector is an elbow, not
    /// a tee. Always true for a root.
    pub is_last_child: bool,
    /// Per indent column (`depth` entries), whether a full-height tree guide
    /// line should be drawn there: `branch_more[i]` is true when the ancestor
    /// owning column `i` still has siblings below this row.
    pub branch_more: Vec<bool>,
}

/// Fill in `is_last_child` / `branch_more` for a pre-ordered, depth-tagged list.
fn annotate_branches(nodes: &mut [TaskNode]) {
    let n = nodes.len();
    // A node is a last child when the next row past its whole subtree is
    // shallower (or there is none).
    let mut last = vec![true; n];
    for k in 0..n {
        let d = nodes[k].depth;
        let mut j = k + 1;
        while j < n && nodes[j].depth > d {
            j += 1;
        }
        last[k] = j == n || nodes[j].depth < d;
    }
    // Walk the path stack: `stack[i]` is `is_last_child` of the ancestor at
    // depth i (or of this node at its own depth). Column i of a row shows a
    // pipe when the ancestor at depth i+1 is not a last child.
    let mut stack: Vec<bool> = Vec::new();
    for k in 0..n {
        let d = nodes[k].depth as usize;
        stack.truncate(d);
        stack.push(last[k]);
        nodes[k].is_last_child = last[k];
        nodes[k].branch_more = (0..d).map(|i| !stack[i + 1]).collect();
    }
}

fn map_task_node(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskNode> {
    Ok(TaskNode {
        task: row_to_task(r)?,
        depth: r.get::<_, i64>(11)? as u32,
        has_children: r.get::<_, i64>(12)? != 0,
        overdue: r.get::<_, i64>(13)? != 0,
        is_last_child: true,
        branch_more: Vec::new(),
    })
}

/// All tasks matching `filter` as a depth-annotated, pre-ordered list for the
/// flattened tree view.
///
/// The filter is applied only to root tasks: subtasks inherit their parent's
/// project, so a whole subtree always belongs to one project.
///
/// When `include_done` is false, completed tasks are omitted - and because the
/// recursive walk stops at them, so is anything nested under a completed task
/// (finishing a parent hides its whole branch). `has_children` reflects the
/// same filter, so the disclosure control never opens onto nothing.
pub fn list_task_tree(
    conn: &Connection,
    filter: &ProjectFilter,
    include_done: bool,
) -> rusqlite::Result<Vec<TaskNode>> {
    let (root_predicate, bind): (&str, Option<&str>) = match filter {
        ProjectFilter::All => ("1", None),
        ProjectFilter::Unfiled => ("project_id IS NULL", None),
        ProjectFilter::Only(id) => ("project_id = ?1", Some(id.as_str())),
    };
    let (root_done, t_done, c_done) = if include_done {
        ("", "", "")
    } else {
        (
            "AND status <> 'done'",
            "AND t.status <> 'done'",
            "AND c.status <> 'done'",
        )
    };
    // `sort_path` concatenates each ancestor's zero-padded sort_order so that
    // ordering the flat result by it yields a correct pre-order traversal.
    let sql = format!(
        "WITH RECURSIVE subtree(task_id, depth, sort_path) AS (
             SELECT id, 0, printf('%020.6f', sort_order)
             FROM tasks WHERE parent_task_id IS NULL AND {root_predicate} {root_done}
           UNION ALL
             SELECT t.id, s.depth + 1,
                    s.sort_path || '/' || printf('%020.6f', t.sort_order)
             FROM tasks t JOIN subtree s ON t.parent_task_id = s.task_id
             WHERE 1 {t_done}
         )
         SELECT {TASK_COLUMNS},
                subtree.depth,
                EXISTS(SELECT 1 FROM tasks c
                       WHERE c.parent_task_id = tasks.id {c_done}),
                (tasks.deadline IS NOT NULL
                    AND tasks.deadline < {TODAY}
                    AND tasks.status <> 'done')
         FROM tasks JOIN subtree ON tasks.id = subtree.task_id
         ORDER BY subtree.sort_path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = match bind {
        Some(id) => stmt.query_map(params![id], map_task_node)?,
        None => stmt.query_map([], map_task_node)?,
    };
    let mut nodes: Vec<TaskNode> = rows.collect::<rusqlite::Result<_>>()?;
    annotate_branches(&mut nodes);
    Ok(nodes)
}

/// Every completed task as a flat list (depth 0), most-recently-finished first.
///
/// The "Finished" view is deliberately not a tree: a done task's place in the
/// hierarchy matters less than when it was done.
pub fn list_finished_tasks(conn: &Connection) -> rusqlite::Result<Vec<TaskNode>> {
    let sql = format!(
        "SELECT {TASK_COLUMNS}, 0, 0, 0
         FROM tasks
         WHERE status = 'done'
         ORDER BY COALESCE(completed_at, created_at) DESC, created_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_task_node)?;
    rows.collect()
}

/// The task tree pruned to deadline-bearing work: a task is kept when it has a
/// deadline of its own, or any descendant does (so the chain from the root down
/// stays connected). Done tasks truncate a branch, exactly as in
/// [`list_task_tree`]. Result is pre-ordered and branch-annotated over the
/// *pruned* set, so the guide lines match what is actually on screen.
pub fn list_deadlined_tree(conn: &Connection) -> rusqlite::Result<Vec<TaskNode>> {
    let sql = format!(
        "WITH RECURSIVE
             -- deadline-bearing tasks, then their ancestor chain - but the
             -- walk stops at a done task, so a deadline buried under a
             -- finished parent keeps nothing (that branch is hidden anyway).
             keep(id) AS (
                 SELECT id FROM tasks
                 WHERE deadline IS NOT NULL AND status <> 'done'
               UNION
                 SELECT t.parent_task_id
                 FROM tasks t JOIN keep k ON t.id = k.id
                 WHERE t.parent_task_id IS NOT NULL AND t.status <> 'done'
             ),
             subtree(task_id, depth, sort_path) AS (
                 SELECT id, 0, printf('%020.6f', sort_order)
                 FROM tasks
                 WHERE parent_task_id IS NULL AND status <> 'done'
                   AND id IN (SELECT id FROM keep)
               UNION ALL
                 SELECT t.id, s.depth + 1,
                        s.sort_path || '/' || printf('%020.6f', t.sort_order)
                 FROM tasks t JOIN subtree s ON t.parent_task_id = s.task_id
                 WHERE t.status <> 'done'
                   AND t.id IN (SELECT id FROM keep)
             )
         SELECT {TASK_COLUMNS},
                subtree.depth,
                EXISTS(SELECT 1 FROM tasks c
                       WHERE c.parent_task_id = tasks.id
                         AND c.status <> 'done'
                         AND c.id IN (SELECT id FROM keep)),
                (tasks.deadline IS NOT NULL
                    AND tasks.deadline < {TODAY}
                    AND tasks.status <> 'done')
         FROM tasks JOIN subtree ON tasks.id = subtree.task_id
         ORDER BY subtree.sort_path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_task_node)?;
    let mut nodes: Vec<TaskNode> = rows.collect::<rusqlite::Result<_>>()?;
    annotate_branches(&mut nodes);
    Ok(nodes)
}

/// A task that has a deadline, for the calendar / agenda page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadlineItem {
    pub id: String,
    pub title: String,
    /// ISO `YYYY-MM-DD`.
    pub deadline: String,
    /// Deadline is before local today.
    pub overdue: bool,
    /// Owning project's name, or `""` when the task has no project.
    pub project: String,
}

/// Every not-done task with a deadline, earliest first (ties broken by title).
pub fn deadline_items(conn: &Connection) -> rusqlite::Result<Vec<DeadlineItem>> {
    let sql = format!(
        "SELECT t.id, t.title, t.deadline,
                (t.deadline < {TODAY}) AS overdue,
                COALESCE(p.name, '')
         FROM tasks t
         LEFT JOIN projects p ON p.id = t.project_id
         WHERE t.deadline IS NOT NULL AND t.status <> 'done'
         ORDER BY t.deadline, t.title COLLATE NOCASE"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(DeadlineItem {
            id: r.get(0)?,
            title: r.get(1)?,
            deadline: r.get(2)?,
            overdue: r.get::<_, i64>(3)? != 0,
            project: r.get(4)?,
        })
    })?;
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

/// Set a task's free-text notes / description (stored verbatim, trailing
/// whitespace trimmed).
pub fn set_task_notes(conn: &Connection, id: &str, notes: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tasks SET notes = ?2 WHERE id = ?1",
        params![id, notes.trim_end()],
    )?;
    Ok(())
}

/// Set (or clear) a task's one-off starting duration - see
/// [`Task::initial_time_seconds`](crate::domain::Task). Not summed into any
/// time total; purely a display field.
// TODO(quick-creation UI): called by the Quick Creation import (next PR).
#[allow(dead_code)]
pub fn set_initial_time(conn: &Connection, id: &str, seconds: Option<i64>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tasks SET initial_time_seconds = ?2 WHERE id = ?1",
        params![id, seconds],
    )?;
    Ok(())
}

/// True for an ISO calendar date, `YYYY-MM-DD`.
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..].iter().all(u8::is_ascii_digit)
}

/// Set (`Some("YYYY-MM-DD")`) or clear (`None`) a task's deadline.
/// A malformed date string is ignored.
pub fn set_task_deadline(
    conn: &Connection,
    id: &str,
    deadline: Option<&str>,
) -> rusqlite::Result<()> {
    match deadline {
        Some(d) if is_iso_date(d.trim()) => {
            conn.execute(
                "UPDATE tasks SET deadline = ?2 WHERE id = ?1",
                params![id, d.trim()],
            )?;
        }
        Some(_) => {} // ignore malformed input
        None => {
            conn.execute(
                "UPDATE tasks SET deadline = NULL WHERE id = ?1",
                params![id],
            )?;
        }
    }
    Ok(())
}

// --- Deadline countdown --------------------------------------------------

/// How "time until a deadline" is worded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountdownMode {
    /// Don't show it.
    Off,
    /// Largest sensible pair: "1 month and 3 days", "2 weeks and 1 day".
    Optimal,
    /// A whole count of one unit.
    Days,
    Weeks,
    Months,
    Hours,
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

/// `"<head> and <n> <unit>s"`, or just `head` when `n <= 0`.
fn and_then(head: String, n: i64, unit: &str) -> String {
    if n <= 0 {
        head
    } else {
        format!("{head} and {}", plural(n, unit))
    }
}

/// `value` in `unit`, rounded to two decimals: a whole number keeps the
/// integer wording (`"3 weeks"`), a fraction is spelled out (`"0.25 months"`,
/// `"1.5 weeks"`). Used for the fixed-unit deadline countdown so a task less
/// than one unit away no longer collapses to `"0 units"`.
fn decimal(value: f64, unit: &str) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded == rounded.trunc() {
        return plural(rounded as i64, unit);
    }
    let text = format!("{rounded:.2}");
    // "1.50" -> "1.5", but "0.25" stays put (strip at most one zero).
    let text = text.strip_suffix('0').unwrap_or(&text);
    format!("{text} {unit}s")
}

/// Whole calendar months from local today up to (not past) `deadline`.
fn whole_months(conn: &Connection, deadline: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        &format!(
            "WITH RECURSIVE m(n) AS (
                 SELECT 0
               UNION ALL
                 SELECT n + 1 FROM m
                 WHERE date({TODAY}, '+' || (n + 1) || ' months') <= date(?1)
             )
             SELECT MAX(n) FROM m"
        ),
        params![deadline],
        |r| r.get(0),
    )
}

/// A human "time left until `deadline`" per `mode`. `""` when the mode is `Off`
/// or the date is unparseable; past deadlines read `"overdue …"`.
pub fn deadline_countdown(
    conn: &Connection,
    deadline: &str,
    mode: CountdownMode,
) -> rusqlite::Result<String> {
    let deadline = deadline.trim();
    if matches!(mode, CountdownMode::Off) || !is_iso_date(deadline) {
        return Ok(String::new());
    }
    let days: i64 = conn.query_row(
        &format!("SELECT CAST(julianday(date(?1)) - julianday({TODAY}) AS INTEGER)"),
        params![deadline],
        |r| r.get(0),
    )?;
    if days < 0 {
        return Ok(match mode {
            CountdownMode::Hours => format!("overdue by {}", plural(-days * 24, "hour")),
            _ => format!("overdue by {}", plural(-days, "day")),
        });
    }
    Ok(match mode {
        CountdownMode::Off => String::new(),
        CountdownMode::Hours => plural(days * 24, "hour"),
        CountdownMode::Days => match days {
            0 => "today".to_owned(),
            1 => "tomorrow".to_owned(),
            _ => plural(days, "day"),
        },
        CountdownMode::Weeks => match days {
            0 => "today".to_owned(),
            _ => decimal(days as f64 / 7.0, "week"),
        },
        CountdownMode::Months => match days {
            0 => "today".to_owned(),
            _ => {
                // whole calendar months, then the leftover days as a fraction
                // of the month they fall in.
                let whole = whole_months(conn, deadline)?;
                let (rem_days, month_len): (f64, f64) = conn.query_row(
                    &format!(
                        "SELECT julianday(date(?1))
                                  - julianday(date({TODAY}, '+' || ?2 || ' months')),
                                julianday(date({TODAY}, '+' || (?2 + 1) || ' months'))
                                  - julianday(date({TODAY}, '+' || ?2 || ' months'))"
                    ),
                    params![deadline, whole],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                decimal(whole as f64 + rem_days / month_len, "month")
            }
        },
        CountdownMode::Optimal => match days {
            0 => "today".to_owned(),
            1 => "tomorrow".to_owned(),
            _ => {
                let months = whole_months(conn, deadline)?;
                if months >= 1 {
                    let rem: i64 = conn.query_row(
                        &format!(
                            "SELECT CAST(julianday(date(?1))
                                       - julianday(date({TODAY}, '+' || ?2 || ' months')) AS INTEGER)"
                        ),
                        params![deadline, months],
                        |r| r.get(0),
                    )?;
                    and_then(plural(months, "month"), rem, "day")
                } else if days >= 7 {
                    and_then(plural(days / 7, "week"), days % 7, "day")
                } else {
                    plural(days, "day")
                }
            }
        },
    })
}

/// Set a task's status, stamping or clearing `completed_at` to match.
pub fn set_task_status(conn: &Connection, id: &str, status: TaskStatus) -> rusqlite::Result<()> {
    let done = matches!(status, TaskStatus::Done);
    conn.execute(
        &format!(
            "UPDATE tasks
             SET status = ?2,
                 completed_at = CASE WHEN ?3 THEN {NOW} ELSE NULL END
             WHERE id = ?1"
        ),
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

/// Whether `task_id` may be re-parented under `new_parent` - both must exist,
/// they must differ, and `new_parent` must not sit inside `task_id`'s subtree
/// (which would make a cycle).
pub fn can_reparent(conn: &Connection, task_id: &str, new_parent: &str) -> rusqlite::Result<bool> {
    if task_id == new_parent {
        return Ok(false);
    }
    let in_subtree: bool = conn.query_row(
        "WITH RECURSIVE subtree(id) AS (
             SELECT ?1
           UNION ALL
             SELECT t.id FROM tasks t JOIN subtree s ON t.parent_task_id = s.id
         )
         SELECT EXISTS(SELECT 1 FROM subtree WHERE id = ?2)",
        params![task_id, new_parent],
        |r| r.get::<_, i64>(0).map(|n| n != 0),
    )?;
    if in_subtree {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)
             AND EXISTS(SELECT 1 FROM tasks WHERE id = ?2)",
        params![task_id, new_parent],
        |r| r.get::<_, i64>(0).map(|n| n != 0),
    )
}

/// Re-parent `task_id` (with its whole subtree) under `new_parent`: sets its
/// `parent_task_id`, places it last among the new parent's children, and
/// propagates the new parent's `project_id` down the subtree. No-op when
/// [`can_reparent`] would reject the move.
pub fn reparent_task(conn: &Connection, task_id: &str, new_parent: &str) -> rusqlite::Result<()> {
    if !can_reparent(conn, task_id, new_parent)? {
        return Ok(());
    }
    let sort_order: f64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), 0) + 1.0 FROM tasks WHERE parent_task_id = ?1",
        params![new_parent],
        |r| r.get(0),
    )?;
    conn.execute(
        "UPDATE tasks SET parent_task_id = ?2, sort_order = ?3 WHERE id = ?1",
        params![task_id, new_parent, sort_order],
    )?;
    // Subtasks inherit their parent's project.
    let project: Option<String> = conn.query_row(
        "SELECT project_id FROM tasks WHERE id = ?1",
        params![new_parent],
        |r| r.get(0),
    )?;
    set_task_project(conn, task_id, project.as_deref())?;
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
        &format!(
            "INSERT INTO projects (id, name, created_at)
             VALUES (?1, ?2, {NOW})"
        ),
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

// --- Time entries & timer ----------------------------------------------------

const ENTRY_COLUMNS: &str = "id, task_id, start_ts, end_ts, source, note, created_at";

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<TimeEntry> {
    Ok(TimeEntry {
        id: r.get(0)?,
        task_id: r.get(1)?,
        start_ts: r.get(2)?,
        end_ts: r.get(3)?,
        source: EntrySource::from_db(&r.get::<_, String>(4)?),
        note: r.get(5)?,
        created_at: r.get(6)?,
    })
}

/// Fetch one time entry by id.
pub fn get_entry(conn: &Connection, id: &str) -> rusqlite::Result<Option<TimeEntry>> {
    conn.query_row(
        &format!("SELECT {ENTRY_COLUMNS} FROM time_entries WHERE id = ?1"),
        params![id],
        row_to_entry,
    )
    .optional()
}

/// The currently running timer entry (`end_ts IS NULL`), if any.
pub fn running_entry(conn: &Connection) -> rusqlite::Result<Option<TimeEntry>> {
    conn.query_row(
        &format!("SELECT {ENTRY_COLUMNS} FROM time_entries WHERE end_ts IS NULL"),
        [],
        row_to_entry,
    )
    .optional()
}

/// The running timer joined with its task, for the UI's timer bar.
#[derive(Debug, Clone, PartialEq)]
pub struct RunningTimer {
    pub entry_id: String,
    pub task_id: String,
    pub task_title: String,
    pub start_ts: String,
}

pub fn running_timer(conn: &Connection) -> rusqlite::Result<Option<RunningTimer>> {
    conn.query_row(
        "SELECT e.id, e.task_id, t.title, e.start_ts
         FROM time_entries e JOIN tasks t ON t.id = e.task_id
         WHERE e.end_ts IS NULL",
        [],
        |r| {
            Ok(RunningTimer {
                entry_id: r.get(0)?,
                task_id: r.get(1)?,
                task_title: r.get(2)?,
                start_ts: r.get(3)?,
            })
        },
    )
    .optional()
}

/// Start timing `task_id`. Any already-running timer is stopped first, so this
/// doubles as "switch the timer to this task".
pub fn start_timer(conn: &Connection, task_id: &str) -> rusqlite::Result<TimeEntry> {
    stop_timer(conn)?;
    let id = new_id();
    conn.execute(
        &format!(
            "INSERT INTO time_entries (id, task_id, start_ts, source, created_at)
             VALUES (?1, ?2, {NOW}, 'timer', {NOW})"
        ),
        params![id, task_id],
    )?;
    Ok(get_entry(conn, &id)?.expect("row just inserted"))
}

/// Stop the running timer, if any, setting its `end_ts` to now.
/// Returns the entry that was stopped.
pub fn stop_timer(conn: &Connection) -> rusqlite::Result<Option<TimeEntry>> {
    let Some(entry) = running_entry(conn)? else {
        return Ok(None);
    };
    conn.execute(
        &format!("UPDATE time_entries SET end_ts = {NOW} WHERE id = ?1"),
        params![entry.id],
    )?;
    get_entry(conn, &entry.id)
}

/// Duration of a closed entry in whole seconds (0 if it is still open or gone).
/// Used by the timer's pause/resume bookkeeping to accumulate worked time.
pub fn entry_duration_seconds(conn: &Connection, id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(strftime('%s', end_ts) - strftime('%s', start_ts), 0)
         FROM time_entries WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .optional()
    .map(|o| o.unwrap_or(0))
}

// --- Crash recovery --------------------------------------------------------

/// `meta` key holding the last local time the running timer was seen alive.
const HEARTBEAT_KEY: &str = "timer_heartbeat";

/// Read a value from the `meta` key/value table.
pub fn get_meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
        r.get(0)
    })
    .optional()
}

/// Upsert a value into the `meta` key/value table.
pub fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Stamp the running timer as alive as of now. Called on start/resume and on a
/// periodic tick from QML so `recover_orphan_timer` has a recent cut-off.
pub fn timer_heartbeat(conn: &Connection) -> rusqlite::Result<()> {
    let now: String = conn.query_row(&format!("SELECT {NOW}"), [], |r| r.get(0))?;
    set_meta(conn, HEARTBEAT_KEY, &now)
}

/// What a crash left running, after it has been closed off.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredTimer {
    pub task_id: String,
    pub task_title: String,
    /// Worked seconds now banked on the (now closed) entry.
    pub seconds: i64,
}

/// Close an entry a previous run left open (`end_ts IS NULL`), billing it only
/// up to the last heartbeat and never outside the entry's own `[start, now]`
/// span. Returns what was recovered so the caller can present it as a paused
/// session; `None` when nothing was left running.
///
/// The cut-off is `max(start_ts, min(heartbeat, now))`: a missing or stale
/// heartbeat collapses to `start_ts` (≈ zero duration - we don't guess), a
/// future one is clamped to now, otherwise the heartbeat wins.
pub fn recover_orphan_timer(conn: &Connection) -> rusqlite::Result<Option<RecoveredTimer>> {
    let Some(rt) = running_timer(conn)? else {
        return Ok(None);
    };
    let heartbeat = get_meta(conn, HEARTBEAT_KEY)?;
    conn.execute(
        &format!(
            "UPDATE time_entries
                SET end_ts = MAX(start_ts, MIN(COALESCE(?2, start_ts), {NOW}))
              WHERE id = ?1"
        ),
        params![rt.entry_id, heartbeat],
    )?;
    let seconds = entry_duration_seconds(conn, &rt.entry_id)?;
    Ok(Some(RecoveredTimer {
        task_id: rt.task_id,
        task_title: rt.task_title,
        seconds,
    }))
}

/// True for `YYYY-MM-DDTHH:MM` or `YYYY-MM-DDTHH:MM:SS` (a space instead of `T`
/// is also accepted). Cheap structural check, not a full calendar validation.
fn is_iso_datetime(s: &str) -> bool {
    let s = s.trim();
    let bytes = s.as_bytes();
    if !(bytes.len() == 16 || bytes.len() == 19) {
        return false;
    }
    let digit = |i: usize| bytes[i].is_ascii_digit();
    (0..4).all(digit)
        && bytes[4] == b'-'
        && (5..7).all(digit)
        && bytes[7] == b'-'
        && (8..10).all(digit)
        && (bytes[10] == b'T' || bytes[10] == b' ')
        && (11..13).all(digit)
        && bytes[13] == b':'
        && (14..16).all(digit)
        && (bytes.len() == 16 || (bytes[16] == b':' && (17..19).all(digit)))
}

/// Normalise an accepted datetime to `YYYY-MM-DDTHH:MM:SS` (adds `:00` seconds,
/// swaps a space separator for `T`). Assumes `is_iso_datetime` already passed.
fn normalise_datetime(s: &str) -> String {
    let mut s = s.trim().replace(' ', "T");
    if s.len() == 16 {
        s.push_str(":00");
    }
    s
}

/// A time entry plus its duration in seconds (0 while running).
#[derive(Debug, Clone, PartialEq)]
pub struct EntryRow {
    pub entry: TimeEntry,
    pub seconds: i64,
}

/// Every time entry for a task, most recent first, each with its duration.
///
/// `strftime('%s', ...)` reads the naive timestamps as if UTC; since both ends
/// of an entry use the same clock, the difference is still correct.
pub fn list_entries_for_task(conn: &Connection, task_id: &str) -> rusqlite::Result<Vec<EntryRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS},
                CASE WHEN end_ts IS NULL THEN 0
                     ELSE strftime('%s', end_ts) - strftime('%s', start_ts) END
         FROM time_entries
         WHERE task_id = ?1
         ORDER BY start_ts DESC, id DESC"
    ))?;
    let rows = stmt.query_map(params![task_id], |r| {
        Ok(EntryRow {
            entry: row_to_entry(r)?,
            seconds: r.get::<_, i64>(7)?,
        })
    })?;
    rows.collect()
}

/// Add a hand-entered time entry. Both timestamps are required and `start` must
/// precede `end`; malformed or backwards input yields `Ok(None)`.
pub fn add_manual_entry(
    conn: &Connection,
    task_id: &str,
    start: &str,
    end: &str,
    note: &str,
) -> rusqlite::Result<Option<TimeEntry>> {
    if !is_iso_datetime(start) || !is_iso_datetime(end) {
        return Ok(None);
    }
    let (start, end) = (normalise_datetime(start), normalise_datetime(end));
    if start >= end {
        return Ok(None);
    }
    let id = new_id();
    conn.execute(
        &format!(
            "INSERT INTO time_entries (id, task_id, start_ts, end_ts, source, note, created_at)
             VALUES (?1, ?2, ?3, ?4, 'manual', ?5, {NOW})"
        ),
        params![id, task_id, start, end, note.trim()],
    )?;
    get_entry(conn, &id)
}

/// Edit an existing entry's span and note. Malformed / backwards input is
/// ignored (returns `Ok(false)`).
pub fn update_entry(
    conn: &Connection,
    id: &str,
    start: &str,
    end: &str,
    note: &str,
) -> rusqlite::Result<bool> {
    if !is_iso_datetime(start) || !is_iso_datetime(end) {
        return Ok(false);
    }
    let (start, end) = (normalise_datetime(start), normalise_datetime(end));
    if start >= end {
        return Ok(false);
    }
    let n = conn.execute(
        "UPDATE time_entries SET start_ts = ?2, end_ts = ?3, note = ?4 WHERE id = ?1",
        params![id, start, end, note.trim()],
    )?;
    Ok(n > 0)
}

/// Delete a time entry.
pub fn delete_entry(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM time_entries WHERE id = ?1", params![id])?;
    Ok(())
}

// --- Time-invested totals ------------------------------------------------

/// SQL summing a set of `time_entries` rows to whole seconds (open rows count 0).
const SUM_SECONDS: &str = "COALESCE(SUM(CASE WHEN end_ts IS NULL THEN 0
                       ELSE strftime('%s', end_ts) - strftime('%s', start_ts) END), 0)";

/// Seconds recorded against `task_id`: just its own entries, or - with
/// `include_subtree` - its own plus every descendant task's.
pub fn task_seconds(
    conn: &Connection,
    task_id: &str,
    include_subtree: bool,
) -> rusqlite::Result<i64> {
    let sql = if include_subtree {
        format!(
            "WITH RECURSIVE subtree(id) AS (
                 SELECT ?1
               UNION ALL
                 SELECT t.id FROM tasks t JOIN subtree s ON t.parent_task_id = s.id
             )
             SELECT {SUM_SECONDS} FROM time_entries
             WHERE task_id IN (SELECT id FROM subtree)"
        )
    } else {
        format!("SELECT {SUM_SECONDS} FROM time_entries WHERE task_id = ?1")
    };
    conn.query_row(&sql, params![task_id], |r| r.get(0))
}

/// Whether `task_id` has at least one direct subtask.
pub fn has_subtasks(conn: &Connection, task_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE parent_task_id = ?1)",
        params![task_id],
        |r| r.get::<_, i64>(0).map(|n| n != 0),
    )
}

/// Seconds recorded across every task in `filter`'s scope (all tasks, the
/// project-less ones, or one project). Subtasks inherit their parent's project,
/// so a project scope already covers whole subtrees.
pub fn scope_seconds(conn: &Connection, filter: &ProjectFilter) -> rusqlite::Result<i64> {
    let (predicate, bind): (&str, Option<&str>) = match filter {
        ProjectFilter::All => ("1", None),
        ProjectFilter::Unfiled => ("t.project_id IS NULL", None),
        ProjectFilter::Only(id) => ("t.project_id = ?1", Some(id.as_str())),
    };
    let sql = format!(
        "SELECT {SUM_SECONDS}
         FROM time_entries e JOIN tasks t ON t.id = e.task_id
         WHERE {predicate}"
    );
    match bind {
        Some(id) => conn.query_row(&sql, params![id], |r| r.get(0)),
        None => conn.query_row(&sql, [], |r| r.get(0)),
    }
}

// --- Time-invested heatmap ------------------------------------------------

/// What the graph is summing over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesTarget {
    /// Every task.
    All,
    /// Tasks in one project.
    Project(String),
    /// One task and its subtasks.
    TaskSubtree(String),
}

/// The span the heatmap covers: the week, month or year around an anchor date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeatRange {
    Week,
    Month,
    Year,
}

impl HeatRange {
    /// `(grid start, grid end, "inside the period proper" predicate)` as SQL.
    /// The first two evaluate to a Monday and a Sunday so the grid squares off;
    /// all three reference `?1`, any ISO date within the period.
    ///
    /// `date(X, 'weekday 1', '-7 days')` is deliberately avoided for the Monday
    /// shift - it overshoots a week when X already is a Monday - so we subtract
    /// the ISO weekday (`%u`, Mon=1) directly.
    fn sql(self) -> (&'static str, &'static str, &'static str) {
        match self {
            HeatRange::Week => (
                "date(?1, '-' || (strftime('%u', ?1) - 1) || ' days')",
                "date(?1, '+' || (7 - strftime('%u', ?1)) || ' days')",
                "1",
            ),
            HeatRange::Month => (
                "date(date(?1,'start of month'), \
                 '-' || (strftime('%u', date(?1,'start of month')) - 1) || ' days')",
                "date(date(?1,'start of month','+1 month','-1 day'), \
                 '+' || (7 - strftime('%u', date(?1,'start of month','+1 month','-1 day'))) || ' days')",
                "strftime('%Y-%m', span.day) = strftime('%Y-%m', ?1)",
            ),
            HeatRange::Year => (
                "date(date(?1,'start of year'), \
                 '-' || (strftime('%u', date(?1,'start of year')) - 1) || ' days')",
                "date(date(?1,'start of year','+1 year','-1 day'), \
                 '+' || (7 - strftime('%u', date(?1,'start of year','+1 year','-1 day'))) || ' days')",
                "strftime('%Y', span.day) = strftime('%Y', ?1)",
            ),
        }
    }

    /// The SQLite date modifier for stepping one period.
    fn step_modifier(self) -> &'static str {
        match self {
            HeatRange::Week => "7 days",
            HeatRange::Month => "1 month",
            HeatRange::Year => "1 year",
        }
    }

    /// The SQLite modifier that snaps a date to this period's first day.
    fn snap_modifier(self) -> &'static str {
        match self {
            // Handled specially (weekday arithmetic); see [`snap_anchor`].
            HeatRange::Week => "",
            HeatRange::Month => "start of month",
            HeatRange::Year => "start of year",
        }
    }
}

/// One day-cell of the calendar heatmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeatCell {
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Seconds recorded that day for the target (0 if none).
    pub seconds: i64,
    /// False for the leading/trailing pad days borrowed from the neighbouring
    /// periods to square off the first and last weeks.
    pub in_period: bool,
}

/// Snap `date` (ISO) to the first day of the `range` period containing it - the
/// Monday for a week, the 1st for a month, Jan 1 for a year. Keeping the anchor
/// normalised means stepping it by whole months/years can't drift.
pub fn snap_anchor(conn: &Connection, range: HeatRange, date: &str) -> rusqlite::Result<String> {
    let expr = match range {
        HeatRange::Week => "date(?1, '-' || (strftime('%u', ?1) - 1) || ' days')".to_owned(),
        _ => format!("date(?1, '{}')", range.snap_modifier()),
    };
    conn.query_row(&format!("SELECT {expr}"), params![date], |r| r.get(0))
}

/// Move `anchor` (ISO, already snapped) by `direction` periods and re-snap.
pub fn step_anchor(
    conn: &Connection,
    range: HeatRange,
    anchor: &str,
    direction: i32,
) -> rusqlite::Result<String> {
    let sign = if direction < 0 { "-" } else { "+" };
    let stepped: String = conn.query_row(
        &format!("SELECT date(?1, '{sign}{}')", range.step_modifier()),
        params![anchor],
        |r| r.get(0),
    )?;
    snap_anchor(conn, range, &stepped)
}

/// True when the period that `anchor` sits in contains today or lies in the
/// future - i.e. there is nothing newer to page to.
pub fn heat_at_latest(conn: &Connection, range: HeatRange, anchor: &str) -> rusqlite::Result<bool> {
    let today: String = conn.query_row(&format!("SELECT {TODAY}"), [], |r| r.get(0))?;
    let today_anchor = snap_anchor(conn, range, &today)?;
    Ok(anchor >= today_anchor.as_str())
}

/// Dense day-by-day totals for `target` over the Monday-aligned grid that spans
/// the `range` period around `anchor`. Ordered by date (so by week then weekday,
/// Mon..Sun); `cells[i]` sits at grid column `i / 7`, row `i % 7` for a
/// week-per-column layout, or row `i / 7`, column `i % 7` for a week-per-row
/// one. A running timer is not counted.
pub fn heatmap(
    conn: &Connection,
    target: &SeriesTarget,
    range: HeatRange,
    anchor: &str,
) -> rusqlite::Result<Vec<HeatCell>> {
    let (subtree_cte, predicate, bind): (&str, &str, Option<&str>) = match target {
        SeriesTarget::All => ("", "1", None),
        SeriesTarget::Project(id) => (
            "",
            "e.task_id IN (SELECT id FROM tasks WHERE project_id = ?2)",
            Some(id.as_str()),
        ),
        SeriesTarget::TaskSubtree(id) => (
            ", subtree(id) AS (
                 SELECT ?2
               UNION ALL
                 SELECT t.id FROM tasks t JOIN subtree s ON t.parent_task_id = s.id
             )",
            "e.task_id IN (SELECT id FROM subtree)",
            Some(id.as_str()),
        ),
    };
    let (lo, hi, in_period) = range.sql();
    let sql = format!(
        "WITH RECURSIVE
           span(day, last) AS (
             SELECT {lo}, {hi}
             UNION ALL
             SELECT date(day, '+1 day'), last FROM span WHERE day < last
           ){subtree_cte}
         SELECT span.day,
                {in_period} AS in_period,
                COALESCE(SUM(strftime('%s', e.end_ts) - strftime('%s', e.start_ts)), 0)
         FROM span
         LEFT JOIN time_entries e
                ON substr(e.start_ts, 1, 10) = span.day
               AND e.end_ts IS NOT NULL
               AND {predicate}
         GROUP BY span.day
         ORDER BY span.day"
    );
    let map_row = |r: &rusqlite::Row<'_>| {
        Ok(HeatCell {
            date: r.get(0)?,
            in_period: r.get::<_, i64>(1)? != 0,
            seconds: r.get(2)?,
        })
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = match bind {
        Some(id) => stmt.query_map(params![anchor, id], map_row)?,
        None => stmt.query_map(params![anchor], map_row)?,
    };
    rows.collect()
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

    /// An ISO date `mods` (SQLite date modifiers) away from local today,
    /// e.g. `date_at(&conn, "'+2 days'")`.
    fn date_at(conn: &Connection, mods: &str) -> String {
        conn.query_row(&format!("SELECT date('now','localtime',{mods})"), [], |r| {
            r.get(0)
        })
        .unwrap()
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
        list_task_tree(conn, &ProjectFilter::All, true)
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
    fn reparent_moves_subtree_and_carries_project() {
        let conn = open_in_memory().unwrap();
        let proj = create_project(&conn, "P").unwrap();
        let a = create_task(&conn, "A", None, Some(&proj.id)).unwrap();
        let b = root(&conn, "B"); // no project
        let b1 = child(&conn, "B1", &b.id);

        // Cycle / self guards.
        assert!(!can_reparent(&conn, &b.id, &b.id).unwrap());
        assert!(!can_reparent(&conn, &b.id, &b1.id).unwrap()); // onto own child
        assert!(!can_reparent(&conn, &b.id, "ghost").unwrap());
        assert!(can_reparent(&conn, &b.id, &a.id).unwrap());

        // Move B (and B1) under A: B becomes A's child, both gain A's project.
        reparent_task(&conn, &b.id, &a.id).unwrap();
        let tree = list_task_tree(&conn, &ProjectFilter::All, true).unwrap();
        let by = |id: &str| tree.iter().find(|n| n.task.id == id).unwrap();
        assert_eq!(by(&b.id).task.parent_id.as_deref(), Some(a.id.as_str()));
        assert_eq!(by(&b.id).task.project_id.as_deref(), Some(proj.id.as_str()));
        assert_eq!(
            by(&b1.id).task.project_id.as_deref(),
            Some(proj.id.as_str())
        );
        assert_eq!(by(&b.id).depth, 1);
        assert_eq!(by(&b1.id).depth, 2);

        // A rejected move leaves everything as it was.
        reparent_task(&conn, &a.id, &b1.id).unwrap(); // would be a cycle
        assert_eq!(
            list_task_tree(&conn, &ProjectFilter::All, true).unwrap()[0]
                .task
                .id,
            a.id
        );
    }

    #[test]
    fn deadline_countdown_words_and_modes() {
        use CountdownMode::*;
        let conn = open_in_memory().unwrap();
        // A date `mods` (SQLite modifiers) away from local today.
        let at = |mods: &str| -> String {
            conn.query_row(&format!("SELECT date('now','localtime',{mods})"), [], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let cd = |date: &str, m| deadline_countdown(&conn, date, m).unwrap();

        assert_eq!(cd(&at("'+3 days'"), Off), "");
        assert_eq!(cd("not-a-date", Optimal), "");

        assert_eq!(cd(&at("'+0 days'"), Days), "today");
        assert_eq!(cd(&at("'+1 day'"), Days), "tomorrow");
        assert_eq!(cd(&at("'+5 days'"), Days), "5 days");
        assert_eq!(cd(&at("'+21 days'"), Weeks), "3 weeks");
        assert_eq!(cd(&at("'+2 days'"), Hours), "48 hours");
        assert_eq!(cd(&at("'-2 days'"), Days), "overdue by 2 days");

        // Fixed unit, less than one unit away: a fraction, not "0".
        assert_eq!(cd(&at("'+5 days'"), Weeks), "0.71 weeks");
        assert_eq!(cd(&at("'+0 days'"), Weeks), "today");
        assert_eq!(cd(&at("'+0 days'"), Months), "today");
        let m = cd(&at("'+10 days'"), Months);
        assert!(m.starts_with("0.3") && m.ends_with(" months"), "got {m}");
        // A whole count still reads as an integer.
        assert_eq!(cd(&at("'+14 days'"), Weeks), "2 weeks");

        assert_eq!(cd(&at("'+3 days'"), Optimal), "3 days");
        assert_eq!(cd(&at("'+8 days'"), Optimal), "1 week and 1 day");
        assert_eq!(cd(&at("'+14 days'"), Optimal), "2 weeks");
        assert!(cd(&at("'+1 month','+3 days'"), Optimal).starts_with("1 month"));
    }

    #[test]
    fn deadlined_tree_keeps_deadline_bearing_branches_and_their_ancestors() {
        let conn = open_in_memory().unwrap();

        let a = root(&conn, "A");
        let a1 = child(&conn, "A1", &a.id);
        let a1a = child(&conn, "A1a", &a1.id);
        child(&conn, "A2", &a.id); // sibling of A1, no deadline -> dropped
        set_task_deadline(&conn, &a1a.id, Some("2099-01-01")).unwrap();

        let b = root(&conn, "B");
        child(&conn, "B1", &b.id); // no deadline -> dropped
        set_task_deadline(&conn, &b.id, Some("2000-01-01")).unwrap();

        let c = root(&conn, "C");
        child(&conn, "C1", &c.id); // nothing deadlined here -> C dropped entirely

        // A deadline buried under a done parent stays hidden.
        let d = root(&conn, "D");
        let d1 = child(&conn, "D1", &d.id);
        let d1a = child(&conn, "D1a", &d1.id);
        set_task_deadline(&conn, &d1a.id, Some("2099-06-01")).unwrap();
        set_task_status(&conn, &d1.id, TaskStatus::Done).unwrap();

        let tree = list_deadlined_tree(&conn).unwrap();
        let shape: Vec<_> = tree
            .iter()
            .map(|n| {
                (
                    n.task.title.as_str(),
                    n.depth,
                    n.has_children,
                    n.is_last_child,
                    n.overdue,
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("A", 0, true, false, false),
                // A1 is now the last visible child of A (A2 was pruned).
                ("A1", 1, true, true, false),
                ("A1a", 2, false, true, false),
                ("B", 0, false, true, true),
            ]
        );
    }

    #[test]
    fn deadline_items_lists_dated_open_tasks_in_date_order() {
        let conn = open_in_memory().unwrap();
        let past = date_at(&conn, "'-3 days'");
        let soon = date_at(&conn, "'+2 days'");

        let a = root(&conn, "Zebra");
        set_task_deadline(&conn, &a.id, Some(&soon)).unwrap();
        let b = root(&conn, "Apple");
        set_task_deadline(&conn, &b.id, Some(&past)).unwrap();
        let c = root(&conn, "no deadline"); // excluded - no deadline
        let _ = c;
        let d = root(&conn, "done and dated");
        set_task_deadline(&conn, &d.id, Some(&soon)).unwrap();
        set_task_status(&conn, &d.id, TaskStatus::Done).unwrap(); // excluded - done

        let items = deadline_items(&conn).unwrap();
        let shape: Vec<_> = items
            .iter()
            .map(|i| (i.title.as_str(), i.deadline.as_str(), i.overdue))
            .collect();
        assert_eq!(
            shape,
            [
                ("Apple", past.as_str(), true),
                ("Zebra", soon.as_str(), false),
            ]
        );
    }

    #[test]
    fn task_tree_is_preordered_with_depth_and_child_flags() {
        let conn = open_in_memory().unwrap();
        let a = root(&conn, "A");
        let a1 = child(&conn, "A1", &a.id);
        child(&conn, "A1a", &a1.id);
        child(&conn, "A2", &a.id);
        root(&conn, "B");

        let tree = list_task_tree(&conn, &ProjectFilter::All, true).unwrap();
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
    fn tree_guide_flags_track_last_child_and_ancestor_branches() {
        let conn = open_in_memory().unwrap();
        let a = root(&conn, "A");
        let a1 = child(&conn, "A1", &a.id);
        child(&conn, "A1a", &a1.id); // only child of A1
        child(&conn, "A2", &a.id); // last child of A
        root(&conn, "B"); // last root

        let tree = list_task_tree(&conn, &ProjectFilter::All, true).unwrap();
        let by: Vec<_> = tree
            .iter()
            .map(|n| {
                (
                    n.task.title.as_str(),
                    n.is_last_child,
                    n.branch_more.clone(),
                )
            })
            .collect();
        assert_eq!(
            by,
            [
                ("A", false, vec![]),             // a root with B after it
                ("A1", false, vec![true]),        // A has A2 after A1 -> col 0 pipes
                ("A1a", true, vec![true, false]), // under A1 (pipes), itself last
                ("A2", true, vec![false]),        // last child of A
                ("B", true, vec![]),              // last root
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
    fn done_tasks_hidden_unless_included_and_listed_separately() {
        let conn = open_in_memory().unwrap();
        let keep = root(&conn, "keep");
        let parent = root(&conn, "parent");
        let buried = child(&conn, "buried", &parent.id);
        let finished = root(&conn, "finished");

        set_task_status(&conn, &finished.id, TaskStatus::Done).unwrap();
        set_task_status(&conn, &parent.id, TaskStatus::Done).unwrap();

        // Active view: no done task, and nothing nested under a done parent.
        let active: Vec<_> = list_task_tree(&conn, &ProjectFilter::All, false)
            .unwrap()
            .into_iter()
            .map(|n| n.task.title)
            .collect();
        assert_eq!(active, ["keep"]);

        // Including done brings the whole tree back.
        let all: Vec<_> = list_task_tree(&conn, &ProjectFilter::All, true)
            .unwrap()
            .into_iter()
            .map(|n| n.task.title)
            .collect();
        assert_eq!(all, ["keep", "parent", "buried", "finished"]);

        // The finished list is flat and holds only completed tasks.
        let done: Vec<_> = list_finished_tasks(&conn)
            .unwrap()
            .into_iter()
            .map(|n| (n.task.title, n.depth))
            .collect();
        assert_eq!(done.len(), 2);
        assert!(done.contains(&("finished".to_owned(), 0)));
        assert!(done.contains(&("parent".to_owned(), 0)));
        let _ = (keep, buried);
    }

    #[test]
    fn has_children_ignores_done_children_in_active_view() {
        let conn = open_in_memory().unwrap();
        let parent = root(&conn, "parent");
        let only_kid = child(&conn, "kid", &parent.id);
        set_task_status(&conn, &only_kid.id, TaskStatus::Done).unwrap();

        let active = list_task_tree(&conn, &ProjectFilter::All, false).unwrap();
        assert_eq!(active.len(), 1);
        assert!(
            !active[0].has_children,
            "parent's only child is done, so no disclosure control"
        );
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

        let in_work: Vec<_> = list_task_tree(&conn, &ProjectFilter::Only(work.id.clone()), true)
            .unwrap()
            .into_iter()
            .map(|n| n.task.title)
            .collect();
        assert_eq!(in_work, ["filed", "sub"]);

        let unfiled: Vec<_> = list_task_tree(&conn, &ProjectFilter::Unfiled, true)
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

    fn node(conn: &Connection, id: &str) -> TaskNode {
        list_task_tree(conn, &ProjectFilter::All, true)
            .unwrap()
            .into_iter()
            .find(|n| n.task.id == id)
            .unwrap()
    }

    #[test]
    fn deadline_set_clear_and_overdue_flag() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "task");
        assert_eq!(node(&conn, &t.id).task.deadline, None);
        assert!(!node(&conn, &t.id).overdue);

        set_task_deadline(&conn, &t.id, Some("2000-01-01")).unwrap();
        assert_eq!(
            node(&conn, &t.id).task.deadline.as_deref(),
            Some("2000-01-01")
        );
        assert!(node(&conn, &t.id).overdue, "a past date is overdue");

        // A completed task is never overdue.
        set_task_status(&conn, &t.id, TaskStatus::Done).unwrap();
        assert!(!node(&conn, &t.id).overdue);

        // A future date is not overdue.
        set_task_status(&conn, &t.id, TaskStatus::Todo).unwrap();
        set_task_deadline(&conn, &t.id, Some("2999-12-31")).unwrap();
        assert!(!node(&conn, &t.id).overdue);

        set_task_deadline(&conn, &t.id, None).unwrap();
        assert_eq!(node(&conn, &t.id).task.deadline, None);
    }

    #[test]
    fn malformed_deadline_is_ignored() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "task");
        set_task_deadline(&conn, &t.id, Some("31/12/2026")).unwrap();
        set_task_deadline(&conn, &t.id, Some("not a date")).unwrap();
        assert_eq!(node(&conn, &t.id).task.deadline, None);
    }

    #[test]
    fn timer_start_stop_and_switch() {
        let conn = open_in_memory().unwrap();
        let a = root(&conn, "a");
        let b = root(&conn, "b");

        assert!(running_timer(&conn).unwrap().is_none());

        let e1 = start_timer(&conn, &a.id).unwrap();
        assert!(e1.end_ts.is_none());
        assert_eq!(running_timer(&conn).unwrap().unwrap().task_id, a.id);

        // Starting on another task closes the first entry.
        start_timer(&conn, &b.id).unwrap();
        assert!(get_entry(&conn, &e1.id).unwrap().unwrap().end_ts.is_some());
        assert_eq!(running_timer(&conn).unwrap().unwrap().task_id, b.id);

        let stopped = stop_timer(&conn).unwrap().unwrap();
        assert_eq!(stopped.task_id, b.id);
        assert!(stopped.end_ts.is_some());
        assert!(running_timer(&conn).unwrap().is_none());

        // Stopping again is a harmless no-op.
        assert!(stop_timer(&conn).unwrap().is_none());
    }

    #[test]
    fn entry_duration_seconds_reads_a_closed_span() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "t");
        conn.execute(
            "INSERT INTO time_entries (id, task_id, start_ts, end_ts, created_at)
             VALUES ('d1', ?1, '2026-03-01T09:00:00', '2026-03-01T09:45:30', '2026-03-01T09:45:30')",
            params![t.id],
        )
        .unwrap();
        assert_eq!(entry_duration_seconds(&conn, "d1").unwrap(), 45 * 60 + 30);
        // Open or missing entries read as zero.
        start_timer(&conn, &t.id).unwrap();
        let open = running_entry(&conn).unwrap().unwrap();
        assert_eq!(entry_duration_seconds(&conn, &open.id).unwrap(), 0);
        assert_eq!(entry_duration_seconds(&conn, "nope").unwrap(), 0);
    }

    #[test]
    fn meta_get_set_roundtrips_and_upserts() {
        let conn = open_in_memory().unwrap();
        assert_eq!(get_meta(&conn, "k").unwrap(), None);
        set_meta(&conn, "k", "one").unwrap();
        assert_eq!(get_meta(&conn, "k").unwrap().as_deref(), Some("one"));
        set_meta(&conn, "k", "two").unwrap();
        assert_eq!(get_meta(&conn, "k").unwrap().as_deref(), Some("two"));
    }

    #[test]
    fn recover_orphan_timer_closes_at_heartbeat() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "t");
        // A run a crash left open, started well in the past.
        conn.execute(
            "INSERT INTO time_entries (id, task_id, start_ts, created_at)
             VALUES ('c1', ?1, '2026-03-01T09:00:00', '2026-03-01T09:00:00')",
            params![t.id],
        )
        .unwrap();
        set_meta(&conn, "timer_heartbeat", "2026-03-01T09:20:00").unwrap();

        let rec = recover_orphan_timer(&conn).unwrap().unwrap();
        assert_eq!(rec.task_id, t.id);
        assert_eq!(rec.task_title, "t");
        assert_eq!(rec.seconds, 20 * 60);

        // Entry closed exactly at the heartbeat; nothing still running.
        assert!(running_timer(&conn).unwrap().is_none());
        assert_eq!(
            get_entry(&conn, "c1").unwrap().unwrap().end_ts.as_deref(),
            Some("2026-03-01T09:20:00")
        );
        // Idempotent: a second call finds nothing open.
        assert!(recover_orphan_timer(&conn).unwrap().is_none());
    }

    #[test]
    fn recover_orphan_timer_clamps_missing_or_stale_heartbeat_to_start() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "t");
        conn.execute(
            "INSERT INTO time_entries (id, task_id, start_ts, created_at)
             VALUES ('c1', ?1, '2026-03-01T09:00:00', '2026-03-01T09:00:00')",
            params![t.id],
        )
        .unwrap();
        // Heartbeat older than the entry's start: don't bill negative time.
        set_meta(&conn, "timer_heartbeat", "2026-02-14T08:00:00").unwrap();

        let rec = recover_orphan_timer(&conn).unwrap().unwrap();
        assert_eq!(rec.seconds, 0);
        assert_eq!(
            get_entry(&conn, "c1").unwrap().unwrap().end_ts.as_deref(),
            Some("2026-03-01T09:00:00")
        );
    }

    #[test]
    fn recover_orphan_timer_noop_without_an_open_entry() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "t");
        start_timer(&conn, &t.id).unwrap();
        stop_timer(&conn).unwrap();
        assert!(recover_orphan_timer(&conn).unwrap().is_none());
    }

    #[test]
    fn manual_entries_add_list_edit_delete() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "task");

        let e = add_manual_entry(
            &conn,
            &t.id,
            "2026-03-01 09:00",
            "2026-03-01T10:30",
            "  morning  ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(e.start_ts, "2026-03-01T09:00:00");
        assert_eq!(e.note, "morning");

        let rows = list_entries_for_task(&conn, &t.id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seconds, 90 * 60);

        assert!(update_entry(
            &conn,
            &e.id,
            "2026-03-01T09:00",
            "2026-03-01T09:15",
            "short"
        )
        .unwrap());
        assert_eq!(
            list_entries_for_task(&conn, &t.id).unwrap()[0].seconds,
            15 * 60
        );

        delete_entry(&conn, &e.id).unwrap();
        assert!(list_entries_for_task(&conn, &t.id).unwrap().is_empty());
    }

    #[test]
    fn manual_entry_rejects_bad_input() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "task");
        // end before start
        assert!(
            add_manual_entry(&conn, &t.id, "2026-03-01T10:00", "2026-03-01T09:00", "")
                .unwrap()
                .is_none()
        );
        // malformed
        assert!(add_manual_entry(&conn, &t.id, "March 1", "later", "")
            .unwrap()
            .is_none());
        assert!(list_entries_for_task(&conn, &t.id).unwrap().is_empty());
    }

    #[test]
    fn list_entries_includes_the_running_timer_with_zero_duration() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "task");
        start_timer(&conn, &t.id).unwrap();
        let rows = list_entries_for_task(&conn, &t.id).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].entry.end_ts.is_none());
        assert_eq!(rows[0].seconds, 0);
    }

    #[test]
    fn time_totals_roll_up_over_subtree_and_scope() {
        let conn = open_in_memory().unwrap();
        let proj = create_project(&conn, "P").unwrap();
        let parent = create_task(&conn, "parent", None, Some(&proj.id)).unwrap();
        let kid = create_task(&conn, "kid", Some(&parent.id), None).unwrap();
        let grandkid = create_task(&conn, "grandkid", Some(&kid.id), None).unwrap();
        let loner = root(&conn, "loner"); // no project

        let entry = |task: &str, start: &str, end: &str| {
            conn.execute(
                "INSERT INTO time_entries (id, task_id, start_ts, end_ts, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?3)",
                params![new_id(), task, start, end],
            )
            .unwrap();
        };
        entry(&parent.id, "2026-03-01T09:00:00", "2026-03-01T10:00:00"); // 1h
        entry(&kid.id, "2026-03-02T09:00:00", "2026-03-02T09:30:00"); // 30m
        entry(&grandkid.id, "2026-03-03T09:00:00", "2026-03-03T11:00:00"); // 2h
        entry(&loner.id, "2026-03-04T09:00:00", "2026-03-04T09:15:00"); // 15m
        start_timer(&conn, &parent.id).unwrap(); // running: counts 0

        // Own vs whole subtree.
        assert_eq!(task_seconds(&conn, &parent.id, false).unwrap(), 3600);
        assert_eq!(
            task_seconds(&conn, &parent.id, true).unwrap(),
            3600 + 1800 + 7200
        );
        assert_eq!(task_seconds(&conn, &kid.id, true).unwrap(), 1800 + 7200);
        assert_eq!(task_seconds(&conn, &grandkid.id, true).unwrap(), 7200);

        assert!(has_subtasks(&conn, &parent.id).unwrap());
        assert!(!has_subtasks(&conn, &grandkid.id).unwrap());

        // Scope totals.
        assert_eq!(
            scope_seconds(&conn, &ProjectFilter::Only(proj.id.clone())).unwrap(),
            3600 + 1800 + 7200
        );
        assert_eq!(scope_seconds(&conn, &ProjectFilter::Unfiled).unwrap(), 900);
        assert_eq!(
            scope_seconds(&conn, &ProjectFilter::All).unwrap(),
            3600 + 1800 + 7200 + 900
        );
    }

    fn day_secs(cells: &[HeatCell], date: &str) -> i64 {
        cells
            .iter()
            .find(|c| c.date == date)
            .map(|c| c.seconds)
            .unwrap_or(-1)
    }

    #[test]
    fn heatmap_year_is_a_squared_371_cell_grid() {
        let conn = open_in_memory().unwrap();
        let cells = heatmap(&conn, &SeriesTarget::All, HeatRange::Year, "2026-06-15").unwrap();

        assert_eq!(cells.len(), 371); // always 53 weeks x 7 days
        assert_eq!(cells.iter().filter(|c| c.in_period).count(), 365); // 2026 not a leap year
        assert_eq!(cells.first().unwrap().date, "2025-12-29"); // Monday before Jan 1
        assert_eq!(cells.last().unwrap().date, "2027-01-03"); // Sunday after Dec 31
        assert!(cells.windows(2).all(|w| w[0].date < w[1].date)); // date order
                                                                  // A leap year still squares off to 371 with 366 in-period days.
        let leap = heatmap(&conn, &SeriesTarget::All, HeatRange::Year, "2024-01-01").unwrap();
        assert_eq!(leap.len(), 371);
        assert_eq!(leap.iter().filter(|c| c.in_period).count(), 366);
    }

    #[test]
    fn heatmap_week_and_month_grids() {
        let conn = open_in_memory().unwrap();

        // Week: Monday..Sunday of the anchor's week, all in-period.
        let wk = heatmap(&conn, &SeriesTarget::All, HeatRange::Week, "2026-09-10").unwrap();
        assert_eq!(wk.len(), 7);
        assert_eq!(wk.first().unwrap().date, "2026-09-07"); // Monday
        assert_eq!(wk.last().unwrap().date, "2026-09-13"); // Sunday
        assert!(wk.iter().all(|c| c.in_period));

        // Month: Monday-aligned weeks spanning the month; March 2026 needs 6.
        let mar = heatmap(&conn, &SeriesTarget::All, HeatRange::Month, "2026-03-20").unwrap();
        assert_eq!(mar.len(), 42);
        assert_eq!(mar.first().unwrap().date, "2026-02-23"); // Monday before Mar 1
        assert_eq!(mar.last().unwrap().date, "2026-04-05"); // Sunday after Mar 31
        assert_eq!(mar.iter().filter(|c| c.in_period).count(), 31);
        // Any month's grid is a whole number of weeks and holds exactly its days.
        let feb = heatmap(&conn, &SeriesTarget::All, HeatRange::Month, "2026-02-15").unwrap();
        assert_eq!(feb.len() % 7, 0);
        assert_eq!(feb.iter().filter(|c| c.in_period).count(), 28);
    }

    #[test]
    fn heatmap_scopes_and_counts_by_day() {
        let conn = open_in_memory().unwrap();
        let proj = create_project(&conn, "P").unwrap();
        let parent = create_task(&conn, "parent", None, Some(&proj.id)).unwrap();
        let kid = create_task(&conn, "kid", Some(&parent.id), None).unwrap();
        let other = root(&conn, "other");

        let entry = |task: &str, start: &str, end: &str| {
            conn.execute(
                "INSERT INTO time_entries (id, task_id, start_ts, end_ts, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?3)",
                params![new_id(), task, start, end],
            )
            .unwrap();
        };
        entry(&parent.id, "2026-03-01T09:00:00", "2026-03-01T10:00:00"); // 1h, day 03-01
        entry(&kid.id, "2026-03-01T14:00:00", "2026-03-01T14:30:00"); // 30m, day 03-01
        entry(&kid.id, "2026-03-05T09:00:00", "2026-03-05T11:00:00"); // 2h,  day 03-05
        entry(&other.id, "2026-03-01T09:00:00", "2026-03-01T12:00:00"); // 3h, other task
        entry(&kid.id, "2025-12-31T09:00:00", "2025-12-31T10:00:00"); // 1h, a pad day

        // Subtree = parent + kid; `other` is excluded.
        let sub = heatmap(
            &conn,
            &SeriesTarget::TaskSubtree(parent.id.clone()),
            HeatRange::Year,
            "2026-01-01",
        )
        .unwrap();
        assert_eq!(sub.len(), 371);
        assert_eq!(day_secs(&sub, "2026-03-01"), 5400);
        assert_eq!(day_secs(&sub, "2026-03-05"), 7200);
        assert_eq!(day_secs(&sub, "2026-03-02"), 0); // dense: a quiet day is still a cell
        assert_eq!(day_secs(&sub, "2025-12-31"), 3600); // pad days carry their totals too

        // Project scope == subtree here (kid inherits P).
        let proj = heatmap(
            &conn,
            &SeriesTarget::Project(proj.id),
            HeatRange::Year,
            "2026-01-01",
        )
        .unwrap();
        assert_eq!(proj.len(), 371);
        assert_eq!(day_secs(&proj, "2026-03-01"), 5400);

        // All picks up `other` on top. Row count matches across every target.
        let all = heatmap(&conn, &SeriesTarget::All, HeatRange::Year, "2026-01-01").unwrap();
        assert_eq!(all.len(), 371);
        assert_eq!(day_secs(&all, "2026-03-01"), 5400 + 10800);

        // A month view of March scopes to that month's cells only.
        let mar = heatmap(
            &conn,
            &SeriesTarget::TaskSubtree(parent.id),
            HeatRange::Month,
            "2026-03-15",
        )
        .unwrap();
        assert_eq!(day_secs(&mar, "2026-03-01"), 5400);
        assert_eq!(day_secs(&mar, "2026-03-05"), 7200);
    }

    #[test]
    fn heatmap_ignores_running_timer() {
        let conn = open_in_memory().unwrap();
        let t = root(&conn, "t");
        start_timer(&conn, &t.id).unwrap();
        let cells = heatmap(&conn, &SeriesTarget::All, HeatRange::Year, "2026-01-01").unwrap();
        assert_eq!(cells.len(), 371);
        assert!(cells.iter().all(|c| c.seconds == 0));
    }

    #[test]
    fn heat_at_latest_tracks_the_current_period() {
        let conn = open_in_memory().unwrap();
        let today: String = conn
            .query_row("SELECT date('now','localtime')", [], |r| r.get(0))
            .unwrap();
        let this_year = snap_anchor(&conn, HeatRange::Year, &today).unwrap();

        assert!(heat_at_latest(&conn, HeatRange::Year, &this_year).unwrap());
        let last_year = step_anchor(&conn, HeatRange::Year, &this_year, -1).unwrap();
        assert!(!heat_at_latest(&conn, HeatRange::Year, &last_year).unwrap());
        // A future period also counts as "nothing newer to page to".
        let next_year = step_anchor(&conn, HeatRange::Year, &this_year, 1).unwrap();
        assert!(heat_at_latest(&conn, HeatRange::Year, &next_year).unwrap());
    }

    #[test]
    fn anchor_snapping_and_stepping_do_not_drift() {
        let conn = open_in_memory().unwrap();

        // Snap: any day in the period -> its first day.
        assert_eq!(
            snap_anchor(&conn, HeatRange::Week, "2026-09-10").unwrap(),
            "2026-09-07"
        );
        assert_eq!(
            snap_anchor(&conn, HeatRange::Month, "2026-09-30").unwrap(),
            "2026-09-01"
        );
        assert_eq!(
            snap_anchor(&conn, HeatRange::Year, "2026-12-31").unwrap(),
            "2026-01-01"
        );

        // Step: from a month end, forward must land in the *next* month, not skip
        // it (the classic `setMonth(+1)` on the 31st bug).
        let jan = snap_anchor(&conn, HeatRange::Month, "2026-01-31").unwrap();
        assert_eq!(jan, "2026-01-01");
        assert_eq!(
            step_anchor(&conn, HeatRange::Month, &jan, 1).unwrap(),
            "2026-02-01"
        );
        assert_eq!(
            step_anchor(&conn, HeatRange::Month, "2026-03-01", -1).unwrap(),
            "2026-02-01"
        );
        assert_eq!(
            step_anchor(&conn, HeatRange::Week, "2026-09-07", 1).unwrap(),
            "2026-09-14"
        );
        assert_eq!(
            step_anchor(&conn, HeatRange::Year, "2026-01-01", -1).unwrap(),
            "2025-01-01"
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
