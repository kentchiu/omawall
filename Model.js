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
  synced: "\uDB81\uDDE1",      // md-check_circle_outline
  folder: "\uDB80\uDE4F"       // md-folder_image
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
  // Wallhaven being slow, busy or down is a warning, not something to fix here.
  var outage = ["rate_limited", "unavailable", "timeout", "network"].indexOf(err.code) !== -1
  return { tone: outage ? "warn" : "error", text: err.message }
}

// Local date of a Unix time in seconds, as YYYY-MM-DD.
function formatDate(seconds) {
  if (typeof seconds !== "number" || !isFinite(seconds) || seconds <= 0) return ""
  var d = new Date(seconds * 1000)
  var pad = function(n) { return (n < 10 ? "0" : "") + n }
  return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate())
}

// The sync record of a source for the current filters, or null.
function sourceSync(status, key) {
  var sources = status && status.sources ? status.sources : []
  for (var i = 0; i < sources.length; i++) if (sources[i].key === key) return sources[i].sync || null
  return null
}

// Sync control next to the page counter in the browse view, for a tag source.
// `running` is the sync in progress anywhere (only one runs at a time, so it offers Cancel on
// every tab); `record` is this source's own result. A finished source has no action: its new
// wallpapers get thumbnails when browsed.
function syncView(running, record, key, status) {
  if (!key || key === FAVS) return null
  var label = function(k) { return sourceLabel(status, k) || k }
  var progress = function(r) { return formatCount(r.done) + " / " + (typeof r.total === "number" ? formatCount(r.total) : "…") }
  var view = { icon: Icons.sync, text: "", tooltip: "Sync all " + label(key) + " thumbnails", action: "start", tone: "normal", spinning: false }
  if (running) {
    var here = running.source === key
    view.text = "Syncing " + (here ? "" : label(running.source) + " ") + progress(running)
    view.tooltip = here ? "Cancel sync" : "Cancel " + label(running.source) + " sync"
    view.action = "cancel"
    view.spinning = true
  } else if (record && record.state === "done") {
    view.icon = Icons.synced
    view.tooltip = formatCount(record.done) + " thumbnails synced" + (formatDate(record.updatedAt) ? " on " + formatDate(record.updatedAt) : "")
    view.action = ""
  } else if (record && record.state === "failed") {
    view.icon = Icons.syncAlert
    view.tone = "error"
    view.text = progress(record)
    view.tooltip = "Sync failed: " + (record.error && typeof record.error.message === "string" ? record.error.message : "unknown error") + " · click to resume"
  } else if (record && (record.state === "canceled" || record.state === "stopped")) {
    view.text = progress(record)
    view.tooltip = "Sync stopped · click to resume"
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

// Wheel acceleration: notches per second, averaged over recent events, sets a 1–4× step.
// A pause of more than 300 ms starts slow again.
function wheelSpeed(prevAvg, angle, dtMs) {
  if (!(dtMs > 0) || dtMs > 300) return { avg: 0, boost: 1 }
  var perSecond = Math.abs(angle) / 120 / (dtMs / 1000)
  var avg = prevAvg > 0 ? (prevAvg + perSecond) / 2 : perSecond
  return { avg: avg, boost: Math.max(1, Math.min(4, avg / 8)) }
}

// Wallhaven and the CLI both return 24 wallpapers per page.
var PER_PAGE = 24

function pageOf(index) { return Math.floor(index / PER_PAGE) + 1 }

// The browse grid holds a card for every wallpaper of the source; pages fill in as needed.
// Next page to fetch: the pages on screen, then 1.5 screens below, then one screen above.
// 0 when they are all loaded. Before the first page arrives, `count` is 0 and page 1 is next.
function nextPage(contentY, originY, height, cols, cellHeight, count, lastPage, loaded) {
  if (count <= 0) return loaded[1] ? 0 : 1
  cols = Math.max(1, cols)
  var firstRow = Math.floor(Math.max(0, contentY - originY) / cellHeight)
  var rows = Math.ceil(height / cellHeight) + 1
  var first = Math.min(count - 1, firstRow * cols)
  var last = Math.min(count - 1, (firstRow + rows) * cols - 1)
  var below = Math.min(count - 1, last + Math.ceil(rows * 1.5) * cols)
  var above = Math.max(0, first - rows * cols)
  var order = []
  for (var p = pageOf(first); p <= pageOf(below); p++) order.push(p)
  for (p = pageOf(first) - 1; p >= pageOf(above); p--) order.push(p)
  for (var i = 0; i < order.length; i++)
    if (order[i] >= 1 && order[i] <= lastPage && !loaded[order[i]]) return order[i]
  return 0
}

// Keyboard selection in the browse grid. `move` is left, right, up, down, pageUp, pageDown,
// home or end; `pageRows` is how many rows fit on screen. Left and right run across rows;
// down from the last full row lands on the last card.
function moveIndex(index, move, cols, count, pageRows) {
  if (count <= 0) return 0
  cols = Math.max(1, cols)
  var i = Math.max(0, Math.min(count - 1, index))
  var page = Math.max(1, pageRows) * cols
  var lastRow = Math.floor((count - 1) / cols)
  switch (move) {
  case "left": return Math.max(0, i - 1)
  case "right": return Math.min(count - 1, i + 1)
  case "up": return i - cols >= 0 ? i - cols : i
  case "down": return i + cols < count ? i + cols : (Math.floor(i / cols) < lastRow ? count - 1 : i)
  case "pageUp": return Math.max(0, i - page)
  case "pageDown": return Math.min(count - 1, i + page)
  case "home": return 0
  case "end": return count - 1
  }
  return i
}
