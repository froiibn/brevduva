// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0
// Bounded sequential delivery experiment. Acceptance is not model completion.
const fs = require('node:fs'), path = require('node:path');
const { spawn } = require('node:child_process');
async function runRounds(count, run) {
  const seen = [];
  for (let round = 1; round <= count; round++) {
    const record = await run(round, [...seen]);
    if (record.state !== 'accepted' || !record.message?.id || seen.includes(record.message.id))
      throw Error('Stop: delivery not accepted or repeated');
    seen.push(record.message.id);
  }
  return seen;
}
if (require.main === module) (async () => {
  const [thread, binding, sender, directory] = process.argv.slice(2);
  if (!thread || !binding || !sender || !directory) throw Error('Usage: node poc_brv_desktop_rounds.cjs THREAD BINDING SENDER NEW_STATE_DIR');
  fs.mkdirSync(directory);
  const ids = await runRounds(3, async (round, seen) => {
    const seenFile = path.join(directory, 'seen.json'), roundDir = path.join(directory, `round-${round}`);
    fs.writeFileSync(seenFile, JSON.stringify(seen), { flush: true });
    console.log(JSON.stringify({ at: new Date().toISOString(), event: 'round-start', round }));
    await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, [path.join(__dirname, 'poc_brv_desktop_bridge.cjs'), thread, binding, sender, roundDir, seenFile],
        { windowsHide: true, stdio: ['ignore', 'inherit', 'inherit'] });
      child.on('error', reject);
      child.on('exit', code => code === 0 ? resolve() : reject(Error(`Round ${round} exited ${code}; no retry`)));
    });
    return JSON.parse(fs.readFileSync(path.join(roundDir, 'delivery.json'), 'utf8'));
  });
  fs.writeFileSync(path.join(directory, 'accepted.json'), JSON.stringify(ids), { flush: true });
  console.log(JSON.stringify({ event: 'three-deliveries-accepted', ids }));
})().catch(error => { console.error(error.message); process.exitCode = 1; });
module.exports = { runRounds };
