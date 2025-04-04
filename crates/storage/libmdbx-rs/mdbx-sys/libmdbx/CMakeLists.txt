# Copyright (c) 2020-2025 Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> ###############################################
# SPDX-License-Identifier: Apache-2.0
#
# Donations are welcome to ETH `0xD104d8f8B2dC312aaD74899F83EBf3EEBDC1EA3A`. Всё будет хорошо!

# libmdbx = { Revised and extended descendant of Symas LMDB. } Please see README.md at
# https://gitflic.ru/project/erthink/libmdbx
#
# Libmdbx is superior to LMDB in terms of features and reliability, not inferior in performance. libmdbx works on Linux,
# FreeBSD, MacOS X and other systems compliant with POSIX.1-2008, but also support Windows as a complementary platform.
#
# The next version is under active non-public development and will be released as MithrilDB and libmithrildb for
# libraries & packages. Admittedly mythical Mithril is resembling silver but being stronger and lighter than steel.
# Therefore MithrilDB is rightly relevant name.
#
# MithrilDB will be radically different from libmdbx by the new database format and API based on C++17, as well as the
# Apache 2.0 License. The goal of this revolution is to provide a clearer and robust API, add more features and new
# valuable properties of database.

if(CMAKE_VERSION VERSION_LESS 3.8.2)
  cmake_minimum_required(VERSION 3.0.2)
elseif(CMAKE_VERSION VERSION_LESS 3.12)
  cmake_minimum_required(VERSION 3.8.2)
else()
  cmake_minimum_required(VERSION 3.12)
endif()

cmake_policy(PUSH)
cmake_policy(VERSION ${CMAKE_MINIMUM_REQUIRED_VERSION})
if(NOT CMAKE_VERSION VERSION_LESS 3.21)
  cmake_policy(SET CMP0126 NEW)
endif()
if(NOT CMAKE_VERSION VERSION_LESS 3.17)
  cmake_policy(SET CMP0102 NEW)
endif()
if(NOT CMAKE_VERSION VERSION_LESS 3.15)
  cmake_policy(SET CMP0091 NEW)
endif()
if(NOT CMAKE_VERSION VERSION_LESS 3.13)
  cmake_policy(SET CMP0077 NEW)
endif()
if(NOT CMAKE_VERSION VERSION_LESS 3.12)
  cmake_policy(SET CMP0075 NEW)
endif()
if(NOT CMAKE_VERSION VERSION_LESS 3.9)
  cmake_policy(SET CMP0068 NEW)
  cmake_policy(SET CMP0069 NEW)
  include(CheckIPOSupported)
  check_ipo_supported(RESULT CMAKE_INTERPROCEDURAL_OPTIMIZATION_AVAILABLE)
else()
  set(CMAKE_INTERPROCEDURAL_OPTIMIZATION_AVAILABLE FALSE)
endif()

cmake_policy(SET CMP0054 NEW)

if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/.git"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/COPYRIGHT"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/LICENSE"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/NOTICE"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/README.md"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx.h++"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/alloy.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-cold.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-copy.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-cursor.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-dbi.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-env.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-extra.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-key-transform.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-misc.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-opts.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-range-estimate.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-txn-data.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/api-txn.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/atomics-ops.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/atomics-types.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/audit.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/chk.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/cogs.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/cogs.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/coherency.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/config.h.in"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/cursor.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/cursor.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/dbi.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/dbi.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/debug_begin.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/debug_end.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/dpl.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/dpl.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/dxb.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/env.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/essentials.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/gc-get.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/gc-put.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/gc.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/global.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/internals.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/layout-dxb.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/layout-lck.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/lck-posix.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/lck-windows.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/lck.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/lck.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/logging_and_debug.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/logging_and_debug.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/man1/mdbx_chk.1"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/man1/mdbx_copy.1"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/man1/mdbx_drop.1"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/man1/mdbx_dump.1"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/man1/mdbx_load.1"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/man1/mdbx_stat.1"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/mdbx.c++"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/meta.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/meta.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/mvcc-readers.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/node.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/node.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/ntdll.def"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/options.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/osal.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/osal.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/page-get.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/page-iov.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/page-iov.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/page-ops.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/page-ops.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tree-search.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/pnl.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/pnl.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/preface.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/proto.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/refund.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/sort.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/spill.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/spill.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/table.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tls.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tls.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tools/chk.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tools/copy.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tools/drop.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tools/dump.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tools/load.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tools/stat.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tools/wingetopt.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tools/wingetopt.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/tree-ops.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/txl.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/txl.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/txn.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/unaligned.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/utils.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/utils.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/version.c.in"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/walk.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/walk.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/windows-import.c"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/windows-import.h"
   AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/test/CMakeLists.txt")
  set(MDBX_AMALGAMATED_SOURCE FALSE)
  find_program(GIT git)
  if(NOT GIT)
    message(SEND_ERROR "Git command-line tool not found")
  endif()
  set(MDBX_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/src")
elseif(
  EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/VERSION.json"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/LICENSE"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/NOTICE"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx.c"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx.c++"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx.h"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx.h++"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx_chk.c"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx_copy.c"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx_dump.c"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx_load.c"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx_stat.c"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/mdbx_drop.c"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/ntdll.def"
  AND EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/config.h.in")
  set(MDBX_AMALGAMATED_SOURCE TRUE)
  set(MDBX_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
else()
  message(
    FATAL_ERROR
      "\nThe set of libmdbx source code files is incomplete!  "
      "Instead just follow the https://libmdbx.dqdkfa.ru/usage.html  " "PLEASE, AVOID USING ANY OTHER TECHNIQUES.")
endif()

# Provide version
include(cmake/utils.cmake)
set(MDBX_BUILD_METADATA
    "${MDBX_BUILD_METADATA}"
    CACHE STRING "An extra/custom information provided during libmdbx build")
semver_provide(MDBX "${CMAKE_CURRENT_SOURCE_DIR}" "${CMAKE_CURRENT_BINARY_DIR}" "${MDBX_BUILD_METADATA}" FALSE)
message(STATUS "libmdbx version is ${MDBX_VERSION}")

if(DEFINED PROJECT_NAME)
  option(MDBX_FORCE_BUILD_AS_MAIN_PROJECT
         "Force libmdbx to full control build options even it added as a subdirectory to your project." OFF)
endif()

if(DEFINED PROJECT_NAME AND NOT MDBX_FORCE_BUILD_AS_MAIN_PROJECT)
  set(SUBPROJECT ON)
  set(NOT_SUBPROJECT OFF)
  if(NOT MDBX_AMALGAMATED_SOURCE AND NOT DEFINED BUILD_TESTING)
    set(BUILD_TESTING OFF)
  endif()
  enable_language(C)
else()
  set(SUBPROJECT OFF)
  set(NOT_SUBPROJECT ON)

  # Setup Apple stuff which should be set prior to the first project() or enable_language()
  if(APPLE)
    # Enable universal binaries for macOS (target arm64 and x86_64)
    if(NOT DEFINED CMAKE_OSX_ARCHITECTURES)
      set(CMAKE_OSX_ARCHITECTURES "arm64;x86_64")
    endif()

    # Set the minimum macOS deployment target if not already defined
    if(NOT DEFINED CMAKE_OSX_DEPLOYMENT_TARGET)
      set(CMAKE_OSX_DEPLOYMENT_TARGET "13.0")
    endif()
  endif()

  project(libmdbx C)
  if(NOT MDBX_AMALGAMATED_SOURCE AND NOT DEFINED BUILD_TESTING)
    set(BUILD_TESTING ON)
  endif()
endif()

if(NOT MDBX_AMALGAMATED_SOURCE)
  include(CTest)
  option(MDBX_ENABLE_TESTS "Build libmdbx tests." ${BUILD_TESTING})
elseif(DEFINED MDBX_ENABLE_TESTS AND MDBX_ENABLE_TESTS)
  message(WARNING "MDBX_ENABLE_TESTS=${MDBX_ENABLE_TESTS}: But amalgamated source code don't includes tests.")
  set(MDBX_ENABLE_TESTS OFF)
endif()

# Try to find a C++ compiler unless sure that this is unnecessary.
if(NOT CMAKE_CXX_COMPILER_LOADED)
  include(CheckLanguage)
  if(NOT DEFINED MDBX_BUILD_CXX
     OR MDBX_BUILD_CXX
     OR (NOT MDBX_AMALGAMATED_SOURCE AND (NOT DEFINED MDBX_ENABLE_TESTS OR MDBX_ENABLE_TESTS)))
    check_language(CXX)
    if(CMAKE_CXX_COMPILER)
      enable_language(CXX)
    endif()
  endif()
else()
  enable_language(CXX)
endif()

# Set default build type to Release. This is to ease a User's life.
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE
      Release
      CACHE STRING "Choose the type of build, options are: Debug Release RelWithDebInfo MinSizeRel." FORCE)
endif()
string(TOUPPER ${CMAKE_BUILD_TYPE} CMAKE_BUILD_TYPE_UPPERCASE)

if(NOT_SUBPROJECT AND (CMAKE_CROSSCOMPILING OR IOS))
  set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
endif()

if(NOT "$ENV{TEAMCITY_PROCESS_FLOW_ID}" STREQUAL "")
  set(CI TEAMCITY)
  message(STATUS "TeamCity CI")
elseif(NOT "$ENV{TRAVIS}" STREQUAL "")
  set(CI TRAVIS)
  message(STATUS "Travis CI")
elseif(NOT "$ENV{CIRCLECI}" STREQUAL "")
  set(CI CIRCLE)
  message(STATUS "Circle CI")
elseif(NOT "$ENV{APPVEYOR}" STREQUAL "")
  set(CI APPVEYOR)
  message(STATUS "AppVeyor CI")
elseif(NOT "$ENV{CI}" STREQUAL "")
  set(CI "$ENV{CI}")
  message(STATUS "Other CI (${CI})")
else()
  message(STATUS "Assume No any CI environment")
  unset(CI)
endif()

# output all mdbx-related targets in single directory
if(NOT DEFINED MDBX_OUTPUT_DIR)
  set(MDBX_OUTPUT_DIR ${CMAKE_CURRENT_BINARY_DIR})
endif()
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY ${MDBX_OUTPUT_DIR})
set(CMAKE_LIBRARY_OUTPUT_DIRECTORY ${MDBX_OUTPUT_DIR})
set(CMAKE_PDB_OUTPUT_DIRECTORY ${MDBX_OUTPUT_DIR})
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${MDBX_OUTPUT_DIR})

