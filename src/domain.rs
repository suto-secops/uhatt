//! Plain-Rust domain types. No Qt types appear here or in `src/db/`, so the
//! whole layer is exercised directly by `cargo test` against an in-memory
//! database. The Qt bridge (`crate::tasks_model`) is a thin adapter on top.

use uuid::Uuid;

/// Stable identifier for every persisted row. UUIDv7 keeps rows sortable by
/// creation time and makes the future bulk-import feature an insert rather than
/// a migration.
pub type Id = String;

/// Generate a fresh row id.
pub fn new_id() -> Id {
    Uuid::now_v7().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Todo,
    Done,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Todo => "todo",
            TaskStatus::Done => "done",
        }
    }

    /// Parse a stored status, defaulting unknown values to `Todo`.
    pub fn from_db(s: &str) -> Self {
        match s {
            "done" => TaskStatus::Done,
            _ => TaskStatus::Todo,
        }
    }
}

/// A task. `parent_id` gives subtask nesting (self-reference); `project_id`
/// groups tasks under a project. `tracked` is a display flag only ("show the
/// time graph for this row") - time invested is always derived by summing time
/// entries, never accumulated on the row.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: Id,
    pub parent_id: Option<Id>,
    pub project_id: Option<Id>,
    pub title: String,
    pub notes: String,
    pub deadline: Option<String>,
    pub tracked: bool,
    pub status: TaskStatus,
    pub sort_order: f64,
    pub created_at: String,
}

impl Task {
    pub fn is_done(&self) -> bool {
        self.status == TaskStatus::Done
    }
}
