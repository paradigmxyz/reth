const assert = require('assert');
const util = require('./../util/util.js');

const client = require('ganache-cli');
const Coverage = require('./../../lib/coverage');
const Api = require('./../../lib/api')

describe('asserts and requires', () => {
  let coverage;
  let api;

  before(async () => {
    api = new Api({silent: true});
    await api.ganache(client);
  })
  beforeEach(() => coverage = new Coverage());
  after(async() => await api.finish());

  // Assert was covered as a branch up to v0.7.11. But since those
  // conditions are never meant to be fullfilled (and assert is really for smt)
  // people disliked this...
  it('should *not* cover assert statements as branches (pass)', async function() {
    const contract = await util.bootstrapCoverage('assert/Assert', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(true);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {});
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  // NB: truffle/contract replays failing txs as .calls to obtain the revert reason from the return
  // data. Hence the 2X measurements.
  it('should *not* cover assert statements as branches (fail)', async function() {
    const contract = await util.bootstrapCoverage('assert/Assert', api);
    coverage.addContract(contract.instrumented, util.filePath);

    try { await contract.instance.a(false) } catch(err) { /* Invalid opcode */ }

    const mapping = coverage.generate(contract.data, util.pathPrefix);
    assert.deepEqual(mapping[util.filePath].l, {
      5: 2,
    });
    assert.deepEqual(mapping[util.filePath].b, {});
    assert.deepEqual(mapping[util.filePath].s, {
      1: 2,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 2,
    });
  });

  it('should cover multi-line require stmts as `if` statements when they pass', async function() {
    const contract = await util.bootstrapCoverage('assert/RequireMultiline', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(true, true, true);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  // NB: Truffle replays failing txs as .calls to obtain the revert reason from the return
  // data. Hence the 2X measurements.
  it('should cover multi-line require stmts as `if` statements when they fail', async function() {
    const contract = await util.bootstrapCoverage('assert/RequireMultiline', api);
    coverage.addContract(contract.instrumented, util.filePath);

    try { await contract.instance.a(true, true, false) } catch(err) { /* Revert */ }

    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 2,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 2],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 2,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 2,
    });
  });

  it('should cover require statements with method arguments', async function() {
    const contract = await util.bootstrapCoverage('assert/Require-fn', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(true);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 9: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1
    });
  });

  it('should cover require statements with method arguments & reason string', async function() {
    const contract = await util.bootstrapCoverage('assert/Require-fn-reason', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(true);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 9: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1
    });
  });
});
