//! `TaskListModel` - a `QAbstractListModel` that exposes tasks to QML.
//!
//! Backed by an in-memory `Vec<Task>` cache; `data()` only ever reads the
//! cache (Qt calls it once per visible cell per repaint). Every mutation
//! writes SQLite first, then reloads the cache inside
//! `beginResetModel`/`endResetModel`. Reset-per-change is fine at milestone-1
//! scale; precise row signals can replace it later without touching QML.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use rusqlite::Connection;

use crate::db;
use crate::domain::{Task, TaskStatus};

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
        /// Nesting level; always 0 until the UI renders subtasks.
        Depth,
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

        /// Remove the task at `row`.
        #[qinvokable]
        fn remove(self: Pin<&mut TaskListModel>, row: i32);

        /// Set the done state of the task at `row`.
        #[qinvokable]
        #[cxx_name = "setDone"]
        fn set_done(self: Pin<&mut TaskListModel>, row: i32, done: bool);

        /// Rename the task at `row`. No-op on blank input.
        #[qinvokable]
        fn rename(self: Pin<&mut TaskListModel>, row: i32, title: &QString);
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
    cache: Vec<Task>,
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
        let cache = db::list_tasks(&conn).unwrap_or_default();
        self.as_mut().rust_mut().conn = Some(conn);
        self.as_mut().rust_mut().cache = cache;
    }
}

impl qobject::TaskListModel {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("TaskListModel used before initialize()")
    }

    /// Re-read every task from SQLite into the cache, wrapped in a model reset.
    fn reload(mut self: Pin<&mut Self>) {
        let tasks = db::list_tasks(self.db_conn()).unwrap_or_default();
        // SAFETY: begin/end are paired around the cache swap.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().cache = tasks;
            self.as_mut().end_reset_model();
        }
    }

    fn task_id_at(&self, row: i32) -> Option<String> {
        usize::try_from(row)
            .ok()
            .and_then(|i| self.cache.get(i))
            .map(|t| t.id.clone())
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

    fn remove(self: Pin<&mut Self>, row: i32) {
        let Some(id) = self.task_id_at(row) else {
            return;
        };
        if let Err(e) = db::delete_task(self.db_conn(), &id) {
            eprintln!("uhatt: delete task failed: {e}");
            return;
        }
        self.reload();
    }

    fn set_done(self: Pin<&mut Self>, row: i32, done: bool) {
        let Some(id) = self.task_id_at(row) else {
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
        let Some(id) = self.task_id_at(row) else {
            return;
        };
        if let Err(e) = db::rename_task(self.db_conn(), &id, title) {
            eprintln!("uhatt: rename task failed: {e}");
            return;
        }
        self.reload();
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(task) = usize::try_from(index.row())
            .ok()
            .and_then(|i| self.cache.get(i))
        else {
            return QVariant::default();
        };

        match (qobject::Role { repr: role }) {
            qobject::Role::Id => QVariant::from(&QString::from(task.id.as_str())),
            qobject::Role::Title => QVariant::from(&QString::from(task.title.as_str())),
            qobject::Role::Done => QVariant::from(&task.is_done()),
            qobject::Role::Depth => QVariant::from(&0_i32),
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        roles.insert(qobject::Role::Id.repr, QByteArray::from("id"));
        roles.insert(qobject::Role::Title.repr, QByteArray::from("title"));
        roles.insert(qobject::Role::Done.repr, QByteArray::from("done"));
        roles.insert(qobject::Role::Depth.repr, QByteArray::from("depth"));
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.cache.len()).unwrap_or(i32::MAX)
    }
}
