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
        // Calendar page: strike a red/blue X through past days.
        #[qproperty(bool, calendar_cross_past, cxx_name = "calendarCrossPast", READ, WRITE = set_calendar_cross_past, NOTIFY)]
        // Calendar page: hide the leading/trailing days of adjacent months.
        #[qproperty(bool, calendar_hide_other_month, cxx_name = "calendarHideOtherMonth", READ, WRITE = set_calendar_hide_other_month, NOTIFY)]
        // Calendar page: show the per-day "TC: N" task-count badge.
        #[qproperty(bool, calendar_show_task_count, cxx_name = "calendarShowTaskCount", READ, WRITE = set_calendar_show_task_count, NOTIFY)]
        type Settings = super::SettingsRust;
    }

    impl cxx_qt::Initialize for Settings {}

    extern "RustQt" {
        #[cxx_name = "setDeadlineCountdown"]
        fn set_deadline_countdown(self: Pin<&mut Settings>, value: i32);

        #[cxx_name = "setCalendarCrossPast"]
        fn set_calendar_cross_past(self: Pin<&mut Settings>, value: bool);

        #[cxx_name = "setCalendarHideOtherMonth"]
        fn set_calendar_hide_other_month(self: Pin<&mut Settings>, value: bool);

        #[cxx_name = "setCalendarShowTaskCount"]
        fn set_calendar_show_task_count(self: Pin<&mut Settings>, value: bool);

        /// "time left until `iso_date`" per the current setting; "" when the
        /// countdown is off or the date can't be read.
        #[qinvokable]
        #[cxx_name = "countdownText"]
        fn countdown_text(self: &Settings, iso_date: &QString) -> QString;
    }
}

/// Backing state for [`qobject::Settings`].
pub struct SettingsRust {
    conn: Option<Connection>,
    deadline_countdown: i32,
    calendar_cross_past: bool,
    calendar_hide_other_month: bool,
    calendar_show_task_count: bool,
}

impl Default for SettingsRust {
    fn default() -> Self {
        Self {
            conn: None,
            deadline_countdown: 0,
            // Cross past days on by default; hiding adjacent-month days off
            // (the grid shows them until the user opts out); task-count badge
            // on (it is the styled version the user asked for).
            calendar_cross_past: true,
            calendar_hide_other_month: false,
            calendar_show_task_count: true,
        }
    }
}

/// `meta` keys settings are stored under.
const COUNTDOWN_KEY: &str = "deadline_countdown";
const CROSS_PAST_KEY: &str = "calendar_cross_past";
const HIDE_OTHER_MONTH_KEY: &str = "calendar_hide_other_month";
const SHOW_TASK_COUNT_KEY: &str = "calendar_show_task_count";

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
        let countdown = db::get_meta(&conn, COUNTDOWN_KEY)
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let cross_past = read_bool(&conn, CROSS_PAST_KEY, true);
        let hide_other_month = read_bool(&conn, HIDE_OTHER_MONTH_KEY, false);
        let show_task_count = read_bool(&conn, SHOW_TASK_COUNT_KEY, true);
        {
            let mut rust = self.as_mut().rust_mut();
            rust.conn = Some(conn);
            rust.deadline_countdown = countdown;
            rust.calendar_cross_past = cross_past;
            rust.calendar_hide_other_month = hide_other_month;
            rust.calendar_show_task_count = show_task_count;
        }
    }
}

/// A `meta` flag stored as `"1"` / `"0"`, falling back to `default`.
fn read_bool(conn: &Connection, key: &str, default: bool) -> bool {
    match db::get_meta(conn, key).ok().flatten() {
        Some(s) => s == "1",
        None => default,
    }
}

/// Persist a `meta` flag as `"1"` / `"0"`, logging on failure.
fn persist_bool(conn: &Connection, key: &str, value: bool) {
    let v = if value { "1" } else { "0" };
    if let Err(e) = db::set_meta(conn, key, v) {
        eprintln!("uhatt: could not save setting {key}: {e}");
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

    fn set_calendar_cross_past(mut self: Pin<&mut Self>, value: bool) {
        if self.calendar_cross_past == value {
            return;
        }
        self.as_mut().rust_mut().calendar_cross_past = value;
        persist_bool(self.db_conn(), CROSS_PAST_KEY, value);
        // Custom-WRITE properties don't auto-emit; the calendar binds to this.
        self.as_mut().calendar_cross_past_changed();
    }

    fn set_calendar_hide_other_month(mut self: Pin<&mut Self>, value: bool) {
        if self.calendar_hide_other_month == value {
            return;
        }
        self.as_mut().rust_mut().calendar_hide_other_month = value;
        persist_bool(self.db_conn(), HIDE_OTHER_MONTH_KEY, value);
        self.as_mut().calendar_hide_other_month_changed();
    }

    fn set_calendar_show_task_count(mut self: Pin<&mut Self>, value: bool) {
        if self.calendar_show_task_count == value {
            return;
        }
        self.as_mut().rust_mut().calendar_show_task_count = value;
        persist_bool(self.db_conn(), SHOW_TASK_COUNT_KEY, value);
        self.as_mut().calendar_show_task_count_changed();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_flags_round_trip_through_meta_with_defaults() {
        let conn = db::open_in_memory().unwrap();

        // Unset -> the caller's default.
        assert!(read_bool(&conn, CROSS_PAST_KEY, true));
        assert!(!read_bool(&conn, HIDE_OTHER_MONTH_KEY, false));
        assert!(read_bool(&conn, SHOW_TASK_COUNT_KEY, true));

        persist_bool(&conn, CROSS_PAST_KEY, false);
        persist_bool(&conn, HIDE_OTHER_MONTH_KEY, true);
        persist_bool(&conn, SHOW_TASK_COUNT_KEY, false);

        // Stored value wins over the default, both ways.
        assert!(!read_bool(&conn, CROSS_PAST_KEY, true));
        assert!(read_bool(&conn, HIDE_OTHER_MONTH_KEY, false));
        assert!(!read_bool(&conn, SHOW_TASK_COUNT_KEY, true));
    }
}
