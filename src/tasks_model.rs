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
use crate::domain::{Periodicity, ProjectFilter, Task, TaskStatus};

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
        /// Owning project's name, or "" when the task is unfiled.
        ProjectName,
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
        // Bumps on every reload; QML reads it (via the comma-operator idiom,
        // e.g. `(tasks.dataVersion, tasks.countAll())`) to force a live
        // re-evaluation of the count invokables below without any per-view
        // cache to keep in sync. Same idiom as `Calendar.revision`.
        #[qproperty(i32, data_version, cxx_name = "dataVersion")]
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

        /// The task at `row`'s one-off starting duration (set by quick
        /// creation), formatted, or "" when it has none.
        #[qinvokable]
        #[cxx_name = "initialTimeText"]
        fn initial_time_text(self: &TaskListModel, row: i32) -> QString;

        /// Set the task's deadline to an ISO `YYYY-MM-DD` date, or clear it when
        /// `deadline` is empty. Malformed dates are ignored.
        #[qinvokable]
        #[cxx_name = "setDeadline"]
        fn set_deadline(self: Pin<&mut TaskListModel>, row: i32, deadline: &QString);

        // ---- Habits (periodicity dropdown, under the deadline row) --------

        /// This task's own periodicity kind code (`0` = Off / not set,
        /// `1..=6` per `Periodicity::kind_code`) - never the inherited one,
        /// since the dropdown always edits what's set directly on this row.
        #[qinvokable]
        #[cxx_name = "periodicityKind"]
        fn periodicity_kind(self: &TaskListModel, row: i32) -> i32;

        /// The `n` in "every n days/weeks/months" for this task's own
        /// periodicity, or `0` when not applicable.
        #[qinvokable]
        #[cxx_name = "periodicityN"]
        fn periodicity_n(self: &TaskListModel, row: i32) -> i32;

        /// This task's own weekday selection as `"1,3,5"` (ISO numbering,
        /// 1 = Monday), or `""` when not applicable.
        #[qinvokable]
        #[cxx_name = "periodicityWeekdays"]
        fn periodicity_weekdays(self: &TaskListModel, row: i32) -> QString;

        /// A human summary of what governs this task - its own periodicity,
        /// or its nearest ancestor's with " (from <ancestor's title>)"
        /// appended, or "" when it isn't part of a habit at all.
        #[qinvokable]
        #[cxx_name = "effectivePeriodicityText"]
        fn effective_periodicity_text(self: &TaskListModel, row: i32) -> QString;

        /// How many scheduled occurrences have already passed while this
        /// task sat on its current deadline - `0` when it isn't a habit, has
        /// no deadline yet, or is on time.
        #[qinvokable]
        #[cxx_name = "missedCount"]
        fn missed_count(self: &TaskListModel, row: i32) -> i32;

        /// Set (`kind > 0`) or clear (`kind <= 0`) the periodicity on the
        /// task at `row` directly - see `Periodicity::from_parts` for how
        /// `kind`/`n`/`weekdays` combine.
        #[qinvokable]
        #[cxx_name = "setPeriodicity"]
        fn set_periodicity(
            self: Pin<&mut TaskListModel>,
            row: i32,
            kind: i32,
            n: i32,
            weekdays: &QString,
        );

        /// Collapse an expanded task or expand a collapsed one.
        #[qinvokable]
        #[cxx_name = "toggleExpanded"]
        fn toggle_expanded(self: Pin<&mut TaskListModel>, row: i32);

        /// Re-read from SQLite. For picking up a mutation made through
        /// another QObject (e.g. marking a task done from the calendar page)
        /// - every mutation here already reloads itself.
        #[qinvokable]
        fn refresh(self: Pin<&mut TaskListModel>);

        /// Count of tasks in "All tasks" (subject to `showDone`).
        #[qinvokable]
        #[cxx_name = "countAll"]
        fn count_all(self: &TaskListModel) -> i32;

        /// Count of tasks with no project (subject to `showDone`).
        #[qinvokable]
        #[cxx_name = "countUnfiled"]
        fn count_unfiled(self: &TaskListModel) -> i32;

        /// Count of tasks in one project (subject to `showDone`).
        #[qinvokable]
        #[cxx_name = "projectTaskCount"]
        fn project_task_count(self: &TaskListModel, project_id: &QString) -> i32;

        /// Count of tasks shown on the "Due today" view.
        #[qinvokable]
        #[cxx_name = "countDueToday"]
        fn count_due_today(self: &TaskListModel) -> i32;

        /// Count of tasks shown on the "Deadlined" view.
        #[qinvokable]
        #[cxx_name = "countDeadlined"]
        fn count_deadlined(self: &TaskListModel) -> i32;

        /// Count of tasks shown on the "Finished" view.
        #[qinvokable]
        #[cxx_name = "countFinished"]
        fn count_finished(self: &TaskListModel) -> i32;
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
    /// Backs the `dataVersion` Q_PROPERTY.
    data_version: i32,
}

