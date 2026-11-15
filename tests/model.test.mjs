import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const context = vm.createContext({});
const source = fs.readFileSync(new URL('../Model.js', import.meta.url), 'utf8');
vm.runInContext(source.replace(/^\.pragma library\s*/, ''), context);
const M = context;

const status = {
  ok: true,
  version: '0.1.0',
  status: { current: null, lastError: null },
  settings: { styles: { general: true, anime: false }, color: 'none', timer: { enabled: false, minutes: 60 } },
  sources: [
    { key: 'landscape', label: 'Landscape', tag: 711, selected: true, total: 3953 },
    { key: 'space', label: 'Space', tag: 32, selected: false, total: null },
    { key: 'city', label: 'City', tag: 17, selected: true, total: null }
  ],
  favorites: { count: 2, selected: false }
};

test('parseResult accepts the envelope and rejects anything else', () => {
  assert.equal(M.parseResult('{"ok":true,"x":1}').x, 1);
  assert.equal(M.parseResult('{"ok":false,"error":{"code":"timeout","message":"Timed out"}}').error.message, 'Timed out');
  assert.equal(M.parseResult('').ok, false);
  assert.equal(M.parseResult('not json').error.code, 'invalid_response');
  assert.equal(M.parseResult('{"ok":false}').error.code, 'invalid_response');
  assert.equal(M.parseResult('[1,2]').ok, false);
});

test('formatting helpers keep invalid input harmless', () => {
  assert.equal(M.formatCount(11588), '11,588');
  assert.equal(M.formatCount(null), '—');
  assert.equal(M.formatSize(8388608), '8.0 MB');
  assert.equal(M.formatResolution('3840x2160'), '3840×2160');
  assert.equal(M.formatResolution('<b>x</b>'), '');
});

test('only Wallhaven wallpaper pages and absolute file paths become URLs', () => {
  assert.equal(M.wallpaperUrl('285e6x'), 'https://wallhaven.cc/w/285e6x');
  assert.equal(M.wallpaperUrl('../evil'), '');
  assert.equal(M.wallpaperUrl('<img src=x>'), '');
  assert.equal(M.fileUrl('/home/a b/x#1.jpg'), 'file:///home/a%20b/x%231.jpg');
  assert.equal(M.fileUrl('relative.jpg'), '');
});

test('source toggles produce the comma list the CLI expects', () => {
  assert.equal(M.selectedSources(status).join(','), 'landscape,city');
  assert.equal(M.toggledSources(status, 'space'), 'landscape,city,space');
  assert.equal(M.toggledSources(status, 'landscape'), 'city');
  assert.equal(M.toggledSources(status, 'favs'), 'landscape,city,favs');
  assert.equal(M.allSources(status), 'favs,landscape,space,city');
  assert.equal(M.sourceLabel(status, 'favs'), 'Favorites');
  assert.equal(M.sourceLabel(status, 'space'), 'Space');
  assert.equal(M.sourceLabel(status, null), 'Wallhaven');
});

test('pool size appears only when every selected total is known', () => {
  assert.equal(M.poolSize(status), null);
  const known = { ...status, sources: status.sources.map((s) => ({ ...s, total: s.total ?? 100 })) };
  assert.equal(M.poolSize(known), 4053);
  assert.equal(M.poolSize({ ...known, favorites: { count: 2, selected: true } }), 4055);
  assert.equal(M.poolSize({ sources: [], favorites: { count: 3, selected: true } }), 3);
  assert.equal(M.poolSize({ sources: [], favorites: { count: 3, selected: false } }), null);
});

test('timer minutes are clamped to 5–1440', () => {
  assert.equal(M.clampMinutes(3), 5);
  assert.equal(M.clampMinutes(2000), 1440);
  assert.equal(M.clampMinutes('90'), 90);
  assert.equal(M.clampMinutes('abc'), 60);
  assert.equal(M.timerText({ timer: { enabled: true, minutes: 90 } }), 'Every 90 min');
  assert.equal(M.timerText({ timer: { enabled: false, minutes: 90 } }), 'Off');
});

