const assert = require('node:assert/strict');
const test = require('node:test');

const { expandWorkspacePath, serverArguments } = require('../dist/config.js');
const languageConfiguration = require('../language-configuration.json');

test('Hoon words preserve hyphenated terms', () => {
  const words = 'kernel-state noun-digest:tip5'.match(
    new RegExp(languageConfiguration.wordPattern, 'g'),
  );
  assert.deepEqual(words, ['kernel-state', 'noun-digest', 'tip5']);
});

test('workspace variables and relative paths resolve consistently', () => {
  assert.equal(
    expandWorkspacePath('${workspaceFolder}/hoon', '/work/nockchain'),
    '/work/nockchain/hoon',
  );
  assert.equal(expandWorkspacePath('hoon/common/hoon.hoon', '/work/nockchain'),
    '/work/nockchain/hoon/common/hoon.hoon');
});

test('server arguments preserve compiler toggles and numeric bounds', () => {
  const args = serverArguments({
    serverPath: '',
    preludePath: 'hoon/common/hoon.hoon',
    dependenciesPath: 'hoon',
    entryPath: '',
    subjectTypeJamPath: '',
    dbug: false,
    vet: false,
    checkDelayMilliseconds: -3,
    maxChecks: 0,
    workerStackBytes: 1,
  }, '/work/nockchain');
  assert.deepEqual(args, [
    '--prelude', '/work/nockchain/hoon/common/hoon.hoon',
    '--deps-dir', '/work/nockchain/hoon',
    '--no-dbug',
    '--no-vet',
    '--check-delay-ms', '0',
    '--max-compiles', '0',
    '--worker-stack-bytes', '1048576',
  ]);
});
