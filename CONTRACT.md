# omawall CLI contract

The Omarchy shell plugin (repo root, QML) talks to the `omawall` CLI (`cli/`, Rust) only through
the commands, files and JSON shapes below. Change this file first when the interface changes.

## Paths

| What | Path |
|---|---|
| Binary | `~/.local/bin/omawall` (callers use the absolute path) |
| User config (optional) | `$XDG_CONFIG_HOME/omawall/config.json` (default `~/.config/omawall/config.json`) |
| State dir | `$XDG_STATE_HOME/omawall/` (default `~/.local/state/omawall/`) |
| Cache dir | `$XDG_CACHE_HOME/omawall/` (default `~/.cache/omawall/`) |
| Full images | `<cache>/full/wallhaven-<id>.<jpg|png>` |
| Thumbnails | `<cache>/thumbs/<id>.jpg` (or `.png`, by detected type) |
| Saved pages | `<cache>/pages/<query key, non-alphanumerics as _>/<page>.json` |
| Favorites | `~/Pictures/Wallpapers/wallhaven/wallhaven-<id>.<jpg|png>` |

Wallhaven ids match `^[a-z0-9]{1,16}$`; anything else is rejected.

## Config (`config.json`, optional)

```json
{ "sources": [ { "key": "landscape", "label": "Landscape", "tag": 711 } ] }
```

Missing file → built-in defaults (in this order):

| key | label | tag |
|---|---|---|
| landscape | Landscape | 711 |
| nature | Nature | 37 |
| space | Space | 32 |
| city | City | 17 |
| mountains | Mountains | 328 |
| architecture | Architecture | 989 |
| minimalism | Minimalism | 2278 |
| abstract | Abstract | 74 |
| digital-art | Digital Art | 479 |
| cars | Cars | 314 |

`key` matches `^[a-z0-9-]{1,32}$`, must be unique and must not be `favs` (reserved for favorites).
An invalid config is an error (`config`), never silently replaced.

## State files (all written atomically: temp file + rename)

- `settings.json` — user choices; defaults when missing (`sources` = every configured source key):
  ```json
  { "version": 1, "sources": ["landscape","nature","space","city","mountains","architecture","minimalism","abstract","digital-art","cars"],
    "styles": { "general": true, "anime": false }, "color": "none",
    "timer": { "enabled": false, "minutes": 60 } }
  ```
  `sources` may include `favs`. Unknown keys in `sources` are dropped on read.
- `seen.json` — `{ "version": 1, "queries": { "<queryKey>": { "page": 1, "ids": ["..."] } }, "favs": ["..."], "all": ["..."] }`
  - `queries` holds the selection cursor per query key; `favs` the ids already shown from favorites;
    `all` every id ever applied (for "Seen" badges, never reset).
- `disliked.json` from 0.1.x is no longer read or written; it may stay on disk.
- `favorites.json` — `{ "version": 1, "items": [ { "id", "path", "thumb", "resolution", "favorites", "fileSize", "addedAt" } ] }` newest first.
- `status.json` — rewritten by every command that changes something:
  ```json
  { "version": 1,
    "current": { "id": "285e6x", "source": "landscape", "path": "/abs/full/wallhaven-285e6x.jpg",
                 "thumb": "/abs/thumbs/285e6x.jpg", "resolution": "3840x2160", "favorites": 2145,
                 "fileSize": 8388608, "url": "https://wallhaven.cc/w/285e6x", "appliedAt": 1794700000 },
    "lastError": { "code": "timeout", "message": "Wallhaven timed out", "trigger": "timer", "at": 1794700000 },
    "totals": { "<queryKey>": 3953 },
    "updatedAt": 1794700000 }
  ```
  `current` and `lastError` may be `null`. `current.source` is `null` after `apply` of a non-favorite id;
  `current.thumb` is `null` when its thumbnail could not be fetched. A successful `next`/`apply` clears `lastError`.
  Times are Unix seconds.

`queryKey` = `tag:<tag>|cat:<categories>|color:<hex or none>`, e.g. `tag:711|cat:100|color:none`.

## Wallhaven queries

`GET https://wallhaven.cc/api/v1/search?q=id:<tag>&categories=<c>&purity=100&atleast=3840x2160&ratios=16x9&sorting=favorites&order=desc&page=<n>[&colors=<hex>]`

- `categories`: styles general only `100`, anime only `010`, both `110`. At least one style stays on.
- `colors`: only when `color` is `theme`: read `accent = "#rrggbb"` from
  `$XDG_STATE_HOME/omarchy/current/theme/colors.toml` and use the nearest (squared RGB distance) of:
  `660000 990000 cc0000 cc3333 ea4c88 993399 663399 333399 0066cc 0099cc 66cccc 77cc33 669900 336600 666600 999900 cccc33 ffff00 ffcc33 ff9900 ff6600 cc6633 996633 663300 000000 999999 cccccc ffffff 424153`
  (`#89b4fa` → `66cccc`). Unreadable theme → treat as `none`.
