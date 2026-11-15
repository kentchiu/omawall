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
| Favorites | `~/Pictures/Wallpapers/wallhaven/wallhaven-<id>.<jpg|png>` |

Wallhaven ids match `^[a-z0-9]{1,16}$`; anything else is rejected.

## Config (`config.json`, optional)

```json
{ "sources": [ { "key": "landscape", "label": "風景", "tag": 711 } ] }
```

Missing file → built-in defaults (in this order):

| key | label | tag |
|---|---|---|
| landscape | 風景 | 711 |
| nature | 自然 | 37 |
| space | 太空 | 32 |
| city | 城市 | 17 |
| mountains | 山岳 | 328 |
| architecture | 建築 | 989 |
| minimalism | 極簡 | 2278 |
| abstract | 抽象 | 74 |
| digital-art | 數位藝術 | 479 |
| cars | 汽車 | 314 |

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
    `all` every id ever applied (for "看過" badges, never reset).
- `disliked.json` — `{ "version": 1, "ids": ["..."] }`
- `favorites.json` — `{ "version": 1, "items": [ { "id", "path", "thumb", "resolution", "favorites", "fileSize", "addedAt" } ] }` newest first.
- `status.json` — rewritten by every command that changes something:
  ```json
  { "version": 1,
    "current": { "id": "285e6x", "source": "landscape", "path": "/abs/full/wallhaven-285e6x.jpg",
                 "thumb": "/abs/thumbs/285e6x.jpg", "resolution": "3840x2160", "favorites": 2145,
                 "fileSize": 8388608, "url": "https://wallhaven.cc/w/285e6x", "appliedAt": 1794700000 },
    "lastError": { "code": "timeout", "message": "無法連線 Wallhaven（逾時）", "trigger": "timer", "at": 1794700000 },
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
  `id, path, thumbs.large, resolution, favorites, file_size, file_type`.
- Full image: must be `https://w.wallhaven.cc/full/...`. Thumbnail: must be `https://th.wallhaven.cc/lg/...`.
  Only HTTPS to `wallhaven.cc`, `w.wallhaven.cc`, `th.wallhaven.cc`; never follow redirects elsewhere.
- Limits: API/thumbnail timeout 15 s, full image 90 s; full ≤ 30 MiB, thumbnail ≤ 1 MiB.
  Check magic bytes: JPEG `FF D8 FF`, PNG `89 50 4E 47 0D 0A 1A 0A`; extension comes from the bytes.
- `/api/v1/w/<id>` for `apply` of an id with no cached metadata.

## Commands

Every command prints exactly one JSON object on stdout (except `--version`). Success: `{"ok": true, ...}`, exit 0.
Failure: `{"ok": false, "error": {"code": "...", "message": "繁體中文訊息"}}`, exit 1 (usage errors exit 2).

Error codes: `usage`, `config`, `network`, `timeout`, `rate_limited` (HTTP 429), `http`, `invalid_response`,
`invalid_image`, `too_large`, `no_candidates`, `busy`, `not_found`, `io`, `set_failed`.

| Command | Network | Result |
|---|---|---|
| `omawall --version` | no | prints `omawall <semver>` (same as `manifest.json` version) |
| `omawall status` | no | `{ok, version, status, settings, sources: [{key, label, tag, selected, total}], favorites: {count, selected}}` — `total` from `status.totals` for the current filters, else `null` |
| `omawall next [--trigger manual\|timer]` | yes | applies the next wallpaper; `{ok, current}` |
| `omawall apply <id> [--trigger manual]` | yes | downloads and applies that id; `{ok, current}` |
| `omawall dislike` | yes | excludes current, removes it from favorites, then behaves like `next --trigger manual` |
| `omawall fav` | no* | toggles favorite for current; `{ok, favorite: bool, current}` |
| `omawall browse <sourceKey\|favs> [--page N]` | yes | `{ok, source, page, lastPage, total, items: [{id, thumb, resolution, favorites, fileSize, seen, disliked, favorite, current}]}`, 24 per page, thumbnails downloaded first |
| `omawall set <name> <value>` | no | `{ok, settings}`; names: `sources` (comma list, may be empty, may include `favs`), `style.general`, `style.anime` (`true`/`false`), `color` (`none`/`theme`), `timer.enabled` (`true`/`false`), `timer.minutes` (integer 5–1440) |

\* `fav` may download the current thumbnail if missing.

### `next` selection

1. Candidate sources = selected sources in random order (uniform). `favs` counts only when a favorite other than the current exists.
2. Tag source: start at `queries[queryKey].page`; fetch the page; pick the first item that is not in that query's `ids`,
   not disliked and not current. None → page + 1 (at most 3 page fetches per call). Past `last_page` → reset that query
   (`page` 1, `ids` []) once and retry. Record the id in the query's `ids` and in `all`.
3. `favs`: first favorite (newest first) not in `seen.favs`, not current; exhausted → clear `seen.favs` once. Uses the local file, no download.
4. Nothing anywhere → `no_candidates`.
5. Apply: download the full image (skip if cached), run `omarchy-theme-bg-set <abs path>` (argv, no shell); non-zero exit → `set_failed`.
6. Update `status.json` (`current`, clear `lastError`, store `totals[queryKey]` from `meta.total`).

### Side rules

- One process at a time: exclusive lock on `<state>/lock`; wait up to 10 s, then `busy`.
- On any failure: keep the current wallpaper, write `lastError` (with `trigger`). When `trigger` is `manual`
  also run `omarchy-notification-send --app-name omawall "換桌布失敗" "<message>"`. `browse`, `fav`, `set` never notify.
- Favorite and dislike are exclusive: `fav` on a disliked id removes it from `disliked`; `dislike` on a favorite deletes
  the favorite copy and its entry. Only delete files matching `wallhaven-<id>.<ext>` inside the favorites dir.
  When that copy is the current wallpaper, copy it to `full/` and rerun `omarchy-theme-bg-set` on the copy first;
  if that fails, delete nothing.
- Cache: keep the 20 newest files in `full/` (never the current one); keep `thumbs/` ≤ 2 GiB, oldest first by mtime.
- Never run downloaded content; never pass untrusted strings through a shell.
