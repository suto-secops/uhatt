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

    TaskListModel {
        id: tasks
    }
    ProjectListModel {
        id: projects
    }
    TimerController {
        id: timer
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

                Label {
                    text: "⏱"
                    font.pointSize: 12
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

                    width: list.width
                    leftPadding: 8 + depth * 20
                    opacity: done ? 0.55 : 1.0

                    contentItem: RowLayout {
                        spacing: 6

                        Item {
                            implicitWidth: 18
                            implicitHeight: 18
                            ToolButton {
                                anchors.fill: parent
                                visible: rowItem.hasChildren
                                padding: 0
                                text: rowItem.expanded ? "▾" : "▸"
                                onClicked: tasks.toggleExpanded(rowItem.index)
                            }
                        }

                        CheckBox {
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

                        ToolButton {
                            readonly property bool running: rowItem.id === timer.runningTaskId
                            text: running ? "⏹" : "▶"
                            ToolTip.text: running ? qsTr("Stop timer") : qsTr("Start timer")
                            ToolTip.visible: hovered
                            onClicked: timer.toggle(rowItem.id)
                        }

                        Label {
                            visible: rowItem.deadline !== ""
                            text: rowItem.deadline
                            font.pointSize: 9
                            color: rowItem.overdue ? "#c0392b" : palette.mid
                        }

                        ToolButton {
                            text: "🗓"
                            ToolTip.text: rowItem.deadline === "" ? qsTr("Set deadline") : qsTr("Change deadline")
                            ToolTip.visible: hovered
                            onClicked: deadlineMenu.popup()

                            Menu {
                                id: deadlineMenu
                                MenuItem {
                                    text: qsTr("Today")
                                    onTriggered: tasks.setDeadline(rowItem.index, root.isoPlusDays(0))
                                }
                                MenuItem {
                                    text: qsTr("Tomorrow")
                                    onTriggered: tasks.setDeadline(rowItem.index, root.isoPlusDays(1))
                                }
                                MenuItem {
                                    text: qsTr("Next week")
                                    onTriggered: tasks.setDeadline(rowItem.index, root.isoPlusDays(7))
                                }
                                MenuItem {
                                    text: qsTr("Next month")
                                    onTriggered: tasks.setDeadline(rowItem.index, root.isoPlusDays(30))
                                }
                                MenuItem {
                                    text: qsTr("Pick date…")
                                    onTriggered: datePopup.open()
                                }
                                MenuSeparator {}
                                MenuItem {
                                    text: qsTr("Clear")
                                    enabled: rowItem.deadline !== ""
                                    onTriggered: tasks.setDeadline(rowItem.index, "")
                                }
                            }
                        }

                        ToolButton {
                            text: "+"
                            ToolTip.text: qsTr("Add subtask")
                            ToolTip.visible: hovered
                            onClicked: tasks.addChild(rowItem.index, qsTr("New subtask"))
                        }

                        ToolButton {
                            text: "✕"
                            ToolTip.text: qsTr("Delete")
                            ToolTip.visible: hovered
                            onClicked: tasks.remove(rowItem.index)
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

                    TapHandler {
                        acceptedButtons: Qt.RightButton
                        onTapped: taskMenu.popup()
                    }
                    Menu {
                        id: taskMenu
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
