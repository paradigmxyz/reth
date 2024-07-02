// var to = require("../utils/to");
var Transaction = require("../utils/transaction");
var Common = require("ethereumjs-common").default;
var ethUtil = require("ethereumjs-util");

const decode = function(json, done) {
  const options = {
    hash: json.hash,
    nonce: json.nonce,
    value: json.value,
    to: json.to,
    from: json.from,
    gasLimit: json.gas || json.gasLimit,
    gasPrice: json.gasPrice,
    data: json.data,
    v: json.v,
    r: json.r,
    s: json.s
  };

  const sigV = ethUtil.bufferToInt(options.v);
  let chainId = Math.floor((sigV - 35) / 2);
  if (chainId < 0) {
    chainId = 0;
  }

  const commonOptions = {
    name: "ganache",
    chainId,
    networkId: 1,
    comment: "Local test network"
  };

  let hardfork = "muirGlacier";
  if (json._options) {
    hardfork = json._options.hardfork;
    commonOptions.chainId = json._options.chainId;
    commonOptions.networkId = json._options.networkId;
  }

  const common = Common.forCustomChain(
    "mainnet", // TODO needs to match chain id
    commonOptions,
    hardfork
  );
  // databases generated before ganache-core@2.3.2 didn't have a `_type` and
  // and were always fake signed. So if _type is undefined it is "fake" (even
  // if we have a valid signature that can generate the tx's `from`).
  const type = json._type === undefined ? Transaction.types.fake : json._type;
  const tx = Transaction.fromJSON(options, type, common);

  // Commenting this out because we don't want to throw if the json.hash we
  // put in is different that the tx.hash() calculation we now have. There
  // may have been bug fixes to the way transactions are hashed in future
  // versions of ganache-core, but we still want tobe able to read in
  // transactions from previously saved databases!
  // if (to.hex(tx.hash()) !== json.hash) {
  //   const e = new Error(
  //     "DB consistency check: Decoded transaction hash " +
  //       "didn't match encoded hash. Expected: " +
  //       json.hash +
  //       "; actual: " +
  //       to.hex(tx.hash())
  //   );
  //   return done(e);
  // }

  done(null, tx);
};

const encode = function(tx, done) {
  const encoded = tx.encode();
  done(null, encoded);
};

module.exports = {
  encode,
  decode
};
