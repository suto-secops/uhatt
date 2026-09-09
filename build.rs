use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(QmlModule::new("dev.suto.uhatt").qml_file("qml/Main.qml"))
        .files([
            "src/tasks_model.rs",
            "src/projects_model.rs",
            "src/timer_controller.rs",
            "src/entries_model.rs",
            "src/graph_model.rs",
        ])
        .build();
}
