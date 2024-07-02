pragma solidity ^0.7.0;

contract Events {
    uint x = 0;
    bool a;
    bool b;
    event LogEventOne( uint x, address y);
    event LogEventTwo( uint x, address y);

    function test(uint val) public {
        // Assert / Require events
        require(true);

        // Contract Events
        emit LogEventOne(100, msg.sender);
        x = x + val;
        emit LogEventTwo(200, msg.sender);

        // Branch events
        if (true) {
            a = false;
        } else {
            b = false;
        }
    }

    function getX() public view returns (uint){
        return x;
    }
}