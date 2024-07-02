const assert = require('assert');
const fs = require('fs');
const path = require('path')
const shell = require('shelljs');

const verify = require('../../util/verifiers')
const mock = require('../../util/integration');

// =======================
// Standard Use-case Tests
// =======================

describe('Hardhat Plugin: standard use cases', function() {
  let hardhatConfig;
  let solcoverConfig;

  beforeEach(() => {
    mock.clean();

    mock.loggerOutput.val = '';
    solcoverConfig = {
      skipFiles: ['Migrations.sol'],
      istanbulReporter: [ "json-summary", "text"]
    };
    hardhatConfig = mock.getDefaultHardhatConfig();
    verify.cleanInitialState();
  })

  afterEach(() => {
    mock.hardhatTearDownEnv();
    mock.clean();
  });

  it('simple contract', async function(){
    mock.install('Simple', 'simple.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    verify.coverageGenerated(hardhatConfig);

    const output = mock.getOutput(hardhatConfig);
    const path = Object.keys(output)[0];

    assert(
      output[path].fnMap['1'].name === 'test',
      'coverage.json missing "test"'
    );

    assert(
      output[path].fnMap['2'].name === 'getX',
      'coverage.json missing "getX"'
    );
  });

  it('default network ("hardhat")', async function(){
    mock.install('Simple', 'simple.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    this.env.hardhatArguments.network = "hardhat"

    await this.env.run("coverage");

    assert(
      mock.loggerOutput.val.includes("HardhatEVM"),
      `Should have displayed HardhatEVM version: ${mock.loggerOutput.val}`
    );

    assert(
      mock.loggerOutput.val.includes("hardhat"),
      `Should have used 'hardhat' network name: ${mock.loggerOutput.val}`
    );
  });

  // Test fixture is not compatible with HH 2.5.0. Throws mysterious error (though fixture has no libs?)
  // HH11: Internal invariant was violated: Libraries should have both name and version, or neither one
  it.skip('with relative path solidity imports', async function() {
    mock.installFullProject('import-paths');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");
  });

  it('uses inheritance', async function() {
    mock.installDouble(
      ['Proxy', 'Owned'],
      'inheritance.js',
      solcoverConfig
    );

    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    verify.coverageGenerated(hardhatConfig);

    const output = mock.getOutput(hardhatConfig);
    const ownedPath = Object.keys(output)[0];
    const proxyPath = Object.keys(output)[1];

    assert(
      output[ownedPath].fnMap['1'].name === 'constructor',
      '"constructor" not covered'
    );

    assert(
      output[proxyPath].fnMap['1'].name === 'isOwner',
      '"isOwner" not covered'
    );
  });

  it('only uses ".call"', async function(){
    mock.install('OnlyCall', 'only-call.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [{
     file: mock.pathToContract(hardhatConfig, 'OnlyCall.sol'),
     pct: 100
    }];

    verify.lineCoverage(expected);
  });

  it('sends / transfers to instrumented fallback', async function(){
    mock.install('Wallet', 'wallet.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    verify.coverageGenerated(hardhatConfig);

    const output = mock.getOutput(hardhatConfig);
    const path = Object.keys(output)[0];
    assert(
      output[path].fnMap['1'].name === 'transferPayment',
      'cov should map "transferPayment"'
    );
  });

  // hardhat-truffle5 test asserts deployment cost is greater than 20,000,000 gas
  it('deployment cost > block gasLimit', async function() {
    mock.install('Expensive', 'block-gas-limit.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");
  });

  // Simple.sol with a failing assertion in a hardhat-truffle5 test
  it('unit tests failing', async function() {
    mock.install('Simple', 'truffle-test-fail.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage");
      assert.fail()
    } catch(err){
      assert(err.message.includes('failed under coverage'));
    }

    verify.coverageGenerated(hardhatConfig);

    const output = mock.getOutput(hardhatConfig);
    const path = Object.keys(output)[0];

    assert(output[path].fnMap['1'].name === 'test', 'cov missing "test"');
    assert(output[path].fnMap['2'].name === 'getX', 'cov missing "getX"');
  });

  // This project has [ @skipForCoverage ] tags in the test descriptions
  // at selected 'contract' and 'it' blocks.
  it('config: mocha options', async function() {
    solcoverConfig.mocha = {
      grep: '@skipForCoverage',
      invert: true,
    };

    solcoverConfig.silent = process.env.SILENT ? true : false,
    solcoverConfig.istanbulReporter = ['json-summary', 'text']

    mock.installFullProject('multiple-suites', solcoverConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'ContractA.sol'),
        pct: 0
      },
      {
        file: mock.pathToContract(hardhatConfig, 'ContractB.sol'),
        pct: 0,
      },
      {
        file: mock.pathToContract(hardhatConfig, 'ContractC.sol'),
        pct: 100,
      },
    ];

    verify.lineCoverage(expected);
  });

  // hardhat-truffle5 test asserts balance is 777 ether
  it('config: providerOptions', async function() {
    const taskArgs = {
      network: 'development'
    };

    solcoverConfig.providerOptions = { default_balance_ether: 777 }

    mock.install('Simple', 'testrpc-options.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage", taskArgs);
  });

  it('config: skipped file', async function() {
    solcoverConfig.skipFiles = ['Migrations.sol', 'Owned.sol'];

    mock.installDouble(
      ['Proxy', 'Owned'],
      'inheritance.js',
      solcoverConfig
    );

    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    verify.coverageGenerated(hardhatConfig);

    const output = mock.getOutput(hardhatConfig);
    const firstKey = Object.keys(output)[0];

    assert(
      Object.keys(output).length === 1,
      'Wrong # of contracts covered'
    );

    assert(
      firstKey.substr(firstKey.length - 9) === 'Proxy.sol',
      'Wrong contract covered'
    );
  });

  it('config: skipped folder', async function() {
    mock.installFullProject('skipping');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [{
     file: mock.pathToContract(hardhatConfig, 'ContractA.sol'),
     pct: 100
    }];

    const missing = [{
     file: mock.pathToContract(hardhatConfig, 'skipped-folder/ContractB.sol'),
    }];

    verify.lineCoverage(expected);
    verify.coverageMissing(missing);
  });

  it('config: "onServerReady", "onTestsComplete", ...', async function() {
    mock.installFullProject('test-files');

    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    assert(
      mock.loggerOutput.val.includes('running onServerReady')     &&
      mock.loggerOutput.val.includes('running onTestsComplete')   &&
      mock.loggerOutput.val.includes('running onCompileComplete') &&
      mock.loggerOutput.val.includes('running onIstanbulComplete'),

      `Should run "on" hooks : ${mock.loggerOutput.val}`
    );
  });

  it('solc 0.6.x', async function(){
    mock.installFullProject('solc-6');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'ContractA.sol'),
        pct: 61.54
      },
      {
        file: mock.pathToContract(hardhatConfig, 'ContractB.sol'),
        pct: 0,
      },
      {
        file: mock.pathToContract(hardhatConfig, 'B_Wallet.sol'),
        pct: 80,
      },

    ];

    verify.lineCoverage(expected);
  })

  it('solc 0.7.x', async function(){
    mock.installFullProject('solc-7');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'Contract_solc7.sol'),
        pct: 75
      },
      {
        file: mock.pathToContract(hardhatConfig, 'Functions_solc7.sol'),
        pct: 50,
      }
    ];

    verify.lineCoverage(expected);
  })

  it('solc 0.8.x', async function(){
    mock.installFullProject('solc-8');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expectedLine = [
      {
        file: mock.pathToContract(hardhatConfig, 'Contract_solc8.sol'),
        pct: 87.5
      },
    ];

    verify.lineCoverage(expectedLine);
  });

  it('hardhat_reset preserves coverage between resets', async function(){
    mock.installFullProject('hardhat-reset');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'ContractAReset.sol'),
        pct: 100
      }
    ];

    verify.lineCoverage(expected);
  })

  // This test freezes when gas-reporter is not disabled
  it('disables hardhat-gas-reporter', async function() {
    mock.installFullProject('hardhat-gas-reporter');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");
  });

  it('uses account[0] as default "from" (ganache)', async function(){
    const mnemonic = "purity blame spice arm main narrow olive roof science verb parrot flash";
    const account0 = "0x42ecc9ab31d7c0240532992682ee3533421dd7f5"
    const taskArgs = {
      network: "development"
    }

    solcoverConfig.providerOptions = {
      mnemonic: mnemonic
    };

    mock.install('Simple', 'account-zero.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage", taskArgs);

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'Simple.sol'),
        pct: 50
      }
    ];

    verify.lineCoverage(expected);
  })

  it('inherits network defined "from" (ganache)', async function(){
    const mnemonic = "purity blame spice arm main narrow olive roof science verb parrot flash";
    const account1 = "0xe7a46b209a65baadc11bf973c0f4d5f19465ae83"
    const taskArgs = {
      network: "development"
    }

    solcoverConfig.providerOptions = {
      mnemonic: mnemonic
    };

    const hardhatConfig = mock.getDefaultHardhatConfig()
    hardhatConfig.networks.development.from = account1;

    mock.install('Simple', 'account-one.js', solcoverConfig, hardhatConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage", taskArgs);

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'Simple.sol'),
        pct: 50
      }
    ];

    verify.lineCoverage(expected);
  })

  it('inherits network defined "from" (hardhat)', async function(){
    const mnemonic = "purity blame spice arm main narrow olive roof science verb parrot flash";
    const account1 = "0xe7a46b209a65baadc11bf973c0f4d5f19465ae83"

    const hardhatConfig = mock.getDefaultHardhatConfig()
    hardhatConfig.networks.hardhat = {
      from: account1,
      accounts: {
        mnemonic: mnemonic
      }
    }

    mock.install('Simple', 'account-one.js', solcoverConfig, hardhatConfig);
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'Simple.sol'),
        pct: 50
      }
    ];

    verify.lineCoverage(expected);
  })

  it('complex compiler configs', async function(){
    mock.installFullProject('hardhat-compile-config');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'ContractA1.sol'),
        pct: 33.33
      },
      {
        file: mock.pathToContract(hardhatConfig, 'ContractB1.sol'),
        pct: 100,
      },
      {
        file: mock.pathToContract(hardhatConfig, 'ContractC1.sol'),
        pct: 100,
      },

    ];

    verify.lineCoverage(expected);
  })

  it('locates .coverage_contracts correctly when dir is subfolder', async function(){
    mock.installFullProject('contract-subfolders');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: 'contracts/A/ContractA2.sol',
        pct: 100
      }
    ];

    verify.lineCoverage(expected);
  })

  it('logicalOR & ternary conditionals', async function(){
    mock.installFullProject('ternary-and-logical-or');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'Contract_OR.sol'),
        pct: 53.85
      },
      {
        file: mock.pathToContract(hardhatConfig, 'Contract_ternary.sol'),
        pct: 44.44
      },
    ];

    verify.branchCoverage(expected);
  })

  it('modifiers (multi-file)', async function(){
    mock.installFullProject('modifiers');
    mock.hardhatSetupEnv(this);

    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'ModifiersA.sol'),
        pct: 75
      },
      {
        file: mock.pathToContract(hardhatConfig, 'ModifiersC.sol'),
        pct: 25
      },
    ];

    verify.branchCoverage(expected);
  })

  it('modifiers (measureModifierCoverage = false)', async function(){
    solcoverConfig.measureModifierCoverage = false;

    mock.install('Modified', 'modified.js', solcoverConfig);
    mock.hardhatSetupEnv(this);
    await this.env.run("coverage");

    const expected = [
      {
        file: mock.pathToContract(hardhatConfig, 'Modified.sol'),
        pct: 100
      }
    ];

    verify.branchCoverage(expected);
  });
})
