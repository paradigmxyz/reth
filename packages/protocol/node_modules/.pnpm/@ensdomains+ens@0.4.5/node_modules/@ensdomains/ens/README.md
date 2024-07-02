# ENS

[![Build Status](https://travis-ci.org/ensdomains/ens.svg?branch=master)](https://travis-ci.org/ensdomains/ens)

Implementations for registrars and local resolvers for the Ethereum Name Service.

For documentation of the ENS system, see [docs.ens.domains](https://docs.ens.domains/).

To run unit tests, clone this repository, and run:

    $ npm install
    $ npm test

# npm package

This repo doubles as an npm package with the compiled JSON contracts

```js
import {
  Deed,
  DeedImplementation,
  ENS,
  ENSRegistry,
  FIFSRegistrar,
  HashRegistrar,
  Migrations,
  Registrar,
  ReverseRegistrar,
  TestRegistrar
} from '@ensdomains/ens'
```

## ENSRegistry.sol

Implementation of the ENS Registry, the central contract used to look up resolvers and owners for domains.

## FIFSRegistrar.sol

Implementation of a simple first-in-first-served registrar, which issues (sub-)domains to the first account to request them.

## HashRegistrar.sol

Implementation of a registrar based on second-price blind auctions and funds held on deposit, with a renewal process that weights renewal costs according to the change in mean price of registering a domain. Largely untested!

## HashRegistrarSimplified.sol

Simplified version of the above, with no support for renewals. This is the current proposal for interim registrar of the ENS system until a permanent registrar is decided on.

# ENS Registry interface

The ENS registry is a single central contract that provides a mapping from domain names to owners and resolvers, as described in [EIP 137](https://github.com/ethereum/EIPs/issues/137).

The ENS operates on 'nodes' instead of human-readable names; a human readable name is converted to a node using the namehash algorithm, which is as follows:

    def namehash(name):
      if name == '':
        return '\0' * 32
      else:
        label, _, remainder = name.partition('.')
        return sha3(namehash(remainder) + sha3(label))

The registry's interface is as follows:

## owner(bytes32 node) constant returns (address)

Returns the owner of the specified node.

## resolver(bytes32 node) constant returns (address)

Returns the resolver for the specified node.

## setOwner(bytes32 node, address owner)

Updates the owner of a node. Only the current owner may call this function.

## setSubnodeOwner(bytes32 node, bytes32 label, address owner)

Updates the owner of a subnode. For instance, the owner of "foo.com" may change the owner of "bar.foo.com" by calling `setSubnodeOwner(namehash("foo.com"), sha3("bar"), newowner)`. Only callable by the owner of `node`.

## setResolver(bytes32 node, address resolver)

Sets the resolver address for the specified node.

# Resolvers

Resolvers can be found in the resolver specific [repository](https://github.com/ensdomains/resolvers).

# Generating LLL ABI and binary data

ENS.lll.bin was generated with the following command, using the lllc packaged with Solidity 0.4.4:

    $ lllc ENS.lll > ENS.lll.bin

The files in the abi directory were generated with the following command:

    $ solc --abi -o abi AbstractENS.sol FIFSRegistrar.sol HashRegistrarSimplified.sol

# Getting started

Install Truffle

    $ npm install -g truffle

Launch the RPC client, for example TestRPC:

    $ testrpc

Deploy `ENS` and `FIFSRegistrar` to the private network, the deployment process is defined at [here](migrations/2_deploy_contracts.js):

    $ truffle migrate --network dev.fifs

alternatively, deploy the `HashRegistrar`:

    $ truffle migrate --network dev.auction

Check the truffle [documentation](http://truffleframework.com/docs/) for more information.
