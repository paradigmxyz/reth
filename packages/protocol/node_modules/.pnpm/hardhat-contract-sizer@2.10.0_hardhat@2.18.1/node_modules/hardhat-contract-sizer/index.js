const { extendConfig } = require('hardhat/config');

require('./tasks/compile.js');
require('./tasks/size_contracts.js');

extendConfig(function (config, userConfig) {
  config.contractSizer = Object.assign(
    {
      alphaSort: false,
      disambiguatePaths: false,
      runOnCompile: false,
      strict: false,
      only: [],
      except: [],
      outputFile: null,
      unit: 'KiB',
    },
    userConfig.contractSizer
  );
});
