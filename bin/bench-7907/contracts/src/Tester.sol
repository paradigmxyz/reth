contract Tester {
    uint256 public deployed;

    function deployContracts(uint256[] calldata sizes) public {
        for (uint256 i = 0; i < sizes.length; i++) {
            uint256 size = sizes[i];
            bytes memory creationCode = bytes.concat(
                hex"7F", // PUSH32
                abi.encode(deployed),
                hex"5F", // PuSH0
                hex"52", // MSTORE(0,deployed)
                hex"7F", // PUSH32
                abi.encode(size),
                hex"5F", // PUSH0
                hex"F3" // RETURN(0, size)
            );
            assembly {
                let result := create(0, add(creationCode, 0x20), mload(creationCode))
                if iszero(result) {
                    revert(0, 0)
                }
            }
            deployed++;
        }
    }

    function call(address[] memory accounts) public {
        for (uint256 i = 0; i < accounts.length; i++) {
            accounts[i].call("");
        }
    }
}

contract TestTester {
    function test() public {
        uint256[] memory sizes = new uint256[](100);
        for (uint256 i = 0; i < 100; i++) {
            sizes[i] = 15000;
        }
        Tester tester = new Tester();
        tester.deployContracts(sizes);
    }
}