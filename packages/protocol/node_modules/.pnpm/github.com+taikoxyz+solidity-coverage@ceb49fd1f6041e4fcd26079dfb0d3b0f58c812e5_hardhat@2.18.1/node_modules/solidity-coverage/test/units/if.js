const assert = require('assert');
const util = require('./../util/util.js');

const client = require('ganache-cli');
const Coverage = require('./../../lib/coverage');
const Api = require('./../../lib/api')

describe('if, else, and else if statements', () => {
  let coverage;
  let api;

  before(async () => {
    api = new Api({silent: true});
    await api.ganache(client);
  })
  beforeEach(() => coverage = new Coverage());
  after(async() => await api.finish());

  it('should compile after instrumenting unbracketed if-elses', () => {
    const info = util.instrumentAndCompile('if/if-else-no-brackets');
    util.report(info.solcOutput.errors);
  });

  it('should compile after instrumenting multiple unbracketed if-elses', () => {
    const info = util.instrumentAndCompile('if/else-if-unbracketed-multi');
    util.report(info.solcOutput.errors);
  });

  it('should cover an if statement with a bracketed consequent', async function() {
    const contract = await util.bootstrapCoverage('if/if-with-brackets', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(1);
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

  // Runs: a(1) => if (x == 1) x = 2;
  it('should cover an unbracketed if consequent (single line)', async function(){
    const contract = await util.bootstrapCoverage('if/if-no-brackets', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(1);
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

  // Runs: a(1) => if (x == 1){\n x = 3; }
  it('should cover an if statement with multiline bracketed consequent', async function() {
    const contract = await util.bootstrapCoverage('if/if-with-brackets-multiline',api);

    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(1);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1,
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

  // Runs: a(1) => if (x == 1)\n x = 3;
  it('should cover an unbracketed if consequent (multi-line)', async function() {
    const contract = await util.bootstrapCoverage('if/if-no-brackets-multiline',api);

    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(1);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 1,
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

  // Runs: a(2) => if (x == 1) { x = 3; }
  it('should cover a simple if statement with a failing condition', async function() {
    const contract = await util.bootstrapCoverage('if/if-with-brackets',api);

    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(2);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1,
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

  // Runs: a(2) => if (x == 1){\n throw;\n }else{\n x = 5; \n}
  it('should cover an if statement with a bracketed alternate', async function() {
    const contract = await util.bootstrapCoverage('if/else-with-brackets',api);

    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(2);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 0, 8: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 0,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover an if statement with an unbracketed alternate', async function() {
    const contract = await util.bootstrapCoverage('if/else-without-brackets',api);

    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(2);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 0, 8: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 0,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });

  });

  it('should cover an else if statement with an unbracketed alternate', async function() {
    const contract = await util.bootstrapCoverage('if/else-if-without-brackets',api);

    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(2);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 0, 8: 0,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1], 2: [0, 1]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 0, 3: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover nested if statements with missing else statements', async function() {
    const contract = await util.bootstrapCoverage('if/nested-if-missing-else',api);

    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(2,3,3);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 7: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1], 2: [1, 0], 3: [1, 0],
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1, 3: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });

  });

  it('should cover if-elseif-else statements that are at the same depth as each other', async function() {
    const contract = await util.bootstrapCoverage('if/if-elseif-else',api);

    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(2,3,3);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 0, 8: 1, 10: 0, 13: 1, 14: 0, 16: 1, 18: 0,
    });
    assert.deepEqual(mapping[util.filePath].b, {
      1: [0, 1], 2: [1, 0], 3: [0, 1], 4: [1, 0]
    });
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1, 3: 1, 4: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });
});
