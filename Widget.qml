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
  property var browseItems: []
  property int browsePage: 0
  property int browseLastPage: 0
  property int browseTotal: 0
  property string browseError: ""
  property bool browseBusy: false

  readonly property var sources: status && status.sources ? status.sources : []
  readonly property var favInfo: status && status.favorites ? status.favorites : ({ count: 0, selected: false })
  readonly property var userSettings: status && status.settings ? status.settings : ({})
  readonly property var current: status && status.status ? status.status.current : null
  readonly property bool currentIsFav: current !== null && root.favoriteIds[current.id] === true
  readonly property var bannerInfo: Model.banner(cliState, status, pluginVersion)
  readonly property bool hasLastError: !!(status && status.status && status.status.lastError)
  readonly property bool timerEnabled: !!(userSettings.timer && userSettings.timer.enabled)
  readonly property int timerMinutes: Model.clampMinutes(userSettings.timer ? userSettings.timer.minutes : 60)
  readonly property bool busy: actionProc.running
  readonly property bool browseLoading: browseBusy

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

  function loadBrowse(key, page) {
    if (browseBusy) return
    browseBusy = true
    if (page === 1) { browseItems = []; browsePage = 0; browseLastPage = 0; browseTotal = 0 }
    browseSource = key
    browseError = ""
    browseProc.start([root.commandPath, "browse", key, "--page", String(page)])
  }
  function loadMore() {
    if (!browseBusy && browsePage > 0 && browsePage < browseLastPage) loadBrowse(browseSource, browsePage + 1)
  }
  function openBrowse() {
    root.close()
    browseOpen = true
    loadBrowse(Model.firstBrowseKey(status), 1)
    Qt.callLater(function() { browseScope.forceActiveFocus() })
  }
  function closeBrowse() { browseOpen = false }
  function applyFromBrowse(id) {
    closeBrowse()
    runAction(["apply", id])
  }

  onOpenedChanged: if (opened) refreshStatus()
  Component.onCompleted: refreshStatus()

  // The plugin version comes from its own manifest so a stale CLI is reported.
  FileView {
    path: decodeURIComponent(Qt.resolvedUrl("manifest.json").toString().replace(/^file:\/\//, ""))
    printErrors: false
    onLoaded: {
      try { root.pluginVersion = String(JSON.parse(text()).version || "") } catch (e) { root.pluginVersion = "" }
    }
  }
  // The timer service and keybinding change state outside this widget.
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
    property bool exitSeen: true
    function start(argv) { exitSeen = false; command = argv; running = true }
    stdout: StdioCollector { id: browseOut; waitForEnd: true }
    onExited: function(code) {
      exitSeen = true
      root.browseBusy = false
      var result = Model.parseResult(browseOut.text)
      if (!result.ok) { root.browseError = result.error.message; return }
      if (result.source !== root.browseSource) return
      var items = Array.isArray(result.items) ? result.items : []
      root.browseItems = result.page === 1 ? items : root.browseItems.concat(items)
      root.browsePage = result.page
      root.browseLastPage = result.lastPage
      root.browseTotal = result.total
      // A page that fits on screen leaves nothing to scroll; fetch the next one.
      Qt.callLater(function() { if (grid.atYEnd) root.loadMore() })
    }
    onRunningChanged: {
      if (!running) Qt.callLater(function() {
        if (!browseProc.exitSeen && !browseProc.running) { browseProc.exitSeen = true; root.browseBusy = false; root.browseError = "無法執行 omawall CLI。" }
      })
    }
  }

  WidgetButton {
    id: barButton
    anchors.fill: parent
    bar: root.bar
    text: Model.Icons.wallpaper
    active: root.bannerInfo !== null && root.bannerInfo.tone === "error"
    Accessible.name: "omawall 桌布"
    tooltipText: root.current
      ? "omawall · " + Model.sourceLabel(root.status, root.current.source) + " · " + root.current.id + "\n左鍵：下一張　右鍵：設定"
      : "omawall\n左鍵：下一張　右鍵：設定"
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
              text: "重試"
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
                  : "尚未套用 Wallhaven 桌布"
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
                text: root.current ? "收藏數 " + Model.formatCount(root.current.favorites) : ""
                color: root.dim
                Layout.fillWidth: true
              }
            }
          }

          GridLayout {
            Layout.fillWidth: true
            columns: 5
            columnSpacing: Style.spacing.md
            ActionButton {
              text: "下一張"
              selected: true
              enabled: !root.busy
              iconSpinning: root.busy
              onClicked: root.runAction(["next"])
            }
            ActionButton { text: "瀏覽"; onClicked: root.openBrowse() }
            ActionButton {
              text: "不喜歡"
              enabled: !root.busy && root.current !== null
              onClicked: root.runAction(["dislike"])
            }
            ActionButton {
              text: root.currentIsFav ? "已收藏" : "收藏"
              selected: root.currentIsFav
              enabled: !root.busy && root.current !== null
              onClicked: root.runAction(["fav"])
            }
            ActionButton {
              text: "原頁"
              enabled: root.current !== null && Model.wallpaperUrl(root.current.id) !== ""
              onClicked: root.openUrl(Model.wallpaperUrl(root.current.id))
            }
          }

          RowLayout {
            Layout.fillWidth: true
            Label { text: "來源"; font.bold: true }
            Label {
              text: "已選 " + Model.selectedSources(root.status).length + "／" + (root.sources.length + 1)
                + (Model.poolSize(root.status) !== null ? " · 約 " + Model.formatCount(Model.poolSize(root.status)) + " 張" : "")
              color: root.dim
              Layout.fillWidth: true
            }
            LinkButton { text: "全選"; onClicked: root.setSetting("sources", Model.allSources(root.status)) }
            LinkButton { text: "清除"; onClicked: root.setSetting("sources", "") }
          }
          ActionButton {
            Layout.fillWidth: true
            leftAlign: true
            text: "收藏 · " + Model.formatCount(root.favInfo.count) + " 張 · 本機，不套用篩選"
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

          Label { text: "畫風"; font.bold: true }
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

          Label { text: "篩色"; font.bold: true }
          RowLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.md
            ActionButton {
              text: "不篩"
              selected: root.userSettings.color !== "theme"
              enabled: !root.busy
              onClicked: root.setSetting("color", "none")
            }
            ActionButton {
              text: "跟隨主題 accent"
              selected: root.userSettings.color === "theme"
              enabled: !root.busy
              onClicked: root.setSetting("color", "theme")
            }
          }

          RowLayout {
            Layout.fillWidth: true
            Label { text: "定時換圖"; font.bold: true }
            Label { text: Model.timerText(root.userSettings); color: root.dim; Layout.fillWidth: true }
            ToggleSwitch {
              checked: root.timerEnabled
              enabled: !root.busy
              Accessible.name: "定時換圖開關"
              onToggled: root.setSetting("timer.enabled", !root.timerEnabled)
            }
          }
          NumberField {
            label: "每幾分鐘換一張（5–1440）"
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

    Rectangle {
      anchors.fill: parent
      color: Qt.rgba(Color.background.r, Color.background.g, Color.background.b, 0.94)
    }

    FocusScope {
      id: browseScope
      anchors.fill: parent
      focus: true
      Keys.onEscapePressed: root.closeBrowse()

      ColumnLayout {
        anchors.fill: parent
        anchors.margins: Style.space(48)
        spacing: Style.spacing.huge

        RowLayout {
          Layout.fillWidth: true
          ColumnLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.xs
            Label { text: "瀏覽桌布"; color: Color.foreground; font.bold: true; font.pixelSize: Style.font.heading }
            Label {
              text: "SFW · 4K 以上 · 16:9 · 依收藏數排序 · 畫風 " + Model.stylesText(root.userSettings) + " · " + Model.colorText(root.userSettings)
              color: Qt.darker(Color.foreground, 1.45)
            }
          }
          ActionButton { text: "關閉"; Layout.fillWidth: false; onClicked: root.closeBrowse() }
        }

        Flow {
          Layout.fillWidth: true
          spacing: Style.spacing.md
          Repeater {
            model: Model.browseTabs(root.status)
            delegate: ActionButton {
              required property var modelData
              text: (modelData.selected ? "• " : "") + modelData.label + "  " + Model.formatCount(modelData.count)
              selected: root.browseSource === modelData.key
              onClicked: root.loadBrowse(modelData.key, 1)
            }
          }
        }

        Label {
          visible: root.browseError !== ""
          text: root.browseError
          color: Color.urgent
        }
        Label {
          visible: root.browseError === "" && root.browseItems.length === 0
          text: root.browseLoading ? "載入中…"
            : (root.browseSource === "favs" ? "還沒有收藏。在 widget 按「收藏」，就會出現在這裡。" : "沒有符合條件的桌布。")
          color: Qt.darker(Color.foreground, 1.45)
        }

        GridView {
          id: grid
          Layout.fillWidth: true
          Layout.fillHeight: true
          clip: true
          reuseItems: true
          cellWidth: 432 + Style.space(16)
          cellHeight: 243 + Style.space(40)
          model: root.browseItems
          boundsBehavior: Flickable.StopAtBounds
          QQC.ScrollBar.vertical: QQC.ScrollBar {}
          onAtYEndChanged: if (atYEnd) root.loadMore()

          delegate: Item {
            id: card
            required property var modelData
            width: grid.cellWidth
            height: grid.cellHeight

            Rectangle {
              id: frame
              width: 432
              height: 243 + Style.space(28)
              color: Qt.rgba(Color.foreground.r, Color.foreground.g, Color.foreground.b, 0.05)
              border.width: card.modelData.current || mouse.containsMouse ? 2 : 0
              border.color: card.modelData.current ? Color.accent : Qt.darker(Color.foreground, 1.6)

              SizedImage {
                x: 0
                y: 0
                width: 432
                height: 243
                url: Model.fileUrl(card.modelData.thumb)
                crop: true
                opacity: card.modelData.disliked ? 0.35 : 1
              }
              Label {
                anchors.left: parent.left
                anchors.bottom: parent.bottom
                anchors.margins: Style.space(6)
                text: Model.formatResolution(card.modelData.resolution) + " · " + Model.formatSize(card.modelData.fileSize)
                color: Qt.darker(Color.foreground, 1.3)
                font.pixelSize: Style.font.bodySmall
                wrapMode: Text.NoWrap
              }
              Label {
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.margins: Style.space(6)
                text: "收藏 " + Model.formatCount(card.modelData.favorites)
                color: Qt.darker(Color.foreground, 1.3)
                font.pixelSize: Style.font.bodySmall
                wrapMode: Text.NoWrap
              }
              Row {
                x: Style.space(8)
                y: Style.space(8)
                spacing: Style.space(6)
                Badge { visible: card.modelData.current === true; text: "使用中"; strong: true }
                Badge { visible: card.modelData.seen === true && card.modelData.current !== true; text: "看過" }
                Badge { visible: card.modelData.favorite === true; text: "已收藏" }
                Badge { visible: card.modelData.disliked === true; text: "已排除" }
              }
              MouseArea {
                id: mouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.applyFromBrowse(card.modelData.id)
              }
              Accessible.role: Accessible.Button
              Accessible.name: "套用 " + card.modelData.id
            }
          }
        }

        Label {
          visible: root.browsePage > 0
          text: "第 " + root.browsePage + "／" + root.browseLastPage + " 頁 · 已載入 " + Model.formatCount(root.browseItems.length)
            + "／" + Model.formatCount(root.browseTotal) + " 張" + (root.browseLoading ? " · 載入中…" : "")
          color: Qt.darker(Color.foreground, 1.45)
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
