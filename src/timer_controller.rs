//! `TimerController` - the app's single running timer, surfaced to QML.
//!
//! Not a list model: a plain QObject whose Q_PROPERTYs describe one timing
//! "session". A session belongs to one task and moves between two states:
//!
//! - **running** - a `time_entries` row is open (`end_ts IS NULL`);
//!   `runningSince` holds its start, `paused` is false.
//! - **paused** - that row has been closed, `runningSince` is "", `paused` is
//!   true, and `baseSeconds` holds the worked time accumulated so far.
//!
//! Resuming opens a fresh entry; the segments simply add up in the database, so
//! pause needs no schema support. `baseSeconds` + the live segment is the
//! elapsed time QML shows.
//!
//! A session lives only in memory: if the app closes while paused, the session
//! is forgotten on restart (the logged segments are safe).
//!
//! **Crash recovery.** While a timer runs, QML pings `heartbeat()` every 30 s,
//! stamping `meta.timer_heartbeat`. On startup `db::recover_orphan_timer` closes
//! any entry a crash left open - billed only up to that heartbeat - and the
//! controller comes up as a *paused* session on that task (`recovered` is set so
//! the UI can say so). Closing the app while running therefore looks, next
//! launch, exactly like having hit Pause.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use rusqlite::Connection;

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
        // The session's task (running or paused); "" when no session.
        #[qproperty(QString, running_task_id, cxx_name = "runningTaskId")]
        #[qproperty(QString, running_task_title, cxx_name = "runningTaskTitle")]
        // Start of the current live segment; "" while paused.
        #[qproperty(QString, running_since, cxx_name = "runningSince")]
        // True when a session exists but its timer is paused.
        #[qproperty(bool, paused)]
        // Worked seconds accumulated from earlier segments of this session.
        #[qproperty(i32, base_seconds, cxx_name = "baseSeconds")]
        // True when this paused session was rebuilt from a crash on startup
        // (rather than an explicit Pause). Cleared once the user acts on it.
        #[qproperty(bool, recovered)]
        type TimerController = super::TimerControllerRust;
    }

    impl cxx_qt::Initialize for TimerController {}

    extern "RustQt" {
        /// Start a fresh session timing `task_id`, replacing any current one.
        #[qinvokable]
        fn start(self: Pin<&mut TimerController>, task_id: &QString);

        /// Pause the running session (no-op if nothing is running).
        #[qinvokable]
        fn pause(self: Pin<&mut TimerController>);

        /// Resume the paused session (no-op if not paused).
        #[qinvokable]
        fn resume(self: Pin<&mut TimerController>);

        /// End the session, closing any open entry.
        #[qinvokable]
        fn stop(self: Pin<&mut TimerController>);

        /// Record that the running timer is still alive (periodic ping from QML).
        #[qinvokable]
        fn heartbeat(self: Pin<&mut TimerController>);
    }
}

/// Backing state for [`qobject::TimerController`].
#[derive(Default)]
pub struct TimerControllerRust {
    conn: Option<Connection>,
    running_task_id: QString,
    running_task_title: QString,
    running_since: QString,
    paused: bool,
    base_seconds: i32,
    recovered: bool,
    /// Worked seconds from completed segments of the current session.
    accumulated: i64,
}

impl cxx_qt::Initialize for qobject::TimerController {
    fn initialize(mut self: Pin<&mut Self>) {
        let conn = match db::open(&db::default_path()) {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("uhatt: could not open database ({e}); running in-memory");
                db::open_in_memory().expect("in-memory database")
            }
        };
        self.as_mut().rust_mut().conn = Some(conn);

