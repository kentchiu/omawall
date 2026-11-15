import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// Mounted once per shell. The bar draws a widget on every monitor, so the
// periodic wallpaper change lives here to fire exactly once per interval.
Item {
  id: root

  // Injected by omarchy-shell.
  property var shell: null

  readonly property string home: Quickshell.env("HOME") || ""
  readonly property string stateHome: Quickshell.env("XDG_STATE_HOME") || (home + "/.local/state")
  readonly property string commandPath: home + "/.local/bin/omawall"
  property bool timerEnabled: false
  property int timerMinutes: 60

  FileView {
    path: root.stateHome + "/omawall/settings.json"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: {
      var timer = Model.timerSettings(text())
      root.timerEnabled = timer.enabled
      root.timerMinutes = timer.minutes
    }
    onLoadFailed: root.timerEnabled = false
  }

  Timer {
    interval: root.timerMinutes * 60000
    repeat: true
    running: root.timerEnabled
    onTriggered: if (!nextProc.running) nextProc.running = true
  }

  // Failures are recorded by the CLI in status.json; the widget shows them.
  Process {
    id: nextProc
    command: [root.commandPath, "next", "--trigger", "timer"]
  }
}
