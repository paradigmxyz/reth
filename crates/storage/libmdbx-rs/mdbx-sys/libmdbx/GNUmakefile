# This makefile is for GNU Make 3.80 or above, and nowadays provided
# just for compatibility and preservation of traditions.
#
# Please use CMake in case of any difficulties or
# problems with this old-school's magic.
#
################################################################################
#
# Basic internal definitios. For a customizable variables and options see below.
#
$(info // The GNU Make $(MAKE_VERSION))
SHELL         := $(shell env bash -c 'echo $$BASH')
MAKE_VERx3    := $(shell printf "%3s%3s%3s" $(subst ., ,$(MAKE_VERSION)))
make_lt_3_81  := $(shell expr "$(MAKE_VERx3)" "<" "  3 81")
ifneq ($(make_lt_3_81),0)
$(error Please use GNU Make 3.81 or above)
endif
make_ge_4_1   := $(shell expr "$(MAKE_VERx3)" ">=" "  4  1")
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
# in README and source code (see src/options.h) if you do.
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

# build options
MDBX_BUILD_OPTIONS   ?=-DNDEBUG=1
MDBX_BUILD_TIMESTAMP ?=$(shell date +%Y-%m-%dT%H:%M:%S%z)
MDBX_BUILD_CXX       ?= YES

# probe and compose common compiler flags with variable expansion trick (seems this work two times per session for GNU Make 3.81)
CFLAGS       ?= $(strip $(eval CFLAGS := -std=gnu11 -O2 -g -Wall -Werror -Wextra -Wpedantic -ffunction-sections -fPIC -fvisibility=hidden -pthread -Wno-error=attributes $$(shell for opt in -fno-semantic-interposition -Wno-unused-command-line-argument -Wno-tautological-compare; do [ -z "$$$$($(CC) '-DMDBX_BUILD_FLAGS="probe"' $$$${opt} -c $(SRC_PROBE_C) -o /dev/null >/dev/null 2>&1 || echo failed)" ] && echo "$$$${opt} "; done)$(CFLAGS_EXTRA))$(CFLAGS))

# choosing C++ standard with variable expansion trick (seems this work two times per session for GNU Make 3.81)
CXXSTD       ?= $(eval CXXSTD := $$(shell for std in gnu++23 c++23 gnu++2b c++2b gnu++20 c++20 gnu++2a c++2a gnu++17 c++17 gnu++1z c++1z gnu++14 c++14 gnu++1y c++1y gnu+11 c++11 gnu++0x c++0x; do $(CXX) -std=$$$${std} -c $(SRC_PROBE_CXX) -o /dev/null 2>probe4std-$$$${std}.err >/dev/null && echo "-std=$$$${std}" && exit; done))$(CXXSTD)
CXXFLAGS     ?= $(strip $(CXXSTD) $(filter-out -std=gnu11,$(CFLAGS)))

# libraries and options for linking
EXE_LDFLAGS  ?= -pthread
ifneq ($(make_ge_4_1),1)
# don't use variable expansion trick as workaround for bugs of GNU Make before 4.1
LIBS         ?= $(shell $(uname2libs))
LDFLAGS      ?= $(shell $(uname2ldflags))
LIB_STDCXXFS ?= $(shell echo '$(cxx_filesystem_probe)' | cat mdbx.h++ - | sed $$'1s/\xef\xbb\xbf//' | $(CXX) -x c++ $(CXXFLAGS) -Wno-error - -Wl,--allow-multiple-definition -lstdc++fs $(LIBS) $(LDFLAGS) $(EXE_LDFLAGS) -o /dev/null 2>probe4lstdfs.err >/dev/null && echo '-Wl,--allow-multiple-definition -lstdc++fs')
else
# using variable expansion trick to avoid repeaded probes
LIBS         ?= $(eval LIBS := $$(shell $$(uname2libs)))$(LIBS)
LDFLAGS      ?= $(eval LDFLAGS := $$(shell $$(uname2ldflags)))$(LDFLAGS)
LIB_STDCXXFS ?= $(eval LIB_STDCXXFS := $$(shell echo '$$(cxx_filesystem_probe)' | cat mdbx.h++ - | sed $$$$'1s/\xef\xbb\xbf//' | $(CXX) -x c++ $(CXXFLAGS) -Wno-error - -Wl,--allow-multiple-definition -lstdc++fs $(LIBS) $(LDFLAGS) $(EXE_LDFLAGS) -o /dev/null 2>probe4lstdfs.err >/dev/null && echo '-Wl,--allow-multiple-definition -lstdc++fs'))$(LIB_STDCXXFS)
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
      echo '-Wl,--gc-sections,-O1';
      ;;
    *)
      $(LD) --help 2>/dev/null | grep -q -- --gc-sections && echo '-Wl,--gc-sections,-z,relro,-O1';
      $(LD) --help 2>/dev/null | grep -q -- -dead_strip && echo '-Wl,-dead_strip';
      ;;
  esac
endef

