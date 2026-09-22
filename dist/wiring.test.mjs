// Headless guard for the DOM wiring in panel.js, which no other test loads.
// Every element panel.js looks up by a literal id must exist in index.html:
// `$('gone').addEventListener(...)` throws while the wiring block runs, and every
// listener registered after it silently never attaches — the panel looks fine
// and its buttons do nothing. Zero npm deps, like the other dist tests.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const here = new URL('.', import.meta.url);
const js = readFileSync(new URL('panel.js', here), 'utf8');
const html = readFileSync(new URL('index.html', here), 'utf8');

// Literal `$('id')` lookups, plus ids handed to setPill(), which looks them up itself.
const referenced = [
  ...new Set([...js.matchAll(/(?:\$|setPill)\('([A-Za-z0-9_-]+)'/g)].map((m) => m[1])),
];
const defined = new Set([...html.matchAll(/\bid="([A-Za-z0-9_-]+)"/g)].map((m) => m[1]));

test('W1: every element panel.js looks up by id exists in index.html', () => {
  assert.ok(referenced.length > 20, `expected to find the wiring ids, found ${referenced.length}`);
  const missing = referenced.filter((id) => !defined.has(id));
  assert.deepEqual(missing, [], `panel.js references ids index.html no longer has: ${missing.join(', ')}`);
});

test('W2: index.html defines each id only once', () => {
  const all = [...html.matchAll(/\bid="([A-Za-z0-9_-]+)"/g)].map((m) => m[1]);
  const dupes = all.filter((id, i) => all.indexOf(id) !== i);
  assert.deepEqual(dupes, [], `duplicate ids: ${dupes.join(', ')}`);
});
