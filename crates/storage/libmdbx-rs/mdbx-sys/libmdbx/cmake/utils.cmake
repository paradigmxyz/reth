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

macro(add_option HIVE NAME DESCRIPTION DEFAULT)
  list(APPEND ${HIVE}_BUILD_OPTIONS ${HIVE}_${NAME})
  if(NOT ${DEFAULT} STREQUAL "AUTO")
    option(${HIVE}_${NAME} "${DESCRIPTION}" ${DEFAULT})
  elseif(NOT DEFINED ${HIVE}_${NAME})
    set(${HIVE}_${NAME}_AUTO ON)
  endif()
endmacro()

macro(set_if_undefined VARNAME)
  if(NOT DEFINED "${VARNAME}")
    set("${VARNAME}" ${ARGN})
  endif()
endmacro()

macro(add_compile_flags languages)
  foreach(_lang ${languages})
    string(REPLACE ";" " " _flags "${ARGN}")
    if(CMAKE_CXX_COMPILER_LOADED AND _lang STREQUAL "CXX")
      set("${_lang}_FLAGS" "${${_lang}_FLAGS} ${_flags}")
    endif()
    if(CMAKE_C_COMPILER_LOADED AND _lang STREQUAL "C")
      set("${_lang}_FLAGS" "${${_lang}_FLAGS} ${_flags}")
    endif()
  endforeach()
  unset(_lang)
  unset(_flags)
endmacro(add_compile_flags)

macro(remove_flag varname flag)
  string(REGEX REPLACE "^(.*)( ${flag} )(.*)$" "\\1 \\3" ${varname} ${${varname}})
  string(REGEX REPLACE "^((.+ )*)(${flag})(( .+)*)$" "\\1\\4" ${varname} ${${varname}})
endmacro(remove_flag)

macro(remove_compile_flag languages flag)
  foreach(_lang ${languages})
    if(CMAKE_CXX_COMPILER_LOADED AND _lang STREQUAL "CXX")
      remove_flag(${_lang}_FLAGS ${flag})
    endif()
    if(CMAKE_C_COMPILER_LOADED AND _lang STREQUAL "C")
      remove_flag(${_lang}_FLAGS ${flag})
    endif()
  endforeach()
  unset(_lang)
endmacro(remove_compile_flag)

macro(set_source_files_compile_flags)
  foreach(file ${ARGN})
    get_filename_component(_file_ext ${file} EXT)
    set(_lang "")
    if("${_file_ext}" STREQUAL ".m")
      set(_lang OBJC)
      # CMake believes that Objective C is a flavor of C++, not C, and uses g++ compiler for .m files. LANGUAGE property
      # forces CMake to use CC for ${file}
      set_source_files_properties(${file} PROPERTIES LANGUAGE C)
    elseif("${_file_ext}" STREQUAL ".mm")
      set(_lang OBJCXX)
    endif()

    if(_lang)
      get_source_file_property(_flags ${file} COMPILE_FLAGS)
      if("${_flags}" STREQUAL "NOTFOUND")
        set(_flags "${CMAKE_${_lang}_FLAGS}")
      else()
        set(_flags "${_flags} ${CMAKE_${_lang}_FLAGS}")
      endif()
      # message(STATUS "Set (${file} ${_flags}")
      set_source_files_properties(${file} PROPERTIES COMPILE_FLAGS "${_flags}")
    endif()
  endforeach()
  unset(_file_ext)
  unset(_lang)
endmacro(set_source_files_compile_flags)

