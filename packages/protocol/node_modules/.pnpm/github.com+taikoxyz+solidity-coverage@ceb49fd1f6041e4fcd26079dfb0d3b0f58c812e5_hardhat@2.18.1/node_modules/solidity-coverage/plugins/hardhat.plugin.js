const path = require('path');
const PluginUI = require('./resources/nomiclabs.ui');

const { task, types } = require("hardhat/config");
const { HardhatPluginError } = require("hardhat/plugins")
const {HARDHAT_NETWORK_RESET_EVENT} = require("hardhat/internal/constants");
const {
  TASK_TEST,
  TASK_COMPILE,
  TASK_COMPILE_SOLIDITY_GET_COMPILER_INPUT,
  TASK_COMPILE_SOLIDITY_GET_COMPILATION_JOB_FOR_FILE,
  TASK_COMPILE_SOLIDITY_LOG_COMPILATION_ERRORS
} = require("hardhat/builtin-tasks/task-names");

// Toggled true for `coverage` task only.
let measureCoverage = false;
let configureYulOptimizer = false;
let instrumentedSources;
let optimizerDetails;

// UI for the task flags...
const ui = new PluginUI();

subtask(TASK_COMPILE_SOLIDITY_GET_COMPILER_INPUT).setAction(async (_, { config }, runSuper) => {
  const solcInput = await runSuper();
  if (measureCoverage) {
    // The source name here is actually the global name in the solc input,
    // but hardhat uses the fully qualified contract names.
    for (const [sourceName, source] of Object.entries(solcInput.sources)) {
      const absolutePath = path.join(config.paths.root, sourceName);
      // Patch in the instrumented source code.
      if (absolutePath in instrumentedSources) {
        source.content = instrumentedSources[absolutePath];
      }
    }
  }
  return solcInput;
});

// Solidity settings are best set here instead of the TASK_COMPILE_SOLIDITY_GET_COMPILER_INPUT task.
subtask(TASK_COMPILE_SOLIDITY_GET_COMPILATION_JOB_FOR_FILE).setAction(async (_, __, runSuper) => {
  const compilationJob = await runSuper();
  if (measureCoverage && typeof compilationJob === "object") {
    if (compilationJob.solidityConfig.settings === undefined) {
      compilationJob.solidityConfig.settings = {};
    }

    const { settings } = compilationJob.solidityConfig;
    if (settings.metadata === undefined) {
      settings.metadata = {};
    }
    if (settings.optimizer === undefined) {
      settings.optimizer = {};
    }
    // Unset useLiteralContent due to solc metadata size restriction
    settings.metadata.useLiteralContent = false;
    // Override optimizer settings for all compilers
    settings.optimizer.enabled = false;

    // This is fixes a stack too deep bug in ABIEncoderV2
    // Experimental because not sure this works as expected across versions....
    if (configureYulOptimizer) {
      if (optimizerDetails === undefined) {
        settings.optimizer.details = {
          yul: true,
          yulDetails: {
            stackAllocation: true,
          },
        }
      // Other configurations may work as well. This loads custom details from .solcoverjs
      } else {
        settings.optimizer.details = optimizerDetails;
      }
    }
  }
  return compilationJob;
});

// Suppress compilation warnings because injected trace function triggers
// complaint about unused variable
subtask(TASK_COMPILE_SOLIDITY_LOG_COMPILATION_ERRORS).setAction(async (_, __, runSuper) => {
  const defaultWarn = console.warn;

  if (measureCoverage) {
    console.warn = () => {};
  }
  await runSuper();
  console.warn = defaultWarn;
});

/**
 * Coverage task implementation
 * @param  {HardhatUserArgs} args
 * @param  {HardhatEnv} env
 */
