'use strict';

var inherits = require('util').inherits;
var HookedWalletEthTxSubprovider = require('web3-provider-engine/subproviders/hooked-wallet-ethtx');

module.exports = WalletSubprovider;

inherits(WalletSubprovider, HookedWalletEthTxSubprovider);

function WalletSubprovider(wallet, opts) {
  opts = opts || {};

  opts.getAccounts = function (cb) {
    cb(null, [wallet.getAddressString()]);
  };

  opts.getPrivateKey = function (address, cb) {
    if (address !== wallet.getAddressString()) {
      cb(new Error('Account not found'));
    } else {
      cb(null, wallet.getPrivateKey());
    }
  };

  WalletSubprovider.super_.call(this, opts);
}