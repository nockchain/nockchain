const assert = require('node:assert/strict');
const test = require('node:test');

const { parseShowReferencesArguments } = require('../dist/codeLens.js');

const position = { line: 3, character: 4 };
const location = {
  uri: 'file:///work/hoon/lib/a.hoon',
  range: { start: { line: 10, character: 2 }, end: { line: 10, character: 8 } },
};

test('well-formed show-references arguments round-trip', () => {
  const parsed = parseShowReferencesArguments(['file:///work/hoon/lib/a.hoon', position, [location]]);
  assert.deepEqual(parsed, {
    uri: 'file:///work/hoon/lib/a.hoon',
    position,
    locations: [location],
  });
  assert.deepEqual(
    parseShowReferencesArguments(['file:///work/hoon/lib/a.hoon', position, []]).locations,
    [],
  );
});

test('malformed show-references arguments are rejected', () => {
  const uri = 'file:///work/hoon/lib/a.hoon';
  const cases = [
    [],
    [uri, position],
    [uri, position, [location], 'extra'],
    ['', position, [location]],
    [uri, { line: -1, character: 0 }, [location]],
    [uri, { line: 1.5, character: 0 }, [location]],
    [uri, { line: '1', character: 0 }, [location]],
    [uri, position, location],
    [uri, position, [{ uri: '', range: location.range }]],
    [uri, position, [{ uri, range: { start: position } }]],
    [uri, position, [null]],
  ];
  for (const args of cases) {
    assert.equal(parseShowReferencesArguments(args), undefined, JSON.stringify(args));
  }
});
