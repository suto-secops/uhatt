import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// URI must match the QmlModule in build.rs
import dev.suto.uhatt

ApplicationWindow {
    id: root

    width: 720
    height: 560
    visible: true
    title: qsTr("uhatt")

    TaskListModel {
        id: tasks
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
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
                required property string title
                required property bool done
                required property int depth

                width: ListView.view ? ListView.view.width : 0
                leftPadding: 8 + depth * 20
                opacity: done ? 0.55 : 1.0

                contentItem: RowLayout {
                    spacing: 8

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
                        text: "✕"
                        ToolTip.text: qsTr("Delete")
                        ToolTip.visible: hovered
                        onClicked: tasks.remove(rowItem.index)
                    }
                }
            }

            Label {
                anchors.centerIn: parent
                visible: list.count === 0
                text: qsTr("No tasks yet - add one above")
                opacity: 0.5
            }
        }
    }
}
