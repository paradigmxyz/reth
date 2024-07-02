const unavailable = () => { throw "This swarm.js function isn't available on the browser."; }

const fs = {readFile: unavailable};
const files = {download: unavailable, safeDownloadArchived: unavailable, directoryTree: unavailable};
const os = {platform: unavailable, arch: unavailable};
const path = {join: unavailable, slice: unavailable};
const child_process = {spawn: unavailable};
const mimetype = {lookup: unavailable};
const defaultArchives = {};
const downloadUrl = null;
const request = require("xhr-request");
const bytes = require("eth-lib/lib/bytes");
const hash = require("./swarm-hash.js");
const pick = require("./pick.js");
const swarm = require("./swarm");

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