# TIP: try add the'-Wl, --no-as-needed,-lrt' for ability to built with modern glibc, but then use with the old.
define uname2libs
  case "$(UNAME)" in
    CYGWIN*|MINGW*|MSYS*|Windows*)
      echo '-lm -lntdll -lwinmm';
      ;;
    *SunOS*|*Solaris*)
      echo '-lm -lkstat -lrt';
      ;;
    *Darwin*|OpenBSD*)
      echo '-lm';
      ;;
    *)
      echo '-lm -lrt';
      ;;
  esac
endef

SO_SUFFIX  := $(shell $(uname2sosuffix))
HEADERS    := mdbx.h mdbx.h++
LIBRARIES  := libmdbx.a libmdbx.$(SO_SUFFIX)
TOOLS      := mdbx_stat mdbx_copy mdbx_dump mdbx_load mdbx_chk mdbx_drop
MANPAGES   := mdbx_stat.1 mdbx_copy.1 mdbx_dump.1 mdbx_load.1 mdbx_chk.1 mdbx_drop.1
TIP        := // TIP:

.PHONY: all help options lib libs tools clean install uninstall check_buildflags_tag tools-static
.PHONY: install-strip install-no-strip strip libmdbx mdbx show-options lib-static lib-shared

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

all: show-options $(LIBRARIES) $(TOOLS)

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

show-options:
	@echo "  MDBX_BUILD_OPTIONS   = $(MDBX_BUILD_OPTIONS)"
	@echo "  MDBX_BUILD_CXX       = $(MDBX_BUILD_CXX)"
	@echo "  MDBX_BUILD_TIMESTAMP = $(MDBX_BUILD_TIMESTAMP)"
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
	@echo ""
	@echo "## Assortment items for MDBX_BUILD_OPTIONS:"
	@echo "##   Note that the defaults should already be correct for most platforms;"
	@echo "##   you should not need to change any of these. Read their descriptions"
	@echo "##   in README and source code (see mdbx.c) if you do."
	@grep -h '#ifndef MDBX_' mdbx.c | grep -v BUILD | uniq | sed 's/#ifndef /  /'

lib libs libmdbx mdbx: libmdbx.a libmdbx.$(SO_SUFFIX)

tools: $(TOOLS)
tools-static: $(addsuffix .static,$(TOOLS)) $(addsuffix .static-lto,$(TOOLS))

strip: all
	@echo '  STRIP libmdbx.$(SO_SUFFIX) $(TOOLS)'
	$(TRACE )strip libmdbx.$(SO_SUFFIX) $(TOOLS)

