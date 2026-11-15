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
    { key: 'landscape', label: '風景', tag: 711, selected: true, total: 3953 },
    { key: 'space', label: '太空', tag: 32, selected: false, total: null },
    { key: 'city', label: '城市', tag: 17, selected: true, total: null }
  ],
  favorites: { count: 2, selected: false }
};

test('parseResult accepts the envelope and rejects anything else', () => {
  assert.equal(M.parseResult('{"ok":true,"x":1}').x, 1);
  assert.equal(M.parseResult('{"ok":false,"error":{"code":"timeout","message":"逾時"}}').error.message, '逾時');
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
  assert.equal(M.sourceLabel(status, 'favs'), '收藏');
  assert.equal(M.sourceLabel(status, 'space'), '太空');
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
  assert.equal(M.timerText({ timer: { enabled: true, minutes: 90 } }), '每 90 分鐘');
  assert.equal(M.timerText({ timer: { enabled: false, minutes: 90 } }), '已關閉');
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
  assert.match(M.banner('ok', { ...status, version: '0.0.9' }, '0.1.0').text, /不一致/);
  assert.equal(M.banner('ok', status, '0.1.0'), null);
  const limited = { ...status, status: { lastError: { code: 'rate_limited', message: '已達上限' } } };
  assert.deepEqual({ ...M.banner('ok', limited, '0.1.0') }, { tone: 'warn', text: '已達上限' });
});

test('markup-shaped CLI text passes through as plain data', () => {
  // QML renders these with Text.PlainText; the model must not interpret or drop them.
  const evil = { ...status, status: { lastError: { code: 'http', message: '<img src="https://example.invalid/t">' } } };
  assert.equal(M.banner('ok', evil, '0.1.0').text, '<img src="https://example.invalid/t">');
  assert.equal(M.sourceLabel({ sources: [{ key: 'x', label: '<b>x</b>' }] }, 'x'), '<b>x</b>');
});
