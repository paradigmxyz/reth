const {
  TASK_COMPILE,
} = require('hardhat/builtin-tasks/task-names');

task(TASK_COMPILE).addFlag(
  'noExportAbi', 'Don\'t export ABI after running this task, even if runOnCompile option is enabled'
).setAction(async function (args, hre, runSuper) {
  await runSuper();

  if (!args.noExportAbi && !hre.__SOLIDITY_COVERAGE_RUNNING) {
    const configs = hre.config.abiExporter;

    await Promise.all(configs.map(abiGroupConfig => {
      if (abiGroupConfig.runOnCompile) {
        return hre.run('export-abi-group', { abiGroupConfig });
      }
    }));
  }
});
