#!/usr/bin/env python2

# This file builds a mapping of utility tables for handling UTR#46 IDNA
# processing. You may find the rules here:
# <http://www.unicode.org/reports/tr46/index.html>.

import json
import re
import sys
import urllib2

# NUM_UCHAR is the number of Unicode characters there are.
NUM_UCHAR = 0x10ffff + 1

def download_unicode(version):
    idna_tables = "http://www.unicode.org/Public/idna/" + version
    infd = urllib2.urlopen(idna_tables + "/IdnaTest.txt")
    #with open("test/test-idna2.js", "w") as outfd:
    #    build_test_code(infd, outfd)
    with open("test/IdnaTest.txt", "w") as outfd:
        for line in infd:
            outfd.write(line)
    infd.close()
    infd = urllib2.urlopen(idna_tables + "/IdnaMappingTable.txt")
    dgc = urllib2.urlopen("http://www.unicode.org/Public/" + version
            + "/ucd/extracted/DerivedGeneralCategory.txt")
    with open("idna-map.js", "w") as outfd:
        build_unicode_map(infd, outfd, dgc)
    infd.close()

def parse_unicode_data_file(fd):
    '''Yield a generator of (start, end, fields) for the given Unicode data
    file. These files are of the same basic format: a semicolon-delimited set
    of columns, where the first column is either a single element or a range of
    characters. In this case, the range implied by start and end are
    inclusive.'''
    for line in fd:
        pos = line.find('#')
        if pos >= 0: line = line[:pos]
        line = line.strip()
        if not line: continue
        parts = [p.strip() for p in line.split(';')]

        stend = map(lambda x: int(x, 16), parts[0].split('..'))
        if len(stend) == 1:
            start = end = stend[0]
        else:
            start, end = stend
        yield start, end, tuple(parts[1:])
    fd.close()

def utf16len(string):
    return sum(2 if ord(c) > 0xffff else 1 for c in string)

class MappedValue(object):
    def __init__(self, parts):
        self.flags = 0
        self.rule = parts[0]
        # If there are two parts, the second part is the mapping in question.
        if len(parts) > 1 and parts[1]:
            self.chars = ''.join(map(lambda u: unichr(int(u, 16)),
                parts[1].split(' ')))
        else:
            self.chars = ''

        # In the case of disallowed_STD3_*, we process the real rule as the
        # text following the last _, and set a flag noting to note the
        # difference.
        if self.rule.startswith('disallowed_STD3'):
            self.flags |= 1
            self.rule = self.rule.split('_')[-1]

    def build_map_string(self, string):
        self.index = 0
        if self.chars:
            self.index = string.find(self.chars)
            if self.index < 0:
                self.index = utf16len(string)
                string = string + self.chars
            else:
                self.index = utf16len(string[0:self.index])
        return string

    def build_int(self):
        if self.rule == 'disallowed':
            status = 0
        elif self.rule == 'ignored':
            status = 1 # We're mapping to a string of length 0
        elif self.rule == 'mapped':
            status = 1
        elif self.rule == 'deviation':
            status = 2
        elif self.rule == 'valid':
            status = 3
        else:
            raise Exception("Unknown rule " + self.rule)

        # Sanity check all the bits
        assert self.flags < (1 << 2)
        assert self.index < (1 << 16)
        numchars = utf16len(self.chars)
        assert numchars < (1 << 5)

        return self.flags << 23 | status << 21 | self.index << 5 | numchars


