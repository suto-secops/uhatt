//! `QuickCreate` - the data behind the "Quick creation" page: parses the
//! pasted `title|time|deadline|description` text live (for the preview) and
//! commits it as a new project + task tree on demand.

use core::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use rusqlite::Connection;
use serde::Serialize;

use crate::db;
use crate::domain;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        type QuickCreate = super::QuickCreateRust;
    }

    impl cxx_qt::Initialize for QuickCreate {}

    extern "RustQt" {
        /// Parse `text` (indent unit: tab when `indent_tab`, else space;
        /// deadlines read per `date_format`) into a JSON array, one entry
        /// per non-blank line, for the live preview - each either
        /// `{lineNo, ok:true, depth, title, error:""}` or
        /// `{lineNo, ok:false, depth:0, title:"", error}`.
        #[qinvokable]
        fn parse(self: &QuickCreate, text: &QString, indent_tab: bool, date_format: i32)
            -> QString;

        /// Re-parses `text` itself (never trusts a client-cached parse) and,
        /// if every line is valid and `project_name` isn't blank, creates the
        /// project and its whole task tree as one undoable action. Returns
        /// JSON `{"ok":true,"projectId":"...","taskCount":N}` on success, or
        /// `{"ok":false,"error":"..."}`.
        #[qinvokable]
        fn create(
            self: Pin<&mut QuickCreate>,
            project_name: &QString,
            text: &QString,
            indent_tab: bool,
            date_format: i32,
        ) -> QString;
    }
}

/// Backing state for [`qobject::QuickCreate`].
#[derive(Default)]
pub struct QuickCreateRust {
    conn: Option<Connection>,
}

impl cxx_qt::Initialize for qobject::QuickCreate {
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

fn indent_char(indent_tab: bool) -> char {
    if indent_tab {
        '\t'
    } else {
        ' '
    }
}

#[derive(Serialize)]
struct PreviewLine<'a> {
    #[serde(rename = "lineNo")]
    line_no: usize,
    ok: bool,
    depth: usize,
    title: &'a str,
    error: &'a str,
}

impl qobject::QuickCreate {
    fn db_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("QuickCreate used before initialize()")
    }

    fn parse(&self, text: &QString, indent_tab: bool, date_format: i32) -> QString {
        let text = text.to_string();
        let parsed = domain::parse_quick_creation(&text, indent_char(indent_tab), date_format);
        let out: Vec<PreviewLine> = parsed
            .iter()
            .map(|l| match &l.result {
                Ok(t) => PreviewLine {
                    line_no: l.line_no,
                    ok: true,
                    depth: t.depth,
                    title: &t.title,
                    error: "",
                },
                Err(e) => PreviewLine {
                    line_no: l.line_no,
                    ok: false,
                    depth: 0,
                    title: "",
                    error: e,
                },
            })
            .collect();
        let json = serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_owned());
        QString::from(json.as_str())
    }

    fn create(
        self: Pin<&mut Self>,
        project_name: &QString,
        text: &QString,
        indent_tab: bool,
        date_format: i32,
    ) -> QString {
        let fail = |msg: String| {
            let json = serde_json::json!({ "ok": false, "error": msg }).to_string();
            QString::from(json.as_str())
        };
        let project_name = project_name.to_string();
        let project_name = project_name.trim();
        if project_name.is_empty() {
            return fail("Project name is required".to_owned());
        }
        let text = text.to_string();
        let parsed = domain::parse_quick_creation(&text, indent_char(indent_tab), date_format);
        if parsed.is_empty() {
            return fail("Nothing to create - the text area is empty".to_owned());
        }
        let mut tasks = Vec::with_capacity(parsed.len());
        for line in &parsed {
            match &line.result {
                Ok(t) => tasks.push(t.clone()),
                Err(e) => return fail(format!("Line {}: {e}", line.line_no)),
            }
        }
        match db::actions::import_quick_creation(self.db_conn(), project_name, &tasks) {
            Ok(outcome) => {
                let json = serde_json::json!({
                    "ok": true,
                    "projectId": outcome.project_id,
                    "taskCount": outcome.task_count,
                })
                .to_string();
                QString::from(json.as_str())
            }
            Err(e) => fail(format!("Couldn't create the project: {e}")),
        }
    }
}
