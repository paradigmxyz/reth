'use strict';

const uvBinding = process.binding('uv');
/**
 * Error codes from libuv.
 * @enum {number}
 */
const codes = {};

if (uvBinding.errmap) {
  // nodejs v8+
  uvBinding.errmap.forEach(function(value, errno) {
    const code = value[0];
    const message = value[1];
    codes[code] = {errno: errno, message: message};
  });
} else {
  // nodejs v4 and v6
  Object.keys(uvBinding).forEach(function(key) {
    if (key.startsWith('UV_')) {
      const code = key.slice(3);
      const errno = uvBinding[key];
      codes[code] = {errno: errno, message: key};
    }
  });
}

/**
 * Create an error.
 * @param {string} code Error code.
 * @param {string} path Path (optional).
 * @constructor
 */
function FSError(code, path) {
  if (!codes.hasOwnProperty(code)) {
    throw new Error('Programmer error, invalid error code: ' + code);
  }
  Error.call(this);
  const details = codes[code];
  let message = code + ', ' + details.message;
  if (path) {
    message += " '" + path + "'";
  }
  this.message = message;
  this.code = code;
  this.errno = details.errno;
  if (path !== undefined) {
    this.path = path;
  }
  Error.captureStackTrace(this, FSError);
}
FSError.prototype = new Error();
FSError.codes = codes;

/**
 * Error constructor.
 */
exports = module.exports = FSError;