macro(semver_parse str)
  set(_semver_ok FALSE)
  set(_semver_err "")
  set(_semver_major 0)
  set(_semver_minor 0)
  set(_semver_patch 0)
  set(_semver_tweak_withdot "")
  set(_semver_tweak "")
  set(_semver_extra "")
  set(_semver_prerelease_withdash "")
  set(_semver_prerelease "")
  set(_semver_buildmetadata_withplus "")
  set(_semver_buildmetadata "")
  if("${str}" MATCHES
     "^v?(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(\\.(0|[1-9][0-9]*))?([-+]-*[0-9a-zA-Z]+.*)?$")
    set(_semver_major ${CMAKE_MATCH_1})
    set(_semver_minor ${CMAKE_MATCH_2})
    set(_semver_patch ${CMAKE_MATCH_3})
    set(_semver_tweak_withdot ${CMAKE_MATCH_4})
    set(_semver_tweak ${CMAKE_MATCH_5})
    set(_semver_extra "${CMAKE_MATCH_6}")
    if("${_semver_extra}" STREQUAL "")
      set(_semver_ok TRUE)
    elseif("${_semver_extra}" MATCHES "^([.-][a-zA-Z0-9-]+)*(\\+[^+]+)?$")
      set(_semver_prerelease_withdash "${CMAKE_MATCH_1}")
      if(NOT "${_semver_prerelease_withdash}" STREQUAL "")
        string(SUBSTRING "${_semver_prerelease_withdash}" 1 -1 _semver_prerelease)
      endif()
      set(_semver_buildmetadata_withplus "${CMAKE_MATCH_2}")
      if(NOT "${_semver_buildmetadata_withplus}" STREQUAL "")
        string(SUBSTRING "${_semver_buildmetadata_withplus}" 1 -1 _semver_buildmetadata)
      endif()
      set(_semver_ok TRUE)
    else()
      set(_semver_err
          "Поля prerelease и/или buildmetadata (строка `-foo+bar` в составе `0.0.0[.0][-foo][+bar]`) не соответствуют SemVer-спецификации"
      )
    endif()
  else()
    set(_semver_err "Версионная отметка в целом не соответствует шаблону `0.0.0[.0][-foo][+bar]` SemVer-спецификации")
  endif()
endmacro(semver_parse)

function(_semver_parse_probe str expect)
  semver_parse(${str})
  if(expect AND NOT _semver_ok)
    message(FATAL_ERROR "semver_parse(${str}) expect SUCCESS, got ${_semver_ok}: ${_semver_err}")
  elseif(NOT expect AND _semver_ok)
    message(FATAL_ERROR "semver_parse(${str}) expect FAIL, got ${_semver_ok}")
  endif()
endfunction()