/// The special `projectFilter` value that selects the completed-tasks list.
const FINISHED: &str = "finished";
/// The special `projectFilter` value for the deadline-bearing subset of the
/// tree (a task and its ancestor chain, kept when it or a descendant has a
/// deadline).
const DEADLINED: &str = "deadlined";
/// The special `projectFilter` value for tasks due the current local day
/// (same ancestor-chain-preserving rule as [`DEADLINED`]).
const DUE_TODAY: &str = "duetoday";

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

/// A human summary of a periodicity, for the "governed by" note under the
/// deadline row.
fn periodicity_summary(p: &Periodicity) -> String {
    match p {
        Periodicity::EveryDays { n: 1 } => "Every day".to_owned(),
        Periodicity::EveryDays { n } => format!("Every {n} days"),
        Periodicity::EveryWeeks { n: 1 } => "Every week".to_owned(),
        Periodicity::EveryWeeks { n } => format!("Every {n} weeks"),
        Periodicity::EveryMonths { n: 1 } => "Every month".to_owned(),
        Periodicity::EveryMonths { n } => format!("Every {n} months"),
        Periodicity::Weekdays { days } => {
            const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
            let names: Vec<&str> = days
                .iter()
                .filter_map(|&d| NAMES.get(usize::from(d.saturating_sub(1))))
                .copied()
                .collect();
            format!("Every {}", names.join(", "))
        }
        Periodicity::FirstOfMonth => "First of the month".to_owned(),
        Periodicity::LastOfMonth => "Last of the month".to_owned(),
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

    /// The task as it is right now, for capturing "before" state ahead of a
    /// mutation - the action log needs the old title/notes/deadline/status,
    /// not what's about to be written.
    fn task_before(&self, id: &str) -> Option<Task> {
        db::get_task(self.db_conn(), id).ok().flatten()
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
        } else if filter_str == DUE_TODAY {
            db::list_due_today_tree(self.db_conn())
        } else {
            db::list_task_tree(self.db_conn(), &parse_filter(&filter_str), self.show_done)
        }
        .unwrap_or_default();
        // Time total for the current scope; blank on the ad-hoc views
        // (finished / deadlined / due-today) where a scope total isn't
        // meaningful.
        let view_total =
            if filter_str == FINISHED || filter_str == DEADLINED || filter_str == DUE_TODAY {
                String::new()
            } else {
                let secs =
                    db::scope_seconds(self.db_conn(), &parse_filter(&filter_str)).unwrap_or(0);
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
        let next = self.data_version.wrapping_add(1);
        self.as_mut().set_data_version(next);
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
            if let Err(e) = db::actions::capture_and_log_delete(self.db_conn(), id) {
                eprintln!("uhatt: log delete failed for {id}: {e}");
            }
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
            match db::actions::revert_habit_completion(self.db_conn(), id) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    eprintln!("uhatt: revert habit completion failed for {id}: {e}");
                    continue;
                }
            }
            let before = self.task_before(id);
            if let Err(e) = db::set_task_status(self.db_conn(), id, TaskStatus::Todo) {
                eprintln!("uhatt: bulk revert failed for {id}: {e}");
                continue;
            }
            if let Some(t) = before {
                if let Err(e) =
                    db::actions::log_set_status(self.db_conn(), id, &t.title, t.status, false)
                {
                    eprintln!("uhatt: log set_status failed for {id}: {e}");
                }
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
        if title.is_empty()
            || filter_str == FINISHED
            || filter_str == DEADLINED
            || filter_str == DUE_TODAY
        {
            return;
        }
        // New root tasks land in the currently filtered project, if any.
        let project = match parse_filter(&filter_str) {
            ProjectFilter::Only(id) => Some(id),
            _ => None,
        };
        let task = match db::create_task(self.db_conn(), title, None, project.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("uhatt: add task failed: {e}");
                return;
            }
        };
        if let Err(e) = db::actions::log_create_task(self.db_conn(), &task) {
            eprintln!("uhatt: log create_task failed: {e}");
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
        let task = match db::create_task(self.db_conn(), title, Some(&parent_id), None) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("uhatt: add subtask failed: {e}");
                return;
            }
        };
        if let Err(e) = db::actions::log_create_task(self.db_conn(), &task) {
            eprintln!("uhatt: log create_task failed: {e}");
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
        let title = self.task_before(&task_id).map(|t| t.title);
        let old_map = db::actions::capture_project_ids(self.db_conn(), &task_id).ok();
        if let Err(e) = db::set_task_project(self.db_conn(), &task_id, target) {
            eprintln!("uhatt: move task to project failed: {e}");
            return;
        }
        if let (Some(title), Some(old_map)) = (title, old_map) {
            let new_name = target
                .and_then(|id| db::get_project(self.db_conn(), id).ok().flatten())
                .map(|p| p.name)
                .unwrap_or_default();
            if let Err(e) =
                db::actions::log_set_project(self.db_conn(), &task_id, &title, &old_map, &new_name)
            {
                eprintln!("uhatt: log set_project failed: {e}");
            }
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
        let before = self.task_before(&task_id);
        let old_map = db::actions::capture_project_ids(self.db_conn(), &task_id).ok();
        let new_parent_title = self.task_before(&target).map(|t| t.title);
        if let Err(e) = db::reparent_task(self.db_conn(), &task_id, &target) {
            eprintln!("uhatt: reparent failed: {e}");
            return;
        }
        // `reparent_task` silently no-ops when `can_reparent` would reject
        // the move (cycle, ghost id, ...) - re-read rather than assume it
        // happened, so a rejected drop doesn't get logged as a real action.
        let changed = self
            .task_before(&task_id)
            .zip(before.as_ref())
            .is_some_and(|(after, t)| {
                after.parent_id != t.parent_id || after.sort_order != t.sort_order
            });
        if changed {
            if let (Some(t), Some(old_map), Some(new_parent_title)) =
                (before, old_map, new_parent_title)
            {
                if let Err(e) = db::actions::log_reparent(
                    self.db_conn(),
                    &task_id,
                    &t.title,
                    t.parent_id.as_deref(),
                    t.sort_order,
                    &old_map,
                    &new_parent_title,
                ) {
                    eprintln!("uhatt: log reparent failed: {e}");
                }
            }
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
        if let Err(e) = db::actions::capture_and_log_delete(self.db_conn(), &id) {
            eprintln!("uhatt: log delete failed: {e}");
        }
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
        if done {
            // A habit's regeneration unit is its whole subtree, not just this
            // task - see `db::actions::complete_recurring_task`.
            if let Ok(Some(spec)) = db::effective_periodicity(self.db_conn(), &id) {
                if let Err(e) = db::actions::complete_recurring_task(self.db_conn(), &id, &spec) {
                    eprintln!("uhatt: complete recurring task failed: {e}");
                }
                self.reload();
                return;
            }
        } else {
            match db::actions::revert_habit_completion(self.db_conn(), &id) {
                Ok(true) => {
                    self.reload();
                    return;
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!("uhatt: revert habit completion failed: {e}");
                    return;
                }
            }
        }
        let status = if done {
            TaskStatus::Done
        } else {
            TaskStatus::Todo
        };
        let before = self.task_before(&id);
        if let Err(e) = db::set_task_status(self.db_conn(), &id, status) {
            eprintln!("uhatt: set task status failed: {e}");
            return;
        }
        if let Some(t) = before {
            if let Err(e) =
                db::actions::log_set_status(self.db_conn(), &id, &t.title, t.status, done)
            {
                eprintln!("uhatt: log set_status failed: {e}");
            }
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
        let before = self.task_before(&id);
        if let Err(e) = db::rename_task(self.db_conn(), &id, title) {
            eprintln!("uhatt: rename task failed: {e}");
            return;
        }
        if let Some(t) = before {
            if t.title != title {
                if let Err(e) = db::actions::log_rename_task(self.db_conn(), &id, &t.title, title) {
                    eprintln!("uhatt: log rename_task failed: {e}");
                }
            }
        }
        self.reload();
    }

    fn set_notes(self: Pin<&mut Self>, row: i32, notes: &QString) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        let before = self.task_before(&id);
        let new_notes = notes.to_string();
        if let Err(e) = db::set_task_notes(self.db_conn(), &id, &new_notes) {
            eprintln!("uhatt: set task notes failed: {e}");
            return;
        }
        if let Some(t) = before {
            if t.notes != new_notes.trim_end() {
                if let Err(e) = db::actions::log_set_notes(self.db_conn(), &id, &t.title, &t.notes)
                {
                    eprintln!("uhatt: log set_notes failed: {e}");
                }
            }
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

    /// The task at `row`'s one-off starting duration (`Task.initial_time_seconds`),
    /// formatted, or "" when unset. Never counted in `time_invested_text` or
    /// any graph total.
    fn initial_time_text(&self, row: i32) -> QString {
        let text = self
            .node_at(row)
            .and_then(|n| n.task.initial_time_seconds)
            .map(human_hm)
            .unwrap_or_default();
        QString::from(text.as_str())
    }

    fn set_deadline(self: Pin<&mut Self>, row: i32, deadline: &QString) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        let deadline = deadline.to_string();
        let deadline = deadline.trim();
        let target = (!deadline.is_empty()).then_some(deadline);
        let before = self.task_before(&id);
        if let Err(e) = db::set_task_deadline(self.db_conn(), &id, target) {
            eprintln!("uhatt: set deadline failed: {e}");
            return;
        }
        // Re-read rather than trust `target`: a malformed date is a silent
        // no-op in `set_task_deadline`, and this must not log a "change"
        // that never actually happened.
        if let (Some(t), Some(after)) = (before, self.task_before(&id)) {
            if t.deadline != after.deadline {
                if let Err(e) = db::actions::log_set_deadline(
                    self.db_conn(),
                    &id,
                    &t.title,
                    t.deadline.as_deref(),
                    after.deadline.as_deref(),
                ) {
                    eprintln!("uhatt: log set_deadline failed: {e}");
                }
            }
        }
        self.reload();
    }

    /// This row's own periodicity (not the effective/inherited one) - the
    /// dropdown always edits what's set directly on this task.
    fn own_periodicity(&self, row: i32) -> Option<Periodicity> {
        self.node_at(row)
            .and_then(|n| n.task.periodicity.as_deref())
            .and_then(Periodicity::from_stored)
    }

    fn periodicity_kind(&self, row: i32) -> i32 {
        self.own_periodicity(row).map_or(0, |p| p.kind_code())
    }

    fn periodicity_n(&self, row: i32) -> i32 {
        self.own_periodicity(row).map_or(0, |p| p.n())
    }

    fn periodicity_weekdays(&self, row: i32) -> QString {
        let text = self
            .own_periodicity(row)
            .map_or_else(String::new, |p| p.weekdays_csv());
        QString::from(text.as_str())
    }

    fn effective_periodicity_text(&self, row: i32) -> QString {
        let Some(id) = self.id_at(row) else {
            return QString::default();
        };
        let text = match db::effective_periodicity(self.db_conn(), &id) {
            Ok(Some(p)) => periodicity_summary(&p),
            _ => String::new(),
        };
        QString::from(text.as_str())
    }

    fn missed_count(&self, row: i32) -> i32 {
        let Some(node) = self.node_at(row) else {
            return 0;
        };
        let Some(deadline) = node.task.deadline.as_deref() else {
            return 0;
        };
        let Ok(Some(spec)) = db::effective_periodicity(self.db_conn(), &node.task.id) else {
            return 0;
        };
        let today = crate::db::today_string(self.db_conn()).unwrap_or_default();
        db::missed_occurrences(self.db_conn(), &spec, deadline, &today).unwrap_or(0) as i32
    }

    fn set_periodicity(self: Pin<&mut Self>, row: i32, kind: i32, n: i32, weekdays: &QString) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        let weekdays = weekdays.to_string();
        let spec = Periodicity::from_parts(kind, n, &weekdays);
        if let Err(e) = db::set_task_periodicity(self.db_conn(), &id, spec.as_ref()) {
            eprintln!("uhatt: set periodicity failed: {e}");
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

    // ---- Sidebar counts ---------------------------------------------------
    //
    // Each re-runs the same query the corresponding view reloads with, rather
    // than a separate `COUNT(*)`, so a count can never drift from what the
    // view it labels actually shows. Cheap enough at this app's scale; QML
    // re-evaluates these on every `dataVersion` bump instead of caching them.

    fn count_all(&self) -> i32 {
        db::list_task_tree(self.db_conn(), &ProjectFilter::All, self.show_done)
            .map(|v| v.len() as i32)
            .unwrap_or(0)
    }

    fn count_unfiled(&self) -> i32 {
        db::list_task_tree(self.db_conn(), &ProjectFilter::Unfiled, self.show_done)
            .map(|v| v.len() as i32)
            .unwrap_or(0)
    }

    fn project_task_count(&self, project_id: &QString) -> i32 {
        let filter = ProjectFilter::Only(project_id.to_string());
        db::list_task_tree(self.db_conn(), &filter, self.show_done)
            .map(|v| v.len() as i32)
            .unwrap_or(0)
    }

    fn count_due_today(&self) -> i32 {
        db::list_due_today_tree(self.db_conn())
            .map(|v| v.len() as i32)
            .unwrap_or(0)
    }

    fn count_deadlined(&self) -> i32 {
        db::list_deadlined_tree(self.db_conn())
            .map(|v| v.len() as i32)
            .unwrap_or(0)
    }

    fn count_finished(&self) -> i32 {
        db::list_finished_tasks(self.db_conn())
            .map(|v| v.len() as i32)
            .unwrap_or(0)
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
            qobject::TaskRole::ProjectName => {
                QVariant::from(&QString::from(node.project_name.as_deref().unwrap_or("")))
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
        roles.insert(
            qobject::TaskRole::ProjectName.repr,
            QByteArray::from("projectName"),
        );
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.visible.len()).unwrap_or(i32::MAX)
    }
}
