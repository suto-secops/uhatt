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

    // Seconds between `sinceIso` and now as HH:MM:SS.
    function fmtDuration(sinceIso) {
        if (!sinceIso)
            return "00:00:00"
        let secs = Math.max(0, Math.floor((Date.now() - Date.parse(sinceIso)) / 1000))
        let parts = [Math.floor(secs / 3600), Math.floor(secs % 3600 / 60), secs % 60]
        return parts.map(n => n < 10 ? "0" + n : "" + n).join(":")
    }

    // Bumped once a second while a timer runs, to re-evaluate elapsed-time bindings.
    property int tick: 0

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
        width: 640
        height: 460
        standardButtons: Dialog.Close

        contentItem: ColumnLayout {
            spacing: 10

            RowLayout {
                Layout.fillWidth: true
                Label {
                    text: qsTr("Total: %1").arg(graph.totalText)
                    font.bold: true
                }
                Item {
                    Layout.fillWidth: true
                }
                Repeater {
                    model: [qsTr("Day"), qsTr("Week"), qsTr("Month"), qsTr("Year")]
                    delegate: Button {
                        required property int index
                        required property string modelData
                        text: modelData
                        checkable: true
                        checked: graph.bucket === index
                        onClicked: graph.bucket = index
                    }
                }
            }

            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true

                Label {
                    anchors.centerIn: parent
                    visible: barRepeater.count === 0
                    text: qsTr("No time recorded yet")
                    opacity: 0.5
                }

                RowLayout {
                    anchors.fill: parent
                    spacing: 3
                    visible: barRepeater.count > 0

                    Repeater {
                        id: barRepeater
                        model: graph

                        delegate: ColumnLayout {
                            id: bar

                            required property string label
                            required property real seconds
                            required property string hoursText

                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            spacing: 2

                            Label {
                                Layout.alignment: Qt.AlignHCenter
                                text: bar.hoursText
                                font.pointSize: 8
                            }
                            Item {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                Rectangle {
                                    anchors.bottom: parent.bottom
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    width: Math.max(6, parent.width * 0.6)
                                    height: parent.height * (graph.maxSeconds > 0 ? bar.seconds / graph.maxSeconds : 0)
                                    radius: 2
                                    color: palette.highlight

                                    HoverHandler {
                                        id: barHover
                                    }
                                    ToolTip.text: bar.label + " · " + bar.hoursText
                                    ToolTip.visible: barHover.hovered
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignHCenter
                                Layout.maximumWidth: 72
                                text: bar.label
                                font.pointSize: 7
                                elide: Text.ElideRight
                            }
                        }
                    }
                }
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
        running: timer.runningTaskId !== ""
        onTriggered: root.tick++
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8

        // ---- Running-timer bar ----------------------------------------
        Frame {
            id: timerBar
            Layout.fillWidth: true
            visible: timer.runningTaskId !== ""

            RowLayout {
                anchors.fill: parent
                spacing: 10

                // Pulsing dot - stands in for a clock glyph the font lacks.
                Rectangle {
                    implicitWidth: 10
                    implicitHeight: 10
                    radius: 5
                    color: "#e74c3c"
                    SequentialAnimation on opacity {
                        running: timerBar.visible
                        loops: Animation.Infinite
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
                    text: (root.tick, root.fmtDuration(timer.runningSince))
                    font.family: "monospace"
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

            Label {
                text: qsTr("Projects")
                font.bold: true
            }

            Repeater {
                model: [
                    { key: "", label: qsTr("All tasks") },
                    { key: "unfiled", label: qsTr("Unfiled") },
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

                    readonly property bool running: rowItem.id === timer.runningTaskId

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
                            visible: rowItem.running
                            implicitWidth: 9
                            implicitHeight: 9
                            radius: 4.5
                            color: "#e74c3c"
                        }

                        Label {
                            visible: rowItem.deadline !== ""
                            text: rowItem.deadline
                            font.pointSize: 9
                            color: rowItem.overdue ? "#e74c3c" : palette.text
                            opacity: rowItem.overdue ? 1 : 0.7
                        }

                        Button {
                            implicitWidth: 62
                            padding: 4
                            opacity: (rowItem.hovered || rowItem.running) ? 1 : 0
                            text: rowItem.running ? qsTr("Stop") : qsTr("Start")
                            onClicked: timer.toggle(rowItem.id)
                        }

                        Button {
                            id: moreButton
                            implicitWidth: 30
                            padding: 4
                            opacity: (rowItem.hovered || down) ? 1 : 0.35
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
                            text: rowItem.running ? qsTr("Stop timer") : qsTr("Start timer")
                            onTriggered: timer.toggle(rowItem.id)
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
