//! `EntriesModel` - the time entries of one task, for the entry editor.
//!
//! Set `taskId` to load a task's entries (newest first); the timer entry, if
//! running, shows with a zero duration. Same cache-then-reset discipline as the
//! other models.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use rusqlite::Connection;

use crate::db::{self, EntryRow};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!(<QtCore/QAbstractListModel>);
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

    #[qenum(EntriesModel)]
    enum EntryRole {
        Id,
        Start,
        End,
        DurationText,
        Running,
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        #[qproperty(QString, task_id, cxx_name = "taskId", READ, WRITE = set_task_id, NOTIFY)]
        // Sum of this task's own entries, e.g. "3h 20m". Driven by the model.
        #[qproperty(QString, total_text, cxx_name = "totalText")]
        // Sum including every subtask's entries. Only meaningful when
        // `hasSubtasks`; equals `totalText` otherwise. Driven by the model.
        #[qproperty(QString, subtree_total_text, cxx_name = "subtreeTotalText")]
        // Whether the task has subtasks (so the rolled-up total is worth showing).
        #[qproperty(bool, has_subtasks, cxx_name = "hasSubtasks")]
        type EntriesModel = super::EntriesModelRust;
    }

    impl cxx_qt::Initialize for EntriesModel {}

    extern "RustQt" {
        /// Load the entries of `task_id` (empty clears the list).
        #[cxx_name = "setTaskId"]
        fn set_task_id(self: Pin<&mut EntriesModel>, value: QString);

        /// Add a manual entry (`YYYY-MM-DD HH:MM` timestamps). No-op on bad input.
        #[qinvokable]
        fn add(self: Pin<&mut EntriesModel>, start: &QString, end: &QString);

        /// Edit the entry at `row`. No-op on bad input.
        #[qinvokable]
        fn update(self: Pin<&mut EntriesModel>, row: i32, start: &QString, end: &QString);

        /// Delete the entry at `row`.
        #[qinvokable]
        fn remove(self: Pin<&mut EntriesModel>, row: i32);
    }

    extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &EntriesModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &EntriesModel) -> QHash_i32_QByteArray;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &EntriesModel, parent: &QModelIndex) -> i32;
    }

    extern "RustQt" {
        /// # Safety
        /// Must be paired with `end_reset_model`.
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut EntriesModel>);

        /// # Safety
        /// Must follow a `begin_reset_model`.
        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut EntriesModel>);
    }
}

/// Backing state for [`qobject::EntriesModel`].
#[derive(Default)]
pub struct EntriesModelRust {
    conn: Option<Connection>,
    task_id: QString,
    total_text: QString,
    subtree_total_text: QString,
    has_subtasks: bool,
    cache: Vec<EntryRow>,
}

/// Seconds as a compact `Hh Mm` / `Mm` / `Ss` string.
fn human_duration(secs: i64) -> String {
    if secs <= 0 {
        return "-".to_owned();
    }
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m"),
        _ => format!("{h}h {m:02}m"),
    }
}

/// Running total of a task's entries. Unlike [`human_duration`] this keeps a
/// zero readable ("0m") rather than showing a dash.
fn human_total(secs: i64) -> String {
    if secs <= 0 {
        return "0m".to_owned();
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h == 0 {
        format!("{m}m")
    } else {
        format!("{h}h {m:02}m")
    }
}

impl cxx_qt::Initialize for qobject::EntriesModel {
    fn initialize(mut self: Pin<&mut Self>) {
        let conn = match db::open(&db::default_path()) {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("uhatt: could not open database ({e}); running in-memory");
                db::open_in_memory().expect("in-memory database")
            }
        };
        self.as_mut().rust_mut().conn = Some(conn);
    }
}

