import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// URI must match the QmlModule in build.rs
import dev.suto.uhatt

ApplicationWindow {
    id: root

    width: 720
    height: 520
    visible: true
    title: qsTr("uhatt")

    App { id: backend }

    ColumnLayout {
        anchors.centerIn: parent
        spacing: 16

        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("uhatt")
            font.pixelSize: 32
            font.bold: true
        }

        Label {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("version %1").arg(backend.version)
            opacity: 0.7
        }

        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            spacing: 8

            TextField {
                id: nameField
                placeholderText: qsTr("your name")
                text: qsTr("world")
            }

            Button {
                text: qsTr("Greet")
                onClicked: greetingLabel.text = backend.greeting(nameField.text)
            }
        }

        Label {
            id: greetingLabel
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("(scaffold - milestone 1)")
        }
    }
}
