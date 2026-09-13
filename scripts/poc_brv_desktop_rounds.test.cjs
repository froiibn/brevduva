// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { runRounds } = require('./poc_brv_desktop_rounds.cjs');
test('bounded rounds pass earlier IDs to the next listener', async () => {
  const ids = await runRounds(3, async (round, seen) => {
    assert.equal(seen.length, round - 1);
    return { state: 'accepted', message: { id: String(round) } };
  });
  assert.deepEqual(ids, ['1', '2', '3']);
});
test('unknown delivery stops subsequent rounds', async () => {
  let calls = 0;
  await assert.rejects(runRounds(3, async () => { calls++; return { state: 'unknown-or-rejected' }; }), /Stop/);
  assert.equal(calls, 1);
});
test('repeated ID is not counted as a new round', async () => {
  await assert.rejects(runRounds(3, async () => ({ state: 'accepted', message: { id: 'same' } })), /Stop/);
});
