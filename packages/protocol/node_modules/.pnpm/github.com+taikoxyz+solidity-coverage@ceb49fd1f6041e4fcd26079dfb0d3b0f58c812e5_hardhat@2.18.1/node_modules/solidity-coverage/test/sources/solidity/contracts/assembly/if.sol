pragma solidity ^0.7.0;

contract Test {

  function isWhitelisted
  (
    address _address
  )
    public
    view
    returns (bool _isWhitelisted)
  {
    bytes4 _signature = bytes4(keccak256("whitelisted(address)"));
    address _whitelistContract = address(this);

    assembly {
      let _pointer := mload(0x40)  // Set _pointer to free memory pointer
      mstore(_pointer, _signature) // Store _signature at _pointer
      mstore(add(_pointer, 0x04), _address) // Store _address at _pointer. Offset by 4 bytes for previously stored _signature

      // staticcall(g, a, in, insize, out, outsize) => returns 0 on error, 1 on success
      let result := staticcall(
        gas(),              // g = gas: whatever was passed already
        _whitelistContract, // a = address: _whitelist address assigned from getContractAddress()
        _pointer,           // in = mem in  mem[in..(in+insize): set to _pointer pointer
        0x24,               // insize = mem insize  mem[in..(in+insize): size of signature (bytes4) + bytes32 = 0x24
        _pointer,           // out = mem out  mem[out..(out+outsize): output assigned to this storage address
        0x20                // outsize = mem outsize  mem[out..(out+outsize): output should be 32byte slot (bool size = 0x01 < slot size 0x20)
      )

      // Revert if not successful
      if iszero(result) {
        revert(0, 0)
      }

      _isWhitelisted := mload(_pointer) // Assign result to returned value
      mstore(0x40, add(_pointer, 0x24)) // Advance free memory pointer by largest _pointer size
    }
  }
}

