//! `GraphModel` - the calendar heatmap of time invested.
//!
//! Configure `targetKind` + `targetId`, pick a `range` (0 = week, 1 = month,
//! 2 = year) and page through periods with `step()` / the `anchor`. The model
//! then exposes one row per day-cell of the Monday-aligned grid spanning that
//! period, ordered by date. `maxSeconds` lets QML scale cell colour and
//! `atLatest` tells it whether paging forward is possible.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use rusqlite::Connection;

use crate::db::{self, HeatCell, HeatRange, SeriesTarget};

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
        /// The cell's date, `YYYY-MM-DD`.
        Date,
        /// Seconds recorded that day.
        Seconds,
        /// Human total, e.g. "2h 05m" / "40m" / "12s" / "0m".
        HoursText,
        /// False for pad days borrowed from the neighbouring periods.
        InPeriod,
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        // 0 = all tasks, 1 = project, 2 = task subtree.
        #[qproperty(i32, target_kind, cxx_name = "targetKind", READ, WRITE = set_target_kind, NOTIFY)]
        #[qproperty(QString, target_id, cxx_name = "targetId", READ, WRITE = set_target_id, NOTIFY)]
        // 0 = week, 1 = month, 2 = year.
        #[qproperty(i32, range, READ, WRITE = set_range, NOTIFY)]
        // First day (ISO) of the period on show; always snapped to the range.
        #[qproperty(QString, anchor, READ, WRITE = set_anchor, NOTIFY)]
        // True when the period contains today or is in the future. Model-driven.
        #[qproperty(bool, at_latest, cxx_name = "atLatest")]
        // Largest single-day total, for colour scaling. Model-driven.
        #[qproperty(f64, max_seconds, cxx_name = "maxSeconds")]
        // Sum across the period (pad days excluded). Model-driven.
        #[qproperty(QString, total_text, cxx_name = "totalText")]
        type GraphModel = super::GraphModelRust;
    }

    impl cxx_qt::Initialize for GraphModel {}

    extern "RustQt" {
        #[cxx_name = "setTargetKind"]
        fn set_target_kind(self: Pin<&mut GraphModel>, value: i32);
        #[cxx_name = "setTargetId"]
        fn set_target_id(self: Pin<&mut GraphModel>, value: QString);
        #[cxx_name = "setRange"]
        fn set_range(self: Pin<&mut GraphModel>, value: i32);
        #[cxx_name = "setAnchor"]
        fn set_anchor(self: Pin<&mut GraphModel>, value: QString);

        /// Page by `direction` whole periods (negative = back).
        #[qinvokable]
        fn step(self: Pin<&mut GraphModel>, direction: i32);
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
    range: i32,
    anchor: QString,
    at_latest: bool,
    max_seconds: f64,
    total_text: QString,
    cells: Vec<HeatCell>,
}

