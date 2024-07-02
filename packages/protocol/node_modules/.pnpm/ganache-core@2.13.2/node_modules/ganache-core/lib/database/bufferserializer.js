var utils = require("ethereumjs-util");
var to = require("../utils/to");

module.exports = {
  encode: function(val, cb) {
    var hex = to.hex(val);
    cb(null, hex);
  },
  decode: function(json, cb) {
    cb(null, utils.toBuffer(json));
  }
};