impl qobject::EntriesModel {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("EntriesModel used before initialize()")
    }

    fn reload(mut self: Pin<&mut Self>) {
        let task_id = self.task_id.to_string();
        let entries = if task_id.is_empty() {
            Vec::new()
        } else {
            db::list_entries_for_task(self.db_conn(), &task_id).unwrap_or_default()
        };
        let total: i64 = entries.iter().map(|r| r.seconds).sum();
        let (subtree_total, has_subtasks) = if task_id.is_empty() {
            (total, false)
        } else {
            (
                db::task_seconds(self.db_conn(), &task_id, true).unwrap_or(total),
                db::has_subtasks(self.db_conn(), &task_id).unwrap_or(false),
            )
        };
        // SAFETY: begin/end are paired around the cache swap.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().cache = entries;
            self.as_mut().end_reset_model();
        }
        self.as_mut()
            .set_total_text(QString::from(human_total(total).as_str()));
        self.as_mut()
            .set_subtree_total_text(QString::from(human_total(subtree_total).as_str()));
        self.as_mut().set_has_subtasks(has_subtasks);
    }

    fn set_task_id(mut self: Pin<&mut Self>, value: QString) {
        self.as_mut().rust_mut().task_id = value;
        // Custom WRITE setter: emit the change ourselves (cxx-qt only auto-emits
        // for auto-generated setters).
        self.as_mut().task_id_changed();
        self.reload();
    }

    fn id_at(&self, row: i32) -> Option<String> {
        usize::try_from(row)
            .ok()
            .and_then(|i| self.cache.get(i))
            .map(|r| r.entry.id.clone())
    }

    fn add(self: Pin<&mut Self>, start: &QString, end: &QString) {
        let task_id = self.task_id.to_string();
        if task_id.is_empty() {
            return;
        }
        match db::add_manual_entry(
            self.db_conn(),
            &task_id,
            &start.to_string(),
            &end.to_string(),
            "",
        ) {
            Ok(Some(_)) => self.reload(),
            Ok(None) => eprintln!("uhatt: manual entry rejected (check the times)"),
            Err(e) => eprintln!("uhatt: add manual entry failed: {e}"),
        }
    }

    fn update(self: Pin<&mut Self>, row: i32, start: &QString, end: &QString) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        match db::update_entry(
            self.db_conn(),
            &id,
            &start.to_string(),
            &end.to_string(),
            "",
        ) {
            Ok(true) => self.reload(),
            Ok(false) => eprintln!("uhatt: entry edit rejected (check the times)"),
            Err(e) => eprintln!("uhatt: update entry failed: {e}"),
        }
    }

    fn remove(self: Pin<&mut Self>, row: i32) {
        let Some(id) = self.id_at(row) else {
            return;
        };
        if let Err(e) = db::delete_entry(self.db_conn(), &id) {
            eprintln!("uhatt: delete entry failed: {e}");
            return;
        }
        self.reload();
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(row) = usize::try_from(index.row())
            .ok()
            .and_then(|i| self.cache.get(i))
        else {
            return QVariant::default();
        };
        let e = &row.entry;
        match (qobject::EntryRole { repr: role }) {
            qobject::EntryRole::Id => QVariant::from(&QString::from(e.id.as_str())),
            qobject::EntryRole::Start => QVariant::from(&QString::from(e.start_ts.as_str())),
            qobject::EntryRole::End => {
                QVariant::from(&QString::from(e.end_ts.as_deref().unwrap_or("")))
            }
            qobject::EntryRole::DurationText => {
                QVariant::from(&QString::from(human_duration(row.seconds).as_str()))
            }
            qobject::EntryRole::Running => QVariant::from(&e.end_ts.is_none()),
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        roles.insert(qobject::EntryRole::Id.repr, QByteArray::from("id"));
        roles.insert(qobject::EntryRole::Start.repr, QByteArray::from("start"));
        roles.insert(qobject::EntryRole::End.repr, QByteArray::from("end"));
        roles.insert(
            qobject::EntryRole::DurationText.repr,
            QByteArray::from("durationText"),
        );
        roles.insert(
            qobject::EntryRole::Running.repr,
            QByteArray::from("running"),
        );
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.cache.len()).unwrap_or(i32::MAX)
    }
}
