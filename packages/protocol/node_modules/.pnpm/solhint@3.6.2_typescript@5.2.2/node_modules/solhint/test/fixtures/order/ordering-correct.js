module.exports = [
  {
    description: 'All units are in order - ^0.4.0',
    code: `
pragma solidity ^0.4.0;

import "./some/library.sol";
import "./some/other-library.sol";

enum MyEnum {
  Foo,
  Bar
}

struct MyStruct {
  uint x;
  uint y;
}

interface IBox {
  function getValue() public;
  function setValue(uint) public;
}

library MyLibrary {
  function add(uint a, uint b, uint c) public returns (uint) {
    return a + b + c;
  }
}

contract MyContract {
  struct InnerStruct {
    bool flag;
  }

  enum InnerEnum {
    A, B, C
  }

  uint public x;
  uint public y;

  event MyEvent(address a);

  constructor () public {}

  fallback () external {}

  function myExternalFunction() external {}
  function myExternalConstantFunction() external constant {}

  function myPublicFunction() public {}
  function myPublicConstantFunction() public constant {}

  function myInternalFunction() internal {}
  function myPrivateFunction() private {}
}
`,
  },
  {
    description: 'All units are in order - ^0.5.0',
    code: `
pragma solidity ^0.5.0;

import "./some/library.sol";
import "./some/other-library.sol";

enum MyEnum {
  Foo,
  Bar
}

struct MyStruct {
  uint x;
  uint y;
}

interface IBox {
  function getValue() public;
  function setValue(uint) public;
}

library MyLibrary {
  function add(uint a, uint b, uint c) public returns (uint) {
    return a + b + c;
  }
}

contract MyContract {
  using MyLibrary for uint;

  struct InnerStruct {
    bool flag;
  }

  enum InnerEnum {
    A, B, C
  }

  address payable owner;
  uint public x;
  uint public y;

  event MyEvent(address a);

  modifier onlyOwner {
    require(
      msg.sender == owner,
      "Only owner can call this function."
    );
    _;
  }

  constructor () public {}

  fallback () external {}

  function myExternalFunction() external {}
  function myExternalViewFunction() external view {}
  function myExternalPureFunction() external pure {}

  function myPublicFunction() public {}
  function myPublicViewFunction() public view {}
  function myPublicPureFunction() public pure {}

  function myInternalFunction() internal {}
  function myInternalViewFunction() internal view {}
  function myInternalPureFunction() internal pure {}

  function myPrivateFunction() private {}
  function myPrivateViewFunction() private view {}
  function myPrivatePureFunction() private pure {}
}
`,
  },
  {
    description: 'All units are in order - ^0.6.0',
    code: `
pragma solidity ^0.6.0;

import "./some/library.sol";
import "./some/other-library.sol";

enum MyEnum {
  Foo,
  Bar
}

struct MyStruct {
  uint x;
  uint y;
}

interface IBox {
  function getValue() public;
  function setValue(uint) public;
}

library MyLibrary {
  function add(uint a, uint b, uint c) public returns (uint) {
    return a + b + c;
  }
}

contract MyContract {
  using MyLibrary for uint;

  struct InnerStruct {
    bool flag;
  }

  enum InnerEnum {
    A, B, C
  }

  address payable owner;
  uint public x;
  uint public y;

  event MyEvent(address a);

  modifier onlyOwner {
    require(
      msg.sender == owner,
      "Only owner can call this function."
    );
    _;
  }

  constructor () public {}

  receive() external payable {}

  fallback () external {}

  function myExternalFunction() external {}
  function myExternalViewFunction() external view {}
  function myExternalPureFunction() external pure {}

  function myPublicFunction() public {}
  function myPublicViewFunction() public view {}
  function myPublicPureFunction() public pure {}

  function myInternalFunction() internal {}
  function myInternalViewFunction() internal view {}
  function myInternalPureFunction() internal pure {}

  function myPrivateFunction() private {}
  function myPrivateViewFunction() private view {}
  function myPrivatePureFunction() private pure {}
}
`,
  },
]
