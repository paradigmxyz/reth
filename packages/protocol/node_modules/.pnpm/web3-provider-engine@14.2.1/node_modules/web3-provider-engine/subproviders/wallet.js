'use strict'

const inherits = require('util').inherits
const HookedWalletEthTxSubprovider = require('./hooked-wallet-ethtx.js')

module.exports = WalletSubprovider

inherits(WalletSubprovider, HookedWalletEthTxSubprovider)

function WalletSubprovider (wallet, opts) {
  opts.getAccounts = function (cb) {
    cb(null, [ wallet.getAddressString() ])
  }

  opts.getPrivateKey = function (address, cb) {
    if (address.toLowerCase() !== wallet.getAddressString()) {
      return cb('Account not found')
    }

    cb(null, wallet.getPrivateKey())
  }

  WalletSubprovider.super_.call(this, opts)
}
