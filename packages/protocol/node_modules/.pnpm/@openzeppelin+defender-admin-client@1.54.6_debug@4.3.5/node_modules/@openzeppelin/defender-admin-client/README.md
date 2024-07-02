# Defender Admin Client

Defender Admin acts as an interface to manage your smart contract project through one or more secure multi-signature contracts. Defender Admin holds no control at all over your system, which is fully controlled by the keys of the signers.

To interact with your contracts, you create _proposals_ that need to be reviewed and approved by the other members of the multi-signature wallets. These proposals can be created directly in the Defender web application, or using this library. You can also rely on this library to add your contracts to the Defender Admin dashboard.

# End Of Support Notice

We will no longer be maintaining or supporting any additional releases for defender-client. Please migrate to defender-sdk as soon as possible to get all the benefits of defender 2.0 and more.

## Install

```bash
npm install @openzeppelin/defender-admin-client
```

```bash
yarn add @openzeppelin/defender-admin-client
```

## Usage

Start by creating a new _Team API Key_ in Defender, and granting it the capability to create new proposals. Use the newly created API key to initialize an instance of the Admin client.

```js
const { AdminClient } = require('@openzeppelin/defender-admin-client');
const client = new AdminClient({ apiKey: API_KEY, apiSecret: API_SECRET });
```

### Action proposals

