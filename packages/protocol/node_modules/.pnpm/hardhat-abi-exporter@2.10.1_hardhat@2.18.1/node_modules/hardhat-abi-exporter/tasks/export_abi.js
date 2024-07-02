const fs = require('fs');
const path = require('path');
const { HardhatPluginError } = require('hardhat/plugins');
const { Interface, FormatTypes } = require('@ethersproject/abi');
const { types } = require('hardhat/config');
const {
  TASK_COMPILE,
} = require('hardhat/builtin-tasks/task-names');

task(
  'export-abi'
).addFlag(
  'noCompile', 'Don\'t compile before running this task'
).setAction(async function (args, hre) {
  if (!args.noCompile) {
    await hre.run(TASK_COMPILE, { noExportAbi: true });
  }

  const configs = hre.config.abiExporter;

  await Promise.all(configs.map(abiGroupConfig => {
    return hre.run('export-abi-group', { abiGroupConfig });
  }));
});

subtask(
  'export-abi-group'
).addParam(
  'abiGroupConfig', 'a single abi-exporter config object', undefined, types.any
).setAction(async function (args, hre) {
  const { abiGroupConfig: config } = args;

  const outputDirectory = path.resolve(hre.config.paths.root, config.path);

  if (outputDirectory === hre.config.paths.root) {
    throw new HardhatPluginError('resolved path must not be root directory');
  }

  const outputData = [];

  const fullNames = await hre.artifacts.getAllFullyQualifiedNames();

  await Promise.all(fullNames.map(async function (fullName) {
    if (config.only.length && !config.only.some(m => fullName.match(m))) return;
    if (config.except.length && config.except.some(m => fullName.match(m))) return;

    let { abi, sourceName, contractName } = await hre.artifacts.readArtifact(fullName);

    if (!abi.length) return;

    abi = abi.filter((element, index, array) => config.filter(element, index, array, fullName));

    if (config.format == 'minimal') {
      abi = new Interface(abi).format(FormatTypes.minimal);
    } else if (config.format == 'fullName') {
      abi = new Interface(abi).format(FormatTypes.fullName);
    } else if (config.format != 'json') {
      throw new HardhatPluginError(`Unknown format: ${config.format}`);
    }

    const destination = path.resolve(
      outputDirectory,
      config.rename(sourceName, contractName)
    ) + '.json';

    outputData.push({ abi, destination });
  }));

  outputData.reduce(function (acc, { abi, destination }) {
    const contents = acc[destination];

    if (contents && JSON.stringify(contents) !== JSON.stringify(abi)) {
      throw new HardhatPluginError(`multiple distinct contracts share same output destination: ${ destination }`);
    }

    acc[destination] = abi;
    return acc;
  }, {});

  if (config.clear) {
    await hre.run('clear-abi-group', { path: config.path });
  }

  await Promise.all(outputData.map(async function ({ abi, destination }) {
    await fs.promises.mkdir(path.dirname(destination), { recursive: true });
    await fs.promises.writeFile(destination, `${JSON.stringify(abi, null, config.spacing)}\n`, { flag: 'w' });
  }));
});
