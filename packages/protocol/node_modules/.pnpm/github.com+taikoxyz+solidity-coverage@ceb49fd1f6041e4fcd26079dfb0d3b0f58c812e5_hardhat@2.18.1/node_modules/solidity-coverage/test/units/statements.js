const assert = require('assert');
const util = require('./../util/util.js');

const client = require('ganache-cli');
const Coverage = require('./../../lib/coverage');
const Api = require('./../../lib/api')

describe('generic statements', () => {
  let coverage;
  let api;

  before(async () => {
    api = new Api({silent: true});
    await api.ganache(client);
  })
  beforeEach(() => coverage = new Coverage());
  after(async() => await api.finish());

  it('should compile function defined in a struct', () => {
    const info = util.instrumentAndCompile('statements/fn-struct');
    util.report(info.solcOutput.errors);
  })

  it('should compile when using the type keyword', () => {
    const info = util.instrumentAndCompile('statements/type-keyword');
    util.report(info.solcOutput.errors);
  })

  it('should compile base contract contructors with string args containing "{"', ()=> {
    const info = util.instrumentAndCompile('statements/interpolation');
    util.report(info.solcOutput.errors);
  })

  it('should instrument a single statement (first line of function)', () => {
    const info = util.instrumentAndCompile('statements/single');
    util.report(info.solcOutput.errors);
  });

  it('should instrument multiple statements', () => {
    const info = util.instrumentAndCompile('statements/multiple');
    util.report(info.solcOutput.errors);
  });

  it('should instrument a statement that is a function argument (single line)', () => {
    const info = util.instrumentAndCompile('statements/fn-argument');
    util.report(info.solcOutput.errors);
  });

  it('should instrument a statement that is a function argument (multi-line)', () => {
    const info = util.instrumentAndCompile('statements/fn-argument-multiline');
    util.report(info.solcOutput.errors);
  });

  it('should instrument an empty-contract-body', () => {
    const info = util.instrumentAndCompile('statements/empty-contract-ala-melonport');
    util.report(info.solcOutput.errors);
  });

  it('should instrument without triggering stack-too-deep', () => {
    const info = util.instrumentAndCompile('statements/stack-too-deep');
    util.report(info.solcOutput.errors);
  });

  it('should instrument an interface contract', () => {
    const info = util.instrumentAndCompile('statements/interface');
    util.report(info.solcOutput.errors);
  })

  it('should NOT pass tests if the contract has a compilation error', () => {
    const info = util.instrumentAndCompile('app/SimpleError');
    try {
      util.report(output.errors);
      assert.fail('failure'); // We shouldn't hit this.
    } catch (err) {
      (err.actual === 'failure') ? assert(false) : assert(true);
    }
  });

  it('should instrument an emit statement after an un-enclosed if statement', () => {
    const info = util.instrumentAndCompile('statements/emit-instrument');
    util.report(info.solcOutput.errors);
  });

  it('should cover an emitted event statement', async function() {
    const contract = await util.bootstrapCoverage('statements/emit-coverage', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(0);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      6: 1
    });
    assert.deepEqual(mapping[util.filePath].b, {});
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1,
    });
  });

  it('should cover a statement following a close brace', async function() {
    const contract = await util.bootstrapCoverage('statements/post-close-brace', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a(1);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      5: 1, 6: 0, 8: 1,
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

  it('should cover a library statement and an invoked library method', async function() {
    const contract = await util.bootstrapCoverage('statements/library', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.not();
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      11: 1, 12: 1, 21: 1,
    });
    assert.deepEqual(mapping[util.filePath].b, {});
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1,
    });
  });

  it('should cover a tuple statement', async function() {
    const contract = await util.bootstrapCoverage('statements/tuple', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a();
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {
      6: 1, 10: 1, 11: 1
    });
    assert.deepEqual(mapping[util.filePath].b, {});
    assert.deepEqual(mapping[util.filePath].s, {
      1: 1, 2: 1,
    });
    assert.deepEqual(mapping[util.filePath].f, {
      1: 1, 2: 1,
    });
  });

  it.skip('should cover a unary statement', async function(){
    const contract = await util.bootstrapCoverage('statements/unary', api);
    coverage.addContract(contract.instrumented, util.filePath);
    await contract.instance.a();
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    // TODO: obtain both statements in unary.sol
  })

  it('should cover an empty bodied contract statement', async function() {
    const contract = await util.bootstrapCoverage('statements/empty-contract-body', api);
    coverage.addContract(contract.instrumented, util.filePath);
    const mapping = coverage.generate(contract.data, util.pathPrefix);

    assert.deepEqual(mapping[util.filePath].l, {});
    assert.deepEqual(mapping[util.filePath].b, {});
    assert.deepEqual(mapping[util.filePath].s, {});
    assert.deepEqual(mapping[util.filePath].f, {});
  });
});
