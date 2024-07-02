const fs = require('fs');
const path = require('path');
const deleteEmpty = require('delete-empty');
const { types } = require('hardhat/config');
const { Interface } = require('@ethersproject/abi');

const readdirRecursive = function(dirPath, output = []) {
  const files = fs.readdirSync(dirPath);

  files.forEach(function(file) {
    file = path.join(dirPath, file);

    if (fs.statSync(file).isDirectory()) {
      output = readdirRecursive(file, output);
    } else {
      output.push(file);
    }
  });

  return output;
};

task('clear-abi', async function (args, hre) {
  const configs = hre.config.abiExporter;

  await Promise.all(configs.map((abiExporterConfig) => {
    return hre.run('clear-abi-group', { path: abiExporterConfig.path });
  }));
});

subtask(
  'clear-abi-group'
).addParam(
  'path', 'path to look for ABIs', undefined, types.string
).setAction(async function (args, hre) {
  const outputDirectory = path.resolve(hre.config.paths.root, args.path);

  if (!fs.existsSync(outputDirectory)) {
    return;
  }

  const files = readdirRecursive(outputDirectory);

  await Promise.all(files.map(async function (file) {
    if (path.extname(file) !== '.json') {
      // ABIs must be stored as JSON
      return;
    }

    const contents = await fs.promises.readFile(file);

    try {
      // attempt to parse ABI from file contents
      new Interface(contents.toString());
    } catch (e) {
      // file is not an ABI - do not delete
      return;
    }

    await fs.promises.rm(file);
  }));

  await deleteEmpty(outputDirectory);
});
