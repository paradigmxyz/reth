pragma solidity >=0.4.21 <0.8.0;

import "../assets/RelativePathImport.sol";
import "package/NodeModulesImport.sol";

contract UsesImports is RelativePathImport, NodeModulesImport {

  constructor() public {}

  function wrapsRelativePathMethod() public {
    isRelativePathMethod();
  }

  function wrapsNodeModulesMethod() public {
    isNodeModulesMethod();
  }
}