const ONE_CONTRACT = `
    pragma solidity 0.8.0;
    
    contract A {
        uint256 public constant TESTA = "testA";
    }
    `

const TWO_CONTRACTS = `
    pragma solidity 0.8.0;
    
    contract A {
        uint256 public constant TESTA = "testA";
    }

    contract B {
        uint256 public constant TESTB = "testB";
    }
    `

const THREE_CONTRACTS = `
    pragma solidity 0.8.0;
    
    contract A {
        uint256 public constant TESTA = "testA";
    }

    contract B {
        uint256 public constant TESTB = "testB";
    }

    contract C {
        uint256 public constant TESTC = "testC";
    }
    `
module.exports = { ONE_CONTRACT, TWO_CONTRACTS, THREE_CONTRACTS }
