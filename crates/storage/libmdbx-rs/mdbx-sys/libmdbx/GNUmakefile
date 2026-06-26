# This makefile is for GNU Make 3.81 or above, and nowadays provided
# just for compatibility and preservation of traditions.
#
# Please use CMake in case of any difficulties or
# problems with this old-school's magic.
#
################################################################################
#
# Basic internal definitions. For a customizable variables and options see below.
#
$(info // The GNU Make $(MAKE_VERSION))
SHELL         := $(shell env bash -c 'echo $$BASH')
MAKE_VERx3    := $(shell printf "%3s%3s%3s" $(subst ., ,$(MAKE_VERSION)))
make_lt_3_81  := $(shell expr "$(MAKE_VERx3)" "<" "  3 81")
ifneq ($(make_lt_3_81),0)
$(error Please use GNU Make 3.81 or above)
endif
make_ge_4_1   := $(shell expr "$(MAKE_VERx3)" ">=" "  4  1")
make_ge_4_4   := $(shell expr "$(MAKE_VERx3)" ">=" "  4  4")
SRC_PROBE_C   := $(shell [ -f mdbx.c ] && echo mdbx.c || echo src/osal.c)
SRC_PROBE_CXX := $(shell [ -f mdbx.c++ ] && echo mdbx.c++ || echo src/mdbx.c++)
UNAME         := $(shell uname -s 2>/dev/null || echo Unknown)

define cxx_filesystem_probe
  int main(int argc, const char*argv[]) {
    mdbx::filesystem::path probe(argv[0]);
    if (argc != 1) throw mdbx::filesystem::filesystem_error(std::string("fake"), std::error_code());
    return mdbx::filesystem::is_directory(probe.relative_path());
  }
endef
#
################################################################################
#
# Use `make options` to list the available libmdbx build options.
#
# Note that the defaults should already be correct for most platforms;
# you should not need to change any of these. Read their descriptions
# in the README and source code (see src/options.h) if you do.
#

# install sandbox
DESTDIR ?=
INSTALL ?= install
# install prefixes (inside sandbox)
prefix  ?= /usr/local
mandir  ?= $(prefix)/man
# lib/bin suffix for multiarch/biarch, e.g. '.x86_64'
suffix  ?=

# toolchain
CC      ?= gcc
CXX     ?= g++
CFLAGS_EXTRA ?=
LD      ?= ld
CMAKE	?= "$(shell which cmake 2>&-)"
CMAKE_BUILD_DIR ?= @cmake-ninja-build
CMAKE_BASE_OPT ?= -DENABLE_ASAN=OFF -DENABLE_UBSAN=OFF -DENABLE_MEMCHECK=OFF -DMDBX_CHECKING=0 \
	-DMDBX_ENABLE_DXB_FAULT_INJECTION=OFF
CMAKE_OPT ?=
CTEST	?= ctest
CTEST_OPT ?=
TEST_LOG ?= test.log
SED     ?= sed
AWK     ?= awk
MIGRATION_EXTENDED_STRESS_REPEAT ?= 3
MIGRATION_BENCH_REPEAT ?= 3
# target directory for `make dist`
DIST_DIR ?= dist

# build options
MDBX_BUILD_OPTIONS   ?=
MDBX_BUILD_TIMESTAMP ?=$(if $(SOURCE_DATE_EPOCH),$(SOURCE_DATE_EPOCH),$(shell date +%Y-%m-%dT%H:%M:%S%z))
MDBX_BUILD_CXX       ?=YES
MDBX_BUILD_METADATA  ?=

# probe and compose common compiler flags with variable expansion trick (seems this work two times per session for GNU Make 3.81)
CFLAGS       ?= $(strip $(eval CFLAGS := -std=gnu11 -O2 -g -Wall -Werror -Wextra -Wpedantic -ffunction-sections -fPIC -fvisibility=hidden -pthread -Wno-error=attributes $$(shell for opt in -fno-semantic-interposition -Wno-unused-command-line-argument -Wno-tautological-compare; do [ -z "$$$$($(CC) '-DMDBX_BUILD_FLAGS="probe"' $$$${opt} -c $(SRC_PROBE_C) -o /dev/null >/dev/null 2>&1 || echo failed)" ] && echo "$$$${opt} "; done)$(CFLAGS_EXTRA))$(CFLAGS))

# choosing C++ standard with variable expansion trick (seems this work two times per session for GNU Make 3.81)
CXXSTD       ?= $(eval CXXSTD := $$(shell for std in gnu++23 c++23 gnu++2b c++2b gnu++20 c++20 gnu++2a c++2a gnu++17 c++17 gnu++1z c++1z gnu++14 c++14 gnu++1y c++1y gnu+11 c++11 gnu++0x c++0x; do $(CXX) -std=$$$${std} -DMDBX_BUILD_CXX=1 -c $(SRC_PROBE_CXX) -o /dev/null 2>probe4std-$$$${std}.err >/dev/null && echo "-std=$$$${std}" && exit; done))$(CXXSTD)
CXXFLAGS     ?= $(strip $(CXXSTD) $(filter-out -std=gnu11,$(CFLAGS)))

# libraries and options for linking
EXE_LDFLAGS  ?= -pthread
ifneq ($(make_ge_4_1),1)
# don't use variable expansion trick as workaround for bugs of GNU Make before 4.1
LIBS         ?= $(shell $(uname2libs))
LDFLAGS      ?= $(shell $(uname2ldflags))
LIB_STDCXXFS ?= $(shell echo '$(cxx_filesystem_probe)' | cat mdbx.h++ - | sed $$'1s/\xef\xbb\xbf//' | grep -v 'pragma once' | $(CXX) -x c++ $(CXXFLAGS) -Wno-error - -Wl,--allow-multiple-definition -lstdc++fs $(LIBS) $(LDFLAGS) $(EXE_LDFLAGS) -o /dev/null 2>probe4lstdfs.err >/dev/null && echo '-Wl,--allow-multiple-definition -lstdc++fs')
else
# using variable expansion trick to avoid repeaded probes
LIBS         ?= $(eval LIBS := $$(shell $$(uname2libs)))$(LIBS)
LDFLAGS      ?= $(eval LDFLAGS := $$(shell $$(uname2ldflags)))$(LDFLAGS)
LIB_STDCXXFS ?= $(eval LIB_STDCXXFS := $$(shell echo '$$(cxx_filesystem_probe)' | cat mdbx.h++ - | sed $$$$'1s/\xef\xbb\xbf//' | grep -v '#pragma once' | $(CXX) -x c++ $(CXXFLAGS) -Wno-error - -Wl,--allow-multiple-definition -lstdc++fs $(LIBS) $(LDFLAGS) $(EXE_LDFLAGS) -o /dev/null 2>probe4lstdfs.err >/dev/null && echo '-Wl,--allow-multiple-definition -lstdc++fs'))$(LIB_STDCXXFS)
endif

ifneq ($(make_ge_4_4),1)
.NOTPARALLEL:
WAIT         =
else
WAIT         = .WAIT
endif

################################################################################

define uname2sosuffix
  case "$(UNAME)" in
    Darwin*|Mach*) echo dylib;;
    CYGWIN*|MINGW*|MSYS*|Windows*) echo dll;;
    *) echo so;;
  esac
