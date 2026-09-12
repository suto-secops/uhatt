// QML's Window/ApplicationWindow has no "icon" property in this Qt build
// (confirmed at runtime: "Cannot assign to non-existent property \"icon\"") -
// cxx-qt-lib 0.10 also doesn't expose QGuiApplication::setWindowIcon or QIcon
// at all, so this one call is plain C++, invoked from main.rs right after
// QGuiApplication is constructed.

#include <QGuiApplication>
#include <QIcon>
#include <QString>

extern "C" void uhatt_set_window_icon(const char *resourcePath) {
  // QCoreApplication::instance() returns the base-class pointer even when
  // the running instance is a QGuiApplication - safe to downcast since
  // main.rs always constructs one before calling this.
  if (auto *app =
          static_cast<QGuiApplication *>(QCoreApplication::instance())) {
    app->setWindowIcon(QIcon(QString::fromUtf8(resourcePath)));
  }
}
