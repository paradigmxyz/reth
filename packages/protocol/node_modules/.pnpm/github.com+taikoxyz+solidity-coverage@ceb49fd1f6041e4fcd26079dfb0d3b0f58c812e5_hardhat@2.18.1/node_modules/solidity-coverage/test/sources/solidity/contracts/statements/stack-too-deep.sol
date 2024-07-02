pragma solidity ^0.7.0;

contract Test {
    // 15 fn args + 1 local variable assignment
    // will normally compile w/out stack too deep
    // error.
    function a(
      uint _a,
      uint _b,
      uint _c,
      uint _d,
      uint _e,
      uint _f,
      uint _g,
      uint _h,
      uint _i,
      uint _j,
      uint _k,
      uint _l,
      uint _m,
      uint _n,
      uint _o
    ) public {
      uint x = _a;
    }
}
