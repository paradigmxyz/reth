const assert = require('assert');
const util = require('./../util/util.js');

const client = require('ganache-cli');
const Coverage = require('./../../lib/coverage');
const Api = require('./../../lib/api')

describe('modifiers', () => {
  let coverage;
  let api;

  before(async () => {
    api = new Api({silent: true});
    await api.ganache(client);
  })
  beforeEach(() => {
    api.config = {};
    coverage = new Coverage()
  });
  after(async() => await api.finish());

  async function setupAndRun(solidityFile){
    const contract = await util.bootstrapCoverage(solidityFile, api);
    coverage.addContract(contract.instrumented, util.filePath);

    /* some modifiers intentionally fail */
    try {
      await contract.instance.a();
    } catch(e){}

    return coverage.generate(contract.data, util.pathPrefix);
  }

  it('should cover a modifier branch which always succeeds', async function() {
    const mapping = await setupAndRun('modifiers/same-contract-pass');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 10: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0], 2: [1, 0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1
    });
  });

  // NB: Failures are replayed by truffle-contract
  it('should cover a modifier branch which never succeeds', async function() {
    const mapping = await setupAndRun('modifiers/same-contract-fail');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 2, 6: 0, 10: 0,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 2], 2: [0, 2]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 2, 2: 0,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 2, 2: 0
    });
  });

  it('should cover a modifier on an overridden function', async function() {
    const mapping = await setupAndRun('modifiers/override-function');

    assert.deepEqual(mapping[util.filePath].l, {
      9: 1, 10: 1, 14: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0], 2: [1, 0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1
    });
  });

  it('should cover multiple modifiers on the same function', async function() {
    const mapping = await setupAndRun('modifiers/multiple-mods-same-fn');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 10: 1, 11: 1, 15: 1
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0], 2: [1, 0], 3: [1, 0], 4: [1, 0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1, 3: 1
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1, 3: 1
    });
  });

  // Same test as above - should have 2 fewer branches
  it('should exclude whitelisted modifiers', async function() {
    api.config.modifierWhitelist = ['mmm', 'nnn'];
    const mapping = await setupAndRun('modifiers/multiple-mods-same-fn');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 10: 1, 11: 1, 15: 1
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0], 2: [1, 0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1, 3: 1
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1, 3: 1
    });
  });

  it('should cover multiple functions which use the same modifier', async function() {
    const contract = await util.bootstrapCoverage('modifiers/multiple-fns-same-mod', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a();
    await contract.instance.b();
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 2, 6: 2, 10: 1, 14: 1
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [2, 0], 2: [1, 0], 3: [1, 0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 2, 2: 1, 3: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 2, 2: 1, 3: 1
    });
  });

  it('should cover when both modifier branches are hit', async function() {
    const contract = await util.bootstrapCoverage('modifiers/both-branches', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a();
    await contract.instance.flip();

    try {
      await contract.instance.a();
    } catch(e) { /*ignore*/ }

    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      7: 3, 8: 1, 12: 1, 16: 1
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 2], 2: [1, 2],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 3, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 3, 2: 1, 3: 1
    });
  });

  // Case: when first modifier always suceeds but a subsequent modifier succeeds and fails,
  // there should be a missing `else` branch on first modifier
  it('should not be influenced by revert from a subsequent modifier', async function() {
    const contract = await util.bootstrapCoverage('modifiers/reverting-neighbor', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a();
    await contract.instance.flip();

    try {
      await contract.instance.a();
    } catch(e) { /*ignore*/ }

    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      "7":3,"8":3,"12":3,"13":1,"17":1,"21":1
    });
    assert.deepEqual(mapping[util.filePath].b, {
      "1":[3,0],"2":[1,2],"3":[3,0],"4":[1,2]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      "1":3,"2":3,"3":1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      "1":3,"2":3,"3":1,"4":1
    });
  });

  // Case: when the modifier always suceeds but fn logic succeeds and fails, there should be
  // a missing `else` branch on modifier
  it('should not be influenced by revert within the function', async function() {
    const contract = await util.bootstrapCoverage('modifiers/reverting-fn', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(true);

    try {
      await contract.instance.a(false);
    } catch(e) { /*ignore*/ }

    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      7: 3, 8: 3, 12: 3
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [3, 0], 2: [3, 0], 3: [1,2]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 3, 2: 3
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 3, 2: 3
    });
  });

  it('should cover when modifiers are listed with newlines', async function() {
    const mapping = await setupAndRun('modifiers/listed-modifiers');

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1, 10: 1, 11: 1, 19: 1
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [1, 0], 2: [1, 0], 3: [1, 0], 4: [1, 0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1, 3: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1, 3: 1
    });
  });

  it('should cover when same modifier is invoked twice on same fn', async function() {
    const mapping = await setupAndRun('modifiers/duplicate-mods-same-fn');

    assert.deepEqual(mapping[util.filePath].l, {
      "5":2,"13":1
    });
    assert.deepEqual(mapping[util.filePath].b, {
      "1":[1,0],"2":[1,0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 2, 2: 1
    });
  });

  it('should *not* treat constructor inheritance invocations as branches', async function() {
    const mapping = await setupAndRun('modifiers/constructor');
    assert.deepEqual(mapping[util.filePath].b, {});
  });

});
