cmake_minimum_required(VERSION 3.12)

foreach(name CHK COPY DEFRAG DROP DUMP LOAD STAT WORKDIR)
  if(NOT DEFINED MDBX_${name} OR "${MDBX_${name}}" STREQUAL "")
    message(FATAL_ERROR "MDBX_${name} is required")
  endif()
endforeach()

function(run_step name)
  execute_process(COMMAND ${ARGN}
                  RESULT_VARIABLE rc
                  OUTPUT_VARIABLE stdout
                  ERROR_VARIABLE stderr)
  if(NOT rc EQUAL 0)
    message(FATAL_ERROR "${name} failed with ${rc}\nstdout:\n${stdout}\nstderr:\n${stderr}")
  endif()
endfunction()

function(run_step_capture name outvar)
  execute_process(COMMAND ${ARGN}
                  RESULT_VARIABLE rc
                  OUTPUT_VARIABLE stdout
                  ERROR_VARIABLE stderr)
  if(NOT rc EQUAL 0)
    message(FATAL_ERROR "${name} failed with ${rc}\nstdout:\n${stdout}\nstderr:\n${stderr}")
  endif()
  set(${outvar} "${stdout}\n${stderr}" PARENT_SCOPE)
endfunction()

function(assert_stat_has_overflow name dbpath)
  run_step_capture("stat ${name}" stat_output "${MDBX_STAT}" -q -a "${dbpath}")
  if(NOT stat_output MATCHES "Large/Overflow pages:[ \t]*[1-9][0-9]*")
    message(FATAL_ERROR "${name} did not report overflow pages\n${stat_output}")
  endif()
endfunction()

set(workdir "${MDBX_WORKDIR}")
file(REMOVE_RECURSE "${workdir}")
file(MAKE_DIRECTORY "${workdir}")

set(seed_dump "${workdir}/seed.dump")
set(source_db "${workdir}/source.mdbx")
set(reloaded_db "${workdir}/reloaded.mdbx")
set(copy_db "${workdir}/copy.mdbx")
set(compact_db "${workdir}/compact-copy.mdbx")
set(defrag_db "${workdir}/defrag.mdbx")
set(defrag_live_db "${workdir}/defrag-live.mdbx")
set(source_dump "${workdir}/source.dump")
set(reloaded_dump "${workdir}/reloaded.dump")
set(defrag_live_before_dump "${workdir}/defrag-live-before.dump")
set(defrag_live_after_dump "${workdir}/defrag-live-after.dump")

file(WRITE "${seed_dump}" [=[
VERSION=3
format=print
type=btree
db_pagesize=4096
duplicates=0
HEADER=END
 alpha
 value-one
 beta
 value-two
 gamma\5cslash
 value\0awith\0anewlines
 omega
 final-value
]=])

set(overflow_a "")
set(overflow_b "")
foreach(i RANGE 1 260)
  string(APPEND overflow_a "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz")
endforeach()
foreach(i RANGE 1 340)
  string(APPEND overflow_b "mdbxOVERFLOWroundtrip")
endforeach()
file(APPEND "${seed_dump}" " overflow-a\n ${overflow_a}\n overflow-b\n ${overflow_b}\nDATA=END\n")

run_step("load seed" "${MDBX_LOAD}" -q -n -f "${seed_dump}" "${source_db}")
run_step("check source" "${MDBX_CHK}" -q "${source_db}")
run_step("stat source env" "${MDBX_STAT}" -q -e "${source_db}")
assert_stat_has_overflow("source tables" "${source_db}")
run_step("dump source" "${MDBX_DUMP}" -q -f "${source_dump}" "${source_db}")
run_step("copy live defrag source" "${MDBX_COPY}" -q "${source_db}" "${defrag_live_db}")
run_step("drop live defrag staging contents" "${MDBX_DROP}" -q "${defrag_live_db}")
run_step("reload live defrag source" "${MDBX_LOAD}" -q -n -f "${source_dump}" "${defrag_live_db}")
run_step("check live defrag before" "${MDBX_CHK}" -q "${defrag_live_db}")
run_step("dump live defrag before" "${MDBX_DUMP}" -q -f "${defrag_live_before_dump}" "${defrag_live_db}")
assert_stat_has_overflow("live defrag before" "${defrag_live_db}")
run_step("defrag live overflow copy" "${MDBX_DEFRAG}" -q -3 "${defrag_live_db}")
run_step("check live defragged copy" "${MDBX_CHK}" -q "${defrag_live_db}")
assert_stat_has_overflow("live defrag after" "${defrag_live_db}")
run_step("dump live defrag after" "${MDBX_DUMP}" -q -f "${defrag_live_after_dump}" "${defrag_live_db}")
run_step("compare live defrag dumps" "${CMAKE_COMMAND}" -E compare_files "${defrag_live_before_dump}"
         "${defrag_live_after_dump}")
run_step("copy defrag source" "${MDBX_COPY}" -q "${source_db}" "${defrag_db}")
run_step("drop defrag contents" "${MDBX_DROP}" -q "${defrag_db}")
run_step("defrag dropped copy" "${MDBX_DEFRAG}" -q -1 "${defrag_db}")
run_step("check defragged copy" "${MDBX_CHK}" -q "${defrag_db}")
run_step("reload dump" "${MDBX_LOAD}" -q -n -f "${source_dump}" "${reloaded_db}")
run_step("check reloaded" "${MDBX_CHK}" -q "${reloaded_db}")
run_step("stat reloaded env" "${MDBX_STAT}" -q -e "${reloaded_db}")
assert_stat_has_overflow("reloaded tables" "${reloaded_db}")
run_step("dump reloaded" "${MDBX_DUMP}" -q -f "${reloaded_dump}" "${reloaded_db}")
run_step("compare dumps" "${CMAKE_COMMAND}" -E compare_files "${source_dump}" "${reloaded_dump}")
run_step("copy source" "${MDBX_COPY}" -q "${source_db}" "${copy_db}")
run_step("check copy" "${MDBX_CHK}" -q "${copy_db}")
run_step("stat copy env" "${MDBX_STAT}" -q -e "${copy_db}")
assert_stat_has_overflow("copy tables" "${copy_db}")
run_step("compact copy source" "${MDBX_COPY}" -q -c "${source_db}" "${compact_db}")
run_step("check compact copy" "${MDBX_CHK}" -q "${compact_db}")
run_step("stat compact copy env" "${MDBX_STAT}" -q -e "${compact_db}")
assert_stat_has_overflow("compact copy tables" "${compact_db}")
