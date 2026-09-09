//! `ProjectListModel` - a flat `QAbstractListModel` of projects for the sidebar.
//!
//! Same cache discipline as [`crate::tasks_model`]: an in-memory `Vec<Project>`,
//! reloaded from SQLite inside `beginResetModel`/`endResetModel` on every change.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use rusqlite::Connection;

use crate::db;
use crate::domain::Project;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!(<QtCore/QAbstractListModel>);
        /// Qt base class.
        type QAbstractListModel;
    }

    unsafe extern "C++" {
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    /// Item roles exposed to QML.
    #[qenum(ProjectListModel)]
    enum ProjectRole {
        Id,
        Name,
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        type ProjectListModel = super::ProjectListModelRust;
    }

    impl cxx_qt::Initialize for ProjectListModel {}

    extern "RustQt" {
        /// Create a project and return its id (empty string on failure / blank name).
        #[qinvokable]
        fn add(self: Pin<&mut ProjectListModel>, name: &QString) -> QString;

        /// Rename the project at `row`.
        #[qinvokable]
        fn rename(self: Pin<&mut ProjectListModel>, row: i32, name: &QString);

        /// Delete the project at `row`; its tasks become unfiled.
        #[qinvokable]
        fn remove(self: Pin<&mut ProjectListModel>, row: i32);
    }

    extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &ProjectListModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &ProjectListModel) -> QHash_i32_QByteArray;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &ProjectListModel, parent: &QModelIndex) -> i32;
    }

    extern "RustQt" {
        /// # Safety
        /// Must be paired with `end_reset_model`.
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut ProjectListModel>);

        /// # Safety
        /// Must follow a `begin_reset_model`.
        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut ProjectListModel>);
    }
}

/// Backing state for [`qobject::ProjectListModel`].
#[derive(Default)]
pub struct ProjectListModelRust {
    conn: Option<Connection>,
    cache: Vec<Project>,
}

impl cxx_qt::Initialize for qobject::ProjectListModel {
    fn initialize(mut self: Pin<&mut Self>) {
        let conn = match db::open(&db::default_path()) {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("uhatt: could not open database ({e}); running in-memory");
                db::open_in_memory().expect("in-memory database")
            }
        };
        let cache = db::list_projects(&conn).unwrap_or_default();
        let mut rust = self.as_mut().rust_mut();
        rust.conn = Some(conn);
        rust.cache = cache;
    }
}

impl qobject::ProjectListModel {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("ProjectListModel used before initialize()")
    }

    fn reload(mut self: Pin<&mut Self>) {
        let projects = db::list_projects(self.db_conn()).unwrap_or_default();
        // SAFETY: begin/end are paired around the cache swap.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().cache = projects;
            self.as_mut().end_reset_model();
        }
    }

    fn add(mut self: Pin<&mut Self>, name: &QString) -> QString {
        let name = name.to_string();
        let name = name.trim();
        if name.is_empty() {
            return QString::default();
        }
        match db::create_project(self.db_conn(), name) {
            Ok(project) => {
                let id = QString::from(project.id.as_str());
                self.as_mut().reload();
                id
            }
            Err(e) => {
                eprintln!("uhatt: create project failed: {e}");
                QString::default()
            }
        }
    }

    fn rename(self: Pin<&mut Self>, row: i32, name: &QString) {
        let name = name.to_string();
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(id) = self.id_at(row) else {
            return;
        };
        if let Err(e) = db::rename_project(self.db_conn(), &id, name) {
            eprintln!("uhatt: rename project failed: {e}");
            return;
        }
        self.reload();
    }

    fn remove(self: Pin<&mut Self>, row: i32) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        if let Err(e) = db::delete_project(self.db_conn(), &id) {
            eprintln!("uhatt: delete project failed: {e}");
            return;
        }
        self.reload();
    }

    fn id_at(&self, row: i32) -> Option<String> {
        usize::try_from(row)
            .ok()
            .and_then(|i| self.cache.get(i))
            .map(|p| p.id.clone())
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(project) = usize::try_from(index.row())
            .ok()
            .and_then(|i| self.cache.get(i))
        else {
            return QVariant::default();
        };
        match (qobject::ProjectRole { repr: role }) {
            qobject::ProjectRole::Id => QVariant::from(&QString::from(project.id.as_str())),
            qobject::ProjectRole::Name => QVariant::from(&QString::from(project.name.as_str())),
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        roles.insert(qobject::ProjectRole::Id.repr, QByteArray::from("id"));
        roles.insert(qobject::ProjectRole::Name.repr, QByteArray::from("name"));
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.cache.len()).unwrap_or(i32::MAX)
    }
}
