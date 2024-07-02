
/*
  Utilities for generating & managing mock projects to test plugins.
*/

const path = require('path');
const fs = require('fs');
const shell = require('shelljs');
const decache = require('decache');
const globalModules = require('global-modules');

const { HARDHAT_NETWORK_NAME } = require("hardhat/plugins")
const { resetHardhatContext } = require("hardhat/plugins-testing")

const temp =              './sc_temp';
const hardhatConfigName = 'hardhat.config.js';
const configPath =        `${temp}/.solcover.js`;
const testPath =          './test/sources/js/';
const sourcesPath =       './test/sources/solidity/contracts/app/';
const migrationPath =     `${temp}/migrations/2_deploy.js`;
const templatePath =      './test/integration/generic/*';
const projectPath =       './test/integration/projects/'

let previousCWD;

// ==========================
// Misc Utils
// ==========================
function decacheConfigs(){
  const paths = [
    `${process.cwd()}/${temp}/.solcover.js`,
    `${process.cwd()}/${temp}/${hardhatConfigName}`,
    `${process.cwd()}/${temp}/contracts/Simple.sol`,
    `${process.cwd()}/${temp}/test/simple.js`,
    `${process.cwd()}/${temp}/test/account-one.js`,
  ];

  paths.forEach(pth => {
    try { decache(pth) } catch (e){}
  });
}

function clean() {
  shell.config.silent = true;
  shell.rm('-Rf', temp);
  shell.rm('-Rf', 'coverage');
  shell.rm('coverage.json');
  shell.rm('.solcover.js');

  shell.config.silent = false;
};

function pathToContract(config, file) {
  return path.join('contracts', file);
}

function pathToTemp(_path) {
  return path.join(temp, _path);
}

function getOutput(config){
  const workingDir = config.working_directory || config.paths.root;
  const jsonPath = path.join(workingDir, "coverage.json");
  return JSON.parse(fs.readFileSync(jsonPath, 'utf8'));
}

// Hardhat env set up
function hardhatSetupEnv(mocha) {
  const mockwd = path.join(process.cwd(), temp);
  previousCWD = process.cwd();
  process.chdir(mockwd);
  mocha.env = require("hardhat");
  mocha.env.config.logger = testLogger
  mocha.logger = testLogger
};

// Hardhat env tear down
function hardhatTearDownEnv() {
  resetHardhatContext();
  process.chdir(previousCWD); // remove?
};


// ==========================
// NomicLab Configuration
// ==========================
function getDefaultNomicLabsConfig(){
  const logger = process.env.SILENT ? { log: () => {} } : console;
  const reporter = process.env.SILENT ? 'dot' : 'spec';

  const mockwd = path.join(process.cwd(), temp);
  const vals = {
    paths : {
      root: mockwd,
      artifacts:  path.join(mockwd, 'artifacts'),
      cache:  path.join(mockwd, 'cache'),
      sources: path.join(mockwd, 'contracts'),
      tests: path.join(mockwd, 'test'),
    },
    logger: logger,
    mocha: {
      reporter: reporter
    },
    networks: {
      development: {
        url: "http://127.0.0.1:8545",
      }
    }
  }

  return vals;
}

function getDefaultHardhatConfig() {
  const config = getDefaultNomicLabsConfig()
  config.defaultNetwork = HARDHAT_NETWORK_NAME;
  config.solidity = {
    version: "0.7.3"
  }
  return config;
}

function getHardhatConfigJS(config){
  const prefix =`
    require("@nomiclabs/hardhat-truffle5");
    require(__dirname + "/../plugins/nomiclabs.plugin");

  `

  if (config) {
    return `${prefix}module.exports = ${JSON.stringify(config, null, ' ')}`;
  } else {
    return `${prefix}module.exports = ${JSON.stringify(getDefaultHardhatConfig(), null, ' ')}`;
  }
}

// ==========================
// .solcover.js Configuration
// ==========================
function getSolcoverJS(config){
  return `module.exports = ${JSON.stringify(config, null, ' ')}`
}