function(semver_parse_selfcheck)
  _semver_parse_probe("0.0.4" TRUE)
  _semver_parse_probe("v1.2.3" TRUE)
  _semver_parse_probe("10.20.30" TRUE)
  _semver_parse_probe("10.20.30.42" TRUE)
  _semver_parse_probe("1.1.2-prerelease+meta" TRUE)
  _semver_parse_probe("1.1.2+meta" TRUE)
  _semver_parse_probe("1.1.2+meta-valid" TRUE)
  _semver_parse_probe("1.0.0-alpha" TRUE)
  _semver_parse_probe("1.0.0-beta" TRUE)
  _semver_parse_probe("1.0.0-alpha.beta" TRUE)
  _semver_parse_probe("1.0.0-alpha.beta.1" TRUE)
  _semver_parse_probe("1.0.0-alpha.1" TRUE)
  _semver_parse_probe("1.0.0-alpha0.valid" TRUE)
  _semver_parse_probe("1.0.0-alpha.0valid" TRUE)
  _semver_parse_probe("1.0.0-alpha-a.b-c-somethinglong+build.1-aef.1-its-okay" TRUE)
  _semver_parse_probe("1.0.0-rc.1+build.1" TRUE)
  _semver_parse_probe("2.0.0-rc.1+build.123" TRUE)
  _semver_parse_probe("1.2.3-beta" TRUE)
  _semver_parse_probe("10.2.3-DEV-SNAPSHOT" TRUE)
  _semver_parse_probe("1.2.3-SNAPSHOT-123" TRUE)
  _semver_parse_probe("1.0.0" TRUE)
  _semver_parse_probe("2.0.0" TRUE)
  _semver_parse_probe("1.1.7" TRUE)
  _semver_parse_probe("2.0.0+build.1848" TRUE)
  _semver_parse_probe("2.0.1-alpha.1227" TRUE)
  _semver_parse_probe("1.0.0-alpha+beta" TRUE)
  _semver_parse_probe("1.2.3----RC-SNAPSHOT.12.9.1--.12+788" TRUE)
  _semver_parse_probe("1.2.3----R-S.12.9.1--.12+meta" TRUE)
  _semver_parse_probe("1.2.3----RC-SNAPSHOT.12.9.1--.12" TRUE)
  _semver_parse_probe("1.0.0+0.build.1-rc.10000aaa-kk-0.1" TRUE)
  _semver_parse_probe("99999999999999999999999.999999999999999999.99999999999999999" TRUE)
  _semver_parse_probe("v1.0.0-0A.is.legal" TRUE)

  _semver_parse_probe("1" FALSE)
  _semver_parse_probe("1.2" FALSE)
  # _semver_parse_probe("1.2.3-0123" FALSE) _semver_parse_probe("1.2.3-0123.0123" FALSE)
  _semver_parse_probe("1.1.2+.123" FALSE)
  _semver_parse_probe("+invalid" FALSE)
  _semver_parse_probe("-invalid" FALSE)
  _semver_parse_probe("-invalid+invalid" FALSE)
  _semver_parse_probe("-invalid.01" FALSE)
  _semver_parse_probe("alpha" FALSE)
  _semver_parse_probe("alpha.beta" FALSE)
  _semver_parse_probe("alpha.beta.1" FALSE)
  _semver_parse_probe("alpha.1" FALSE)
  _semver_parse_probe("alpha+beta" FALSE)
  _semver_parse_probe("alpha_beta" FALSE)
  _semver_parse_probe("alpha." FALSE)
  _semver_parse_probe("alpha.." FALSE)
  _semver_parse_probe("beta" FALSE)
  _semver_parse_probe("1.0.0-alpha_beta" FALSE)
  _semver_parse_probe("-alpha." FALSE)
  _semver_parse_probe("1.0.0-alpha.." FALSE)
  _semver_parse_probe("1.0.0-alpha..1" FALSE)
  _semver_parse_probe("1.0.0-alpha...1" FALSE)
  _semver_parse_probe("1.0.0-alpha....1" FALSE)
  _semver_parse_probe("1.0.0-alpha.....1" FALSE)
  _semver_parse_probe("1.0.0-alpha......1" FALSE)
  _semver_parse_probe("1.0.0-alpha.......1" FALSE)
  _semver_parse_probe("01.1.1" FALSE)
  _semver_parse_probe("1.01.1" FALSE)
  _semver_parse_probe("1.1.01" FALSE)
  _semver_parse_probe("1.2" FALSE)
  _semver_parse_probe("1.2.3.DEV" FALSE)
  _semver_parse_probe("1.2-SNAPSHOT" FALSE)
  _semver_parse_probe("1.2.31.2.3----RC-SNAPSHOT.12.09.1--..12+788" FALSE)
  _semver_parse_probe("1.2-RC-SNAPSHOT" FALSE)
  _semver_parse_probe("-1.0.3-gamma+b7718" FALSE)
  _semver_parse_probe("+justmeta" FALSE)
  _semver_parse_probe("9.8.7+meta+meta" FALSE)
  _semver_parse_probe("9.8.7-whatever+meta+meta" FALSE)
  _semver_parse_probe(
    "99999999999999999999999.999999999999999999.99999999999999999----RC-SNAPSHOT.12.09.1--------------------------------..12"
    FALSE)
endfunction()