clean:
	@echo '  REMOVE ...'
	$(QUIET)rm -rf $(TOOLS) mdbx_test @* *.[ao] *.[ls]o *.$(SO_SUFFIX) *.dSYM *~ tmp.db/* \
		*.gcov *.log *.err src/*.o test/*.o mdbx_example dist \
		config.h src/config.h src/version.c *.tar* buildflags.tag \
		mdbx_*.static mdbx_*.static-lto

MDBX_BUILD_FLAGS =$(strip MDBX_BUILD_CXX=$(MDBX_BUILD_CXX) $(MDBX_BUILD_OPTIONS) $(call select_by,MDBX_BUILD_CXX,$(CXXFLAGS) $(LDFLAGS) $(LIB_STDCXXFS) $(LIBS),$(CFLAGS) $(LDFLAGS) $(LIBS)))
check_buildflags_tag:
	$(QUIET)if [ "$(MDBX_BUILD_FLAGS)" != "$$(cat buildflags.tag 2>&1)" ]; then \
		echo -n "  CLEAN for build with specified flags..." && \
		$(MAKE) IOARENA=false CXXSTD= -s clean >/dev/null && echo " Ok" && \
		echo '$(MDBX_BUILD_FLAGS)' > buildflags.tag; \
	fi

buildflags.tag: check_buildflags_tag

lib-static libmdbx.a: mdbx-static.o $(call select_by,MDBX_BUILD_CXX,mdbx++-static.o)
	@echo '  AR $@'
	$(QUIET)$(AR) rcs $@ $? $(HUSH)

lib-shared libmdbx.$(SO_SUFFIX): mdbx-dylib.o $(call select_by,MDBX_BUILD_CXX,mdbx++-dylib.o)
	@echo '  LD $@'
	$(QUIET)$(call select_by,MDBX_BUILD_CXX,$(CXX) $(CXXFLAGS),$(CC) $(CFLAGS)) $^ -pthread -shared $(LDFLAGS) $(call select_by,MDBX_BUILD_CXX,$(LIB_STDCXXFS)) $(LIBS) -o $@


################################################################################
# Amalgamated source code, i.e. distributed after `make dist`
MAN_SRCDIR := man1/

config.h: buildflags.tag mdbx.c $(lastword $(MAKEFILE_LIST))
	@echo '  MAKE $@'
	$(QUIET)(echo '#define MDBX_BUILD_TIMESTAMP "$(MDBX_BUILD_TIMESTAMP)"' \
	&& echo "#define MDBX_BUILD_FLAGS \"$$(cat buildflags.tag)\"" \
	&& echo '#define MDBX_BUILD_COMPILER "$(shell (LC_ALL=C $(CC) --version || echo 'Please use GCC or CLANG compatible compiler') | head -1)"' \
	&& echo '#define MDBX_BUILD_TARGET "$(shell set -o pipefail; (LC_ALL=C $(CC) -v 2>&1 | grep -i '^Target:' | cut -d ' ' -f 2- || (LC_ALL=C $(CC) --version | grep -qi e2k && echo E2K) || echo 'Please use GCC or CLANG compatible compiler') | head -1)"' \
	) >$@

mdbx-dylib.o: config.h mdbx.c mdbx.h $(lastword $(MAKEFILE_LIST))
	@echo '  CC $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config.h"' -DLIBMDBX_EXPORTS=1 -c mdbx.c -o $@

mdbx-static.o: config.h mdbx.c mdbx.h $(lastword $(MAKEFILE_LIST))
	@echo '  CC $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config.h"' -ULIBMDBX_EXPORTS -c mdbx.c -o $@

mdbx++-dylib.o: config.h mdbx.c++ mdbx.h mdbx.h++ $(lastword $(MAKEFILE_LIST))
	@echo '  CC $@'
	$(QUIET)$(CXX) $(CXXFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config.h"' -DLIBMDBX_EXPORTS=1 -c mdbx.c++ -o $@

mdbx++-static.o: config.h mdbx.c++ mdbx.h mdbx.h++ $(lastword $(MAKEFILE_LIST))
	@echo '  CC $@'
	$(QUIET)$(CXX) $(CXXFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config.h"' -ULIBMDBX_EXPORTS -c mdbx.c++ -o $@

mdbx_%:	mdbx_%.c mdbx-static.o
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config.h"' $^ $(EXE_LDFLAGS) $(LIBS) -o $@

mdbx_%.static: mdbx_%.c mdbx-static.o
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) $(MDBX_BUILD_OPTIONS) '-DMDBX_CONFIG_H="config.h"' $^ $(EXE_LDFLAGS) -static -Wl,--strip-all -o $@

mdbx_%.static-lto: mdbx_%.c config.h mdbx.c mdbx.h
	@echo '  CC+LD $@'
	$(QUIET)$(CC) $(CFLAGS) -Os -flto $(MDBX_BUILD_OPTIONS) '-DLIBMDBX_API=' '-DMDBX_CONFIG_H="config.h"' \
		$< mdbx.c $(EXE_LDFLAGS) $(LIBS) -static -Wl,--strip-all -o $@


install: $(LIBRARIES) $(TOOLS) $(HEADERS)
	@echo '  INSTALLING...'
	$(QUIET)mkdir -p $(DESTDIR)$(prefix)/bin$(suffix) && \
		$(INSTALL) -p $(EXE_INSTALL_FLAGS) $(TOOLS) $(DESTDIR)$(prefix)/bin$(suffix)/ && \
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
	$(QUIET)rm -f $(addprefix $(DESTDIR)$(prefix)/bin$(suffix)/,$(TOOLS)) \
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
  (echo false && echo '$(TIP) Clone and build the https://github.com/pmwkaa/ioarena.git within a neighbouring directory for availability of benchmarking.' >&2))
endif
NN	?= 25000000
BENCH_CRUD_MODE ?= nosync

bench-clean:
	@echo '  REMOVE bench-*.txt _ioarena/*'
	$(QUIET)rm -rf bench-*.txt _ioarena/*

re-bench: bench-clean bench

ifeq ($(or $(IOARENA),false),false)
bench bench-quartet bench-triplet bench-couple:
	$(QUIET)echo 'The `ioarena` benchmark is required.' >&2 && \
	echo 'Please clone and build the https://github.com/pmwkaa/ioarena.git within a neighbouring `ioarena` directory.' >&2 && \
	false

else

.PHONY: bench bench-clean bench-couple re-bench bench-quartet bench-triplet

define bench-rule
bench-$(1)_$(2).txt: $(3) $(IOARENA) $(lastword $(MAKEFILE_LIST))
	@echo '  RUNNING ioarena for $1/$2...'
	$(QUIET)(export LD_LIBRARY_PATH="./:$$$${LD_LIBRARY_PATH}"; \
		ldd $(IOARENA) && \
		$(IOARENA) -D $(1) -B crud -m $(BENCH_CRUD_MODE) -n $(2) \
			| tee $$@ | grep throughput && \
		$(IOARENA) -D $(1) -B iterate,get,iterate,get,iterate -m $(BENCH_CRUD_MODE) -r 4 -n $(2) \
			| tee -a $$@ | grep throughput \
	) || mv -f $$@ $$@.error

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

# $(eval $(call bench-rule,debug,10))
# .PHONY: bench-debug
# bench-debug: bench-debug_10.txt

endif
