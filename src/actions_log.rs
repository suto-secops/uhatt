//! `ActionsLog` - the data behind the "Recent actions" page: every logged
//! task mutation, newest first, with per-row revert. The set is small (one
//! app's worth of edits), so like `Calendar` this hands QML the whole thing
//! as JSON rather than being a list model; `revision` bumps on every
//! [`reload`](qobject::ActionsLog::reload) so the binding that reads
//! [`items_json`](qobject::ActionsLog::items_json) re-parses.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use rusqlite::Connection;
use serde::Serialize;

use crate::db;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        // Bumped on every reload; QML reads it in the binding that parses
        // `itemsJson()` so the page refreshes after a log change.
        #[qproperty(i32, revision)]
        type ActionsLog = super::ActionsLogRust;
    }

    impl cxx_qt::Initialize for ActionsLog {}

    extern "RustQt" {
        /// Re-read the log from the database.
        #[qinvokable]
        fn reload(self: Pin<&mut ActionsLog>);

        /// Every logged action, newest first, as a JSON array of
        /// `{id, summary, createdAt}`.
        #[qinvokable]
        #[cxx_name = "itemsJson"]
        fn items_json(self: &ActionsLog) -> QString;

        /// Undo just this one action.
        #[qinvokable]
        fn reset(self: Pin<&mut ActionsLog>, id: i64);

        /// Undo this action and everything logged after it.
        #[qinvokable]
        #[cxx_name = "resetFrom"]
        fn reset_from(self: Pin<&mut ActionsLog>, id: i64);
    }
}

/// Backing state for [`qobject::ActionsLog`].
#[derive(Default)]
pub struct ActionsLogRust {
    conn: Option<Connection>,
    items: Vec<db::actions::ActionRow>,
    revision: i32,
}

impl cxx_qt::Initialize for qobject::ActionsLog {
    fn initialize(mut self: Pin<&mut Self>) {
        let conn = match db::open(&db::default_path()) {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("uhatt: could not open database ({e}); running in-memory");
                db::open_in_memory().expect("in-memory database")
            }
        };
        let items = db::actions::list_actions(&conn).unwrap_or_default();
        self.as_mut().rust_mut().conn = Some(conn);
        self.as_mut().rust_mut().items = items;
    }
}

#[derive(Serialize)]
struct ActionJson<'a> {
    id: i64,
    summary: &'a str,
    #[serde(rename = "createdAt")]
    created_at: &'a str,
}

impl qobject::ActionsLog {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("ActionsLog used before initialize()")
    }

    fn reload(mut self: Pin<&mut Self>) {
        match db::actions::list_actions(self.db_conn()) {
            Ok(items) => self.as_mut().rust_mut().items = items,
            Err(e) => {
                eprintln!("uhatt: actions log reload failed: {e}");
                return;
            }
        }
        let next = self.revision.wrapping_add(1);
        self.as_mut().set_revision(next);
    }

    fn items_json(&self) -> QString {
        let out: Vec<ActionJson> = self
            .items
            .iter()
            .map(|a| ActionJson {
                id: a.id,
                summary: &a.summary,
                created_at: &a.created_at,
            })
            .collect();
        let json = serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_owned());
        QString::from(json.as_str())
    }

    fn reset(self: Pin<&mut Self>, id: i64) {
        if let Err(e) = db::actions::revert_action(self.db_conn(), id) {
            eprintln!("uhatt: revert action {id} failed: {e}");
            return;
        }
        self.reload();
    }

    fn reset_from(self: Pin<&mut Self>, id: i64) {
        if let Err(e) = db::actions::revert_from(self.db_conn(), id) {
            eprintln!("uhatt: revert-from action {id} failed: {e}");
            return;
        }
        self.reload();
    }
}
