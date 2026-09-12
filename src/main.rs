//! uhatt - task and time-tracking desktop app.

mod actions_log;
mod calendar;
mod db;
mod domain;
mod entries_model;
mod graph_model;
mod projects_model;
mod quick_create;
mod settings;
mod tasks_model;
mod timer_controller;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

// QML's Window has no "icon" property in this build, and cxx-qt-lib has no
// QGuiApplication::setWindowIcon/QIcon binding - set from plain C++ instead
// (src/window_icon.cpp). `resourcePath` is a Qt resource path (leading ":",
// not the "qrc:" URL form QML uses).
extern "C" {
    fn uhatt_set_window_icon(resource_path: *const std::os::raw::c_char);
}

fn main() {
    let mut app = QGuiApplication::new();

    let icon_path = c":/qt/qml/dev/suto/uhatt/assets/uhatt-logo.png";
    unsafe { uhatt_set_window_icon(icon_path.as_ptr()) };

    let mut engine = QQmlApplicationEngine::new();

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/dev/suto/uhatt/qml/Main.qml"));
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