- No API key. Response JSON limit 2 MiB. Use `meta.last_page`, `meta.total`, and per item
  `id, path, thumbs.large, resolution, favorites, file_size, file_type`, plus `category` and `colors` (kept in saved pages for local filtering; missing ones read as empty).
- Full image: must be `https://w.wallhaven.cc/full/...`. Thumbnail: must be `https://th.wallhaven.cc/lg/...`.
  Only HTTPS to `wallhaven.cc`, `w.wallhaven.cc`, `th.wallhaven.cc`; never follow redirects elsewhere.
- Limits: API/thumbnail timeout 15 s, full image 90 s; full ≤ 30 MiB, thumbnail ≤ 1 MiB.
  Check magic bytes: JPEG `FF D8 FF`, PNG `89 50 4E 47 0D 0A 1A 0A`; extension comes from the bytes.
- `/api/v1/w/<id>` for `apply` of an id with no cached metadata.
- Rate limit is 45 API requests per minute. `sync` spaces its API requests at least 2 s apart so
  `next` and `browse` keep headroom. Thumbnail downloads are not API requests.
- `browse` and `sync` download a page's thumbnails up to 6 at a time.
- HTTP 5xx (Wallhaven or Cloudflare in front of it is down) is `unavailable`, not `http`.
- Saved pages: every search page fetched by `next`, `browse` or `sync` is saved. `next`, `browse` and the
  metadata lookup of `apply` retry once after 2 s on `network`, `timeout`, `unavailable` or `rate_limited`;
  if it still fails that way, they use the saved page (`apply`: the saved entry with that id from any page)
  and fail only when there is none. The full image itself still comes from `w.wallhaven.cc`.

## Commands

Every command prints exactly one JSON object on stdout (except `--version`). Success: `{"ok": true, ...}`, exit 0.
Failure: `{"ok": false, "error": {"code": "...", "message": "English message"}}`, exit 1 (usage errors exit 2).

Error codes: `usage`, `config`, `network`, `timeout`, `rate_limited` (HTTP 429), `unavailable` (HTTP 5xx), `http`, `invalid_response`,
`invalid_image`, `too_large`, `no_candidates`, `busy`, `not_found`, `io`, `set_failed`.

| Command | Network | Result |
|---|---|---|
| `omawall --version` | no | prints `omawall <semver>` (same as `manifest.json` version) |
| `omawall status` | no | `{ok, version, status, settings, sources: [{key, label, tag, selected, total, sync}], favorites: {count, selected}, sync}` — `total` from `status.totals` for the current filters, else `null`; `sync` fields see below |
| `omawall next [--trigger manual\|timer]` | yes | applies the next wallpaper; `{ok, current}` |
| `omawall apply <id> [--trigger manual]` | yes | downloads and applies that id; `{ok, current}` |
| `omawall fav [<id>]` | no* | toggles favorite for `<id>` (default: current) without applying it; a non-current id is downloaded straight into the favorites dir; `{ok, id, favorite: bool, current}` |
| `omawall browse <sourceKey\|favs> [--page N]` | yes | `{ok, source, page, lastPage, total, items: [{id, thumb, resolution, favorites, fileSize, seen, favorite, current}], cached}`, 24 per page, thumbnails downloaded first; a source whose sync is `done` is filtered from that sync's saved pages without calling Wallhaven (style by `category`, color by `colors` containing `#<hex>`, an id met twice counted once); `cached` is true when the page came from disk because Wallhaven failed; a thumbnail that cannot be fetched is `null` (only `io` fails the page) |
| `omawall set <name> <value>` | no | `{ok, settings}`; names: `sources` (comma list, may be empty, may include `favs`), `style.general`, `style.anime` (`true`/`false`), `color` (`none`/`theme`), `timer.enabled` (`true`/`false`), `timer.minutes` (integer 5–1440) |

| `omawall sync <sourceKey>` | yes | long-running: downloads every thumbnail of that tag source (whole category, any filters); progress in `syncs.json`; prints `{ok, sync}` when it ends as `done` or `canceled`, the failure envelope when `failed` |
| `omawall sync --all` | yes | long-running: `sync` for every configured source in config order under one lock; finished ones return their record with no network; stops after a `canceled` one; prints `{ok, syncs: [record, ...]}` for the sources it reached, the failure envelope at the first `failed` one |
| `omawall sync --cancel` | no | asks a running sync to stop; `{ok, running: bool}` (whether a sync was running) |

\* `fav` may download the current thumbnail if missing; `fav <id>` downloads that wallpaper and its thumbnail.

### `sync`

The plugin starts `sync --all` detached (`Quickshell.execDetached`), so it outlives the panel and the browse view.
`sync --all` runs the steps below for each source in turn, keeps the 2 s API pacing across sources, and stops at
the first canceled or failed one; the next run resumes there, since finished sources are skipped.

