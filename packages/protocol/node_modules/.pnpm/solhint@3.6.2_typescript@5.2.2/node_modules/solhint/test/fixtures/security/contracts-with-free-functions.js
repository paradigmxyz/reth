const CONTRACTS_FREE_FUNCTIONS_ERRORS_2 = `
function freeAa() returns(bool) {
    return true;
}
contract A {
    // error here
    function functionPublicViewA() returns (uint256) {
        return 1;
    }
}

function freeBb() returns(bool) {
    return true;
}
contract B {
    // error here
    function functionPublicViewA() returns (uint256) {
        return 1;
    }

    function functionPublicPureB() internal pure returns (bool) {
        return true;
    }
}
`

const CONTRACT_FREE_FUNCTIONS_ERRORS_1 = `
contract A {
    constructor() {}

    // error here
    function functionPublicViewA() returns (uint256) {
        return 1;
    }
}

function freeBb() returns(bool) {
    return true;
}
`

const NOCONTRACT_FREE_FUNCTION_ERRORS_0 = `
    // NO error here
    function functionPublicViewA() returns (uint256) {
        return 1;
    }
`

const CONTRACT_FREE_FUNCTIONS_ERRORS_0 = `
function freeBb() returns(bool) {
    return true;
}

contract A {
    constructor() {}

    // NO error here
    function functionPublicViewA() external returns (uint256) {
        return 1;
    }
}
`

module.exports = {
  CONTRACTS_FREE_FUNCTIONS_ERRORS_2,
  CONTRACT_FREE_FUNCTIONS_ERRORS_1,
  NOCONTRACT_FREE_FUNCTION_ERRORS_0,
  CONTRACT_FREE_FUNCTIONS_ERRORS_0,
}
