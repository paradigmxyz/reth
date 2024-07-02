pragma solidity ^0.7.0;

/*
Library and Using statements: invoking 'Test.not' should generate line and statement
coverage for L 9, 10, and 19, plus function coverage for 'flip' and 'not'.
 */
library Boolean {
    struct Value { bool val; }

    function flip(Value storage self) internal returns (bool) {
        self.val = !self.val;
        return self.val;
    }
}

contract Test {
    using Boolean for Boolean.Value;
    Boolean.Value b;

    function not() public returns (bool) {
        return b.flip();
    }
}