endef

define uname2ldflags
  case "$(UNAME)" in
    CYGWIN*|MINGW*|MSYS*|Windows*)
      echo '-Wl,--gc-sections,-O1,--as-needed';
      ;;
    *)
      $(LD) --help 2>/dev/null | grep -q -- --gc-sections && echo '-Wl,--gc-sections,-z,relro,-O1';
      $(LD) --help 2>/dev/null | grep -q -- -dead_strip && echo '-Wl,-dead_strip';
      $(LD) --help 2>/dev/null | grep -q -- --as-needed && echo '-Wl,--as-needed';
      ;;
  esac
endef

# TIP: try add the'-Wl, --no-as-needed,-lrt' for ability to built with modern glibc, but then use with the old.
define uname2libs
  case "$(UNAME)" in
    CYGWIN*|MINGW*|MSYS*|Windows*)
      echo '-lntdll -lwinmm';
      ;;
    *SunOS*|*Solaris*)
      echo '-lkstat -lrt';
      ;;
    *Darwin*|OpenBSD*)
      echo '';
      ;;
    *)
      echo '-lrt';
      ;;
  esac
endef

SO_SUFFIX  := $(shell $(uname2sosuffix))
HEADERS    := mdbx.h mdbx.h++
LIBRARIES  := libmdbx.a libmdbx.$(SO_SUFFIX)
TOOLS      := chk copy defrag drop dump load stat
MDBX_TOOLS := $(addprefix mdbx_,$(TOOLS))
MANPAGES   := mdbx_stat.1 mdbx_copy.1 mdbx_dump.1 mdbx_load.1 mdbx_chk.1 mdbx_drop.1
TIP        := // TIP:

.PHONY: all help options lib libs tools clean install uninstall check_buildflags_tag tools-static run-ut
.PHONY: mdbx_migration_smoke_nommap mdbx_migration_smoke_nommap_tinycache \
	mdbx_migration_smoke_stress_nommap_tinycache mdbx_migration_smoke_randomized_stress_nommap_tinycache \
	mdbx_migration_smoke_crash_stress_nommap_tinycache \
	mdbx_migration_extended_stress_nommap_tinycache \
	mdbx_migration_fault_injection_nommap mdbx_migration_fault_injection_nommap_tinycache \
	mdbx_migration_public_ctest mdbx_migration_fault_ctest \
	mdbx_migration_repeat_ctest \
	mdbx_migration_assertion_ctest \
	mdbx_migration_audit_ctest \
	mdbx_migration_memcheck_ctest \
	mdbx_migration_leak_ctest \
	mdbx_migration_tool_roundtrip mdbx_migration_tool_roundtrip_nommap \
	mdbx_migration_tool_roundtrip_nommap_tinycache mdbx_migration_bench_lazy \
	mdbx_migration_bench_lazy_repeat mdbx_migration_check
.PHONY: install-strip install-no-strip strip libmdbx mdbx show-options lib-static lib-shared cmake-build ninja

boolean = $(if $(findstring $(strip $($1)),YES Yes yes y ON On on 1 true True TRUE),1,$(if $(findstring $(strip $($1)),NO No no n OFF Off off 0 false False FALSE),,$(error Wrong value `$($1)` of $1 for YES/NO option)))
select_by = $(if $(call boolean,$(1)),$(2),$(3))

ifeq ("$(origin V)", "command line")
  MDBX_BUILD_VERBOSE := $(V)
endif
ifndef MDBX_BUILD_VERBOSE
  MDBX_BUILD_VERBOSE := 0
endif

ifeq ($(call boolean,MDBX_BUILD_VERBOSE),1)
  QUIET :=
  HUSH :=
  $(info $(TIP) Use `make V=0` for quiet.)
else
  QUIET := @
  HUSH := >/dev/null
  $(info $(TIP) Use `make V=1` for verbose.)
endif

ifeq ($(UNAME),Darwin)
  $(info $(TIP) Use `brew install gnu-sed gnu-tar` and add ones to the beginning of the PATH.)
endif

all: show-options $(LIBRARIES) $(MDBX_TOOLS)

help:
	@echo "  make all                 - build libraries and tools"
	@echo "  make help                - print this help"
	@echo "  make options             - list build options"
	@echo "  make lib                 - build libraries, also lib-static and lib-shared"
	@echo "  make tools               - build the tools"
	@echo "  make tools-static        - build the tools with statically linking with system libraries and compiler runtime"
	@echo "  make clean               "
	@echo "  make install             "
	@echo "  make uninstall           "
	@echo "  make cmake-build | ninja - build by CMake & Ninja"
	@echo ""
	@echo "  make strip               - strip debug symbols from binaries"
	@echo "  make install-no-strip    - install explicitly without strip"
	@echo "  make install-strip       - install explicitly with strip"
	@echo ""
	@echo "  make bench               - run ioarena-benchmark"
	@echo "  make bench-couple        - run ioarena-benchmark for mdbx and lmdb"
	@echo "  make bench-triplet       - run ioarena-benchmark for mdbx, lmdb, sqlite3"
	@echo "  make bench-quartet       - run ioarena-benchmark for mdbx, lmdb, rocksdb, wiredtiger"
	@echo "  make bench-clean         - remove temp database(s) after benchmark"
	@echo "  make mdbx_migration_smoke_stress_nommap_tinycache - run heavier no-map migration stress"
	@echo "  make mdbx_migration_smoke_randomized_stress_nommap_tinycache - run permuted no-map migration stress"
	@echo "  make mdbx_migration_smoke_crash_stress_nommap_tinycache - run no-map crash/restart stress"
	@echo "  make mdbx_migration_extended_stress_nommap_tinycache - repeat randomized and crash no-map stress"
	@echo "  make mdbx_migration_fault_injection_nommap - run no-map explicit I/O fault checks"
	@echo "  make mdbx_migration_fault_injection_nommap_tinycache - run no-map fault checks with 64K cache"
	@echo "  make mdbx_migration_public_ctest - run public CTest migration/API gates"
	@echo "  make mdbx_migration_repeat_ctest - repeat public CTest migration/API gates"
	@echo "  make mdbx_migration_assertion_ctest - run public CTest gates with MDBX_CHECKING=2"
	@echo "  make mdbx_migration_fault_ctest - run fault-enabled public CTest migration/API gates"
	@echo "  make mdbx_migration_audit_ctest - run no-map migration CTest gates with MDBX_CHECKING=3/audit"
	@echo "  make mdbx_migration_memcheck_ctest - run migration CTest gates with ENABLE_MEMCHECK"
	@echo "  make mdbx_migration_leak_ctest - run migration CTest gates with LeakSanitizer"
	@echo "  make mdbx_migration_bench_lazy_repeat - repeat paired migration performance gate"
	@echo "  make mdbx_migration_check - run migration correctness, sanitizer, memcheck, audit and performance gates"
	@echo "  make test                - basic test(s)"
	@echo "  make build-test          - build test(s) executable(s)"
	@echo "  make test-asan           - build with AddressSanitizer and run basic test"
	@echo "  make test-leak           - build with LeakSanitizer and run basic test"
	@echo "  make test-ubsan          - build with UndefinedBehaviourSanitizer and run basic test"

