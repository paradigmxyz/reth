// SPDX-License-Identifier: MIT
pragma solidity >=0.8.0 <0.9.0;
pragma abicoder v2;

error InvalidSomeAddress(address someAddress);

contract ContractA {

    address public someAddress;

    function throwError(address _add) external {
        this;

        if (_add == address(0)) {
          revert InvalidSomeAddress(_add);
        }

        someAddress = _add;
    }

    function checkSomething() external {
      uint a = 5;

      unchecked {
        a++;
      }

      unchecked {}
    }
}
