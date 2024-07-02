pragma solidity ^0.7.0;


contract Contract_ternary {

  // Sameline consequent
  function a() public {
      bool x = true;
      bool y = true;
      x && y ? y = false : y = false;
  }

  // Multiline consequent
  function b() public {
      bool x = false;
      bool y = false;
      (x)
          ? y = false
          : y = false;
  }

  // Sameline w/ logicalOR
  function c() public {
      bool x = false;
      bool y = true;
      (x || y) ? y = false : y = false;
  }

  // Multiline w/ logicalOR
  function d() public {
      bool x = false;
      bool y = true;
      (x || y)
        ? y = false
        : y = false;
  }

  // Sameline alternate
  function e() public {
      bool x = false;
      bool y = false;
      (x) ? y = false : y = false;
  }

  // Multiline w/ logicalOR (both false)
  function f() public {
      bool x = false;
      bool y = false;
      (x || y)
        ? y = false
        : y = false;
  }

}
