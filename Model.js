.pragma library

// Nerd Fonts Material Design glyphs, verified against nerd-fonts/glyphnames.json.
var iconFont = "Symbols Nerd Font"
var Icons = {
  wallpaper: "\uDB83\uDE09",   // md-wallpaper
  next: "\uDB81\uDCAD",        // md-skip_next
  browse: "\uDB80\uDEF9",      // md-image_multiple
  fav: "\uDB80\uDED5",         // md-heart_outline
  faved: "\uDB80\uDED1",       // md-heart
  open: "\uDB80\uDFCC",        // md-open_in_new
  alert: "\uDB80\uDC28",       // md-alert_circle
  timer: "\uDB81\uDD1B",       // md-timer_outline
  sync: "\uDB81\uDCE6",        // md-sync
  syncAlert: "\uDB81\uDCE7",   // md-sync_alert
  synced: "\uDB81\uDDE1"       // md-check_circle_outline
}

var MIN_MINUTES = 5
var MAX_MINUTES = 1440
var FAVS = "favs"

// Parse one CLI stdout payload. Never throws; malformed output becomes an error result.
function parseResult(text) {
  var raw = String(text || "").trim()
  if (!raw) return { ok: false, error: { code: "invalid_response", message: "omawall printed nothing." } }
  try {
    var parsed = JSON.parse(raw)
    if (parsed && typeof parsed === "object" && typeof parsed.ok === "boolean") {
      if (!parsed.ok && !(parsed.error && typeof parsed.error.message === "string"))
        parsed.error = { code: "invalid_response", message: "omawall returned a malformed error." }
      return parsed
    }
  } catch (e) {}
  return { ok: false, error: { code: "invalid_response", message: "omawall returned malformed output." } }
}