show-options:
	@echo "  MDBX_BUILD_OPTIONS   = $(MDBX_BUILD_OPTIONS)"
	@echo "  MDBX_BUILD_CXX       = $(MDBX_BUILD_CXX)"
	@echo "  MDBX_BUILD_TIMESTAMP = $(MDBX_BUILD_TIMESTAMP)"
	@echo "  MDBX_BUILD_METADATA  = $(MDBX_BUILD_METADATA)"
	@echo '$(TIP) Use `make options` to listing available build options.'
	@echo $(call select_by,MDBX_BUILD_CXX,"  CXX      =`which $(CXX)` | `$(CXX) --version | head -1`","  CC       =`which $(CC)` | `$(CC) --version | head -1`")
	@echo $(call select_by,MDBX_BUILD_CXX,"  CXXFLAGS =$(CXXFLAGS)","  CFLAGS   =$(CFLAGS)")
	@echo $(call select_by,MDBX_BUILD_CXX,"  LDFLAGS  =$(LDFLAGS) $(LIB_STDCXXFS) $(LIBS) $(EXE_LDFLAGS)","  LDFLAGS  =$(LDFLAGS) $(LIBS) $(EXE_LDFLAGS)")
	@echo '$(TIP) Use `make help` to listing available targets.'

options:
	@echo "  INSTALL      =$(INSTALL)"
	@echo "  DESTDIR      =$(DESTDIR)"
	@echo "  prefix       =$(prefix)"
	@echo "  mandir       =$(mandir)"
	@echo "  suffix       =$(suffix)"
	@echo ""
	@echo "  CC           =$(CC)"
	@echo "  CFLAGS_EXTRA =$(CFLAGS_EXTRA)"
	@echo "  CFLAGS       =$(CFLAGS)"
	@echo "  CXX          =$(CXX)"
	@echo "  CXXSTD       =$(CXXSTD)"
	@echo "  CXXFLAGS     =$(CXXFLAGS)"
	@echo ""
	@echo "  LD           =$(LD)"
	@echo "  LDFLAGS      =$(LDFLAGS)"
	@echo "  EXE_LDFLAGS  =$(EXE_LDFLAGS)"
	@echo "  LIBS         =$(LIBS)"
	@echo ""
	@echo "  MDBX_BUILD_OPTIONS   = $(MDBX_BUILD_OPTIONS)"
	@echo "  MDBX_BUILD_TIMESTAMP = $(MDBX_BUILD_TIMESTAMP)"
	@echo "  MDBX_BUILD_METADATA  = $(MDBX_BUILD_METADATA)"
	@echo ""
	@echo "## Assortment items for MDBX_BUILD_OPTIONS:"
	@echo "##   Note that the defaults should already be correct for most platforms;"
	@echo "##   you should not need to change any of these. Read their descriptions"
	@echo "##   in the README and source code (see mdbx.c) if you do."
	@grep -h '#ifndef MDBX_' mdbx.c | grep -v BUILD | sort -u | sed 's/#ifndef /  /'

lib libs libmdbx mdbx: libmdbx.a libmdbx.$(SO_SUFFIX)

tools: $(MDBX_TOOLS)
tools-static: $(addsuffix .static,$(MDBX_TOOLS)) $(addsuffix .static-lto,$(MDBX_TOOLS))

strip: all
	@echo '  STRIP libmdbx.$(SO_SUFFIX) $(MDBX_TOOLS)'
	$(TRACE )strip libmdbx.$(SO_SUFFIX) $(MDBX_TOOLS)

