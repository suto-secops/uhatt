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
                    text: qsTr("Total: %1").arg(entries.totalText)
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

    // App-level settings, opened from the toolbar gear.
    Menu {
        id: settingsMenu
        width: 200
        MenuItem {
            text: qsTr("Show finished tasks")
            checkable: true
            checked: tasks.showDone
            onToggled: tasks.showDone = checked
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8

        // ---- Toolbar -------------------------------------------------
        RowLayout {
            Layout.fillWidth: true
            Item { Layout.fillWidth: true }
            ToolButton {
                id: settingsButton
                focusPolicy: Qt.NoFocus
                implicitWidth: 30
                implicitHeight: 30
                ToolTip.text: qsTr("Settings")
                ToolTip.visible: hovered
                onClicked: settingsMenu.popup(settingsButton,
                                              settingsButton.width - settingsMenu.width,
                                              settingsButton.height)
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

        // ---- Running-timer bar ----------------------------------------
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

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 12

        // ---- Sidebar -----------------------------------------------------
        ColumnLayout {
            Layout.preferredWidth: 190
            Layout.fillHeight: true
            spacing: 4

            // Built-in views: every task, the ones with no project, the archive.
            Repeater {
                model: [
                    { key: "", label: qsTr("All tasks") },
                    { key: "unfiled", label: qsTr("Tasks w/o project") },
                    { key: "finished", label: qsTr("Finished") },
                ]
                delegate: ItemDelegate {
                    required property var modelData
                    Layout.fillWidth: true
                    text: modelData.label
                    highlighted: tasks.projectFilter === modelData.key
                    onClicked: tasks.projectFilter = modelData.key
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
                Layout.fillHeight: true
                clip: true
                model: projects

                delegate: ItemDelegate {
                    id: pdel

                    required property int index
                    required property string id
                    required property string name
                    property bool editing: false

                    width: projectList.width
                    highlighted: tasks.projectFilter === pdel.id
                    onClicked: if (!pdel.editing)
                        tasks.projectFilter = pdel.id

                    contentItem: RowLayout {
                        Label {
                            visible: !pdel.editing
                            Layout.fillWidth: true
                            text: pdel.name
                            elide: Text.ElideRight
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
                            }
                        }
                    }
                }
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

        ToolSeparator {
            Layout.fillHeight: true
        }

        // ---- Tasks -----------------------------------------------------
        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8

            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                // No adding tasks while looking at the finished list.
                visible: tasks.projectFilter !== "finished"

                TextField {
                    id: input
                    Layout.fillWidth: true
                    placeholderText: qsTr("New task, then Enter")
                    onAccepted: {
                        tasks.add(text)
                        text = ""
                    }
                }

                Button {
                    text: qsTr("Add")
                    enabled: input.text.trim().length > 0
                    onClicked: {
                        tasks.add(input.text)
                        input.text = ""
                    }
                }
            }

            ListView {
                id: list

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

                    readonly property bool sessionTask: rowItem.id === timer.runningTaskId
                    readonly property bool running: rowItem.sessionTask && !timer.paused
                    readonly property bool paused: rowItem.sessionTask && timer.paused

                    width: list.width
                    leftPadding: 8 + depth * 18
                    opacity: done ? 0.5 : 1.0

                    contentItem: RowLayout {
                        spacing: 4

                        // Expand/collapse control. A plain Label + TapHandler rather than
                        // a Button: Button styles add unpredictable padding that clipped
                        // the single-glyph label to nothing on the Basic style. ASCII
                        // "[+]" / "[-]" because the system font has no box-drawing glyphs.
                        Label {
                            Layout.preferredWidth: 26
                            horizontalAlignment: Text.AlignHCenter
                            text: rowItem.hasChildren ? (rowItem.expanded ? "[-]" : "[+]") : ""
                            color: disclosureHover.hovered ? palette.highlight : palette.text
                            font.pointSize: 11

                            HoverHandler {
                                id: disclosureHover
                            }
                            TapHandler {
                                enabled: rowItem.hasChildren
                                onTapped: tasks.toggleExpanded(rowItem.index)
                            }
                        }

                        CheckBox {
                            padding: 0
                            checked: rowItem.done
                            onToggled: tasks.setDone(rowItem.index, checked)
                        }

                        TextField {
                            Layout.fillWidth: true
                            text: rowItem.title
                            padding: 4
                            font.strikeout: rowItem.done
                            background: Rectangle {
                                color: "transparent"
                            }
                            onEditingFinished: {
                                if (text !== rowItem.title)
                                    tasks.rename(rowItem.index, text)
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
                            text: rowItem.deadline
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
                    }

                    TapHandler {
                        acceptedButtons: Qt.RightButton
                        onTapped: rowMenu.popup()
                    }

                    Menu {
                        id: rowMenu

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
                                text: qsTr("Deadline (YYYY-MM-DD)")
                            }
                            TextField {
                                id: dateInput
                                Layout.fillWidth: true
                                inputMask: "9999-99-99"
                                text: rowItem.deadline
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
                                        tasks.setDeadline(rowItem.index, dateInput.text)
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
        }
    }
}
