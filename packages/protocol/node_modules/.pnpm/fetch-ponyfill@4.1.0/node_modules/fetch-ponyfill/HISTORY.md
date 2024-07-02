# History

## 4.1.0

 - Bumps node fetch from ~1.6.0 to ~1.7.1.
 - Bumps whatwg-fetch from ~2.0.1 to ~2.0.3.

## 4.0.0

This release:

 - Bumps whatwg-fetch from ~1.0.0 to ~2.0.1.
 - Better handling of self/this for browser fetch (more testing friendly).

## 3.0.2

Dependencies now use tilde to allow patch versions to be tracked (this was
waiting for whatwg-fetch to reach version 1).

## 3.0.1

A link was added to the README to point to the ponyfill definition.

## 3.0.0

Fixes an issue with detection of features like `URLSearchParams`. This is a
major version bump since apparent behaviour could change in a breaking way in
browsers which support detected features.

## 2.0.0

Now exposes associated constructors along with `fetch` like:

```javascript
const {fetch, Request, Response, Headers} = require('fetch-ponyfill')(options);
```
