# JSON RPC 2.0 Error

Error constructors for JSON RPC 2.0 errors as described in the
[JSON-RPC 2.0 Error Specification]
(http://www.jsonrpc.org/specification#error_object)

| code             | message          | meaning                                                                                               |
|------------------|------------------|-------------------------------------------------------------------------------------------------------|
| -32700           | Parse error      | Invalid JSON was received by the server. An error occurred on the server while parsing the JSON text. |
| -32600           | Invalid Request  | The JSON sent is not a valid Request object.                                                          |
| -32601           | Method not found | The method does not exist / is not available.                                                         |
| -32602           | Invalid params   | Invalid method parameter(s).                                                                          |
| -32603           | Internal error   | Internal JSON-RPC error.                                                                              |
| -32000 to -32099 | Server error     | Reserved for implementation-defined server-errors.                                                    |

Specific errors are instances of the base constructor `JsonRpcError`, which in
turn is an instance of the native JavaScript Error object.

Each error can be constructed with or without the `new` keyword, for example

`var err = new JsonRpcError.ParseError();`

is the same as

`var err = JsonRpcError.ParseError();`

Also see related packages [json-rpc-response](https://github.com/claudijo/json-rpc-response),
[json-rpc-request](https://github.com/claudijo/json-rpc-request), and
[json-rpc-notification](https://github.com/claudijo/json-rpc-notification)

## Usage

Import the JSON RPC 2.0 error module:

```js
var JsonRpcError = require('json-rpc-error');
```

### JsonRpcError
General base constructor for JSON RPC 2 errors:

```js
new JsonRpcError(message, code[, data]);
```

### Parse error
Invalid JSON was received by the server.

```js
new JsonRpcError.ParseError();
```

### Invalid Request
The JSON sent is not a valid Request object.

```js
new JsonRpcError.InvalidRequest();
```

### Method not found
The method does not exist / is not available.

```js
new JsonRpcError.MethodNotFound();
```

### Invalid params
Invalid method parameter(s).

```js
new JsonRpcError.InvalidParams();
```

### Internal error
Internal JSON-RPC error. The constructor can take an optional error object, in
which case the error's `message` property will be passed on.

```js
new JsonRpcError.InternalError([error]);
```

### Server Error
Reserved for implementation-defined server-errors. Provided error code must be
in the range -32000 to -32099.

```js
new JsonRpcError.ServerError(code);
```

## Test

Run unit tests:

`$ npm test`

Create test coverage report:

`$ npm run-script test-cov`

# License

[MIT](LICENSE)


