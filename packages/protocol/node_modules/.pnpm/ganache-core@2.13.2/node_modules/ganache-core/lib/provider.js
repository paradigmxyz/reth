// make sourcemaps work!
require("source-map-support/register");

const ProviderEngine = require("web3-provider-engine");
const SubscriptionSubprovider = require("web3-provider-engine/subproviders/subscriptions");

const RequestFunnel = require("./subproviders/requestfunnel");
const DelayedBlockFilter = require("./subproviders/delayedblockfilter");
const GethDefaults = require("./subproviders/gethdefaults");
const GethApiDouble = require("./subproviders/geth_api_double");

const BlockTracker = require("./block_tracker");

const RuntimeError = require("./utils/runtimeerror");
const EventEmitter = require("events");

const _ = require("lodash");

function Provider(options) {
  const self = this;
  EventEmitter.call(this);

  this.options = options = self._applyDefaultOptions(options || {});

  const gethApiDouble = new GethApiDouble(options, this);

  this.engine = new ProviderEngine({
    blockTracker: new BlockTracker({ blockchain: gethApiDouble.state.blockchain })
  });

  const subscriptionSubprovider = new SubscriptionSubprovider();

  this.engine.manager = gethApiDouble;
  this.engine.addProvider(new RequestFunnel());
  this.engine.addProvider(new DelayedBlockFilter());
  this.engine.addProvider(subscriptionSubprovider);
  this.engine.addProvider(new GethDefaults());
  this.engine.addProvider(gethApiDouble);

  this.engine.setMaxListeners(100);
  this.engine.start();

  this.manager = gethApiDouble;
  this.sendAsync = this.send.bind(this);
  this.send = this.send.bind(this);
  this.close = this.close.bind(this);
  this._queueRequest = this._queueRequest.bind(this);
  this._processRequestQueue = this._processRequestQueue.bind(this);

  subscriptionSubprovider.on("data", function(err, notification) {
    self.emit("data", err, notification);
  });
}

const defaultOptions = {
  _chainId: 1,
  _chainIdRpc: 1337,
  vmErrorsOnRPCResponse: true,
  verbose: false,
  asyncRequestProcessing: false,
  logger: {
    log: function() {}
  }
};

Provider.prototype = Object.create(EventEmitter.prototype);
Provider.prototype.constructor = Provider;

Provider.prototype._applyDefaultOptions = function(options) {
  return _.merge({}, defaultOptions, options);
};

Provider.prototype.send = function(payload, callback) {
  if (typeof callback !== "function") {
    throw new Error(
      "No callback provided to provider's send function. As of web3 1.0, provider.send " +
        "is no longer synchronous and must be passed a callback as its final argument."
    );
  }

  const self = this;

  const externalize = function(payload) {
    return _.cloneDeep(payload);
  };

  if (Array.isArray(payload)) {
    payload = payload.map(externalize);
  } else {
    payload = externalize(payload);
  }

  const intermediary = function(err, result) {
    // clone result so that we can mutate the response without worrying about
    // that messing up assumptions the calling logic might have about us
    // mutating things
    result = _.cloneDeep(result);
    let response;
    if (Array.isArray(result)) {
      response = [];
      for (let i = 0; i < result.length; i++) {
        response.push(self.reportErrorInResponse(payload[i], err, result[i]));
      }
    } else {
      response = self.reportErrorInResponse(payload, err, result);
    }

    if (self.options.verbose) {
      self.options.logger.log(
        " <   " +
          JSON.stringify(response, null, 2)
            .split("\n")
            .join("\n <   ")
      );
    }
    process.nextTick(() => callback(response.error ? err : null, response));
  };

  if (self.options.verbose) {
    self.options.logger.log(
      "   > " +
        JSON.stringify(payload, null, 2)
          .split("\n")
          .join("\n   > ")
    );
  }

  if (self.options.asyncRequestProcessing) {
    self.engine.sendAsync(payload, intermediary);
  } else {
    self._queueRequest(payload, intermediary);
  }
};

Provider.prototype.close = function(callback) {
  // This is a little gross reaching, but...
  this.manager.state.stopMining();
  this.manager.state.blockchain.close(callback);
  this.engine.stop();
};

Provider.prototype._queueRequest = function(payload, intermediary) {
  if (!this._requestQueue) {
    this._requestQueue = [];
  }

  this._requestQueue.push({
    payload: payload,
    callback: intermediary
  });

  setImmediate(this._processRequestQueue);
};

Provider.prototype._processRequestQueue = function() {
  const self = this;

  if (self._requestInProgress) {
    return;
  }

  self._requestInProgress = true;

  const args = self._requestQueue.shift();

  if (args) {
    self.engine.sendAsync(args.payload, (err, result) => {
      if (self._requestQueue.length > 0) {
        setImmediate(self._processRequestQueue);
      }
      args.callback(err, result);
      self._requestInProgress = false;
    });
  } else {
    // still need to free the lock
    self._requestInProgress = false;

    if (self._requestQueue.length > 0) {
      setImmediate(self._processRequestQueue);
    }
  }
};

Provider.prototype.cleanUpErrorObject = function(err, response) {
  // Our response should already have an error field at this point, if it
  // doesn't, this was likely intentional. If not, this is the wrong place to
  // fix that problem.
  if (!err || !response.error) {
    return response;
  }

  const errorObject = {
    error: {
      data: {}
    }
  };

  if (err.message) {
    // clean up the error reporting done by the provider engine so the error message isn't lost in the stack trace noise
    errorObject.error.message = err.message;
    errorObject.error.data.stack = err.stack;
    errorObject.error.data.name = err.name;
    if ("code" in err) {
      errorObject.error.code = err.code;
    }
  } else if (!response.error) {
    errorObject.error = {
      message: err.toString()
    };
  }

  return _.merge(response, errorObject);
};

// helper set of RPC methods which execute code and respond with a transaction hash as their result
const transactionMethods = new Set(["eth_sendTransaction", "eth_sendRawTransaction", "personal_sendTransaction"]);

Provider.prototype._isTransactionRequest = function(request) {
  return transactionMethods.has(request.method);
};

Provider.prototype.reportErrorInResponse = function(request, err, response) {
  const self = this;

  if (!err) {
    return response;
  }

  // TODO: for next major release: move reporting of tx hash on error to error
  // field to prevent poorly-written clients which assume that the existence of
  // the "result" field implies no errors from breaking.
  if (self._isTransactionRequest(request)) {
    if (err instanceof RuntimeError) {
      // Make sure we always return the transaction hash on failed transactions so
      // the caller can get their tx receipt. This breaks JSONRPC 2.0, but it's how
      // we've always done it.
      response.result = err.hashes[0];

      if (self.options.vmErrorsOnRPCResponse) {
        if (!response.error.data) {
          response.error.data = {};
        }
        response.error.data[err.hashes[0]] = err.results[err.hashes[0]];
      } else {
        delete response.error;
      }
    }
  }

  if (request.method === "eth_call") {
    if (err instanceof RuntimeError) {
      if (self.options.vmErrorsOnRPCResponse) {
        if (!response.error.data) {
          response.error.data = {};
        }
        response.error.data[err.hashes[0]] = err.results[err.hashes[0]];
      } else {
        response.result = err.results[err.hashes[0]].return || "0x";
        delete response.error;
      }
    }
  }

  return self.cleanUpErrorObject(err, response);
};

module.exports = Provider;
