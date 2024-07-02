/*
 * Etherscan.io API connector
 * @author github.com/axic
 *
 * The etherscan.io API supports:
 *
 * 1) Natively via proxy methods
 * - eth_blockNumber *
 * - eth_getBlockByNumber *
 * - eth_getBlockTransactionCountByNumber
 * - getTransactionByHash
 * - getTransactionByBlockNumberAndIndex
 * - eth_getTransactionCount *
 * - eth_sendRawTransaction *
 * - eth_call *
 * - eth_getTransactionReceipt *
 * - eth_getCode *
 * - eth_getStorageAt *
 *
 * 2) Via non-native methods
 * - eth_getBalance
 * - eth_listTransactions (non-standard)
 */

const xhr = process.browser ? require('xhr') : require('request')
const inherits = require('util').inherits
const Subprovider = require('./subprovider.js')

module.exports = EtherscanProvider

inherits(EtherscanProvider, Subprovider)

function EtherscanProvider(opts) {
  opts = opts || {}
  this.network = opts.network || 'api'
  this.proto = (opts.https || false) ? 'https' : 'http'
  this.requests = [];
  this.times = isNaN(opts.times) ? 4 : opts.times;
  this.interval = isNaN(opts.interval) ? 1000 : opts.interval;
  this.retryFailed = typeof opts.retryFailed === 'boolean' ? opts.retryFailed : true; // not built yet
  
  setInterval(this.handleRequests, this.interval, this);
}

EtherscanProvider.prototype.handleRequests = function(self){
	if(self.requests.length == 0) return;
	
	//console.log('Handling the next ' + self.times + ' of ' + self.requests.length + ' requests');
	
	for(var requestIndex = 0; requestIndex < self.times; requestIndex++) {
		var requestItem = self.requests.shift()
  		
		if(typeof requestItem !== 'undefined')
			handlePayload(requestItem.proto, requestItem.network, requestItem.payload, requestItem.next, requestItem.end)
	}
}

EtherscanProvider.prototype.handleRequest = function(payload, next, end){
  var requestObject = {proto: this.proto, network: this.network, payload: payload, next: next, end: end},
	  self = this;
  
  if(this.retryFailed)
	  requestObject.end = function(err, result){
		  if(err === '403 - Forbidden: Access is denied.')
			 self.requests.push(requestObject);
		  else
			 end(err, result);
		  };
	
  this.requests.push(requestObject);
}

function handlePayload(proto, network, payload, next, end){
  switch(payload.method) {
    case 'eth_blockNumber':
      etherscanXHR(true, proto, network, 'proxy', 'eth_blockNumber', {}, end)
      return

    case 'eth_getBlockByNumber':
      etherscanXHR(true, proto, network, 'proxy', 'eth_getBlockByNumber', {
        tag: payload.params[0],
        boolean: payload.params[1] }, end)
      return

    case 'eth_getBlockTransactionCountByNumber':
      etherscanXHR(true, proto, network, 'proxy', 'eth_getBlockTransactionCountByNumber', {
        tag: payload.params[0]
      }, end)
      return

    case 'eth_getTransactionByHash':
      etherscanXHR(true, proto, network, 'proxy', 'eth_getTransactionByHash', {
        txhash: payload.params[0]
      }, end)
      return

    case 'eth_getBalance':
      etherscanXHR(true, proto, network, 'account', 'balance', {
        address: payload.params[0],
        tag: payload.params[1] }, end)
      return

    case 'eth_listTransactions':
      return (function(){
        const props = [
          'address',
          'startblock',
          'endblock',
          'sort',
          'page',
          'offset'
        ]

        const params = {}
        for (let i = 0, l = Math.min(payload.params.length, props.length); i < l; i++) {
          params[props[i]] = payload.params[i]
        }

        etherscanXHR(true, proto, network, 'account', 'txlist', params, end)
      })()

    case 'eth_call':
      etherscanXHR(true, proto, network, 'proxy', 'eth_call', payload.params[0], end)
      return

    case 'eth_sendRawTransaction':
      etherscanXHR(false, proto, network, 'proxy', 'eth_sendRawTransaction', { hex: payload.params[0] }, end)
      return

    case 'eth_getTransactionReceipt':
      etherscanXHR(true, proto, network, 'proxy', 'eth_getTransactionReceipt', { txhash: payload.params[0] }, end)
      return

    // note !! this does not support topic filtering yet, it will return all block logs
    case 'eth_getLogs':
      return (function(){
        var payloadObject = payload.params[0],
            txProcessed = 0,
            logs = [];

        etherscanXHR(true, proto, network, 'proxy', 'eth_getBlockByNumber', {
          tag: payloadObject.toBlock,
          boolean: payload.params[1] }, function(err, blockResult) {
            if(err) return end(err);

            for(var transaction in blockResult.transactions){
              etherscanXHR(true, proto, network, 'proxy', 'eth_getTransactionReceipt', { txhash: transaction.hash }, function(err, receiptResult) {
                if(!err) logs.concat(receiptResult.logs);
                txProcessed += 1;
                if(txProcessed === blockResult.transactions.length) end(null, logs)
              })
            }
          })
      })()

    case 'eth_getTransactionCount':
      etherscanXHR(true, proto, network, 'proxy', 'eth_getTransactionCount', {
        address: payload.params[0],
        tag: payload.params[1]
      }, end)
      return

    case 'eth_getCode':
      etherscanXHR(true, proto, network, 'proxy', 'eth_getCode', {
        address: payload.params[0],
        tag: payload.params[1]
      }, end)
      return

    case 'eth_getStorageAt':
      etherscanXHR(true, proto, network, 'proxy', 'eth_getStorageAt', {
        address: payload.params[0],
        position: payload.params[1],
        tag: payload.params[2]
      }, end)
      return

    default:
      next();
      return
  }
}

function toQueryString(params) {
  return Object.keys(params).map(function(k) {
    return encodeURIComponent(k) + '=' + encodeURIComponent(params[k])
  }).join('&')
}

function etherscanXHR(useGetMethod, proto, network, module, action, params, end) {
  var uri = proto + '://' + network + '.etherscan.io/api?' + toQueryString({ module: module, action: action }) + '&' + toQueryString(params)
	
  xhr({
    uri: uri,
    method: useGetMethod ? 'GET' : 'POST',
    headers: {
      'Accept': 'application/json',
      // 'Content-Type': 'application/json',
    },
    rejectUnauthorized: false,
  }, function(err, res, body) {
    // console.log('[etherscan] response: ', err)

    if (err) return end(err)
	
	  /*console.log('[etherscan request]' 
				  + ' method: ' + useGetMethod
				  + ' proto: ' + proto
				  + ' network: ' + network
				  + ' module: ' + module
				  + ' action: ' + action
				  + ' params: ' + params
				  + ' return body: ' + body);*/
	
    if(body.indexOf('403 - Forbidden: Access is denied.') > -1)
    	return end('403 - Forbidden: Access is denied.')
	  
    var data
    try {
      data = JSON.parse(body)
    } catch (err) {
      console.error(err.stack)
      return end(err)
    }

    // console.log('[etherscan] response decoded: ', data)

    // NOTE: or use id === -1? (id=1 is 'success')
    if ((module === 'proxy') && data.error) {
      // Maybe send back the code too?
      return end(data.error.message)
    }

    // NOTE: or data.status !== 1?
    if ((module === 'account') && (data.message !== 'OK')) {
      return end(data.message)
    }

    end(null, data.result)
  })
}
