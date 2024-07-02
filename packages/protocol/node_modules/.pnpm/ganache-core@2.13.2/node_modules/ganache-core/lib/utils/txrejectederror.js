var inherits = require("util").inherits;

// raised when the transaction is rejected prior to running it in the EVM.
function TXRejectedError(message) {
  // Why not just Error.apply(this, [message])? See
  // https://gist.github.com/justmoon/15511f92e5216fa2624b#anti-patterns
  Error.captureStackTrace(this, this.constructor);
  this.name = this.constructor.name;
  this.message = message;
}

inherits(TXRejectedError, Error);

module.exports = TXRejectedError;
