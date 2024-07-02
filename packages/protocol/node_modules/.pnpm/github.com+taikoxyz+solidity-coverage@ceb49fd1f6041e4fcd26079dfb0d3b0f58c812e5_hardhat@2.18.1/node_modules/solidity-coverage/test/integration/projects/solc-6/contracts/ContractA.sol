pragma solidity ^0.6.0;

import "./ContractB.sol";

/**
 * New syntaxes in solc 0.6.x
 */
contract ContractA is ContractB {
  uint counter;
  uint errorCount;

  uint private immutable _a = 100;
  uint private immutable override _b = 100;

  modifier overridden() override {
    require(true);
    _;
  }

  constructor() public {
  }

  function simpleSet(uint i)
    public
    override(ContractB)
  {
    counter = counter + i;
  }

  function simpleView(uint i)
    view
    overridden
    external
    returns (uint, bool)
  {
    return (counter + i, true);
  }

  function tryCatch() public returns (uint, bool) {
    try this.simpleView(5) returns (uint, bool) {
        return (2, true);
    } catch Error(string memory) {
        errorCount++;
        return (0, false);
    } catch (bytes memory) {
        errorCount = errorCount + 1;
        return (0, false);
    }
  }

  function arraySlice(uint _a, uint b_) public pure {
    abi.decode(msg.data[4:], (uint, uint));
  }

  function payableFn() public pure {
    address x;
    //address y = payable(x); // parser-diligence crashing here...
  }
}

// Making sure same-file inheritance works for solc-6...
contract ContractC is ContractA {
  function simpleC(uint x) public {
    x++;
  }
}
