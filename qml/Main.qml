import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// URI must match the QmlModule in build.rs
import dev.suto.uhatt

ApplicationWindow {
    id: root

    width: 860
    height: 580
    visible: true
    title: qsTr("uhatt")

    // Today plus `days`, formatted as an ISO calendar date.
    function isoPlusDays(days) {
        let d = new Date()
        d.setDate(d.getDate() + days)
        return Qt.formatDate(d, "yyyy-MM-dd")
    }

    // ---- Date format (settings.dateFormat: 0 ISO, 1 US, 2 EU) -----------
    // Display and input for deadlines follow the user's choice; everything
    // is still stored and sent to Rust as ISO "yyyy-MM-dd" - only these
    // functions know about the other two formats.

    // "yyyy-MM-dd" -> [y, m, d] ints, or null if not parseable.
    function isoToParts(iso) {
        if (!iso)
            return null
        let p = iso.split("-")
        if (p.length !== 3)
            return null
        let y = parseInt(p[0]), m = parseInt(p[1]), d = parseInt(p[2])
        return isNaN(y) || isNaN(m) || isNaN(d) ? null : [y, m, d]
    }

    // An ISO deadline as the user prefers to see it. "" stays "".
    function fmtDate(iso) {
        let p = root.isoToParts(iso)
        if (!p)
            return iso
        let pad2 = n => (n < 10 ? "0" : "") + n
        switch (settings.dateFormat) {
        case 1: return pad2(p[1]) + "/" + pad2(p[2]) + "/" + p[0]  // MM/DD/YYYY
        case 2: return pad2(p[2]) + "/" + pad2(p[1]) + "/" + p[0]  // DD/MM/YYYY
        default: return iso                                        // YYYY-MM-DD
        }
    }

    // The reverse of fmtDate: text in the current format -> ISO, or "" if
    // it isn't a plausible date.
    function parseDate(text) {
        let parts = text.split(/[\/-]/).map(s => parseInt(s, 10))
        if (parts.length !== 3 || parts.some(isNaN))
            return ""
        let y, m, d
        if (settings.dateFormat === 1) { m = parts[0]; d = parts[1]; y = parts[2] }
        else if (settings.dateFormat === 2) { d = parts[0]; m = parts[1]; y = parts[2] }
        else { y = parts[0]; m = parts[1]; d = parts[2] }
        if (y < 1000 || m < 1 || m > 12 || d < 1 || d > 31)
            return ""
        let pad2 = n => (n < 10 ? "0" : "") + n
        return y + "-" + pad2(m) + "-" + pad2(d)
    }

    function dateInputMask() {
        return settings.dateFormat === 0 ? "9999-99-99" : "99/99/9999"
    }

    function dateInputLabel() {
        switch (settings.dateFormat) {
        case 1: return qsTr("Deadline (MM/DD/YYYY)")
        case 2: return qsTr("Deadline (DD/MM/YYYY)")
        default: return qsTr("Deadline (YYYY-MM-DD)")
        }
    }

    // Sidebar task count for a "Views" entry's `projectFilter` key.
    function viewTaskCount(key) {
        switch (key) {
        case "": return tasks.countAll()
        case "unfiled": return tasks.countUnfiled()
        case "duetoday": return tasks.countDueToday()
        case "deadlined": return tasks.countDeadlined()
        case "finished": return tasks.countFinished()
        default: return 0
        }
    }

    // `baseSecs` of already-counted time plus the live segment since `sinceIso`
    // (pass "" for a paused timer), as HH:MM:SS.
    function fmtElapsed(baseSecs, sinceIso) {
        let live = sinceIso
            ? Math.max(0, Math.floor((Date.now() - Date.parse(sinceIso)) / 1000))
            : 0
        let secs = Math.max(0, baseSecs) + live
        let parts = [Math.floor(secs / 3600), Math.floor(secs % 3600 / 60), secs % 60]
        return parts.map(n => n < 10 ? "0" + n : "" + n).join(":")
    }

    // Bumped once a second while a timer runs, to re-evaluate elapsed-time bindings.
    property int tick: 0

    // A graceful close still counts the time worked up to this moment: stamp a
    // final heartbeat so startup recovery closes the open entry at ~now, not at
    // the last periodic tick. A hard crash falls back to that periodic tick.
    onClosing: if (timer.runningSince !== "") timer.heartbeat()

    // Now, as "yyyy-MM-dd HH:mm" - the format the entry editor expects.
    function localNow() {
        return Qt.formatDateTime(new Date(), "yyyy-MM-dd hh:mm")
    }

    TaskListModel {
        id: tasks
    }
    ProjectListModel {
        id: projects
    }
    TimerController {
        id: timer
    }
    EntriesModel {
        id: entries
    }
    GraphModel {
        id: graph
    }
    Settings {
        id: settings
    }
    Calendar {
        id: calendar
    }
    ActionsLog {
        id: actionsLog
    }
    QuickCreate {
        id: quickCreate
    }

    // Which page fills the centre pane: "tasks", "calendar", "actions" or
    // "quickcreate".
    property string mainView: "tasks"
    // Sidebar width - fixed unless the user drags the divider.
    property real sidebarWidth: 200

    // Both pages load their data once at startup; re-read each time the page
    // is opened so edits made elsewhere show up.
    onMainViewChanged: {
        if (mainView === "calendar") calendar.reload()
        if (mainView === "actions") actionsLog.reload()
    }

    // Floating chip shown under the cursor while a task is being dragged onto
    // another to re-parent it. Lives at the window level so it isn't clipped by
    // the task list.
    Control {
        id: dragProxy
        parent: Overlay.overlay
        z: 9999
        visible: Drag.active
        property int sourceRow: -1
        property string label: ""

        Drag.active: false
        Drag.hotSpot.x: 12
        Drag.hotSpot.y: height / 2

        padding: 6
        background: Rectangle {
            color: palette.highlight
            radius: 4
            opacity: 0.92
        }
        contentItem: Label {
            text: dragProxy.label
            color: palette.highlightedText
            font.pointSize: 9
        }
    }

    Dialog {
        id: graphDialog

        property string subject: ""

        title: qsTr("Time invested — %1").arg(subject)
        modal: true
        anchors.centerIn: Overlay.overlay
        width: 800
        height: 400
        standardButtons: Dialog.Close

        // Highlight colour at a given 0..1 intensity; the faint "no time" tint
        // when below the floor.
        function heat(intensity) {
            if (intensity < 0.1)
                return Qt.rgba(palette.text.r, palette.text.g, palette.text.b, 0.08)
            return Qt.rgba(palette.highlight.r, palette.highlight.g, palette.highlight.b,
                           0.2 + 0.8 * Math.min(1, intensity))
        }

        // ISO "yyyy-MM-dd" to a local Date (component-wise, so no TZ shift).
        function isoToDate(s) {
            let p = s.split("-")
            return new Date(parseInt(p[0]), parseInt(p[1]) - 1, parseInt(p[2]))
        }

        // Label for the period on show: "2026" / "September 2026" / "7–13 Sep 2026".
        function periodLabel() {
            let d = graphDialog.isoToDate(graph.anchor)
            if (graph.range === 2)
                return "" + d.getFullYear()
            if (graph.range === 1)
                return Qt.locale().standaloneMonthName(d.getMonth()) + " " + d.getFullYear()
            let end = new Date(d)
            end.setDate(d.getDate() + 6)
            let a = Qt.locale().standaloneMonthName(d.getMonth(), Locale.ShortFormat)
            let b = Qt.locale().standaloneMonthName(end.getMonth(), Locale.ShortFormat)
            return d.getDate() + " " + a + " – " + end.getDate() + " " + b + " " + end.getFullYear()
        }

        // Year view only: the grid column each month's 1st falls in. The grid
        // starts on the Monday on-or-before Jan 1, so column = whole weeks.
        function monthColumns() {
            let y = graphDialog.isoToDate(graph.anchor).getFullYear()
            let jan1 = new Date(y, 0, 1)
            let isoDow = (jan1.getDay() + 6) % 7   // Mon=0 .. Sun=6
            let gridStart = new Date(y, 0, 1 - isoDow)
            let out = []
            for (let m = 0; m < 12; m++) {
                let col = Math.floor((new Date(y, m, 1) - gridStart) / (7 * 86400000))
                out.push({ name: Qt.locale().standaloneMonthName(m, Locale.ShortFormat), col: col })
            }
            return out
        }

        contentItem: ColumnLayout {
            spacing: 12

            // ---- Header: total + range switch + period stepper ---------
            RowLayout {
                Layout.fillWidth: true
                spacing: 6
                Label {
                    text: qsTr("Total: %1").arg(graph.totalText)
                    font.bold: true
                }
                Item { Layout.fillWidth: true }
                Repeater {
                    model: [qsTr("Week"), qsTr("Month"), qsTr("Year")]
                    delegate: Button {
                        required property int index
                        required property string modelData
                        text: modelData
                        checkable: true
                        checked: graph.range === index
                        onClicked: graph.range = index
                    }
                }
                Item { Layout.preferredWidth: 8 }
                ToolButton {
                    text: "‹"
                    onClicked: graph.step(-1)
                }
                Label {
                    text: graphDialog.periodLabel()
                    font.bold: true
                    horizontalAlignment: Text.AlignHCenter
                    Layout.minimumWidth: 128
                }
                ToolButton {
                    text: "›"
                    enabled: !graph.atLatest
                    onClicked: graph.step(1)
                }
            }

            // ---- Heatmap ----------------------------------------------
            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true

                Label {
                    anchors.centerIn: parent
                    visible: graph.maxSeconds <= 0
                    text: qsTr("No time recorded in this %1")
                          .arg([qsTr("week"), qsTr("month"), qsTr("year")][graph.range])
                    opacity: 0.5
                }

                // ---- Year: the GitHub-style 53-week grid --------------
                ColumnLayout {
                    id: yearBody
                    anchors.top: parent.top
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.topMargin: 6
                    visible: graph.maxSeconds > 0 && graph.range === 2
                    spacing: 6

                    readonly property int weekdayColWidth: 26

                    Item {
                        Layout.leftMargin: yearBody.weekdayColWidth + 4
                        Layout.preferredWidth: 53 * (yearGrid.cell + yearGrid.columnSpacing)
                        Layout.preferredHeight: 12
                        Repeater {
                            model: graphDialog.monthColumns()
                            delegate: Label {
                                required property var modelData
                                x: modelData.col * (yearGrid.cell + yearGrid.columnSpacing)
                                text: modelData.name
                                font.pointSize: 7
                                opacity: 0.6
                            }
                        }
                    }

                    RowLayout {
                        spacing: 4

                        ColumnLayout {
                            Layout.preferredWidth: yearBody.weekdayColWidth
                            spacing: yearGrid.rowSpacing
                            Repeater {
                                model: [qsTr("Mon"), "", qsTr("Wed"), "", qsTr("Fri"), "", ""]
                                delegate: Label {
                                    required property string modelData
                                    text: modelData
                                    font.pointSize: 7
                                    opacity: 0.6
                                    Layout.preferredHeight: yearGrid.cell
                                    verticalAlignment: Text.AlignVCenter
                                }
                            }
                        }

                        // 53 columns x 7 rows, filled column-by-column: the
                        // model is date-ordered (week then weekday) to match.
                        Grid {
                            id: yearGrid
                            readonly property int cell: 11
                            rows: 7
                            flow: Grid.TopToBottom
                            rowSpacing: 3
                            columnSpacing: 3

                            Repeater {
                                model: graph.range === 2 ? graph : 0
                                delegate: Rectangle {
                                    required property string date
                                    required property real seconds
                                    required property string hoursText
                                    required property bool inPeriod

                                    width: yearGrid.cell
                                    height: yearGrid.cell
                                    radius: 2
                                    color: !inPeriod
                                           ? "transparent"
                                           : graphDialog.heat(graph.maxSeconds > 0 ? seconds / graph.maxSeconds : 0)

                                    HoverHandler { id: yHover; enabled: inPeriod }
                                    ToolTip.visible: yHover.hovered
                                    ToolTip.text: date + " · " + hoursText
                                }
                            }
                        }
                    }
                }

                // ---- Week / Month: a calendar grid -------------------
                ColumnLayout {
                    id: calBody
                    anchors.centerIn: parent
                    visible: graph.maxSeconds > 0 && graph.range !== 2
                    spacing: 4

                    readonly property int cell: 34

                    Row {
                        spacing: calGrid.columnSpacing
                        Repeater {
                            model: [qsTr("Mon"), qsTr("Tue"), qsTr("Wed"), qsTr("Thu"),
                                    qsTr("Fri"), qsTr("Sat"), qsTr("Sun")]
                            delegate: Label {
                                required property string modelData
                                width: calBody.cell
                                horizontalAlignment: Text.AlignHCenter
                                text: modelData
                                font.pointSize: 8
                                opacity: 0.6
                            }
                        }
                    }

                    // columns:7, filled row-by-row: the model is date-ordered
                    // starting on a Monday, so weeks become rows.
                    Grid {
                        id: calGrid
                        columns: 7
                        flow: Grid.LeftToRight
                        rowSpacing: 4
                        columnSpacing: 4

                        Repeater {
                            model: graph.range !== 2 ? graph : 0
                            delegate: Rectangle {
                                required property string date
                                required property real seconds
                                required property string hoursText
                                required property bool inPeriod

                                width: calBody.cell
                                height: calBody.cell
                                radius: 3
                                color: !inPeriod
                                       ? "transparent"
                                       : graphDialog.heat(graph.maxSeconds > 0 ? seconds / graph.maxSeconds : 0)

                                Label {
                                    anchors.left: parent.left
                                    anchors.top: parent.top
                                    anchors.margins: 3
                                    text: parseInt(parent.date.substring(8, 10))
                                    font.pointSize: 8
                                    opacity: parent.inPeriod ? 0.85 : 0.25
                                }

                                HoverHandler { id: cHover; enabled: inPeriod }
                                ToolTip.visible: cHover.hovered
                                ToolTip.text: date + " · " + hoursText
                            }
                        }
                    }
                }

            }

            // ---- Legend --------------------------------------------------
            RowLayout {
                Layout.alignment: Qt.AlignRight
                visible: graph.maxSeconds > 0
                spacing: 4
                Label { text: qsTr("Less"); font.pointSize: 7; opacity: 0.6 }
                Repeater {
                    model: [0, 0.35, 0.6, 0.85, 1.0]
                    delegate: Rectangle {
                        required property real modelData
                        width: 11
                        height: 11
                        radius: 2
                        color: graphDialog.heat(modelData)
                    }
                }
                Label { text: qsTr("More"); font.pointSize: 7; opacity: 0.6 }
            }
        }
    }

    Dialog {
        id: entriesDialog

        property string taskTitle: ""
        // The list and the add-form are never shown together.
        property bool adding: false

        title: qsTr("Time entries — %1").arg(taskTitle)
        modal: true
        anchors.centerIn: Overlay.overlay
        width: 540
        height: 460
        standardButtons: Dialog.Close
        onOpened: {
            entriesDialog.adding = false
            addStart.text = root.localNow()
            addEnd.text = root.localNow()
        }

        contentItem: ColumnLayout {
            spacing: 8

            // ---- Header: total + add toggle -----------------------------
            RowLayout {
                Layout.fillWidth: true
                Label {
                    // For a parent task, its own entries and the subtree rollup.
                    text: entries.hasSubtasks
                          ? qsTr("This task: %1  ·  incl. subtasks: %2")
                              .arg(entries.totalText).arg(entries.subtreeTotalText)
                          : qsTr("Total: %1").arg(entries.totalText)
                    font.bold: true
                }
                Item { Layout.fillWidth: true }
                Button {
                    visible: !entriesDialog.adding
                    text: qsTr("Add entry")
                    onClicked: {
                        addStart.text = root.localNow()
                        addEnd.text = root.localNow()
                        entriesDialog.adding = true
                    }
                }
            }

            // ---- List of entries --------------------------------------
            ListView {
                id: entryList
                visible: !entriesDialog.adding
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: entries
                spacing: 4

                delegate: Frame {
                    id: erow

                    required property int index
                    required property string start
                    required property string end
                    required property string durationText
                    required property bool running

                    width: entryList.width
                    padding: 6

                    function commit() {
                        if (!erow.running)
                            entries.update(erow.index, startField.text, endField.text)
                    }

                    RowLayout {
                        anchors.fill: parent
                        spacing: 6

                        TextField {
                            id: startField
                            Layout.fillWidth: true
                            text: erow.start
                            enabled: !erow.running
                            onEditingFinished: erow.commit()
                        }
                        Label { text: qsTr("to") }
                        TextField {
                            id: endField
                            Layout.fillWidth: true
                            text: erow.running ? qsTr("running") : erow.end
                            enabled: !erow.running
                            onEditingFinished: erow.commit()
                        }
                        Label {
                            text: erow.durationText
                            Layout.preferredWidth: 52
                            horizontalAlignment: Text.AlignRight
                            opacity: 0.8
                        }
                        // Drawn "×" - the font has no cross glyph.
                        Button {
                            implicitWidth: 26
                            implicitHeight: 26
                            padding: 0
                            enabled: !erow.running
                            ToolTip.text: qsTr("Delete entry")
                            ToolTip.visible: hovered
                            onClicked: entries.remove(erow.index)
                            contentItem: Item {
                                Repeater {
                                    model: 2
                                    delegate: Rectangle {
                                        required property int index
                                        anchors.centerIn: parent
                                        width: 12
                                        height: 2
                                        radius: 1
                                        color: palette.buttonText
                                        rotation: index === 0 ? 45 : -45
                                    }
                                }
                            }
                        }
                    }
                }

                Label {
                    anchors.centerIn: parent
                    visible: entryList.count === 0
                    text: qsTr("No time recorded yet")
                    opacity: 0.5
                }
            }

            // ---- Add form (replaces the list) -------------------------
            ColumnLayout {
                visible: entriesDialog.adding
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 8

                GridLayout {
                    columns: 2
                    columnSpacing: 8
                    rowSpacing: 6
                    Layout.fillWidth: true

                    Label { text: qsTr("Start") }
                    TextField {
                        id: addStart
                        Layout.fillWidth: true
                        placeholderText: "YYYY-MM-DD HH:MM"
                    }
                    Label { text: qsTr("End") }
                    TextField {
                        id: addEnd
                        Layout.fillWidth: true
                        placeholderText: "YYYY-MM-DD HH:MM"
                    }
                }

                Item { Layout.fillHeight: true }

                RowLayout {
                    Layout.alignment: Qt.AlignRight
                    Button {
                        text: qsTr("Cancel")
                        onClicked: entriesDialog.adding = false
                    }
                    Button {
                        text: qsTr("Save")
                        enabled: addStart.text.length > 0 && addEnd.text.length > 0
                        onClicked: {
                            entries.add(addStart.text, addEnd.text)
                            entriesDialog.adding = false
                        }
                    }
                }
            }
        }
    }

    Timer {
        interval: 1000
        repeat: true
        // Only tick while a segment is actually counting (not while paused).
        running: timer.runningSince !== ""
        onTriggered: root.tick++
    }

    // Keep meta.timer_heartbeat fresh so even a hard kill (SIGKILL, power loss)
    // is recovered to within ~2 s. A graceful close is exact via root.onClosing.
    // Only runs while a segment is actually counting; one tiny UPSERT per tick.
    Timer {
        interval: 2000
        repeat: true
        running: timer.runningSince !== ""
        triggeredOnStart: true
        onTriggered: timer.heartbeat()
    }

    // App-level settings, opened from the toolbar gear. A full dialog rather
    // than a menu so there is room to grow as more settings arrive.
    Dialog {
        id: settingsDialog
        title: qsTr("Settings")
        modal: true
        anchors.centerIn: Overlay.overlay
        width: 500
        height: Math.min(640, root.height - 60)
        padding: 18
        standardButtons: Dialog.Close

        // Scrolls once there are enough setting groups to overflow a
        // reasonable dialog height, rather than a hand-tuned fixed height
        // that needs bumping every time a group is added.
        contentItem: ScrollView {
            id: settingsScroll
            clip: true
            contentWidth: availableWidth

            ColumnLayout {
                width: settingsDialog.availableWidth
                spacing: 18

            // ---- Tasks group ----
            Label {
                text: qsTr("Tasks")
                font.bold: true
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: 8
                spacing: 10
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2
                    Label { text: qsTr("Show finished tasks") }
                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Include completed tasks in the normal views.")
                        font.pointSize: 8
                        opacity: 0.6
                        wrapMode: Text.WordWrap
                    }
                }
                Switch {
                    checked: tasks.showDone
                    onToggled: tasks.showDone = checked
                }
            }

            // ---- Dates group ----
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Label {
                    text: qsTr("Dates")
                    font.bold: true
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    text: qsTr("How a deadline is shown and typed:")
                    wrapMode: Text.WordWrap
                }
                Flow {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    Layout.topMargin: 4
                    spacing: 6
                    Repeater {
                        model: [
                            { v: 0, label: qsTr("YYYY-MM-DD") },
                            { v: 1, label: qsTr("MM/DD/YYYY") },
                            { v: 2, label: qsTr("DD/MM/YYYY") },
                        ]
                        delegate: Button {
                            required property var modelData
                            text: modelData.label
                            checkable: true
                            checked: settings.dateFormat === modelData.v
                            onClicked: settings.dateFormat = modelData.v
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    Layout.topMargin: 4
                    font.pointSize: 8
                    opacity: 0.6
                    text: qsTr("e.g. “%1”").arg(root.fmtDate(root.isoPlusDays(0)))
                }
            }

            // ---- Deadlines group ----
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Label {
                    text: qsTr("Deadlines")
                    font.bold: true
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    text: qsTr("Show time left next to a deadline, worded as:")
                    wrapMode: Text.WordWrap
                }
                Flow {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    Layout.topMargin: 4
                    spacing: 6
                    Repeater {
                        model: [
                            { v: 0, label: qsTr("Off") },
                            { v: 1, label: qsTr("Auto") },
                            { v: 5, label: qsTr("Hours") },
                            { v: 2, label: qsTr("Days") },
                            { v: 3, label: qsTr("Weeks") },
                            { v: 4, label: qsTr("Months") },
                        ]
                        delegate: Button {
                            required property var modelData
                            text: modelData.label
                            checkable: true
                            checked: settings.deadlineCountdown === modelData.v
                            onClicked: settings.deadlineCountdown = modelData.v
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    Layout.topMargin: 4
                    font.pointSize: 8
                    opacity: 0.6
                    wrapMode: Text.WordWrap
                    // Live preview against a date ~5 weeks out. The mode is
                    // named in the binding so it re-runs on change.
                    text: {
                        settings.deadlineCountdown
                        settings.dateFormat
                        let d = new Date()
                        d.setDate(d.getDate() + 38)
                        let iso = Qt.formatDate(d, "yyyy-MM-dd")
                        let s = settings.countdownText(iso)
                        return s === "" ? qsTr("(hidden)")
                                        : qsTr("e.g. “%1  ·  %2”").arg(root.fmtDate(iso)).arg(s)
                    }
                }
            }

            // ---- Calendar group ----
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Label {
                    text: qsTr("Calendar")
                    font.bold: true
                }
                RowLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    spacing: 10
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Label { text: qsTr("Cross out past days") }
                        Label {
                            Layout.fillWidth: true
                            text: qsTr("Strike a line through days that have already passed (blue, or a faint red when the day has overdue tasks).")
                            font.pointSize: 8
                            opacity: 0.6
                            wrapMode: Text.WordWrap
                        }
                    }
                    Switch {
                        checked: settings.calendarCrossPast
                        onToggled: settings.calendarCrossPast = checked
                    }
                }
                RowLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    spacing: 10
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Label { text: qsTr("Hide days from other months") }
                        Label {
                            Layout.fillWidth: true
                            text: qsTr("Leave the leading and trailing days of the next/previous month blank instead of dimmed.")
                            font.pointSize: 8
                            opacity: 0.6
                            wrapMode: Text.WordWrap
                        }
                    }
                    Switch {
                        checked: settings.calendarHideOtherMonth
                        onToggled: settings.calendarHideOtherMonth = checked
                    }
                }
                RowLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    spacing: 10
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Label { text: qsTr("Show the task-count badge") }
                        Label {
                            Layout.fillWidth: true
                            text: qsTr("The “TC: N” tag on each day showing how many deadlines fall on it.")
                            font.pointSize: 8
                            opacity: 0.6
                            wrapMode: Text.WordWrap
                        }
                    }
                    Switch {
                        checked: settings.calendarShowTaskCount
                        onToggled: settings.calendarShowTaskCount = checked
                    }
                }
            }

            // ---- Quick creation group ----
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Label {
                    text: qsTr("Quick creation")
                    font.bold: true
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    text: qsTr("One level of task nesting in the pasted text is:")
                    wrapMode: Text.WordWrap
                }
                Flow {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    Layout.topMargin: 4
                    spacing: 6
                    Button {
                        text: qsTr("One space")
                        checkable: true
                        checked: !settings.quickCreateIndentTab
                        onClicked: settings.quickCreateIndentTab = false
                    }
                    Button {
                        text: qsTr("One tab")
                        checkable: true
                        checked: settings.quickCreateIndentTab
                        onClicked: settings.quickCreateIndentTab = true
                    }
                }
                RowLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 8
                    Layout.topMargin: 6
                    spacing: 10
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Label { text: qsTr("Show the “Initial time” field") }
                        Label {
                            Layout.fillWidth: true
                            text: qsTr("A one-off starting duration set by quick creation, shown on the task's info panel. Never counted in time totals or the graph.")
                            font.pointSize: 8
                            opacity: 0.6
                            wrapMode: Text.WordWrap
                        }
                    }
                    Switch {
                        checked: settings.showInitialTime
                        onToggled: settings.showInitialTime = checked
                    }
                }
            }

            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8

        // ---- Top bar: running timer (left) + settings gear (right) ----
        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            Frame {
                id: timerBar
                Layout.fillWidth: true
                visible: timer.runningTaskId !== ""

                RowLayout {
                    anchors.fill: parent
                    spacing: 10

                    // Red pulsing while counting; amber and still while paused.
                    Rectangle {
                        implicitWidth: 10
                        implicitHeight: 10
                        radius: 5
                        color: timer.paused ? "#e0a030" : "#e74c3c"
                        SequentialAnimation on opacity {
                            running: timerBar.visible && !timer.paused
                            loops: Animation.Infinite
                            alwaysRunToEnd: true
                            NumberAnimation { to: 0.3; duration: 700 }
                            NumberAnimation { to: 1.0; duration: 700 }
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        text: timer.runningTaskTitle
                        font.bold: true
                    }
                    Label {
                        visible: timer.paused
                        // A crash-recovered session reads as a pause too, but say why.
                        text: timer.recovered
                              ? qsTr("paused — app closed while running")
                              : qsTr("paused")
                        opacity: 0.7
                    }
                    Label {
                        text: (root.tick, root.fmtElapsed(timer.baseSeconds, timer.runningSince))
                        font.family: "monospace"
                    }
                    Button {
                        text: timer.paused ? qsTr("Resume") : qsTr("Pause")
                        onClicked: timer.paused ? timer.resume() : timer.pause()
                    }
                    Button {
                        text: qsTr("Stop")
                        onClicked: timer.stop()
                    }
                }
            }

            // Holds the gear to the right when no timer is running.
            Item {
                Layout.fillWidth: true
                visible: !timerBar.visible
            }

            ToolButton {
                id: settingsButton
                focusPolicy: Qt.NoFocus
                implicitWidth: 30
                implicitHeight: 30
                Layout.alignment: Qt.AlignVCenter
                ToolTip.text: qsTr("Settings")
                ToolTip.visible: hovered
                onClicked: settingsDialog.open()
                contentItem: Canvas {
                    property color ink: palette.buttonText
                    onInkChanged: requestPaint()
                    onPaint: {
                        let ctx = getContext("2d")
                        ctx.reset()
                        ctx.fillStyle = ink
                        ctx.strokeStyle = ink
                        ctx.lineWidth = 1.6
                        let cx = width / 2
                        let cy = height / 2
                        for (let i = 0; i < 8; i++) {
                            ctx.save()
                            ctx.translate(cx, cy)
                            ctx.rotate(i * Math.PI / 4)
                            ctx.fillRect(-1.4, -7, 2.8, 4)
                            ctx.restore()
                        }
                        ctx.beginPath()
                        ctx.arc(cx, cy, 4, 0, 2 * Math.PI)
                        ctx.stroke()
                    }
                }
            }
        }

        RowLayout {
            id: mainSplit
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

        // ---- Sidebar -----------------------------------------------------
        ColumnLayout {
            // Rigid width - only the divider drag changes it, so it stays put
            // across view/panel switches.
            Layout.preferredWidth: root.sidebarWidth
            Layout.minimumWidth: root.sidebarWidth
            Layout.maximumWidth: root.sidebarWidth
            Layout.fillHeight: true
            spacing: 4

            Label {
                text: qsTr("Views")
                font.bold: true
            }

            // Built-in views: every task, the ones with no project, the archive.
            Repeater {
                model: [
                    { key: "", label: qsTr("All tasks") },
                    { key: "unfiled", label: qsTr("Tasks w/o project") },
                    { key: "duetoday", label: qsTr("Due today") },
                    { key: "deadlined", label: qsTr("Deadlined") },
                    { key: "finished", label: qsTr("Finished") },
                ]
                delegate: ItemDelegate {
                    required property var modelData
                    Layout.fillWidth: true
                    highlighted: root.mainView === "tasks"
                                 && tasks.projectFilter === modelData.key
                    onClicked: {
                        tasks.projectFilter = modelData.key
                        root.mainView = "tasks"
                    }
                    contentItem: RowLayout {
                        Label {
                            Layout.fillWidth: true
                            text: modelData.label
                            elide: Text.ElideRight
                        }
                        Label {
                            // Comma-operator idiom: reads `dataVersion` purely
                            // to re-run this binding on every reload, so the
                            // count never needs its own cache to keep in sync.
                            text: (tasks.dataVersion, root.viewTaskCount(modelData.key))
                            opacity: 0.6
                        }
                    }
                }
            }

            MenuSeparator {
                Layout.fillWidth: true
            }

            Label {
                text: qsTr("Projects")
                font.bold: true
                Layout.topMargin: 2
            }

            ListView {
                id: projectList
                Layout.fillWidth: true
                // As tall as its content, capped; scrolls past the cap. Not
                // `fillHeight` - the trailing spacer takes the slack so the
                // "Others" section sits directly under the list, not at the
                // bottom of the sidebar.
                Layout.preferredHeight: Math.min(contentHeight, 320)
                clip: true
                model: projects

                delegate: ItemDelegate {
                    id: pdel

                    required property int index
                    required property string id
                    required property string name
                    property bool editing: false

                    width: projectList.width
                    highlighted: root.mainView === "tasks"
                                 && tasks.projectFilter === pdel.id
                    onClicked: if (!pdel.editing) {
                        tasks.projectFilter = pdel.id
                        root.mainView = "tasks"
                    }

                    contentItem: RowLayout {
                        Label {
                            visible: !pdel.editing
                            Layout.fillWidth: true
                            text: pdel.name
                            elide: Text.ElideRight
                        }
                        Label {
                            visible: !pdel.editing
                            text: (tasks.dataVersion, tasks.projectTaskCount(pdel.id))
                            opacity: 0.6
                        }
                        TextField {
                            id: pedit
                            visible: pdel.editing
                            Layout.fillWidth: true
                            text: pdel.name
                            onAccepted: {
                                projects.rename(pdel.index, text)
                                pdel.editing = false
                            }
                            onActiveFocusChanged: if (!activeFocus)
                                pdel.editing = false
                        }
                    }

                    TapHandler {
                        acceptedButtons: Qt.RightButton
                        onTapped: pmenu.popup()
                    }
                    Menu {
                        id: pmenu
                        MenuItem {
                            text: qsTr("Time graph")
                            onTriggered: {
                                graph.targetKind = 1
                                graph.targetId = pdel.id
                                graphDialog.subject = pdel.name
                                graphDialog.open()
                            }
                        }
                        MenuItem {
                            text: qsTr("Rename")
                            onTriggered: {
                                pdel.editing = true
                                pedit.forceActiveFocus()
                                pedit.selectAll()
                            }
                        }
                        MenuItem {
                            text: qsTr("Delete")
                            onTriggered: {
                                if (tasks.projectFilter === pdel.id)
                                    tasks.projectFilter = ""
                                projects.remove(pdel.index)
                                // Deleting a project unfiles its tasks
                                // (ON DELETE SET NULL, not cascade) - refresh
                                // so "Tasks w/o project" picks up the change.
                                tasks.refresh()
                            }
                        }
                    }
                }
            }

            // ---- Others (directly under the project list) --------------
            MenuSeparator {
                Layout.fillWidth: true
            }
            Label {
                text: qsTr("Others")
                font.bold: true
                Layout.topMargin: 2
            }
            ItemDelegate {
                Layout.fillWidth: true
                text: qsTr("Calendar")
                highlighted: root.mainView === "calendar"
                onClicked: root.mainView = "calendar"
            }
            ItemDelegate {
                Layout.fillWidth: true
                text: qsTr("Recent actions")
                highlighted: root.mainView === "actions"
                onClicked: root.mainView = "actions"
            }

            // ---- Project creation ------------------------------------
            MenuSeparator {
                Layout.fillWidth: true
            }
            Label {
                text: qsTr("Project creation")
                font.bold: true
                Layout.topMargin: 2
            }
            ItemDelegate {
                Layout.fillWidth: true
                text: qsTr("Quick creation")
                highlighted: root.mainView === "quickcreate"
                onClicked: root.mainView = "quickcreate"
            }

            // "Empty project" is the other way to create a project - not a
            // page (clicking the label does nothing, unlike "Quick
            // creation" above), just a caption for the field + button below
            // it. Boxed instead of separator-divided from "Quick creation"
            // so the two read as siblings under "Project creation" rather
            // than unrelated controls.
            Rectangle {
                Layout.fillWidth: true
                Layout.topMargin: 4
                radius: 4
                color: Qt.rgba(palette.mid.r, palette.mid.g, palette.mid.b, 0.18)
                border.color: palette.mid
                border.width: 1
                implicitHeight: emptyProjectCol.implicitHeight + 12

                ColumnLayout {
                    id: emptyProjectCol
                    anchors.fill: parent
                    anchors.margins: 6
                    spacing: 4

                    Label {
                        text: qsTr("Empty project")
                        font.pointSize: 8
                        opacity: 0.7
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        TextField {
                            id: newProject
                            Layout.fillWidth: true
                            placeholderText: qsTr("New project")
                            onAccepted: {
                                projects.add(text)
                                text = ""
                            }
                        }
                        Button {
                            text: "+"
                            enabled: newProject.text.trim().length > 0
                            onClicked: {
                                projects.add(newProject.text)
                                newProject.text = ""
                            }
                        }
                    }
                }
            }

            // Slack, so the sidebar's contents sit at the top.
            Item { Layout.fillHeight: true }
        }

        // ---- Divider: drag to resize the sidebar --------------------
        Rectangle {
            Layout.preferredWidth: 8
            Layout.fillHeight: true
            color: sidebarDrag.pressed
                   ? Qt.rgba(palette.highlight.r, palette.highlight.g,
                             palette.highlight.b, 0.5)
                   : sidebarDrag.containsMouse
                   ? Qt.rgba(palette.highlight.r, palette.highlight.g,
                             palette.highlight.b, 0.22)
                   : "transparent"
            ToolSeparator {
                anchors.centerIn: parent
                height: parent.height
            }
            MouseArea {
                id: sidebarDrag
                anchors.fill: parent
                anchors.leftMargin: -3
                anchors.rightMargin: -3
                hoverEnabled: true
                cursorShape: Qt.SplitHCursor
                // Offset of the cursor from the divider's left edge, captured
                // on press so the divider doesn't jump under the cursor.
                property real grabDx: 0
                onPressed: grabDx = mapToItem(mainSplit, mouseX, 0).x
                                    - root.sidebarWidth
                onPositionChanged: if (pressed)
                    root.sidebarWidth = Math.max(160, Math.min(440,
                        mapToItem(mainSplit, mouseX, 0).x - grabDx))
            }
        }

        // ---- Centre pane: task list or calendar ----------------------
        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: root.mainView === "calendar" ? 1
                          : root.mainView === "actions" ? 2
                          : root.mainView === "quickcreate" ? 3 : 0

        // ---- Tasks -----------------------------------------------------
        ColumnLayout {
            id: taskPane
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8

            // Read-only, derived views: no add field, no scope total. New
            // tasks would have no project / no deadline and vanish on reload.
            // These three also prefix each row's title with its project
            // (task #2 in the todo batch) since, unlike the other views, a
            // row here can sit next to one from a different project.
            readonly property bool derivedView:
                tasks.projectFilter === "finished"
                || tasks.projectFilter === "deadlined"
                || tasks.projectFilter === "duetoday"

            // Time recorded across everything in the current view (a project,
            // "All tasks", or the project-less ones).
            Label {
                Layout.fillWidth: true
                visible: !taskPane.derivedView && tasks.viewTotalText !== ""
                text: qsTr("Time invested: %1").arg(tasks.viewTotalText)
                font.pointSize: 9
                opacity: 0.7
            }

            // ---- Action row: Select toggle + (add task | bulk actions) ----
            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                Button {
                    text: tasks.selectionMode ? qsTr("Done") : qsTr("Select")
                    checkable: true
                    checked: tasks.selectionMode
                    onToggled: tasks.selectionMode = checked
                }

                // --- add a task (hidden while picking / on a derived view) ---
                TextField {
                    id: input
                    Layout.fillWidth: true
                    visible: !tasks.selectionMode && !taskPane.derivedView
                    placeholderText: qsTr("New task, then Enter")
                    onAccepted: {
                        tasks.add(text)
                        text = ""
                    }
                }
                Button {
                    visible: !tasks.selectionMode && !taskPane.derivedView
                    text: qsTr("Add")
                    enabled: input.text.trim().length > 0
                    onClicked: {
                        tasks.add(input.text)
                        input.text = ""
                    }
                }

                // --- bulk actions (while picking) ---
                Label {
                    visible: tasks.selectionMode
                    text: qsTr("%1 selected").arg(tasks.selectedCount)
                    font.pointSize: 9
                    opacity: 0.7
                }
                Item { Layout.fillWidth: true; visible: tasks.selectionMode }
                Button {
                    visible: tasks.selectionMode
                    text: qsTr("All")
                    onClicked: tasks.selectAll()
                }
                Button {
                    visible: tasks.selectionMode && tasks.projectFilter === "finished"
                    enabled: tasks.selectedCount > 0
                    text: qsTr("Revert")
                    onClicked: tasks.revertSelected()
                }
                Button {
                    visible: tasks.selectionMode
                    enabled: tasks.selectedCount > 0
                    text: qsTr("Delete")
                    onClicked: tasks.deleteSelected()
                }
            }

            ListView {
                id: list

                // Which task's info panel is open. Held here, not on the
                // delegate, so it survives a model reset (e.g. after editing a
                // deadline) - the panel stays open until the user closes it.
                property string openTaskId: ""

                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: tasks
                spacing: 2

                delegate: ItemDelegate {
                    id: rowItem

                    required property int index
                    required property string id
                    required property string title
                    required property bool done
                    required property int depth
                    required property bool hasChildren
                    required property bool expanded
                    required property string deadline
                    required property bool overdue
                    required property string notes
                    required property bool selected
                    required property string branchMask
                    required property string projectName

                    readonly property bool sessionTask: rowItem.id === timer.runningTaskId
                    readonly property bool running: rowItem.sessionTask && !timer.paused
                    readonly property bool paused: rowItem.sessionTask && timer.paused

                    // Click opens an info panel; renaming is deliberate (double-
                    // click the title, the ⋯ menu, or click the title while the
                    // panel is already open).
                    readonly property bool infoOpen: rowItem.id !== ""
                                                     && rowItem.id === list.openTaskId
                    property bool editing: false

                    function startEdit() {
                        list.openTaskId = rowItem.id
                        rowItem.editing = true
                        titleEdit.forceActiveFocus()
                        titleEdit.selectAll()
                    }
                    function endEdit(commit) {
                        if (commit && titleEdit.text.trim() !== ""
                                && titleEdit.text !== rowItem.title)
                            tasks.rename(rowItem.index, titleEdit.text)
                        rowItem.editing = false
                    }

                    width: list.width
                    leftPadding: 8
                    opacity: done ? 0.5 : 1.0
                    // Don't take keyboard focus: when a button inside the info
                    // panel is destroyed on a model reset, focus would otherwise
                    // jump to a neighbouring row and draw a stray highlight.
                    focusPolicy: Qt.NoFocus

                    onClicked: {
                        if (tasks.selectionMode) {
                            tasks.toggleSelected(rowItem.index)
                        } else if (rowItem.editing) {
                            // A click anywhere on the row commits the rename and
                            // closes the panel in one go.
                            rowItem.endEdit(true)
                            list.openTaskId = ""
                        } else {
                            list.openTaskId = rowItem.infoOpen ? "" : rowItem.id
                        }
                    }
                    onInfoOpenChanged: if (rowItem.infoOpen)
                        infoPanel.timeText = tasks.timeInvestedText(rowItem.index)

                    contentItem: ColumnLayout {
                        spacing: 4
                        // Above `guides` (default z) so the title/info panel
                        // paint over a continuing tree-guide line rather than
                        // the other way round - see the comment on `guides`.
                        z: 1

                    RowLayout {
                        id: mainRow
                        Layout.fillWidth: true
                        spacing: 4

                        // Reserves the indent + expander width. The guide
                        // lines and the collapse/expand box that fill this
                        // space are drawn by `guides` (below), a full-height
                        // overlay outside this RowLayout - so a line that
                        // continues past the row runs unbroken through the
                        // delegate padding and an open info panel.
                        Item {
                            Layout.preferredWidth: (rowItem.depth + 1) * 18
                            Layout.minimumWidth: (rowItem.depth + 1) * 18
                        }

                        CheckBox {
                            padding: 0
                            checked: rowItem.done
                            onToggled: tasks.setDone(rowItem.index, checked)
                        }

                        // Title: a plain label until you deliberately edit it.
                        // The click target is only as wide as the text, so the
                        // rest of the row still toggles the info panel.
                        Item {
                            Layout.fillWidth: true
                            implicitHeight: Math.max(titleLabel.implicitHeight,
                                                     titleEdit.implicitHeight)

                            Label {
                                id: titleLabel
                                anchors.left: parent.left
                                anchors.verticalCenter: parent.verticalCenter
                                width: Math.min(parent.width, implicitWidth)
                                visible: !rowItem.editing
                                text: (taskPane.derivedView
                                       ? (rowItem.projectName !== ""
                                          ? rowItem.projectName + ": " : qsTr("W/o project: "))
                                       : "") + rowItem.title
                                font.strikeout: rowItem.done
                                elide: Text.ElideRight

                                MouseArea {
                                    anchors.fill: parent
                                    enabled: !rowItem.editing && !tasks.selectionMode
                                    onClicked: {
                                        if (rowItem.infoOpen)
                                            rowItem.startEdit()
                                        else
                                            list.openTaskId = rowItem.id
                                    }
                                    onDoubleClicked: rowItem.startEdit()
                                }
                            }
                            TextField {
                                id: titleEdit
                                anchors.fill: parent
                                visible: rowItem.editing
                                text: rowItem.title
                                padding: 4
                                background: Rectangle {
                                    color: "transparent"
                                    border.color: palette.highlight
                                    border.width: 1
                                    radius: 2
                                }
                                onAccepted: rowItem.endEdit(true)
                                onActiveFocusChanged: if (!activeFocus && rowItem.editing)
                                    rowItem.endEdit(true)
                            }
                        }

                        Rectangle {
                            visible: rowItem.sessionTask
                            implicitWidth: 9
                            implicitHeight: 9
                            radius: 4.5
                            color: rowItem.paused ? "#e0a030" : "#e74c3c"
                        }

                        Label {
                            visible: rowItem.deadline !== ""
                            // `settings.deadlineCountdown` / `dateFormat` are
                            // referenced so the binding re-runs when either
                            // preference changes.
                            readonly property string countdown:
                                (settings.deadlineCountdown, rowItem.deadline !== "")
                                    ? settings.countdownText(rowItem.deadline) : ""
                            text: (settings.dateFormat, countdown !== ""
                                  ? root.fmtDate(rowItem.deadline) + "  ·  " + countdown
                                  : root.fmtDate(rowItem.deadline))
                            font.pointSize: 9
                            color: rowItem.overdue ? "#e74c3c" : palette.text
                            opacity: rowItem.overdue ? 1 : 0.7
                        }

                        // Primary row action. Idle -> start; running -> pause;
                        // paused -> resume. (Stop lives in the top bar and the
                        // ⋯ menu.) The mark is drawn (Canvas) - the system font
                        // has no media glyphs.
                        Button {
                            padding: 4
                            leftPadding: 8
                            rightPadding: 8
                            onClicked: {
                                if (rowItem.running)
                                    timer.pause()
                                else if (rowItem.paused)
                                    timer.resume()
                                else
                                    timer.start(rowItem.id)
                            }
                            ToolTip.text: rowItem.running ? qsTr("Pause the timer")
                                        : rowItem.paused ? qsTr("Resume the timer")
                                        : qsTr("Start timing this task")
                            ToolTip.visible: hovered

                            contentItem: Row {
                                spacing: 6

                                Canvas {
                                    id: timerGlyph
                                    width: 10
                                    height: 10
                                    anchors.verticalCenter: parent.verticalCenter
                                    // Redraw when either input changes.
                                    property bool showPause: rowItem.running
                                    property color mark: rowItem.running ? "#e0a030"
                                                       : rowItem.paused ? "#e0a030"
                                                       : palette.buttonText
                                    onShowPauseChanged: requestPaint()
                                    onMarkChanged: requestPaint()
                                    onPaint: {
                                        var ctx = getContext("2d")
                                        ctx.reset()
                                        ctx.fillStyle = mark
                                        if (showPause) {
                                            ctx.fillRect(1, 0, 3, 10)
                                            ctx.fillRect(6, 0, 3, 10)
                                        } else {
                                            ctx.beginPath()
                                            ctx.moveTo(1, 0)
                                            ctx.lineTo(10, 5)
                                            ctx.lineTo(1, 10)
                                            ctx.closePath()
                                            ctx.fill()
                                        }
                                    }
                                }

                                Label {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: rowItem.running ? qsTr("Pause")
                                        : rowItem.paused ? qsTr("Resume")
                                        : qsTr("Timer")
                                    font.pointSize: 9
                                }
                            }
                        }

                        Button {
                            id: moreButton
                            implicitWidth: 30
                            padding: 4
                            opacity: (rowItem.hovered || down) ? 1 : 0.5
                            ToolTip.text: qsTr("More actions")
                            ToolTip.visible: hovered
                            onClicked: rowMenu.popup()

                            // The system font has no "⋯" glyph, so draw the dots.
                            contentItem: Item {
                                implicitWidth: 14
                                implicitHeight: 14
                                Row {
                                    anchors.centerIn: parent
                                    spacing: 2
                                    Repeater {
                                        model: 3
                                        delegate: Rectangle {
                                            width: 3
                                            height: 3
                                            radius: 1.5
                                            color: palette.buttonText
                                        }
                                    }
                                }
                            }
                        }

                        // Multi-select tick - at the row's end, clear of the
                        // "mark done" box on the left.
                        CheckBox {
                            visible: tasks.selectionMode
                            padding: 0
                            Layout.leftMargin: 4
                            checked: rowItem.selected
                            onToggled: tasks.toggleSelected(rowItem.index)
                        }
                    }

                    // ---- Info panel: description, deadline, time invested ----
                    Frame {
                        id: infoPanel
                        Layout.fillWidth: true
                        visible: rowItem.infoOpen
                        padding: 8

                        // Recomputed whenever the panel becomes visible - after
                        // a model reset the delegate is rebuilt from scratch.
                        property string timeText: ""
                        property string initialTimeText: ""
                        onVisibleChanged: if (visible) {
                            timeText = tasks.timeInvestedText(rowItem.index)
                            initialTimeText = tasks.initialTimeText(rowItem.index)
                        }
                        Component.onCompleted: if (visible) {
                            timeText = tasks.timeInvestedText(rowItem.index)
                            initialTimeText = tasks.initialTimeText(rowItem.index)
                        }

                        contentItem: ColumnLayout {
                            spacing: 6

                            RowLayout {
                                Layout.fillWidth: true
                                Label {
                                    text: qsTr("Time invested:")
                                    font.pointSize: 9
                                    opacity: 0.7
                                }
                                Label {
                                    text: infoPanel.timeText
                                    font.pointSize: 9
                                }
                                Item { Layout.fillWidth: true }
                            }

                            RowLayout {
                                Layout.fillWidth: true
                                visible: settings.showInitialTime && infoPanel.initialTimeText !== ""
                                Label {
                                    text: qsTr("Initial time:")
                                    font.pointSize: 9
                                    opacity: 0.7
                                }
                                Label {
                                    text: infoPanel.initialTimeText
                                    font.pointSize: 9
                                }
                                Item { Layout.fillWidth: true }
                            }

                            RowLayout {
                                Layout.fillWidth: true
                                Label {
                                    text: qsTr("Deadline:")
                                    font.pointSize: 9
                                    opacity: 0.7
                                }
                                Label {
                                    readonly property string countdown:
                                        (settings.deadlineCountdown, rowItem.deadline !== "")
                                            ? settings.countdownText(rowItem.deadline) : ""
                                    text: (settings.dateFormat, rowItem.deadline === ""
                                          ? qsTr("none")
                                          : (countdown !== ""
                                             ? root.fmtDate(rowItem.deadline) + "  ·  " + countdown
                                             : root.fmtDate(rowItem.deadline)))
                                    font.pointSize: 9
                                    color: rowItem.overdue ? "#e74c3c" : palette.text
                                }
                                Item { Layout.fillWidth: true }
                                Button {
                                    text: qsTr("Set deadline")
                                    font.pointSize: 8
                                    padding: 3
                                    focusPolicy: Qt.NoFocus
                                    onClicked: datePopup.open()
                                }
                                Button {
                                    text: qsTr("Clear")
                                    font.pointSize: 8
                                    padding: 3
                                    focusPolicy: Qt.NoFocus
                                    enabled: rowItem.deadline !== ""
                                    onClicked: tasks.setDeadline(rowItem.index, "")
                                }
                            }

                            Label {
                                text: qsTr("Description")
                                font.pointSize: 9
                                opacity: 0.7
                            }
                            TextArea {
                                id: notesArea
                                Layout.fillWidth: true
                                Layout.minimumHeight: 52
                                text: rowItem.notes
                                wrapMode: TextArea.Wrap
                                placeholderText: qsTr("Add a description…")
                                background: Rectangle {
                                    color: "transparent"
                                    border.color: palette.mid
                                    border.width: 1
                                    radius: 2
                                }
                                onActiveFocusChanged: if (!activeFocus && text !== rowItem.notes)
                                    tasks.setNotes(rowItem.index, text)
                            }
                        }
                    }
                    }

                    // Nesting guides + collapse/expand box, one coupled
                    // component on an 18px grid. Columns 0..depth-1 are
                    // ancestor guide lines (`branchMask` marks the ones that
                    // continue past this row); column `depth` carries the box,
                    // wired to the parent guide above and the first child's
                    // guide below. Drawn here, as a direct child of the
                    // delegate spanning its full height, so a continuing line
                    // is unbroken from one row to the next - through the
                    // delegate padding and an open info panel - and every
                    // segment is the same single ink (no doubled-up, darker
                    // overlap in the gap between rows).
                    Item {
                        id: guides
                        // Default z (same as the delegate's own background).
                        // `contentItem` above is explicitly `z: 1` so the
                        // title/info panel still paint over a continuing
                        // guide line - do NOT give this a negative z instead:
                        // that sinks it below the background too (both
                        // default to z: 0, so anything negative here goes
                        // behind it), hiding the guides/expand box entirely.

                        anchors.left: parent.left
                        anchors.leftMargin: rowItem.leftPadding
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        width: (rowItem.depth + 1) * 18

                        readonly property color ink: palette.text
                        readonly property real fade: 0.35
                        // Vertical centre of the task row, in delegate coords:
                        // the content sits below `topPadding`, `mainRow` is its
                        // first child.
                        readonly property real mid: rowItem.topPadding + mainRow.height / 2
                        // A line that continues also covers the gap to the next
                        // row, so the guide never breaks between rows.
                        readonly property real span: height + list.spacing

                        // Ancestor guide columns.
                        Repeater {
                            model: rowItem.depth
                            delegate: Item {
                                required property int index
                                readonly property bool connector:
                                    index === rowItem.depth - 1
                                readonly property bool carries:
                                    rowItem.branchMask.charAt(index) === "1"
                                x: index * 18
                                width: 18
                                height: guides.height

                                // Vertical guide. The connector column always
                                // drops in from above to meet this row; it
                                // carries on down only if a sibling follows.
                                Rectangle {
                                    x: 9
                                    width: 1
                                    color: guides.ink
                                    opacity: guides.fade
                                    y: 0
                                    height: parent.connector
                                            ? (parent.carries ? guides.span : guides.mid)
                                            : (parent.carries ? guides.span : 0)
                                }
                                // Elbow into this row.
                                Rectangle {
                                    visible: parent.connector
                                    x: 9
                                    y: guides.mid
                                    width: 11
                                    height: 1
                                    color: guides.ink
                                    opacity: guides.fade
                                }
                            }
                        }

                        // Collapse/expand column.
                        Item {
                            visible: rowItem.hasChildren
                            x: rowItem.depth * 18
                            width: 18
                            height: guides.height

                            // Stub from the box down to the first child's guide.
                            Rectangle {
                                visible: rowItem.expanded
                                x: 9
                                width: 1
                                y: guides.mid + 7
                                height: guides.span - (guides.mid + 7)
                                color: guides.ink
                                opacity: guides.fade
                            }
                            // The box: a "-" bar always, plus a "|" bar when
                            // collapsed (making a "+"). Same ink as the guides
                            // - only the hover tint sets it apart.
                            Rectangle {
                                x: 2.5
                                y: guides.mid - 6.5
                                width: 13
                                height: 13
                                radius: 2
                                color: "transparent"
                                border.width: 1
                                border.color: disc.containsMouse
                                              ? palette.highlight : guides.ink
                                opacity: disc.containsMouse ? 1.0 : guides.fade

                                Rectangle {
                                    x: 3
                                    y: 6
                                    width: 7
                                    height: 1
                                    color: parent.border.color
                                }
                                Rectangle {
                                    visible: !rowItem.expanded
                                    x: 6
                                    y: 3
                                    width: 1
                                    height: 7
                                    color: parent.border.color
                                }
                            }
                            MouseArea {
                                id: disc
                                anchors.fill: parent
                                hoverEnabled: true
                                enabled: !tasks.selectionMode
                                onClicked: tasks.toggleExpanded(rowItem.index)
                            }
                        }
                    }

                    TapHandler {
                        acceptedButtons: Qt.RightButton
                        onTapped: rowMenu.popup()
                    }

                    // ---- Drag to re-parent ----
                    property bool dropHover: false

                    // Press and drag the row onto another task to make it (and
                    // its subtree) a child of that task.
                    DragHandler {
                        id: rowDrag
                        target: dragProxy
                        enabled: !tasks.selectionMode && !rowItem.editing
                        onActiveChanged: {
                            if (active) {
                                dragProxy.sourceRow = rowItem.index
                                dragProxy.label = rowItem.title
                                dragProxy.x = centroid.scenePosition.x - dragProxy.Drag.hotSpot.x
                                dragProxy.y = centroid.scenePosition.y - dragProxy.Drag.hotSpot.y
                                dragProxy.Drag.active = true
                            } else {
                                dragProxy.Drag.drop()
                                dragProxy.Drag.active = false
                            }
                        }
                    }

                    // Drop target: the whole row.
                    DropArea {
                        anchors.fill: parent
                        onEntered: (drag) => {
                            rowItem.dropHover =
                                tasks.canReparent(drag.source.sourceRow, rowItem.id)
                        }
                        onExited: rowItem.dropHover = false
                        onDropped: (drop) => {
                            if (tasks.canReparent(drop.source.sourceRow, rowItem.id))
                                tasks.reparent(drop.source.sourceRow, rowItem.id)
                            rowItem.dropHover = false
                        }
                    }

                    // Valid drop target outline.
                    Rectangle {
                        anchors.fill: parent
                        visible: rowItem.dropHover
                        color: "transparent"
                        border.color: palette.highlight
                        border.width: 2
                        radius: 3
                    }

                    Menu {
                        id: rowMenu

                        MenuItem {
                            text: qsTr("Rename")
                            onTriggered: rowItem.startEdit()
                        }
                        MenuItem {
                            text: qsTr("Add subtask")
                            onTriggered: tasks.addChild(rowItem.index, qsTr("New subtask"))
                        }
                        MenuItem {
                            text: rowItem.running ? qsTr("Pause timer")
                                : rowItem.paused ? qsTr("Resume timer")
                                : qsTr("Start timer")
                            onTriggered: {
                                if (rowItem.running)
                                    timer.pause()
                                else if (rowItem.paused)
                                    timer.resume()
                                else
                                    timer.start(rowItem.id)
                            }
                        }
                        MenuItem {
                            text: qsTr("Stop timer")
                            enabled: rowItem.sessionTask
                            onTriggered: timer.stop()
                        }
                        MenuItem {
                            text: qsTr("Time entries…")
                            onTriggered: {
                                entries.taskId = rowItem.id
                                entriesDialog.taskTitle = rowItem.title
                                entriesDialog.open()
                            }
                        }
                        MenuItem {
                            text: qsTr("Time graph…")
                            onTriggered: {
                                graph.targetKind = 2
                                graph.targetId = rowItem.id
                                graphDialog.subject = rowItem.title
                                graphDialog.open()
                            }
                        }

                        Menu {
                            title: qsTr("Deadline")
                            MenuItem {
                                text: qsTr("Today")
                                onTriggered: tasks.setDeadline(rowItem.index, root.isoPlusDays(0))
                            }
                            MenuItem {
                                text: qsTr("Tomorrow")
                                onTriggered: tasks.setDeadline(rowItem.index, root.isoPlusDays(1))
                            }
                            MenuItem {
                                text: qsTr("Pick a date…")
                                onTriggered: datePopup.open()
                            }
                            MenuSeparator {}
                            MenuItem {
                                text: qsTr("Clear")
                                enabled: rowItem.deadline !== ""
                                onTriggered: tasks.setDeadline(rowItem.index, "")
                            }
                        }

                        Menu {
                            id: moveMenu
                            title: qsTr("Move to project")
                            MenuItem {
                                text: qsTr("Unfiled")
                                onTriggered: tasks.moveToProject(rowItem.index, "")
                            }
                            MenuSeparator {}
                            Instantiator {
                                model: projects
                                delegate: MenuItem {
                                    required property string id
                                    required property string name
                                    text: name
                                    onTriggered: tasks.moveToProject(rowItem.index, id)
                                }
                                onObjectAdded: (i, obj) => moveMenu.insertItem(i + 2, obj)
                                onObjectRemoved: (i, obj) => moveMenu.removeItem(obj)
                            }
                        }

                        MenuSeparator {}
                        MenuItem {
                            // Un-finish: the task keeps its project, so it
                            // reappears there.
                            text: qsTr("Revert to not done")
                            visible: rowItem.done
                            height: visible ? implicitHeight : 0
                            onTriggered: tasks.setDone(rowItem.index, false)
                        }
                        MenuItem {
                            text: qsTr("Delete")
                            onTriggered: tasks.remove(rowItem.index)
                        }
                    }

                    Popup {
                        id: datePopup
                        modal: true
                        anchors.centerIn: Overlay.overlay
                        padding: 12

                        contentItem: ColumnLayout {
                            spacing: 8
                            Label {
                                text: root.dateInputLabel()
                            }
                            TextField {
                                id: dateInput
                                Layout.fillWidth: true
                                inputMask: root.dateInputMask()
                                text: root.fmtDate(rowItem.deadline)
                            }
                            RowLayout {
                                Layout.alignment: Qt.AlignRight
                                Button {
                                    text: qsTr("Cancel")
                                    onClicked: datePopup.close()
                                }
                                Button {
                                    text: qsTr("Set")
                                    onClicked: {
                                        let iso = root.parseDate(dateInput.text)
                                        if (iso !== "")
                                            tasks.setDeadline(rowItem.index, iso)
                                        datePopup.close()
                                    }
                                }
                            }
                        }
                    }
                }

                Label {
                    anchors.centerIn: parent
                    visible: list.count === 0
                    text: qsTr("No tasks here yet")
                    opacity: 0.5
                }
            }
        }

        // ---- Calendar page -------------------------------------------
        ColumnLayout {
            id: calendarPane
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8

            // "grid" (month calendar + day list) or "agenda" (a flat,
            // date-grouped list of every upcoming deadline).
            property string mode: "grid"
            // Month on show, as year + 0-based month.
            property int viewYear: new Date().getFullYear()
            property int viewMonth: new Date().getMonth()
            // Selected day, ISO "yyyy-MM-dd".
            property string selectedIso: Qt.formatDate(new Date(), "yyyy-MM-dd")
            // Day-detail pane width - fixed unless the divider is dragged.
            property real detailWidth: 240

            // Deadline items, re-parsed whenever the model bumps `revision`.
            readonly property var items:
                (calendar.revision, JSON.parse(calendar.itemsJson()))
            // { "yyyy-MM-dd": [item, ...] }.
            readonly property var byDay: {
                let m = ({})
                for (let it of calendarPane.items)
                    (m[it.date] = m[it.date] || []).push(it)
                return m
            }
            // [{ date, label, items }] in date order, for the agenda.
            readonly property var agenda: {
                let out = []
                let seen = ({})
                for (let it of calendarPane.items) {
                    if (!seen[it.date]) {
                        seen[it.date] = { date: it.date,
                                          label: calendarPane.longDate(it.date),
                                          items: [] }
                        out.push(seen[it.date])
                    }
                    seen[it.date].items.push(it)
                }
                return out
            }

            function isoOf(y, m, d) {
                return Qt.formatDate(new Date(y, m, d), "yyyy-MM-dd")
            }
            function longDate(iso) {
                let p = iso.split("-")
                let dt = new Date(parseInt(p[0]), parseInt(p[1]) - 1, parseInt(p[2]))
                return Qt.formatDate(dt, "ddd d MMM yyyy")
            }
            function stepMonth(delta) {
                let m = calendarPane.viewMonth + delta
                calendarPane.viewYear += Math.floor(m / 12)
                calendarPane.viewMonth = ((m % 12) + 12) % 12
            }
            function goToday() {
                let now = new Date()
                calendarPane.viewYear = now.getFullYear()
                calendarPane.viewMonth = now.getMonth()
                calendarPane.selectedIso = Qt.formatDate(now, "yyyy-MM-dd")
            }
            // Monday on or before the 1st of the shown month.
            function gridStart() {
                let first = new Date(calendarPane.viewYear, calendarPane.viewMonth, 1)
                let isoDow = (first.getDay() + 6) % 7
                return new Date(calendarPane.viewYear, calendarPane.viewMonth,
                                1 - isoDow)
            }

            readonly property string todayIso:
                Qt.formatDate(new Date(), "yyyy-MM-dd")
            // Weeks to draw: a full 6 rows normally; only the rows that hold a
            // day of this month when adjacent-month days are hidden.
            readonly property int weeksShown: {
                if (!settings.calendarHideOtherMonth)
                    return 6
                let first = new Date(calendarPane.viewYear, calendarPane.viewMonth, 1)
                let pad = (first.getDay() + 6) % 7
                let days = new Date(calendarPane.viewYear,
                                    calendarPane.viewMonth + 1, 0).getDate()
                return Math.ceil((pad + days) / 7)
            }

            // ---- Header: mode switch + month stepper -----------------
            RowLayout {
                Layout.fillWidth: true
                spacing: 6

                Repeater {
                    model: [
                        { key: "grid", label: qsTr("Calendar") },
                        { key: "agenda", label: qsTr("Agenda") },
                    ]
                    delegate: Button {
                        required property var modelData
                        text: modelData.label
                        checkable: true
                        checked: calendarPane.mode === modelData.key
                        onClicked: calendarPane.mode = modelData.key
                    }
                }

                Item { Layout.fillWidth: true }

                ToolButton {
                    visible: calendarPane.mode === "grid"
                    text: "‹"
                    onClicked: calendarPane.stepMonth(-1)
                }
                Label {
                    visible: calendarPane.mode === "grid"
                    Layout.minimumWidth: 140
                    horizontalAlignment: Text.AlignHCenter
                    font.bold: true
                    text: Qt.locale().standaloneMonthName(calendarPane.viewMonth)
                          + " " + calendarPane.viewYear
                }
                ToolButton {
                    visible: calendarPane.mode === "grid"
                    text: "›"
                    onClicked: calendarPane.stepMonth(1)
                }
                Button {
                    visible: calendarPane.mode === "grid"
                    text: qsTr("Today")
                    onClicked: calendarPane.goToday()
                }
            }

            // ---- Grid mode: month calendar + selected-day list -------
            RowLayout {
                id: calSplit
                visible: calendarPane.mode === "grid"
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 0

                // Month grid, held to the top so the cells stay compact.
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 0

                GridLayout {
                    columns: 7
                    rowSpacing: 4
                    columnSpacing: 4
                    Layout.fillWidth: true

                    Repeater {
                        model: [qsTr("Mon"), qsTr("Tue"), qsTr("Wed"),
                                qsTr("Thu"), qsTr("Fri"), qsTr("Sat"), qsTr("Sun")]
                        delegate: Label {
                            required property string modelData
                            Layout.fillWidth: true
                            // Equal columns: ignore the text's own width so a
                            // long weekday name can't widen its column.
                            Layout.preferredWidth: 0
                            Layout.minimumWidth: 0
                            horizontalAlignment: Text.AlignHCenter
                            text: modelData
                            font.pointSize: 8
                            opacity: 0.6
                        }
                    }

                    Repeater {
                        model: calendarPane.weeksShown * 7
                        delegate: ItemDelegate {
                            id: cell
                            required property int index

                            readonly property date cellDate: {
                                let s = calendarPane.gridStart()
                                return new Date(s.getFullYear(), s.getMonth(),
                                                s.getDate() + cell.index)
                            }
                            readonly property string iso:
                                Qt.formatDate(cell.cellDate, "yyyy-MM-dd")
                            readonly property bool inMonth:
                                cell.cellDate.getMonth() === calendarPane.viewMonth
                            readonly property bool isToday:
                                cell.iso === calendarPane.todayIso
                            // Hidden slot: an adjacent-month day when the user
                            // has chosen not to see them. Kept in the grid so
                            // the columns still line up, but blank and inert.
                            readonly property bool blank:
                                settings.calendarHideOtherMonth && !cell.inMonth
                            readonly property bool past:
                                cell.iso < calendarPane.todayIso
                            readonly property var dayItems:
                                cell.blank ? [] : (calendarPane.byDay[cell.iso] || [])
                            readonly property bool hasOverdue:
                                cell.dayItems.some(function (it) { return it.overdue })

                            Layout.fillWidth: true
                            // Every cell the same width: the GridLayout must
                            // not grow a column to fit a long task title
                            // (that skewed the cells and threw off the X).
                            Layout.preferredWidth: 0
                            Layout.minimumWidth: 0
                            Layout.preferredHeight: 92
                            padding: 4
                            enabled: !cell.blank
                            highlighted: !cell.blank
                                         && cell.iso === calendarPane.selectedIso
                            onClicked: if (!cell.blank)
                                calendarPane.selectedIso = cell.iso

                            contentItem: ColumnLayout {
                                visible: !cell.blank
                                spacing: 2
                                RowLayout {
                                    Layout.fillWidth: true
                                    Label {
                                        text: cell.cellDate.getDate()
                                        font.pointSize: 9
                                        font.bold: cell.isToday
                                        opacity: cell.inMonth ? 1 : 0.35
                                    }
                                    Item { Layout.fillWidth: true }
                                    // Task-count badge: bordered pill, "TC:"
                                    // prefix, so it can't be read as a date.
                                    Rectangle {
                                        visible: settings.calendarShowTaskCount
                                                 && cell.dayItems.length > 0
                                        radius: 3
                                        color: Qt.rgba(palette.highlight.r,
                                                       palette.highlight.g,
                                                       palette.highlight.b, 0.15)
                                        border.width: 1
                                        border.color: Qt.rgba(palette.highlight.r,
                                                              palette.highlight.g,
                                                              palette.highlight.b, 0.55)
                                        implicitWidth: tcLabel.implicitWidth + 8
                                        implicitHeight: tcLabel.implicitHeight + 3
                                        Label {
                                            id: tcLabel
                                            anchors.centerIn: parent
                                            text: qsTr("TC: %1").arg(cell.dayItems.length)
                                            font.pointSize: 7
                                            font.bold: true
                                        }
                                    }
                                }
                                // Up to three deadline chips, then "+N". Each
                                // gets a small "mark done" box on the left -
                                // everything here is by definition not done,
                                // so it's a one-way tick, not a toggle.
                                Repeater {
                                    model: Math.min(3, cell.dayItems.length)
                                    delegate: RowLayout {
                                        id: chip
                                        required property int index
                                        readonly property var item:
                                            cell.dayItems[chip.index]
                                        Layout.fillWidth: true
                                        spacing: 3

                                        Rectangle {
                                            implicitWidth: 9
                                            implicitHeight: 9
                                            radius: 2
                                            color: "transparent"
                                            border.width: 1
                                            border.color: chip.item.overdue
                                                ? "#e74c3c" : palette.text
                                            opacity: 0.6
                                            MouseArea {
                                                anchors.fill: parent
                                                anchors.margins: -3
                                                onClicked: {
                                                    calendar.setDone(chip.item.id, true)
                                                    tasks.refresh()
                                                }
                                            }
                                        }
                                        Label {
                                            Layout.fillWidth: true
                                            text: chip.item.title
                                            elide: Text.ElideRight
                                            font.pointSize: 8
                                            color: chip.item.overdue
                                                   ? "#e74c3c" : palette.text
                                            opacity: cell.inMonth ? 0.9 : 0.4
                                        }
                                    }
                                }
                                Label {
                                    visible: cell.dayItems.length > 3
                                    text: qsTr("+%1 more").arg(cell.dayItems.length - 3)
                                    font.pointSize: 8
                                    opacity: 0.5
                                }
                                Item { Layout.fillHeight: true }
                            }

                            // Today's ring: independent of the selection fill,
                            // so today stays easy to spot while browsing.
                            Rectangle {
                                visible: cell.isToday
                                anchors.fill: parent
                                anchors.margins: 1
                                radius: 3
                                color: "transparent"
                                border.width: 2
                                border.color: palette.highlight
                                z: 1
                            }

                            // Strike-through for past days: an X from corner
                            // to corner. Blue normally; a faint red when the
                            // day holds overdue tasks so their titles stay
                            // readable underneath. `clip` keeps it inside the
                            // cell even if a line is a sub-pixel long.
                            Item {
                                anchors.fill: parent
                                clip: true
                                visible: settings.calendarCrossPast
                                         && cell.past && !cell.blank
                                z: 2

                                readonly property color mark: cell.hasOverdue
                                    ? Qt.rgba(0.9, 0.25, 0.2, 0.35)
                                    : palette.highlight
                                readonly property real diag:
                                    Math.hypot(width, height)
                                // Angle of the cell's own diagonal, so a line
                                // of length `diag` spans exactly corner to
                                // corner regardless of the cell's aspect.
                                readonly property real ang:
                                    Math.atan2(height, width) * 180 / Math.PI

                                Rectangle {
                                    anchors.centerIn: parent
                                    width: parent.diag
                                    height: 2
                                    color: parent.mark
                                    rotation: parent.ang
                                    antialiasing: true
                                }
                                Rectangle {
                                    anchors.centerIn: parent
                                    width: parent.diag
                                    height: 2
                                    color: parent.mark
                                    rotation: -parent.ang
                                    antialiasing: true
                                }
                            }
                        }
                    }
                }
                    Item { Layout.fillHeight: true }
                }

                // ---- Divider: drag to resize the day-detail pane ----
                Rectangle {
                    Layout.preferredWidth: 8
                    Layout.fillHeight: true
                    color: detailDrag.pressed
                           ? Qt.rgba(palette.highlight.r, palette.highlight.g,
                                     palette.highlight.b, 0.5)
                           : detailDrag.containsMouse
                           ? Qt.rgba(palette.highlight.r, palette.highlight.g,
                                     palette.highlight.b, 0.22)
                           : "transparent"
                    ToolSeparator {
                        anchors.centerIn: parent
                        height: parent.height
                    }
                    MouseArea {
                        id: detailDrag
                        anchors.fill: parent
                        anchors.leftMargin: -3
                        anchors.rightMargin: -3
                        hoverEnabled: true
                        cursorShape: Qt.SplitHCursor
                        property real grabDx: 0
                        onPressed: grabDx = calSplit.width
                                   - mapToItem(calSplit, mouseX, 0).x
                                   - calendarPane.detailWidth
                        onPositionChanged: if (pressed)
                            calendarPane.detailWidth = Math.max(180,
                                Math.min(520, calSplit.width
                                    - mapToItem(calSplit, mouseX, 0).x - grabDx))
                    }
                }

                // Selected-day detail.
                ColumnLayout {
                    Layout.preferredWidth: calendarPane.detailWidth
                    Layout.minimumWidth: calendarPane.detailWidth
                    Layout.maximumWidth: calendarPane.detailWidth
                    Layout.fillHeight: true
                    spacing: 6

                    Label {
                        text: calendarPane.longDate(calendarPane.selectedIso)
                        font.bold: true
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                    Repeater {
                        model: calendarPane.byDay[calendarPane.selectedIso] || []
                        delegate: ItemDelegate {
                            required property var modelData
                            Layout.fillWidth: true
                            contentItem: RowLayout {
                                spacing: 6
                                // Everything listed here is not done by
                                // definition, so this is a one-way tick.
                                CheckBox {
                                    checked: false
                                    onToggled: {
                                        calendar.setDone(modelData.id, true)
                                        tasks.refresh()
                                    }
                                }
                                ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 0
                                Label {
                                    text: modelData.title
                                    elide: Text.ElideRight
                                    Layout.fillWidth: true
                                    color: modelData.overdue ? "#e74c3c" : palette.text
                                }
                                Label {
                                    visible: modelData.project !== ""
                                    text: modelData.project
                                    font.pointSize: 8
                                    opacity: 0.6
                                }
                                }
                            }
                        }
                    }
                    Label {
                        visible: (calendarPane.byDay[calendarPane.selectedIso] || []).length === 0
                        text: qsTr("Nothing due")
                        opacity: 0.5
                    }
                    Item { Layout.fillHeight: true }
                }
            }

            // ---- Agenda mode: date-grouped list ---------------------
            ScrollView {
                visible: calendarPane.mode === "agenda"
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true

                ColumnLayout {
                    width: parent.width
                    spacing: 4

                    Repeater {
                        model: calendarPane.agenda
                        delegate: ColumnLayout {
                            required property var modelData
                            Layout.fillWidth: true
                            spacing: 2

                            Label {
                                text: modelData.label
                                font.bold: true
                                Layout.topMargin: 6
                            }
                            Repeater {
                                model: modelData.items
                                delegate: RowLayout {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    Layout.leftMargin: 8
                                    Label {
                                        text: modelData.title
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                        color: modelData.overdue
                                               ? "#e74c3c" : palette.text
                                    }
                                    Label {
                                        visible: modelData.project !== ""
                                        text: modelData.project
                                        font.pointSize: 8
                                        opacity: 0.6
                                    }
                                }
                            }
                        }
                    }
                    Label {
                        visible: calendarPane.agenda.length === 0
                        text: qsTr("No upcoming deadlines")
                        opacity: 0.5
                        Layout.topMargin: 6
                    }
                    Item { Layout.fillHeight: true }
                }
            }
        }

        // ---- Recent actions --------------------------------------------
        ColumnLayout {
            id: actionsPane
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8

            // Re-parsed whenever the model bumps `revision`.
            readonly property var items:
                (actionsLog.revision, JSON.parse(actionsLog.itemsJson()))

            function fmt(iso) {
                return Qt.formatDateTime(new Date(iso), "yyyy-MM-dd hh:mm")
            }

            Label {
                text: qsTr("Recent actions")
                font.bold: true
                font.pointSize: 12
                Layout.margins: 8
                Layout.bottomMargin: 0
            }
            Label {
                visible: actionsPane.items.length === 0
                text: qsTr("Nothing to show yet")
                opacity: 0.5
                Layout.leftMargin: 8
            }
            ListView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.margins: 8
                clip: true
                model: actionsPane.items
                spacing: 4

                delegate: ItemDelegate {
                    id: adel
                    required property var modelData

                    width: ListView.view.width
                    hoverEnabled: false

                    contentItem: RowLayout {
                        spacing: 8
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            Label {
                                text: adel.modelData.projectLabel
                                font.bold: true
                                font.pointSize: 8
                                opacity: 0.75
                            }
                            Label {
                                text: adel.modelData.summary
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                            }
                            Label {
                                text: actionsPane.fmt(adel.modelData.createdAt)
                                font.pointSize: 8
                                opacity: 0.6
                            }
                        }
                        Button {
                            text: qsTr("Undo")
                            onClicked: {
                                actionsLog.reset(adel.modelData.id)
                                tasks.refresh()
                                calendar.reload()
                            }
                        }
                        Button {
                            text: qsTr("Undo from here")
                            onClicked: {
                                actionsLog.resetFrom(adel.modelData.id)
                                tasks.refresh()
                                calendar.reload()
                            }
                        }
                    }
                }
            }
        }

        // ---- Quick creation ---------------------------------------------
        ColumnLayout {
            id: quickCreatePane
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8

            // Parsed preview, re-computed (debounced) as the user types.
            property var previewLines: []
            readonly property bool hasError:
                previewLines.length === 0 || previewLines.some(l => !l.ok)
            property string createError: ""

            function reparse() {
                previewLines = JSON.parse(quickCreate.parse(
                    bodyArea.text, settings.quickCreateIndentTab, settings.dateFormat))
            }

            Timer {
                id: reparseTimer
                interval: 150
                repeat: false
                onTriggered: quickCreatePane.reparse()
            }

            RowLayout {
                Layout.fillWidth: true
                Label {
                    text: qsTr("Quick creation")
                    font.bold: true
                    font.pointSize: 12
                }
                Item { Layout.fillWidth: true }
                Button {
                    text: qsTr("Format help")
                    onClicked: formatHelpDialog.open()
                }
            }

            TextField {
                id: quickProjectName
                Layout.fillWidth: true
                placeholderText: qsTr("Project name")
            }

            // Always-visible format reminder, so the user isn't expected to
            // memorize the field order or open Format help every time.
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0
                Label {
                    Layout.fillWidth: true
                    font.family: "monospace"
                    font.pointSize: 9
                    elide: Text.ElideRight
                    text: qsTr("Full task syntax: title | time invested | deadline | description")
                }
                Label {
                    Layout.fillWidth: true
                    font.family: "monospace"
                    font.pointSize: 9
                    opacity: 0.6
                    elide: Text.ElideRight
                    text: qsTr("Full example: Finish painting the bike | 01:10 | 2025-09-30 | Use matte black")
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 8

                ScrollView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    // Equal, explicit preferredWidth on both sides of this
                    // row - otherwise the TextArea's own content-driven
                    // implicitWidth (it doesn't wrap) skews the fillWidth
                    // split and starves the preview panel down to ~0.
                    Layout.preferredWidth: 1
                    clip: true
                    TextArea {
                        id: bodyArea
                        placeholderText: qsTr("One task per line, e.g. just \"title\" - see Format help for more")
                        wrapMode: TextArea.NoWrap
                        onTextChanged: reparseTimer.restart()
                        Component.onCompleted: quickCreatePane.reparse()
                    }
                }

                Frame {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Layout.preferredWidth: 1

                    contentItem: ListView {
                        clip: true
                        model: quickCreatePane.previewLines
                        spacing: 2
                        ScrollBar.vertical: ScrollBar {}
                        delegate: Item {
                                id: previewRow
                                required property var modelData
                                width: ListView.view.width
                                height: previewLabel.implicitHeight + 4

                                RowLayout {
                                    anchors.fill: parent
                                    spacing: 4

                                    // Simple per-depth indent guides - not the
                                    // full elbow/tee precision of the task
                                    // list, just enough to see nesting while
                                    // typing.
                                    Repeater {
                                        model: previewRow.modelData.ok ? previewRow.modelData.depth : 0
                                        delegate: Item {
                                            width: 14
                                            height: previewRow.height
                                            Rectangle {
                                                anchors.horizontalCenter: parent.horizontalCenter
                                                width: 1
                                                height: parent.height
                                                color: palette.text
                                                opacity: 0.3
                                            }
                                        }
                                    }
                                    Label {
                                        id: previewLabel
                                        Layout.fillWidth: true
                                        wrapMode: Text.Wrap
                                        color: previewRow.modelData.ok ? palette.text : "#e74c3c"
                                        text: previewRow.modelData.ok
                                              ? previewRow.modelData.title
                                              : qsTr("Line %1: %2")
                                                    .arg(previewRow.modelData.lineNo)
                                                    .arg(previewRow.modelData.error)
                                    }
                                }
                            }
                        }
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Label {
                    Layout.fillWidth: true
                    visible: text !== ""
                    color: "#e74c3c"
                    wrapMode: Text.WordWrap
                    text: quickCreatePane.createError

                }
                Button {
                    text: qsTr("Create")
                    enabled: quickProjectName.text.trim() !== "" && !quickCreatePane.hasError
                    onClicked: {
                        let res = JSON.parse(quickCreate.create(
                            quickProjectName.text, bodyArea.text,
                            settings.quickCreateIndentTab, settings.dateFormat))
                        if (res.ok) {
                            quickCreatePane.createError = ""
                            projects.refresh()
                            tasks.refresh()
                            tasks.projectFilter = res.projectId
                            bodyArea.text = ""
                            quickProjectName.text = ""
                            root.mainView = "tasks"
                        } else {
                            quickCreatePane.createError = res.error
                        }
                    }
                }
            }
        }
        }
        }
    }

    Dialog {
        id: formatHelpDialog
        title: qsTr("Quick creation format")
        modal: true
        anchors.centerIn: Overlay.overlay
        width: Math.min(560, root.width - 60)
        height: Math.min(520, root.height - 60)
        standardButtons: Dialog.Close

        contentItem: ScrollView {
            clip: true
            ColumnLayout {
                width: formatHelpDialog.availableWidth
                spacing: 8

                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    text: qsTr(
                        "One line per task: <b>title|timeinvested|deadline|description</b>. " +
                        "Only the title is required, and a “|” is only needed up to the " +
                        "last field you're giving - a bare title needs no “|” at all. To " +
                        "skip a field before a later one you do want, leave it empty " +
                        "between two “|”, or write “na”.")
                }
                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    text: qsTr(
                        "Nesting is by indentation: more indent than the line above makes it " +
                        "a child (exactly one %1 deeper); same indent is a sibling; less " +
                        "indent must land exactly on an earlier line's indent. Change the " +
                        "indent unit (space or tab) in Settings.")
                        .arg(settings.quickCreateIndentTab ? qsTr("tab") : qsTr("space"))
                }
                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    text: qsTr(
                        "Time invested is a one-off starting duration, written as " +
                        "hours:minutes (e.g. “33:22”, or “2:30” for two and a half " +
                        "hours) - the “2h30m” style also still works if you prefer it. " +
                        "It's shown on the task's info panel as “Initial time”, never " +
                        "counted toward tracked time totals or the graph. " +
                        "Deadlines use whatever date format is set in Settings.")
                }
                Label {
                    Layout.fillWidth: true
                    font.bold: true
                    text: qsTr("Example:")
                }
                Label {
                    Layout.fillWidth: true
                    font.family: "monospace"
                    wrapMode: Text.NoWrap
                    text: "math homework||2025-09-30|boring maths\n" +
                          " exercise 1|1:30\n" +
                          " exercise 2\n" +
                          "  exercise 2.1\n" +
                          "  exercise 2.2\n" +
                          "  exercise 2.3\n" +
                          " exercise 3\n" +
                          "english project|||essays n stuff\n" +
                          " essay on the reading book\n" +
                          " essay on the class topic\n" +
                          "  draft\n" +
                          "  write out on laptop\n" +
                          "  print"
                }
            }
        }
    }
}
