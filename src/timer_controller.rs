//! `TimerController` - the app's single running timer, surfaced to QML.
//!
//! Not a list model: a plain QObject whose Q_PROPERTYs mirror `db::running_timer`.
//! The elapsed-time display is QML's job (a 1 s tick over `runningSince`).
//!
//! Crash recovery: an entry left open by a previous run simply stays running and
//! shows up here on startup. No data is lost; the user can stop it and, once the
//! manual-entry editor exists, trim it. A heartbeat table can refine this later.

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
        #[qproperty(QString, running_task_id, cxx_name = "runningTaskId")]
        #[qproperty(QString, running_task_title, cxx_name = "runningTaskTitle")]
        #[qproperty(QString, running_since, cxx_name = "runningSince")]
        type TimerController = super::TimerControllerRust;
    }

    impl cxx_qt::Initialize for TimerController {}

    extern "RustQt" {
        /// Start timing `task_id`, switching from any other running task.
        #[qinvokable]
        fn start(self: Pin<&mut TimerController>, task_id: &QString);

        /// Stop the running timer, if any.
        #[qinvokable]
        fn stop(self: Pin<&mut TimerController>);

        /// Stop `task_id` if it is already running, otherwise start it.
        #[qinvokable]
        fn toggle(self: Pin<&mut TimerController>, task_id: &QString);
    }
}

/// Backing state for [`qobject::TimerController`].
#[derive(Default)]
pub struct TimerControllerRust {
    conn: Option<Connection>,
    running_task_id: QString,
    running_task_title: QString,
    running_since: QString,
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
        self.as_mut().refresh();
    }
}

impl qobject::TimerController {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("TimerController used before initialize()")
    }

    /// Re-read the running timer from the DB into the Q_PROPERTYs.
    fn refresh(mut self: Pin<&mut Self>) {
        let (id, title, since) = match db::running_timer(self.db_conn()).unwrap_or(None) {
            Some(running) => (running.task_id, running.task_title, running.start_ts),
            None => (String::new(), String::new(), String::new()),
        };
        self.as_mut()
            .set_running_task_id(QString::from(id.as_str()));
        self.as_mut()
            .set_running_task_title(QString::from(title.as_str()));
        self.as_mut()
            .set_running_since(QString::from(since.as_str()));
    }

    fn start(mut self: Pin<&mut Self>, task_id: &QString) {
        let task_id = task_id.to_string();
        if task_id.is_empty() {
            return;
        }
        if let Err(e) = db::start_timer(self.db_conn(), &task_id) {
            eprintln!("uhatt: start timer failed: {e}");
            return;
        }
        self.as_mut().refresh();
    }

    fn stop(mut self: Pin<&mut Self>) {
        if let Err(e) = db::stop_timer(self.db_conn()) {
            eprintln!("uhatt: stop timer failed: {e}");
            return;
        }
        self.as_mut().refresh();
    }

    fn toggle(self: Pin<&mut Self>, task_id: &QString) {
        if self.running_task_id == *task_id {
            self.stop();
        } else {
            self.start(task_id);
        }
    }
}