test('favorite ids and browse tabs come from CLI data only', () => {
  const ids = M.favoriteIds('{"items":[{"id":"285e6x"},{"id":"../x"},{"id":"abc123"}]}');
  assert.deepEqual(Object.keys(ids).sort(), ['285e6x', 'abc123']);
  assert.deepEqual({ ...M.favoriteIds('nope') }, {});
  const tabs = M.browseTabs(status);
  assert.equal(tabs.length, 4);
  assert.equal(tabs[3].key, 'favs');
  assert.equal(M.firstBrowseKey(status), 'landscape');
  assert.equal(M.firstBrowseKey({ sources: [], favorites: { selected: false } }), 'favs');
});

test('timer settings come only from a well-formed settings.json', () => {
  assert.deepEqual({ ...M.timerSettings('{"timer":{"enabled":true,"minutes":2}}') }, { enabled: true, minutes: 5 });
  assert.deepEqual({ ...M.timerSettings('{"timer":{"enabled":"yes","minutes":30}}') }, { enabled: false, minutes: 60 });
  assert.deepEqual({ ...M.timerSettings('garbage') }, { enabled: false, minutes: 60 });
});

test('banner prefers install problems, then version mismatch, then last error', () => {
  assert.equal(M.banner('missing', null, '0.1.0').tone, 'error');
  assert.match(M.banner('ok', { ...status, version: '0.0.9' }, '0.1.0').text, /does not match/);
  assert.equal(M.banner('ok', status, '0.1.0'), null);
  const limited = { ...status, status: { lastError: { code: 'rate_limited', message: 'Rate limited' } } };
  assert.deepEqual({ ...M.banner('ok', limited, '0.1.0') }, { tone: 'warn', text: 'Rate limited' });
  const down = { ...status, status: { lastError: { code: 'unavailable', message: 'Down' } } };
  assert.equal(M.banner('ok', down, '0.1.0').tone, 'warn');
  const broken = { ...status, status: { lastError: { code: 'set_failed', message: 'x' } } };
  assert.equal(M.banner('ok', broken, '0.1.0').tone, 'error');
});

test('markup-shaped CLI text passes through as plain data', () => {
  // QML renders these with Text.PlainText; the model must not interpret or drop them.
  const evil = { ...status, status: { lastError: { code: 'http', message: '<img src="https://example.invalid/t">' } } };
  assert.equal(M.banner('ok', evil, '0.1.0').text, '<img src="https://example.invalid/t">');
  assert.equal(M.sourceLabel({ sources: [{ key: 'x', label: '<b>x</b>' }] }, 'x'), '<b>x</b>');
});

test('sync control: progress while running, a check when done, resume otherwise', () => {
  assert.equal(M.syncView(null, null, 'favs', status), null);
  const idle = M.syncView(null, null, 'landscape', status);
  assert.deepEqual([idle.icon, idle.text, idle.tooltip, idle.action], [M.Icons.sync, '', 'Sync all Landscape thumbnails', 'start']);
  const running = { source: 'landscape', state: 'running', done: 1234, total: 4071, skipped: 0, updatedAt: 1794700000 };
  const here = M.syncView(running, running, 'landscape', status);
  assert.deepEqual([here.text, here.action, here.spinning, here.tooltip], ['Syncing 1,234 / 4,071', 'cancel', true, 'Cancel sync']);
  assert.equal(M.syncView(running, null, 'city', status).text, 'Syncing Landscape 1,234 / 4,071');
  assert.equal(M.syncView({ ...running, total: null, done: 0 }, null, 'landscape', status).text, 'Syncing 0 / …');
  // Finished: a check, no action, and the date it finished.
  const done = M.syncView(null, { ...running, state: 'done' }, 'landscape', status);
  assert.deepEqual([done.icon, done.text, done.action], [M.Icons.synced, '', '']);
  assert.match(done.tooltip, /^1,234 thumbnails synced on \d{4}-\d{2}-\d{2}$/);
  const stopped = M.syncView(null, { ...running, state: 'stopped' }, 'landscape', status);
  assert.deepEqual([stopped.text, stopped.action, stopped.tooltip], ['1,234 / 4,071', 'start', 'Sync stopped · click to resume']);
  const failed = M.syncView(null, { ...running, state: 'failed', error: { code: 'network', message: 'No connection' } }, 'landscape', status);
  assert.deepEqual([failed.icon, failed.tone, failed.tooltip], [M.Icons.syncAlert, 'error', 'Sync failed: No connection · click to resume']);
});

