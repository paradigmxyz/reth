const fs = require('fs');
const path = require('path');
const chalk = require('chalk');
const stripAnsi = require('strip-ansi');
const Table = require('cli-table3');
const { HardhatPluginError } = require('hardhat/plugins');
const {
  TASK_COMPILE,
} = require('hardhat/builtin-tasks/task-names');

// see EIPs 170 and 3860 for more information
// https://eips.ethereum.org/EIPS/eip-170
// https://eips.ethereum.org/EIPS/eip-3860
const DEPLOYED_SIZE_LIMIT = 24576;
const INIT_SIZE_LIMIT = 49152;

const UNITS = { 'B': 1, 'kB': 1000, 'KiB': 1024 };

task(
  'size-contracts', 'Output the size of compiled contracts'
).addFlag(
  'noCompile', 'Don\'t compile before running this task'
).setAction(async function (args, hre) {
  if (!args.noCompile) {
    await hre.run(TASK_COMPILE, { noSizeContracts: true });
  }

  const config = hre.config.contractSizer;

  if (!UNITS[config.unit]) {
    throw new HardhatPluginError(`Invalid unit: ${ config.unit }`);
  }

  const formatSize = function (size) {
    const divisor = UNITS[config.unit];
    return (size / divisor).toFixed(3);
  };

  const outputData = [];

  const fullNames = await hre.artifacts.getAllFullyQualifiedNames();

  const outputPath = path.resolve(
    hre.config.paths.cache,
    '.hardhat_contract_sizer_output.json'
  );

  const previousSizes = {};
  const previousInitSizes = {};

  if (fs.existsSync(outputPath)) {
    const previousOutput = await fs.promises.readFile(outputPath);

    JSON.parse(previousOutput).forEach(function (el) {
      previousSizes[el.fullName] = el.deploySize;
      previousInitSizes[el.fullName] = el.initSize;
    });
  }

  await Promise.all(fullNames.map(async function (fullName) {
    if (config.only.length && !config.only.some(m => fullName.match(m))) return;
    if (config.except.length && config.except.some(m => fullName.match(m))) return;

    const { deployedBytecode, bytecode } = await hre.artifacts.readArtifact(fullName);
    const deploySize = Buffer.from(
      deployedBytecode.replace(/__\$\w*\$__/g, '0'.repeat(40)).slice(2),
      'hex'
    ).length;
    const initSize = Buffer.from(
      bytecode.replace(/__\$\w*\$__/g, '0'.repeat(40)).slice(2),
      'hex'
    ).length;

    outputData.push({
      fullName,
      displayName: config.disambiguatePaths ? fullName : fullName.split(':').pop(),
      deploySize,
      previousDeploySize: previousSizes[fullName] || null,
      initSize,
      previousInitSize: previousInitSizes[fullName] || null,
    });
  }));

  if (config.alphaSort) {
    outputData.sort((a, b) => a.displayName.toUpperCase() > b.displayName.toUpperCase() ? 1 : -1);
  } else {
    outputData.sort((a, b) => a.deploySize - b.deploySize);
  }

  await fs.promises.writeFile(outputPath, JSON.stringify(outputData), { flag: 'w' });

  const table = new Table({
    style: { head: [], border: [], 'padding-left': 2, 'padding-right': 2 },
    chars: {
      mid: '·',
      'top-mid': '|',
      'left-mid': ' ·',
      'mid-mid': '|',
      'right-mid': '·',
      left: ' |',
      'top-left': ' ·',
      'top-right': '·',
      'bottom-left': ' ·',
      'bottom-right': '·',
      middle: '·',
      top: '-',
      bottom: '-',
      'bottom-mid': '|',
    },
  });

  const compiler = hre.config.solidity.compilers[0];

  table.push([
    {
      content: chalk.gray(`Solc version: ${compiler.version}`),
    },
    {
      content: chalk.gray(`Optimizer enabled: ${compiler.settings.optimizer.enabled}`),
    },
    {
      content: chalk.gray(`Runs: ${compiler.settings.optimizer.runs}`),
    },
  ]);

  table.push([
    {
      content: chalk.bold('Contract Name'),
    },
    {
      content: chalk.bold(`Deployed size (${config.unit}) (change)`),
    },
    {
      content: chalk.bold(`Initcode size (${config.unit}) (change)`),
    },
  ]);

  let oversizedContracts = 0;

  for (let item of outputData) {
    if (item.deploySize === 0 && item.initSize === 0) {
      continue;
    }

    let deploySize = formatSize(item.deploySize);
    let initSize = formatSize(item.initSize);

    if (item.deploySize > DEPLOYED_SIZE_LIMIT || item.initSize > INIT_SIZE_LIMIT) {
      oversizedContracts++;
    }

    if (item.deploySize > DEPLOYED_SIZE_LIMIT) {
      deploySize = chalk.red.bold(deploySize);
    } else if (item.deploySize > DEPLOYED_SIZE_LIMIT * 0.9) {
      deploySize = chalk.yellow.bold(deploySize);
    }

    if (item.initSize > INIT_SIZE_LIMIT) {
      initSize = chalk.red.bold(initSize);
    } else if (item.initSize > INIT_SIZE_LIMIT * 0.9) {
      initSize = chalk.yellow.bold(initSize);
    }

    let deployDiff = '';
    let initDiff = '';

    if (item.previousDeploySize) {
      if (item.deploySize < item.previousDeploySize) {
        deployDiff = chalk.green(`-${formatSize(item.previousDeploySize - item.deploySize)}`);
      } else if (item.deploySize > item.previousDeploySize) {
        deployDiff = chalk.red(`+${formatSize(item.deploySize - item.previousDeploySize)}`);
      } else {
        deployDiff = chalk.gray(formatSize(0));
      }
    }

    if (item.previousInitSize) {
      if (item.initSize < item.previousInitSize) {
        initDiff = chalk.green(`-${formatSize(item.previousInitSize - item.initSize)}`);
      } else if (item.initSize > item.previousInitSize) {
        initDiff = chalk.red(`+${formatSize(item.initSize - item.previousInitSize)}`);
      } else {
        initDiff = chalk.gray(formatSize(0));
      }
    }

    table.push([
      { content: item.displayName },
      { content: `${deploySize} (${deployDiff})`, hAlign: 'right' },
      { content: `${initSize} (${initDiff})`, hAlign: 'right' },
    ]);
  }

  console.log(table.toString());
  if (config.outputFile)
    fs.writeFileSync(config.outputFile, `${stripAnsi(table.toString())}\n`);

  if (oversizedContracts > 0) {
    console.log();

    const message = `Warning: ${oversizedContracts} contracts exceed the size limit for mainnet deployment (${formatSize(DEPLOYED_SIZE_LIMIT)} ${config.unit} deployed, ${formatSize(INIT_SIZE_LIMIT)} ${config.unit} init).`;

    if (config.strict) {
      throw new HardhatPluginError(message);
    } else {
      console.log(chalk.red(message));
    }
  }
});