        // Did a previous run leave a timer going? Close it off and come back up
        // as a paused session on that task, as if Pause had been pressed.
        match db::recover_orphan_timer(self.db_conn()).unwrap_or(None) {
            Some(rec) => {
                self.as_mut().rust_mut().accumulated = rec.seconds;
                self.as_mut()
                    .set_base_seconds(rec.seconds.try_into().unwrap_or(i32::MAX));
                self.as_mut()
                    .set_running_task_id(QString::from(rec.task_id.as_str()));
                self.as_mut()
                    .set_running_task_title(QString::from(rec.task_title.as_str()));
                self.as_mut().set_running_since(QString::default());
                self.as_mut().set_paused(true);
                self.as_mut().set_recovered(true);
            }
            None => self.as_mut().refresh(),
        }
    }
}

impl qobject::TimerController {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("TimerController used before initialize()")
    }

    /// Mirror the database's open entry into the running-* properties and clear
    /// the paused flag. Leaves `base_seconds` / `accumulated` untouched so it is
    /// safe to call after `resume`. When the database has no open entry it also
    /// clears the session.
    fn refresh(mut self: Pin<&mut Self>) {
        let (id, title, since) = match db::running_timer(self.db_conn()).unwrap_or(None) {
            Some(running) => (running.task_id, running.task_title, running.start_ts),
            None => (String::new(), String::new(), String::new()),
        };
        let had_entry = !id.is_empty();
        self.as_mut()
            .set_running_task_id(QString::from(id.as_str()));
        self.as_mut()
            .set_running_task_title(QString::from(title.as_str()));
        self.as_mut()
            .set_running_since(QString::from(since.as_str()));
        self.as_mut().set_paused(false);
        if !had_entry {
            self.as_mut().rust_mut().accumulated = 0;
            self.as_mut().set_base_seconds(0);
        }
    }

    fn start(mut self: Pin<&mut Self>, task_id: &QString) {
        let task_id = task_id.to_string();
        if task_id.is_empty() {
            return;
        }
        // A new session: drop any accumulated time from the previous one.
        self.as_mut().rust_mut().accumulated = 0;
        self.as_mut().set_base_seconds(0);
        self.as_mut().set_recovered(false);
        if let Err(e) = db::start_timer(self.db_conn(), &task_id) {
            eprintln!("uhatt: start timer failed: {e}");
            return;
        }
        let _ = db::timer_heartbeat(self.db_conn());
        self.as_mut().refresh();
    }

    fn pause(mut self: Pin<&mut Self>) {
        if self.running_since.is_empty() {
            return; // nothing running
        }
        match db::stop_timer(self.db_conn()) {
            Ok(Some(entry)) => {
                let secs = db::entry_duration_seconds(self.db_conn(), &entry.id).unwrap_or(0);
                let total = self.accumulated + secs;
                self.as_mut().rust_mut().accumulated = total;
                self.as_mut()
                    .set_base_seconds(total.try_into().unwrap_or(i32::MAX));
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("uhatt: pause timer failed: {e}");
                return;
            }
        }
        self.as_mut().set_running_since(QString::default());
        self.as_mut().set_paused(true);
    }

    fn resume(mut self: Pin<&mut Self>) {
        if !self.paused {
            return;
        }
        let task_id = self.running_task_id.to_string();
        if task_id.is_empty() {
            return;
        }
        if let Err(e) = db::start_timer(self.db_conn(), &task_id) {
            eprintln!("uhatt: resume timer failed: {e}");
            return;
        }
        let _ = db::timer_heartbeat(self.db_conn());
        self.as_mut().set_recovered(false);
        self.as_mut().refresh();
    }

    fn stop(mut self: Pin<&mut Self>) {
        if let Err(e) = db::stop_timer(self.db_conn()) {
            eprintln!("uhatt: stop timer failed: {e}");
            return;
        }
        self.as_mut().rust_mut().accumulated = 0;
        self.as_mut().set_base_seconds(0);
        self.as_mut().set_recovered(false);
        self.as_mut().refresh();
    }

    fn heartbeat(self: Pin<&mut Self>) {
        if let Err(e) = db::timer_heartbeat(self.db_conn()) {
            eprintln!("uhatt: timer heartbeat failed: {e}");
        }
    }
}
