# Copyright (c) 2020-2026 Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> ###############################################
# SPDX-License-Identifier: Apache-2.0
#
project(mdbx_ut_and_examples)

if(NOT DEFINED MDBX_LIBRARY)
  set(MDBX_LIBRARY mdbx)
endif()

if(MDBX_BUILD_CXX AND CMAKE_CXX_COMPILER_LOADED AND NOT MDBX_CXX_STANDARD LESS 17)
  add_executable(mdbx_modern_example example-mdbx.c++)
  set_target_properties(mdbx_modern_example PROPERTIES CXX_STANDARD ${MDBX_CXX_STANDARD} CXX_STANDARD_REQUIRED ON)
  target_link_libraries(mdbx_modern_example ${MDBX_LIBRARY})
  if(DEFINED INTERPROCEDURAL_OPTIMIZATION)
    set_target_properties(mdbx_modern_example PROPERTIES INTERPROCEDURAL_OPTIMIZATION $<BOOL:${INTERPROCEDURAL_OPTIMIZATION}>)
  endif()
else()
  message(NOTICE "The C++ example will be skipped, since C++17 standard is unavailable or C++ API of libmdbx was disabled.")
  set(MDBX_BUILD_CXX FALSE)
endif()

add_executable(mdbx_legacy_example example-mdbx.c)
target_link_libraries(mdbx_legacy_example ${MDBX_LIBRARY})

if(CMAKE_CROSSCOMPILING AND NOT CMAKE_CROSSCOMPILING_EMULATOR)
  message(NOTICE "No emulator to run cross-compiled tests")
  add_test(NAME fake_since_no_crosscompiling_emulator COMMAND ${CMAKE_COMMAND} -E echo
                                                              "No emulator to run cross-compiled tests")
else()
  add_test(NAME c_api COMMAND mdbx_legacy_example)
  if(MDBX_BUILD_CXX)
    add_test(NAME c++_api COMMAND mdbx_modern_example)
  endif()
endif()

if(UNIX)
  add_executable(pcrf_simulator pcrf/pcrf_simulator.c)
  target_link_libraries(pcrf_simulator ${MDBX_LIBRARY})
endif()
