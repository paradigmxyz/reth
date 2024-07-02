const assert = require('assert');
const util = require('./../util/util.js');

const client = require('ganache-cli');
const Coverage = require('./../../lib/coverage');
const Api = require('./../../lib/api')

describe('measureCoverage options', () => {
  let coverage;
  let api;

  before(async () => {
    api = new Api({silent: true});
    await api.ganache(client);
  })
  beforeEach(() => {
    api.config = {}
    coverage = new Coverage()
  });
  after(async() => await api.finish());

  async function setupAndRun(solidityFile, val){
    const contract = await util.bootstrapCoverage(solidityFile, api);
    coverage.addContract(contract.instrumented, util.filePath);

    /* some methods intentionally fail */
    try {
      (val)
        ? await contract.instance.a(val)
        : await contract.instance.a();
    } catch(e){}

    return coverage.generate(contract.data, util.pathPrefix);
  }

  // if (x == 1 || x == 2) { } else ...
  it('should ignore OR branches when measureBranchCoverage = false', async function() {
    api.config.measureBranchCoverage = false;
    const mapping = await setupAndRun('or/if-or', 1);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 8: 0
    });
    assert.deepEqual(mapping[util.filePath].b, {});
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 0,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should ignore if/else branches when measureBranchCoverage = false', async function() {
    api.config.measureBranchCoverage = false;
    const mapping = await setupAndRun('if/if-with-brackets', 1);

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

  it('should ignore ternary conditionals when measureBranchCoverage = false', async function() {
    api.config.measureBranchCoverage = false;
    const mapping = await setupAndRun('conditional/sameline-consequent');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {});

    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should ignore modifier branches when measureModifierCoverage = false', async function() {
    api.config.measureModifierCoverage = false;
    const mapping = await setupAndRun('modifiers/same-contract-pass');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 10: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, { // Source contains a `require`
      1: [1, 0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1
    });
  });

  it('should ignore statements when measureStatementCoverage = false', async function() {
    api.config.measureStatementCoverage = false;
    const mapping = await setupAndRun('modifiers/same-contract-pass');
    assert.deepEqual(mapping[util.filePath].s, {});
  });

  it('should ignore lines when measureLineCoverage = false', async function() {
    api.config.measureLineCoverage = false;
    const mapping = await setupAndRun('modifiers/same-contract-pass');
    assert.deepEqual(mapping[util.filePath].l, {});
  });

  it('should ignore functions when measureFunctionCoverage = false', async function() {
    api.config.measureFunctionCoverage = false;
    const mapping = await setupAndRun('modifiers/same-contract-pass');
    assert.deepEqual(mapping[util.filePath].f, {});
  });
});
