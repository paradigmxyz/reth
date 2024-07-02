// make sourcemaps work!
require("source-map-support/register");

const Provider = require("./lib/provider");
const Server = require("./lib/server");

// This interface exists so as not to cause breaking changes.
module.exports = {
  server: function(options) {
    return Server.create(options);
  },
  provider: function(options) {
    return new Provider(options);
  },
  _webpacked: true
};