include(CheckFunctionExists)
include(FindPackageMessage)
include(GNUInstallDirs)

if(CMAKE_C_COMPILER_ID STREQUAL "MSVC" AND MSVC_VERSION LESS 1900)
  message(
    SEND_ERROR "MSVC compiler ${MSVC_VERSION} is too old for building MDBX."
               " At least \"Microsoft C/C++ Compiler\" version 19.0.24234.1 (Visual Studio 2015 Update 3) is required.")
endif()

if(NOT DEFINED THREADS_PREFER_PTHREAD_FLAG)
  set(THREADS_PREFER_PTHREAD_FLAG TRUE)
endif()
find_package(Threads REQUIRED)

include(cmake/compiler.cmake)
include(cmake/profile.cmake)

# Workaround for `-pthread` toolchain/cmake bug
if(NOT APPLE
   AND NOT MSVC
   AND CMAKE_USE_PTHREADS_INIT
   AND NOT CMAKE_THREAD_LIBS_INIT
   AND (CMAKE_COMPILER_IS_GNUCC OR CMAKE_COMPILER_IS_CLANG))
  check_compiler_flag("-pthread" CC_HAS_PTHREAD)
  if(CC_HAS_PTHREAD AND NOT CMAKE_EXE_LINKER_FLAGS MATCHES "-pthread")
    message(STATUS "Force add -pthread for linker flags to avoid troubles")
    set(CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} -pthread")
    set(CMAKE_SHARED_LINKER_FLAGS "${CMAKE_SHARED_LINKER_FLAGS} -pthread")
    set(CMAKE_MODULE_LINKER_FLAGS "${CMAKE_MODULE_LINKER_FLAGS} -pthread")
  endif()
endif()

check_function_exists(pow NOT_NEED_LIBM)
if(NOT_NEED_LIBM)
  set(LIB_MATH "")
else()
  set(CMAKE_REQUIRED_LIBRARIES m)
  check_function_exists(pow HAVE_LIBM)
  if(HAVE_LIBM)
    set(LIB_MATH m)
  else()
    message(FATAL_ERROR "No libm found for math support")
  endif()
endif()

if(SUBPROJECT)
  if(NOT DEFINED BUILD_SHARED_LIBS)
    option(BUILD_SHARED_LIBS "Build shared libraries (DLLs)" OFF)
  endif()
  if(NOT DEFINED CMAKE_POSITION_INDEPENDENT_CODE)
    option(CMAKE_POSITION_INDEPENDENT_CODE "Generate position independent (PIC)" ON)
  endif()
  set(MDBX_MANAGE_BUILD_FLAGS_DEFAULT OFF)
