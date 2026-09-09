//! `GraphModel` - time-invested totals per calendar bucket, for the graph view.
//!
//! Configure `targetKind` + `targetId` + `bucket`; the model exposes one row
//! per bucket that has recorded time. `maxSeconds` lets the QML scale bar
//! heights without walking the rows itself.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use rusqlite::Connection;

use crate::db::{self, Bucket, SeriesTarget};

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

    #[qenum(GraphModel)]
    enum GraphRole {
        /// Bucket key, e.g. "2026-03-01".
        Label,
        /// Seconds recorded in the bucket.
        Seconds,
        /// Human total, e.g. "2h 05m".
        HoursText,
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        // 0 = all tasks, 1 = project, 2 = task subtree.
        #[qproperty(i32, target_kind, cxx_name = "targetKind", READ, WRITE = set_target_kind, NOTIFY)]
        #[qproperty(QString, target_id, cxx_name = "targetId", READ, WRITE = set_target_id, NOTIFY)]
        // 0 = day, 1 = week, 2 = month, 3 = year.
        #[qproperty(i32, bucket, READ, WRITE = set_bucket, NOTIFY)]
        // Largest bucket total, for scaling bars. Driven by the model.
        #[qproperty(f64, max_seconds, cxx_name = "maxSeconds")]
        // Sum across all shown buckets. Driven by the model.
        #[qproperty(QString, total_text, cxx_name = "totalText")]
        type GraphModel = super::GraphModelRust;
    }

    impl cxx_qt::Initialize for GraphModel {}

    extern "RustQt" {
        #[cxx_name = "setTargetKind"]
        fn set_target_kind(self: Pin<&mut GraphModel>, value: i32);
        #[cxx_name = "setTargetId"]
        fn set_target_id(self: Pin<&mut GraphModel>, value: QString);
        #[cxx_name = "setBucket"]
        fn set_bucket(self: Pin<&mut GraphModel>, value: i32);
    }

    extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &GraphModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &GraphModel) -> QHash_i32_QByteArray;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &GraphModel, parent: &QModelIndex) -> i32;
    }

    extern "RustQt" {
        /// # Safety
        /// Must be paired with `end_reset_model`.
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut GraphModel>);

        /// # Safety
        /// Must follow a `begin_reset_model`.
        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut GraphModel>);
    }
}

/// Backing state for [`qobject::GraphModel`].
#[derive(Default)]
pub struct GraphModelRust {
    conn: Option<Connection>,
    target_kind: i32,
    target_id: QString,
    bucket: i32,
    max_seconds: f64,
    total_text: QString,
    rows: Vec<(String, i64)>,
}

fn human_hours(secs: i64) -> String {
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

impl cxx_qt::Initialize for qobject::GraphModel {
    fn initialize(mut self: Pin<&mut Self>) {
        let conn = match db::open(&db::default_path()) {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("uhatt: could not open database ({e}); running in-memory");
                db::open_in_memory().expect("in-memory database")
            }
        };
        self.as_mut().rust_mut().conn = Some(conn);
        self.as_mut().reload();
    }
}

impl qobject::GraphModel {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("GraphModel used before initialize()")
    }

    fn target(&self) -> SeriesTarget {
        let id = self.target_id.to_string();
        match self.target_kind {
            1 if !id.is_empty() => SeriesTarget::Project(id),
            2 if !id.is_empty() => SeriesTarget::TaskSubtree(id),
            _ => SeriesTarget::All,
        }
    }

    fn bucket_enum(&self) -> Bucket {
        match self.bucket {
            1 => Bucket::Week,
            2 => Bucket::Month,
            3 => Bucket::Year,
            _ => Bucket::Day,
        }
    }

    fn reload(mut self: Pin<&mut Self>) {
        let rows =
            db::time_series(self.db_conn(), &self.target(), self.bucket_enum()).unwrap_or_default();
        let max = rows.iter().map(|(_, s)| *s).max().unwrap_or(0);
        let total: i64 = rows.iter().map(|(_, s)| *s).sum();

        // SAFETY: begin/end are paired around the state swap.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().rows = rows;
            self.as_mut().end_reset_model();
        }
        self.as_mut().set_max_seconds(max as f64);
        self.as_mut()
            .set_total_text(QString::from(human_hours(total).as_str()));
    }

    // Custom WRITE setters must emit their own change signal - cxx-qt only
    // auto-emits for auto-generated setters. Without the emit, QML bindings on
    // these properties (e.g. the bucket toggle's checked state) go stale.
    fn set_target_kind(mut self: Pin<&mut Self>, value: i32) {
        self.as_mut().rust_mut().target_kind = value;
        self.as_mut().target_kind_changed();
        self.reload();
    }

    fn set_target_id(mut self: Pin<&mut Self>, value: QString) {
        self.as_mut().rust_mut().target_id = value;
        self.as_mut().target_id_changed();
        self.reload();
    }

    fn set_bucket(mut self: Pin<&mut Self>, value: i32) {
        self.as_mut().rust_mut().bucket = value.clamp(0, 3);
        self.as_mut().bucket_changed();
        self.reload();
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some((label, secs)) = usize::try_from(index.row())
            .ok()
            .and_then(|i| self.rows.get(i))
        else {
            return QVariant::default();
        };
        match (qobject::GraphRole { repr: role }) {
            qobject::GraphRole::Label => QVariant::from(&QString::from(label.as_str())),
            qobject::GraphRole::Seconds => QVariant::from(&(*secs as f64)),
            qobject::GraphRole::HoursText => {
                QVariant::from(&QString::from(human_hours(*secs).as_str()))
            }
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        roles.insert(qobject::GraphRole::Label.repr, QByteArray::from("label"));
        roles.insert(
            qobject::GraphRole::Seconds.repr,
            QByteArray::from("seconds"),
        );
        roles.insert(
            qobject::GraphRole::HoursText.repr,
            QByteArray::from("hoursText"),
        );
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.rows.len()).unwrap_or(i32::MAX)
    }
}
