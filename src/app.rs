//! Root application object bridged to QML.
//!
//! For the scaffold this only exposes the app version and a greeting invokable,
//! enough to prove the Rust <-> QML boundary works. Task/project/timer state
//! lands here (or in dedicated bridge objects) in later milestones.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        /// An alias to the QString type.
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, version)]
        #[namespace = "uhatt"]
        type App = super::AppRust;

        /// Return a human-readable greeting for the given name.
        #[qinvokable]
        fn greeting(&self, name: &QString) -> QString;
    }
}

use cxx_qt_lib::QString;

/// Backing Rust struct for the `App` QObject.
pub struct AppRust {
    version: QString,
}

impl Default for AppRust {
    fn default() -> Self {
        Self {
            version: QString::from(env!("CARGO_PKG_VERSION")),
        }
    }
}

impl qobject::App {
    fn greeting(&self, name: &QString) -> QString {
        QString::from(&format!("Hello, {name}. uhatt is wired up."))
    }
}