clean:
	@echo '  CLEANING...'
	$(QUIET)rm -rf $(MDBX_TOOLS) mdbx_test @* *.[ao] *.[ls]o *.$(SO_SUFFIX) *.dSYM *~ tmp.db/* \
		*.gcov *.log *.err src/*.o test/*.o mdbx_example dist @dist-check \
		config-gnumake.h src/config-gnumake.h *.tar* @buildflags.tag @dist-checked.tag \
		mdbx_*.static mdbx_*.static-lto CMakeFiles

MDBX_BUILD_FLAGS =$(strip MDBX_BUILD_CXX=$(MDBX_BUILD_CXX) $(MDBX_BUILD_OPTIONS) $(call select_by,MDBX_BUILD_CXX,$(CXXFLAGS) $(LDFLAGS) $(LIB_STDCXXFS) $(LIBS),$(CFLAGS) $(LDFLAGS) $(LIBS)))
check_buildflags_tag:
	$(QUIET)if [ "$(MDBX_BUILD_FLAGS)" != "$$(cat @buildflags.tag 2>&1)" ]; then \
		echo "  TOUCH @buildflags.tag to force re-build with the (new) specified flags..." && \
		echo '$(MDBX_BUILD_FLAGS)' > @buildflags.tag; \
	fi

@buildflags.tag: check_buildflags_tag $(WAIT)

lib-static libmdbx.a: mdbx-static.o $(call select_by,MDBX_BUILD_CXX,mdbx++-static.o)
	@echo '  AR $@'
	$(QUIET)$(AR) rcs $@ $? $(HUSH)

lib-shared libmdbx.$(SO_SUFFIX): mdbx-dylib.o $(call select_by,MDBX_BUILD_CXX,mdbx++-dylib.o)
	@echo '  LD $@'
	$(QUIET)$(call select_by,MDBX_BUILD_CXX,$(CXX) $(CXXFLAGS),$(CC) $(CFLAGS)) $^ -pthread -shared $(LDFLAGS) $(call select_by,MDBX_BUILD_CXX,$(LIB_STDCXXFS)) $(LIBS) -o $@

ninja-assertions: CMAKE_OPT += -DMDBX_CHECKING=2 $(MDBX_BUILD_OPTIONS)
ninja-assertions: cmake-build
ninja-debug: CMAKE_OPT += -DCMAKE_BUILD_TYPE=Debug $(MDBX_BUILD_OPTIONS)
ninja-debug: cmake-build
ninja: cmake-build
cmake-build:
	@echo "  RUN: cmake -G Ninja && cmake --build"
	$(QUIET)mkdir -p $(CMAKE_BUILD_DIR) && $(CMAKE) $(CMAKE_BASE_OPT) $(CMAKE_OPT) -G Ninja -S . \
		-B $(CMAKE_BUILD_DIR) && $(CMAKE) --build $(CMAKE_BUILD_DIR)

ctest: cmake-build
	@echo "  RUN: ctest .."
	$(QUIET)$(CTEST) --test-dir $(CMAKE_BUILD_DIR) --parallel `(nproc | sysctl -n hw.ncpu | echo 2) 2>/dev/null` \
		--schedule-random $(CTEST_OPT)

run-ut: mdbx_example
	$(QUIET)for UT in $^; do echo "  Running $$UT" && ./$${UT} || exit -1; done

TEST_TARGETS :=
TEST_BUILD_TARGETS :=
ifneq ($(CMAKE),"")
TEST_TARGETS += ctest
TEST_BUILD_TARGETS += cmake-build
endif
TEST_TARGETS += mdbx_legacy_example mdbx_migration_smoke $(call select_by,MDBX_BUILD_CXX,mdbx_modern_example,)

.PHONY: ninja-assertions ninja-debug ninja $(TEST_TARGETS) $(TEST_BUILD_TARGETS) \
	build-stochastic test-stochastic test-ubsan test-asan test-memcheck test-leak \
	test-assertion test-long test-long-assertion test-ci test-ci-extra test-singleprocess \
	smoke-fault smoke-singleprocess smoke-assertion smoke-memcheck memcheck mdbx_test \
	test-valgrind test build-test smoke check
test: $(TEST_TARGETS)
build-test: $(TEST_BUILD_TARGETS)

test-assertion: MDBX_BUILD_OPTIONS += -DMDBX_CHECKING=2
test-assertion: CMAKE_BUILD_DIR = @cmake-assertion-build
test-assertion: CMAKE_OPT += -DMDBX_CHECKING=2
test-assertion: smoke

mdbx_test: mdbx_migration_smoke
memcheck: test-memcheck
test-singleprocess smoke-singleprocess: mdbx_migration_smoke_nommap_tinycache
smoke-fault: mdbx_migration_fault_injection_nommap
smoke-assertion: mdbx_migration_assertion_ctest
smoke-memcheck: mdbx_migration_memcheck_ctest

test-long: mdbx_migration_repeat_ctest
	@echo '  RUN public long-test fallback'
	$(QUIET)$(MAKE) -f GNUmakefile IOARENA=false STOCHASTIC_ARGS="--loops 8 --db-upto-mb 512" test-stochastic

test-long-assertion: mdbx_migration_assertion_ctest
	@echo '  RUN assertion long-test fallback'
	$(QUIET)$(MAKE) -f GNUmakefile IOARENA=false CMAKE_BUILD_DIR=@cmake-assertion-build \
		CMAKE_OPT="-DMDBX_CHECKING=2" CTEST_OPT="--output-on-failure --repeat until-fail:3" \
		STOCHASTIC_ARGS="--loops 8 --db-upto-mb 512" test-stochastic

test-ci: mdbx_migration_public_ctest mdbx_migration_repeat_ctest mdbx_migration_assertion_ctest \
	mdbx_migration_fault_ctest test-asan test-ubsan

test-ci-extra: mdbx_migration_check

test-valgrind: test-memcheck
STOCHASTIC_TEST := test/stochastic.sh
STOCHASTIC_ARGS ?= --loops 2 --db-upto-mb 256

ifneq ($(wildcard $(STOCHASTIC_TEST)),)
build-stochastic: build-test
test-stochastic: build-stochastic
	@echo '  RUNNING `$(STOCHASTIC_TEST) $(STOCHASTIC_ARGS)`...'
	$(QUIET)$(STOCHASTIC_TEST) $(STOCHASTIC_ARGS) --skip-make >$(TEST_LOG) || (cat $(TEST_LOG) && false)
else
build-stochastic: build-test
	@echo '  SKIP $(STOCHASTIC_TEST) is absent; using public CTest gates'
test-stochastic: ctest
	@echo '  PASS public CTest fallback for missing stochastic harness'
endif

test-memcheck:
	@echo '  RE-TEST with Valgrind/Memcheck option...'
	$(QUIET)$(MAKE) IOARENA=false CXXSTD=$(CXXSTD) CMAKE_BUILD_DIR=@cmake-memcheck-build \
		CMAKE_OPT="-DENABLE_MEMCHECK=ON" \
		CFLAGS_EXTRA="-Ofast -DENABLE_MEMCHECK" \
		STOCHASTIC_ARGS="--with-valgrind --loops 2 --db-upto-mb 256" build-test test-stochastic

test-ubsan:
	@echo '  RE-TEST with `-fsanitize=undefined` option...'
	$(QUIET)$(MAKE) IOARENA=false CXXSTD=$(CXXSTD) CMAKE_BUILD_DIR=@cmake-ubsan-build \
		CMAKE_OPT="-DENABLE_UBSAN=ON" \
		CFLAGS_EXTRA="-DENABLE_UBSAN -Ofast -fsanitize=undefined -fsanitize-undefined-trap-on-error" \
		build-test test-stochastic

test-asan:
	@echo '  RE-TEST with `-fsanitize=address` option...'
	$(QUIET)$(MAKE) IOARENA=false CXXSTD=$(CXXSTD) CMAKE_BUILD_DIR=@cmake-asan-build \
		CMAKE_OPT="-DENABLE_ASAN=ON" CFLAGS_EXTRA="-Os -fsanitize=address" build-test test-stochastic

test-leak:
	@echo '  RE-TEST with `-fsanitize=leak` option...'
	$(QUIET)$(MAKE) IOARENA=false CXXSTD=$(CXXSTD) CMAKE_BUILD_DIR=@cmake-leak-build \
		CMAKE_OPT="-DCMAKE_C_FLAGS=-fsanitize=leak -DCMAKE_CXX_FLAGS=-fsanitize=leak" \
		CFLAGS_EXTRA="-fsanitize=leak" test-stochastic

mdbx_legacy_example: mdbx.h ut_and_examples/example-mdbx.c libmdbx.$(SO_SUFFIX)
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) -I. ut_and_examples/example-mdbx.c ./libmdbx.$(SO_SUFFIX) -o $@

mdbx_modern_example: mdbx.h ut_and_examples/example-mdbx.c++ libmdbx.$(SO_SUFFIX)
	@echo '  CC+LD $@'
	$(QUIET)$(CXX) $(CXXFLAGS) -I. ut_and_examples/example-mdbx.c++ ./libmdbx.$(SO_SUFFIX) -o $@

mdbx_migration_smoke: mdbx.h ut_and_examples/migration-smoke.c libmdbx.$(SO_SUFFIX)
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) -I. ut_and_examples/migration-smoke.c ./libmdbx.$(SO_SUFFIX) -o $@

mdbx_migration_smoke_nommap: mdbx_migration_smoke
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 LD_LIBRARY_PATH=. ./mdbx_migration_smoke

mdbx_migration_smoke_nommap_tinycache: mdbx_migration_smoke
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K LD_LIBRARY_PATH=. ./mdbx_migration_smoke

mdbx_migration_smoke_stress: mdbx.h ut_and_examples/migration-smoke.c libmdbx.$(SO_SUFFIX)
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) -DMULTIPROCESS_STRESS_READERS=4 -DMULTIPROCESS_STRESS_WRITERS=4 \
		-DMULTIPROCESS_STRESS_WAVES=8 -I. ut_and_examples/migration-smoke.c ./libmdbx.$(SO_SUFFIX) -o $@

mdbx_migration_smoke_stress_nommap_tinycache: mdbx_migration_smoke_stress
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K LD_LIBRARY_PATH=. ./mdbx_migration_smoke_stress

mdbx_migration_smoke_randomized_stress: mdbx.h ut_and_examples/migration-smoke.c libmdbx.$(SO_SUFFIX)
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) -DMULTIPROCESS_STRESS_READERS=4 -DMULTIPROCESS_STRESS_WRITERS=4 \
		-DMULTIPROCESS_STRESS_WAVES=8 -DMULTIPROCESS_STRESS_RANDOMIZED=1 \
		-I. ut_and_examples/migration-smoke.c ./libmdbx.$(SO_SUFFIX) -o $@

mdbx_migration_smoke_randomized_stress_nommap_tinycache: mdbx_migration_smoke_randomized_stress
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K LD_LIBRARY_PATH=. ./mdbx_migration_smoke_randomized_stress

mdbx_migration_smoke_crash_stress: mdbx.h ut_and_examples/migration-smoke.c libmdbx.$(SO_SUFFIX)
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) -DMULTIPROCESS_STRESS_READERS=4 -DMULTIPROCESS_STRESS_WRITERS=4 \
		-DMULTIPROCESS_STRESS_WAVES=8 -DMULTIPROCESS_CRASH_STRESS=1 \
		-I. ut_and_examples/migration-smoke.c ./libmdbx.$(SO_SUFFIX) -o $@

mdbx_migration_smoke_crash_stress_nommap_tinycache: mdbx_migration_smoke_crash_stress
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K LD_LIBRARY_PATH=. ./mdbx_migration_smoke_crash_stress

mdbx_migration_extended_stress_nommap_tinycache: mdbx_migration_smoke_randomized_stress mdbx_migration_smoke_crash_stress
	@echo '  RUN $@ ($(MIGRATION_EXTENDED_STRESS_REPEAT)x randomized + crash stress)'
	$(QUIET)for n in $$(seq 1 $(MIGRATION_EXTENDED_STRESS_REPEAT)); do \
		echo "  RUN randomized no-map stress $$n/$(MIGRATION_EXTENDED_STRESS_REPEAT)"; \
		MDBX_FORCE_NO_DATA_MMAP=1 MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K LD_LIBRARY_PATH=. \
			./mdbx_migration_smoke_randomized_stress || exit $$?; \
		echo "  RUN crash no-map stress $$n/$(MIGRATION_EXTENDED_STRESS_REPEAT)"; \
		MDBX_FORCE_NO_DATA_MMAP=1 MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K LD_LIBRARY_PATH=. \
			./mdbx_migration_smoke_crash_stress || exit $$?; \
	done

mdbx-fault-static.o: config-gnumake.h mdbx.c mdbx.h mdbx-internals.h $(lastword $(MAKEFILE_LIST)) LICENSE NOTICE COPYRIGHT
	@echo '  CC $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) -DMDBX_ENABLE_DXB_FAULT_INJECTION=1 \
		'-DMDBX_CONFIG_H="config-gnumake.h"' -ULIBMDBX_EXPORTS -c mdbx.c -o $@

mdbx_migration_fault_injection: mdbx.h ut_and_examples/migration-smoke.c mdbx-fault-static.o
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) -Wno-unused-function -DMIGRATION_FAULT_INJECTION=1 -I. \
		ut_and_examples/migration-smoke.c mdbx-fault-static.o $(LDFLAGS) $(EXE_LDFLAGS) $(LIBS) -o $@

mdbx_migration_fault_injection_nommap: mdbx_migration_fault_injection
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 ./mdbx_migration_fault_injection

mdbx_migration_fault_injection_nommap_tinycache: mdbx_migration_fault_injection
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K ./mdbx_migration_fault_injection

mdbx_migration_public_ctest:
	@echo '  RUN $@'
	$(QUIET)$(MAKE) -f GNUmakefile CTEST_OPT="--output-on-failure" ctest

mdbx_migration_repeat_ctest:
	@echo '  RUN $@'
	$(QUIET)$(MAKE) -f GNUmakefile CTEST_OPT="--output-on-failure --repeat until-fail:3" ctest

mdbx_migration_assertion_ctest:
	@echo '  RUN $@'
	$(QUIET)$(MAKE) -f GNUmakefile CMAKE_BUILD_DIR=@cmake-assertion-build \
		CMAKE_OPT="-DMDBX_CHECKING=2" CTEST_OPT="--output-on-failure" ctest

mdbx_migration_fault_ctest:
	@echo '  RUN $@'
	$(QUIET)$(MAKE) -f GNUmakefile CMAKE_BUILD_DIR=@cmake-fault-build \
		CMAKE_OPT="-DMDBX_ENABLE_DXB_FAULT_INJECTION=ON" \
		CTEST_OPT="--output-on-failure" ctest

mdbx_migration_audit_ctest:
	@echo '  RUN $@'
	$(QUIET)MDBX_DBG_AUDIT=1 $(MAKE) -f GNUmakefile IOARENA=false CMAKE_BUILD_DIR=@cmake-audit-build \
		CMAKE_OPT="-DMDBX_CHECKING=3" CTEST_OPT="--output-on-failure" ctest

mdbx_migration_memcheck_ctest:
	@echo '  RUN $@'
	$(QUIET)$(MAKE) -f GNUmakefile test-memcheck

mdbx_migration_leak_ctest:
	@echo '  RUN $@'
	$(QUIET)$(MAKE) -f GNUmakefile test-leak

mdbx_migration_tool_roundtrip: $(MDBX_TOOLS) ut_and_examples/migration-tool-roundtrip.cmake
	@echo '  RUN $@'
	$(QUIET)$(CMAKE) -DMDBX_CHK=$(abspath ./mdbx_chk) -DMDBX_COPY=$(abspath ./mdbx_copy) \
		-DMDBX_DEFRAG=$(abspath ./mdbx_defrag) -DMDBX_DROP=$(abspath ./mdbx_drop) \
		-DMDBX_DUMP=$(abspath ./mdbx_dump) -DMDBX_LOAD=$(abspath ./mdbx_load) \
		-DMDBX_STAT=$(abspath ./mdbx_stat) \
		-DMDBX_WORKDIR=$(abspath ./@migration-tool-roundtrip) \
		-P ut_and_examples/migration-tool-roundtrip.cmake

mdbx_migration_tool_roundtrip_nommap: $(MDBX_TOOLS) ut_and_examples/migration-tool-roundtrip.cmake
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 $(CMAKE) -DMDBX_CHK=$(abspath ./mdbx_chk) -DMDBX_COPY=$(abspath ./mdbx_copy) \
		-DMDBX_DEFRAG=$(abspath ./mdbx_defrag) -DMDBX_DROP=$(abspath ./mdbx_drop) \
		-DMDBX_DUMP=$(abspath ./mdbx_dump) -DMDBX_LOAD=$(abspath ./mdbx_load) \
		-DMDBX_STAT=$(abspath ./mdbx_stat) \
		-DMDBX_WORKDIR=$(abspath ./@migration-tool-roundtrip-nommap) \
		-P ut_and_examples/migration-tool-roundtrip.cmake

mdbx_migration_tool_roundtrip_nommap_tinycache: $(MDBX_TOOLS) ut_and_examples/migration-tool-roundtrip.cmake
	@echo '  RUN $@'
	$(QUIET)MDBX_FORCE_NO_DATA_MMAP=1 MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K $(CMAKE) \
		-DMDBX_CHK=$(abspath ./mdbx_chk) -DMDBX_COPY=$(abspath ./mdbx_copy) \
		-DMDBX_DEFRAG=$(abspath ./mdbx_defrag) -DMDBX_DROP=$(abspath ./mdbx_drop) \
		-DMDBX_DUMP=$(abspath ./mdbx_dump) -DMDBX_LOAD=$(abspath ./mdbx_load) \
		-DMDBX_STAT=$(abspath ./mdbx_stat) \
		-DMDBX_WORKDIR=$(abspath ./@migration-tool-roundtrip-nommap-tinycache) \
		-P ut_and_examples/migration-tool-roundtrip.cmake

################################################################################
# Amalgamated source code, i.e. distributed after `make dist`
MAN_SRCDIR := man1/

dist:
	@echo '  Starting 2026 libmdbx is distributed in an amalgamated source code form.'
	@echo '  So amalgamation is no longer required. Please update your build scripts.'

config-gnumake.h: @buildflags.tag mdbx.c $(lastword $(MAKEFILE_LIST)) LICENSE NOTICE COPYRIGHT
	@echo '  MAKE $@'
	$(QUIET)(echo '#define MDBX_BUILD_TIMESTAMP "$(MDBX_BUILD_TIMESTAMP)"' \
	&& echo "#define MDBX_BUILD_FLAGS \"$$(cat @buildflags.tag)\"" \
	&& echo '#define MDBX_BUILD_COMPILER "$(shell (LC_ALL=C $(CC) --version || echo 'Please use GCC or CLANG compatible compiler') | head -1)"' \
	&& echo '#define MDBX_BUILD_TARGET "$(shell set -o pipefail; (LC_ALL=C $(CC) -v 2>&1 | grep -i '^Target:' | cut -d ' ' -f 2- || (LC_ALL=C $(CC) --version | grep -qi e2k && echo E2K) || echo 'Please use GCC or CLANG compatible compiler') | head -1)"' \
	&& echo '#define MDBX_BUILD_CXX $(call select_by,MDBX_BUILD_CXX,1,0)' \
	&& echo '#define MDBX_BUILD_METADATA "$(MDBX_BUILD_METADATA)"' \
	) >$@

mdbx-dylib.o: config-gnumake.h mdbx.c mdbx.h mdbx-internals.h $(lastword $(MAKEFILE_LIST)) LICENSE NOTICE COPYRIGHT
	@echo '  CC $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config-gnumake.h"' -DLIBMDBX_EXPORTS=1 -c mdbx.c -o $@

mdbx-static.o: config-gnumake.h mdbx.c mdbx.h mdbx-internals.h $(lastword $(MAKEFILE_LIST)) LICENSE NOTICE COPYRIGHT
	@echo '  CC $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config-gnumake.h"' -ULIBMDBX_EXPORTS -c mdbx.c -o $@

mdbx++-dylib.o: config-gnumake.h mdbx.c++ $(HEADERS) mdbx-internals.h $(lastword $(MAKEFILE_LIST)) LICENSE NOTICE COPYRIGHT
	@echo '  CC $@'
	$(QUIET)$(CXX) $(CXXFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config-gnumake.h"' -DLIBMDBX_EXPORTS=1 -c mdbx.c++ -o $@

mdbx++-static.o: config-gnumake.h mdbx.c++ $(HEADERS) mdbx-internals.h $(lastword $(MAKEFILE_LIST)) LICENSE NOTICE COPYRIGHT
	@echo '  CC $@'
	$(QUIET)$(CXX) $(CXXFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config-gnumake.h"' -ULIBMDBX_EXPORTS -c mdbx.c++ -o $@

mdbx_%:	mdbx_%.c mdbx-static.o mdbx-wingetopt.h
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config-gnumake.h"' $< mdbx-static.o $(LDFLAGS) $(EXE_LDFLAGS) $(LIBS) -o $@

mdbx_%.static: mdbx_%.c mdbx-static.o
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config-gnumake.h"' $^ $(LDFLAGS) $(EXE_LDFLAGS) -static -Wl,--strip-all -o $@

mdbx_%.static-lto: mdbx_%.c config-gnumake.h mdbx.c mdbx.h
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) -Os -flto $(MDBX_BUILD_OPTIONS) '-DLIBMDBX_API=' '-DMDBX_CONFIG_H="config-gnumake.h"' \
		$< mdbx.c $(LDFLAGS) $(EXE_LDFLAGS) $(LIBS) -static -Wl,--strip-all -o $@

check smoke: test

install: $(LIBRARIES) $(MDBX_TOOLS) $(HEADERS)
	@echo '  INSTALLING...'
	$(QUIET)mkdir -p $(DESTDIR)$(prefix)/bin$(suffix) && \
		$(INSTALL) -p $(EXE_INSTALL_FLAGS) $(MDBX_TOOLS) $(DESTDIR)$(prefix)/bin$(suffix)/ && \
	mkdir -p $(DESTDIR)$(prefix)/lib$(suffix)/ && \
		$(INSTALL) -p $(EXE_INSTALL_FLAGS) $(filter-out libmdbx.a,$(LIBRARIES)) $(DESTDIR)$(prefix)/lib$(suffix)/ && \
	mkdir -p $(DESTDIR)$(prefix)/lib$(suffix)/ && \
		$(INSTALL) -p libmdbx.a $(DESTDIR)$(prefix)/lib$(suffix)/ && \
	mkdir -p $(DESTDIR)$(prefix)/include/ && \
		$(INSTALL) -p -m 444 $(HEADERS) $(DESTDIR)$(prefix)/include/ && \
	mkdir -p $(DESTDIR)$(mandir)/man1/ && \
		$(INSTALL) -p -m 444 $(addprefix $(MAN_SRCDIR), $(MANPAGES)) $(DESTDIR)$(mandir)/man1/

install-strip: EXE_INSTALL_FLAGS = -s
install-strip: install

install-no-strip: EXE_INSTALL_FLAGS =
install-no-strip: install

uninstall:
	@echo '  UNINSTALLING/REMOVE...'
	$(QUIET)rm -f $(addprefix $(DESTDIR)$(prefix)/bin$(suffix)/,$(MDBX_TOOLS)) \
		$(addprefix $(DESTDIR)$(prefix)/lib$(suffix)/,$(LIBRARIES)) \
		$(addprefix $(DESTDIR)$(prefix)/include/,$(HEADERS)) \
		$(addprefix $(DESTDIR)$(mandir)/man1/,$(MANPAGES))

################################################################################
# Benchmarking by ioarena

ifeq ($(origin IOARENA),undefined)
IOARENA := $(shell \
  (test -x ../ioarena/@BUILD/src/ioarena && echo ../ioarena/@BUILD/src/ioarena) || \
  (test -x ../../@BUILD/src/ioarena && echo ../../@BUILD/src/ioarena) || \
  (test -x ../../src/ioarena && echo ../../src/ioarena) || which ioarena 2>&- || \
  (echo false && echo '$(TIP) Clone and build the https://abf.io/erthink/ioarena.git within a neighbouring directory for availability of benchmarking.' >&2))
endif
NN	?= 25000000
BENCH_CRUD_MODE ?= nosync
MIGRATION_BENCH_NN ?= 10000
MIGRATION_BENCH_MODE ?= lazy
MIGRATION_BENCH_MIN_RATIO ?= 0.60
MIGRATION_BENCH_READ_MIN_RATIO ?= 0.70
MIGRATION_BENCH_DEFAULT_LOG := perf-mdbx_$(MIGRATION_BENCH_NN)_default-$(MIGRATION_BENCH_MODE).log
MIGRATION_BENCH_FORCED_LOG := perf-mdbx_$(MIGRATION_BENCH_NN)_forced-nomap-$(MIGRATION_BENCH_MODE).log

bench-clean:
	@echo '  REMOVE bench-*.txt _ioarena/*'
	$(QUIET)rm -rf bench-*.txt _ioarena/*

re-bench: bench-clean bench

ifeq ($(or $(IOARENA),false),false)
bench bench-quartet bench-triplet bench-couple:
	$(QUIET)echo 'The `ioarena` benchmark is required.' >&2 && \
	echo 'Please clone and build the https://abf.io/erthink/ioarena.git within a neighbouring `ioarena` directory.' >&2 && \
	false

mdbx_migration_bench_lazy mdbx_migration_bench_lazy_repeat mdbx_migration_check:
	$(QUIET)echo 'The `ioarena` benchmark is required for migration performance checks.' >&2 && \
	echo 'Please clone and build the https://abf.io/erthink/ioarena.git within a neighbouring `ioarena` directory.' >&2 && \
	false

else

.PHONY: bench bench-clean bench-couple re-bench bench-quartet bench-triplet

define bench-rule
bench-$(1)_$(2).txt: $(3) $(IOARENA) $(lastword $(MAKEFILE_LIST))
	@echo '  RUNNING ioarena for $1/$2...'
	$(QUIET)(export LD_LIBRARY_PATH="./:$$$${LD_LIBRARY_PATH}"; \
		ldd $(IOARENA) | grep -i $(1) && \
		$(IOARENA) -D $(1) -B batch -m $(BENCH_CRUD_MODE) -n $(2) \
			| tee $$@ | grep throughput | $(SED) 's/throughput/batch×N/' && \
		$(IOARENA) -D $(1) -B crud -m $(BENCH_CRUD_MODE) -n $(2) \
			| tee -a $$@ | grep throughput | $(SED) 's/throughput/   crud/' && \
		$(IOARENA) -D $(1) -B iterate,get,iterate,get,iterate -m $(BENCH_CRUD_MODE) -r 4 -n $(2) \
			| tee -a $$@ | grep throughput | $(SED) '0,/throughput/{s/throughput/iterate/};s/throughput/    get/' && \
		$(IOARENA) -D $(1) -B delete -m $(BENCH_CRUD_MODE) -n $(2) \
			| tee -a $$@ | grep throughput | $(SED) 's/throughput/ delete/' && \
	true) || mv -f $$@ $$@.error

endef

$(eval $(call bench-rule,mdbx,$(NN),libmdbx.$(SO_SUFFIX)))

$(eval $(call bench-rule,sophia,$(NN)))
$(eval $(call bench-rule,leveldb,$(NN)))
$(eval $(call bench-rule,rocksdb,$(NN)))
$(eval $(call bench-rule,wiredtiger,$(NN)))
$(eval $(call bench-rule,forestdb,$(NN)))
$(eval $(call bench-rule,lmdb,$(NN)))
$(eval $(call bench-rule,nessdb,$(NN)))
$(eval $(call bench-rule,sqlite3,$(NN)))
$(eval $(call bench-rule,ejdb,$(NN)))
$(eval $(call bench-rule,vedisdb,$(NN)))
$(eval $(call bench-rule,dummy,$(NN)))
bench: bench-mdbx_$(NN).txt
bench-quartet: bench-mdbx_$(NN).txt bench-lmdb_$(NN).txt bench-rocksdb_$(NN).txt bench-wiredtiger_$(NN).txt
bench-triplet: bench-mdbx_$(NN).txt bench-lmdb_$(NN).txt bench-sqlite3_$(NN).txt
bench-couple: bench-mdbx_$(NN).txt bench-lmdb_$(NN).txt

mdbx_migration_bench_lazy: libmdbx.$(SO_SUFFIX) $(IOARENA)
	@echo '  RUN migration benchmark default explicit storage -> $(MIGRATION_BENCH_DEFAULT_LOG)'
	$(QUIET)rm -f $(MIGRATION_BENCH_DEFAULT_LOG) $(MIGRATION_BENCH_FORCED_LOG) \
		bench-mdbx_$(MIGRATION_BENCH_NN).txt bench-mdbx_$(MIGRATION_BENCH_NN).txt.error
	$(QUIET)(export LD_LIBRARY_PATH="./:$${LD_LIBRARY_PATH}"; unset MDBX_FORCE_NO_DATA_MMAP; \
		echo '  RUNNING ioarena for mdbx/$(MIGRATION_BENCH_NN)...' > $(MIGRATION_BENCH_DEFAULT_LOG) && \
		ldd $(IOARENA) | grep -i mdbx >> $(MIGRATION_BENCH_DEFAULT_LOG) && \
		$(IOARENA) -D mdbx -B batch -m $(MIGRATION_BENCH_MODE) -n $(MIGRATION_BENCH_NN) \
			| grep throughput | $(SED) 's/throughput/batch×N/' | tee -a $(MIGRATION_BENCH_DEFAULT_LOG) && \
		$(IOARENA) -D mdbx -B crud -m $(MIGRATION_BENCH_MODE) -n $(MIGRATION_BENCH_NN) \
			| grep throughput | $(SED) 's/throughput/   crud/' | tee -a $(MIGRATION_BENCH_DEFAULT_LOG) && \
		$(IOARENA) -D mdbx -B iterate,get,iterate,get,iterate -m $(MIGRATION_BENCH_MODE) -r 4 \
			-n $(MIGRATION_BENCH_NN) | grep throughput \
			| $(SED) '0,/throughput/{s/throughput/iterate/};s/throughput/    get/' \
			| tee -a $(MIGRATION_BENCH_DEFAULT_LOG) && \
		$(IOARENA) -D mdbx -B delete -m $(MIGRATION_BENCH_MODE) -n $(MIGRATION_BENCH_NN) \
			| grep throughput | $(SED) 's/throughput/ delete/' | tee -a $(MIGRATION_BENCH_DEFAULT_LOG))
	@echo '  RUN migration benchmark forced explicit storage -> $(MIGRATION_BENCH_FORCED_LOG)'
	$(QUIET)(export LD_LIBRARY_PATH="./:$${LD_LIBRARY_PATH}"; export MDBX_FORCE_NO_DATA_MMAP=1; \
		echo '  RUNNING ioarena for mdbx/$(MIGRATION_BENCH_NN)...' > $(MIGRATION_BENCH_FORCED_LOG) && \
		ldd $(IOARENA) | grep -i mdbx >> $(MIGRATION_BENCH_FORCED_LOG) && \
		$(IOARENA) -D mdbx -B batch -m $(MIGRATION_BENCH_MODE) -n $(MIGRATION_BENCH_NN) \
			| grep throughput | $(SED) 's/throughput/batch×N/' | tee -a $(MIGRATION_BENCH_FORCED_LOG) && \
		$(IOARENA) -D mdbx -B crud -m $(MIGRATION_BENCH_MODE) -n $(MIGRATION_BENCH_NN) \
			| grep throughput | $(SED) 's/throughput/   crud/' | tee -a $(MIGRATION_BENCH_FORCED_LOG) && \
		$(IOARENA) -D mdbx -B iterate,get,iterate,get,iterate -m $(MIGRATION_BENCH_MODE) -r 4 \
			-n $(MIGRATION_BENCH_NN) | grep throughput \
			| $(SED) '0,/throughput/{s/throughput/iterate/};s/throughput/    get/' \
			| tee -a $(MIGRATION_BENCH_FORCED_LOG) && \
		$(IOARENA) -D mdbx -B delete -m $(MIGRATION_BENCH_MODE) -n $(MIGRATION_BENCH_NN) \
			| grep throughput | $(SED) 's/throughput/ delete/' | tee -a $(MIGRATION_BENCH_FORCED_LOG))
	@echo '  CHECK migration benchmark logs'
	$(QUIET)for log in $(MIGRATION_BENCH_DEFAULT_LOG) $(MIGRATION_BENCH_FORCED_LOG); do \
		grep -q 'batch×N:' "$$log" && grep -q 'crud:' "$$log" && grep -q 'iterate:' "$$log" && \
			grep -q 'get:' "$$log" && grep -q 'delete:' "$$log" || \
			{ echo "Missing benchmark summary in $$log" >&2; tail -n 40 "$$log" >&2; exit 1; }; \
		if grep -Eiq 'error|failed|panic|assert|restore|cursor|MDBX_' "$$log"; then \
			echo "Benchmark diagnostics found in $$log" >&2; \
			grep -Ein 'error|failed|panic|assert|restore|cursor|MDBX_' "$$log" >&2; \
			exit 1; \
		fi; \
	done
	@echo '  CHECK migration benchmark ratios'
	$(QUIET)$(AWK) -v min='$(MIGRATION_BENCH_MIN_RATIO)' -v read_min='$(MIGRATION_BENCH_READ_MIN_RATIO)' ' \
		function bench_key(line) { \
			if (line ~ /^[[:space:]]*batch/) return "batch"; \
			if (line ~ /^[[:space:]]*crud:/) return "crud"; \
			if (line ~ /^[[:space:]]*iterate:/) return "iterate"; \
			if (line ~ /^[[:space:]]*get:/) return "get"; \
			if (line ~ /^[[:space:]]*delete:/) return "delete"; \
			return ""; \
		} \
		function to_ops(text, n) { \
			n = text + 0; \
			if (text ~ /Gops\/s$$/) n *= 1000000000; \
			else if (text ~ /Mops\/s$$/) n *= 1000000; \
			else if (text ~ /Kops\/s$$/) n *= 1000; \
			return n; \
		} \
			FNR == NR { key = bench_key($$0); if (key != "") base[key] = to_ops($$2); next } \
			{ key = bench_key($$0); if (key != "") forced[key] = to_ops($$2) } \
			END { \
				split("batch crud iterate get delete", keys); \
				for (i = 1; i <= 5; ++i) { \
					key = keys[i]; \
					if (!(key in base) || !(key in forced) || base[key] <= 0 || forced[key] <= 0) { \
						printf("Missing parsed benchmark value for %s\n", key) > "/dev/stderr"; bad = 1; continue; \
					} \
					limit = (key == "iterate" || key == "get") ? read_min : min; \
					ratio = forced[key] / base[key]; \
					printf("  PERF %-7s forced/default %.3f (min %.3f)\n", key, ratio, limit); \
					if (ratio < limit) { \
						printf("Benchmark regression for %s: forced/default %.3f < %.3f\n", key, ratio, limit) > "/dev/stderr"; \
						bad = 1; \
					} \
				} \
				exit bad; \
			}' $(MIGRATION_BENCH_DEFAULT_LOG) $(MIGRATION_BENCH_FORCED_LOG)
	@tail -n 8 $(MIGRATION_BENCH_DEFAULT_LOG)
	@tail -n 8 $(MIGRATION_BENCH_FORCED_LOG)

mdbx_migration_bench_lazy_repeat: libmdbx.$(SO_SUFFIX) $(IOARENA)
	@echo '  RUN $@ ($(MIGRATION_BENCH_REPEAT)x paired migration benchmark)'
	$(QUIET)for n in $$(seq 1 $(MIGRATION_BENCH_REPEAT)); do \
		default_log="perf-mdbx_$(MIGRATION_BENCH_NN)_default-$(MIGRATION_BENCH_MODE)-repeat-$${n}.log"; \
		forced_log="perf-mdbx_$(MIGRATION_BENCH_NN)_forced-nomap-$(MIGRATION_BENCH_MODE)-repeat-$${n}.log"; \
		echo "  RUN migration benchmark repeat $$n/$(MIGRATION_BENCH_REPEAT)"; \
		$(MAKE) -f GNUmakefile mdbx_migration_bench_lazy \
			MIGRATION_BENCH_DEFAULT_LOG="$$default_log" \
			MIGRATION_BENCH_FORCED_LOG="$$forced_log" || exit $$?; \
	done

mdbx_migration_check:
	@echo '  RUN migration correctness, sanitizer, memcheck, audit and performance gates'
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_smoke_nommap
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_smoke_nommap_tinycache
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_smoke_stress_nommap_tinycache
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_smoke_randomized_stress_nommap_tinycache
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_smoke_crash_stress_nommap_tinycache
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_extended_stress_nommap_tinycache
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_fault_injection_nommap
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_fault_injection_nommap_tinycache
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_public_ctest
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_repeat_ctest
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_assertion_ctest
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_fault_ctest
	$(QUIET)$(MAKE) -f GNUmakefile test-asan
	$(QUIET)$(MAKE) -f GNUmakefile test-ubsan
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_audit_ctest
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_memcheck_ctest
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_leak_ctest
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_tool_roundtrip
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_tool_roundtrip_nommap
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_tool_roundtrip_nommap_tinycache
	$(QUIET)$(MAKE) -f GNUmakefile mdbx_migration_bench_lazy_repeat
	@echo '  PASS $@'

# $(eval $(call bench-rule,debug,10))
# .PHONY: bench-debug
# bench-debug: bench-debug_10.txt

endif
