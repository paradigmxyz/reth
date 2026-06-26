# Copyright (c) 2020-2026 Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> ###############################################
# SPDX-License-Identifier: Apache-2.0
#
project(mdbx_ut_and_examples)

if(NOT DEFINED MDBX_LIBRARY)
  set(MDBX_LIBRARY mdbx)
endif()

if(MDBX_BUILD_CXX
   AND CMAKE_CXX_COMPILER_LOADED
   AND NOT MDBX_CXX_STANDARD LESS 17)
  add_executable(mdbx_modern_example example-mdbx.c++)
  set_target_properties(mdbx_modern_example PROPERTIES CXX_STANDARD ${MDBX_CXX_STANDARD} CXX_STANDARD_REQUIRED ON)
  target_link_libraries(mdbx_modern_example ${MDBX_LIBRARY})
  if(DEFINED INTERPROCEDURAL_OPTIMIZATION)
    set_target_properties(mdbx_modern_example PROPERTIES INTERPROCEDURAL_OPTIMIZATION
                                                         $<BOOL:${INTERPROCEDURAL_OPTIMIZATION}>)
  endif()
else()
  message(NOTICE
          "The C++ example will be skipped, since C++17 standard is unavailable or C++ API of libmdbx was disabled.")
  set(MDBX_BUILD_CXX FALSE)
endif()

add_executable(mdbx_legacy_example example-mdbx.c)
target_link_libraries(mdbx_legacy_example ${MDBX_LIBRARY})

add_executable(mdbx_migration_smoke migration-smoke.c)
target_link_libraries(mdbx_migration_smoke ${MDBX_LIBRARY})

add_executable(mdbx_migration_smoke_stress migration-smoke.c)
target_compile_definitions(mdbx_migration_smoke_stress
                           PRIVATE MULTIPROCESS_STRESS_READERS=4 MULTIPROCESS_STRESS_WRITERS=4
                                   MULTIPROCESS_STRESS_WAVES=8)
target_link_libraries(mdbx_migration_smoke_stress ${MDBX_LIBRARY})

add_executable(mdbx_migration_smoke_randomized_stress migration-smoke.c)
target_compile_definitions(mdbx_migration_smoke_randomized_stress
                           PRIVATE MULTIPROCESS_STRESS_READERS=4 MULTIPROCESS_STRESS_WRITERS=4
                                   MULTIPROCESS_STRESS_WAVES=8 MULTIPROCESS_STRESS_RANDOMIZED=1)
target_link_libraries(mdbx_migration_smoke_randomized_stress ${MDBX_LIBRARY})

add_executable(mdbx_migration_smoke_crash_stress migration-smoke.c)
target_compile_definitions(mdbx_migration_smoke_crash_stress
                           PRIVATE MULTIPROCESS_STRESS_READERS=4 MULTIPROCESS_STRESS_WRITERS=4
                                   MULTIPROCESS_STRESS_WAVES=8 MULTIPROCESS_CRASH_STRESS=1)
target_link_libraries(mdbx_migration_smoke_crash_stress ${MDBX_LIBRARY})

if(MDBX_ENABLE_DXB_FAULT_INJECTION)
  add_executable(mdbx_migration_fault_injection migration-smoke.c)
  target_compile_definitions(mdbx_migration_fault_injection PRIVATE MIGRATION_FAULT_INJECTION=1)
  if(CMAKE_C_COMPILER_ID MATCHES "GNU|Clang")
    target_compile_options(mdbx_migration_fault_injection PRIVATE -Wno-unused-function)
  endif()
  target_link_libraries(mdbx_migration_fault_injection ${MDBX_LIBRARY})
endif()

if(CMAKE_CROSSCOMPILING AND NOT CMAKE_CROSSCOMPILING_EMULATOR)
  message(NOTICE "No emulator to run cross-compiled tests")
  add_test(NAME fake_since_no_crosscompiling_emulator COMMAND ${CMAKE_COMMAND} -E echo
                                                              "No emulator to run cross-compiled tests")
