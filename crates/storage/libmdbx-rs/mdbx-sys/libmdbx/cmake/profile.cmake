# Copyright (c) 2012-2025 Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> ###############################################
# SPDX-License-Identifier: Apache-2.0

if(CMAKE_VERSION VERSION_LESS 3.8.2)
  cmake_minimum_required(VERSION 3.0.2)
elseif(CMAKE_VERSION VERSION_LESS 3.12)
  cmake_minimum_required(VERSION 3.8.2)
else()
  cmake_minimum_required(VERSION 3.12)
endif()

cmake_policy(PUSH)
cmake_policy(VERSION ${CMAKE_MINIMUM_REQUIRED_VERSION})

unset(MEMCHECK_OPTION_NAME)
if(NOT DEFINED ENABLE_MEMCHECK)
  if(DEFINED MDBX_USE_VALGRIND)
    set(MEMCHECK_OPTION_NAME "MDBX_USE_VALGRIND")
  elseif(DEFINED ENABLE_VALGRIND)
    set(MEMCHECK_OPTION_NAME "ENABLE_VALGRIND")
  else()
    set(MEMCHECK_OPTION_NAME "ENABLE_MEMCHECK")
  endif()
  if(MEMCHECK_OPTION_NAME STREQUAL "ENABLE_MEMCHECK")
    option(ENABLE_MEMCHECK "Enable integration with valgrind, a memory analyzing tool" OFF)
  elseif(${MEMCHECK_OPTION_NAME})
    set(ENABLE_MEMCHECK ON)
  else()
    set(ENABLE_MEMCHECK OFF)
  endif()
endif()

include(CheckLibraryExists)
check_library_exists(gcov __gcov_flush "" HAVE_GCOV)

option(ENABLE_GCOV "Enable integration with gcov, a code coverage program" OFF)

option(ENABLE_GPROF "Enable integration with gprof, a performance analyzing tool" OFF)

option(ENABLE_ASAN "Enable AddressSanitizer, a fast memory error detector based on compiler instrumentation" OFF)

option(ENABLE_UBSAN
       "Enable UndefinedBehaviorSanitizer, a fast undefined behavior detector based on compiler instrumentation" OFF)

if(ENABLE_MEMCHECK)
  if(CMAKE_CXX_COMPILER_LOADED)
    include(CheckIncludeFileCXX)
    check_include_file_cxx(valgrind/memcheck.h HAVE_VALGRIND_MEMCHECK_H)
  else()
    include(CheckIncludeFile)
    check_include_file(valgrind/memcheck.h HAVE_VALGRIND_MEMCHECK_H)
  endif()
  if(NOT HAVE_VALGRIND_MEMCHECK_H)
    message(FATAL_ERROR "${MEMCHECK_OPTION_NAME} option is set but valgrind/memcheck.h is not found")
  endif()
endif()

cmake_policy(POP)
