const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { test } = require('node:test');

const workflow = fs.readFileSync(path.join(__dirname, '../workflows/bench.yml'), 'utf8');
const source = workflow.match(/            const validBalModes = [\s\S]*?(?=\n      - name: Acknowledge request)/)[0];

async function parse(args = {}, eventName = 'workflow_dispatch') {
  const outputs = {};
  const failures = [];
  await vm.runInNewContext(`(async () => {${source}\n})()`, {
    process: { env: Object.fromEntries(Object.entries(args).map(([key, value]) => [
      `INPUT_${key.toUpperCase().replaceAll('-', '_')}`, value,
    ])) },
    context: {
      eventName,
      repo: { owner: 'paradigmxyz', repo: 'reth' },
      issue: { number: 27928 },
      payload: { comment: { user: { login: 'tester' }, body: `derek bench ${Object.entries(args).map(([key, value]) => `${key}=${value}`).join(' ')}` } },
    },
    github: { rest: { pulls: {
      list: async () => ({ data: [] }),
      get: async () => ({ data: { head: { sha: 'test-sha', ref: 'test-ref', repo: { full_name: 'paradigmxyz/reth' } } } }),
    } } },
    core: {
      setOutput: (key, value) => { outputs[key] = value; },
      setFailed: message => failures.push(message),
      info: () => {},
    },
  });
  return { outputs, failures };
}

for (const event of ['workflow_dispatch', 'issue_comment']) {
  for (const bal of ['true', 'baseline', 'feature']) {
    test(`${event}: bal=${bal} uses the existing big-block format in both arms`, async () => {
      const { outputs, failures } = await parse({ bal }, event);
      assert.deepEqual(failures, []);
      assert.equal(outputs['big-blocks'], 'true');
      assert.equal(outputs['big-blocks-target-gas'], '1');
      assert.equal(outputs['node-bin'], 'reth-bb');
      assert.equal(outputs.bal, bal);
      assert.equal(outputs.blocks, '500');
      assert.equal(outputs.warmup, '125');
      assert.equal(outputs['run-pairs'], '6');
    });
    for (const extra of [{ reorg: '5' }, { mempool: 'true' }]) {
      test(`${event}: rejects bal=${bal} with ${Object.keys(extra)[0]}`, async () => {
        const { outputs, failures } = await parse({ bal, ...extra }, event);
        assert.deepEqual(outputs, {});
        assert.deepEqual(failures, [extra.reorg
          ? 'reorg is not supported with big-blocks or BAL (reth-bb cannot build reorg blocks)'
          : 'mempool is only supported in Engine mode without big-blocks or reorg']);
      });
    }
  }

  test(`${event}: retains explicit big-block targets and run sizes`, async () => {
    const { outputs, failures } = await parse({ bal: 'true', 'big-blocks': '2G', blocks: '17', warmup: '3', 'run-pairs': '2' }, event);
    assert.deepEqual(failures, []);
    assert.equal(outputs['big-blocks-target-gas'], '2G');
    assert.equal(outputs.blocks, '17');
    assert.equal(outputs.warmup, '3');
    assert.equal(outputs['run-pairs'], '2');
  });

  test(`${event}: non-BAL reorg replay continues using reth`, async () => {
    const { outputs, failures } = await parse({ reorg: '5' }, event);
    assert.deepEqual(failures, []);
    assert.equal(outputs['node-bin'], 'reth');
    assert.equal(outputs['big-blocks'], 'false');
    assert.equal(outputs.reorg, '5');
  });

  test(`${event}: rejects big-block reorgs even without BAL`, async () => {
    const { outputs, failures } = await parse({ 'big-blocks': '1', reorg: '5' }, event);
    assert.deepEqual(outputs, {});
    assert.deepEqual(failures, ['reorg is not supported with big-blocks or BAL (reth-bb cannot build reorg blocks)']);
  });

  for (const mode of ['rpc', 'call']) {
    test(`${event}: rejects BAL in ${mode} mode`, async () => {
      const { outputs, failures } = await parse({ mode, bal: 'true' }, event);
      assert.deepEqual(outputs, {});
      assert.deepEqual(failures, [`${mode} mode does not support big-blocks, reorg, or BAL`]);
    });
  }
}
