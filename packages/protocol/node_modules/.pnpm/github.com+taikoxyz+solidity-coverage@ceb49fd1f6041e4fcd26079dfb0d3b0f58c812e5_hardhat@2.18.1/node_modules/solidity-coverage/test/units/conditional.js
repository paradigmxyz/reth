const assert = require('assert');
const util = require('./../util/util.js');

const client = require('ganache-cli');
const Coverage = require('./../../lib/coverage');
const Api = require('./../../lib/api')

describe('ternary conditionals', () => {
  let coverage;
  let api;

  before(async () => {
    api = new Api({silent: true});
    await api.ganache(client);
  })
  beforeEach(() => coverage = new Coverage());
  after(async() => await api.finish());

  async function setupAndRun(solidityFile){
    const contract = await util.bootstrapCoverage(solidityFile, api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a();
    return coverage.generate(contract.data, util.pathPrefix);
  }

  it('should cover a conditional that reaches the consequent (same-line)', async function() {
    const mapping = await setupAndRun('conditional/sameline-consequent');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover an unbracketed conditional that reaches the consequent (same-line)', async function() {
    const mapping = await setupAndRun('conditional/unbracketed-condition');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover a multi-part conditional (&&) that reaches the consequent', async function() {
    const mapping = await setupAndRun('conditional/and-condition');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover a multi-part conditional (||) that reaches the consequent', async function() {
    const mapping = await setupAndRun('conditional/or-condition');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1], 2: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover a multi-part unbracketed conditional (||) that reaches the consequent', async function() {
    const mapping = await setupAndRun('conditional/unbracketed-or-condition');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1], 2: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover an always-false multi-part unbracketed conditional (||)', async function() {
    const mapping = await setupAndRun('conditional/or-always-false-condition');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 0], 2: [0, 1],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover a conditional that reaches the alternate (same-line)', async function() {
    const mapping = await setupAndRun('conditional/sameline-alternate');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover a conditional that reaches the consequent (multi-line)', async function() {
    const mapping = await setupAndRun('conditional/multiline-consequent');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover a conditional that reaches the alternate (multi-line)', async function() {
    const mapping = await setupAndRun('conditional/multiline-alternate');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  // Runs bool z = (x) ? false : true;
  it('should cover a definition assignment by conditional that reaches the alternate', async function() {
    const mapping = await setupAndRun('conditional/declarative-exp-assignment-alternate');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1, 3: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  // Runs z = (x) ? false : true;
  it('should cover an identifier assignment by conditional that reaches the alternate', async function() {
    const mapping = await setupAndRun('conditional/identifier-assignment-alternate');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1, 8: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1, 3: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover an assignment to a member expression (reaches the alternate)', async function() {
    const mapping = await setupAndRun('conditional/mapping-assignment');

    assert.deepEqual(mapping[util.filePath].l, {
      11: 1, 12: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

});