1. `favs` or an unknown key → `usage` (checked first). Take `<state>/sync.lock` (wait up to 2 s; held → `busy`).
   Never hold `<state>/lock` while syncing, so `next`, `browse`, `fav` and `set` keep working. Delete a stale
   `<state>/sync.cancel`. `usage`, `busy` and `config` errors write no `syncs.json`.
2. Query the whole category, `categories=110` and no color, whatever the settings: every filtered view is a
   subset, so changing styles or color never makes a finished sync stale. Each category has one record:
   - `done` with `filters` → print it and stop, with no network: a finished query is never walked again.
     Wallpapers added later do not show in `browse` of that source.
   - `done` without `filters` (written before 0.3.4, its saved pages lack `category` and `colors`) → start
     over at page 1.
   - `canceled`, `failed` or a dead `running` → resume at its `page` with `done = pageDone`,
     `skipped = pageSkipped`, so that page is walked again and counted once.
   - none → start at page 1.
   Walk pages to `last_page`.
3. Per page: download its thumbnails up to 6 at a time; one already in `thumbs/` (any extension) is counted
   without a download. A failed thumbnail (`http`, `invalid_image`, `too_large`, `network`, `timeout`) counts as
   `skipped` and the sync goes on. Any other thumbnail error (`rate_limited`, `io`) ends it as `failed`.
4. API page errors: `rate_limited` → wait 60 s and retry the same page, at most 3 times; `network`/`timeout` →
   retry once after 5 s; anything still failing → `state: "failed"` with `error`.
5. Before each page, if `<state>/sync.cancel` exists → delete it, `state: "canceled"`, stop (the page in flight
   finishes first).
6. `syncs.json` (atomic, written only by the sync holding the lock; other queries' records are kept) is
   written when the sync starts, at every page boundary and when it ends:
   ```json
   { "version": 1, "queries": { "tag:711|cat:100|color:none": {
       "source": "landscape", "queryKey": "tag:711|cat:100|color:none", "state": "running",
       "done": 120, "skipped": 0, "total": 4071, "page": 6, "lastPage": 170, "pageDone": 120, "pageSkipped": 0,
       "error": null, "startedAt": 1794700000, "updatedAt": 1794700000, "filters": true } } }
   ```
   `state`: `running`, `done`, `canceled`, `failed`. `done` counts thumbnails now on disk (downloaded or already there).
   `page` is the page being walked (1 before the first page arrives); `pageDone`/`pageSkipped` are the counts
   before it. `total` and `lastPage` are `null` until the first page arrives. `error` is `{code, message}` or
   `null`. `filters` is true when the saved pages carry `category` and `colors`. At the end the thumbnail cache is trimmed like `browse`. `sync.json` from 0.2.0 is not read.
7. `sync` saves every page it walks, never notifies and never touches `status.json`, `seen.json` or `favorites.json`.

`status.sources[].sync` is that category's record (the whole category, so it does not depend on the filters), or `null`. `status.sync` is the record
being synced right now, or `null`. A `running` record while `<state>/sync.lock` is free (the process died) is
reported as `state: "stopped"`, and so is a `done` record without `filters`.
`sync --cancel` creates `<state>/sync.cancel` only when `sync.lock` is held.

### `next` selection

1. Candidate sources = selected sources in random order (uniform). `favs` counts only when a favorite other than the current exists.
2. Tag source: start at `queries[queryKey].page`; fetch the page; pick the first item that is not in that query's `ids`,
   not in `all` (until that query resets) and not current. None → page + 1 (at most 3 page fetches per call). Past `last_page` → reset that query
   (`page` 1, `ids` []) once and retry. Record the id in the query's `ids` and in `all`.
3. `favs`: first favorite (newest first) not in `seen.favs`, not current; exhausted → clear `seen.favs` once. Uses the local file, no download.
4. Nothing anywhere → `no_candidates`.
5. Apply: download the full image (skip if cached), run `omarchy-theme-bg-set <abs path>` (argv, no shell); non-zero exit → `set_failed`.
6. Update `status.json` (`current`, clear `lastError`, store `totals[queryKey]` from `meta.total`).

### Side rules

- One process at a time (except `sync`, see above): exclusive lock on `<state>/lock`; wait up to 10 s, then `busy`.
- On any failure: keep the current wallpaper, write `lastError` (with `trigger`). When `trigger` is `manual`
  also run `omarchy-notification-send --app-name omawall "Wallpaper change failed" "<message>"`. `browse`, `fav`, `set`, `sync` never notify.
- Unfavoriting deletes the favorite copy and its entry. Only delete files matching `wallhaven-<id>.<ext>` inside the
  favorites dir. When that copy is the current wallpaper, copy it to `full/` and rerun `omarchy-theme-bg-set` on the
  copy first; if that fails, delete nothing.
- All user-facing text (error messages, notifications, labels) is English.
- Cache: keep the 20 newest files in `full/` (never the current one); keep `thumbs/` ≤ 2 GiB, oldest first by mtime.
- Never run downloaded content; never pass untrusted strings through a shell.