macro(git_get_versioninfo source_root_directory)
  set(_git_describe "")
  set(_git_timestamp "")
  set(_git_tree "")
  set(_git_commit "")
  set(_git_last_vtag "")
  set(_git_trailing_commits 0)
  set(_git_is_dirty FALSE)

  execute_process(
    COMMAND ${GIT} show --no-patch --format=%cI HEAD
    OUTPUT_VARIABLE _git_timestamp
    OUTPUT_STRIP_TRAILING_WHITESPACE
    WORKING_DIRECTORY ${source_root_directory}
    RESULT_VARIABLE _rc)
  if(_rc OR "${_git_timestamp}" STREQUAL "%cI")
    execute_process(
      COMMAND ${GIT} show --no-patch --format=%ci HEAD
      OUTPUT_VARIABLE _git_timestamp
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE _rc)
    if(_rc OR "${_git_timestamp}" STREQUAL "%ci")
      message(FATAL_ERROR "Please install latest version of git (`show --no-patch --format=%cI HEAD` failed)")
    endif()
  endif()

  execute_process(
    COMMAND ${GIT} show --no-patch --format=%T HEAD
    OUTPUT_VARIABLE _git_tree
    OUTPUT_STRIP_TRAILING_WHITESPACE
    WORKING_DIRECTORY ${source_root_directory}
    RESULT_VARIABLE _rc)
  if(_rc OR "${_git_tree}" STREQUAL "")
    message(FATAL_ERROR "Please install latest version of git (`show --no-patch --format=%T HEAD` failed)")
  endif()

  execute_process(
    COMMAND ${GIT} show --no-patch --format=%H HEAD
    OUTPUT_VARIABLE _git_commit
    OUTPUT_STRIP_TRAILING_WHITESPACE
    WORKING_DIRECTORY ${source_root_directory}
    RESULT_VARIABLE _rc)
  if(_rc OR "${_git_commit}" STREQUAL "")
    message(FATAL_ERROR "Please install latest version of git (`show --no-patch --format=%H HEAD` failed)")
  endif()

  execute_process(
    COMMAND ${GIT} status --untracked-files=no --porcelain
    OUTPUT_VARIABLE _git_status
    OUTPUT_STRIP_TRAILING_WHITESPACE
    WORKING_DIRECTORY ${source_root_directory}
    RESULT_VARIABLE _rc)
  if(_rc)
    message(FATAL_ERROR "Please install latest version of git (`status --untracked-files=no --porcelain` failed)")
  endif()
  if(NOT "${_git_status}" STREQUAL "")
    set(_git_commit "DIRTY-${_git_commit}")
    set(_git_is_dirty TRUE)
  endif()
  unset(_git_status)

  execute_process(
    COMMAND ${GIT} describe --tags --abbrev=0 "--match=v[0-9]*"
    OUTPUT_VARIABLE _git_last_vtag
    OUTPUT_STRIP_TRAILING_WHITESPACE
    WORKING_DIRECTORY ${source_root_directory}
    RESULT_VARIABLE _rc)
  if(_rc OR "${_git_last_vtag}" STREQUAL "")
    execute_process(
      COMMAND ${GIT} tag
      OUTPUT_VARIABLE _git_tags_dump
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE _rc)
    execute_process(
      COMMAND ${GIT} rev-list --count --no-merges --remove-empty HEAD
      OUTPUT_VARIABLE _git_whole_count
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE _rc)
    if(_rc)
      message(
        FATAL_ERROR
          "Please install latest version of git (`git rev-list --count --no-merges --remove-empty HEAD` failed)")
    endif()
    if(_git_whole_count GREATER 42 AND "${_git_tags_dump}" STREQUAL "")
      message(FATAL_ERROR "Please fetch tags (`describe --tags --abbrev=0 --match=v[0-9]*` failed)")
    else()
      message(NOTICE "Falling back to version `0.0.0` (have you made an initial release?")
    endif()
    set(_git_last_vtag "0.0.0")
    set(_git_trailing_commits ${_git_whole_count})
    execute_process(
      COMMAND ${GIT} describe --tags --dirty --long --always
      OUTPUT_VARIABLE _git_describe
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE _rc)
    if(_rc OR "${_git_describe}" STREQUAL "")
      execute_process(
        COMMAND ${GIT} describe --tags --all --dirty --long --always
        OUTPUT_VARIABLE _git_describe
        OUTPUT_STRIP_TRAILING_WHITESPACE
        WORKING_DIRECTORY ${source_root_directory}
        RESULT_VARIABLE _rc)
      if(_rc OR "${_git_describe}" STREQUAL "")
        message(FATAL_ERROR "Please install latest version of git (`describe --tags --all --long` failed)")
      endif()
    endif()
  else()
    execute_process(
      COMMAND ${GIT} describe --tags --dirty --long "--match=v[0-9]*"
      OUTPUT_VARIABLE _git_describe
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE _rc)
    if(_rc OR "${_git_describe}" STREQUAL "")
      message(FATAL_ERROR "Please install latest version of git (`describe --tags --long --match=v[0-9]*`)")
    endif()
    execute_process(
      COMMAND ${GIT} rev-list --count "${_git_last_vtag}..HEAD"
      OUTPUT_VARIABLE _git_trailing_commits
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE _rc)
    if(_rc OR "${_git_trailing_commits}" STREQUAL "")
      message(FATAL_ERROR "Please install latest version of git (`rev-list --count ${_git_last_vtag}..HEAD` failed)")
    endif()
  endif()
