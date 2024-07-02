// make sourcemaps work!
require("source-map-support/register");

const debug = require("debug")("ganache");

// we use optional dependencies which may, or may not exist, so try native first
try {
  // make sure these exist before we try to load ganache with native modules
  const optionalDependencies = require("./package.json").optionalDependencies;
  const wrongWeb3 = require("web3/package.json").version !== optionalDependencies.web3;
  const wrongEthereumJs =
    require("ethereumjs-wallet/package.json").version !== optionalDependencies["ethereumjs-wallet"];
  if (wrongWeb3 || wrongEthereumJs) {
    useBundled();
  } else {
    module.exports = require("./public-exports.js");
    module.exports._webpacked = false;
    debug("Optional dependencies installed; exporting ganache-core with native optional dependencies.");
  }
} catch (nativeError) {
  debug(nativeError);

  // grabbing the native/optional deps failed, try using our webpacked build.
  useBundled();
}

function useBundled() {
  try {
    module.exports = require("./build/ganache.core.node.js");
    module.exports._webpacked = true;
    debug("Optional dependencies not installed; exporting ganache-core from `./build` directory.");
  } catch (webpackError) {
    debug("ganache-core could not be exported; optional dependencies nor webpack build available for export.");
    throw webpackError;
  }
}
