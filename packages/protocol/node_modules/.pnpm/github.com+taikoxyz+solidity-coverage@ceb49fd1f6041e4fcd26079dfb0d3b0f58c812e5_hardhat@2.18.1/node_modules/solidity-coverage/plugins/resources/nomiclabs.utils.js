const shell = require('shelljs');
const globby = require('globby');
const pluginUtils = require("./plugin.utils");
const path = require('path');
const DataCollector = require("./../../lib/collector")
const semver = require("semver")
const util = require('util')

// =============================
// Nomiclabs Plugin Utils
// =============================

/**
 * Returns a list of test files to pass to mocha.
 * @param  {String}   files   file or glob
 * @return {String[]}         list of files to pass to mocha
 */
function getTestFilePaths(files){
  const target = globby.sync([files])

  // Buidler/Hardhat supports js & ts
  const testregex = /.*\.(js|ts)$/;
  return target.filter(f => f.match(testregex) != null);
}

/**
 * Normalizes Buidler/Hardhat paths / logging for use by the plugin utilities and
 * attaches them to the config
 * @param  {Buidler/HardhatConfig} config
 * @return {Buidler/HardhatConfig}        updated config
 */
function normalizeConfig(config, args={}){
  config.workingDir = config.paths.root;
  config.contractsDir = config.paths.sources;
  config.testDir = config.paths.tests;
  config.artifactsDir = config.paths.artifacts;
  config.logger = config.logger ? config.logger : {log: null};
  config.solcoverjs = args.solcoverjs
  config.gasReporter = { enabled: false }
  config.matrix = args.matrix;

  try {
    const hardhatPackage = require('hardhat/package.json');
    if (semver.gt(hardhatPackage.version, '2.0.3')){
      config.useHardhatDefaultPaths = true;
    }
  } catch(e){ /* ignore */ }

  return config;
}

function setupBuidlerNetwork(env, api, ui){
  const { createProvider } = require("@nomiclabs/buidler/internal/core/providers/construction");

  let networkConfig = {};

  let networkName = (env.buidlerArguments.network !== 'buidlerevm')
    ? env.buidlerArguments.network
    : api.defaultNetworkName;

  if (networkName !== api.defaultNetworkName){
    networkConfig = env.config.networks[networkName];
    configureHttpProvider(networkConfig, api, ui)
  } else {
    networkConfig.url = `http://${api.host}:${api.port}`
  }

  const provider = createProvider(networkName, networkConfig);

  return configureNetworkEnv(
    env,
    networkName,
    networkConfig,
    provider
  )
}

function setupHardhatNetwork(env, api, ui){
  const { createProvider } = require("hardhat/internal/core/providers/construction");
  const { HARDHAT_NETWORK_NAME } = require("hardhat/plugins")

  let provider, networkName, networkConfig;
  let isHardhatEVM = false;

  networkName = env.hardhatArguments.network || HARDHAT_NETWORK_NAME;

  // HardhatEVM
  if (networkName === HARDHAT_NETWORK_NAME){
    isHardhatEVM = true;

    networkConfig = env.network.config;
    configureHardhatEVMGas(networkConfig, api);

    provider = createProvider(
      networkName,
      networkConfig,
      env.config.paths,
      env.artifacts,
    )

  // HttpProvider
  } else {
    if (!(env.config.networks && env.config.networks[networkName])){
      throw new Error(ui.generate('network-fail', [networkName]))
    }
    networkConfig = env.config.networks[networkName]
    configureNetworkGas(networkConfig, api);
    configureHttpProvider(networkConfig, api, ui)
    provider = createProvider(networkName, networkConfig);
  }

  return configureNetworkEnv(
    env,
    networkName,
    networkConfig,
    provider,
    isHardhatEVM
  )
}

function configureNetworkGas(networkConfig, api){
  networkConfig.gas =  api.gasLimit;
  networkConfig.gasPrice = api.gasPrice;
}

function configureHardhatEVMGas(networkConfig, api){
  networkConfig.allowUnlimitedContractSize = true;
  networkConfig.blockGasLimit = api.gasLimitNumber;
  networkConfig.gas =  api.gasLimit;
  networkConfig.gasPrice = api.gasPrice;
  networkConfig.initialBaseFeePerGas = 0;
}

