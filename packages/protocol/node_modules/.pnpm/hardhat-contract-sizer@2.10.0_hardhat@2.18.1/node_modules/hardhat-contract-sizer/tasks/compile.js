const {
  TASK_COMPILE,
} = require('hardhat/builtin-tasks/task-names');

task(TASK_COMPILE).addFlag(
  'noSizeContracts', 'Don\'t size contracts after running this task, even if runOnCompile option is enabled'
).setAction(async function (args, hre, runSuper) {
  await runSuper();

  if (hre.config.contractSizer.runOnCompile && !args.noSizeContracts && !hre.__SOLIDITY_COVERAGE_RUNNING) {
    // Disable compile to avoid an infinite loop
    await hre.run('size-contracts', { noCompile: true });
  }
});