else()
  option(BUILD_SHARED_LIBS "Build shared libraries (DLLs)" ON)
  option(CMAKE_POSITION_INDEPENDENT_CODE "Generate position independent (PIC)" ON)
  if(CC_HAS_ARCH_NATIVE)
    option(BUILD_FOR_NATIVE_CPU "Generate code for the compiling machine CPU" OFF)
  endif()

  if(CMAKE_INTERPROCEDURAL_OPTIMIZATION_AVAILABLE
     OR GCC_LTO_AVAILABLE
     OR MSVC_LTO_AVAILABLE
     OR CLANG_LTO_AVAILABLE)
    if((CMAKE_CONFIGURATION_TYPES OR NOT CMAKE_BUILD_TYPE_UPPERCASE STREQUAL "DEBUG")
       AND ((MSVC_LTO_AVAILABLE AND NOT CMAKE_C_COMPILER_VERSION VERSION_LESS 19)
            OR (GCC_LTO_AVAILABLE AND NOT CMAKE_C_COMPILER_VERSION VERSION_LESS 7)
            OR (CLANG_LTO_AVAILABLE AND NOT CMAKE_C_COMPILER_VERSION VERSION_LESS 5)))
      set(INTERPROCEDURAL_OPTIMIZATION_DEFAULT ON)
    else()
      set(INTERPROCEDURAL_OPTIMIZATION_DEFAULT OFF)
    endif()
    option(INTERPROCEDURAL_OPTIMIZATION "Enable interprocedural/LTO optimization."
           ${INTERPROCEDURAL_OPTIMIZATION_DEFAULT})
  endif()

  if(INTERPROCEDURAL_OPTIMIZATION)
    if(GCC_LTO_AVAILABLE)
      set(LTO_ENABLED TRUE)
      set(CMAKE_AR
          ${CMAKE_GCC_AR}
          CACHE PATH "Path to ar program with LTO-plugin" FORCE)
      set(CMAKE_C_COMPILER_AR
          ${CMAKE_AR}
          CACHE PATH "Path to ar program with LTO-plugin" FORCE)
      set(CMAKE_CXX_COMPILER_AR
          ${CMAKE_AR}
          CACHE PATH "Path to ar program with LTO-plugin" FORCE)
      set(CMAKE_NM
          ${CMAKE_GCC_NM}
          CACHE PATH "Path to nm program with LTO-plugin" FORCE)
      set(CMAKE_RANLIB
          ${CMAKE_GCC_RANLIB}
          CACHE PATH "Path to ranlib program with LTO-plugin" FORCE)
      set(CMAKE_C_COMPILER_RANLIB
          ${CMAKE_RANLIB}
          CACHE PATH "Path to ranlib program with LTO-plugin" FORCE)
      set(CMAKE_CXX_COMPILER_RANLIB
          ${CMAKE_RANLIB}
          CACHE PATH "Path to ranlib program with LTO-plugin" FORCE)
      message(STATUS "MDBX indulge Link-Time Optimization by GCC")
    elseif(CLANG_LTO_AVAILABLE)
      set(LTO_ENABLED TRUE)
      if(CMAKE_CLANG_LD)
        set(CMAKE_LINKER
            ${CMAKE_CLANG_LD}
            CACHE PATH "Path to lld or ld program with LTO-plugin" FORCE)
      endif()
      set(CMAKE_AR
          ${CMAKE_CLANG_AR}
          CACHE PATH "Path to ar program with LTO-plugin" FORCE)
      set(CMAKE_C_COMPILER_AR
          ${CMAKE_AR}
          CACHE PATH "Path to ar program with LTO-plugin" FORCE)
      set(CMAKE_CXX_COMPILER_AR
          ${CMAKE_AR}
          CACHE PATH "Path to ar program with LTO-plugin" FORCE)
      set(CMAKE_NM
          ${CMAKE_CLANG_NM}
          CACHE PATH "Path to nm program with LTO-plugin" FORCE)
      set(CMAKE_RANLIB
          ${CMAKE_CLANG_RANLIB}
          CACHE PATH "Path to ranlib program with LTO-plugin" FORCE)
      set(CMAKE_C_COMPILER_RANLIB
          ${CMAKE_RANLIB}
          CACHE PATH "Path to ranlib program with LTO-plugin" FORCE)
      set(CMAKE_CXX_COMPILER_RANLIB
          ${CMAKE_RANLIB}
          CACHE PATH "Path to ranlib program with LTO-plugin" FORCE)
      message(STATUS "MDBX indulge Link-Time Optimization by CLANG")
    elseif(MSVC_LTO_AVAILABLE)
      set(LTO_ENABLED TRUE)
      message(STATUS "MDBX indulge Link-Time Optimization by MSVC")
    elseif(CMAKE_INTERPROCEDURAL_OPTIMIZATION_AVAILABLE)
      message(STATUS "MDBX indulge Interprocedural Optimization by CMake")
      set(CMAKE_INTERPROCEDURAL_OPTIMIZATION TRUE)
      set(LTO_ENABLED TRUE)
    else()
      message(WARNING "Unable to engage interprocedural/LTO optimization.")
    endif()
  else()
    set(CMAKE_INTERPROCEDURAL_OPTIMIZATION FALSE)
    set(LTO_ENABLED FALSE)
  endif()

  if(NOT MDBX_AMALGAMATED_SOURCE)
    find_program(VALGRIND valgrind)
    if(VALGRIND)
      # (LY) cmake is ugly and nasty. Therefore memcheck-options should be defined before including ctest. Otherwise
      # ctest may ignore it.
      set(MEMORYCHECK_SUPPRESSIONS_FILE
          "${CMAKE_CURRENT_SOURCE_DIR}/test/valgrind_suppress.txt"
          CACHE FILEPATH "Suppressions file for Valgrind" FORCE)
      set(MEMORYCHECK_COMMAND_OPTIONS
          "--trace-children=yes --leak-check=full --track-origins=yes --track-origins=yes --error-exitcode=42 --error-markers=@ --errors-for-leak-kinds=definite --fair-sched=yes --suppressions=${MEMORYCHECK_SUPPRESSIONS_FILE}"
          CACHE STRING "Valgrind options" FORCE)
      set(VALGRIND_COMMAND_OPTIONS
          "${MEMORYCHECK_COMMAND_OPTIONS}"
          CACHE STRING "Valgrind options" FORCE)
    endif()

    # Enable 'make tags' target.
    find_program(CTAGS ctags)
    if(CTAGS)
      add_custom_target(
        tags
        COMMAND ${CTAGS} -R -f tags
        WORKING_DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR})
      add_custom_target(ctags DEPENDS tags)
    endif(CTAGS)

    if(UNIX)
      find_program(CLANG_FORMAT NAMES clang-format-13 clang-format)
      if(CLANG_FORMAT)
        execute_process(COMMAND ${CLANG_FORMAT} "--version" OUTPUT_VARIABLE clang_format_version_info)
        string(REGEX MATCH "version ([0-9]+)\\.([0-9]+)\\.([0-9]+)(.*)?" clang_format_version_info CLANG_FORMAT_VERSION)
        if(clang_format_version_info AND NOT CLANG_FORMAT_VERSION VERSION_LESS 13.0)
          # Enable 'make reformat' target.
          add_custom_target(
            reformat
            VERBATIM
            COMMAND git ls-files | grep -E \\.\(c|cxx|cc|cpp|h|hxx|hpp\)\(\\.in\)?\$ | xargs ${CLANG_FORMAT} -i
                    --style=file
            WORKING_DIRECTORY ${CMAKE_SOURCE_DIR})
        endif()
      endif()
    endif()

    if(NOT "${PROJECT_BINARY_DIR}" STREQUAL "${PROJECT_SOURCE_DIR}")
      add_custom_target(distclean)
      add_custom_command(
        TARGET distclean
        POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E remove_directory "${PROJECT_BINARY_DIR}"
        COMMENT "Removing the build directory and its content")
    elseif(IS_DIRECTORY .git AND GIT)
      add_custom_target(distclean)
      add_custom_command(
        TARGET distclean
        POST_BUILD
        WORKING_DIRECTORY ${PROJECT_SOURCE_DIR}
        COMMAND ${GIT} submodule foreach --recursive git clean -f -X -d
        COMMAND ${GIT} clean -f -X -d
        COMMENT "Removing all build files from the source directory")
    endif()
  endif(NOT MDBX_AMALGAMATED_SOURCE)

  set(MDBX_MANAGE_BUILD_FLAGS_DEFAULT ON)
endif(SUBPROJECT)

option(MDBX_MANAGE_BUILD_FLAGS "Allow libmdbx to configure/manage/override its own build flags"
       ${MDBX_MANAGE_BUILD_FLAGS_DEFAULT})
if(MDBX_MANAGE_BUILD_FLAGS)
  setup_compile_flags()
endif()

list(FIND CMAKE_CXX_COMPILE_FEATURES cxx_std_11 HAS_CXX11)
list(FIND CMAKE_CXX_COMPILE_FEATURES cxx_std_14 HAS_CXX14)
list(FIND CMAKE_CXX_COMPILE_FEATURES cxx_std_17 HAS_CXX17)
list(FIND CMAKE_CXX_COMPILE_FEATURES cxx_std_20 HAS_CXX20)
list(FIND CMAKE_CXX_COMPILE_FEATURES cxx_std_23 HAS_CXX23)
if(NOT DEFINED MDBX_CXX_STANDARD)
  if(DEFINED ENV{CMAKE_CXX_STANDARD})
    set(CMAKE_CXX_STANDARD $ENV{CMAKE_CXX_STANDARD})
  endif()
  if(DEFINED CMAKE_CXX_STANDARD)
    set(MDBX_CXX_STANDARD ${CMAKE_CXX_STANDARD})
  elseif(NOT HAS_CXX23 LESS 0 AND NOT (CMAKE_COMPILER_IS_CLANG AND CMAKE_CXX_COMPILER_VERSION VERSION_LESS 12))
    set(MDBX_CXX_STANDARD 23)
  elseif(NOT HAS_CXX20 LESS 0 AND NOT (CMAKE_COMPILER_IS_CLANG AND CMAKE_CXX_COMPILER_VERSION VERSION_LESS 10))
    set(MDBX_CXX_STANDARD 20)
  elseif(NOT HAS_CXX17 LESS 0 AND NOT (CMAKE_COMPILER_IS_CLANG AND CMAKE_CXX_COMPILER_VERSION VERSION_LESS 5))
    set(MDBX_CXX_STANDARD 17)
  elseif(NOT HAS_CXX14 LESS 0)
    set(MDBX_CXX_STANDARD 14)
  elseif(NOT HAS_CXX11 LESS 0)
    set(MDBX_CXX_STANDARD 11)
  elseif(CXX_FALLBACK_GNU11 OR CXX_FALLBACK_11)
    set(MDBX_CXX_STANDARD 11)
  else()
    set(MDBX_CXX_STANDARD 98)
  endif()
endif()

