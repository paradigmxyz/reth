ChangeLog
---------

## v0.11.8 at 2022-06-12

Acknowledgements:

 - [Masatoshi Fukunaga](https://github.com/mah0x211) for [Lua bindings](https://github.com/mah0x211/lua-libmdbx).

New:

 - Added most of transactions flags to the public API.
 - Added `MDBX_NOSUCCESS_EMPTY_COMMIT` build option to return non-success result (`MDBX_RESULT_TRUE`) on empty commit.
 - Reworked validation and import of DBI-handles into a transaction.
   Assumes  these changes will be invisible to most users, but will cause fewer surprises in complex DBI cases.
 - Added ability to open DB in without-LCK (exclusive read-only) mode in case no permissions to create/write LCK-file.

Fixes:

 - A series of fixes and improvements for automatically generated documentation (Doxygen).
 - Fixed copy&paste bug with could lead to `SIGSEGV` (nullptr dereference) in the exclusive/no-lck mode.
 - Fixed minor warnings from modern Apple's CLANG 13.
 - Fixed minor warnings from CLANG 14 and in-development CLANG 15.
 - Fixed `SIGSEGV` regression in without-LCK (exclusive read-only) mode.
 - Fixed `mdbx_check_fs_local()` for CDROM case on Windows.
 - Fixed nasty typo of typename which caused false `MDBX_CORRUPTED` error in a rare execution path,
   when the size of the thread-ID type not equal to 8.
 - Fixed write-after-free memory corruption on latest `macOS` during finalization/cleanup of thread(s) that executed read transaction(s).
   > The issue was suddenly discovered by a [CI](https://en.wikipedia.org/wiki/Continuous_integration)
   > after adding an iteration with macOS 11 "Big Sur", and then reproduced on recent release of macOS 12 "Monterey".
   > The issue was never noticed nor reported on macOS 10 "Catalina" nor others.
   > Analysis shown that the problem caused by a change in the behavior of the system library (internals of dyld and pthread)
   > during thread finalization/cleanup: now a memory allocated for a `__thread` variable(s) is released
   > before execution of the registered Thread-Local-Storage destructor(s),
   > thus a TLS-destructor will write-after-free just by legitime dereference any `__thread` variable.
   > This is unexpected crazy-like behavior since the order of resources releasing/destroying
   > is not the reverse of ones acquiring/construction order. Nonetheless such surprise
   > is now workarounded by using atomic compare-and-swap operations on a 64-bit signatures/cookies.
 - Fixed Elbrus/E2K LCC 1.26 compiler warnings (memory model for atomic operations, etc).

Minors:

 - Refined `release-assets` GNU Make target.
 - Added logging to `mdbx_fetch_sdb()` to help debugging complex DBI-handels use cases.
 - Added explicit error message from probe of no-support for `std::filesystem`.
 - Added contributors "score" table by `git fame` to generated docs.
 - Added `mdbx_assert_fail()` to public API (mostly for backtracing).
 - Now C++20 concepts used/enabled only when `__cpp_lib_concepts >= 202002`.
 - Don't provide nor report package information if used as a CMake subproject.


-------------------------------------------------------------------------------

## v0.11.7 at 2022-04-22

The stable risen release after the Github's intentional malicious disaster.

#### We have migrated to a reliable trusted infrastructure
The origin for now is at [GitFlic](https://gitflic.ru/project/erthink/libmdbx)
since on 2022-04-15 the Github administration, without any warning nor
explanation, deleted _libmdbx_ along with a lot of other projects,
simultaneously blocking access for many developers.
For the same reason ~~Github~~ is blacklisted forever.

GitFlic already support Russian and English languages, plan to support more,
including 和 中文. You are welcome!

New:

 - Added the `tools-static` make target to build statically linked MDBX tools.
 - Support for Microsoft Visual Studio 2022.
 - Support build by MinGW' make from command line without CMake.
 - Added `mdbx::filesystem` C++ API namespace that corresponds to `std::filesystem` or `std::experimental::filesystem`.
 - Created [website](https://libmdbx.dqdkfa.ru/) for online auto-generated documentation.
 - Used `https://web.archive.org/web/20220414235959/https://github.com/erthink/` for dead (or temporarily lost) resources deleted by ~~Github~~.
 - Added `--loglevel=` command-line option to the `mdbx_test` tool.
 - Added few fast smoke-like tests into CMake builds.

Fixes:

 - Fixed a race between starting a transaction and creating a DBI descriptor that could lead to `SIGSEGV` in the cursor tracking code.
 - Clarified description of `MDBX_EPERM` error returned from `mdbx_env_set_geometry()`.
 - Fixed non-promoting the parent transaction to be dirty in case the undo of the geometry update failed during abortion of a nested transaction.
 - Resolved linking issues with `libstdc++fs`/`libc++fs`/`libc++experimental` for C++ `std::filesystem` or `std::experimental::filesystem` for legacy compilers.
 - Added workaround for GNU Make 3.81 and earlier.
 - Added workaround for Elbrus/LCC 1.25 compiler bug of class inline `static constexpr` member field.
 - [Fixed](https://github.com/ledgerwatch/erigon/issues/3874) minor assertion regression (only debug builds were affected).
 - Fixed detection of `C++20` concepts accessibility.
 - Fixed detection of Clang's LTO availability for Android.
 - Fixed extra definition of `_FILE_OFFSET_BITS=64` for Android that is problematic for 32-bit Bionic.
 - Fixed build for ARM/ARM64 by MSVC.
 - Fixed non-x86 Windows builds with `MDBX_WITHOUT_MSVC_CRT=ON` and `MDBX_BUILD_SHARED_LIBRARY=ON`.

Minors:

 - Resolve minor MSVC warnings: avoid `/INCREMENTAL[:YES]` with `/LTCG`, `/W4` with `/W3`, the `C5105` warning.
 - Switched to using `MDBX_EPERM` instead of `MDBX_RESULT_TRUE` to indicate that the geometry cannot be updated.
 - Added `NULL` checking during memory allocation inside `mdbx_chk`.
 - Resolved all warnings from MinGW while used without CMake.
 - Added inheretable `target_include_directories()` to `CMakeLists.txt` for easy integration.
 - Added build-time checks and paranoid runtime assertions for the `off_t` arguments of `fcntl()` which are used for locking.
 - Added `-Wno-lto-type-mismatch` to avoid false-positive warnings from old GCC during LTO-enabled builds.
 - Added checking for TID (system thread id) to avoid hang on 32-bit Bionic/Android within `pthread_mutex_lock()`.
 - Reworked `MDBX_BUILD_TARGET` of CMake builds.
 - Added `CMAKE_HOST_ARCH` and `CMAKE_HOST_CAN_RUN_EXECUTABLES_BUILT_FOR_TARGET`.


-------------------------------------------------------------------------------


## v0.11.6 at 2022-03-24

The stable release with the complete workaround for an incoherence flaw of Linux unified page/buffer cache.
Nonetheless the cause for this trouble may be an issue of Intel CPU cache/MESI.
See [issue#269](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/269) for more information.

Acknowledgements:

 - [David Bouyssié](https://github.com/david-bouyssie) for [Scala bindings](https://github.com/david-bouyssie/mdbx4s).
 - [Michelangelo Riccobene](https://github.com/mriccobene) for reporting and testing.

Fixes:

 - [Added complete workaround](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/269) for an incoherence flaw of Linux unified page/buffer cache.
 - [Fixed](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/272) cursor reusing for read-only transactions.
 - Fixed copy&paste typo inside `mdbx::cursor::find_multivalue()`.

Minors:

 - Minor refine C++ API for convenience.
 - Minor internals refines.
 - Added `lib-static` and `lib-shared` targets for make.
 - Added minor workaround for AppleClang 13.3 bug.
 - Clarified error messages of a signature/version mismatch.


## v0.11.5 at 2022-02-23

The release with the temporary hotfix for a flaw of Linux unified page/buffer cache.
See [issue#269](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/269) for more information.

Acknowledgements:

 - [Simon Leier](https://github.com/leisim) for reporting and testing.
 - [Kai Wetlesen](https://github.com/kaiwetlesen) for [RPMs](http://copr.fedorainfracloud.org/coprs/kwetlesen/libmdbx/).
 - [Tullio Canepa](https://github.com/canepat) for reporting C++ API issue and contributing.

Fixes:

 - [Added hotfix](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/269) for a flaw of Linux unified page/buffer cache.
 - [Fixed/Reworked](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/pull/270) move-assignment operators for "managed" classes of C++ API.
 - Fixed potential `SIGSEGV` while open DB with overrided non-default page size.
 - [Made](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/267) `mdbx_env_open()` idempotence in failure cases.
 - Refined/Fixed pages reservation inside `mdbx_update_gc()` to avoid non-reclamation in a rare cases.
 - Fixed typo in a retained space calculation for the hsr-callback.

Minors:

 - Reworked functions for meta-pages, split-off non-volatile.
 - Disentangled C11-atomic fences/barriers and pure-functions (with `__attribute__((__pure__))`) to avoid compiler misoptimization.
 - Fixed hypotetic unaligned access to 64-bit dwords on ARM with `__ARM_FEATURE_UNALIGNED` defined.
 - Reasonable paranoia that makes clarity for code readers.
 - Minor fixes Doxygen references, comments, descriptions, etc.


## v0.11.4 at 2022-02-02

The stable release with fixes for large and huge databases sized of 4..128 TiB.

Acknowledgements:

 - [Ledgerwatch](https://github.com/ledgerwatch), [Binance](https://github.com/binance-chain) and [Positive Technologies](https://www.ptsecurity.com/) teams for reporting, assistance in investigation and testing.
 - [Alex Sharov](https://github.com/AskAlexSharov) for reporting, testing and provide resources for remote debugging/investigation.
 - [Kris Zyp](https://github.com/kriszyp) for [Deno](https://deno.land/) support.

New features, extensions and improvements:

 - Added treating the `UINT64_MAX` value as maximum for given option inside `mdbx_env_set_option()`.
 - Added `to_hex/to_base58/to_base64::output(std::ostream&)` overloads without using temporary string objects as buffers.
 - Added `--geometry-jitter=YES|no` option to the test framework.
 - Added support for [Deno](https://deno.land/) support by [Kris Zyp](https://github.com/kriszyp).

Fixes:

 - Fixed handling `MDBX_opt_rp_augment_limit` for GC's records from huge transactions (Erigon/Akula/Ethereum).
 - [Fixed](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/258) build on Android (avoid including `sys/sem.h`).
 - [Fixed](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/pull/261) missing copy assignment operator for `mdbx::move_result`.
 - Fixed missing `&` for `std::ostream &operator<<()` overloads.
 - Fixed unexpected `EXDEV` (Cross-device link) error from `mdbx_env_copy()`.
 - Fixed base64 encoding/decoding bugs in auxillary C++ API.
 - Fixed overflow of `pgno_t` during checking PNL on 64-bit platforms.
 - [Fixed](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/260) excessive PNL checking after sort for spilling.
 - Reworked checking `MAX_PAGENO` and DB upper-size geometry limit.
 - [Fixed](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/265) build for some combinations of versions of  MSVC and Windows SDK.

Minors:

 - Added workaround for CLANG bug [D79919/PR42445](https://reviews.llvm.org/D79919).
 - Fixed build test on Android (using `pthread_barrier_t` stub).
 - Disabled C++20 concepts for CLANG < 14 on Android.
 - Fixed minor `unused parameter` warning.
 - Added CI for Android.
 - Refine/cleanup internal logging.
 - Refined line splitting inside hex/base58/base64 encoding to avoid `\n` at the end.
 - Added workaround for modern libstdc++ with CLANG < 4.x
 - Relaxed txn-check rules for auxiliary functions.
 - Clarified a comments and descriptions, etc.
 - Using the `-fno-semantic interposition` option to reduce the overhead to calling self own public functions.


## v0.11.3 at 2021-12-31

Acknowledgements:

 - [gcxfd <i@rmw.link>](https://github.com/gcxfd) for reporting, contributing and testing.
 - [장세연 (Чан Се Ен)](https://github.com/sasgas) for reporting and testing.
 - [Alex Sharov](https://github.com/AskAlexSharov) for reporting, testing and provide resources for remote debugging/investigation.

New features, extensions and improvements:

 - [Added](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/236) `mdbx_cursor_get_batch()`.
 - [Added](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/250) `MDBX_SET_UPPERBOUND`.
 - C++ API is finalized now.
 - The GC update stage has been [significantly speeded](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/254) when fixing huge Erigon's transactions (Ethereum ecosystem).

Fixes:

 - Disabled C++20 concepts for stupid AppleClang 13.x
 - Fixed internal collision of `MDBX_SHRINK_ALLOWED` with `MDBX_ACCEDE`.

Minors:

 - Fixed returning `MDBX_RESULT_TRUE` (unexpected -1) from `mdbx_env_set_option()`.
 - Added `mdbx_env_get_syncbytes()` and `mdbx_env_get_syncperiod()`.
 - [Clarified](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/pull/249) description of `MDBX_INTEGERKEY`.
 - Reworked/simplified `mdbx_env_sync_internal()`.
 - [Fixed](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/248) extra assertion inside `mdbx_cursor_put()` for `MDBX_DUPFIXED` cases.
 - Avoiding extra looping inside `mdbx_env_info_ex()`.
 - Explicitly enabled core dumps from stochastic tests scripts on Linux.
 - [Fixed](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/253) `mdbx_override_meta()` to avoid false-positive assertions.
 - For compatibility reverted returning `MDBX_ENODATA`for some cases.


## v0.11.2 at 2021-12-02

Acknowledgements:

 - [장세연 (Чан Се Ен)](https://github.com/sasgas) for contributing to C++ API.
 - [Alain Picard](https://github.com/castortech) for [Java bindings](https://github.com/castortech/mdbxjni).
 - [Alex Sharov](https://github.com/AskAlexSharov) for reporting and testing.
 - [Kris Zyp](https://github.com/kriszyp) for reporting and testing.
 - [Artem Vorotnikov](https://github.com/vorot93) for support [Rust wrapper](https://github.com/vorot93/libmdbx-rs).

Fixes:

 - [Fixed compilation](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/pull/239) with `devtoolset-9` on CentOS/RHEL 7.
 - [Fixed unexpected `MDBX_PROBLEM` error](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/242) because of update an obsolete meta-page.
 - [Fixed returning `MDBX_NOTFOUND` error](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/243) in case an inexact value found for `MDBX_GET_BOTH` operation.
 - [Fixed compilation](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/245) without kernel/libc-devel headers.

Minors:

 - Fixed `constexpr`-related macros for legacy compilers.
 - Allowed to define 'CMAKE_CXX_STANDARD` using an environment variable.
 - Simplified collection statistics of page operation .
 - Added `MDBX_FORCE_BUILD_AS_MAIN_PROJECT` cmake option.
 - Remove unneeded `#undef P_DIRTY`.


## v0.11.1 at 2021-10-23

### Backward compatibility break:

The database format signature has been changed to prevent
forward-interoperability with an previous releases, which may lead to a
[false positive diagnosis of database corruption](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/238)
due to flaws of an old library versions.

This change is mostly invisible:

 - previously versions are unable to read/write a new DBs;
 - but the new release is able to handle an old DBs and will silently upgrade ones.

Acknowledgements:

 - [Alex Sharov](https://github.com/AskAlexSharov) for reporting and testing.


-------------------------------------------------------------------------------


## v0.10.5 at 2021-10-13 (obsolete, please use v0.11.1)

Unfortunately, the `v0.10.5` accidentally comes not full-compatible with previous releases:

 - `v0.10.5` can read/processing DBs created by previous releases, i.e. the backward-compatibility is provided;
 - however, previous releases may lead to false-corrupted state with DB that was touched by `v0.10.5`, i.e. the forward-compatibility is broken for `v0.10.4` and earlier.

This cannot be fixed, as it requires fixing past versions, which as a result we will just get a current version.
Therefore, it is recommended to use `v0.11.1` instead of `v0.10.5`.

Acknowledgements:

 - [Noel Kuntze](https://github.com/Thermi) for immediately bug reporting.

Fixes:

 - Fixed unaligned access regression after the `#pragma pack` fix for modern compilers.
 - Added UBSAN-test to CI to avoid a regression(s) similar to lately fixed.
 - Fixed possibility of meta-pages clashing after manually turn to a particular meta-page using `mdbx_chk` utility.

Minors:

 - Refined handling of weak or invalid meta-pages while a DB opening.
 - Refined providing information for the `@MAIN` and `@GC` sub-databases of a last committed modification transaction's ID.


## v0.10.4 at 2021-10-10

Acknowledgements:

 - [Artem Vorotnikov](https://github.com/vorot93) for support [Rust wrapper](https://github.com/vorot93/libmdbx-rs).
 - [Andrew Ashikhmin](https://github.com/yperbasis) for contributing to C++ API.

Fixes:

 - Fixed possibility of looping update GC during transaction commit (no public issue since the problem was discovered inside [Positive Technologies](https://www.ptsecurity.ru)).
 - Fixed `#pragma pack` to avoid provoking some compilers to generate code with [unaligned access](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/235).
 - Fixed `noexcept` for potentially throwing `txn::put()` of C++ API.

Minors:

 - Added stochastic test script for checking small transactions cases.
 - Removed extra transaction commit/restart inside test framework.
 - In debugging builds fixed a too small (single page) by default DB shrink threshold.


## v0.10.3 at 2021-08-27

Acknowledgements:

 - [Francisco Vallarino](https://github.com/fjvallarino) for [Haskell bindings for libmdbx](https://hackage.haskell.org/package/libmdbx).
 - [Alex Sharov](https://github.com/AskAlexSharov) for reporting and testing.
 - [Andrea Lanfranchi](https://github.com/AndreaLanfranchi) for contributing.

Extensions and improvements:

 - Added `cursor::erase()` overloads for `key` and for `key-value`.
 - Resolve minor Coverity Scan issues (no fixes but some hint/comment were added).
 - Resolve minor UndefinedBehaviorSanitizer issues (no fixes but some workaround were added).

Fixes:

 - Always setup `madvise` while opening DB (fixes https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/231).
 - Fixed checking legacy `P_DIRTY` flag (`0x10`) for nested/sub-pages.

Minors:

 - Fixed getting revision number from middle of history during amalgamation (GNU Makefile).
 - Fixed search GCC tools for LTO (CMake scripts).
 - Fixed/reorder dirs list for search CLANG tools for LTO (CMake scripts).
 - Fixed/workarounds for CLANG < 9.x
 - Fixed CMake warning about compatibility with 3.8.2


## v0.10.2 at 2021-07-26

Acknowledgements:

 - [Alex Sharov](https://github.com/AskAlexSharov) for reporting and testing.
 - [Andrea Lanfranchi](https://github.com/AndreaLanfranchi) for reporting bugs.
 - [Lionel Debroux](https://github.com/debrouxl) for fuzzing tests and reporting bugs.
 - [Sergey Fedotov](https://github.com/SergeyFromHell/) for [`node-mdbx` NodeJS bindings](https://www.npmjs.com/package/node-mdbx).
 - [Kris Zyp](https://github.com/kriszyp) for [`lmdbx-store` NodeJS bindings](https://github.com/kriszyp/lmdbx-store).
 - [Noel Kuntze](https://github.com/Thermi) for [draft Python bindings](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/commits/python-bindings).

New features, extensions and improvements:

 - [Allow to predefine/override `MDBX_BUILD_TIMESTAMP` for builds reproducibility](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/201).
 - Added options support for `long-stochastic` script.
 - Avoided `MDBX_TXN_FULL` error for large transactions when possible.
 - The `MDBX_READERS_LIMIT` increased to `32767`.
 - Raise `MDBX_TOO_LARGE` under Valgrind/ASAN if being opened DB is 100 larger than RAM (to avoid hangs and OOM).
 - Minimized the size of poisoned/unpoisoned regions to avoid Valgrind/ASAN stuck.
 - Added more workarounds for QEMU for testing builds for 32-bit platforms, Alpha and Sparc architectures.
 - `mdbx_chk` now skips iteration & checking of DB' records if corresponding page-tree is corrupted (to avoid `SIGSEGV`, ASAN failures, etc).
 - Added more checks for [rare/fuzzing corruption cases](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/217).

Backward compatibility break:

 - Use file `VERSION.txt` for version information instead of `VERSION` to avoid collision with `#include <version>`.
 - Rename `slice::from/to_FOO_bytes()` to `slice::envisage_from/to_FOO_length()'.
 - Rename `MDBX_TEST_EXTRA` make's variable to `MDBX_SMOKE_EXTRA`.
 - Some details of the C++ API have been changed for subsequent freezing.

Fixes:

 - Fixed excess meta-pages checks in case `mdbx_chk` is called to check the DB for a specific meta page and thus could prevent switching to the selected meta page, even if the check passed without errors.
 - Fixed [recursive use of SRW-lock on Windows cause by `MDBX_NOTLS` option](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/203).
 - Fixed [log a warning during a new DB creation](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/205).
 - Fixed [false-negative `mdbx_cursor_eof()` result](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/207).
 - Fixed [`make install` with non-GNU `install` utility (OSX, BSD)](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/208).
 - Fixed [installation by `CMake` in special cases by complete use `GNUInstallDirs`'s variables](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/209).
 - Fixed [C++ Buffer issue with `std::string` and alignment](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/191).
 - Fixed `safe64_reset()` for platforms without atomic 64-bit compare-and-swap.
 - Fixed hang/shutdown on big-endian platforms without `__cxa_thread_atexit()`.
 - Fixed [using bad meta-pages if DB was partially/recoverable corrupted](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/217).
 - Fixed extra `noexcept` for `buffer::&assign_reference()`.
 - Fixed `bootid` generation on Windows for case of change system' time.
 - Fixed [test framework keygen-related issue](https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/127).


## v0.10.1 at 2021-06-01

Acknowledgements:

 - [Alexey Akhunov](https://github.com/AlexeyAkhunov) and [Alex Sharov](https://github.com/AskAlexSharov) for bug reporting and testing.
 - [Andrea Lanfranchi](https://github.com/AndreaLanfranchi) for bug reporting and testing related to WSL2.

New features:

 - Added `-p` option to `mdbx_stat` utility for printing page operations statistic.
 - Added explicit checking for and warning about using unfit github's archives.
 - Added fallback from [OFD locking](https://bit.ly/3yFRtYC) to legacy non-OFD POSIX file locks on an `EINVAL` error.
 - Added [Plan 9](https://en.wikipedia.org/wiki/9P_(protocol)) network file system to the whitelist for an ability to open a DB in exclusive mode.
 - Support for opening from WSL2 environment a DB hosted on Windows drive and mounted via [DrvFs](https://docs.microsoft.com/it-it/archive/blogs/wsl/wsl-file-system-support#drvfs) (i.e by Plan 9 noted above).

Fixes:

 - Fixed minor "foo not used" warnings from modern C++ compilers when building the C++ part of the library.
 - Fixed confusing/messy errors when build library from unfit github's archives (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/197).
 - Fixed `#​e​l​s​i​f` typo.
 - Fixed rare unexpected `MDBX_PROBLEM` error during altering data in huge transactions due to wrong spilling/oust of dirty pages (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/195).
 - Re-Fixed WSL1/WSL2 detection with distinguishing (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/97).


## v0.10.0 at 2021-05-09

Acknowledgements:

 - [Mahlon E. Smith](https://github.com/mahlonsmith) for [Ruby bindings](https://rubygems.org/gems/mdbx/).
 - [Alex Sharov](https://github.com/AskAlexSharov) for [mdbx-go](https://github.com/torquem-ch/mdbx-go), bug reporting and testing.
 - [Artem Vorotnikov](https://github.com/vorot93) for bug reporting and PR.
 - [Paolo Rebuffo](https://www.linkedin.com/in/paolo-rebuffo-8255766/), [Alexey Akhunov](https://github.com/AlexeyAkhunov) and Mark Grosberg for donations.
 - [Noel Kuntze](https://github.com/Thermi) for preliminary [Python bindings](https://github.com/Thermi/libmdbx/tree/python-bindings)

New features:

 - Added `mdbx_env_set_option()` and `mdbx_env_get_option()` for controls
   various runtime options for an environment (announce of this feature  was missed in a previous news).
 - Added `MDBX_DISABLE_PAGECHECKS` build option to disable some checks to reduce an overhead
   and detection probability of database corruption to a values closer to the LMDB.
   The `MDBX_DISABLE_PAGECHECKS=1` provides a performance boost of about 10% in CRUD scenarios,
   and conjointly with the `MDBX_ENV_CHECKPID=0` and `MDBX_TXN_CHECKOWNER=0` options can yield
   up to 30% more performance compared to LMDB.
 - Using float point (exponential quantized) representation for internal 16-bit values
   of grow step and shrink threshold when huge ones (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/166).
   To minimize the impact on compatibility, only the odd values inside the upper half
   of the range (i.e. 32769..65533) are used for the new representation.
 - Added the `mdbx_drop` similar to LMDB command-line tool to purge or delete (sub)database(s).
 - [Ruby bindings](https://rubygems.org/gems/mdbx/) is available now by [Mahlon E. Smith](https://github.com/mahlonsmith).
 - Added `MDBX_ENABLE_MADVISE` build option which controls the use of POSIX `madvise()` hints and friends.
 - The internal node sizes were refined, resulting in a reduction in large/overflow pages in some use cases
   and a slight increase in limits for a keys size to ≈½ of page size.
 - Added to `mdbx_chk` output number of keys/items on pages.
 - Added explicit `install-strip` and `install-no-strip` targets to the `Makefile` (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/pull/180).
 - Major rework page splitting (af9b7b560505684249b76730997f9e00614b8113) for
     - An "auto-appending" feature upon insertion for both ascending and
       descending key sequences. As a result, the optimality of page filling
       increases significantly (more densely, less slackness) while
       inserting ordered sequences of keys,
     - A "splitting at middle" to make page tree more balanced on average.
 - Added `mdbx_get_sysraminfo()` to the API.
 - Added guessing a reasonable maximum DB size for the default upper limit of geometry (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/183).
 - Major rework internal labeling of a dirty pages (958fd5b9479f52f2124ab7e83c6b18b04b0e7dda) for
   a "transparent spilling" feature with the gist to make a dirty pages
   be ready to spilling (writing to a disk) without further altering ones.
   Thus in the `MDBX_WRITEMAP` mode the OS kernel able to oust dirty pages
   to DB file without further penalty during transaction commit.
   As a result, page swapping and I/O could be significantly reduced during extra large transactions and/or lack of memory.
 - Minimized reading leaf-pages during dropping subDB(s) and nested trees.
 - Major rework a spilling of dirty pages to support [LRU](https://en.wikipedia.org/wiki/Cache_replacement_policies#Least_recently_used_(LRU))
   policy and prioritization for a large/overflow pages.
 - Statistics of page operations (split, merge, copy, spill, etc) now available through `mdbx_env_info_ex()`.
 - Auto-setup limit for length of dirty pages list (`MDBX_opt_txn_dp_limit` option).
 - Support `make options` to list available build options.
 - Support `make help` to list available make targets.
 - Silently `make`'s build by default.
 - Preliminary [Python bindings](https://github.com/Thermi/libmdbx/tree/python-bindings) is available now
   by [Noel Kuntze](https://github.com/Thermi) (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/147).

Backward compatibility break:

 - The `MDBX_AVOID_CRT` build option was renamed to `MDBX_WITHOUT_MSVC_CRT`.
   This option is only relevant when building for Windows.
 - The `mdbx_env_stat()` always, and `mdbx_env_stat_ex()` when called with the zeroed transaction parameter,
   now internally start temporary read transaction and thus may returns `MDBX_BAD_RSLOT` error.
   So, just never use deprecated `mdbx_env_stat()' and call `mdbx_env_stat_ex()` with transaction parameter.
 - The build option `MDBX_CONFIG_MANUAL_TLS_CALLBACK` was removed and now just a non-zero value of
   the `MDBX_MANUAL_MODULE_HANDLER` macro indicates the requirement to manually call `mdbx_module_handler()`
   when loading libraries and applications uses statically linked libmdbx on an obsolete Windows versions.

Fixes:

 - Fixed performance regression due non-optimal C11 atomics usage (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/160).
 - Fixed "reincarnation" of subDB after it deletion (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/168).
 - Fixed (disallowing) implicit subDB deletion via operations on `@MAIN`'s DBI-handle.
 - Fixed a crash of `mdbx_env_info_ex()` in case of a call for a non-open environment (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/171).
 - Fixed the selecting/adjustment values inside `mdbx_env_set_geometry()` for implicit out-of-range cases (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/170).
 - Fixed `mdbx_env_set_option()` for set initial and limit size of dirty page list ((https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/179).
 - Fixed an unreasonably huge default upper limit for DB geometry (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/183).
 - Fixed `constexpr` specifier for the `slice::invalid()`.
 - Fixed (no)readahead auto-handling (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/164).
 - Fixed non-alloy build for Windows.
 - Switched to using Heap-functions instead of LocalAlloc/LocalFree on Windows.
 - Fixed `mdbx_env_stat_ex()` to returning statistics of the whole environment instead of MainDB only (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/190).
 - Fixed building by GCC 4.8.5 (added workaround for a preprocessor's bug).
 - Fixed building C++ part for iOS <= 13.0 (unavailability of  `std::filesystem::path`).
 - Fixed building for Windows target versions prior to Windows Vista (`WIN32_WINNT < 0x0600`).
 - Fixed building by MinGW for Windows (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/155).


-------------------------------------------------------------------------------


## v0.9.3 at 2021-02-02

Acknowledgements:

 - [Mahlon E. Smith](http://www.martini.nu/) for [FreeBSD port of libmdbx](https://svnweb.freebsd.org/ports/head/databases/mdbx/).
 - [장세연](http://www.castis.com) for bug fixing and PR.
 - [Clément Renault](https://github.com/Kerollmops/heed) for [Heed](https://github.com/Kerollmops/heed) fully typed Rust wrapper.
 - [Alex Sharov](https://github.com/AskAlexSharov) for bug reporting.
 - [Noel Kuntze](https://github.com/Thermi) for bug reporting.

Removed options and features:

 - Drop `MDBX_HUGE_TRANSACTIONS` build-option (now no longer required).

New features:

 - Package for FreeBSD is available now by Mahlon E. Smith.
 - New API functions to get/set various options (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/128):
    - the maximum number of named databases for the environment;
    - the maximum number of threads/reader slots;
    - threshold (since the last unsteady commit) to force flush the data buffers to disk;
    - relative period (since the last unsteady commit) to force flush the data buffers to disk;
    - limit to grow a list of reclaimed/recycled page's numbers for finding a sequence of contiguous pages for large data items;
    - limit to grow a cache of dirty pages for reuse in the current transaction;
    - limit of a pre-allocated memory items for dirty pages;
    - limit of dirty pages for a write transaction;
    - initial allocation size for dirty pages list of a write transaction;
    - maximal part of the dirty pages may be spilled when necessary;
    - minimal part of the dirty pages should be spilled when necessary;
    - how much of the parent transaction dirty pages will be spilled while start each child transaction;
 - Unlimited/Dynamic size of retired and dirty page lists (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/123).
 - Added `-p` option (purge subDB before loading) to `mdbx_load` tool.
 - Reworked spilling of large transaction and committing of nested transactions:
    - page spilling code reworked to avoid the flaws and bugs inherited from LMDB;
    - limit for number of dirty pages now is controllable at runtime;
    - a spilled pages, including overflow/large pages, now can be reused and refunded/compactified in nested transactions;
    - more effective refunding/compactification especially for the loosed page cache.
 - Added `MDBX_ENABLE_REFUND` and `MDBX_PNL_ASCENDING` internal/advanced build options.
 - Added `mdbx_default_pagesize()` function.
 - Better support architectures with a weak/relaxed memory consistency model (ARM, AARCH64, PPC, MIPS, RISC-V, etc) by means [C11 atomics](https://en.cppreference.com/w/c/atomic).
 - Speed up page number lists and dirty page lists (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/132).
 - Added `LIBMDBX_NO_EXPORTS_LEGACY_API` build option.

Fixes:

 - Fixed missing cleanup (null assigned) in the C++ commit/abort (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/pull/143).
 - Fixed `mdbx_realloc()` for case of nullptr and `MDBX_WITHOUT_MSVC_CRT=ON` for Windows.
 - Fixed the possibility to use invalid and renewed (closed & re-opened, dropped & re-created) DBI-handles (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/146).
 - Fixed 4-byte aligned access to 64-bit integers, including access to the `bootid` meta-page's field (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/153).
 - Fixed minor/potential memory leak during page flushing and unspilling.
 - Fixed handling states of cursors's and subDBs's for nested transactions.
 - Fixed page leak in extra rare case the list of retired pages changed during update GC on transaction commit.
 - Fixed assertions to avoid false-positive UB detection by CLANG/LLVM (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/153).
 - Fixed `MDBX_TXN_FULL` and regressive `MDBX_KEYEXIST` during large transaction commit with `MDBX_LIFORECLAIM` (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/123).
 - Fixed auto-recovery (`weak->steady` with the same boot-id) when Database size at last weak checkpoint is large than at last steady checkpoint.
 - Fixed operation on systems with unusual small/large page size, including PowerPC (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/157).


## v0.9.2 at 2020-11-27

Acknowledgements:

 - Jens Alfke (Mobile Architect at [Couchbase](https://www.couchbase.com/)) for [NimDBX](https://github.com/snej/nimdbx).
 - Clément Renault (CTO at [MeiliSearch](https://www.meilisearch.com/)) for [mdbx-rs](https://github.com/Kerollmops/mdbx-rs).
 - Alex Sharov (Go-Lang Teach Lead at [TurboGeth/Ethereum](https://ethereum.org/)) for an extreme test cases and bug reporting.
 - George Hazan (CTO at [Miranda NG](https://www.miranda-ng.org/)) for bug reporting.
 - [Positive Technologies](https://www.ptsecurity.com/) for funding and [The Standoff](https://standoff365.com/).

Added features:

 - Provided package for [buildroot](https://buildroot.org/).
 - Binding for Nim is [available](https://github.com/snej/nimdbx) now by Jens Alfke.
 - Added `mdbx_env_delete()` for deletion an environment files in a proper and multiprocess-safe way.
 - Added `mdbx_txn_commit_ex()` with collecting latency information.
 - Fast completion pure nested transactions.
 - Added `LIBMDBX_INLINE_API` macro and inline versions of some API functions.
 - Added `mdbx_cursor_copy()` function.
 - Extended tests for checking cursor tracking.
 - Added `MDBX_SET_LOWERBOUND` operation for `mdbx_cursor_get()`.

Fixes:

 - Fixed missing installation of `mdbx.h++`.
 - Fixed use of obsolete `__noreturn`.
 - Fixed use of `yield` instruction on ARM if unsupported.
 - Added pthread workaround for buggy toolchain/cmake/buildroot.
 - Fixed use of `pthread_yield()` for non-GLIBC.
 - Fixed use of `RegGetValueA()` on Windows 2000/XP.
 - Fixed use of `GetTickCount64()` on Windows 2000/XP.
 - Fixed opening DB on a network shares (in the exclusive mode).
 - Fixed copy&paste typos.
 - Fixed minor false-positive GCC warning.
 - Added workaround for broken `DEFINE_ENUM_FLAG_OPERATORS` from Windows SDK.
 - Fixed cursor state after multimap/dupsort repeated deletes (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/121).
 - Added `SIGPIPE` suppression for internal thread during `mdbx_env_copy()`.
 - Fixed extra-rare `MDBX_KEY_EXIST` error during `mdbx_commit()` (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/131).
 - Fixed spilled pages checking (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/126).
 - Fixed `mdbx_load` for 'plain text' and without `-s name` cases (https://web.archive.org/web/20220414235959/https://github.com/erthink/libmdbx/issues/136).
 - Fixed save/restore/commit of cursors for nested transactions.
 - Fixed cursors state in rare/special cases (move next beyond end-of-data, after deletion and so on).
 - Added workaround for MSVC 19.28 (Visual Studio 16.8) (but may still hang during compilation).
 - Fixed paranoidal Clang C++ UB for bitwise operations with flags defined by enums.
 - Fixed large pages checking (for compatibility and to avoid false-positive errors from `mdbx_chk`).
 - Added workaround for Wine (https://github.com/miranda-ng/miranda-ng/issues/1209).
 - Fixed `ERROR_NOT_SUPPORTED` while opening DB by UNC pathnames (https://github.com/miranda-ng/miranda-ng/issues/2627).
 - Added handling `EXCEPTION_POSSIBLE_DEADLOCK` condition for Windows.


## v0.9.1 2020-09-30

Added features:

 - Preliminary C++ API with support for C++17 polymorphic allocators.
 - [Online C++ API reference](https://libmdbx.dqdkfa.ru/) by Doxygen.
 - Quick reference for Insert/Update/Delete operations.
 - Explicit `MDBX_SYNC_DURABLE` to sync modes for API clarity.
 - Explicit `MDBX_ALLDUPS` and `MDBX_UPSERT` for API clarity.
 - Support for read transactions preparation (`MDBX_TXN_RDONLY_PREPARE` flag).
 - Support for cursor preparation/(pre)allocation and reusing (`mdbx_cursor_create()` and `mdbx_cursor_bind()` functions).
 - Support for checking database using specified meta-page (see `mdbx_chk -h`).
 - Support for turn to the specific meta-page after checking (see `mdbx_chk -h`).
 - Support for explicit reader threads (de)registration.
 - The `mdbx_txn_break()` function to explicitly mark a transaction as broken.
 - Improved handling of corrupted databases by `mdbx_chk` utility and `mdbx_walk_tree()` function.
 - Improved DB corruption detection by checking parent-page-txnid.
 - Improved opening large DB (> 4Gb) from 32-bit code.
 - Provided `pure-function` and `const-function` attributes to C API.
 - Support for user-settable context for transactions & cursors.
 - Revised API and documentation related to Handle-Slow-Readers callback feature.

Deprecated functions and flags:

 - For clarity and API simplification the `MDBX_MAPASYNC` flag is deprecated.
   Just use `MDBX_SAFE_NOSYNC` or `MDBX_UTTERLY_NOSYNC` instead of it.
 - `MDBX_oom_func`, `mdbx_env_set_oomfunc()` and `mdbx_env_get_oomfunc()`
   replaced with `MDBX_hsr_func`, `mdbx_env_get_hsr` and `mdbx_env_get_hsr()`.

Fixes:

 - Fix `mdbx_strerror()` for `MDBX_BUSY` error (no error description is returned).
 - Fix update internal meta-geo information in read-only mode (`EACCESS` or `EBADFD` error).
 - Fix `mdbx_page_get()` null-defer when DB corrupted (crash by `SIGSEGV`).
 - Fix `mdbx_env_open()` for re-opening after non-fatal errors (`mdbx_chk` unexpected failures).
 - Workaround for MSVC 19.27 `static_assert()` bug.
 - Doxygen descriptions and refinement.
 - Update Valgrind's suppressions.
 - Workaround to avoid infinite loop of 'nested' testcase on MIPS under QEMU.
 - Fix a lot of typos & spelling (Thanks to Josh Soref for PR).
 - Fix `getopt()` messages for Windows (Thanks to Andrey Sporaw for reporting).
 - Fix MSVC compiler version requirements (Thanks to Andrey Sporaw for reporting).
 - Workarounds for QEMU's bugs to run tests for cross-builded library under QEMU.
 - Now C++ compiler optional for building by CMake.


## v0.9.0 2020-07-31 (not a release, but API changes)

Added features:

 - [Online C API reference](https://libmdbx.dqdkfa.ru/) by Doxygen.
 - Separated enums for environment, sub-databases, transactions, copying and data-update flags.

Deprecated functions and flags:

 - Usage of custom comparators and the `mdbx_dbi_open_ex()` are deprecated, since such databases couldn't be checked by the `mdbx_chk` utility.
   Please use the value-to-key functions to provide keys that are compatible with the built-in libmdbx comparators.


-------------------------------------------------------------------------------


## 2020-07-06

 - Added support multi-opening the same DB in a process with SysV locking (BSD).
 - Fixed warnings & minors for LCC compiler (E2K).
 - Enabled to simultaneously open the same database from processes with and without the `MDBX_WRITEMAP` option.
 - Added key-to-value, `mdbx_get_keycmp()` and `mdbx_get_datacmp()` functions (helpful to avoid using custom comparators).
 - Added `ENABLE_UBSAN` CMake option to enabling the UndefinedBehaviorSanitizer from GCC/CLANG.
 - Workaround for [CLANG bug](https://bugs.llvm.org/show_bug.cgi?id=43275).
 - Returning `MDBX_CORRUPTED` in case all meta-pages are weak and no other error.
 - Refined mode bits while auto-creating LCK-file.
 - Avoids unnecessary database file re-mapping in case geometry changed by another process(es).
   From the user's point of view, the `MDBX_UNABLE_EXTEND_MAPSIZE` error will now be returned less frequently and only when using the DB in the current process really requires it to be reopened.
 - Remapping on-the-fly and of the database file was implemented.
   Now remapping with a change of address is performed automatically if there are no dependent readers in the current process.


## 2020-06-12

 - Minor change versioning. The last number in the version now means the number of commits since last release/tag.
 - Provide ChangeLog file.
 - Fix for using libmdbx as a C-only sub-project with CMake.
 - Fix `mdbx_env_set_geometry()` for case it is called from an opened environment outside of a write transaction.
 - Add support for huge transactions and `MDBX_HUGE_TRANSACTIONS` build-option (default `OFF`).
 - Refine LTO (link time optimization) for clang.
 - Force enabling exceptions handling for MSVC (`/EHsc` option).


## 2020-06-05

 - Support for Android/Bionic.
 - Support for iOS.
 - Auto-handling `MDBX_NOSUBDIR` while opening for any existing database.
 - Engage github-actions to make release-assets.
 - Clarify API description.
 - Extended keygen-cases in stochastic test.
 - Fix fetching of first/lower key from LEAF2-page during page merge.
 - Fix missing comma in array of error messages.
 - Fix div-by-zero while copy-with-compaction for non-resizable environments.
 - Fixes & enhancements for custom-comparators.
 - Fix `MDBX_WITHOUT_MSVC_CRT` option and missing `ntdll.def`.
 - Fix `mdbx_env_close()` to work correctly called concurrently from several threads.
 - Fix null-deref in an ASAN-enabled builds while opening the environment with error and/or read-only.
 - Fix AddressSanitizer errors after closing the environment.
 - Fix/workaround to avoid GCC 10.x pedantic warnings.
 - Fix using `ENODATA` for FreeBSD.
 - Avoid invalidation of DBI-handle(s) when it just closes.
 - Avoid using `pwritev()` for single-writes (up to 10% speedup for some kernels & scenarios).
 - Avoiding `MDBX_UTTERLY_NOSYNC` as result of flags merge.
 - Add `mdbx_dbi_dupsort_depthmask()` function.
 - Add `MDBX_CP_FORCE_RESIZEABLE` option.
 - Add deprecated `MDBX_MAP_RESIZED` for compatibility.
 - Add `MDBX_BUILD_TOOLS` option (default `ON`).
 - Refine `mdbx_dbi_open_ex()` to safe concurrently opening the same handle from different threads.
 - Truncate clk-file during environment closing. So a zero-length lck-file indicates that the environment was closed properly.
 - Refine `mdbx_update_gc()` for huge transactions with small sizes of database page.
 - Extends dump/load to support all MDBX attributes.
 - Avoid upsertion the same key-value data, fix related assertions.
 - Rework min/max length checking for keys & values.
 - Checking the order of keys on all pages during checking.
 - Support `CFLAGS_EXTRA` make-option for convenience.
 - Preserve the last txnid while copying with compactification.
 - Auto-reset running transaction in mdbx_txn_renew().
 - Automatically abort errored transaction in mdbx_txn_commit().
 - Auto-choose page size for large databases.
 - Rearrange source files, rework build, options-support by CMake.
 - Crutch for WSL1 (Windows subsystem for Linux).
 - Refine install/uninstall targets.
 - Support for Valgrind 3.14 and later.
 - Add check-analyzer check-ubsan check-asan check-leak targets to Makefile.
 - Minor fix/workaround to avoid UBSAN traps for `memcpy(ptr, NULL, 0)`.
 - Avoid some GCC-analyzer false-positive warnings.


## 2020-03-18

 - Workarounds for Wine (Windows compatibility layer for Linux).
 - `MDBX_MAP_RESIZED` renamed to `MDBX_UNABLE_EXTEND_MAPSIZE`.
 - Clarify API description, fix typos.
 - Speedup runtime checks in debug/checked builds.
 - Added checking for read/write transactions overlapping for the same thread, added `MDBX_TXN_OVERLAPPING` error and `MDBX_DBG_LEGACY_OVERLAP` option.
 - Added `mdbx_key_from_jsonInteger()`, `mdbx_key_from_double()`, `mdbx_key_from_float()`, `mdbx_key_from_int64()` and `mdbx_key_from_int32()` functions. See `mdbx.h` for description.
 - Fix compatibility (use zero for invalid DBI).
 - Refine/clarify error messages.
 - Avoids extra error messages "bad txn" from mdbx_chk when DB is corrupted.


## 2020-01-21

 - Fix `mdbx_load` utility for custom comparators.
 - Fix checks related to `MDBX_APPEND` flag inside `mdbx_cursor_put()`.
 - Refine/fix dbi_bind() internals.
 - Refine/fix handling `STATUS_CONFLICTING_ADDRESSES`.
 - Rework `MDBX_DBG_DUMP` option to avoid disk I/O performance degradation.
 - Add built-in help to test tool.
 - Fix `mdbx_env_set_geometry()` for large page size.
 - Fix env_set_geometry() for large pagesize.
 - Clarify API description & comments, fix typos.


## 2019-12-31

 - Fix returning MDBX_RESULT_TRUE from page_alloc().
 - Fix false-positive ASAN issue.
 - Fix assertion for `MDBX_NOTLS` option.
 - Rework `MADV_DONTNEED` threshold.
 - Fix `mdbx_chk` utility for don't checking some numbers if walking on the B-tree was disabled.
 - Use page's mp_txnid for basic integrity checking.
 - Add `MDBX_FORCE_ASSERTIONS` built-time option.
 - Rework `MDBX_DBG_DUMP` to avoid performance degradation.
 - Rename `MDBX_NOSYNC` to `MDBX_SAFE_NOSYNC` for clarity.
 - Interpret `ERROR_ACCESS_DENIED` from `OpenProcess()` as 'process exists'.
 - Avoid using `FILE_FLAG_NO_BUFFERING` for compatibility with small database pages.
 - Added install section for CMake.


## 2019-12-02

 - Support for Mac OSX, FreeBSD, NetBSD, OpenBSD, DragonFly BSD, OpenSolaris, OpenIndiana (AIX and HP-UX pending).
 - Use bootid for decisions of rollback.
 - Counting retired pages and extended transaction info.
 - Add `MDBX_ACCEDE` flag for database opening.
 - Using OFD-locks and tracking for in-process multi-opening.
 - Hot backup into pipe.
 - Support for cmake & amalgamated sources.
 - Fastest internal sort implementation.
 - New internal dirty-list implementation with lazy sorting.
 - Support for lazy-sync-to-disk with polling.
 - Extended key length.
 - Last update transaction number for each sub-database.
 - Automatic read ahead enabling/disabling.
 - More auto-compactification.
 - Using -fsanitize=undefined and -Wpedantic options.
 - Rework page merging.
 - Nested transactions.
 - API description.
 - Checking for non-local filesystems to avoid DB corruption.

-------------------------------------------------------------------------------

For early changes see the git commit history.