To create a `custom` action proposal, you need to provide the function interface (which you can extract from the contract's ABI), its inputs, and the multisig that will be used for approving it:

```js
await client.createProposal({
  contract: { address: '0x28a8746e75304c0780E011BEd21C72cD78cd535E', network: 'sepolia' }, // Target contract
  title: 'Adjust fee to 10%', // Title of the proposal
  description: 'Adjust the contract fee collected per action to 10%', // Description of the proposal
  type: 'custom', // Use 'custom' for custom admin actions
  functionInterface: { name: 'setFee', inputs: [{ type: 'uint256', name: 'fee' }] }, // Function ABI
  functionInputs: ['10'], // Arguments to the function
  via: '0x22d491Bde2303f2f43325b2108D26f1eAbA1e32b', // Address to execute proposal
  viaType: 'Safe', // 'Gnosis Multisig', 'Safe' or 'EOA'
});
```

You can also optionally set the `simulate` flag as part of the `createProposal` request (as long as this is not a batch proposal) to simulate the proposal within the same request. You can override simulation parameters by setting the `overrideSimulationOpts` property, which is a `SimulationRequest` object.

```js
const proposalWithSimulation = await client.createProposal({
  contract: {
    address: '0xA91382E82fB676d4c935E601305E5253b3829dCD',
    network: 'mainnet',
    // provide abi OR overrideSimulationOpts.transactionData.data
    abi: JSON.stringify(contractABI),
  },
  title: 'Flash',
  description: 'Call the Flash() function',
  type: 'custom',
  metadata: {
    sendTo: '0xA91382E82fB676d4c935E601305E5253b3829dCD',
    sendValue: '10000000000000000',
    sendCurrency: {
      name: 'Ethereum',
      symbol: 'ETH',
      decimals: 18,
      type: 'native',
    },
  },
  functionInterface: { name: 'flash', inputs: [] },
  functionInputs: [],
  via: '0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266',
  viaType: 'EOA',
  // set simulate to true
  simulate: true,
  // optional
  overrideSimulationOpts: {
    transactionData: {
      // or instead of ABI, you can provide data
      data: '0xd336c82d',
    },
  },
});
```

#### Issuing DELEGATECALLs

When invoking a function via a Safe Wallet, it's possible to call it via a `DELEGATECALL` instruction instead of a regular call. This has the effect of executing the code in the called contract _in the context of the multisig_, meaning any operations that affect storage will affect the multisig, and any calls to additional contracts will be executed as if the `msg.sender` were the multisig. To do this, add a `metadata` parameter with the value `{ operationType: 'delegateCall' }` to your `createProposal` call:

```js
await client.createProposal({
  // ... Include all parameters from the example above
  via: '0x22d491Bde2303f2f43325b2108D26f1eAbA1e32b', // Multisig address
  viaType: 'Safe', // Must be Safe to handle delegate calls
  metadata: { operationType: 'delegateCall' }, // Issue a delegatecall instead of a regular call
});
```

Note that this can potentially brick your multisig, if the contract you delegatecall into accidentally modifies the multisig's storage, rendering it unusable. Make sure you understand the risks before issuing a delegatecall.

### Upgrade proposals

To create an `upgrade` action proposal, provide the proxy contract network and address, along with the new implementation address, and Defender will automatically resolve the rest (note that if no newImplementationAbi is provided the previous implementation ABI will be assumed for the proposal):

```js
const newImplementation = '0x3E5e9111Ae8eB78Fe1CC3bb8915d5D461F3Ef9A9';
const newImplementationAbi = '[...]';
const contract = { network: 'sepolia', address: '0x28a8746e75304c0780E011BEd21C72cD78cd535E' };
await client.proposeUpgrade({ newImplementation, newImplementationAbi }, contract);
```

If your proxies do not implement the [EIP1967 admin slot](https://eips.ethereum.org/EIPS/eip-1967#admin-address), you will need to provide either the [`ProxyAdmin` contract](https://github.com/OpenZeppelin/openzeppelin-contracts/blob/v4.0.0/contracts/proxy/transparent/ProxyAdmin.sol) or the Account with rights to execute the upgrade, as shown below.

#### Explicit ProxyAdmin

```js
const newImplementation = '0x3E5e9111Ae8eB78Fe1CC3bb8915d5D461F3Ef9A9';
const proxyAdmin = '0x2fC100f1BeA4ACCD5dA5e5ed725D763c90e8ca96';
const newImplementationAbi = '[...]';
const contract = { network: 'sepolia', address: '0x28a8746e75304c0780E011BEd21C72cD78cd535E' };
await client.proposeUpgrade({ newImplementation, newImplementationAbi, proxyAdmin }, contract);
```

#### Explicit owner account

```js
const newImplementation = '0x3E5e9111Ae8eB78Fe1CC3bb8915d5D461F3Ef9A9';
const via = '0xF608FA64c4fF8aDdbEd106E69f3459effb4bC3D1';
const viaType = 'Safe'; // 'Gnosis Multisig', 'Safe' or 'EOA'
const contract = { network: 'sepolia', address: '0x28a8746e75304c0780E011BEd21C72cD78cd535E' };
const newImplementationAbi = '[...]';
await client.proposeUpgrade({ newImplementation, newImplementationAbi, via, viaType }, contract);
```

### Pause proposals

To create `pause` and `unpause` action proposals, you need to provide the contract network and address, as well as the multisig that will be used for approving it. Defender takes care of the rest:

```js
const contract = { network: 'sepolia', address: '0x28a8746e75304c0780E011BEd21C72cD78cd535E' };

// Create a pause proposal
await client.proposePause({ via: '0x22d491Bde2303f2f43325b2108D26f1eAbA1e32b', viaType: 'Safe' }, contract);

// Create an unpause proposal
await client.proposeUnpause({ via: '0x22d491Bde2303f2f43325b2108D26f1eAbA1e32b', viaType: 'Safe' }, contract);
```

Note that for `pause` and `unpause` proposals to work, your contract ABI must include corresponding `pause()` and `unpause()` functions.

### Batch proposals

To create a `batch` proposal, you'll need to provide the contracts to use as an array in the `contract` param, and specify a list of `steps` to execute, in which you provide the information of execution for each function you'll call.

```js
const ERC20Token = '0x24B5C627cF54582F93eDbcF6186989227400Ac75';
const RolesContract = '0xa50d145697530e8fef3F59a9643c6E9992d0f30D';

const contracts = [
  {
    address: ERC20Token,
    name: 'ERC20 Token',
    network: 'sepolia',
    abi: '[{"inputs":[{"internalType":"uint256","name":"_amount","type":"uint256"}],"name":"mint","outputs":[],"stateMutability":"nonpayable","type":"function"},{"inputs":[{"internalType":"address","name":"to","type":"address"},{"internalType":"uint256","name":"amount","type":"uint256"}],"name":"transfer","outputs":[{"internalType":"bool","name":"","type":"bool"}],"stateMutability":"nonpayable","type":"function"}]',
  },
  {
    address: RolesContract,
    network: 'sepolia',
    name: 'Roles Contract',
    abi: '[{"inputs":[{"internalType":"bytes32","name":"role","type":"bytes32"},{"internalType":"address","name":"account","type":"address"}],"name":"grantRole","outputs":[],"stateMutability":"nonpayable","type":"function"}]',
  },
];

const safeAddress = '0x22d491Bde2303f2f43325b2108D26f1eAbA1e32b';

const steps = [
  {
    contractId: `sepolia-${ERC20Token}`,
    targetFunction: {
      name: 'mint',
      inputs: [{ type: 'uint256', name: 'amount' }],
    },
    functionInputs: ['999'],
    type: 'custom',
  },
  {
    contractId: `sepolia-${ERC20Token}`,
    targetFunction: {
      name: 'transfer',
      inputs: [
        { type: 'address', name: 'to' },
        { type: 'uint256', name: 'amount' },
      ],
    },
    functionInputs: [safeAddress, '999'],
    type: 'custom',
  },
  {
    contractId: `sepolia-${RolesContract}`,
    metadata: {
      action: 'grantRole',
      role: '0x0000000000000000000000000000000000000000000000000000000000000000',
      account: safeAddress,
    },
    type: 'access-control',
  },
];

await client.createProposal({
  contract: contracts,
  title: 'Batch test',
  description: 'Mint, transfer and modify access control',
  type: 'batch',
  via: safeAddress,
  viaType: 'Safe',
  metadata: {}, // Required field but empty
  steps,
});
```

### Relayer Execution Strategy

To use a relayer as an execution strategy you need to provide the `relayerId` as well as setting `via` to the relayer address and `viaType: 'Relayer'`

```js
const contract = { network: 'sepolia', address: '0xC73dAd1D9a356Ab2F3c6bC0049034aFe4B59DbB5' };

const proposal = await client.proposePause(
  {
    title: 'Pause contract',
    via: '0x6b74fa33f198a65fe374c8146387f1653d190c7a',
    viaType: 'Relayer',
    relayerId: 'dfa8b9a9-0f88-4d38-892a-93e1f5a8d2a7',
  },
  contract,
);
```

### List proposals

You can list all proposals:

```js
const proposals = await client.listProposals();
```

You can filter your active proposals by `isActive` property present on each proposal in the list response. By default, only unarchived proposals are returned, but you can override this by adding an `includeArchived: true` option in the call.

### Retrieve a proposal

You can retrieve a proposal given its contract and proposal ids:

```js
await client.getProposal(contractId, proposalId);
```

### Archiving proposals

You can archive or unarchive a proposal given its contract and proposal ids:

```js
await client.archiveProposal(contractId, proposalId);
await client.unarchiveProposal(contractId, proposalId);
```

### Simulate proposals

You can simulate an existing proposal. The results of a simulation (`SimulationResponse`) will be stored and can be retrieved with the getProposalSimulation endpoint:

```js
const proposal = await client.getProposal(contractId, proposalId);

// import the ABI and create an ethers interface
const contractInterface = new utils.Interface(contractABI);

// encode function data
const data = contractInterface.encodeFunctionData(proposal.functionInterface.name, proposal.functionInputs);

const simulation = await client.simulateProposal(
  proposal.contractId, // contractId
  proposal.proposalId, // proposalId
  {
    transactionData: {
      // this is the default hardhat address
      from: '0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266', // change this to impersonate the `from` address
      type: 'function-call', // or 'send-funds'
      data,
      to: proposal.contract.address,
      value: proposal.metadata.sendValue,
    },
    // default to latest finalized block,
    // can be up to 100 blocks ahead of current block,
    // does not support previous blocks
    blockNumber: undefined,
  },
);
```

> Note that a simulation may fail due to a number of reasons, such as network congestion, unstable providers or hitting a quota limitation. We would advise you to track the response code to assure a successful response was returned. If a transaction was reverted with a reason string, this can be obtained from the response object under `response.meta.returnString`. A transaction revert can be tracked from `response.meta.reverted`.

### Retrieve a proposal simulation

You can also retrieve existing simulations for a proposal:

```js
const proposal = await client.getProposal(contractId, proposalId);

const simulation = await client.getProposalSimulation(
  proposal.contractId, // contractId
  proposal.proposalId, // proposalId
);
```

## Adding Contracts

If you create a new proposal for a Contract that has not yet been added to Defender Admin, it will be automatically added with an autogenerated name and an empty ABI. You can optionally control these values by providing values for them in the `contract` object of the proposal:

```js
const contract = {
  network: 'sepolia',
  address: '0x28a8746e75304c0780E011BEd21C72cD78cd535E',
  name: 'My contract', // Name of the contract if it is created along with this proposal
  abi: '[...]', // ABI to set for this contract if it is created
};
await client.proposeUpgrade({ newImplementation }, contract);
```

Alternatively, you can add any contract explicitly by using the `addContract` method, and setting network, address, name, natspec and ABI. The same method can be used to update the contract's name or ABI.

```js
await client.addContract({
  network: 'sepolia',
  address: '0x28a8746e75304c0780E011BEd21C72cD78cd535E',
  name: 'My contract',
  abi: '[...]',
  natSpec: '{devdoc:{...}, userdoc: {...}}',
});
```

You can also list all contracts in your Defender Admin dashboard via `listContracts`.

## FAQ

**Can I use this package in a browser?**

This package is not designed to be used in a browser environment. Using this package requires sensitive API KEYS that should not be exposed publicly.
