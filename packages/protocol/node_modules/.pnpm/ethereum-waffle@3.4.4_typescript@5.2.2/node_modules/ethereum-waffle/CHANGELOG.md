# ethereum-waffle

## 3.4.0

### Minor Changes

- 80d215b: - Fix vulnerabilities shown by `yarn audit`
  - Fix typings in `closeTo` matcher
  - Add `flattenSingleFile` function to compiler

### Patch Changes

- Updated dependencies [80d215b]
  - @ethereum-waffle/compiler@3.4.0
  - @ethereum-waffle/chai@3.4.0
  - @ethereum-waffle/mock-contract@3.3.0
  - @ethereum-waffle/provider@3.4.0

## 3.3.0

### Minor Changes

- 1d7b466: Fix changeTokenBalance and changeTokenBalances matchers for contracts with overloaded balanceOf

  New matchers for BigNumber: within and closeTo

  Typechain integration

  Fix revertedWith functionality

### Patch Changes

- 246281f: Allow contract deployment using TypeChain-generated types
- Updated dependencies [246281f]
- Updated dependencies [1d7b466]
  - @ethereum-waffle/compiler@3.3.0
  - @ethereum-waffle/chai@3.3.0
  - @ethereum-waffle/provider@3.3.0
