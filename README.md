# omawall

Wallhaven wallpapers for [Omarchy](https://omarchy.org). A bar widget that rotates SFW, 4K 16:9
wallpapers from [Wallhaven](https://wallhaven.cc) tags, with favorites, a browse view and an
optional timer. No API key is needed.

## Features

- Left-click the bar icon for the next wallpaper; right-click opens the panel.
- Pick any mix of sources: 10 Wallhaven tags (landscape, nature, space, city, mountains,
  architecture, minimalism, abstract, digital art, cars) plus your local favorites.
- Filter by style (General / Anime) and optionally by the current theme's accent color.
- Browse every source in a grid and click a thumbnail to apply it. The next page loads in the
  background while you look at the current one.
- Sync all thumbnails of a source to disk with the sync icon next to the page counter in the browse view,
  so its pages no longer wait for downloads. It runs in the background; click the icon again to cancel.
  A finished source keeps a check mark and is not synced again; a stopped one resumes where it left off.
- Open the thumbnails folder in your file manager with the folder icon next to it.
- Favorite a wallpaper to keep a copy in `~/Pictures/Wallpapers/wallhaven/`.
- An applied wallpaper is not picked again until its source has been shown in full.
- Change the wallpaper on a timer (5–1440 minutes).

## Requirements

- Omarchy 4 (the shell plugin system and `omarchy-theme-bg-set`)
- Rust toolchain (`cargo`) and `jq`, to build the CLI

## Install

1. Add the plugin:
   ```sh
   omarchy plugin add https://github.com/kentchiu/omawall --enable
   ```
2. Build and install the CLI to `~/.local/bin/omawall`:
   ```sh
   ~/.config/omarchy/plugins/kent.omawall/scripts/install-cli
   ```
3. Make sure `omarchy.background` is not listed in `disabledPlugins` in `~/.config/omarchy/shell.json`;
   it draws still wallpapers.
4. Optional: add a key binding, e.g. in `~/.config/hypr/bindings.lua`:
   ```lua
   o.bind("SUPER + ALT + W", "Next wallpaper (omawall)", "~/.local/bin/omawall next")
   ```

## Update

```sh
omarchy plugin update kent.omawall
~/.config/omarchy/plugins/kent.omawall/scripts/install-cli
```

The widget shows a warning when the CLI and plugin versions differ.

## Uninstall

```sh
omarchy plugin remove kent.omawall
rm ~/.local/bin/omawall
rm -rf ~/.local/state/omawall ~/.cache/omawall   # optional
```

Favorites stay in `~/Pictures/Wallpapers/wallhaven/`.

## Custom sources

Create `~/.config/omawall/config.json` to replace the built-in tags. Find a tag id in the URL of a
Wallhaven tag page (`https://wallhaven.cc/tag/711` → `711`).

```json
{ "sources": [ { "key": "landscape", "label": "Landscape", "tag": 711 } ] }
```

## Files

| What | Where |
|---|---|
| Config (optional) | `~/.config/omawall/config.json` |
| State | `~/.local/state/omawall/` |
| Cache | `~/.cache/omawall/` (20 full images, thumbnails up to 2 GiB) |
| Favorites | `~/Pictures/Wallpapers/wallhaven/` |

## How it works

The repo root is the Omarchy shell plugin (`manifest.json`, `Widget.qml`, `Service.qml`, `Model.js`).
`cli/` is a Rust CLI that does all network access, download checks, state and caching; the plugin
only calls it. [CONTRACT.md](CONTRACT.md) describes the interface between them.

## Development

- Plugin logic: `node --test`
- CLI: `cd cli && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- Manifest: `omarchy plugin validate .`

## License

[MIT](LICENSE)
