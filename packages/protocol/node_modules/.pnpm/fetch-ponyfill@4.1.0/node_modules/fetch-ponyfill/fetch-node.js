'use strict';

var fetch = require('node-fetch');

function wrapFetchForNode(fetch) {
  // Support schemaless URIs on the server for parity with the browser.
  // https://github.com/matthew-andrews/isomorphic-fetch/pull/10
  return function (u, options) {
    if (u instanceof fetch.Request) {
      return fetch(u, options);
    }

    var url = u.slice(0, 2) === '//' ? 'https:' + u : u;

    return fetch(url, options);
  };
}

module.exports = function (context) {
  // This modifies the global `node-fetch` object, which isn't great, since
  // different callers to `fetch-ponyfill` which pass a different Promise
  // implementation would each expect to have their implementation used. But,
  // given the way `node-fetch` is implemented, this is the only way to make
  // it work at all.
  if (context && context.Promise) {
    fetch.Promise = context.Promise;
  }

  return {
    fetch: wrapFetchForNode(fetch),
    Headers: fetch.Headers,
    Request: fetch.Request,
    Response: fetch.Response
  };
};
