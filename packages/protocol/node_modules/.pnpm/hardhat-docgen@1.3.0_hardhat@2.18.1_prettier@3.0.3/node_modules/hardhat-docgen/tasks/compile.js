const {
  TASK_COMPILE,
} = require('hardhat/builtin-tasks/task-names');

task(TASK_COMPILE).addFlag(
  'noDocgen', 'Don\'t generate documentation after running this task, even if runOnCompile option is enabled'
).setAction(async function (args, hre, runSuper) {
  for (let compiler of hre.config.solidity.compilers) {
    compiler.settings.outputSelection['*']['*'].push('devdoc');
    compiler.settings.outputSelection['*']['*'].push('userdoc');
  }

  await runSuper();

  if (hre.config.docgen.runOnCompile && !args.noDocgen) {
    // Disable compile to avoid an infinite loop
    await hre.run('docgen', { noCompile: true });
  }
});