def build_unicode_map(idnaMapTable, out, derivedGeneralCategory):
    unicharMap = [0] * NUM_UCHAR
    vals = []
    for start, end, parts in parse_unicode_data_file(idnaMapTable):
        for ch in range(start, end + 1):
            value = MappedValue(parts)
            vals.append(value)
            unicharMap[ch] = value

    # Note which characters have the combining mark property.
    for start, end, parts in parse_unicode_data_file(derivedGeneralCategory):
        if parts[0] in ('Mc', 'Mn', 'Me'):
            for ch in range(start, end + 1):
                unicharMap[ch].flags |= 2

    # Build up the string to use to map the output
    vals.sort(cmp=lambda x, y: cmp(len(x.chars), len(y.chars)), reverse=True)
    mappedStr = reduce(lambda s, v: v.build_map_string(s), vals, u"")

    # Convert this to integers
    unicharMap = map(lambda v: v.build_int(), unicharMap)

    # We're going to do a funky special case here. Since planes 3-17 are
    # basically unused, we're going to divert these from the standard two-phase
    # table lookup and use hardcoded JS for this code. Ensure that the values
    # are what we would write in the code.
    # (The special case here is that the variation selections, in plane 14, are
    # set to ignored, not disallowed).
    specialCase = unicharMap[0xe0100]
    for ch in range(0x30000, len(unicharMap)):
        assert unicharMap[ch] == 0 or (
                unicharMap[ch] == specialCase and
                (0xe0100 <= ch and ch <= 0xe01ef))

    mem, lg_block_size, blocks = min(find_block_sizes(unicharMap[:0x30000]))
    block_size = 1 << lg_block_size
    blocks = list(blocks)
    out.write("/* This file is generated from the Unicode IDNA table, using\n")
    out.write("   the build-unicode-tables.py script. Please edit that\n")
    out.write("   script instead of this file. */\n\n")
    out.write("""/* istanbul ignore next */
(function (root, factory) {
  if (typeof define === 'function' && define.amd) {
    define([], function () { return factory(); });
  } else if (typeof exports === 'object') {
    module.exports = factory();
  } else {
    root.uts46_map = factory();
  }
}(this, function () {
""")

    # Emit the blocks
    out.write("var blocks = [\n");
    for block in blocks:
        out.write("  new Uint32Array([%s]),\n" % ','.join(map(str, block)))
    out.write("];\n")

    # Now emit the block index map
    out.write("var blockIdxes = new Uint%dArray([" %
        (8 if len(blocks) < 256 else 16))
    out.write(','.join(str(blocks.index(tuple(unicharMap[i:i + block_size])))
        for i in range(0, 0x30000, block_size)))
    out.write(']);\n')

    # And the string
    out.write("var mappingStr = %s;\n" %
        json.dumps(mappedStr, ensure_ascii=False).encode('utf-8'))

    # Finish off with the function to actually look everything up
    out.write("""
function mapChar(codePoint) {
  if (codePoint >= 0x30000) {
    // High planes are special cased.
    if (codePoint >= 0xE0100 && codePoint <= 0xE01EF)
      return %(codepoint)d;
    return 0;
  }
  return blocks[blockIdxes[codePoint >> %(block_size)d]][codePoint & %(mask)d];
}

return {
  mapStr: mappingStr,
  mapChar: mapChar
};
}));
""" % {'codepoint': unicharMap[0xE0100], 'block_size': lg_block_size,
    'mask': (1 << lg_block_size) - 1})

# The next two functions are helpers to find the block size that minimizes the
# total memory use. Notice that we're being clever in finding memory use by
# noting when we can use Uint8Array versus Uint32Array.
def find_block_sizes(unicharMap):
    for lg_block_size in range(1, 15):
        block_size = 1 << lg_block_size
        memUsage, blocks = compute_block_size(unicharMap, block_size)
        yield memUsage, lg_block_size, blocks

def compute_block_size(unicharMap, block_size):
    blocks = set()
    for i in range(0, len(unicharMap), block_size):
        block = tuple(unicharMap[i:i + block_size])
        blocks.add(block)
    num = len(blocks)
    if num < 256:
        mem = len(unicharMap) / block_size
    elif num < 0x10000:
        mem = 2 * len(unicharMap) / block_size
    else:
        raise Exception("Way too many blocks: %d" % num)
    mem += num * block_size * 4
    return mem, blocks


# All of the following code builds a test suite from the IdnaTest.txt file.

def convert_escape(string):
    return string

# These functions build the test IDNA vectors.
def build_body(mode, test_vector, func, expected):
    lines = []
    if expected[0] == '[':
        if not re.search('[AVP]', expected):
            return []
        if mode == 'T' or mode == 'B':
            lines.append("assert.throws(function () { %s(\"%s\", true); });" %
                (func, test_vector))
        if mode == 'N' or mode == 'B':
            lines.append("assert.throws(function () { %s(\"%s\", false); });" %
                (func, test_vector))
    else:
        if mode == 'T' or mode == 'B':
            lines.append("assert.equal(%s(\"%s\", true), \"%s\");" %
                (func, test_vector, expected))
        if mode == 'N' or mode == 'B':
            lines.append("assert.equal(%s(\"%s\", false), \"%s\");" %
                (func, test_vector, expected))

    return lines

def build_test_code(infd, out):
    out.write("/* This file is generated from the Unicode IDNA table, using\n")
    out.write("   the build-unicode-tables.py script. Please edit that\n")
    out.write("   script instead of this file. */\n\n")
    out.write("var assert = require(\"assert\");\n")
    out.write("var uts46 = require(\"../utr46-gold\");\n\n")
    out.write("""
function toAscii(input, transitional) {
  return uts46.toAscii(input, true, transitional, true);
}
function toUnicode(input, transitional) {
  return uts46.toUnicode(input, true, true);
}\n
""")
    out.write("suite('IdnaTest.txt automated tests', function () {\n")
    for line in infd:
        line = line.split('#')[0].strip()
        if not line:
            continue
        strings = map(lambda x: x.strip(), line.split(';'))
        mode = strings[0]
        test_vector = convert_escape(strings[1])
        unicode_data = convert_escape(strings[2]) or test_vector
        ascii_data = convert_escape(strings[3]) or unicode_data
        tests = []
        tests.extend(build_body(mode, test_vector, 'toUnicode', unicode_data))
        tests.extend(build_body(mode, test_vector, 'toAscii', ascii_data))
        if len(tests) == 0:
            continue
        line = line.replace('\\', '\\\\')
        out.write("  test('%s', function () {\n" % line)
        for test in tests:
            out.write("    %s\n" % test)
        out.write("  });\n")
    out.write("});\n")

if len(sys.argv) != 2:
    sys.stderr.write("Usage: %s <version>" % sys.argv[0])
download_unicode(sys.argv[1])
