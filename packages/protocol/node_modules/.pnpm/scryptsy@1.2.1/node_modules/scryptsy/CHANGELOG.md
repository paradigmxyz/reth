1.2.1 / 2015-03-01
------------------
- now using standard for code formatting
- now using `pbkdf2` module over `pbkdf2-sha256`, huge performance increase in Node

1.2.0 / 2014-12-11
------------------
- upgraded `pbkdf2-sha256` from `1.0.1` to `1.1.0`
- removed `browser` field for `crypto`; not-necessary anymore

1.1.0 / 2014-07-28
------------------
- added `progressCallback` (Nadav Ivgi / #4)[https://github.com/cryptocoinjs/scryptsy/pull/4]

1.0.0 / 2014-06-10
------------------
- moved tests to fixtures
- removed semilcolons per http://cryptocoinjs.com/about/contributing/#semicolons
- changed `module.exports.scrypt = funct..` to `module.exports = funct...`
- removed `terst` from dev deps
- upgraded `"pbkdf2-sha256": "~0.1.1"` to `"pbkdf2-sha256": "^1.0.1"`
- added `crypto-browserify` dev dep for `pbkdf2-sha256` tests
- added TravisCI
- added Coveralls
- added testling

0.2.0 / 2014-03-05
------------------
- made a lot of scrypt functions internal along with variables to make thread safe

0.1.0 / 2014-02-18
------------------
- changed spacing from 4 to 2
- removed unneeded JavaScript implementations. Using `pbkdf2-sha256` dep now.
- add browser test support
- convert from `Array` to typed arrays and `Buffer`

0.0.1 / 2014-02-18
------------------
- initial release. Forked from https://github.com/cheongwy/node-scrypt-js and added tests.