list(FIND CMAKE_C_COMPILE_FEATURES c_std_11 HAS_C11)
list(FIND CMAKE_C_COMPILE_FEATURES c_std_23 HAS_C23)
if(NOT DEFINED MDBX_C_STANDARD)
  if(DEFINED ENV{CMAKE_C_STANDARD})
    set(CMAKE_C_STANDARD $ENV{CMAKE_C_STANDARD})
  endif()
  if(DEFINED CMAKE_C_STANDARD)
    set(MDBX_C_STANDARD ${CMAKE_C_STANDARD})
  elseif(
    MSVC
    # MSVC >= 19.28 (Microsoft Visual Studio 16.8) is mad! It unable process Windows SDK headers in the C11 mode!
    AND MSVC_VERSION GREATER 1927
    AND NOT MSVC_VERSION GREATER 1929)
    set(MDBX_C_STANDARD 99)
    set(C_FALLBACK_11 OFF)
    set(C_FALLBACK_GNU11 OFF)
  elseif(NOT HAS_C23 LESS 0)
    set(MDBX_C_STANDARD 23)
  elseif(
    HAS_C11 LESS 0
    AND NOT C_FALLBACK_GNU11
    AND NOT C_FALLBACK_11)
    set(MDBX_C_STANDARD 99)
  else()
    set(MDBX_C_STANDARD 11)
  endif()
endif()

if(WIN32 AND EXISTS "${MDBX_SOURCE_DIR}/ntdll.def")
  if(MSVC)
    if(NOT MSVC_LIB_EXE)
      # Find lib.exe
      get_filename_component(CL_NAME ${CMAKE_C_COMPILER} NAME)
      string(REPLACE cl.exe lib.exe MSVC_LIB_EXE ${CL_NAME})
      find_program(MSVC_LIB_EXE ${MSVC_LIB_EXE})
    endif()
    if(MSVC_LIB_EXE)
      message(STATUS "Found MSVC's lib tool: ${MSVC_LIB_EXE}")
      set(MDBX_NTDLL_EXTRA_IMPLIB "${CMAKE_CURRENT_BINARY_DIR}/mdbx_ntdll_extra.lib")
      add_custom_command(
        OUTPUT "${MDBX_NTDLL_EXTRA_IMPLIB}"
        COMMENT "Create extra-import-library for ntdll.dll"
        MAIN_DEPENDENCY "${MDBX_SOURCE_DIR}/ntdll.def"
        COMMAND ${MSVC_LIB_EXE} /def:"${MDBX_SOURCE_DIR}/ntdll.def" /out:"${MDBX_NTDLL_EXTRA_IMPLIB}"
                ${INITIAL_CMAKE_STATIC_LINKER_FLAGS})
    else()
      message(WARNING "MSVC's lib tool not found")
    endif()
  elseif(MINGW OR MINGW64)
    if(NOT DLLTOOL)
      # Find dlltool
      get_filename_component(GCC_NAME ${CMAKE_C_COMPILER} NAME)
      string(REPLACE gcc dlltool DLLTOOL_NAME ${GCC_NAME})
      find_program(DLLTOOL NAMES ${DLLTOOL_NAME})
    endif()
    if(DLLTOOL)
      message(STATUS "Found dlltool: ${DLLTOOL}")
      set(MDBX_NTDLL_EXTRA_IMPLIB "${CMAKE_CURRENT_BINARY_DIR}/mdbx_ntdll_extra.a")
      add_custom_command(
        OUTPUT "${MDBX_NTDLL_EXTRA_IMPLIB}"
        COMMENT "Create extra-import-library for ntdll.dll"
        MAIN_DEPENDENCY "${MDBX_SOURCE_DIR}/ntdll.def"
        COMMAND ${DLLTOOL} -d "${MDBX_SOURCE_DIR}/ntdll.def" -l "${MDBX_NTDLL_EXTRA_IMPLIB}")
    else()
      message(WARNING "dlltool not found")
    endif()
  endif()

  if(MDBX_NTDLL_EXTRA_IMPLIB)
    # Sometimes CMake requires a nightmarish magic for simple things.
    #
    # (1) create a target out of the library compilation result
    add_custom_target(ntdll_extra_target DEPENDS "${MDBX_NTDLL_EXTRA_IMPLIB}")
    # (2) create an library target out of the library compilation result
    add_library(ntdll_extra STATIC IMPORTED GLOBAL)
    add_dependencies(ntdll_extra ntdll_extra_target)
    # (3) specify where the library is (and where to find the headers)
    set_target_properties(ntdll_extra PROPERTIES IMPORTED_LOCATION "${MDBX_NTDLL_EXTRA_IMPLIB}")
  endif()
endif()

# ######################################################################################################################
# ~~~
#
#           ####   #####    #####     #     ####   #    #   ####
#          #    #  #    #     #       #    #    #  ##   #  #
#          #    #  #    #     #       #    #    #  # #  #   ####
#          #    #  #####      #       #    #    #  #  # #       #
#          #    #  #          #       #    #    #  #   ##  #    #
#           ####   #          #       #     ####   #    #   ####
#
# ~~~
# ######################################################################################################################

set(MDBX_BUILD_OPTIONS ENABLE_UBSAN ENABLE_ASAN ENABLE_MEMCHECK ENABLE_GPROF ENABLE_GCOV)

if(IOS)
  set(MDBX_BUILD_TOOLS_DEFAULT OFF)
  if(NOT_SUBPROJECT)
    cmake_policy(SET CMP0006 OLD)
    set(CMAKE_XCODE_ATTRIBUTE_CODE_SIGNING_ALLOWED NO)
  endif()
else()
  set(MDBX_BUILD_TOOLS_DEFAULT ON)
endif()

add_option(MDBX INSTALL_STATIC "Build and install libmdbx for static linking" OFF)
add_option(MDBX BUILD_SHARED_LIBRARY "Build libmdbx as shared library (DLL)" ${BUILD_SHARED_LIBS})
add_option(MDBX BUILD_TOOLS "Build MDBX tools (mdbx_chk/stat/dump/load/copy/drop)" ${MDBX_BUILD_TOOLS_DEFAULT})
cmake_dependent_option(MDBX_INSTALL_MANPAGES "Install man-pages for MDBX tools (mdbx_chk/stat/dump/load/copy)" ON
                       MDBX_BUILD_TOOLS OFF)
add_option(MDBX TXN_CHECKOWNER "Checking transaction matches the calling thread inside libmdbx's API" ON)
add_option(MDBX ENV_CHECKPID "Checking PID inside libmdbx's API against reuse DB environment after the fork()" AUTO)
mark_as_advanced(MDBX_ENV_CHECKPID)
if(${CMAKE_SYSTEM_NAME} STREQUAL "Linux")
  add_option(MDBX DISABLE_GNU_SOURCE "Don't use GNU/Linux libc extensions" OFF)
  mark_as_advanced(MDBX_DISABLE_GNU_SOURCE)
endif()
if(${CMAKE_SYSTEM_NAME} STREQUAL "Darwin" OR IOS)
  add_option(MDBX APPLE_SPEED_INSTEADOF_DURABILITY "Disable use fcntl(F_FULLFSYNC) in favor of speed" OFF)
  mark_as_advanced(MDBX_APPLE_SPEED_INSTEADOF_DURABILITY)
endif()
if(WIN32)
  if(MDBX_NTDLL_EXTRA_IMPLIB)
    add_option(MDBX WITHOUT_MSVC_CRT "Avoid dependence from MSVC CRT and use ntdll.dll instead" OFF)
  endif()
  set(MDBX_AVOID_MSYNC_DEFAULT ON)
else()
  add_option(MDBX USE_OFDLOCKS "Use Open file description locks (aka OFD locks, non-POSIX)" AUTO)
  mark_as_advanced(MDBX_USE_OFDLOCKS)
  add_option(MDBX USE_MINCORE "Use Unix' mincore() to determine whether DB-pages are resident in memory" ON)
  mark_as_advanced(MDBX_USE_MINCORE)
  set(MDBX_AVOID_MSYNC_DEFAULT OFF)
endif()
add_option(
  MDBX AVOID_MSYNC
  "Disable in-memory database updating with consequent flush-to-disk/msync syscall in `MDBX_WRITEMAP` mode"
  ${MDBX_AVOID_MSYNC_DEFAULT})
add_option(MDBX MMAP_NEEDS_JOLT "Assume system needs explicit syscall to sync/flush/write modified mapped memory" AUTO)
mark_as_advanced(MDBX_MMAP_NEEDS_JOLT)
add_option(MDBX LOCKING "Locking method (Windows=-1, SystemV=5, POSIX=1988, POSIX=2001, POSIX=2008)" AUTO)
mark_as_advanced(MDBX_LOCKING)
add_option(MDBX TRUST_RTC "Does a system have battery-backed Real-Time Clock or just a fake" AUTO)
mark_as_advanced(MDBX_TRUST_RTC)
add_option(MDBX FORCE_ASSERTIONS "Force enable assertion checking" OFF)
add_option(
  MDBX
  DISABLE_VALIDATION
  "Disable some checks to reduce an overhead and detection probability of database corruption to a values closer to the LMDB"
  OFF)
