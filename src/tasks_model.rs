//! `TaskListModel` - a `QAbstractListModel` that exposes the task tree to QML
//! as a flattened, depth-annotated list.
//!
//! The full tree (pre-ordered, from SQLite) lives in `tree`; `visible` is the
//! subset of row indices currently shown, recomputed whenever the collapsed
//! set changes. `data()` only ever reads these in-memory structures - Qt calls
//! it once per visible cell per repaint. Every mutation writes SQLite first,
//! then reloads inside `beginResetModel`/`endResetModel`. Reset-per-change is
//! fine at milestone-1 scale; precise row signals can replace it later without
//! touching QML.

use core::pin::Pin;
use std::collections::HashSet;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use rusqlite::Connection;

use crate::db::{self, TaskNode};
use crate::domain::{ProjectFilter, TaskStatus};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!(<QtCore/QAbstractListModel>);
        /// Qt base class.
        type QAbstractListModel;
    }

    unsafe extern "C++" {
        include!("cxx-qt-lib/qhash.h");
        /// `QHash<i32, QByteArray>` from cxx-qt-lib.
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;

        include!("cxx-qt-lib/qvariant.h");
        /// `QVariant` from cxx-qt-lib.
        type QVariant = cxx_qt_lib::QVariant;

        include!("cxx-qt-lib/qmodelindex.h");
        /// `QModelIndex` from cxx-qt-lib.
        type QModelIndex = cxx_qt_lib::QModelIndex;

        include!("cxx-qt-lib/qstring.h");
        /// `QString` from cxx-qt-lib.
        type QString = cxx_qt_lib::QString;
    }

    /// Item roles exposed to QML delegates.
    #[qenum(TaskListModel)]
    enum TaskRole {
        Id,
        Title,
        Done,
        /// Nesting level (0 = root).
        Depth,
        /// Whether this task has any subtasks.
        HasChildren,
        /// Whether this task's subtasks are currently shown.
        Expanded,
        /// ISO `YYYY-MM-DD` deadline, or "" if unset.
        Deadline,
        /// Deadline is past and the task isn't done.
        Overdue,
        /// Free-text notes / description.
        Notes,
        /// Whether the row is ticked in multi-select mode.
        Selected,
        /// One char per indent column, "1" where a tree guide line runs
        /// full-height, "0" where it stops at this row's connector.
        BranchMask,
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        // "" = active tasks, "unfiled" = active + no project, "finished" = the
        // completed-tasks list, otherwise a project id (active tasks in it).
        #[qproperty(QString, project_filter, cxx_name = "projectFilter", READ, WRITE = set_project_filter, NOTIFY)]
        // When true, completed tasks also show in the "", "unfiled" and project
        // views. The "finished" view is unaffected (it is always only done).
        #[qproperty(bool, show_done, cxx_name = "showDone", READ, WRITE = set_show_done, NOTIFY)]
        // Time recorded across every task in the current view's scope, e.g.
        // "18h 40m" ("" while on the finished list). Driven by the model.
        #[qproperty(QString, view_total_text, cxx_name = "viewTotalText")]
        // Multi-select: while on, rows show a tick box for mass delete / revert.
        #[qproperty(bool, selection_mode, cxx_name = "selectionMode", READ, WRITE = set_selection_mode, NOTIFY)]
        // How many rows are ticked. Driven by the model.
        #[qproperty(i32, selected_count, cxx_name = "selectedCount")]
        type TaskListModel = super::TaskListModelRust;
    }

    impl cxx_qt::Initialize for TaskListModel {}

    extern "RustQt" {
        /// Change which project's tasks are shown and reload.
        #[cxx_name = "setProjectFilter"]
        fn set_project_filter(self: Pin<&mut TaskListModel>, value: QString);

        /// Toggle whether completed tasks appear in the normal views.
        #[cxx_name = "setShowDone"]
        fn set_show_done(self: Pin<&mut TaskListModel>, value: bool);

        /// Enter / leave multi-select mode (leaving clears the selection).
        #[cxx_name = "setSelectionMode"]
        fn set_selection_mode(self: Pin<&mut TaskListModel>, value: bool);

        /// Tick / untick the row at `row`.
        #[qinvokable]
        #[cxx_name = "toggleSelected"]
        fn toggle_selected(self: Pin<&mut TaskListModel>, row: i32);

        /// Tick every visible row.
        #[qinvokable]
        #[cxx_name = "selectAll"]
        fn select_all(self: Pin<&mut TaskListModel>);

        /// Untick everything.
        #[qinvokable]
        #[cxx_name = "clearSelection"]
        fn clear_selection(self: Pin<&mut TaskListModel>);

        /// Delete every ticked task (subtasks cascade), then leave select mode.
        #[qinvokable]
        #[cxx_name = "deleteSelected"]
        fn delete_selected(self: Pin<&mut TaskListModel>);

        /// Mark every ticked task not-done (it returns to its project), then
        /// leave select mode.
        #[qinvokable]
        #[cxx_name = "revertSelected"]
        fn revert_selected(self: Pin<&mut TaskListModel>);

        /// Append a new root task in the current project filter (if any). No-op on blank input.
        #[qinvokable]
        fn add(self: Pin<&mut TaskListModel>, title: &QString);

        /// Move the task at `row` (and its subtree) to `project_id`, or to
        /// unfiled when `project_id` is empty.
        #[qinvokable]
        #[cxx_name = "moveToProject"]
        fn move_to_project(self: Pin<&mut TaskListModel>, row: i32, project_id: &QString);

        /// Re-parent the task at `row` (and its subtree) under `target_id`.
        /// No-op if the move would create a cycle.
        #[qinvokable]
        fn reparent(self: Pin<&mut TaskListModel>, row: i32, target_id: &QString);

        /// Whether [`reparent`] with these arguments would do anything - used to
        /// light up a valid drop target while dragging.
        #[qinvokable]
        #[cxx_name = "canReparent"]
        fn can_reparent(self: &TaskListModel, row: i32, target_id: &QString) -> bool;

        /// Add a subtask under the task at `row`, expanding it. No-op on blank input.
        #[qinvokable]
        #[cxx_name = "addChild"]
        fn add_child(self: Pin<&mut TaskListModel>, row: i32, title: &QString);

        /// Remove the task at `row` (subtasks cascade).
        #[qinvokable]
        fn remove(self: Pin<&mut TaskListModel>, row: i32);

        /// Set the done state of the task at `row`.
        #[qinvokable]
        #[cxx_name = "setDone"]
        fn set_done(self: Pin<&mut TaskListModel>, row: i32, done: bool);

        /// Rename the task at `row`. No-op on blank input.
        #[qinvokable]
        fn rename(self: Pin<&mut TaskListModel>, row: i32, title: &QString);

        /// Replace the notes / description of the task at `row`.
        #[qinvokable]
        #[cxx_name = "setNotes"]
        fn set_notes(self: Pin<&mut TaskListModel>, row: i32, notes: &QString);

        /// A display string for the task at `row`'s time invested - its own,
        /// plus "(incl. subtasks: …)" when it has any.
        #[qinvokable]
        #[cxx_name = "timeInvestedText"]
        fn time_invested_text(self: &TaskListModel, row: i32) -> QString;

        /// Set the task's deadline to an ISO `YYYY-MM-DD` date, or clear it when
        /// `deadline` is empty. Malformed dates are ignored.
        #[qinvokable]
        #[cxx_name = "setDeadline"]
        fn set_deadline(self: Pin<&mut TaskListModel>, row: i32, deadline: &QString);

        /// Collapse an expanded task or expand a collapsed one.
        #[qinvokable]
        #[cxx_name = "toggleExpanded"]
        fn toggle_expanded(self: Pin<&mut TaskListModel>, row: i32);

        /// Re-read from SQLite. For picking up a mutation made through
        /// another QObject (e.g. marking a task done from the calendar page)
        /// - every mutation here already reloads itself.
        #[qinvokable]
        fn refresh(self: Pin<&mut TaskListModel>);
    }

    // QAbstractListModel overrides.
    extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &TaskListModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &TaskListModel) -> QHash_i32_QByteArray;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &TaskListModel, parent: &QModelIndex) -> i32;
    }

    // Inherited begin/end reset helpers from QAbstractItemModel.
    extern "RustQt" {
        /// # Safety
        /// Must be paired with `end_reset_model`.
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut TaskListModel>);

        /// # Safety
        /// Must follow a `begin_reset_model`.
        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut TaskListModel>);
    }
}

