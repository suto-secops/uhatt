//! `Calendar` - the data behind the Calendar / Agenda page.
//!
//! The deadline set is small, so rather than a filtered list model this hands
//! QML the whole thing as a JSON array (date-ordered) and QML slices it into a
//! month grid, a selected-day list and an agenda. `revision` bumps on every
//! [`reload`](qobject::Calendar::reload) so bindings that call
//! [`items_json`](qobject::Calendar::items_json) re-run.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use rusqlite::Connection;

use crate::db;
use crate::domain::TaskStatus;

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
        // `itemsJson()` so the page refreshes when a deadline changes.
        #[qproperty(i32, revision)]
        type Calendar = super::CalendarRust;
    }

    impl cxx_qt::Initialize for Calendar {}

    extern "RustQt" {
        /// Re-read the deadline set from the database.
        #[qinvokable]
        fn reload(self: Pin<&mut Calendar>);

        /// Every not-done task with a deadline, earliest first, as a JSON
        /// array of `{id, title, date, overdue, project}`.
        #[qinvokable]
        #[cxx_name = "itemsJson"]
        fn items_json(self: &Calendar) -> QString;

        /// Mark a task done / not-done by id and reload. A task marked done
        /// drops out of the deadline list (same rule as everywhere else in
        /// the app), so this also removes it from the calendar page.
        #[qinvokable]
        #[cxx_name = "setDone"]
        fn set_done(self: Pin<&mut Calendar>, id: &QString, done: bool);
    }
}

/// Backing state for [`qobject::Calendar`].
#[derive(Default)]
pub struct CalendarRust {
    conn: Option<Connection>,
    items: Vec<db::DeadlineItem>,
    revision: i32,
}

impl cxx_qt::Initialize for qobject::Calendar {
    fn initialize(mut self: Pin<&mut Self>) {
        let conn = match db::open(&db::default_path()) {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("uhatt: could not open database ({e}); running in-memory");
                db::open_in_memory().expect("in-memory database")
            }
        };
        let items = db::deadline_items(&conn).unwrap_or_default();
        self.as_mut().rust_mut().conn = Some(conn);
        self.as_mut().rust_mut().items = items;
    }
}

impl qobject::Calendar {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("Calendar used before initialize()")
    }

    fn reload(mut self: Pin<&mut Self>) {
        match db::deadline_items(self.db_conn()) {
            Ok(items) => self.as_mut().rust_mut().items = items,
            Err(e) => {
                eprintln!("uhatt: calendar reload failed: {e}");
                return;
            }
        }
        let next = self.revision.wrapping_add(1);
        self.as_mut().set_revision(next);
    }

    fn set_done(self: Pin<&mut Self>, id: &QString, done: bool) {
        let id = id.to_string();
        let status = if done {
            TaskStatus::Done
        } else {
            TaskStatus::Todo
        };
        let before = db::get_task(self.db_conn(), &id).ok().flatten();
        if let Err(e) = db::set_task_status(self.db_conn(), &id, status) {
            eprintln!("uhatt: calendar setDone failed: {e}");
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

    fn items_json(&self) -> QString {
        let mut out = String::from("[");
        for (i, it) in self.items.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"id\":{},\"title\":{},\"date\":{},\"overdue\":{},\"project\":{}}}",
                json_str(&it.id),
                json_str(&it.title),
                json_str(&it.deadline),
                it.overdue,
                json_str(&it.project),
            ));
        }
        out.push(']');
        QString::from(out.as_str())
    }
}

/// Minimal JSON string escaping (quotes, backslash, control chars).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
