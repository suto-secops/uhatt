//! uhatt - task and time-tracking desktop app.

mod actions_log;
mod calendar;
mod db;
mod domain;
mod entries_model;
mod graph_model;
mod projects_model;
mod settings;
mod tasks_model;
mod timer_controller;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/dev/suto/uhatt/qml/Main.qml"));
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