/// Backing state for [`qobject::TaskListModel`].
#[derive(Default)]
pub struct TaskListModelRust {
    conn: Option<Connection>,
    /// Backs the `projectFilter` Q_PROPERTY (see [`parse_filter`]).
    project_filter: QString,
    /// Backs the `showDone` Q_PROPERTY.
    show_done: bool,
    /// Backs the `viewTotalText` Q_PROPERTY.
    view_total_text: QString,
    /// Backs the `selectionMode` Q_PROPERTY.
    selection_mode: bool,
    /// Backs the `selectedCount` Q_PROPERTY.
    selected_count: i32,
    /// Task ids ticked in multi-select mode.
    selected: HashSet<String>,
    tree: Vec<TaskNode>,
    collapsed: HashSet<String>,
    /// Indices into `tree` that are currently visible, in display order.
    visible: Vec<usize>,
}

/// The special `projectFilter` value that selects the completed-tasks list.
const FINISHED: &str = "finished";
/// The special `projectFilter` value for the deadline-bearing subset of the
/// tree (a task and its ancestor chain, kept when it or a descendant has a
/// deadline).
const DEADLINED: &str = "deadlined";

/// Seconds as `"0m"` / `"45m"` / `"6h 20m"`, for the view total line.
fn human_hm(secs: i64) -> String {
    if secs <= 0 {
        return "0m".to_owned();
    }
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h == 0 {
        format!("{m}m")
    } else {
        format!("{h}h {m:02}m")
    }
}