mark_as_advanced(MDBX_DISABLE_VALIDATION)
add_option(MDBX ENABLE_REFUND "Zerocost auto-compactification during write-transactions" ON)
add_option(MDBX ENABLE_BIGFOOT
           "Chunking long list of retired pages during huge transactions commit to avoid use sequences of pages" ON)
add_option(MDBX ENABLE_PGOP_STAT "Gathering statistics for page operations" ON)
add_option(MDBX ENABLE_PROFGC "Profiling of GC search and updates" OFF)
mark_as_advanced(MDBX_ENABLE_PROFGC)
add_option(MDBX ENABLE_DBI_SPARSE
           "Support for sparse sets of DBI handles to reduce overhead when starting and processing transactions" ON)
add_option(MDBX ENABLE_DBI_LOCKFREE "Support for deferred releasing and a lockfree path to quickly open DBI handles" ON)

if(NOT MDBX_AMALGAMATED_SOURCE)
  if(CMAKE_CONFIGURATION_TYPES OR CMAKE_BUILD_TYPE_UPPERCASE STREQUAL "DEBUG")
    set(MDBX_ALLOY_BUILD_DEFAULT OFF)
  else()
    set(MDBX_ALLOY_BUILD_DEFAULT ON)
  endif()
  add_option(MDBX ALLOY_BUILD "Build MDBX library through single/alloyed object file" ${MDBX_ALLOY_BUILD_DEFAULT})
endif()

if((MDBX_BUILD_TOOLS OR MDBX_ENABLE_TESTS) AND MDBX_BUILD_SHARED_LIBRARY)
  add_option(MDBX LINK_TOOLS_NONSTATIC "Link MDBX tools with non-static libmdbx" OFF)
else()
  unset(MDBX_LINK_TOOLS_NONSTATIC CACHE)
endif()

if(CMAKE_CXX_COMPILER_LOADED
   AND MDBX_CXX_STANDARD LESS 83
   AND NOT MDBX_CXX_STANDARD LESS 11)
  if(NOT MDBX_AMALGAMATED_SOURCE)
    option(MDBX_ENABLE_TESTS "Build MDBX tests" ${BUILD_TESTING})
  endif()
  if(NOT MDBX_WITHOUT_MSVC_CRT
     AND NOT (CMAKE_COMPILER_IS_GNUCXX AND CMAKE_CXX_COMPILER_VERSION VERSION_LESS 4.8)
     AND NOT (CMAKE_COMPILER_IS_CLANG AND CMAKE_CXX_COMPILER_VERSION VERSION_LESS 3.9)
     AND NOT (MSVC AND MSVC_VERSION LESS 1900))
    option(MDBX_BUILD_CXX "Build C++ portion" ON)
  else()
    set(MDBX_BUILD_CXX FALSE)
  endif()
else()
  set(MDBX_BUILD_CXX FALSE)
  set(MDBX_ENABLE_TESTS FALSE)
endif()

if(CI)
  add_definitions(-DMDBX_CI="${CI}")
endif()

# ######################################################################################################################

if(MDBX_BUILD_CXX AND NOT CMAKE_CXX_COMPILER_LOADED)
  message(FATAL_ERROR "MDBX_BUILD_CXX=${MDBX_BUILD_CXX}: The C++ compiler is required to build the C++API.")
endif()

if(MDBX_BUILD_CXX)
  # determine library for C++ std::filesystem
  probe_libcxx_filesystem()
endif()

# sources list
set(LIBMDBX_PUBLIC_HEADERS mdbx.h)
set(LIBMDBX_SOURCES mdbx.h "${CMAKE_CURRENT_BINARY_DIR}/config.h")
if(MDBX_AMALGAMATED_SOURCE)
  list(APPEND LIBMDBX_SOURCES mdbx.c)
else()
  # generate version file
  configure_file("${MDBX_SOURCE_DIR}/version.c.in" "${CMAKE_CURRENT_BINARY_DIR}/version.c" ESCAPE_QUOTES)
  file(SHA256 "${CMAKE_CURRENT_BINARY_DIR}/version.c" MDBX_SOURCERY_DIGEST)
  string(MAKE_C_IDENTIFIER "${MDBX_GIT_DESCRIBE}" MDBX_SOURCERY_SUFFIX)
  set(MDBX_BUILD_SOURCERY "${MDBX_SOURCERY_DIGEST}_${MDBX_SOURCERY_SUFFIX}")

  if(MDBX_ALLOY_BUILD)
    list(APPEND LIBMDBX_SOURCES "${MDBX_SOURCE_DIR}/alloy.c")
    include_directories("${MDBX_SOURCE_DIR}" "${CMAKE_CURRENT_BINARY_DIR}")
  else()
    list(
      APPEND
      LIBMDBX_SOURCES
      "${MDBX_SOURCE_DIR}/api-cold.c"
      "${MDBX_SOURCE_DIR}/api-copy.c"
      "${MDBX_SOURCE_DIR}/api-cursor.c"
      "${MDBX_SOURCE_DIR}/api-dbi.c"
      "${MDBX_SOURCE_DIR}/api-env.c"
      "${MDBX_SOURCE_DIR}/api-extra.c"
      "${MDBX_SOURCE_DIR}/api-key-transform.c"
      "${MDBX_SOURCE_DIR}/api-misc.c"
      "${MDBX_SOURCE_DIR}/api-opts.c"
      "${MDBX_SOURCE_DIR}/api-range-estimate.c"
      "${MDBX_SOURCE_DIR}/api-txn-data.c"
      "${MDBX_SOURCE_DIR}/api-txn.c"
      "${MDBX_SOURCE_DIR}/atomics-ops.h"
      "${MDBX_SOURCE_DIR}/atomics-types.h"
      "${MDBX_SOURCE_DIR}/audit.c"
      "${MDBX_SOURCE_DIR}/chk.c"
      "${MDBX_SOURCE_DIR}/cogs.c"
      "${MDBX_SOURCE_DIR}/cogs.h"
      "${MDBX_SOURCE_DIR}/coherency.c"
      "${MDBX_SOURCE_DIR}/cursor.c"
      "${MDBX_SOURCE_DIR}/cursor.h"
      "${MDBX_SOURCE_DIR}/dbi.c"
      "${MDBX_SOURCE_DIR}/dbi.h"
      "${MDBX_SOURCE_DIR}/dpl.c"
      "${MDBX_SOURCE_DIR}/dpl.h"
      "${MDBX_SOURCE_DIR}/dxb.c"
      "${MDBX_SOURCE_DIR}/env.c"
      "${MDBX_SOURCE_DIR}/essentials.h"
      "${MDBX_SOURCE_DIR}/gc-get.c"
      "${MDBX_SOURCE_DIR}/gc-put.c"
      "${MDBX_SOURCE_DIR}/gc.h"
      "${MDBX_SOURCE_DIR}/global.c"
      "${MDBX_SOURCE_DIR}/internals.h"
      "${MDBX_SOURCE_DIR}/layout-dxb.h"
      "${MDBX_SOURCE_DIR}/layout-lck.h"
      "${MDBX_SOURCE_DIR}/lck.c"
      "${MDBX_SOURCE_DIR}/lck.h"
      "${MDBX_SOURCE_DIR}/logging_and_debug.c"
      "${MDBX_SOURCE_DIR}/logging_and_debug.h"
      "${MDBX_SOURCE_DIR}/meta.c"
      "${MDBX_SOURCE_DIR}/meta.h"
      "${MDBX_SOURCE_DIR}/mvcc-readers.c"
      "${MDBX_SOURCE_DIR}/node.c"
      "${MDBX_SOURCE_DIR}/node.h"
      "${MDBX_SOURCE_DIR}/options.h"
      "${MDBX_SOURCE_DIR}/osal.c"
      "${MDBX_SOURCE_DIR}/osal.h"
      "${MDBX_SOURCE_DIR}/page-get.c"
      "${MDBX_SOURCE_DIR}/page-iov.c"
      "${MDBX_SOURCE_DIR}/page-iov.h"
      "${MDBX_SOURCE_DIR}/page-ops.c"
      "${MDBX_SOURCE_DIR}/page-ops.h"
      "${MDBX_SOURCE_DIR}/tree-search.c"
      "${MDBX_SOURCE_DIR}/pnl.c"
      "${MDBX_SOURCE_DIR}/pnl.h"
      "${MDBX_SOURCE_DIR}/preface.h"
      "${MDBX_SOURCE_DIR}/proto.h"
      "${MDBX_SOURCE_DIR}/refund.c"
      "${MDBX_SOURCE_DIR}/sort.h"
      "${MDBX_SOURCE_DIR}/spill.c"
      "${MDBX_SOURCE_DIR}/spill.h"
      "${MDBX_SOURCE_DIR}/table.c"
      "${MDBX_SOURCE_DIR}/tls.c"
      "${MDBX_SOURCE_DIR}/tls.h"
      "${MDBX_SOURCE_DIR}/tree-ops.c"
      "${MDBX_SOURCE_DIR}/txl.c"
      "${MDBX_SOURCE_DIR}/txl.h"
      "${MDBX_SOURCE_DIR}/txn.c"
      "${MDBX_SOURCE_DIR}/unaligned.h"
      "${MDBX_SOURCE_DIR}/utils.c"
      "${MDBX_SOURCE_DIR}/utils.h"
      "${MDBX_SOURCE_DIR}/walk.c"
      "${MDBX_SOURCE_DIR}/walk.h"
      "${CMAKE_CURRENT_BINARY_DIR}/version.c")
    if(NOT MSVC)
      list(APPEND LIBMDBX_SOURCES "${MDBX_SOURCE_DIR}/lck-posix.c")
    endif()
    if(NOT APPLE)
      list(APPEND LIBMDBX_SOURCES "${MDBX_SOURCE_DIR}/windows-import.h" "${MDBX_SOURCE_DIR}/windows-import.c"
           "${MDBX_SOURCE_DIR}/lck-windows.c")
    endif()
    include_directories("${MDBX_SOURCE_DIR}")
  endif()
