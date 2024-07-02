# IDNA-UTS #46 in JavaScript

This is a maintained fork of the idna-uts46 library originally written by jcranmer.

The [JS Punycode converter library](https://github.com/bestiejs/punycode.js/) is
a great tool for handling Unicode domain names, but it only implements the
Punycode encoding of domain labels, not the full IDNA algorithm. In simple
cases, a mere conversion to lowercase text before input would seem sufficient,
but the real mapping for strings is far more complex. This library implements
the full mapping for these strings, as defined by
[UTS #46](http://unicode.org/reports/tr46/).


## Install

npm install idna-uts46-hx --save

## IDNA mess for dummies

Unfortunately, the situation of internationalized domain names is rather
complicated by the existence of multiple incompatible standards (IDNA2003 and
IDNA2008, predominantly). While UTS #46 tries to bridge the incompatibility,
there are four characters which cannot be so bridged: ß (the German sharp s),
ς (Greek final sigma), and the ZWJ and ZWNJ characters. These are handled
differently depending on the mode; in ``transitional`` mode, these strings are
mapped to different ones, preserving capability with IDNA2003; in
``nontransitional`` mode, these strings are mapped to themselves, in accordance
with IDNA2008.

Presently, this library uses ``transitional`` mode, compatible with all known
browser implementations at this point. It is expected that, in the future, this
will be changed to ``nontransitional`` mode.

`It is highly recommended that you use the ASCII form of the label for storing
or comparing strings.`

## API

### `uts46.toAscii(domain, options={transitional: false, useStd3ASCII: false, verifyDnsLength: false })`

Converts a domain name to the correct ASCII label. The second parameter is an
optional options parameter, which has two configurable options. The
`transitional` option controls whether or not transitional processing (see the
IDNA mess for dummies section for more details) is requested, defaulting to
false. The `useStd3ASCII` option controls whether or not characters that are
illegal in domain names per the DNS specification should be omitted. The
`verifyDnsLength` option controls whether or not the resulting DNS label should
be checked for length validity (i.e., no empty components and not too long). The
options parameter and its associated fields are all optional and should be
omitted for most users.

```js
uts46.toAscii('öbb.at'); // 'xn-bb-eka.at'
uts46.toAscii('ÖBB.AT'); // 'xn-bb-eka.at'
uts46.toAscii('XN-BB-EKA.AT'); // 'xn-bb-eka.at'
uts46.toAscii('faß.de'); // 'fass.de'
uts46.toAscii('faß.de', {transitional: true}); // 'fass.de'
uts46.toAscii('faß.de', {transitional: false}); // 'xn--fa-hia.de'
uts46.toAscii('xn--fa-hia.de', {transitional: false}); // 'xn--fa-hia.de'
uts46.toAscii(String.fromCodePoint(0xd0000)); // Error (as it is unassigned)
```

### `uts46.toUnicode(domain, options={useStd3ASCII: false})`

Converts a domain name to a normalized Unicode label. The second parameter is an
optional options parameter. The `useStd3ASCII` option controls whether or not
characters that are illegal in domain names per the DNS specification should be
omitted. The latter parameter is optional and should be omitted for most users.

```js
uts46.toUnicode('xn-bb-eka.at'); // 'öbb.at'
uts46.toUnicode('ÖBB.AT'); // 'öbb.at'
uts46.toUnicode('O\u0308BB.AT'); // 'öbb.at'
uts46.toUnicode('faß.de'); // 'faß.de'
uts46.toUnicode('xn--fa-hia.de'); // 'faß.de'
uts46.toUnicode('﷼'); // "ریال"
uts46.toUnicode(String.fromCodePoint(0xd0000)); // Error (as it is unassigned)
```

## Pull latest idna-map.js
Call the below python script by providing the most current RELEASED unicode version.
The latest released version can be found in here: http://www.unicode.org/Public/UCD/latest/ReadMe.txt
e.g.:

```bash
python build-unicode-tables.py 10.0.0
```

## Known issues

It also does not try to implement the Bidi and contextual rules for validation:
these do not affect any mapping of the domain names; instead, they restrict the
set of valid domain names. Since registrars shouldn't be accepting these names
in the first place, a domain that violates these rules will simply fail to
resolve.
