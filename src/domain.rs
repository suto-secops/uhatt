//! Plain-Rust domain types. No Qt types appear here or in `src/db/`, so the
//! whole layer is exercised directly by `cargo test` against an in-memory
//! database. The Qt bridge (`crate::tasks_model`) is a thin adapter on top.

use uuid::Uuid;

/// Stable identifier for every persisted row. UUIDv7 keeps rows sortable by
/// creation time and makes the future bulk-import feature an insert rather than
/// a migration.
pub type Id = String;

/// Generate a fresh row id.
pub fn new_id() -> Id {
    Uuid::now_v7().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Todo,
    Done,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Todo => "todo",
            TaskStatus::Done => "done",
        }
    }

    /// Parse a stored status, defaulting unknown values to `Todo`.
    pub fn from_db(s: &str) -> Self {
        match s {
            "done" => TaskStatus::Done,
            _ => TaskStatus::Todo,
        }
    }
}

/// A task. `parent_id` gives subtask nesting (self-reference); `project_id`
/// groups tasks under a project. `tracked` is a display flag only ("show the
/// time graph for this row") - time invested is always derived by summing time
/// entries, never accumulated on the row.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: Id,
    pub parent_id: Option<Id>,
    pub project_id: Option<Id>,
    pub title: String,
    pub notes: String,
    pub deadline: Option<String>,
    pub tracked: bool,
    pub status: TaskStatus,
    pub sort_order: f64,
    pub created_at: String,
    /// A one-off starting duration set at creation (e.g. by the quick-creation
    /// import), in seconds. Purely informational - never summed into time
    /// totals or the heatmap, never backed by a `time_entries` row.
    pub initial_time_seconds: Option<i64>,
}

impl Task {
    pub fn is_done(&self) -> bool {
        self.status == TaskStatus::Done
    }
}

/// A project groups root tasks. `tracked` is a display flag (show its time
/// graph); time is always derived by summing the entries under its tasks.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub tracked: bool,
    pub archived: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrySource {
    /// Recorded by the in-app timer.
    Timer,
    /// Entered or edited by hand.
    Manual,
}

impl EntrySource {
    pub fn from_db(s: &str) -> Self {
        match s {
            "manual" => EntrySource::Manual,
            _ => EntrySource::Timer,
        }
    }
}

/// A span of time spent on a task. `end_ts == None` marks the one currently
/// running timer.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeEntry {
    pub id: Id,
    pub task_id: Id,
    pub start_ts: String,
    pub end_ts: Option<String>,
    pub source: EntrySource,
    pub note: String,
    pub created_at: String,
}

// --- Quick creation: bulk text import for a new project --------------------
//
// One line per task: `title|timeinvested|deadline|description`, `|`
// separated. `||` (or a field that trims to "na"/"n/a", case-insensitive)
// marks an optional field as empty. Nesting is by indentation: more indent
// than the line above is a child (exactly one more unit of the configured
// indent character - space or tab, never mixed); same indent is a sibling;
// less indent must land exactly on some ancestor's indent, or it's an error.
// Blank lines are skipped. Kept here, independent of any live `Settings`
// object, so it's directly unit-testable.

/// Parse a duration like `"2h"`, `"45m"`, `"2h30m"`, `"2h 30m"`, `"1.5h"` into
/// whole seconds. A bare number with no unit is rejected - it's genuinely
/// ambiguous (minutes? hours?) and this format fails loud, not silently.
// TODO(quick-creation UI): wired up by the Quick Creation page (next PR);
// exercised by tests until then, so an explicit allow beats a dead-code warning.
#[allow(dead_code)]
pub fn parse_duration(s: &str) -> Result<i64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".to_owned());
    }
    let lower = s.to_ascii_lowercase();
    let mut rest = lower.as_str();
    let mut seconds: i64 = 0;
    let mut matched = false;

    if let Some(h_pos) = rest.find('h') {
        let (num, after) = rest.split_at(h_pos);
        let hours: f64 = num
            .trim()
            .parse()
            .map_err(|_| format!("bad hours in \"{s}\""))?;
        seconds += (hours * 3600.0).round() as i64;
        rest = after[1..].trim_start();
        matched = true;
    }
    if let Some(m_pos) = rest.find('m') {
        let (num, after) = rest.split_at(m_pos);
        let minutes: i64 = num
            .trim()
            .parse()
            .map_err(|_| format!("bad minutes in \"{s}\""))?;
        seconds += minutes * 60;
        rest = &after[1..];
        matched = true;
    }
    if !rest.trim().is_empty() {
        return Err(format!("unexpected trailing text in \"{s}\""));
    }
    if !matched {
        return Err(format!(
            "\"{s}\" has no h/m unit - write e.g. \"2h\", \"45m\", \"2h30m\""
        ));
    }
    Ok(seconds)
}

/// True for a structurally valid `YYYY-MM-DD` calendar date string.
#[allow(dead_code)]
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..].iter().all(u8::is_ascii_digit)
}

