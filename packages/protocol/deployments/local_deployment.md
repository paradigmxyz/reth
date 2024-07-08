# How to deploy Gwyneth locally

1. Run anvil and grab an address and PK it provides with premined ETH.
2. Paste one PK and ADDR pair from anvil output to .env file and set the correct corresponding (PRIVATE_KEY and MAINNET_CONTRACT_OWNER) variables.
3. Run script:
```shell
$ forge script --rpc-url  http://127.0.0.1:8545 scripts/DeployL1OnAnvil.s.sol -vvvv --broadcast --private-key <YOUR_PRIVATE_KEY>
```

Important: <YOUR_PRIVATE_KEY> shall be the same PK as you set in the ENV file.