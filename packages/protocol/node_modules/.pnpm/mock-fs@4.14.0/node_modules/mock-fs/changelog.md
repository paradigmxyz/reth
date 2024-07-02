# Change Log

## 4.14.0

 * Attempt to fix logging when using `mock-fs` with `tape` (see [#322][#322])
 * Minor fix for `bypass()` (thanks @3cp, see [#320][#320])

## 4.13.0

 * Make `process.chdir()`, `process.cwd()`, and `fs.createWriteStream()` work with `bypass()` (thanks @3cp, see [#307][#307])
 * Fix memory leak associated with Node 10 (thanks #3cp, see [#303][#303])
 * Async function handling in `bypass()` (see [#306][#306])
 * Big new feature!  Temporarily bypass the mocked filesystem with the `bypass()` function (thanks @nonara, see [#304][#304])

## 4.12.0

 * Check permissions in `readdir` and `readdirSync` (thanks @warpdesign, see [#295][#295])
 * Add support for `Buffer` arg on many `fs` functions (thanks @3cp, see [#293][#293])
 * Fix checks for read permission (thanks @3cp, see [#289][#289])
 * Correct error code for `readFile` on a directory (thanks @3cp, see [#286][#286])

## 4.11.0

 * Support `withFileTypes` when reading directories (thanks @mrmlnc, see [#287][#287])

## 4.10.4

 * Throw ENOTDIR when trying to open an incorrect path (thanks @niieani, see [#282][#282])
 * Fix maybeCallback return (thanks @3cp, see [#281][#281])

## 4.10.3

 * Fix bad promise rejection on some fs.promises methods (thanks @3cp, see [#279][#279])

## 4.10.2

 * Fix timestamps for Node > 12.10 (thanks @3cp, see [#277][#277])

## 4.10.1

 * Fix for `fs.mkdir` with the `recursive` option on existing directories (thanks @3cp, see [#271][#271])

## 4.10.0

 * Support the `recursive` option for `fs.mkdir` (thanks @3cp, see [#268][#268])

## 4.9.0

 * Improve `readFile` support for Node 10+ (thanks @huochunpeng, see [#265][#265])
 * Updated dev dependencies (see [#267][#267])

## 4.8.0

 * Fix compatibility issues with Node 10 and 11 (thanks @huochunpeng [#260][#260])
 * Support experimental `fs.promises` (thanks @huochunpeng [#260][#260])

## 4.7.0

 * Fix for readdir on Node 10.10 (thanks @maxwellgerber [#251][#251])
 * Fix for reading and writing using Uint8Array (thanks @maxwellgerber [#249][#249])
 * Document how to properly restore the fs with Jest snapshot testing (thanks @tricoder42 [#247][#247])
 * More informative error when opening a directory (thanks @maxwellgerber [#242][#242])

## 4.6.0

Note that the mocked fs.createReadStream and fs.createWriteStream are not working properly with Node 10.5+.

 * Implement binding.copyFile ([#243][#243])
 * Stat fixes for Node 10.5 (thanks @tomhughes, see [#241][#241])

## 4.5.0

 * Updates for Node 10 compatibility ([#237][#237])
 * Throw ENOENT in readlink when item is missing (thanks @deployable, see [#232][#232])
 * Add path to errors when it exists (thanks @deployable, see [#230][#230])

## 4.4.2

 * Throw if item content is invalid in config (thanks @mutantcornholio, see [#221][#221])
 * Use const in readme (thanks @denar90, see [#222][#222])

## 4.4.1

 * Document that tests are run on Node 8.x as well.

## 4.4.0

 * Fix stat issue with Node 8 (thanks @AGrzes, see [#209][#209])
 * Make code prettier (see [#210][#210])

## 4.3.0

 * Add support for fs.mkdtemp() and fs.mkdtempSync (see [#207][#207])

## 4.2.0

 * Update fs.stat(), fs.lstat(), and fs.fstat() for Node 7.7+ (thanks @not-an-aardvark, see [#198][#198])

## 4.1.0

 * Correctly follow a symlink chain in `binding.open()` (thanks @vlindhol, see [#195][#195])

## 4.0.0

In earlier versions of `mock-fs`, a monkey-patched version of the `fs` module was used to provide an in-memory filesystem.  With each major release of Node, the `mock-fs` package needed to include a modified copy of the `fs` module.  With the `mock-fs@4` release, this package no longer includes a modified copy of the `fs` module.  Instead, this package overrides `process.binding('fs')`.  While this is not part of Node's stable API, it has proven to be a more stable interface than the `fs` module itself (based on experience implementing it with Node 0.8 through 7.0).

Upgrading from 3.x to 4.0 should be straightforward for most applications.  There are several breaking changes that may be restored in future releases:

 * The `mock.fs()` function has been removed.
 * The object created by `fs.stat()` and friends is no longer an instance of `fs.Stats` (though it behaves as one).
 * Lazy `require()` calls do not work consistently.

Detailed changes:

 * Only override `process.binding('fs')` ([#182][#182])
 * Expose the root of the mocked filesystem (thanks @ciaranj, see [#194][#194])

## 3.12.1

 * Revert the require cache clearing behavior ([#181][#181]).

## 3.12.0

 * Support for Node 7.x (thanks @goliney, see [#174][#174]).
 * Remove calls to `printDeprecation` ([#175][#175]).
 * Break early when checking version (thanks @isiahmeadows, see [#157][#157]).
 * Add a note about restoring `fs` (thanks @matheuss, see [#147][#147]).
 * Clear the require cache before overriding fs functions ([#141][#141])

## 3.11.0

 * Make `require()` calls use the real filesystem ([#139][#139]).
 * Reduce the manual `fs` module patching ([#140][#140]).

## 3.10.0

 * Fixes for Node 6.3 ([#138][#138]).
 * Fix permissions issues on directories (thanks @as3richa, see [#105][#105]).

## 3.9.0

 * Support for Node 6.x (thanks @tmcw, see [#107][#107]).

## 3.8.0

 * Implement `binding.writeBuffers()` (see [#94][#94]).

## 3.7.0

 * Add support for `fs.access()` and `fs.accessSync()` (thanks @shyiko, see [#78][#78] and [#80][#80]).

## 3.6.0

 * Add `createCwd` and `createTmp` options to control the creation of `process.cwd()` and `os.tmpdir()` directories in the mocked filesystem (see [#72][#72]).
 * Update Travis and AppVeyor configurations (see [#73][#73])
 * Remove unused dev dependency (see [#75][#75])

## 3.5.0

 * Support for Node 5.x (thanks @tmcw, see [#69][#69]).

## 3.4.0

 * Support for Node 4.x (thanks @AlexMeah, see [#65][#65]).

## 3.3.0

 * Traverse symlinks recursively (thanks @caitp, see [#57][#57]).
 * Upgrade to rewire@2.3.4 (thanks @mbarlock, see [#60][#60]).

## 3.2.0

 * Support for io.js 3.0 (thanks @JustBlackBird, see [#61][#61]).

## 3.1.0

 * Follow symlinks in `readdir()` and `readdirSync()` (thanks @caitp, see [#56][#56]).

## 3.0.0

 * Override `process.cwd()` and `process.chdir()` to work with mocked filesystem (thanks @timkendrick, see [#41][#41]).
 * Add note about known incompatibilities (thanks @psalaets, see [#45][#45]).

## 2.7.0

 * Support for io.js 2.0 (thanks @jwilsson, see [#38][#38]).

## 2.6.0

 * Add `birthtime` to `Stats` objects (thanks @meandmycode, see [#33][#33]).

## 2.5.0

 * Support for io.js 1.1 (thanks @andrewblond, see [#21][#21]).
 * Testing on Windows with AppVeyor (thanks @andrewblond, see [#22][#22]).

## 2.4.0

 * Support for Node 0.12 (thanks @mlegenhausen, see [#18][#18]).

## 2.3.1

 * Preserve arity of callbacks (see [#11][#11]).

## 2.3.0

 * Fixes for Node 0.11.13 (see [#9][#9]).

## 2.2.0

 * Respect file mode on POSIX-compliant systems (see [#7][#7]).
 * Add benchmarks comparing mock-fs and fs modules (see [#6][#6]).

## 2.1.2

 * Added more complete license text.
 * Test on Node 0.9 and 0.11 in addition to 0.8 and 0.10.

## 2.1.1

 * Added this changelog.
 * Removed unused gruntfile.js.

## 2.1.0

 * Directory mtime is now updated when items are added, removed, or modified ([#2][#2]).
 * Fixed several issues on Windows (see [#3][#3]).  One issue remains on Windows with Node 0.8 (see [#4][#4]).
 * Swapped out Grunt with a single script to run tasks (see [#5][#5]).

## 2.0.0

 * Simplified API (see [#1][#1]).


[#1]: https://github.com/tschaub/mock-fs/pull/1
[#2]: https://github.com/tschaub/mock-fs/pull/2
[#3]: https://github.com/tschaub/mock-fs/pull/3
[#4]: https://github.com/tschaub/mock-fs/issues/4
[#5]: https://github.com/tschaub/mock-fs/pull/5
[#6]: https://github.com/tschaub/mock-fs/pull/6
[#7]: https://github.com/tschaub/mock-fs/pull/7
[#9]: https://github.com/tschaub/mock-fs/issues/9
[#11]: https://github.com/tschaub/mock-fs/pull/11
[#18]: https://github.com/tschaub/mock-fs/pull/18
[#21]: https://github.com/tschaub/mock-fs/pull/21
[#22]: https://github.com/tschaub/mock-fs/pull/22
[#33]: https://github.com/tschaub/mock-fs/pull/33
[#38]: https://github.com/tschaub/mock-fs/pull/38
[#41]: https://github.com/tschaub/mock-fs/pull/41
[#45]: https://github.com/tschaub/mock-fs/pull/45
[#56]: https://github.com/tschaub/mock-fs/pull/56
[#61]: https://github.com/tschaub/mock-fs/pull/61
[#60]: https://github.com/tschaub/mock-fs/pull/60
[#57]: https://github.com/tschaub/mock-fs/pull/57
[#65]: https://github.com/tschaub/mock-fs/pull/65
[#69]: https://github.com/tschaub/mock-fs/pull/69
[#72]: https://github.com/tschaub/mock-fs/pull/72
[#73]: https://github.com/tschaub/mock-fs/pull/73
[#75]: https://github.com/tschaub/mock-fs/pull/75
[#78]: https://github.com/tschaub/mock-fs/pull/78
[#80]: https://github.com/tschaub/mock-fs/pull/80
[#94]: https://github.com/tschaub/mock-fs/pull/94
[#107]: https://github.com/tschaub/mock-fs/pull/107
[#105]: https://github.com/tschaub/mock-fs/pull/105
[#138]: https://github.com/tschaub/mock-fs/pull/138
[#139]: https://github.com/tschaub/mock-fs/pull/139
[#140]: https://github.com/tschaub/mock-fs/pull/140
[#141]: https://github.com/tschaub/mock-fs/pull/141
[#147]: https://github.com/tschaub/mock-fs/pull/147
[#157]: https://github.com/tschaub/mock-fs/pull/157
[#174]: https://github.com/tschaub/mock-fs/pull/174
[#175]: https://github.com/tschaub/mock-fs/pull/175
[#181]: https://github.com/tschaub/mock-fs/pull/181
[#182]: https://github.com/tschaub/mock-fs/pull/182
[#194]: https://github.com/tschaub/mock-fs/pull/194
[#195]: https://github.com/tschaub/mock-fs/pull/195
[#198]: https://github.com/tschaub/mock-fs/pull/198
[#207]: https://github.com/tschaub/mock-fs/pull/207
[#209]: https://github.com/tschaub/mock-fs/pull/209
[#210]: https://github.com/tschaub/mock-fs/pull/210
[#221]: https://github.com/tschaub/mock-fs/pull/221
[#222]: https://github.com/tschaub/mock-fs/pull/222
[#230]: https://github.com/tschaub/mock-fs/pull/230
[#232]: https://github.com/tschaub/mock-fs/pull/232
[#237]: https://github.com/tschaub/mock-fs/pull/237
[#243]: https://github.com/tschaub/mock-fs/pull/243
[#242]: https://github.com/tschaub/mock-fs/pull/242
[#247]: https://github.com/tschaub/mock-fs/pull/247
[#249]: https://github.com/tschaub/mock-fs/pull/249
[#251]: https://github.com/tschaub/mock-fs/pull/251
[#260]: https://github.com/tschaub/mock-fs/pull/260
[#265]: https://github.com/tschaub/mock-fs/pull/265
[#267]: https://github.com/tschaub/mock-fs/pull/267
[#268]: https://github.com/tschaub/mock-fs/pull/268
[#271]: https://github.com/tschaub/mock-fs/pull/271
[#277]: https://github.com/tschaub/mock-fs/pull/277
[#279]: https://github.com/tschaub/mock-fs/pull/279
[#281]: https://github.com/tschaub/mock-fs/pull/281
[#282]: https://github.com/tschaub/mock-fs/pull/282
[#286]: https://github.com/tschaub/mock-fs/pull/286
[#287]: https://github.com/tschaub/mock-fs/pull/287
[#289]: https://github.com/tschaub/mock-fs/pull/289
[#293]: https://github.com/tschaub/mock-fs/pull/293
[#295]: https://github.com/tschaub/mock-fs/pull/295
[#303]: https://github.com/tschaub/mock-fs/pull/303
[#304]: https://github.com/tschaub/mock-fs/pull/304
[#306]: https://github.com/tschaub/mock-fs/pull/306
[#307]: https://github.com/tschaub/mock-fs/pull/307
[#320]: https://github.com/tschaub/mock-fs/pull/320
[#322]: https://github.com/tschaub/mock-fs/pull/322
