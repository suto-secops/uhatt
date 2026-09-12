//! `ProjectListModel` - a `QAbstractListModel` exposing the project tree to
//! QML as a flattened, depth-annotated list (sidebar "folders" for grouping
//! projects; a project's own task filter never includes a sub-project's
//! tasks - nesting is purely organizational).
//!
//! Same cache discipline as [`crate::tasks_model`]: an in-memory `Vec<ProjectNode>`,
//! reloaded from SQLite inside `beginResetModel`/`endResetModel` on every change.

use core::pin::Pin;
use std::collections::HashSet;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use rusqlite::Connection;

use crate::db::{self, ProjectNode};

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
        /// Nesting level (0 = top level).
        Depth,
        /// Whether this project has any sub-projects.
        HasChildren,
        /// Whether this project's sub-projects are currently shown.
        Expanded,
        /// One char per indent column, "1" where a tree guide line runs
        /// full-height, "0" where it stops at this row's connector - same
        /// meaning as `TaskListModel`'s `branchMask` role.
        BranchMask,
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

        /// Delete the project at `row`; its tasks become unfiled and any
        /// sub-projects are promoted to the top level.
        #[qinvokable]
        fn remove(self: Pin<&mut ProjectListModel>, row: i32);

        /// Re-parent the project at `row` under `target_id`, or to the top
        /// level when `target_id` is empty. No-op if the move would create a
        /// cycle.
        #[qinvokable]
        fn reparent(self: Pin<&mut ProjectListModel>, row: i32, target_id: &QString);

        /// Whether [`reparent`] with these arguments would do anything - used
        /// to light up a valid drop target while dragging.
        #[qinvokable]
        #[cxx_name = "canReparent"]
        fn can_reparent(self: &ProjectListModel, row: i32, target_id: &QString) -> bool;

        /// Collapse an expanded project or expand a collapsed one.
        #[qinvokable]
        #[cxx_name = "toggleExpanded"]
        fn toggle_expanded(self: Pin<&mut ProjectListModel>, row: i32);

        /// Re-read from SQLite. For picking up a project created through
        /// another QObject (e.g. quick creation).
        #[qinvokable]
        fn refresh(self: Pin<&mut ProjectListModel>);
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
    tree: Vec<ProjectNode>,
    collapsed: HashSet<String>,
    /// Indices into `tree` that are currently visible, in display order.
    visible: Vec<usize>,
}

/// Walk a pre-ordered tree and return the indices whose ancestors are all
/// expanded. A collapsed node stays visible; its descendants do not. Same
/// logic as `tasks_model::compute_visible`, duplicated rather than shared
/// since the two node types differ.
fn compute_visible(tree: &[ProjectNode], collapsed: &HashSet<String>) -> Vec<usize> {
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
        if node.has_children && collapsed.contains(&node.project.id) {
            hidden_below = Some(node.depth);
        }
    }
    visible
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
        let tree = db::list_project_tree(&conn).unwrap_or_default();
        let visible = compute_visible(&tree, &HashSet::new());
        let mut rust = self.as_mut().rust_mut();
        rust.conn = Some(conn);
        rust.tree = tree;
        rust.visible = visible;
    }
}

impl qobject::ProjectListModel {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("ProjectListModel used before initialize()")
    }

    fn node_at(&self, row: i32) -> Option<&ProjectNode> {
        let row = usize::try_from(row).ok()?;
        let idx = *self.visible.get(row)?;
        self.tree.get(idx)
    }

    fn id_at(&self, row: i32) -> Option<String> {
        self.node_at(row).map(|n| n.project.id.clone())
    }

    fn reload(mut self: Pin<&mut Self>) {
        let tree = db::list_project_tree(self.db_conn()).unwrap_or_default();
        let live: HashSet<&str> = tree.iter().map(|n| n.project.id.as_str()).collect();
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

    fn refresh(self: Pin<&mut Self>) {
        self.reload();
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

    fn reparent(self: Pin<&mut Self>, row: i32, target_id: &QString) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        let target = target_id.to_string();
        let target = (!target.is_empty()).then_some(target.as_str());
        if let Err(e) = db::reparent_project(self.db_conn(), &id, target) {
            eprintln!("uhatt: reparent project failed: {e}");
            return;
        }
        self.reload();
    }

    fn can_reparent(&self, row: i32, target_id: &QString) -> bool {
        let Some(id) = self.id_at(row) else {
            return false;
        };
        let target = target_id.to_string();
        let target = (!target.is_empty()).then_some(target.as_str());
        db::can_reparent_project(self.db_conn(), &id, target).unwrap_or(false)
    }

    fn toggle_expanded(mut self: Pin<&mut Self>, row: i32) {
        let Some(node) = self.node_at(row) else {
            return;
        };
        if !node.has_children {
            return;
        }
        let id = node.project.id.clone();
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
        match (qobject::ProjectRole { repr: role }) {
            qobject::ProjectRole::Id => QVariant::from(&QString::from(node.project.id.as_str())),
            qobject::ProjectRole::Name => {
                QVariant::from(&QString::from(node.project.name.as_str()))
            }
            qobject::ProjectRole::Depth => QVariant::from(&(node.depth as i32)),
            qobject::ProjectRole::HasChildren => QVariant::from(&node.has_children),
            qobject::ProjectRole::Expanded => {
                QVariant::from(&(node.has_children && !self.collapsed.contains(&node.project.id)))
            }
            qobject::ProjectRole::BranchMask => {
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
        roles.insert(qobject::ProjectRole::Id.repr, QByteArray::from("id"));
        roles.insert(qobject::ProjectRole::Name.repr, QByteArray::from("name"));
        roles.insert(qobject::ProjectRole::Depth.repr, QByteArray::from("depth"));
        roles.insert(
            qobject::ProjectRole::HasChildren.repr,
            QByteArray::from("hasChildren"),
        );
        roles.insert(
            qobject::ProjectRole::Expanded.repr,
            QByteArray::from("expanded"),
        );
        roles.insert(
            qobject::ProjectRole::BranchMask.repr,
            QByteArray::from("branchMask"),
        );
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.visible.len()).unwrap_or(i32::MAX)
    }
}