endif(MDBX_AMALGAMATED_SOURCE)
if(MDBX_BUILD_CXX)
  message(STATUS "Use C${MDBX_C_STANDARD} and C++${MDBX_CXX_STANDARD} for libmdbx")
  list(APPEND LIBMDBX_PUBLIC_HEADERS mdbx.h++)
  list(APPEND LIBMDBX_SOURCES "${MDBX_SOURCE_DIR}/mdbx.c++" mdbx.h++)
else()
  message(STATUS "Use C${MDBX_C_STANDARD} for libmdbx but C++ portion is disabled")
endif()

if(MSVC)
  add_compile_options("/utf-8")
endif()

macro(target_setup_options TARGET)
  if(DEFINED INTERPROCEDURAL_OPTIMIZATION)
    set_target_properties(${TARGET} PROPERTIES INTERPROCEDURAL_OPTIMIZATION $<BOOL:${INTERPROCEDURAL_OPTIMIZATION}>)
  endif()
  if(NOT C_FALLBACK_GNU11 AND NOT C_FALLBACK_11)
    set_target_properties(${TARGET} PROPERTIES C_STANDARD ${MDBX_C_STANDARD} C_STANDARD_REQUIRED ON)
  endif()
  if(MDBX_BUILD_CXX)
    if(NOT CXX_FALLBACK_GNU11 AND NOT CXX_FALLBACK_11)
      set_target_properties(${TARGET} PROPERTIES CXX_STANDARD ${MDBX_CXX_STANDARD} CXX_STANDARD_REQUIRED ON)
    endif()
    if(MSVC AND NOT MSVC_VERSION LESS 1910)
      target_compile_options(${TARGET} INTERFACE "/Zc:__cplusplus")
    endif()
  endif()
  if(CC_HAS_FASTMATH AND NOT (CMAKE_COMPILER_IS_CLANG AND CMAKE_CXX_COMPILER_VERSION VERSION_LESS 10))
    target_compile_options(${TARGET} PRIVATE "-ffast-math")
  endif()
  if(CC_HAS_VISIBILITY)
    target_compile_options(${TARGET} PRIVATE "-fvisibility=hidden")
  endif()
  if(BUILD_FOR_NATIVE_CPU AND CC_HAS_ARCH_NATIVE)
    target_compile_options(${TARGET} PRIVATE "-march=native")
  endif()
endmacro()

macro(libmdbx_setup_libs TARGET MODE)
  if(CMAKE_VERSION VERSION_LESS 3.1)
    target_link_libraries(${TARGET} ${MODE} ${CMAKE_THREAD_LIBS_INIT})
  else()
    target_link_libraries(${TARGET} ${MODE} Threads::Threads)
  endif()
  if(WIN32)
    target_link_libraries(
      ${TARGET}
      ${MODE}
      ntdll
      user32
      kernel32
      advapi32
      ole32)
    if(MDBX_NTDLL_EXTRA_IMPLIB AND MDBX_WITHOUT_MSVC_CRT)
      target_link_libraries(${TARGET} ${MODE} ntdll_extra)
    endif()
  elseif(${CMAKE_SYSTEM_NAME} STREQUAL "SunOS" OR ${CMAKE_SYSTEM_NAME} STREQUAL "Solaris")
    target_link_libraries(${TARGET} ${MODE} kstat)
  elseif(${CMAKE_SYSTEM_NAME} STREQUAL "Android")
    target_link_libraries(${TARGET} ${MODE} log)
  endif()
  if(LIBCXX_FILESYSTEM AND MDBX_BUILD_CXX)
    if(CMAKE_COMPILER_IS_ELBRUSCXX
       AND CMAKE_CXX_COMPILER_VERSION VERSION_LESS 1.25.23
       AND NOT CMAKE_VERSION VERSION_LESS 3.13)
      target_link_options(${TARGET} PUBLIC "-Wl,--allow-multiple-definition")
    endif()
    target_link_libraries(${TARGET} PUBLIC ${LIBCXX_FILESYSTEM})
  endif()
endmacro()

# build static library
if(MDBX_INSTALL_STATIC)
  add_library(mdbx-static STATIC ${LIBMDBX_SOURCES})
else()
  add_library(mdbx-static STATIC EXCLUDE_FROM_ALL ${LIBMDBX_SOURCES})
endif()
set_target_properties(mdbx-static PROPERTIES PUBLIC_HEADER "${LIBMDBX_PUBLIC_HEADERS}")
target_compile_definitions(mdbx-static PRIVATE MDBX_BUILD_SHARED_LIBRARY=0)
target_setup_options(mdbx-static)
libmdbx_setup_libs(mdbx-static INTERFACE)
if(MDBX_BUILD_SHARED_LIBRARY)
  set_target_properties(mdbx-static PROPERTIES OUTPUT_NAME mdbx-static)
else()
  set_target_properties(mdbx-static PROPERTIES OUTPUT_NAME mdbx)
endif()
target_include_directories(mdbx-static INTERFACE "${CMAKE_CURRENT_SOURCE_DIR}")

# ######################################################################################################################