/// Parse a date typed in display format `format` (0 ISO `YYYY-MM-DD`, 1 US
/// `MM/DD/YYYY`, 2 EU `DD/MM/YYYY` - matching `Settings.dateFormat`) into
/// ISO `YYYY-MM-DD`. Mirrors the QML-side `parseDate` used by the deadline
/// popup, duplicated here so quick-creation parsing doesn't need a live
/// `Settings` object.
#[allow(dead_code)]
pub fn parse_date_with_format(s: &str, format: i32) -> Option<String> {
    let s = s.trim();
    if format == 0 {
        return is_iso_date(s).then(|| s.to_owned());
    }
    let parts: Vec<&str> = s.split('/').collect();
    let [a, b, y] = parts[..] else { return None };
    let (mm, dd) = if format == 1 { (a, b) } else { (b, a) };
    if y.len() != 4 || mm.len() != 2 || dd.len() != 2 {
        return None;
    }
    let y: u32 = y.parse().ok()?;
    let mm: u32 = mm.parse().ok()?;
    let dd: u32 = dd.parse().ok()?;
    if !(1..=12).contains(&mm) || !(1..=31).contains(&dd) {
        return None;
    }
    Some(format!("{y:04}-{mm:02}-{dd:02}"))
}

/// `true` for an empty field or an explicit "na" / "n/a" marker.
#[allow(dead_code)]
fn is_blank_field(s: &str) -> bool {
    let s = s.trim();
    s.is_empty() || s.eq_ignore_ascii_case("na") || s.eq_ignore_ascii_case("n/a")
}

/// One successfully parsed task line from quick-creation input.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct QuickCreateTask {
    /// Nesting depth, 0 = root.
    pub depth: usize,
    pub title: String,
    pub initial_time_seconds: Option<i64>,
    /// ISO `YYYY-MM-DD`.
    pub deadline: Option<String>,
    pub description: String,
}

/// One line of quick-creation input, parsed or not. `line_no` is 1-based
/// against the original pasted text, for error display.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct QuickCreateLine {
    pub line_no: usize,
    pub result: Result<QuickCreateTask, String>,
}

/// Parse one line's `title|time|deadline|description` content (indentation
/// already stripped) into a task, or an error message.
#[allow(dead_code)]
fn parse_task_fields(content: &str, date_format: i32) -> Result<QuickCreateTask, String> {
    let parts: Vec<&str> = content.splitn(4, '|').collect();
    let [title, time, deadline, description] = parts[..] else {
        return Err(
            "expected `title|timeinvested|deadline|description` (3 `|` separators)".to_owned(),
        );
    };
    let title = title.trim();
    if title.is_empty() {
        return Err("title is required".to_owned());
    }
    let initial_time_seconds = if is_blank_field(time) {
        None
    } else {
        Some(parse_duration(time)?)
    };
    let deadline = if is_blank_field(deadline) {
        None
    } else {
        Some(
            parse_date_with_format(deadline, date_format)
                .ok_or_else(|| format!("bad deadline \"{}\"", deadline.trim()))?,
        )
    };
    let description = if is_blank_field(description) {
        String::new()
    } else {
        description.trim().to_owned()
    };
    Ok(QuickCreateTask {
        depth: 0, // filled in by the caller, which tracks the indent stack
        title: title.to_owned(),
        initial_time_seconds,
        deadline,
        description,
    })
}

/// Leading run of `indent_char`, or an error if it's mixed with the other
/// whitespace character (the classic pasted-from-elsewhere footgun).
#[allow(dead_code)]
fn leading_indent(line: &str, indent_char: char) -> Result<usize, String> {
    let other = if indent_char == ' ' { '\t' } else { ' ' };
    let mut n = 0;
    for c in line.chars() {
        if c == indent_char {
            n += 1;
        } else if c == other {
            let bad = if other == '\t' { "tabs" } else { "spaces" };
            return Err(format!(
                "line uses {bad}, but indent is set to \"{indent_char}\""
            ));
        } else {
            break;
        }
    }
    Ok(n)
}

/// Parse the whole quick-creation text area into one entry per non-blank
/// line - each either a task (with `depth` resolved from indentation) or a
/// line-level error. Blank lines are silently skipped.
#[allow(dead_code)]
pub fn parse_quick_creation(
    text: &str,
    indent_char: char,
    date_format: i32,
) -> Vec<QuickCreateLine> {
    let mut out = Vec::new();
    // stack[i] = the indent-character count of the currently open line at
    // depth i. A child must be exactly one more than the deepest open
    // line; a dedent must land exactly on some entry already on the stack.
    let mut stack: Vec<usize> = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let result = (|| -> Result<QuickCreateTask, String> {
            let n = leading_indent(raw, indent_char)?;
            let content = &raw[n..];
            let depth = if stack.is_empty() {
                if n != 0 {
                    return Err("first task can't be indented".to_owned());
                }
                stack.push(0);
                0
            } else {
                let top = *stack.last().unwrap();
                if n == top + 1 {
                    stack.push(n);
                    stack.len() - 1
                } else if n == top {
                    stack.len() - 1
                } else if n < top {
                    while let Some(&t) = stack.last() {
                        if t == n {
                            break;
                        } else if t < n {
                            return Err("indentation doesn't match any parent level".to_owned());
                        }
                        stack.pop();
                    }
                    if stack.last() != Some(&n) {
                        return Err("indentation doesn't match any parent level".to_owned());
                    }
                    stack.len() - 1
                } else {
                    return Err(
                        "indented more than one level deeper than the line above".to_owned()
                    );
                }
            };
            let mut task = parse_task_fields(content, date_format)?;
            task.depth = depth;
            Ok(task)
        })();
        out.push(QuickCreateLine { line_no, result });
    }
    out
}