function formatCount(n) {
  if (typeof n !== "number" || !isFinite(n) || n < 0) return "—"
  return String(Math.round(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",")
}

function formatSize(bytes) {
  if (typeof bytes !== "number" || !isFinite(bytes) || bytes < 0) return ""
  return (bytes / 1048576).toFixed(1) + " MB"
}

function formatResolution(res) {
  var m = /^(\d{1,5})x(\d{1,5})$/.exec(String(res || ""))
  return m ? m[1] + "×" + m[2] : ""
}

// Only Wallhaven wallpaper pages are opened from the plugin.
function wallpaperUrl(id) {
  return /^[a-z0-9]{1,16}$/.test(String(id || "")) ? "https://wallhaven.cc/w/" + id : ""
}

function fileUrl(path) {
  var p = String(path || "")
  if (p.charAt(0) !== "/") return ""
  return "file://" + p.split("/").map(encodeURIComponent).join("/")
}

function sourceLabel(status, key) {
  if (key === FAVS) return "Favorites"
  // `apply` from the browse overlay records no source.
  if (!key) return "Wallhaven"
  var sources = status && status.sources ? status.sources : []
  for (var i = 0; i < sources.length; i++) if (sources[i].key === key) return String(sources[i].label)
  return ""
}

function selectedSources(status) {
  var keys = []
  if (status && status.favorites && status.favorites.selected) keys.push(FAVS)
  var sources = status && status.sources ? status.sources : []
  for (var i = 0; i < sources.length; i++) if (sources[i].selected) keys.push(sources[i].key)
  return keys
}

function toggledSources(status, key) {
  var keys = selectedSources(status)
  var at = keys.indexOf(key)
  if (at === -1) keys.push(key)
  else keys.splice(at, 1)
  return keys.join(",")
}

function allSources(status) {
  var keys = [FAVS]
  var sources = status && status.sources ? status.sources : []
  for (var i = 0; i < sources.length; i++) keys.push(sources[i].key)
  return keys.join(",")
}

// Pool size of the selected sources; null until every selected total is known,
// so a partial sum is never shown as the whole pool.
function poolSize(status) {
  var total = 0
  var any = false
  var sources = status && status.sources ? status.sources : []
  for (var i = 0; i < sources.length; i++) {
    if (!sources[i].selected) continue
    if (typeof sources[i].total !== "number") return null
    total += sources[i].total
    any = true
  }
  if (status && status.favorites && status.favorites.selected) { total += status.favorites.count || 0; any = true }
  return any ? total : null
}

// Set of favorite ids from favorites.json text.
function favoriteIds(text) {
  var ids = {}
  try {
    var items = JSON.parse(String(text || "")).items
    if (Array.isArray(items))
      for (var i = 0; i < items.length; i++)
        if (items[i] && /^[a-z0-9]{1,16}$/.test(String(items[i].id))) ids[items[i].id] = true
  } catch (e) {}
  return ids
}

// Tabs for the browse overlay: every configured source, then favorites.
function browseTabs(status) {
  var tabs = []
  var sources = status && status.sources ? status.sources : []
  for (var i = 0; i < sources.length; i++)
    tabs.push({ key: sources[i].key, label: String(sources[i].label), count: sources[i].total, selected: !!sources[i].selected })
  var favs = status && status.favorites ? status.favorites : { count: 0, selected: false }
  tabs.push({ key: FAVS, label: "Favorites", count: favs.count || 0, selected: !!favs.selected })
  return tabs
}

function firstBrowseKey(status) {
  var selected = selectedSources(status).filter(function(k) { return k !== FAVS })
  if (selected.length) return selected[0]
  var sources = status && status.sources ? status.sources : []
  return sources.length ? sources[0].key : FAVS
}

function clampMinutes(value) {
  var n = Math.round(Number(value))
  if (!isFinite(n)) return 60
  return Math.min(MAX_MINUTES, Math.max(MIN_MINUTES, n))
}

// Timer settings straight from settings.json text; anything unreadable disables the timer.
function timerSettings(text) {
  try {
    var parsed = JSON.parse(String(text || ""))
    var timer = parsed && parsed.timer ? parsed.timer : null
    if (timer && timer.enabled === true) return { enabled: true, minutes: clampMinutes(timer.minutes) }
  } catch (e) {}
  return { enabled: false, minutes: 60 }
}

function timerText(settings) {
  var timer = settings && settings.timer ? settings.timer : null
  if (!timer || !timer.enabled) return "Off"
  return "Every " + formatCount(clampMinutes(timer.minutes)) + " min"
}

function colorText(settings) {
  return settings && settings.color === "theme" ? "Theme accent" : "Any color"
}

function stylesText(settings) {
  var styles = settings && settings.styles ? settings.styles : {}
  var names = []
  if (styles.general) names.push("General")
  if (styles.anime) names.push("Anime")
  return names.join(" + ")
}

// Banner for the popup: CLI missing or outdated wins over the last command error.
function banner(cliState, status, pluginVersion) {
  if (cliState === "missing")
    return { tone: "error", text: "omawall CLI not found. Run scripts/install-cli from the plugin folder." }
  if (cliState === "broken")
    return { tone: "error", text: "omawall CLI failed to run or returned malformed output." }
  if (status && pluginVersion && status.version && status.version !== pluginVersion)
    return { tone: "error", text: "CLI " + status.version + " does not match plugin " + pluginVersion + ". Run scripts/install-cli again." }
  var err = status && status.status ? status.status.lastError : null
  if (!err || typeof err.message !== "string") return null
  return { tone: err.code === "rate_limited" ? "warn" : "error", text: err.message }
}

// Sync control next to the page counter in the browse view, for a tag source.
// Only one sync runs at a time, so a running sync of another source offers Cancel too.
function syncView(sync, key, status) {
  if (!key || key === FAVS) return null
  var label = function(k) { return sourceLabel(status, k) || k }
  var progress = sync ? formatCount(sync.done) + " / " + (typeof sync.total === "number" ? formatCount(sync.total) : "…") : ""
  var view = { icon: Icons.sync, text: "", tooltip: "Sync all " + label(key) + " thumbnails", action: "start", tone: "normal", spinning: false }
  if (sync && sync.state === "running") {
    var here = sync.source === key
    view.text = here ? progress : label(sync.source) + " " + progress
    view.tooltip = here ? "Cancel sync" : "Cancel " + label(sync.source) + " sync"
    view.action = "cancel"
    view.spinning = true
  } else if (sync && sync.source === key) {
    if (sync.state === "done") {
      view.icon = Icons.synced
      view.tooltip = formatCount(sync.done) + " thumbnails synced · click to sync again"
    } else if (sync.state === "failed") {
      view.icon = Icons.syncAlert
      view.tone = "error"
      view.tooltip = "Sync failed: " + (sync.error && typeof sync.error.message === "string" ? sync.error.message : "unknown error") + " · click to retry"
    } else if (sync.state === "canceled" || sync.state === "stopped") {
      view.text = progress
      view.tooltip = "Sync stopped · click to resume"
    }
  }
  return view
}

// One browse card as a flat ListModel row; null and missing fields get typed defaults
// so the first row fixes each role's type. `id` cannot be a QML property, hence `wid`.
function browseRow(item) {
  return {
    wid: String(item && item.id || ""),
    thumb: item && typeof item.thumb === "string" ? item.thumb : "",
    resolution: String(item && item.resolution || ""),
    favs: item && typeof item.favorites === "number" ? item.favorites : -1,
    size: item && typeof item.fileSize === "number" ? item.fileSize : -1,
    seen: !!(item && item.seen),
    fav: !!(item && item.favorite),
    current: !!(item && item.current)
  }
}

// True when less than `screens` viewport heights of content remain below the view.
function nearEnd(contentY, height, originY, contentHeight, screens) {
  return originY + contentHeight - (contentY + height) < height * screens
}
