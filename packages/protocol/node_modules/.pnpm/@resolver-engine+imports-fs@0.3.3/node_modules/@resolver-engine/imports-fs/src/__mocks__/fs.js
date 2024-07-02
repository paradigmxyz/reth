// In a freshly-mocked file system, current working directory doesn't exist.
// We are changing directory to the only one that exists on the mounted fs.
process.chdir("/");
module.exports = require("memfs");