# build shared library
if(MDBX_BUILD_SHARED_LIBRARY)
  add_library(mdbx SHARED ${LIBMDBX_SOURCES})
  set_target_properties(mdbx PROPERTIES PUBLIC_HEADER "${LIBMDBX_PUBLIC_HEADERS}")
  target_compile_definitions(
    mdbx
    PRIVATE LIBMDBX_EXPORTS MDBX_BUILD_SHARED_LIBRARY=1
    INTERFACE LIBMDBX_IMPORTS)
  target_setup_options(mdbx)
  libmdbx_setup_libs(mdbx PRIVATE)
  if(MSVC)
    if(MDBX_NTDLL_EXTRA_IMPLIB AND MDBX_WITHOUT_MSVC_CRT)
      set_property(TARGET mdbx PROPERTY LINKER_FLAGS "/NODEFAULTLIB")
    else()
      set_property(TARGET mdbx PROPERTY MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>DLL")
    endif()
  endif()
  if(CC_HAS_VISIBILITY AND (LTO_ENABLED OR INTERPROCEDURAL_OPTIMIZATION))
    set_target_properties(mdbx PROPERTIES LINK_FLAGS "-fvisibility=hidden")
  endif()
  list(APPEND MDBX_BUILD_FLAGS ${CMAKE_SHARED_LINKER_FLAGS})
  target_include_directories(mdbx INTERFACE "${CMAKE_CURRENT_SOURCE_DIR}")
endif()

if(MDBX_BUILD_SHARED_LIBRARY AND MDBX_LINK_TOOLS_NONSTATIC)
  set(TOOL_MDBX_LIB mdbx)

  # use, i.e. don't skip the full RPATH for the build tree
  set(CMAKE_SKIP_BUILD_RPATH FALSE)

  # when building, don't use the install RPATH already (but later on when installing)
  set(CMAKE_BUILD_WITH_INSTALL_RPATH FALSE)

  # add the automatically determined parts of the RPATH which point to directories outside the build tree to the install
  # RPATH
  set(CMAKE_INSTALL_RPATH_USE_LINK_PATH TRUE)

  # the RPATH to be used when installing, but only if it's not a system directory
  list(FIND CMAKE_PLATFORM_IMPLICIT_LINK_DIRECTORIES "${CMAKE_INSTALL_PREFIX}/lib" isSystemDir)
  if(isSystemDir EQUAL -1)
    if(${CMAKE_SYSTEM_NAME} STREQUAL "Darwin")
      set(CMAKE_INSTALL_RPATH "@executable_path/../lib")
    else()
      set(CMAKE_INSTALL_RPATH "\$ORIGIN/../lib")
    endif()
  endif()

  if(WIN32)
    # Windows don't have RPATH feature, therefore we should prepare PATH or copy DLL(s)
    set(TOOL_MDBX_DLLCRUTCH "Crutch for ${CMAKE_SYSTEM_NAME}")
    if(NOT CMAKE_CONFIGURATION_TYPES AND NOT CMAKE_VERSION VERSION_LESS 3.0)
      # will use LOCATION property to compose DLLPATH
      cmake_policy(SET CMP0026 OLD)
    endif()
  else()
    set(TOOL_MDBX_DLLCRUTCH FALSE)
  endif()
else()
  set(TOOL_MDBX_LIB mdbx-static)
  set(TOOL_MDBX_DLLCRUTCH FALSE)
endif()

# build mdbx-tools
if(MDBX_BUILD_TOOLS)
  set(WINGETOPT_SRC "")
  if(WIN32)
    set(WINGETOPT_SRC ${MDBX_SOURCE_DIR}/tools/wingetopt.c ${MDBX_SOURCE_DIR}/tools/wingetopt.h)
  endif()

  foreach(TOOL chk copy stat dump load drop)
    if(MDBX_AMALGAMATED_SOURCE)
      add_executable(mdbx_${TOOL} mdbx.h ${MDBX_SOURCE_DIR}/mdbx_${TOOL}.c)
    else()
      add_executable(mdbx_${TOOL} mdbx.h ${MDBX_SOURCE_DIR}/tools/${TOOL}.c ${WINGETOPT_SRC})
    endif()
    if(NOT C_FALLBACK_GNU11 AND NOT C_FALLBACK_11)
      set_target_properties(mdbx_${TOOL} PROPERTIES C_STANDARD ${MDBX_C_STANDARD} C_STANDARD_REQUIRED ON)
    endif()
    target_setup_options(mdbx_${TOOL})
    target_link_libraries(mdbx_${TOOL} ${TOOL_MDBX_LIB})
  endforeach()
  if(LIB_MATH)
    target_link_libraries(mdbx_chk ${LIB_MATH})
    target_link_libraries(mdbx_stat ${LIB_MATH})
  endif()
endif()

# ######################################################################################################################

# mdbx-shared-lib installation
if(NOT DEFINED MDBX_DLL_INSTALL_DESTINATION)
  if(WIN32)
    set(MDBX_DLL_INSTALL_DESTINATION ${CMAKE_INSTALL_BINDIR})
  else()
    set(MDBX_DLL_INSTALL_DESTINATION ${CMAKE_INSTALL_LIBDIR})
  endif()
endif()
if(MDBX_BUILD_SHARED_LIBRARY)
  if(CMAKE_VERSION VERSION_LESS 3.12)
    install(
      TARGETS mdbx
      EXPORT libmdbx
      LIBRARY DESTINATION ${MDBX_DLL_INSTALL_DESTINATION} COMPONENT runtime
      ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT devel
      PUBLIC_HEADER DESTINATION ${CMAKE_INSTALL_INCLUDEDIR} COMPONENT devel
      INCLUDES
      DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}
      COMPONENT devel)
  else()
    install(
      TARGETS mdbx
      EXPORT libmdbx
      LIBRARY DESTINATION ${MDBX_DLL_INSTALL_DESTINATION}
              COMPONENT runtime
              NAMELINK_COMPONENT devel
      OBJECTS DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT devel
      ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT devel
      PUBLIC_HEADER DESTINATION ${CMAKE_INSTALL_INCLUDEDIR} COMPONENT devel
      INCLUDES
      DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}
      COMPONENT devel)
  endif()
endif(MDBX_BUILD_SHARED_LIBRARY)

# mdbx-tools installation
if(MDBX_BUILD_TOOLS)
  if(NOT DEFINED MDBX_TOOLS_INSTALL_DESTINATION)
    set(MDBX_TOOLS_INSTALL_DESTINATION ${CMAKE_INSTALL_BINDIR})
  endif()
  install(TARGETS mdbx_chk mdbx_stat mdbx_copy mdbx_dump mdbx_load mdbx_drop
          RUNTIME DESTINATION ${MDBX_TOOLS_INSTALL_DESTINATION} COMPONENT runtime)
  if(MDBX_INSTALL_MANPAGES)
    if(NOT DEFINED MDBX_MAN_INSTALL_DESTINATION)
      set(MDBX_MAN_INSTALL_DESTINATION ${CMAKE_INSTALL_MANDIR}/man1)
    endif()
    install(
      FILES "${MDBX_SOURCE_DIR}/man1/mdbx_chk.1" "${MDBX_SOURCE_DIR}/man1/mdbx_stat.1"
            "${MDBX_SOURCE_DIR}/man1/mdbx_copy.1" "${MDBX_SOURCE_DIR}/man1/mdbx_dump.1"
            "${MDBX_SOURCE_DIR}/man1/mdbx_load.1" "${MDBX_SOURCE_DIR}/man1/mdbx_drop.1"
      DESTINATION ${MDBX_MAN_INSTALL_DESTINATION}
      COMPONENT doc)
  endif()
endif(MDBX_BUILD_TOOLS)

# mdbx-static-lib installation
if(MDBX_INSTALL_STATIC)
  if(CMAKE_VERSION VERSION_LESS 3.12)
    install(
      TARGETS mdbx-static
      EXPORT libmdbx
      LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT devel
      OBJECTS DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT devel
      ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT devel
      PUBLIC_HEADER DESTINATION ${CMAKE_INSTALL_INCLUDEDIR} COMPONENT devel
      INCLUDES
      DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}
      COMPONENT devel)
  else()
    install(
      TARGETS mdbx-static
      EXPORT libmdbx
      LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
              COMPONENT devel
              NAMELINK_COMPONENT devel
      OBJECTS DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT devel
      ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR} COMPONENT devel
      PUBLIC_HEADER DESTINATION ${CMAKE_INSTALL_INCLUDEDIR} COMPONENT devel
      INCLUDES
      DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}
      COMPONENT devel)
  endif()
endif(MDBX_INSTALL_STATIC)

# ######################################################################################################################

# collect options & build info
if(NOT DEFINED MDBX_BUILD_TIMESTAMP)
  if(NOT "$ENV{SOURCE_DATE_EPOCH}" STREQUAL "")
    set(MDBX_BUILD_TIMESTAMP "$ENV{SOURCE_DATE_EPOCH}")
  else()
    string(TIMESTAMP MDBX_BUILD_TIMESTAMP UTC)
  endif()
endif()
set(MDBX_BUILD_FLAGS ${CMAKE_C_FLAGS})
if(MDBX_BUILD_CXX)
  set(MDBX_BUILD_FLAGS ${CMAKE_CXX_FLAGS})
