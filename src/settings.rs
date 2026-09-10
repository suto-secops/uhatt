//! `Settings` - user preferences, persisted in the `meta` key/value table.
//!
//! Currently one setting: how (if at all) a countdown is shown next to a task's
//! deadline. Kept in its own QObject so the settings dialog binds to it directly
//! and more preferences can be added without touching the task models.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use rusqlite::Connection;

use crate::db::{self, CountdownMode};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        // 0 off, 1 optimal, 2 days, 3 weeks, 4 months, 5 hours.
        #[qproperty(i32, deadline_countdown, cxx_name = "deadlineCountdown", READ, WRITE = set_deadline_countdown, NOTIFY)]
        type Settings = super::SettingsRust;
    }

    impl cxx_qt::Initialize for Settings {}

    extern "RustQt" {
        #[cxx_name = "setDeadlineCountdown"]
        fn set_deadline_countdown(self: Pin<&mut Settings>, value: i32);

        /// "time left until `iso_date`" per the current setting; "" when the
        /// countdown is off or the date can't be read.
        #[qinvokable]
        #[cxx_name = "countdownText"]
        fn countdown_text(self: &Settings, iso_date: &QString) -> QString;
    }
}

/// Backing state for [`qobject::Settings`].
#[derive(Default)]
pub struct SettingsRust {
    conn: Option<Connection>,
    deadline_countdown: i32,
}

/// `meta` key the deadline-countdown mode is stored under.
const COUNTDOWN_KEY: &str = "deadline_countdown";

fn mode_of(value: i32) -> CountdownMode {
    match value {
        1 => CountdownMode::Optimal,
        2 => CountdownMode::Days,
        3 => CountdownMode::Weeks,
        4 => CountdownMode::Months,
        5 => CountdownMode::Hours,
        _ => CountdownMode::Off,
    }
}

impl cxx_qt::Initialize for qobject::Settings {
    fn initialize(mut self: Pin<&mut Self>) {
        let conn = match db::open(&db::default_path()) {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("uhatt: could not open database ({e}); running in-memory");
                db::open_in_memory().expect("in-memory database")
            }
        };
        let stored = db::get_meta(&conn, COUNTDOWN_KEY)
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        self.as_mut().rust_mut().conn = Some(conn);
        self.as_mut().rust_mut().deadline_countdown = stored;
    }
}

impl qobject::Settings {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("Settings used before initialize()")
    }

    fn set_deadline_countdown(mut self: Pin<&mut Self>, value: i32) {
        let value = value.clamp(0, 5);
        if self.deadline_countdown == value {
            return;
        }
        self.as_mut().rust_mut().deadline_countdown = value;
        if let Err(e) = db::set_meta(self.db_conn(), COUNTDOWN_KEY, &value.to_string()) {
            eprintln!("uhatt: could not save setting: {e}");
        }
        self.as_mut().deadline_countdown_changed();
    }

    fn countdown_text(&self, iso_date: &QString) -> QString {
        let text = db::deadline_countdown(
            self.db_conn(),
            &iso_date.to_string(),
            mode_of(self.deadline_countdown),
        )
        .unwrap_or_default();
        QString::from(text.as_str())
    }
}
