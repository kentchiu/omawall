import QtQuick
import QtQuick.Controls as QQC
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import qs.Commons
import qs.Ui
import "Model.js" as Model

Panel {
  id: root
  moduleName: "kent.omawall"
  ipcTarget: "kent.omawall"

  readonly property string home: Quickshell.env("HOME") || ""
  readonly property string stateHome: Quickshell.env("XDG_STATE_HOME") || (home + "/.local/state")
  readonly property string thumbsDir: (Quickshell.env("XDG_CACHE_HOME") || (home + "/.cache")) + "/omawall/thumbs"
  readonly property string commandPath: home + "/.local/bin/omawall"
  readonly property color fg: Color.popups.text
  readonly property color dim: Qt.darker(fg, 1.45)

  property string pluginVersion: ""
  property var status: null
  property string cliState: "unknown"   // unknown | ok | missing | broken
  property string actionError: ""
  property bool statusPending: false
  property var favoriteIds: ({})

  property bool browseOpen: false
  property string browseSource: ""
  property int browseLastPage: 0
  property int browseTotal: 0
  property int browseLoaded: 0
  property string browseError: ""
  // Pages of the current source already in the grid, by page number.
  property var loadedPages: ({})
  // The card the keyboard acts on.
  property int selectedIndex: 0
  readonly property bool browseBusy: browseProc.running
  // Repeaters rebuild every delegate when handed a new array; these change only when their content does.
  property var sources: []
  property var tabList: []

  readonly property var favInfo: status && status.favorites ? status.favorites : ({ count: 0, selected: false })
  readonly property var userSettings: status && status.settings ? status.settings : ({})
  readonly property var current: status && status.status ? status.status.current : null
  readonly property bool currentIsFav: current !== null && root.favoriteIds[current.id] === true
  readonly property var bannerInfo: Model.banner(cliState, status, pluginVersion)
  readonly property bool hasLastError: !!(status && status.status && status.status.lastError)
  readonly property bool timerEnabled: !!(userSettings.timer && userSettings.timer.enabled)
  readonly property int timerMinutes: Model.clampMinutes(userSettings.timer ? userSettings.timer.minutes : 60)
  readonly property bool busy: actionProc.running
  readonly property var syncInfo: Model.syncView(status && status.sync ? status.sync : null,
                                                 Model.sourceSync(status, browseSource), browseSource, status)

  implicitWidth: barButton.implicitWidth
  implicitHeight: barButton.implicitHeight

  function refreshStatus() {
    if (statusProc.running) { statusPending = true; return }
    statusProc.start()
  }
  function runAction(args) {
    if (actionProc.running) return
    actionError = ""
    actionProc.start([root.commandPath].concat(args))
  }
  function setSetting(name, value) { runAction(["set", name, String(value)]) }
  function openUrl(url) { if (url) Qt.openUrlExternally(url) }

  // The grid holds a card for every wallpaper of the source, so the scrollbar spans all of it;
  // pages fill in around wherever the view is, one CLI call at a time.
  function loadBrowse(key) {
    wheelGlide.stop()
    browseModel.clear()
    browseSource = key
    browseLastPage = 0
    browseTotal = 0
    browseLoaded = 0
    browseError = ""
    loadedPages = ({})
    selectedIndex = 0
    pumpBrowse()
  }
  function pumpBrowse() {
    if (!browseOpen || browseSource === "" || browseProc.running || browseError !== "") return
    var page = Model.nextPage(grid.contentY, grid.originY, grid.height, gridColumns(), grid.cellHeight,
                              browseModel.count, browseLastPage, loadedPages)
    if (page > 0) browseProc.start(browseSource, page)
  }
  function acceptPage(result) {
    var items = Array.isArray(result.items) ? result.items : []
    var start = (result.page - 1) * Model.PER_PAGE
    var first = browseLastPage === 0
    if (first) {
      browseTotal = result.total
      browseLastPage = result.lastPage
      var blanks = []
      for (var i = 0; i < result.total; i++) blanks.push(Model.browseRow(null))
      browseModel.append(blanks)
    }
    for (var k = 0; k < items.length; k++) {
      var row = Model.browseRow(items[k])
      if (start + k < browseModel.count) browseModel.set(start + k, row)
      else browseModel.append(row)
      // Start the keyboard on the wallpaper in use when it is on the first page.
      if (first && row.current) selectedIndex = start + k
    }
    // Wallhaven's total drifts a little; the last page settles the real length.
    var end = start + items.length
    if (result.page >= browseLastPage && browseModel.count > end) browseModel.remove(end, browseModel.count - end)
    var pages = loadedPages
    pages[result.page] = true
    loadedPages = pages
    browseLoaded += items.length
  }
  function openBrowse() {
    root.close()
    browseOpen = true
    loadBrowse(Model.firstBrowseKey(status))
    Qt.callLater(function() { browseScope.forceActiveFocus() })
  }
  function closeBrowse() { browseOpen = false }
  function gridColumns() { return Math.max(1, Math.floor(grid.width / grid.cellWidth)) }
  function selectMove(move) {
    var rows = Math.max(1, Math.floor(grid.height / grid.cellHeight))
    selectedIndex = Model.moveIndex(selectedIndex, move, gridColumns(), browseModel.count, rows)
    revealSelected()
  }
  // Glide just far enough to bring the selected card fully into view.
  function revealSelected() {
    var top = grid.originY + Math.floor(selectedIndex / gridColumns()) * grid.cellHeight
    var from = wheelGlide.running ? grid.glideTarget : grid.contentY
    var target = from
    if (top < from) target = top
    else if (top + grid.cellHeight > from + grid.height) target = top + grid.cellHeight - grid.height
    var bottom = grid.originY + Math.max(0, grid.contentHeight - grid.height)
    target = Math.max(grid.originY, Math.min(bottom, target))
    if (target === from) return
    grid.glideTarget = target
    if (!wheelGlide.running) wheelGlide.start()
  }
  function selectedWallpaper() {
    if (selectedIndex < 0 || selectedIndex >= browseModel.count) return ""
    return browseModel.get(selectedIndex).wid
  }
  function switchTab(step) {
    var tabs = tabList
    if (!tabs.length) return
    var at = 0
    for (var i = 0; i < tabs.length; i++) if (tabs[i].key === browseSource) at = i
    loadBrowse(tabs[(at + step + tabs.length) % tabs.length].key)
  }
  function browseKey(event) {
    switch (event.key) {
    case Qt.Key_Left: case Qt.Key_H: selectMove("left"); break
    case Qt.Key_Right: case Qt.Key_L: selectMove("right"); break
    case Qt.Key_Up: case Qt.Key_K: selectMove("up"); break
    case Qt.Key_Down: case Qt.Key_J: selectMove("down"); break
    case Qt.Key_PageUp: selectMove("pageUp"); break
    case Qt.Key_PageDown: selectMove("pageDown"); break
    case Qt.Key_Home: selectMove("home"); break
    case Qt.Key_End: selectMove("end"); break
    case Qt.Key_G: selectMove(event.modifiers & Qt.ShiftModifier ? "end" : "home"); break
    case Qt.Key_Return: case Qt.Key_Enter: if (selectedWallpaper()) applyFromBrowse(selectedWallpaper()); break
    case Qt.Key_O: openUrl(Model.wallpaperUrl(selectedWallpaper())); break
    case Qt.Key_Tab: switchTab(1); break
    case Qt.Key_Backtab: switchTab(-1); break
    case Qt.Key_S: syncAction(); break
    case Qt.Key_Escape: case Qt.Key_Q: closeBrowse(); break
    default: event.accepted = false; return
    }
    event.accepted = true
  }
  function applyFromBrowse(id) {
    closeBrowse()
    runAction(["apply", id])
  }
  // Sync outlives the panel and the browse view; progress comes back through syncs.json.
  function syncAction() {
    if (!syncInfo) return
    if (syncInfo.action === "cancel") runAction(["sync", "--cancel"])
    else if (syncInfo.action === "start") Quickshell.execDetached([root.commandPath, "sync", browseSource])
  }

  onOpenedChanged: if (opened) refreshStatus()
  // `omarchy-shell kent.omawall.browse toggle` opens the browse view without the panel.
  ShellIpc {
    target: "kent.omawall.browse"
    function toggle(): void { if (root.browseOpen) root.closeBrowse(); else root.openBrowse() }
    function open(): void { if (!root.browseOpen) root.openBrowse() }
    function close(): void { root.closeBrowse() }
  }
  onStatusChanged: {
    var list = status && status.sources ? status.sources : []
    if (JSON.stringify(list) !== JSON.stringify(sources)) sources = list
    var tabs = Model.browseTabs(status)
    if (JSON.stringify(tabs) !== JSON.stringify(tabList)) tabList = tabs
  }
  Component.onCompleted: refreshStatus()

  // The plugin version comes from its own manifest so a stale CLI is reported.
  FileView {
    path: decodeURIComponent(Qt.resolvedUrl("manifest.json").toString().replace(/^file:\/\//, ""))
    printErrors: false
    onLoaded: {
      try { root.pluginVersion = String(JSON.parse(text()).version || "") } catch (e) { root.pluginVersion = "" }
    }
  }
  // The timer service, keybinding and background sync change state outside this widget.
  FileView {
    path: root.stateHome + "/omawall/status.json"
    watchChanges: true
    printErrors: false
    onFileChanged: statusDebounce.restart()
  }
  FileView {
    path: root.stateHome + "/omawall/settings.json"
    watchChanges: true
    printErrors: false
    onFileChanged: statusDebounce.restart()
  }
  FileView {
    path: root.stateHome + "/omawall/syncs.json"
    watchChanges: true
    printErrors: false
    onFileChanged: statusDebounce.restart()
  }
  FileView {
    id: favoritesFile
    path: root.stateHome + "/omawall/favorites.json"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.favoriteIds = Model.favoriteIds(text())
    onLoadFailed: root.favoriteIds = ({})
  }
  Timer { id: statusDebounce; interval: 150; onTriggered: root.refreshStatus() }
  // Fallback for files that did not exist yet when the watches were set up.
  Timer { interval: 60000; repeat: true; running: true; onTriggered: { root.refreshStatus(); favoritesFile.reload() } }

  Process {
    id: statusProc
    property bool exitSeen: true
    function start() { exitSeen = false; command = [root.commandPath, "status"]; running = true }
    stdout: StdioCollector { id: statusOut; waitForEnd: true }
    onExited: function(code) {
      exitSeen = true
      var result = Model.parseResult(statusOut.text)
      if (result.ok) { root.status = result; root.cliState = "ok" }
      else { root.cliState = "broken"; root.actionError = result.error.message }
      if (root.statusPending) { root.statusPending = false; Qt.callLater(root.refreshStatus) }
    }
    onRunningChanged: {
      if (!running) Qt.callLater(function() {
        if (!statusProc.exitSeen && !statusProc.running) { statusProc.exitSeen = true; root.cliState = "missing" }
      })
    }
  }

  Process {
    id: actionProc
    property bool exitSeen: true
    function start(argv) { exitSeen = false; command = argv; running = true }
    stdout: StdioCollector { id: actionOut; waitForEnd: true }
    onExited: function(code) {
      exitSeen = true
      var result = Model.parseResult(actionOut.text)
      if (!result.ok) root.actionError = result.error.message
      root.refreshStatus()
      favoritesFile.reload()
    }
    onRunningChanged: {
      if (!running) Qt.callLater(function() {
        if (!actionProc.exitSeen && !actionProc.running) { actionProc.exitSeen = true; root.cliState = "missing" }
      })
    }
  }

  Process {
    id: browseProc
    property string source: ""
    property bool exitSeen: true
    function start(key, page) {
      exitSeen = false
      source = key
      command = [root.commandPath, "browse", key, "--page", String(page)]
      running = true
    }
    stdout: StdioCollector { id: browseOut; waitForEnd: true }
    onExited: function(code) {
      exitSeen = true
      // A reply for a source the user has left is dropped.
      if (source === root.browseSource) {
        var result = Model.parseResult(browseOut.text)
        if (!result.ok) root.browseError = result.error.message
        else if (result.source === root.browseSource) root.acceptPage(result)
      }
      Qt.callLater(root.pumpBrowse)
    }
    onRunningChanged: {
      if (!running) Qt.callLater(function() {
        if (!browseProc.exitSeen && !browseProc.running) { browseProc.exitSeen = true; root.browseError = "Could not run the omawall CLI." }
      })
    }
  }

  WidgetButton {
    id: barButton
    anchors.fill: parent
    bar: root.bar
    text: Model.Icons.wallpaper
    active: root.bannerInfo !== null && root.bannerInfo.tone === "error"
    Accessible.name: "omawall wallpaper"
    tooltipText: (root.current
      ? "omawall · " + Model.sourceLabel(root.status, root.current.source) + " · " + root.current.id
      : "omawall") + "\nLeft click: next wallpaper · Right click: settings"
    onPressed: function(button) {
      if (button === Qt.LeftButton) root.runAction(["next"])
      else root.toggle()
    }
  }

  KeyboardPanel {
    id: popup
    anchorItem: barButton
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: focusScope
    contentWidth: fittedContentWidth(Style.space(400))
    contentHeight: fittedContentHeight(Math.ceil(content.implicitHeight))

    FocusScope {
      id: focusScope
      anchors.fill: parent
      Keys.onEscapePressed: root.close()

      Flickable {
        anchors.fill: parent
        contentHeight: content.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        QQC.ScrollBar.vertical: QQC.ScrollBar {}

        ColumnLayout {
          id: content
          width: parent.width
          spacing: Style.spacing.lg

          RowLayout {
            visible: root.actionError !== "" || root.bannerInfo !== null
            Layout.fillWidth: true
            Label {
              text: root.actionError || (root.bannerInfo ? root.bannerInfo.text : "")
              color: root.actionError || (root.bannerInfo && root.bannerInfo.tone === "error") ? Color.urgent : Color.accent
              Layout.fillWidth: true
            }
            ActionButton {
              visible: root.hasLastError && root.cliState === "ok"
              text: "Retry"
              onClicked: root.runAction(["next"])
            }
          }

          RowLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.xl
            SizedImage {
              visible: root.current !== null && Model.fileUrl(root.current.thumb) !== ""
              url: root.current ? Model.fileUrl(root.current.thumb) : ""
              crop: true
              Layout.preferredWidth: Style.space(128)
              Layout.preferredHeight: Style.space(72)
            }
            ColumnLayout {
              Layout.fillWidth: true
              spacing: Style.spacing.xs
              Label {
                text: root.current
                  ? Model.sourceLabel(root.status, root.current.source) + " · " + root.current.id
                  : "No Wallhaven wallpaper yet"
                font.bold: true
                font.pixelSize: Style.font.title
                Layout.fillWidth: true
              }
              Label {
                visible: root.current !== null
                text: root.current ? Model.formatResolution(root.current.resolution) + " · " + Model.formatSize(root.current.fileSize) : ""
                color: root.dim
                Layout.fillWidth: true
              }
              Label {
                visible: root.current !== null
                text: root.current ? Model.formatCount(root.current.favorites) + " favorites on Wallhaven" : ""
                color: root.dim
                Layout.fillWidth: true
              }
            }
          }

          GridLayout {
            Layout.fillWidth: true
            columns: 4
            columnSpacing: Style.spacing.md
            ActionButton {
              text: "Next"
              selected: true
              enabled: !root.busy
              iconSpinning: root.busy
              onClicked: root.runAction(["next"])
            }
            ActionButton { text: "Browse"; onClicked: root.openBrowse() }
            ActionButton {
              text: root.currentIsFav ? "Favorited" : "Favorite"
              selected: root.currentIsFav
              enabled: !root.busy && root.current !== null
              onClicked: root.runAction(["fav"])
            }
            ActionButton {
              text: "Open page"
              enabled: root.current !== null && Model.wallpaperUrl(root.current.id) !== ""
              onClicked: root.openUrl(Model.wallpaperUrl(root.current.id))
            }
          }

          RowLayout {
            Layout.fillWidth: true
            Label { text: "Sources"; font.bold: true }
            Label {
              text: Model.selectedSources(root.status).length + " of " + (root.sources.length + 1) + " selected"
                + (Model.poolSize(root.status) !== null ? " · about " + Model.formatCount(Model.poolSize(root.status)) + " wallpapers" : "")
              color: root.dim
              Layout.fillWidth: true
            }
            LinkButton { text: "All"; onClicked: root.setSetting("sources", Model.allSources(root.status)) }
            LinkButton { text: "None"; onClicked: root.setSetting("sources", "") }
          }
          ActionButton {
            Layout.fillWidth: true
            leftAlign: true
            text: "Favorites · " + Model.formatCount(root.favInfo.count) + " · local, filters don't apply"
            selected: root.favInfo.selected === true
            enabled: !root.busy
            onClicked: root.setSetting("sources", Model.toggledSources(root.status, "favs"))
          }
          GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: Style.spacing.md
            rowSpacing: Style.spacing.md
            Repeater {
              model: root.sources
              delegate: ActionButton {
                required property var modelData
                leftAlign: true
                text: String(modelData.label) + "  " + Model.formatCount(modelData.total)
                selected: modelData.selected === true
                enabled: !root.busy
                onClicked: root.setSetting("sources", Model.toggledSources(root.status, modelData.key))
              }
            }
          }

          Label { text: "Style"; font.bold: true }
          RowLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.md
            ActionButton {
              text: "General"
              selected: !!(root.userSettings.styles && root.userSettings.styles.general)
              enabled: !root.busy
              onClicked: root.setSetting("style.general", !selected)
            }
            ActionButton {
              text: "Anime"
              selected: !!(root.userSettings.styles && root.userSettings.styles.anime)
              enabled: !root.busy
              onClicked: root.setSetting("style.anime", !selected)
            }
          }

          Label { text: "Color"; font.bold: true }
          RowLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.md
            ActionButton {
              text: "Any"
              selected: root.userSettings.color !== "theme"
              enabled: !root.busy
              onClicked: root.setSetting("color", "none")
            }
            ActionButton {
              text: "Theme accent"
              selected: root.userSettings.color === "theme"
              enabled: !root.busy
              onClicked: root.setSetting("color", "theme")
            }
          }

          RowLayout {
            Layout.fillWidth: true
            Label { text: "Timer"; font.bold: true }
            Label { text: Model.timerText(root.userSettings); color: root.dim; Layout.fillWidth: true }
            ToggleSwitch {
              checked: root.timerEnabled
              enabled: !root.busy
              Accessible.name: "Change wallpaper on a timer"
              onToggled: root.setSetting("timer.enabled", !root.timerEnabled)
            }
          }
          NumberField {
            label: "Minutes between wallpapers (5–1440)"
            from: Model.MIN_MINUTES
            to: Model.MAX_MINUTES
            value: root.timerMinutes
            enabled: root.timerEnabled && !root.busy
            foreground: root.fg
            onModified: function(value) { root.setSetting("timer.minutes", Model.clampMinutes(value)) }
          }
        }
      }
    }
  }

  OverlayWindow {
    id: browseWindow
    shown: root.browseOpen
    WlrLayershell.namespace: "omawall-browse"

    // Dimmed backdrop; a click outside the panel closes it.
    Rectangle {
      anchors.fill: parent
      color: Qt.rgba(0, 0, 0, 0.35)
      MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton | Qt.MiddleButton
        onClicked: root.closeBrowse()
      }
    }

    Rectangle {
      id: browsePanel
      anchors.centerIn: parent
      width: Math.round(parent.width * 0.8)
      height: Math.round(parent.height * 0.85)
      color: Color.popups.background
      border.color: Color.popups.border
      border.width: Math.max(1, Style.space(2))
      radius: Style.cornerRadius

      // Clicks inside the panel must not reach the backdrop.
      MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons }

      FocusScope {
        id: browseScope
        anchors.fill: parent
        focus: true
        Keys.onPressed: function(event) { root.browseKey(event) }

        ColumnLayout {
          id: browseColumn
          anchors.fill: parent
          anchors.margins: Style.space(32)
          spacing: Style.spacing.huge

          RowLayout {
            Layout.fillWidth: true
            ColumnLayout {
              Layout.fillWidth: true
              spacing: Style.spacing.xs
              // Labels fill the column so it can grow and push Close to the right edge.
              Label { text: "Browse wallpapers"; color: Color.foreground; font.bold: true; font.pixelSize: Style.font.heading; Layout.fillWidth: true }
              Label {
                text: "SFW · 4K or larger · 16:9 · most favorited first · " + Model.stylesText(root.userSettings) + " · " + Model.colorText(root.userSettings)
                color: Qt.darker(Color.foreground, 1.45)
                Layout.fillWidth: true
              }
            }
            ActionButton { text: "Close"; Layout.fillWidth: false; focusable: false; onClicked: root.closeBrowse() }
          }

          Flow {
            Layout.fillWidth: true
            spacing: Style.spacing.md
            Repeater {
              model: root.tabList
              delegate: ActionButton {
                required property var modelData
                text: (modelData.selected ? "• " : "") + modelData.label + "  " + Model.formatCount(modelData.count)
                selected: root.browseSource === modelData.key
                focusable: false
                onClicked: root.loadBrowse(modelData.key)
              }
            }
          }

          Label {
            visible: root.browseError !== ""
            text: root.browseError
            color: Color.urgent
          }
          Label {
            visible: root.browseError === "" && browseModel.count === 0
            text: root.browseLastPage === 0 ? "Loading…"
              : (root.browseSource === "favs" ? "No favorites yet. Press Favorite in the panel to add the current wallpaper." : "No wallpapers match these filters.")
            color: Qt.darker(Color.foreground, 1.45)
          }

          GridView {
            id: grid
            // Whole columns only, centered in the panel.
            Layout.preferredWidth: Math.max(cellWidth, Math.floor(browseColumn.width / cellWidth) * cellWidth)
            Layout.alignment: Qt.AlignHCenter
            Layout.fillHeight: true
            clip: true
            reuseItems: true
            cellWidth: 432 + Style.space(16)
            cellHeight: 243 + Style.space(40)
            model: ListModel { id: browseModel }
            boundsBehavior: Flickable.StopAtBounds
            // Build cards and decode thumbnails a screen ahead, so they are ready before they scroll in.
            cacheBuffer: Math.max(0, Math.round(height))
            // Always shown: the wheel glide sets contentY directly, which never marks the view as moving,
            // so an as-needed bar would stay hidden. Thousands of cards would shrink the handle to a
            // sliver; keep it grabbable.
            QQC.ScrollBar.vertical: QQC.ScrollBar {
              policy: QQC.ScrollBar.AlwaysOn
              minimumSize: 0.05
              onPressedChanged: if (pressed) wheelGlide.stop()
            }
            // Fetch the pages under the view at most every 150 ms while it moves.
            onContentYChanged: if (!pumpTimer.running) pumpTimer.start()
            onHeightChanged: if (!pumpTimer.running) pumpTimer.start()
            Timer { id: pumpTimer; interval: 150; onTriggered: root.pumpBrowse() }

            // A mouse wheel moves one row per notch, up to four when spun fast. Each frame closes
            // part of the gap to the target, so continued spinning keeps a steady speed instead of
            // restarting an ease per notch. Touchpads keep Flickable's own scrolling.
            property real wheelAvg: 0
            property double wheelAt: 0
            property real glideTarget: 0
            FrameAnimation {
              id: wheelGlide
              onTriggered: {
                var k = 1 - Math.exp(-Math.min(frameTime, 0.05) / 0.07)
                var y = grid.contentY + (grid.glideTarget - grid.contentY) * k
                if (Math.abs(grid.glideTarget - y) < 0.5) {
                  y = grid.glideTarget
                  stop()
                }
                grid.contentY = y
              }
            }
            WheelHandler {
              target: null
              // Wayland hands wheel events to Qt as a touchpad device, so a mouse-only handler sees nothing.
              acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
              onWheel: function(event) {
                // Touchpads scroll in phases; a wheel has none. Wayland gives wheels pixel deltas too,
                // so those cannot tell them apart.
                if (event.phase !== Qt.NoScrollPhase || event.angleDelta.y === 0) { event.accepted = false; return }
                var now = Date.now()
                var speed = Model.wheelSpeed(grid.wheelAvg, event.angleDelta.y, now - grid.wheelAt)
                grid.wheelAvg = speed.avg
                grid.wheelAt = now
                var bottom = grid.originY + Math.max(0, grid.contentHeight - grid.height)
                var from = wheelGlide.running ? grid.glideTarget : grid.contentY
                grid.glideTarget = Math.max(grid.originY, Math.min(bottom, from - event.angleDelta.y / 120 * grid.cellHeight * speed.boost))
                if (!wheelGlide.running) wheelGlide.start()
                event.accepted = true
              }
            }

            delegate: Item {
              id: card
              required property string wid
              required property string thumb
              required property string resolution
              required property real favs
              required property real size
              required property bool seen
              required property bool fav
              required property bool current
              required property int index
              readonly property bool picked: index === root.selectedIndex
              width: grid.cellWidth
              height: grid.cellHeight

              Rectangle {
                id: frame
                width: 432
                height: 243 + Style.space(28)
                color: Qt.rgba(Color.foreground.r, Color.foreground.g, Color.foreground.b, 0.05)
                // Keyboard selection outranks the wallpaper in use and the hover.
                border.width: card.picked ? 3 : (card.current || mouse.containsMouse ? 2 : 0)
                border.color: card.picked ? Color.foreground : (card.current ? Color.accent : Qt.darker(Color.foreground, 1.6))

                SizedImage {
                  x: 0
                  y: 0
                  width: 432
                  height: 243
                  url: Model.fileUrl(card.thumb)
                  crop: true
                }
                Label {
                  anchors.left: parent.left
                  anchors.bottom: parent.bottom
                  anchors.margins: Style.space(6)
                  visible: card.wid !== ""
                  text: Model.formatResolution(card.resolution) + " · " + Model.formatSize(card.size)
                  color: Qt.darker(Color.foreground, 1.3)
                  font.pixelSize: Style.font.bodySmall
                  wrapMode: Text.NoWrap
                }
                Label {
                  anchors.right: parent.right
                  anchors.bottom: parent.bottom
                  anchors.margins: Style.space(6)
                  visible: card.wid !== ""
                  text: Model.formatCount(card.favs) + " favorites"
                  color: Qt.darker(Color.foreground, 1.3)
                  font.pixelSize: Style.font.bodySmall
                  wrapMode: Text.NoWrap
                }
                Row {
                  x: Style.space(8)
                  y: Style.space(8)
                  spacing: Style.space(6)
                  Badge { visible: card.current; text: "Current"; strong: true }
                  Badge { visible: card.seen && !card.current; text: "Seen" }
                  Badge { visible: card.fav; text: "Favorite" }
                }
                MouseArea {
                  id: mouse
                  anchors.fill: parent
                  enabled: card.wid !== ""
                  hoverEnabled: true
                  cursorShape: Qt.PointingHandCursor
                  onClicked: root.applyFromBrowse(card.wid)
                }
                Accessible.role: Accessible.Button
                Accessible.name: "Apply " + card.wid
              }
            }
          }

          RowLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.md
            Label {
              visible: root.browseLastPage > 0
              text: "Page " + Math.min(root.browseLastPage, Model.pageOf(Math.floor(Math.max(0, grid.contentY - grid.originY) / grid.cellHeight)
                                                                         * Math.max(1, Math.floor(grid.width / grid.cellWidth))))
                + " of " + root.browseLastPage + " · " + Model.formatCount(root.browseLoaded) + " of "
                + Model.formatCount(root.browseTotal) + " loaded" + (root.browseBusy ? " · loading…" : "")
              color: Qt.darker(Color.foreground, 1.45)
              wrapMode: Text.NoWrap
            }
            Button {
              visible: root.syncInfo !== null && root.browseLastPage > 0
              iconText: root.syncInfo ? root.syncInfo.icon : ""
              iconSpinning: root.syncInfo !== null && root.syncInfo.spinning
              text: root.syncInfo ? root.syncInfo.text : ""
              tooltipText: root.syncInfo ? root.syncInfo.tooltip : ""
              foreground: root.syncInfo && root.syncInfo.tone === "error" ? Color.urgent : Qt.darker(Color.foreground, 1.45)
              fontSize: Style.font.bodySmall
              enabled: root.cliState === "ok" && !(root.syncInfo && root.syncInfo.action === "cancel" && root.busy)
              Accessible.role: Accessible.Button
              Accessible.name: tooltipText
              onClicked: root.syncAction()
            }
            Button {
              visible: root.browseLastPage > 0
              iconText: Model.Icons.folder
              tooltipText: "Open thumbnails folder"
              foreground: Qt.darker(Color.foreground, 1.45)
              Accessible.role: Accessible.Button
              Accessible.name: tooltipText
              onClicked: Util.execArgv(["xdg-open", root.thumbsDir])
            }
            Item { Layout.fillWidth: true }
            Label {
              text: "←↑↓→ / hjkl move · Enter apply · o open · Tab category · s sync · Esc close"
              color: Qt.darker(Color.foreground, 1.6)
              font.pixelSize: Style.font.bodySmall
              wrapMode: Text.NoWrap
            }
          }
        }
      }
    }
  }

  component Label: Text {
    textFormat: Text.PlainText
    wrapMode: Text.Wrap
    color: root.fg
    font.family: Style.font.family
    font.pixelSize: Style.font.body
  }

  component ActionButton: Button {
    Accessible.role: Accessible.Button
    Accessible.name: text
    Layout.fillWidth: true
    foreground: root.fg
    bordered: true
    focusable: true
  }

  component LinkButton: Button {
    Accessible.role: Accessible.Button
    Accessible.name: text
    foreground: Color.accent
    focusable: true
    fontSize: Style.font.bodySmall
  }

  component Badge: Rectangle {
    property string text: ""
    property bool strong: false
    width: badgeText.implicitWidth + Style.space(12)
    height: badgeText.implicitHeight + Style.space(4)
    radius: Style.cornerRadius > 0 ? Style.space(4) : 0
    color: strong ? Color.accent : Qt.rgba(Color.background.r, Color.background.g, Color.background.b, 0.85)
    Text {
      id: badgeText
      anchors.centerIn: parent
      textFormat: Text.PlainText
      text: parent.text
      color: parent.strong ? Color.background : Color.foreground
      font.family: Style.font.family
      font.pixelSize: Style.font.caption
    }
  }
}
