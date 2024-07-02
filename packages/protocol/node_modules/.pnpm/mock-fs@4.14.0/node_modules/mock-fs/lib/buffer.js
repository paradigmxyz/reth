exports.from =
  Buffer.from ||
  function(value, encoding) {
    if (encoding) {
      return new Buffer(value, encoding);
    }
    return new Buffer(value);
  };

exports.alloc =
  Buffer.alloc ||
  function(size) {
    return new Buffer(size);
  };
