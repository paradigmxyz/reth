const assert = require('assert');
const fs = require('fs');
const path = require('path')
const pify = require('pify')
const shell = require('shelljs');
const ganache = require('ganache-cli')

const verify = require('../../util/verifiers')
const mock = require('../../util/integration');

// =======
// Errors
// =======

describe('Hardhat Plugin: error cases', function() {
  let hardhatConfig;
  let solcoverConfig;

  beforeEach(() => {
    mock.clean();

    mock.loggerOutput.val = '';
    solcoverConfig = { skipFiles: ['Migrations.sol']};
    hardhatConfig = mock.getDefaultHardhatConfig();
    verify.cleanInitialState();
  })

  afterEach(() => {
    mock.hardhatTearDownEnv();
    mock.clean();
  });

  // We're no longer checking context in HH since 2.0.4 because it just uses
  // all the default paths. (HH dev dep currently above that: (>= 2.0.7))
  it.skip('project contains no contract sources folder', async function() {
    mock.installFullProject('no-sources');
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage");
      assert.fail()
    } catch(err){
      assert(
        err.message.includes('Cannot locate expected contract sources folder'),
        `Should error when contract sources cannot be found:: ${err.message}`
      );

      assert(
        err.message.includes('sc_temp/contracts'),
        `Error message should contain path:: ${err.message}`
      );
    }

    verify.coverageNotGenerated(hardhatConfig);
  });

  it('.solcover.js has syntax error', async function(){
    mock.installFullProject('bad-solcoverjs');
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage");
      assert.fail()
    } catch(err){
      assert(
        err.message.includes('Could not load .solcover.js config file.'),
        `Should notify when solcoverjs has syntax error:: ${err.message}`
      );
    }

    verify.coverageNotGenerated(hardhatConfig);
  })

  it('.solcover.js has incorrectly formatted option', async function(){
    solcoverConfig.port = "Antwerpen";

    mock.install('Simple', 'simple.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage");
      assert.fail()
    } catch (err) {
      assert(
        err.message.includes('config option'),
        `Should error on incorrect config options: ${err.message}`
      );
    }
  });

  it('tries to launch with a port already in use', async function(){
    const taskArgs = {
      network: "development"
    }

    const server = ganache.server();

    mock.install('Simple', 'simple.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    await pify(server.listen)(8545);

    try {
      await this.env.run("coverage", taskArgs);
      assert.fail();
    } catch(err){
      assert(
        err.message.includes('already in use') &&
        err.message.includes('lsof'),
        `Should error on port-in-use with advice: ${err.message}`
      )
    }

    await pify(server.close)();
  });

  it('tries to launch with a non-existent network', async function(){
    const taskArgs = {
      network: "does-not-exist"
    }

    mock.install('Simple', 'simple.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage", taskArgs);
      assert.fail();
    } catch(err){
      assert(
        err.message.includes('is not a defined network in hardhat.config.js') &&
        err.message.includes('does-not-exist'),
        `Should error missing network error: ${err.message}`
      )
    }
  });

  it('uses an invalid istanbul reporter', async function() {
    solcoverConfig = {
      silent: process.env.SILENT ? true : false,
      istanbulReporter: ['does-not-exist']
    };

    mock.install('Simple', 'simple.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage");
      assert.fail();
    } catch(err){
      assert(
        err.message.includes('does-not-exist') &&
        err.message.includes('coverage reports could not be generated'),
        `Should error on invalid reporter: ${err.message}`
      )
    }

  });

  // Hardhat test contains syntax error
  it('hardhat crashes', async function() {
    mock.install('Simple', 'truffle-crash.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage");
      assert.fail()
    } catch(err){
      assert(err.toString().includes('SyntaxError'));
    }
  });

  // Solidity syntax errors
  it('compilation failure', async function(){
    mock.install('SimpleError', 'simple.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage");
      assert.fail()
    } catch(err){
      assert(err.message.includes('Compilation failed'));
    }

    verify.coverageNotGenerated(hardhatConfig);
  });

  it('instrumentation failure', async function(){
    mock.install('Unparseable', 'simple.js', solcoverConfig);
    mock.hardhatSetupEnv(this);

    try {
      await this.env.run("coverage");
      assert.fail()
    } catch(err){
      assert(
        err.message.includes('Unparseable.sol.'),
        `Should throw instrumentation errors with file name: ${err.toString()}`
      );

      assert(err.stack !== undefined, 'Should have error trace')
    }

    verify.coverageNotGenerated(hardhatConfig);
  });
})
