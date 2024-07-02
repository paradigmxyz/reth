const fs = require('fs');
const path = require('path');
const webpack = require('webpack');
const { HardhatPluginError } = require('hardhat/plugins');
const {
  TASK_COMPILE,
} = require('hardhat/builtin-tasks/task-names');

const webpackConfig = require('../webpack.config.js');

task(
  'docgen', 'Generate NatSpec documentation automatically on compilation'
).addFlag(
  'noCompile', 'Don\'t compile before running this task'
).setAction(async function (args, hre) {
  if (!args.noCompile) {
    await hre.run(TASK_COMPILE, { noDocgen: true });
  }

  const config = hre.config.docgen;

  const output = {};

  const outputDirectory = path.resolve(hre.config.paths.root, config.path);

  if (!outputDirectory.startsWith(hre.config.paths.root)) {
    throw new HardhatPluginError('resolved path must be inside of project directory');
  }

  if(outputDirectory === hre.config.paths.root) {
    throw new HardhatPluginError('resolved path must not be root directory');
  }

  if (config.clear && fs.existsSync(outputDirectory)) {
    fs.rmSync(outputDirectory, { recursive: true });
  }

  const contractNames = await hre.artifacts.getAllFullyQualifiedNames();

  for (let contractName of contractNames) {
    if (config.only.length && !config.only.some(m => contractName.match(m))) continue;
    if (config.except.length && config.except.some(m => contractName.match(m))) continue;

    const [source, name] = contractName.split(':');

    const { abi, devdoc, userdoc } = (
      await hre.artifacts.getBuildInfo(contractName)
    ).output.contracts[source][name];

    if (!(devdoc && userdoc)) {
      throw new HardhatPluginError('devdoc and/or userdoc not found in compilation artifacts (try running `hardhat compile --force`)');
    }

    const { title, author, details } = devdoc;
    const { notice } = userdoc;

    // derive external signatures from internal types

    const getSigType = function ({ type, components = [] }) {
      return type.replace('tuple', `(${ components.map(getSigType).join(',') })`);
    };

    const members = abi.reduce(function (acc, el) {
      // constructor, fallback, and receive do not have names
      let name = el.name || el.type;
      let inputs = el.inputs || [];
      acc[`${ name }(${ inputs.map(getSigType)})`] = el;
      return acc;
    }, {});

    // associate devdoc and userdoc comments with abi elements

    Object.keys(devdoc.events || {}).forEach(function (sig) {
      Object.assign(
        members[sig] || {},
        devdoc.events[sig]
      );
    });

    Object.keys(devdoc.stateVariables || {}).forEach(function (name) {
      Object.assign(
        members[`${ name }()`] || {},
        devdoc.stateVariables[name],
        { type: 'stateVariable' }
      );
    });

    Object.keys(devdoc.methods || {}).forEach(function (sig) {
      Object.assign(
        members[sig] || {},
        devdoc.methods[sig]
      );
    });

    Object.keys(userdoc.events || {}).forEach(function (sig) {
      Object.assign(
        members[sig] || {},
        userdoc.events[sig]
      );
    });

    Object.keys(userdoc.methods || {}).forEach(function (sig) {
      Object.assign(
        members[sig] || {},
        userdoc.methods[sig]
      );
    });

    const membersByType = Object.keys(members).reduce(function (acc, sig) {
      const { type } = members[sig];
      acc[type] = acc[type] || {};
      acc[type][sig] = members[sig];
      return acc;
    }, {});

    const constructor = members[Object.keys(members).find(k => k.startsWith('constructor('))];
    const { 'fallback()': fallback, 'receive()': receive } = members;

    output[contractName] = {
      // metadata
      source,
      name,
      // top-level docs
      title,
      author,
      details,
      notice,
      // special functions
      constructor,
      fallback,
      receive,
      // docs
      events: membersByType.event,
      stateVariables: membersByType.stateVariable,
      methods: membersByType.function,
    };
  }

  let error = await new Promise(function (resolve) {
    webpackConfig.output = { ...webpackConfig.output, path: outputDirectory };
    webpackConfig.plugins.push(
      new webpack.EnvironmentPlugin({
        'DOCGEN_DATA': output,
      })
    );

    webpack(
      webpackConfig,
      function (error, stats) {
        resolve(error || stats.compilation.errors[0]);
      }
    );
  });

  if (error) {
    throw new HardhatPluginError(error);
  }
});
