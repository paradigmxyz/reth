var fs = require("fs-extra");

var files = require("./files.js");

var os = require("os");

var path = require("path");

var child_process = require("child_process");

var mimetype = require('mime-types');

var defaultArchives = require("./../archives/archives.json");

var requester = require("xhr-request");

var downloadUrl = "http://ethereum-mist.s3.amazonaws.com/swarm/";

var bytes = require("eth-lib/lib/bytes");

var hash = require("./swarm-hash.js");

var pick = require("./pick.js");

var swarm = require("./swarm"); // Fixes issue that causes xhr-request-promise on Node.js to only accept Buffer


var request = function request(url, params, callback) {
  var newParams = {};

  for (var key in params) {
    newParams[key] = params[key];
  }

  if (typeof newParams.body !== "undefined") {
    newParams.body = newParams.body instanceof Buffer ? newParams.body : new Buffer(newParams.body);
  }

  return requester(url, newParams, callback);
};

module.exports = swarm({
  fs: fs,
  files: files,
  os: os,
  path: path,
  child_process: child_process,
  defaultArchives: defaultArchives,
  mimetype: mimetype,
  request: request,
  downloadUrl: downloadUrl,
  bytes: bytes,
  hash: hash,
  pick: pick
});