/// Which tasks the task view should show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectFilter {
    /// Every task, regardless of project.
    All,
    /// Only tasks not assigned to any project.
    Unfiled,
    /// Only tasks in the project with this id.
    Only(Id),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parses_hours_minutes_and_combinations() {
        assert_eq!(parse_duration("2h"), Ok(7200));
        assert_eq!(parse_duration("45m"), Ok(2700));
        assert_eq!(parse_duration("2h30m"), Ok(9000));
        assert_eq!(parse_duration("2h 30m"), Ok(9000));
        assert_eq!(parse_duration("1.5h"), Ok(5400));
        assert_eq!(parse_duration(" 2H "), Ok(7200));
    }

    #[test]
    fn duration_rejects_bare_numbers_and_garbage() {
        assert!(parse_duration("90").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("2h garbage").is_err());
    }

    #[test]
    fn date_parses_per_selected_format() {
        assert_eq!(
            parse_date_with_format("2026-09-30", 0),
            Some("2026-09-30".to_owned())
        );
        assert_eq!(
            parse_date_with_format("09/30/2026", 1),
            Some("2026-09-30".to_owned())
        );
        assert_eq!(
            parse_date_with_format("30/09/2026", 2),
            Some("2026-09-30".to_owned())
        );
        assert_eq!(parse_date_with_format("30/09/2026", 1), None); // month 30 is invalid
        assert_eq!(parse_date_with_format("garbage", 0), None);
    }

    #[test]
    fn quick_create_builds_correct_depths_with_pipe_format() {
        let text = "\
math homework||2025-09-30|boring maths
 exercise 1|||
 exercise 2|||
  exercise 2.1|||
  exercise 2.2|||
 exercise 3|||
english project|||essays n stuff";
        let lines = parse_quick_creation(text, ' ', 0);
        let depths: Vec<usize> = lines
            .iter()
            .map(|l| l.result.as_ref().unwrap().depth)
            .collect();
        assert_eq!(depths, vec![0, 1, 1, 2, 2, 1, 0]);
        assert_eq!(
            lines[0].result.as_ref().unwrap().deadline,
            Some("2025-09-30".to_owned())
        );
        assert_eq!(
            lines[0].result.as_ref().unwrap().description,
            "boring maths"
        );
        assert_eq!(lines[3].result.as_ref().unwrap().title, "exercise 2.1");
    }

    #[test]
    fn quick_create_skips_blank_lines() {
        let text = "root|||\n\n child|||\n";
        let lines = parse_quick_creation(text, ' ', 0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line_no, 1);
        assert_eq!(lines[1].line_no, 3);
    }

    #[test]
    fn quick_create_errors_on_a_jump_of_more_than_one_level() {
        let text = "root|||\n   child|||"; // 3 spaces, not 1
        let lines = parse_quick_creation(text, ' ', 0);
        assert!(lines[1].result.is_err());
    }

    // No test for "dedent matches no ancestor": under this strict scheme a
    // child is always exactly one indent unit deeper than its parent, so the
    // indent-stack is always the contiguous run 0..depth - any dedent value
    // that isn't a jump-too-far is guaranteed to match some ancestor. The
    // stack-search code in `parse_quick_creation` keeps the check anyway, as
    // a defensive fallback, but there's no legitimate input that reaches it.

    #[test]
    fn quick_create_errors_on_first_line_indented() {
        let text = " root|||";
        let lines = parse_quick_creation(text, ' ', 0);
        assert!(lines[0].result.is_err());
    }

    #[test]
    fn quick_create_errors_on_mixed_whitespace() {
        let text = "root|||\n\tchild|||"; // configured for space, this line uses a tab
        let lines = parse_quick_creation(text, ' ', 0);
        assert!(lines[1].result.is_err());
    }

    #[test]
    fn quick_create_errors_on_missing_title() {
        let text = "|||description only";
        let lines = parse_quick_creation(text, ' ', 0);
        assert!(lines[0].result.is_err());
    }

    #[test]
    fn quick_create_na_marker_means_empty() {
        let text = "task|na|N/A|na";
        let lines = parse_quick_creation(text, ' ', 0);
        let t = lines[0].result.as_ref().unwrap();
        assert_eq!(t.initial_time_seconds, None);
        assert_eq!(t.deadline, None);
        assert_eq!(t.description, "");
    }

    #[test]
    fn quick_create_supports_tab_indent() {
        let text = "root|||\n\tchild|||";
        let lines = parse_quick_creation(text, '\t', 0);
        assert!(lines[1].result.is_ok());
        assert_eq!(lines[1].result.as_ref().unwrap().depth, 1);
    }
}