/// Interpret a `projectFilter` string as a project scope. The special values
/// (`finished`, `deadlined`) are handled by the caller before this point.
fn parse_filter(s: &str) -> ProjectFilter {
    match s {
        "" => ProjectFilter::All,
        "unfiled" => ProjectFilter::Unfiled,
        id => ProjectFilter::Only(id.to_owned()),
    }
}

/// Walk a pre-ordered tree and return the indices whose ancestors are all
/// expanded. A collapsed node stays visible; its descendants do not.
fn compute_visible(tree: &[TaskNode], collapsed: &HashSet<String>) -> Vec<usize> {
    let mut visible = Vec::with_capacity(tree.len());
    let mut hidden_below: Option<u32> = None;
    for (i, node) in tree.iter().enumerate() {
        if let Some(depth) = hidden_below {
            if node.depth > depth {
                continue;
            }
            hidden_below = None;
        }
        visible.push(i);
        if node.has_children && collapsed.contains(&node.task.id) {
            hidden_below = Some(node.depth);
        }
    }
    visible
}

impl cxx_qt::Initialize for qobject::TaskListModel {
    fn initialize(mut self: Pin<&mut Self>) {
        let conn = match db::open(&db::default_path()) {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("uhatt: could not open database ({e}); running in-memory");
                db::open_in_memory().expect("in-memory database")
            }
        };
        let tree = db::list_task_tree(&conn, &ProjectFilter::All, false).unwrap_or_default();
        let visible = compute_visible(&tree, &HashSet::new());
        let total = db::scope_seconds(&conn, &ProjectFilter::All).unwrap_or(0);
        {
            let mut rust = self.as_mut().rust_mut();
            rust.conn = Some(conn);
            rust.tree = tree;
            rust.visible = visible;
        }
        self.as_mut()
            .set_view_total_text(QString::from(human_hm(total).as_str()));
    }
}