endif()

# append cmake's build-type flags and defines
if(NOT CMAKE_CONFIGURATION_TYPES)
  list(APPEND MDBX_BUILD_FLAGS ${CMAKE_C_FLAGS_${CMAKE_BUILD_TYPE_UPPERCASE}})
  if(MDBX_BUILD_CXX)
    list(APPEND MDBX_BUILD_FLAGS ${CMAKE_CXX_FLAGS_${CMAKE_BUILD_TYPE_UPPERCASE}})
  endif()
endif()

# choice target to fetch definitions and options
if(MDBX_BUILD_SHARED_LIBRARY)
  set(target4fetch mdbx)
else()
  set(target4fetch mdbx-static)
endif()

# get definitions
get_target_property(defs_list ${target4fetch} COMPILE_DEFINITIONS)
if(defs_list)
  list(APPEND MDBX_BUILD_FLAGS ${defs_list})
endif()

# get target compile options
get_target_property(options_list ${target4fetch} COMPILE_OPTIONS)
if(options_list)
  list(APPEND MDBX_BUILD_FLAGS ${options_list})
endif()

list(REMOVE_DUPLICATES MDBX_BUILD_FLAGS)
string(REPLACE ";" " " MDBX_BUILD_FLAGS "${MDBX_BUILD_FLAGS}")
if(CMAKE_CONFIGURATION_TYPES)
  # add dynamic part via per-configuration define
  message(STATUS "MDBX Compile Flags: ${MDBX_BUILD_FLAGS} <AND CONFIGURATION DEPENDENT>")
  add_definitions(
    -DMDBX_BUILD_FLAGS_CONFIG="$<$<CONFIG:Debug>:${CMAKE_C_FLAGS_DEBUG} ${CMAKE_C_DEFINES_DEBUG}>$<$<CONFIG:Release>:${CMAKE_C_FLAGS_RELEASE} ${CMAKE_C_DEFINES_RELEASE}>$<$<CONFIG:RelWithDebInfo>:${CMAKE_C_FLAGS_RELWITHDEBINFO} ${CMAKE_C_DEFINES_RELWITHDEBINFO}>$<$<CONFIG:MinSizeRel>:${CMAKE_C_FLAGS_MINSIZEREL} ${CMAKE_C_DEFINES_MINSIZEREL}>"
  )
else()
  message(STATUS "MDBX Compile Flags: ${MDBX_BUILD_FLAGS}")
endif()

# get compiler info
execute_process(
  COMMAND sh -c "${CMAKE_C_COMPILER} --version | head -1"
  OUTPUT_VARIABLE MDBX_BUILD_COMPILER
  OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET
  RESULT_VARIABLE rc)
if(rc OR NOT MDBX_BUILD_COMPILER)
  string(STRIP "${CMAKE_C_COMPILER_ID}-${CMAKE_C_COMPILER_VERSION}" MDBX_BUILD_COMPILER)
endif()

# make a build-target triplet
if(CMAKE_C_COMPILER_TARGET)
  set(MDBX_BUILD_TARGET "${CMAKE_C_COMPILER_TARGET}")
else()
  if(CMAKE_C_COMPILER_ARCHITECTURE_ID)
    string(STRIP "${CMAKE_C_COMPILER_ARCHITECTURE_ID}" MDBX_BUILD_TARGET)
  elseif(CMAKE_GENERATOR_PLATFORM AND NOT CMAKE_GENERATOR_PLATFORM STREQUAL CMAKE_SYSTEM_NAME)
    string(STRIP "${CMAKE_GENERATOR_PLATFORM}" MDBX_BUILD_TARGET)
  elseif(CMAKE_SYSTEM_ARCH)
    string(STRIP "${CMAKE_SYSTEM_ARCH}" MDBX_BUILD_TARGET)
  elseif(CMAKE_LIBRARY_ARCHITECTURE)
    string(STRIP "${CMAKE_LIBRARY_ARCHITECTURE}" MDBX_BUILD_TARGET)
  elseif(CMAKE_SYSTEM_PROCESSOR)
    string(STRIP "${CMAKE_SYSTEM_PROCESSOR}" MDBX_BUILD_TARGET)
  else()
    set(MDBX_BUILD_TARGET "unknown")
  endif()
  if(CMAKE_C_COMPILER_ABI AND NOT (CMAKE_C_COMPILER_ABI MATCHES ".*${MDBX_BUILD_TARGET}.*"
                                   OR MDBX_BUILD_TARGET MATCHES ".*${CMAKE_C_COMPILER_ABI}.*"))
    string(CONCAT MDBX_BUILD_TARGET "${MDBX_BUILD_TARGET}-${CMAKE_C_COMPILER_ABI}")
  endif()
  if(CMAKE_C_PLATFORM_ID
     AND NOT (CMAKE_SYSTEM_NAME AND (CMAKE_C_PLATFORM_ID MATCHES ".*${CMAKE_SYSTEM_NAME}.*"
                                     OR CMAKE_SYSTEM_NAME MATCHES ".*${CMAKE_C_PLATFORM_ID}.*"))
     AND NOT (CMAKE_C_PLATFORM_ID MATCHES ".*${CMAKE_C_PLATFORM_ID}.*" OR MDBX_BUILD_TARGET MATCHES
                                                                          ".*${CMAKE_C_PLATFORM_ID}.*"))
    string(CONCAT MDBX_BUILD_TARGET "${MDBX_BUILD_TARGET}-${CMAKE_C_COMPILER_ABI}")
  endif()
  if(CMAKE_SYSTEM_NAME)
    string(CONCAT MDBX_BUILD_TARGET "${MDBX_BUILD_TARGET}-${CMAKE_SYSTEM_NAME}")
  endif()
endif()

# provide build-type
if(CMAKE_CONFIGURATION_TYPES)
  # via per-configuration define
  add_definitions(-DMDBX_BUILD_TYPE="$<CONFIG>")
  set(MDBX_BUILD_TYPE "<CONFIGURATION DEPENDENT>")
else()
  set(MDBX_BUILD_TYPE ${CMAKE_BUILD_TYPE})
endif()

# options
set(options VERSION C_COMPILER CXX_COMPILER MDBX_BUILD_TARGET MDBX_BUILD_TYPE ${MDBX_BUILD_OPTIONS})
foreach(item IN LISTS options)
  if(DEFINED ${item})
    set(value "${${item}}")
  elseif(DEFINED MDBX_${item})
    set(item MDBX_${item})
    set(value "${${item}}")
  elseif(DEFINED CMAKE_${item})
    set(item CMAKE_${item})
    set(value "${${item}}")
  else()
    set(value "AUTO (not pre-defined explicitly)")
  endif()
  message(STATUS "${item}: ${value}")
endforeach(item)

# provide config.h for library build info
configure_file("${MDBX_SOURCE_DIR}/config.h.in" "${CMAKE_CURRENT_BINARY_DIR}/config.h" ESCAPE_QUOTES)
add_definitions(-DMDBX_CONFIG_H="${CMAKE_CURRENT_BINARY_DIR}/config.h")

# ######################################################################################################################

if(NOT MDBX_AMALGAMATED_SOURCE AND MDBX_ENABLE_TESTS)
  if(NOT CMAKE_CXX_COMPILER_LOADED)
    message(FATAL_ERROR "MDBX_ENABLE_TESTS=${MDBX_ENABLE_TESTS}: The C++ compiler is required to build the tests.")
  endif()
  add_subdirectory(test)
endif()

# ######################################################################################################################

if(NOT SUBPROJECT)
  set(PACKAGE "libmdbx")
  set(CPACK_PACKAGE_VERSION_MAJOR ${MDBX_VERSION_MAJOR})
  set(CPACK_PACKAGE_VERSION_MINOR ${MDBX_VERSION_MINOR})
  set(CPACK_PACKAGE_VERSION_PATCH ${MDBX_VERSION_PATCH})
  set(CPACK_PACKAGE_VERSION_TWEAK ${MDBX_VERSION_TWEAK})
  set(PACKAGE_VERSION ${MDBX_VERSION})
  message(STATUS "libmdbx package version is ${PACKAGE_VERSION}")
endif()

cmake_policy(POP)
