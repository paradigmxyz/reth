pragma solidity ^0.7.0;


contract Contract_OR {

  function _if(uint i) public pure {
    if (i == 0 || i > 5){
      /* ignore */
    }
  }

  function _if_and(uint i) public pure {
    if (i != 0 && (i < 2 || i > 5)){
      /* ignore */
    }
  }

  function _return(uint i) public pure returns (bool){
    return (i != 0 && i != 1 ) ||
           ((i + 1) == 2);
  }

  function _while(uint i) public pure returns (bool){
    uint counter;
    while( (i == 1 || i == 2) && counter < 2 ){
      counter++;
    }
  }

  function _require(uint x) public {
      require(x == 1 || x == 2);
  }

  function _require_multi_line(uint x) public {
      require(
        (x == 1 || x == 2) ||
         x == 3
      );
  }

  function _if_neither(uint i) public {
      if (i == 1 || i == 2){
        /* ignore */
      }
  }
}
