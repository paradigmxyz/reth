const fs = require("fs-extra");
const files = require("./files.js");
const os = require("os");
const path = require("path");
const child_process = require("child_process");
const mimetype = require('mime-types');
const defaultArchives = require("./../archives/archives.json");
const requester = require("xhr-request");
const downloadUrl = "http://ethereum-mist.s3.amazonaws.com/swarm/";
const bytes = require("eth-lib/lib/bytes");
const hash = require("./swarm-hash.js");
const pick = require("./pick.js");
const swarm = require("./swarm");

// Fixes issue that causes xhr-request-promise on Node.js to only accept Buffer
const request = (url, params, callback) => {
  let newParams = {};
  for (let key in params) {
    newParams[key] = params[key];
  }
  if (typeof newParams.body !== "undefined") {
    newParams.body = newParams.body instanceof Buffer
      ? newParams.body
      : new Buffer(newParams.body);
  }
  return requester(url, newParams, callback);
};

module.exports = swarm({
  fs,
  files,
  os,
  path,
  child_process,
  defaultArchives,
  mimetype,
  request,
  downloadUrl,
  bytes,
  hash,
  pick
});
