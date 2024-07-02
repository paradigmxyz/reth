var linkBytecode = function (bytecode, libraries) {
  // NOTE: for backwards compatibility support old compiler which didn't use file names
  var librariesComplete = {};
  for (var libraryName in libraries) {
    if (typeof libraries[libraryName] === 'object') {
      // API compatible with the standard JSON i/o
      for (var lib in libraries[libraryName]) {
        librariesComplete[lib] = libraries[libraryName][lib];
        librariesComplete[libraryName + ':' + lib] = libraries[libraryName][lib];
      }
    } else {
      // backwards compatible API for early solc-js verisons
      var parsed = libraryName.match(/^([^:]*):?(.*)$/);
      if (parsed) {
        librariesComplete[parsed[2]] = libraries[libraryName];
      }
      librariesComplete[libraryName] = libraries[libraryName];
    }
  }

  for (libraryName in librariesComplete) {
    // truncate to 37 characters
    var internalName = libraryName.slice(0, 36);
    // prefix and suffix with __
    var libLabel = '__' + internalName + Array(37 - internalName.length).join('_') + '__';

    var hexAddress = librariesComplete[libraryName];
    if (hexAddress.slice(0, 2) !== '0x' || hexAddress.length > 42) {
      throw new Error('Invalid address specified for ' + libraryName);
    }
    // remove 0x prefix
    hexAddress = hexAddress.slice(2);
    hexAddress = Array(40 - hexAddress.length + 1).join('0') + hexAddress;

    while (bytecode.indexOf(libLabel) >= 0) {
      bytecode = bytecode.replace(libLabel, hexAddress);
    }
  }

  return bytecode;
};

var findLinkReferences = function (bytecode) {
  // find 40 bytes in the pattern of __...<36 digits>...__
  // e.g. __Lib.sol:L_____________________________
  var linkReferences = {};
  var offset = 0;
  while (true) {
    var found = bytecode.match(/__(.{36})__/);
    if (!found) {
      break;
    }

    var start = found.index;
    // trim trailing underscores
    // NOTE: this has no way of knowing if the trailing underscore was part of the name
    var libraryName = found[1].replace(/_+$/gm, '');

    if (!linkReferences[libraryName]) {
      linkReferences[libraryName] = [];
    }

    linkReferences[libraryName].push({
      // offsets are in bytes in binary representation (and not hex)
      start: (offset + start) / 2,
      length: 20
    });

    offset += start + 20;

    bytecode = bytecode.slice(start + 20);
  }
  return linkReferences;
};

module.exports = {
  linkBytecode: linkBytecode,
  findLinkReferences: findLinkReferences
};