endmacro(git_get_versioninfo)

macro(semver_provide name source_root_directory build_directory_for_json_output build_metadata parent_scope)
  set(_semver "")
  set(_git_describe "")
  set(_git_timestamp "")
  set(_git_tree "")
  set(_git_commit "")
  set(_version_from "")
  set(_git_root FALSE)

  find_program(GIT git)
  if(GIT)
    execute_process(
      COMMAND ${GIT} rev-parse --show-toplevel
      OUTPUT_VARIABLE _git_root
      ERROR_VARIABLE _git_root_error
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE _rc)
    if(_rc OR "${_git_root}" STREQUAL "")
      if(EXISTS "${source_root_directory}/.git")
        message(ERROR "`git rev-parse --show-toplevel` failed '${_git_root_error}'")
      else()
        message(VERBOSE "`git rev-parse --show-toplevel` failed '${_git_root_error}'")
      endif()
    else()
      set(_source_root "${source_root_directory}")
      if(NOT CMAKE_VERSION VERSION_LESS 3.19)
        file(REAL_PATH "${_git_root}" _git_root)
        file(REAL_PATH "${_source_root}" _source_root)
      endif()
      if(_source_root STREQUAL _git_root AND EXISTS "${_git_root}/VERSION.json")
        message(
          FATAL_ERROR
            "Несколько источников информации о версии, допустим только один из: репозиторий git, либо файл VERSION.json"
        )
      endif()
    endif()
  endif()

  if(EXISTS "${source_root_directory}/VERSION.json")
    set(_version_from "${source_root_directory}/VERSION.json")

    if(CMAKE_VERSION VERSION_LESS 3.19)
      message(FATAL_ERROR "Требуется CMake версии >= 3.19 для чтения VERSION.json")
    endif()
    file(
      STRINGS "${_version_from}" _versioninfo_json NEWLINE_CONSUME
      LIMIT_COUNT 9
      LIMIT_INPUT 999
      ENCODING UTF-8)
    string(JSON _git_describe GET ${_versioninfo_json} git_describe)
    string(JSON _git_timestamp GET "${_versioninfo_json}" "git_timestamp")
    string(JSON _git_tree GET "${_versioninfo_json}" "git_tree")
    string(JSON _git_commit GET "${_versioninfo_json}" "git_commit")
    string(JSON _semver GET "${_versioninfo_json}" "semver")
    unset(_json_object)
    if(NOT _semver)
      message(FATAL_ERROR "Unable to retrieve ${name} version from \"${_version_from}\" file.")
    endif()
    semver_parse("${_semver}")
    if(NOT _semver_ok)
      message(FATAL_ERROR "SemVer `${_semver}` from ${_version_from}: ${_semver_err}")
    endif()
  elseif(_git_root AND _source_root STREQUAL _git_root)
    set(_version_from git)
    git_get_versioninfo(${source_root_directory})
    semver_parse(${_git_last_vtag})
    if(NOT _semver_ok)
      message(FATAL_ERROR "Git tag `${_git_last_vtag}`: ${_semver_err}")
    endif()
    if(_git_trailing_commits GREATER 0 AND "${_semver_tweak}" STREQUAL "")
      set(_semver_tweak ${_git_trailing_commits})
    endif()

  elseif(GIT)
    message(
      FATAL_ERROR
        "Нет источника информации о версии (${source_root_directory}), требуется один из: репозиторий git, либо VERSION.json"
    )
  else()
    message(FATAL_ERROR "Требуется git для получения информации о версии")
  endif()

  if(NOT _git_describe
     OR NOT _git_timestamp
     OR NOT _git_tree
     OR NOT _git_commit
     OR "${_semver_major}" STREQUAL ""
     OR "${_semver_minor}" STREQUAL ""
     OR "${_semver_patch}" STREQUAL "")
    message(ERROR "Unable to retrieve ${name} version from ${_version_from}.")
  endif()

  set(_semver "${_semver_major}.${_semver_minor}.${_semver_patch}")
  if("${_semver_tweak}" STREQUAL "")
    set(_semver_tweak 0)
  elseif(_semver_tweak GREATER 0)
    string(APPEND _semver ".${_semver_tweak}")
  endif()
  if(NOT "${_semver_prerelease}" STREQUAL "")
    string(APPEND _semver "-${_semver_prerelease}")
  endif()
  if(_git_is_dirty)
    string(APPEND _semver "-DIRTY")
  endif()

  set(_semver_complete "${_semver}")
  if(NOT "${build_metadata}" STREQUAL "")
    string(APPEND _semver_complete "+${build_metadata}")
  endif()

  set(${name}_VERSION "${_semver_complete}")
  set(${name}_VERSION_PURE "${_semver}")
  set(${name}_VERSION_MAJOR ${_semver_major})
  set(${name}_VERSION_MINOR ${_semver_minor})
  set(${name}_VERSION_PATCH ${_semver_patch})
  set(${name}_VERSION_TWEAK ${_semver_tweak})
  set(${name}_VERSION_PRERELEASE "${_semver_prerelease}")
  set(${name}_GIT_DESCRIBE "${_git_describe}")
  set(${name}_GIT_TIMESTAMP "${_git_timestamp}")
  set(${name}_GIT_TREE "${_git_tree}")
  set(${name}_GIT_COMMIT "${_git_commit}")

  if(${parent_scope})
    set(${name}_VERSION
        "${_semver_complete}"
        PARENT_SCOPE)
    set(${name}_VERSION_PURE
        "${_semver}"
        PARENT_SCOPE)
    set(${name}_VERSION_MAJOR
        ${_semver_major}
        PARENT_SCOPE)
    set(${name}_VERSION_MINOR
        ${_semver_minor}
        PARENT_SCOPE)
    set(${name}_VERSION_PATCH
        ${_semver_patch}
        PARENT_SCOPE)
    set(${name}_VERSION_TWEAK
        "${_semver_tweak}"
        PARENT_SCOPE)
    set(${name}_VERSION_PRERELEASE
        "${_semver_prerelease}"
        PARENT_SCOPE)
    set(${name}_GIT_DESCRIBE
        "${_git_describe}"
        PARENT_SCOPE)
    set(${name}_GIT_TIMESTAMP
        "${_git_timestamp}"
        PARENT_SCOPE)
    set(${name}_GIT_TREE
        "${_git_tree}"
        PARENT_SCOPE)
    set(${name}_GIT_COMMIT
        "${_git_commit}"
        PARENT_SCOPE)
  endif()

  if(_version_from STREQUAL "git")
    string(
      CONFIGURE
        "{
      \"git_describe\" : \"@_git_describe@\",
      \"git_timestamp\" : \"@_git_timestamp@\",
      \"git_tree\" : \"@_git_tree@\",
      \"git_commit\" : \"@_git_commit@\",
      \"semver\" : \"@_semver@\"\n}"
        _versioninfo_json
      @ONLY ESCAPE_QUOTES)
    file(WRITE "${build_directory_for_json_output}/VERSION.json" "${_versioninfo_json}")
  endif()
endmacro(semver_provide)

cmake_policy(POP)