function configureNetworkEnv(env, networkName, networkConfig, provider, isHardhatEVM){
  env.config.networks[networkName] = networkConfig;
  env.config.defaultNetwork = networkName;

  env.network = Object.assign(env.network, {
    name: networkName,
    config: networkConfig,
    provider: provider,
    isHardhatEVM: isHardhatEVM
  });

  env.ethereum = provider;

  // Return a reference so we can set the from account
  return env.network;
}

/**
 * Extracts port from url / sets network.url
 * @param  {Object} networkConfig
 * @param  {SolidityCoverage} api
 */
function configureHttpProvider(networkConfig, api, ui){
  const configPort = networkConfig.url.split(':')[2];

  // Warn: port conflicts
  if (api.port !== api.defaultPort && api.port !== configPort){
    ui.report('port-clash', [ configPort ])
  }

  // Prefer network port
  api.port = parseInt(configPort);
  networkConfig.url = `http://${api.host}:${api.port}`;
}

/**
 * Configures mocha to generate a json object which maps which tests
 * hit which lines of code.
 */
function collectTestMatrixData(args, env, api){
  if (args.matrix){
    mochaConfig = env.config.mocha || {};
    mochaConfig.reporter = api.matrixReporterPath;
    mochaConfig.reporterOptions = {
      collectTestMatrixData: api.collectTestMatrixData.bind(api),
      saveMochaJsonOutput: api.saveMochaJsonOutput.bind(api),
      cwd: api.cwd
    }
    env.config.mocha = mochaConfig;
  }
}

/**
 * Returns all Hardhat artifacts.
 * @param  {HRE} env
 * @return {Artifact[]}
 */
async function getAllArtifacts(env){
  const all = [];
  const qualifiedNames = await env.artifacts.getArtifactPaths();
  for (const name of qualifiedNames){
    all.push(require(name));
  }
  return all;
}

/**
 * Compiles project
 * Collects all artifacts from Hardhat project,
 * Converts them to a format that can be consumed by api.abiUtils.diff
 * Saves them to `api.abiOutputPath`
 * @param  {HRE}    env
 * @param  {SolidityCoverageAPI} api
 */
async function generateHumanReadableAbiList(env, api, TASK_COMPILE){
  await env.run(TASK_COMPILE);
  const _artifacts = await getAllArtifacts(env);
  const list = api.abiUtils.generateHumanReadableAbiList(_artifacts)
  api.saveHumanReadableAbis(list);
}

/**
 * Sets the default `from` account field in the network that will be used.
 * This needs to be done after accounts are fetched from the launched client.
 * @param {env} config
 * @param {Array}         accounts
 */
function setNetworkFrom(networkConfig, accounts){
  if (!networkConfig.from){
    networkConfig.from = accounts[0];
  }
}

// TODO: Hardhat cacheing??
/**
 * Generates a path to a temporary compilation cache directory
 * @param  {BuidlerConfig} config
 * @return {String}        .../.coverage_cache
 */
function tempCacheDir(config){
  return path.join(config.paths.root, '.coverage_cache');
}

/**
 * Silently removes temporary folders and calls api.finish to shut server down
 * @param  {Buidler/HardhatConfig}     config
 * @param  {SolidityCoverage}  api
 * @return {Promise}
 */
async function finish(config, api, shouldKill){
  const {
    tempContractsDir,
    tempArtifactsDir
  } = pluginUtils.getTempLocations(config);

  shell.config.silent = true;
  shell.rm('-Rf', tempContractsDir);
  shell.rm('-Rf', tempArtifactsDir);
  shell.rm('-Rf', path.join(config.paths.root, '.coverage_cache'));
  shell.config.silent = false;

  if (api) await api.finish();
  if (shouldKill) process.exit(1)
}

module.exports = {
  normalizeConfig,
  finish,
  tempCacheDir,
  setupBuidlerNetwork,
  setupHardhatNetwork,
  getTestFilePaths,
  setNetworkFrom,
  collectTestMatrixData,
  getAllArtifacts,
  generateHumanReadableAbiList
}

