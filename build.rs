use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(QmlModule::new("dev.suto.uhatt").qml_file("qml/Main.qml"))
        .files([
            "src/tasks_model.rs",
            "src/projects_model.rs",
            "src/timer_controller.rs",
            "src/entries_model.rs",
            "src/graph_model.rs",
            "src/settings.rs",
            "src/calendar.rs",
            "src/actions_log.rs",
            "src/quick_create.rs",
        ])
        // App/window icon, rasterized from assets/uhatt-logo.svg (kept
        // alongside it as the source). Set from C++ (window_icon.cpp) since
        // QML's Window has no "icon" property in this build, and
        // cxx-qt-lib has no QGuiApplication::setWindowIcon/QIcon binding.
        .qrc_resources(["assets/uhatt-logo.png"])
        .cpp_file("src/window_icon.cpp")
        .build();
}