task("coverage", "Generates a code coverage report for tests")
  .addOptionalParam("testfiles",  ui.flags.file,       "", types.string)
  .addOptionalParam("solcoverjs", ui.flags.solcoverjs, "", types.string)
  .addOptionalParam('temp',       ui.flags.temp,       "", types.string)
  .addFlag('matrix', ui.flags.testMatrix)
  .addFlag('abi', ui.flags.abi)
  .setAction(async function(args, env){

  const API = require('./../lib/api');
  const utils = require('./resources/plugin.utils');
  const nomiclabsUtils = require('./resources/nomiclabs.utils');
  const pkg = require('./../package.json');

  let error;
  let ui;
  let api;
  let config;
  let client;
  let address;
  let failedTests = 0;

  instrumentedSources = {};
  measureCoverage = true;

  // Set a variable on the environment so other tasks can detect if this task is executing
  env.__SOLIDITY_COVERAGE_RUNNING = true;

  try {
    config = nomiclabsUtils.normalizeConfig(env.config, args);
    ui = new PluginUI(config.logger.log);
    api = new API(utils.loadSolcoverJS(config));

    optimizerDetails = api.solcOptimizerDetails;

    // Catch interrupt signals
    process.on("SIGINT", nomiclabsUtils.finish.bind(null, config, api, true));

    // Version Info
    ui.report('hardhat-versions', [pkg.version]);

    // Merge non-null flags into hardhatArguments
    const flags = {};
    for (const key of Object.keys(args)){
      if (args[key] && args[key].length){
        flags[key] = args[key]
      }
    }
    env.hardhatArguments = Object.assign(env.hardhatArguments, flags)

    // ===========================
    // Generate abi diff component
    // (This flag only useful within codecheck context)
    // ===========================
    if (args.abi){
      measureCoverage = false;
      await nomiclabsUtils.generateHumanReadableAbiList(env, api, TASK_COMPILE);
      return;
    }

    // ================
    // Instrumentation
    // ================

    const skipFiles = api.skipFiles || [];

    let {
      targets,
      skipped
    } = utils.assembleFiles(config, skipFiles);

    targets = api.instrument(targets);
    for (const target of targets) {
      instrumentedSources[target.canonicalPath] = target.source;
    }
    utils.reportSkipped(config, skipped);

    // ==============
    // Compilation
    // ==============
    ui.report('compilation', []);

    config.temp = args.temp;
    configureYulOptimizer = api.config.configureYulOptimizer;

    // With Hardhat >= 2.0.4, everything should automatically recompile
    // after solidity-coverage corrupts the artifacts.
    // Prior to that version, we (try to) save artifacts to a temp folder.
    if (!config.useHardhatDefaultPaths){
      const {
        tempArtifactsDir,
        tempContractsDir
      } = utils.getTempLocations(config);

      utils.setupTempFolders(config, tempContractsDir, tempArtifactsDir)
      config.paths.artifacts = tempArtifactsDir;
      config.paths.cache = nomiclabsUtils.tempCacheDir(config);
    }

    await env.run(TASK_COMPILE);

    await api.onCompileComplete(config);

    // ==============
    // Server launch
    // ==============
    let network = nomiclabsUtils.setupHardhatNetwork(env, api, ui);

    if (network.isHardhatEVM){
      accounts = await utils.getAccountsHardhat(network.provider);
      nodeInfo = await utils.getNodeInfoHardhat(network.provider);

      // Note: this only works if the reset block number is before any transactions have fired on the fork.
      // e.g you cannot fork at block 1, send some txs (blocks 2,3,4) and reset to block 2
      env.network.provider.on(HARDHAT_NETWORK_RESET_EVENT, () => {
        api.attachToHardhatVM(env.network.provider);
      });

      api.attachToHardhatVM(network.provider);

      ui.report('hardhat-network', [
        nodeInfo.split('/')[1],
        env.network.name,
      ]);
    } else {
      client = api.client || require('ganache-cli');
      address = await api.ganache(client);
      const accountsRequest = await utils.getAccountsGanache(api.server.provider);
      const nodeInfoRequest = await utils.getNodeInfoGanache(api.server.provider);

      ui.report('ganache-network', [
        nodeInfoRequest.result.split('/')[1],
        env.network.name,
        api.port
      ]);

      accounts = accountsRequest.result;
    }

    // Set default account (if not already configured)
    nomiclabsUtils.setNetworkFrom(network.config, accounts);

    // Run post-launch server hook;
    await api.onServerReady(config);

    // ======
    // Tests
    // ======
    const testfiles = args.testfiles
      ? nomiclabsUtils.getTestFilePaths(args.testfiles)
      : [];

    // Optionally collect tests-per-line-of-code data
    nomiclabsUtils.collectTestMatrixData(args, env, api);

    try {
      failedTests = await env.run(TASK_TEST, {testFiles: testfiles})
    } catch (e) {
      error = e;
    }
    await api.onTestsComplete(config);

    // =================================
    // Output (Istanbul or Test Matrix)
    // =================================
    (args.matrix)
      ? await api.saveTestMatrix()
      : await api.report();

    await api.onIstanbulComplete(config);

  } catch(e) {
    error = e;
  } finally {
    measureCoverage = false;
  }

  await nomiclabsUtils.finish(config, api);

  if (error !== undefined ) throw new HardhatPluginError(error);
  if (failedTests > 0) throw new HardhatPluginError(ui.generate('tests-fail', [failedTests]));
})
