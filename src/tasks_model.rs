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
use crate::domain::TaskStatus;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);
        /// Qt base class.
        #[qobject]
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
    enum Role {
        Id,
        Title,
        Done,
        /// Nesting level (0 = root).
        Depth,
        /// Whether this task has any subtasks.
        HasChildren,
        /// Whether this task's subtasks are currently shown.
        Expanded,
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        type TaskListModel = super::TaskListModelRust;
    }

    impl cxx_qt::Initialize for TaskListModel {}

    extern "RustQt" {
        /// Append a new root task. No-op on blank input.
        #[qinvokable]
        fn add(self: Pin<&mut TaskListModel>, title: &QString);

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

        /// Collapse an expanded task or expand a collapsed one.
        #[qinvokable]
        #[cxx_name = "toggleExpanded"]
        fn toggle_expanded(self: Pin<&mut TaskListModel>, row: i32);
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
    tree: Vec<TaskNode>,
    collapsed: HashSet<String>,
    /// Indices into `tree` that are currently visible, in display order.
    visible: Vec<usize>,
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
        let tree = db::list_task_tree(&conn).unwrap_or_default();
        let visible = compute_visible(&tree, &HashSet::new());
        let mut rust = self.as_mut().rust_mut();
        rust.conn = Some(conn);
        rust.tree = tree;
        rust.visible = visible;
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
        let tree = db::list_task_tree(self.db_conn()).unwrap_or_default();
        let live: HashSet<&str> = tree.iter().map(|n| n.task.id.as_str()).collect();
        let collapsed: HashSet<String> = self
            .collapsed
            .iter()
            .filter(|id| live.contains(id.as_str()))
            .cloned()
            .collect();
        let visible = compute_visible(&tree, &collapsed);
        // SAFETY: begin/end are paired around the state swap.
        unsafe {
            self.as_mut().begin_reset_model();
            {
                let mut rust = self.as_mut().rust_mut();
                rust.tree = tree;
                rust.collapsed = collapsed;
                rust.visible = visible;
            }
            self.as_mut().end_reset_model();
        }
    }

    fn add(self: Pin<&mut Self>, title: &QString) {
        let title = title.to_string();
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        if let Err(e) = db::create_task(self.db_conn(), title, None) {
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
        if let Err(e) = db::create_task(self.db_conn(), title, Some(&parent_id)) {
            eprintln!("uhatt: add subtask failed: {e}");
            return;
        }
        // Make sure the new child is visible.
        self.as_mut().rust_mut().collapsed.remove(&parent_id);
        self.reload();
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

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(node) = self.node_at(index.row()) else {
            return QVariant::default();
        };
        let task = &node.task;

        match (qobject::Role { repr: role }) {
            qobject::Role::Id => QVariant::from(&QString::from(task.id.as_str())),
            qobject::Role::Title => QVariant::from(&QString::from(task.title.as_str())),
            qobject::Role::Done => QVariant::from(&task.is_done()),
            qobject::Role::Depth => QVariant::from(&(node.depth as i32)),
            qobject::Role::HasChildren => QVariant::from(&node.has_children),
            qobject::Role::Expanded => {
                QVariant::from(&(node.has_children && !self.collapsed.contains(&task.id)))
            }
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        roles.insert(qobject::Role::Id.repr, QByteArray::from("id"));
        roles.insert(qobject::Role::Title.repr, QByteArray::from("title"));
        roles.insert(qobject::Role::Done.repr, QByteArray::from("done"));
        roles.insert(qobject::Role::Depth.repr, QByteArray::from("depth"));
        roles.insert(
            qobject::Role::HasChildren.repr,
            QByteArray::from("hasChildren"),
        );
        roles.insert(qobject::Role::Expanded.repr, QByteArray::from("expanded"));
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.visible.len()).unwrap_or(i32::MAX)
    }
}