test('sync records come from the source entry for the current filters', () => {
  const withSync = { ...status, sources: [{ ...status.sources[0], sync: { state: 'done', done: 3 } }, status.sources[1]] };
  assert.equal(M.sourceSync(withSync, 'landscape').state, 'done');
  assert.equal(M.sourceSync(withSync, 'space'), null);
  assert.equal(M.sourceSync(null, 'landscape'), null);
  assert.equal(M.formatDate(null), '');
});

test('browse rows are flat and typed even for missing fields', () => {
  assert.deepEqual({ ...M.browseRow({ id: 'abc', thumb: null, resolution: '3840x2160', favorites: 5, fileSize: 10, seen: true }) },
    { wid: 'abc', thumb: '', resolution: '3840x2160', favs: 5, size: 10, seen: true, fav: false, current: false });
  assert.equal(M.browseRow(null).favs, -1);
});

test('wheel steps grow with spin speed and reset after a pause', () => {
  assert.equal(M.wheelSpeed(0, -120, 1000).boost, 1);            // first notch after a pause
  assert.equal(M.wheelSpeed(0, -120, 250).boost, 1);             // 4 notches/s: normal
  let s = { avg: 0 };
  for (let i = 0; i < 6; i++) s = M.wheelSpeed(s.avg, -120, 30); // 33 notches/s: fast spin
  assert.equal(s.boost, 4);
  assert.ok(M.wheelSpeed(0, -120, 80).boost > 1 && M.wheelSpeed(0, -120, 80).boost < 2);
  // A high-resolution wheel sends small deltas often; speed is the same as whole notches.
  assert.equal(M.wheelSpeed(0, -15, 31.25).boost, 1);
  assert.equal(M.wheelSpeed(s.avg, -120, 400).boost, 1);         // pause resets
});

test('pages load where the grid is, then below, then above', () => {
  // 5 columns, 200 px rows, 600 px view: 4 rows (20 cards) on screen.
  const at = (contentY, loaded) => M.nextPage(contentY, 0, 600, 5, 200, 4991, 208, loaded);
  assert.equal(M.nextPage(0, 0, 600, 5, 200, 0, 0, {}), 1);           // nothing yet
  assert.equal(M.nextPage(0, 0, 600, 5, 200, 0, 0, { 1: true }), 0);  // first page pending total
  assert.equal(at(0, {}), 1);
  assert.equal(at(0, { 1: true }), 2);                                // 1.5 screens below reach card 49,
  assert.equal(at(0, { 1: true, 2: true }), 3);                       // which is on page 3
  assert.equal(at(0, { 1: true, 2: true, 3: true }), 0);
  // Dragged to the middle: row 400 starts at card 2000, which is on page 84.
  assert.equal(M.pageOf(2000), 84);
  assert.equal(at(80000, { 1: true }), 84);
  assert.equal(at(80000, { 84: true }), 85);
  assert.equal(at(80000, { 84: true, 85: true }), 86);                // below first,
  assert.equal(at(80000, { 84: true, 85: true, 86: true }), 83);      // then one screen above
  assert.equal(at(80000, { 83: true, 84: true, 85: true, 86: true }), 0);
  // Never past the last page.
  assert.equal(M.nextPage(1e9, 0, 600, 5, 200, 4991, 208, {}), 208);
});

test('keyboard moves the selection across the grid', () => {
  // 5 columns, 23 cards: the last row holds cards 20–22; 3 rows fit on screen.
  const m = (i, move) => M.moveIndex(i, move, 5, 23, 3);
  assert.equal(m(0, 'left'), 0);
  assert.equal(m(5, 'left'), 4);                // wraps to the previous row
  assert.equal(m(4, 'right'), 5);
  assert.equal(m(22, 'right'), 22);
  assert.equal(m(2, 'up'), 2);
  assert.equal(m(7, 'up'), 2);
  assert.equal(m(7, 'down'), 12);
  assert.equal(m(18, 'down'), 22);              // no card below: the last one
  assert.equal(m(21, 'down'), 21);              // already on the last row
  assert.equal(m(1, 'pageDown'), 16);
  assert.equal(m(20, 'pageDown'), 22);
  assert.equal(m(16, 'pageUp'), 1);
  assert.equal(m(9, 'home'), 0);
  assert.equal(m(9, 'end'), 22);
  assert.equal(M.moveIndex(3, 'down', 5, 0, 3), 0);   // nothing loaded yet
});