impl qobject::TaskListModel {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("TaskListModel used before initialize()")
    }

    fn node_at(&self, row: i32) -> Option<&TaskNode> {
        let row = usize::try_from(row).ok()?;
        let idx = *self.visible.get(row)?;
        self.tree.get(idx)
    }

    fn id_at(&self, row: i32) -> Option<String> {
        self.node_at(row).map(|n| n.task.id.clone())
    }

    /// Re-read the tree from SQLite, drop stale collapsed ids, recompute the
    /// visible set, all wrapped in a model reset.
    fn reload(mut self: Pin<&mut Self>) {
        let filter_str = self.project_filter.to_string();
        let tree = if filter_str == FINISHED {
            db::list_finished_tasks(self.db_conn())
        } else if filter_str == DEADLINED {
            db::list_deadlined_tree(self.db_conn())
        } else {
            db::list_task_tree(self.db_conn(), &parse_filter(&filter_str), self.show_done)
        }
        .unwrap_or_default();
        // Time total for the current scope; blank on the ad-hoc views
        // (finished / deadlined) where a scope total isn't meaningful.
        let view_total = if filter_str == FINISHED || filter_str == DEADLINED {
            String::new()
        } else {
            let secs = db::scope_seconds(self.db_conn(), &parse_filter(&filter_str)).unwrap_or(0);
            human_hm(secs)
        };

        let live: HashSet<&str> = tree.iter().map(|n| n.task.id.as_str()).collect();
        let collapsed: HashSet<String> = self
            .collapsed
            .iter()
            .filter(|id| live.contains(id.as_str()))
            .cloned()
            .collect();
        let selected: HashSet<String> = self
            .selected
            .iter()
            .filter(|id| live.contains(id.as_str()))
            .cloned()
            .collect();
        let selected_count = selected.len() as i32;
        let visible = compute_visible(&tree, &collapsed);
        // SAFETY: begin/end are paired around the state swap.
        unsafe {
            self.as_mut().begin_reset_model();
            {
                let mut rust = self.as_mut().rust_mut();
                rust.tree = tree;
                rust.collapsed = collapsed;
                rust.selected = selected;
                rust.visible = visible;
            }
            self.as_mut().end_reset_model();
        }
        self.as_mut()
            .set_view_total_text(QString::from(view_total.as_str()));
        self.as_mut().set_selected_count(selected_count);
    }

    fn set_project_filter(mut self: Pin<&mut Self>, value: QString) {
        if self.project_filter == value {
            return;
        }
        self.as_mut().rust_mut().project_filter = value;
        // Custom WRITE setters must emit the change themselves - cxx-qt only
        // auto-emits for auto-generated setters. Without this the sidebar
        // highlight (bound to `projectFilter`) never follows the selection.
        self.as_mut().project_filter_changed();
        self.reload();
    }

    fn set_show_done(mut self: Pin<&mut Self>, value: bool) {
        if self.show_done == value {
            return;
        }
        self.as_mut().rust_mut().show_done = value;
        self.as_mut().show_done_changed();
        self.reload();
    }

    // ---- Multi-select ---------------------------------------------------

    /// Reset the model around a change to the selection set, then republish
    /// `selectedCount`. No SQLite read - the tree is untouched.
    fn selection_changed(mut self: Pin<&mut Self>) {
        let count = self.selected.len() as i32;
        // SAFETY: begin/end are paired; `data()` reads `selected` for its role.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().end_reset_model();
        }
        self.as_mut().set_selected_count(count);
    }

    fn set_selection_mode(mut self: Pin<&mut Self>, value: bool) {
        if self.selection_mode == value {
            return;
        }
        {
            let mut rust = self.as_mut().rust_mut();
            rust.selection_mode = value;
            if !value {
                rust.selected.clear();
            }
        }
        self.as_mut().selection_mode_changed();
        self.as_mut().selection_changed();
    }

    fn toggle_selected(mut self: Pin<&mut Self>, row: i32) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        {
            let mut rust = self.as_mut().rust_mut();
            if !rust.selected.remove(&id) {
                rust.selected.insert(id);
            }
        }
        self.as_mut().selection_changed();
    }

    fn select_all(mut self: Pin<&mut Self>) {
        let ids: Vec<String> = self
            .visible
            .iter()
            .filter_map(|&i| self.tree.get(i))
            .map(|n| n.task.id.clone())
            .collect();
        self.as_mut().rust_mut().selected.extend(ids);
        self.as_mut().selection_changed();
    }

    fn clear_selection(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().selected.clear();
        self.as_mut().selection_changed();
    }

    fn delete_selected(mut self: Pin<&mut Self>) {
        let ids: Vec<String> = self.selected.iter().cloned().collect();
        for id in &ids {
            if let Err(e) = db::delete_task(self.db_conn(), id) {
                eprintln!("uhatt: bulk delete failed for {id}: {e}");
            }
        }
        {
            let mut rust = self.as_mut().rust_mut();
            rust.selected.clear();
            rust.selection_mode = false;
        }
        self.as_mut().selection_mode_changed();
        self.as_mut().set_selected_count(0);
        self.reload();
    }

    fn revert_selected(mut self: Pin<&mut Self>) {
        let ids: Vec<String> = self.selected.iter().cloned().collect();
        for id in &ids {
            if let Err(e) = db::set_task_status(self.db_conn(), id, TaskStatus::Todo) {
                eprintln!("uhatt: bulk revert failed for {id}: {e}");
            }
        }
        {
            let mut rust = self.as_mut().rust_mut();
            rust.selected.clear();
            rust.selection_mode = false;
        }
        self.as_mut().selection_mode_changed();
        self.as_mut().set_selected_count(0);
        self.reload();
    }

    fn add(self: Pin<&mut Self>, title: &QString) {
        let title = title.to_string();
        let title = title.trim();
        let filter_str = self.project_filter.to_string();
        if title.is_empty() || filter_str == FINISHED || filter_str == DEADLINED {
            return;
        }
        // New root tasks land in the currently filtered project, if any.
        let project = match parse_filter(&filter_str) {
            ProjectFilter::Only(id) => Some(id),
            _ => None,
        };
        if let Err(e) = db::create_task(self.db_conn(), title, None, project.as_deref()) {
            eprintln!("uhatt: add task failed: {e}");
            return;
        }
        self.reload();
    }

    fn add_child(mut self: Pin<&mut Self>, row: i32, title: &QString) {
        let title = title.to_string();
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        let Some(parent_id) = self.id_at(row) else {
            return;
        };
        // Subtasks inherit the parent's project, so pass None here.
        if let Err(e) = db::create_task(self.db_conn(), title, Some(&parent_id), None) {
            eprintln!("uhatt: add subtask failed: {e}");
            return;
        }
        // Make sure the new child is visible.
        self.as_mut().rust_mut().collapsed.remove(&parent_id);
        self.reload();
    }

    fn move_to_project(self: Pin<&mut Self>, row: i32, project_id: &QString) {
        let Some(task_id) = self.id_at(row) else {
            return;
        };
        let project_id = project_id.to_string();
        let target = (!project_id.is_empty()).then_some(project_id.as_str());
        if let Err(e) = db::set_task_project(self.db_conn(), &task_id, target) {
            eprintln!("uhatt: move task to project failed: {e}");
            return;
        }
        self.reload();
    }

    fn reparent(self: Pin<&mut Self>, row: i32, target_id: &QString) {
        let Some(task_id) = self.id_at(row) else {
            return;
        };
        let target = target_id.to_string();
        if target.is_empty() {
            return;
        }
        if let Err(e) = db::reparent_task(self.db_conn(), &task_id, &target) {
            eprintln!("uhatt: reparent failed: {e}");
            return;
        }
        self.reload();
    }

    fn can_reparent(&self, row: i32, target_id: &QString) -> bool {
        let Some(task_id) = self.id_at(row) else {
            return false;
        };
        let target = target_id.to_string();
        !target.is_empty() && db::can_reparent(self.db_conn(), &task_id, &target).unwrap_or(false)
    }

    fn remove(self: Pin<&mut Self>, row: i32) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        if let Err(e) = db::delete_task(self.db_conn(), &id) {
            eprintln!("uhatt: delete task failed: {e}");
            return;
        }
        self.reload();
    }

    fn set_done(self: Pin<&mut Self>, row: i32, done: bool) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        let status = if done {
            TaskStatus::Done
        } else {
            TaskStatus::Todo
        };
        if let Err(e) = db::set_task_status(self.db_conn(), &id, status) {
            eprintln!("uhatt: set task status failed: {e}");
            return;
        }
        self.reload();
    }

    fn rename(self: Pin<&mut Self>, row: i32, title: &QString) {
        let title = title.to_string();
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        let Some(id) = self.id_at(row) else {
            return;
        };
        if let Err(e) = db::rename_task(self.db_conn(), &id, title) {
            eprintln!("uhatt: rename task failed: {e}");
            return;
        }
        self.reload();
    }

    fn set_notes(self: Pin<&mut Self>, row: i32, notes: &QString) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        if let Err(e) = db::set_task_notes(self.db_conn(), &id, &notes.to_string()) {
            eprintln!("uhatt: set task notes failed: {e}");
            return;
        }
        self.reload();
    }

    fn time_invested_text(&self, row: i32) -> QString {
        let Some(id) = self.id_at(row) else {
            return QString::default();
        };
        let conn = self.db_conn();
        let own = db::task_seconds(conn, &id, false).unwrap_or(0);
        let text = if db::has_subtasks(conn, &id).unwrap_or(false) {
            let sub = db::task_seconds(conn, &id, true).unwrap_or(own);
            format!("{}  (incl. subtasks: {})", human_hm(own), human_hm(sub))
        } else {
            human_hm(own)
        };
        QString::from(text.as_str())
    }

    fn set_deadline(self: Pin<&mut Self>, row: i32, deadline: &QString) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        let deadline = deadline.to_string();
        let deadline = deadline.trim();
        let target = (!deadline.is_empty()).then_some(deadline);
        if let Err(e) = db::set_task_deadline(self.db_conn(), &id, target) {
            eprintln!("uhatt: set deadline failed: {e}");
            return;
        }
        self.reload();
    }

    fn toggle_expanded(mut self: Pin<&mut Self>, row: i32) {
        let Some(node) = self.node_at(row) else {
            return;
        };
        if !node.has_children {
            return;
        }
        let id = node.task.id.clone();
        {
            let mut rust = self.as_mut().rust_mut();
            if !rust.collapsed.remove(&id) {
                rust.collapsed.insert(id);
            }
        }
        let visible = compute_visible(&self.tree, &self.collapsed);
        // SAFETY: begin/end are paired around the visible-set swap.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().visible = visible;
            self.as_mut().end_reset_model();
        }
    }

    fn refresh(self: Pin<&mut Self>) {
        self.reload();
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(node) = self.node_at(index.row()) else {
            return QVariant::default();
        };
        let task = &node.task;

        match (qobject::TaskRole { repr: role }) {
            qobject::TaskRole::Id => QVariant::from(&QString::from(task.id.as_str())),
            qobject::TaskRole::Title => QVariant::from(&QString::from(task.title.as_str())),
            qobject::TaskRole::Done => QVariant::from(&task.is_done()),
            qobject::TaskRole::Depth => QVariant::from(&(node.depth as i32)),
            qobject::TaskRole::HasChildren => QVariant::from(&node.has_children),
            qobject::TaskRole::Expanded => {
                QVariant::from(&(node.has_children && !self.collapsed.contains(&task.id)))
            }
            qobject::TaskRole::Deadline => {
                QVariant::from(&QString::from(task.deadline.as_deref().unwrap_or("")))
            }
            qobject::TaskRole::Overdue => QVariant::from(&node.overdue),
            qobject::TaskRole::Notes => QVariant::from(&QString::from(task.notes.as_str())),
            qobject::TaskRole::Selected => QVariant::from(&self.selected.contains(&task.id)),
            qobject::TaskRole::BranchMask => {
                let mask: String = node
                    .branch_more
                    .iter()
                    .map(|&more| if more { '1' } else { '0' })
                    .collect();
                QVariant::from(&QString::from(mask.as_str()))
            }
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        roles.insert(qobject::TaskRole::Id.repr, QByteArray::from("id"));
        roles.insert(qobject::TaskRole::Title.repr, QByteArray::from("title"));
        roles.insert(qobject::TaskRole::Done.repr, QByteArray::from("done"));
        roles.insert(qobject::TaskRole::Depth.repr, QByteArray::from("depth"));
        roles.insert(
            qobject::TaskRole::HasChildren.repr,
            QByteArray::from("hasChildren"),
        );
        roles.insert(
            qobject::TaskRole::Expanded.repr,
            QByteArray::from("expanded"),
        );
        roles.insert(
            qobject::TaskRole::Deadline.repr,
            QByteArray::from("deadline"),
        );
        roles.insert(qobject::TaskRole::Overdue.repr, QByteArray::from("overdue"));
        roles.insert(qobject::TaskRole::Notes.repr, QByteArray::from("notes"));
        roles.insert(
            qobject::TaskRole::Selected.repr,
            QByteArray::from("selected"),
        );
        roles.insert(
            qobject::TaskRole::BranchMask.repr,
            QByteArray::from("branchMask"),
        );
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.visible.len()).unwrap_or(i32::MAX)
    }
}