/// Seconds as a compact human string. Sub-minute spans read in seconds rather
/// than rounding to "0m" - that rounding was half of the "graph shows 0" bug.
fn human_hours(secs: i64) -> String {
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

fn range_of(value: i32) -> HeatRange {
    match value {
        0 => HeatRange::Week,
        1 => HeatRange::Month,
        _ => HeatRange::Year,
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
        // Default to the current year.
        let today: String = conn
            .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
            .unwrap_or_else(|_| "2026-01-01".to_owned());
        let anchor = db::snap_anchor(&conn, HeatRange::Year, &today).unwrap_or(today);
        self.as_mut().rust_mut().conn = Some(conn);
        self.as_mut().rust_mut().range = 2;
        self.as_mut().rust_mut().anchor = QString::from(anchor.as_str());
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

    fn reload(mut self: Pin<&mut Self>) {
        let range = range_of(self.range);
        let anchor = self.anchor.to_string();
        let cells = db::heatmap(self.db_conn(), &self.target(), range, &anchor).unwrap_or_default();
        // Pad days are drawn transparent, so scale and total over the period
        // only. (This also makes `maxSeconds > 0` mean "the period has time",
        // which the QML relies on for its empty state.)
        let in_period = || cells.iter().filter(|c| c.in_period).map(|c| c.seconds);
        let max = in_period().max().unwrap_or(0);
        let total: i64 = in_period().sum();
        let at_latest = db::heat_at_latest(self.db_conn(), range, &anchor).unwrap_or(true);

        // SAFETY: begin/end are paired around the state swap.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().cells = cells;
            self.as_mut().end_reset_model();
        }
        self.as_mut().set_at_latest(at_latest);
        self.as_mut().set_max_seconds(max as f64);
        self.as_mut()
            .set_total_text(QString::from(human_hours(total).as_str()));
    }

    /// Overwrite `anchor` with `value` snapped to the current range, emit, reload.
    fn reanchor(mut self: Pin<&mut Self>, value: &str) {
        let range = range_of(self.range);
        let snapped =
            db::snap_anchor(self.db_conn(), range, value).unwrap_or_else(|_| value.to_owned());
        self.as_mut().rust_mut().anchor = QString::from(snapped.as_str());
        self.as_mut().anchor_changed();
    }

    // Custom WRITE setters must emit their own change signal - cxx-qt only
    // auto-emits for auto-generated setters. Without the emit, QML bindings on
    // these properties go stale.
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

    fn set_range(mut self: Pin<&mut Self>, value: i32) {
        // If the current period includes today ("now" mode), stay in "now" mode
        // for the new range; otherwise keep looking at wherever the anchor is.
        let was_latest = {
            let range = range_of(self.range);
            let anchor = self.anchor.to_string();
            db::heat_at_latest(self.db_conn(), range, &anchor).unwrap_or(false)
        };
        self.as_mut().rust_mut().range = value.clamp(0, 2);
        self.as_mut().range_changed();
        let seed = if was_latest {
            self.db_conn()
                .query_row("SELECT date('now', 'localtime')", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap_or_else(|_| self.anchor.to_string())
        } else {
            self.anchor.to_string()
        };
        self.as_mut().reanchor(&seed);
        self.reload();
    }

    fn set_anchor(mut self: Pin<&mut Self>, value: QString) {
        self.as_mut().reanchor(&value.to_string());
        self.reload();
    }

    fn step(mut self: Pin<&mut Self>, direction: i32) {
        let range = range_of(self.range);
        let anchor = self.anchor.to_string();
        let Ok(next) = db::step_anchor(self.db_conn(), range, &anchor, direction) else {
            return;
        };
        self.as_mut().rust_mut().anchor = QString::from(next.as_str());
        self.as_mut().anchor_changed();
        self.reload();
    }

    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(cell) = usize::try_from(index.row())
            .ok()
            .and_then(|i| self.cells.get(i))
        else {
            return QVariant::default();
        };
        match (qobject::GraphRole { repr: role }) {
            qobject::GraphRole::Date => QVariant::from(&QString::from(cell.date.as_str())),
            qobject::GraphRole::Seconds => QVariant::from(&(cell.seconds as f64)),
            qobject::GraphRole::HoursText => {
                QVariant::from(&QString::from(human_hours(cell.seconds).as_str()))
            }
            qobject::GraphRole::InPeriod => QVariant::from(&cell.in_period),
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        roles.insert(qobject::GraphRole::Date.repr, QByteArray::from("date"));
        roles.insert(
            qobject::GraphRole::Seconds.repr,
            QByteArray::from("seconds"),
        );
        roles.insert(
            qobject::GraphRole::HoursText.repr,
            QByteArray::from("hoursText"),
        );
        roles.insert(
            qobject::GraphRole::InPeriod.repr,
            QByteArray::from("inPeriod"),
        );
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.cells.len()).unwrap_or(i32::MAX)
    }
}