// ==========================
// Migration Generators
// ==========================
function deploySingle(contractName){
  return `
    const A = artifacts.require("${contractName}");
    module.exports = function(deployer) { deployer.deploy(A) };
  `;
}

function deployDouble(contractNames){
  return `
    var A = artifacts.require("${contractNames[0]}");
    var B = artifacts.require("${contractNames[1]}");
    module.exports = function(deployer) {
      deployer.deploy(A);
      deployer.link(A, B);
      deployer.deploy(B);
    };
  `;
}

// ==========================
// Project Installers
// ==========================
/**
 * Installs mock project at ./temp with a single contract
 * and test specified by the params.
 * @param  {String} contract <contractName.sol> located in /test/sources/cli/
 * @param  {[type]} test     <testName.js> located in /test/cli/
 */
function install(
  contract,
  test,
  solcoverConfig,
  devPlatformConfig,
  noMigrations
) {
  if(solcoverConfig) solcoverJS = getSolcoverJS(solcoverConfig);

  const migration = deploySingle(contract);

  // Scaffold
  shell.mkdir(temp);
  shell.cp('-Rf', templatePath, temp);

  // Contract
  shell.cp(`${sourcesPath}${contract}.sol`, `${temp}/contracts/${contract}.sol`);

  // Migration
  if (!noMigrations) fs.writeFileSync(migrationPath, migration);

  // Test
  shell.cp(`${testPath}${test}`, `${temp}/test/${test}`);

  // Configs
  fs.writeFileSync(`${temp}/${hardhatConfigName}`, getHardhatConfigJS(devPlatformConfig));
  if(solcoverConfig) fs.writeFileSync(configPath, solcoverJS);

  decacheConfigs();
};

/**
 * Installs mock project with two contracts (for inheritance, libraries, etc)
 */
function installDouble(contracts, test, config, skipMigration) {
  const configjs = getSolcoverJS(config);
  const migration = deployDouble(contracts);

  // Scaffold
  shell.mkdir(temp);
  shell.cp('-Rf', templatePath, temp);

  // Contracts
  contracts.forEach(item => {
    (item.includes('.'))
      ? shell.cp(`${sourcesPath}${item}`, `${temp}/contracts/${item}`)
      : shell.cp(`${sourcesPath}${item}.sol`, `${temp}/contracts/${item}.sol`);
  });

  // Migration
  if (!skipMigration){
    fs.writeFileSync(migrationPath, migration)
  }

  // Test
  shell.cp(`${testPath}${test}`, `${temp}/test/${test}`);

  // Configs
  fs.writeFileSync(`${temp}/${hardhatConfigName}`, getHardhatConfigJS());
  fs.writeFileSync(configPath, configjs);

  decacheConfigs();
};

/**
 * Installs full project
 */
function installFullProject(name, config) {
  shell.mkdir(temp);
  shell.cp('-Rf', `${projectPath}${name}/{.,}*`, temp);

  if (config){
    const configjs = getSolcoverJS(config);
    fs.writeFileSync(`${temp}/.solcover.js`, configjs);
  }

  decacheConfigs();
}

// ==========================
// Logging
// ==========================
const loggerOutput = {
  val: ''
};

const testLogger = {
  log: (val) => {
    if (val !== undefined) loggerOutput.val += val;
    if (!process.env.SILENT && val !== undefined)
       console.log(val)
  }
}

module.exports = {
  pathToTemp: pathToTemp,
  testLogger: testLogger,
  loggerOutput: loggerOutput,
  getDefaultHardhatConfig: getDefaultHardhatConfig,
  install: install,
  installDouble: installDouble,
  installFullProject: installFullProject,
  clean: clean,
  pathToContract: pathToContract,
  getOutput: getOutput,
  hardhatSetupEnv: hardhatSetupEnv,
  hardhatTearDownEnv: hardhatTearDownEnv
}