else()
  add_test(NAME c_api COMMAND mdbx_legacy_example)
  add_test(NAME c_api_nommap COMMAND mdbx_legacy_example)
  set_tests_properties(c_api c_api_nommap PROPERTIES RESOURCE_LOCK mdbx_example_database)
  set_tests_properties(c_api_nommap PROPERTIES ENVIRONMENT
                                               "MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  add_test(NAME migration_smoke COMMAND mdbx_migration_smoke)
  add_test(NAME migration_smoke_nommap COMMAND mdbx_migration_smoke)
  set_tests_properties(migration_smoke_nommap PROPERTIES ENVIRONMENT "MDBX_FORCE_NO_DATA_MMAP=1")
  add_test(NAME migration_smoke_nommap_tinycache COMMAND mdbx_migration_smoke)
  set_tests_properties(migration_smoke_nommap_tinycache
                       PROPERTIES ENVIRONMENT "MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  add_test(NAME migration_smoke_stress_nommap_tinycache COMMAND mdbx_migration_smoke_stress)
  set_tests_properties(migration_smoke_stress_nommap_tinycache
                       PROPERTIES ENVIRONMENT "MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  add_test(NAME migration_smoke_randomized_stress_nommap_tinycache COMMAND mdbx_migration_smoke_randomized_stress)
  set_tests_properties(migration_smoke_randomized_stress_nommap_tinycache
                       PROPERTIES ENVIRONMENT "MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  add_test(NAME migration_smoke_crash_stress_nommap_tinycache COMMAND mdbx_migration_smoke_crash_stress)
  set_tests_properties(migration_smoke_crash_stress_nommap_tinycache
                       PROPERTIES ENVIRONMENT "MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  if(MDBX_ENABLE_DXB_FAULT_INJECTION)
    add_test(NAME migration_fault_injection_nommap COMMAND mdbx_migration_fault_injection)
    set_tests_properties(migration_fault_injection_nommap PROPERTIES ENVIRONMENT "MDBX_FORCE_NO_DATA_MMAP=1")
    add_test(NAME migration_fault_injection_nommap_tinycache COMMAND mdbx_migration_fault_injection)
    set_tests_properties(migration_fault_injection_nommap_tinycache
                         PROPERTIES ENVIRONMENT "MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  endif()
  if(TARGET mdbx_chk
     AND TARGET mdbx_copy
     AND TARGET mdbx_defrag
     AND TARGET mdbx_drop
     AND TARGET mdbx_dump
     AND TARGET mdbx_load
     AND TARGET mdbx_stat)
    add_test(NAME migration_tool_roundtrip
             COMMAND
               ${CMAKE_COMMAND} -DMDBX_CHK=$<TARGET_FILE:mdbx_chk> -DMDBX_COPY=$<TARGET_FILE:mdbx_copy>
               -DMDBX_DEFRAG=$<TARGET_FILE:mdbx_defrag> -DMDBX_DROP=$<TARGET_FILE:mdbx_drop>
               -DMDBX_DUMP=$<TARGET_FILE:mdbx_dump>
               -DMDBX_LOAD=$<TARGET_FILE:mdbx_load>
               -DMDBX_STAT=$<TARGET_FILE:mdbx_stat>
               -DMDBX_WORKDIR=${CMAKE_CURRENT_BINARY_DIR}/migration-tool-roundtrip
               -P ${CMAKE_CURRENT_SOURCE_DIR}/migration-tool-roundtrip.cmake)
    add_test(NAME migration_tool_roundtrip_nommap
             COMMAND
               ${CMAKE_COMMAND} -DMDBX_CHK=$<TARGET_FILE:mdbx_chk> -DMDBX_COPY=$<TARGET_FILE:mdbx_copy>
               -DMDBX_DEFRAG=$<TARGET_FILE:mdbx_defrag> -DMDBX_DROP=$<TARGET_FILE:mdbx_drop>
               -DMDBX_DUMP=$<TARGET_FILE:mdbx_dump>
               -DMDBX_LOAD=$<TARGET_FILE:mdbx_load>
               -DMDBX_STAT=$<TARGET_FILE:mdbx_stat>
               -DMDBX_WORKDIR=${CMAKE_CURRENT_BINARY_DIR}/migration-tool-roundtrip-nommap
               -P ${CMAKE_CURRENT_SOURCE_DIR}/migration-tool-roundtrip.cmake)
    set_tests_properties(migration_tool_roundtrip_nommap PROPERTIES ENVIRONMENT "MDBX_FORCE_NO_DATA_MMAP=1")
    add_test(NAME migration_tool_roundtrip_nommap_tinycache
             COMMAND
               ${CMAKE_COMMAND} -DMDBX_CHK=$<TARGET_FILE:mdbx_chk> -DMDBX_COPY=$<TARGET_FILE:mdbx_copy>
               -DMDBX_DEFRAG=$<TARGET_FILE:mdbx_defrag> -DMDBX_DROP=$<TARGET_FILE:mdbx_drop>
               -DMDBX_DUMP=$<TARGET_FILE:mdbx_dump>
               -DMDBX_LOAD=$<TARGET_FILE:mdbx_load>
               -DMDBX_STAT=$<TARGET_FILE:mdbx_stat>
               -DMDBX_WORKDIR=${CMAKE_CURRENT_BINARY_DIR}/migration-tool-roundtrip-nommap-tinycache
               -P ${CMAKE_CURRENT_SOURCE_DIR}/migration-tool-roundtrip.cmake)
    set_tests_properties(migration_tool_roundtrip_nommap_tinycache
                         PROPERTIES ENVIRONMENT
                                    "MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  endif()
  if(MDBX_BUILD_CXX)
    add_test(NAME c++_api COMMAND mdbx_modern_example)
    add_test(NAME c++_api_nommap COMMAND mdbx_modern_example)
    set_tests_properties(c++_api c++_api_nommap PROPERTIES RESOURCE_LOCK mdbx_example_database)
    set_tests_properties(c++_api_nommap PROPERTIES ENVIRONMENT
                                                   "MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  endif()
endif()

if(UNIX)
  add_executable(pcrf_simulator pcrf/pcrf_simulator.c)
  target_link_libraries(pcrf_simulator ${MDBX_LIBRARY})
  if(NOT CMAKE_CROSSCOMPILING OR CMAKE_CROSSCOMPILING_EMULATOR)
    add_test(NAME pcrf_simulator_smoke COMMAND pcrf_simulator)
    set_tests_properties(pcrf_simulator_smoke
                         PROPERTIES
                           ENVIRONMENT
                           "PCRF_DB_PATH=./pcrf-smoke;PCRF_INITIAL_RECORDS=256;PCRF_CHURN_ROUNDS=3;PCRF_CHURN_BATCH=32;PCRF_CHURN_EXTRA=8;PCRF_STAT_INTERVAL_US=0")
    add_test(NAME pcrf_simulator_nommap_smoke COMMAND pcrf_simulator)
    set_tests_properties(pcrf_simulator_nommap_smoke
                         PROPERTIES
                           ENVIRONMENT
                           "PCRF_DB_PATH=./pcrf-nommap-smoke;PCRF_INITIAL_RECORDS=256;PCRF_CHURN_ROUNDS=3;PCRF_CHURN_BATCH=32;PCRF_CHURN_EXTRA=8;PCRF_STAT_INTERVAL_US=0;MDBX_FORCE_NO_DATA_MMAP=1;MDBX_EXPLICIT_PAGE_CACHE_LIMIT=64K")
  endif()
endif()
