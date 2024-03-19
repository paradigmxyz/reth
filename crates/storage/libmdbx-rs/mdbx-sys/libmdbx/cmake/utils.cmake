##  Copyright (c) 2012-2024 Leonid Yuriev <leo@yuriev.ru>.
##
##  Licensed under the Apache License, Version 2.0 (the "License");
##  you may not use this file except in compliance with the License.
##  You may obtain a copy of the License at
##
##      http://www.apache.org/licenses/LICENSE-2.0
##
##  Unless required by applicable law or agreed to in writing, software
##  distributed under the License is distributed on an "AS IS" BASIS,
##  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
##  See the License for the specific language governing permissions and
##  limitations under the License.
##

if(CMAKE_VERSION VERSION_LESS 3.8.2)
  cmake_minimum_required(VERSION 3.0.2)
elseif(CMAKE_VERSION VERSION_LESS 3.12)
  cmake_minimum_required(VERSION 3.8.2)
else()
  cmake_minimum_required(VERSION 3.12)
endif()

cmake_policy(PUSH)
cmake_policy(VERSION ${CMAKE_MINIMUM_REQUIRED_VERSION})

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
      # CMake believes that Objective C is a flavor of C++, not C,
      # and uses g++ compiler for .m files.
      # LANGUAGE property forces CMake to use CC for ${file}
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
      set_source_files_properties(${file} PROPERTIES COMPILE_FLAGS
        "${_flags}")
    endif()
  endforeach()
  unset(_file_ext)
  unset(_lang)
endmacro(set_source_files_compile_flags)

macro(fetch_version name source_root_directory parent_scope)
  set(${name}_VERSION "")
  set(${name}_GIT_DESCRIBE "")
  set(${name}_GIT_TIMESTAMP "")
  set(${name}_GIT_TREE "")
  set(${name}_GIT_COMMIT "")
  set(${name}_GIT_REVISION 0)
  set(${name}_GIT_VERSION "")
  if(GIT AND EXISTS "${source_root_directory}/.git")
    execute_process(COMMAND ${GIT} show --no-patch --format=%cI HEAD
      OUTPUT_VARIABLE ${name}_GIT_TIMESTAMP
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE rc)
    if(rc OR "${name}_GIT_TIMESTAMP" STREQUAL "%cI")
      execute_process(COMMAND ${GIT} show --no-patch --format=%ci HEAD
        OUTPUT_VARIABLE ${name}_GIT_TIMESTAMP
        OUTPUT_STRIP_TRAILING_WHITESPACE
        WORKING_DIRECTORY ${source_root_directory}
        RESULT_VARIABLE rc)
      if(rc OR "${name}_GIT_TIMESTAMP" STREQUAL "%ci")
        message(FATAL_ERROR "Please install latest version of git (`show --no-patch --format=%cI HEAD` failed)")
      endif()
    endif()

    execute_process(COMMAND ${GIT} show --no-patch --format=%T HEAD
      OUTPUT_VARIABLE ${name}_GIT_TREE
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE rc)
    if(rc OR "${name}_GIT_TREE" STREQUAL "")
      message(FATAL_ERROR "Please install latest version of git (`show --no-patch --format=%T HEAD` failed)")
    endif()

    execute_process(COMMAND ${GIT} show --no-patch --format=%H HEAD
      OUTPUT_VARIABLE ${name}_GIT_COMMIT
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE rc)
    if(rc OR "${name}_GIT_COMMIT" STREQUAL "")
      message(FATAL_ERROR "Please install latest version of git (`show --no-patch --format=%H HEAD` failed)")
    endif()

    execute_process(COMMAND ${GIT} rev-list --tags --count
      OUTPUT_VARIABLE tag_count
      OUTPUT_STRIP_TRAILING_WHITESPACE
      WORKING_DIRECTORY ${source_root_directory}
      RESULT_VARIABLE rc)
    if(rc)
      message(FATAL_ERROR "Please install latest version of git (`git rev-list --tags --count` failed)")
    endif()

    if(tag_count EQUAL 0)
      execute_process(COMMAND ${GIT} rev-list --all --count
        OUTPUT_VARIABLE whole_count
        OUTPUT_STRIP_TRAILING_WHITESPACE
        WORKING_DIRECTORY ${source_root_directory}
        RESULT_VARIABLE rc)
      if(rc)
        message(FATAL_ERROR "Please install latest version of git (`git rev-list --all --count` failed)")
      endif()
      if(whole_count GREATER 42)
        message(FATAL_ERROR "Please fetch tags (no any tags for ${whole_count} commits)")
      endif()
      set(${name}_GIT_VERSION "0;0;0")
      execute_process(COMMAND ${GIT} rev-list --count --all --no-merges
        OUTPUT_VARIABLE ${name}_GIT_REVISION
        OUTPUT_STRIP_TRAILING_WHITESPACE
        WORKING_DIRECTORY ${source_root_directory}
        RESULT_VARIABLE rc)
      if(rc OR "${name}_GIT_REVISION" STREQUAL "")
        message(FATAL_ERROR "Please install latest version of git (`rev-list --count --all --no-merges` failed)")
      endif()
    else(tag_count EQUAL 0)
      execute_process(COMMAND ${GIT} describe --tags --long --dirty=-dirty "--match=v[0-9]*"
        OUTPUT_VARIABLE ${name}_GIT_DESCRIBE
        OUTPUT_STRIP_TRAILING_WHITESPACE
        WORKING_DIRECTORY ${source_root_directory}
        RESULT_VARIABLE rc)
      if(rc OR "${name}_GIT_DESCRIBE" STREQUAL "")
        if(_whole_count GREATER 42)
          message(FATAL_ERROR "Please fetch tags (`describe --tags --long --dirty --match=v[0-9]*` failed)")
        else()
          execute_process(COMMAND ${GIT} describe --all --long --dirty=-dirty
            OUTPUT_VARIABLE ${name}_GIT_DESCRIBE
            OUTPUT_STRIP_TRAILING_WHITESPACE
            WORKING_DIRECTORY ${source_root_directory}
            RESULT_VARIABLE rc)
          if(rc OR "${name}_GIT_DESCRIBE" STREQUAL "")
            message(FATAL_ERROR "Please install latest version of git (`git rev-list --tags --count` and/or `git rev-list --all --count` failed)")
          endif()
        endif()
      endif()

      execute_process(COMMAND ${GIT} describe --tags --abbrev=0 "--match=v[0-9]*"
        OUTPUT_VARIABLE last_release_tag
        OUTPUT_STRIP_TRAILING_WHITESPACE
        WORKING_DIRECTORY ${source_root_directory}
        RESULT_VARIABLE rc)
      if(rc)
        message(FATAL_ERROR "Please install latest version of git (`describe --tags --abbrev=0 --match=v[0-9]*` failed)")
      endif()
      if (last_release_tag)
        set(git_revlist_arg "${last_release_tag}..HEAD")
      else()
        execute_process(COMMAND ${GIT} tag --sort=-version:refname
          OUTPUT_VARIABLE tag_list
          OUTPUT_STRIP_TRAILING_WHITESPACE
          WORKING_DIRECTORY ${source_root_directory}
          RESULT_VARIABLE rc)
        if(rc)
          message(FATAL_ERROR "Please install latest version of git (`tag --sort=-version:refname` failed)")
        endif()
        string(REGEX REPLACE "\n" ";" tag_list "${tag_list}")
        set(git_revlist_arg "HEAD")
        foreach(tag IN LISTS tag_list)
          if(NOT last_release_tag)
            string(REGEX MATCH "^v[0-9]+(\.[0-9]+)+" last_release_tag "${tag}")
            set(git_revlist_arg "${tag}..HEAD")
          endif()
        endforeach(tag)
      endif()
      execute_process(COMMAND ${GIT} rev-list --count "${git_revlist_arg}"
        OUTPUT_VARIABLE ${name}_GIT_REVISION
        OUTPUT_STRIP_TRAILING_WHITESPACE
        WORKING_DIRECTORY ${source_root_directory}
        RESULT_VARIABLE rc)
      if(rc OR "${name}_GIT_REVISION" STREQUAL "")
        message(FATAL_ERROR "Please install latest version of git (`rev-list --count ${git_revlist_arg}` failed)")
      endif()

      string(REGEX MATCH "^(v)?([0-9]+)\\.([0-9]+)\\.([0-9]+)(.*)?" git_version_valid "${${name}_GIT_DESCRIBE}")
      if(git_version_valid)
        string(REGEX REPLACE "^(v)?([0-9]+)\\.([0-9]+)\\.([0-9]+)(.*)?" "\\2;\\3;\\4" ${name}_GIT_VERSION ${${name}_GIT_DESCRIBE})
      else()
        string(REGEX MATCH "^(v)?([0-9]+)\\.([0-9]+)(.*)?" git_version_valid "${${name}_GIT_DESCRIBE}")
        if(git_version_valid)
          string(REGEX REPLACE "^(v)?([0-9]+)\\.([0-9]+)(.*)?" "\\2;\\3;0" ${name}_GIT_VERSION ${${name}_GIT_DESCRIBE})
        else()
          message(AUTHOR_WARNING "Bad ${name} version \"${${name}_GIT_DESCRIBE}\"; falling back to 0.0.0 (have you made an initial release?)")
          set(${name}_GIT_VERSION "0;0;0")
        endif()
      endif()
    endif(tag_count EQUAL 0)
  endif()

  if(NOT ${name}_GIT_VERSION OR NOT ${name}_GIT_TIMESTAMP OR ${name}_GIT_REVISION STREQUAL "")
    if(GIT AND EXISTS "${source_root_directory}/.git")
      message(WARNING "Unable to retrieve ${name} version from git.")
    endif()
    set(${name}_GIT_VERSION "0;0;0;0")
    set(${name}_GIT_TIMESTAMP "")
    set(${name}_GIT_REVISION 0)

    # Try to get version from VERSION file
    set(version_file "${source_root_directory}/VERSION.txt")
    if(NOT EXISTS "${version_file}")
      set(version_file "${source_root_directory}/VERSION")
    endif()
    if(EXISTS "${version_file}")
      file(STRINGS "${version_file}" ${name}_VERSION LIMIT_COUNT 1 LIMIT_INPUT 42)
    endif()

    if(NOT ${name}_VERSION)
      message(WARNING "Unable to retrieve ${name} version from \"${version_file}\" file.")
      set(${name}_VERSION_LIST ${${name}_GIT_VERSION})
      string(REPLACE ";" "." ${name}_VERSION "${${name}_GIT_VERSION}")
    else()
      string(REPLACE "." ";" ${name}_VERSION_LIST ${${name}_VERSION})
    endif()

  else()
    list(APPEND ${name}_GIT_VERSION ${${name}_GIT_REVISION})
    set(${name}_VERSION_LIST ${${name}_GIT_VERSION})
    string(REPLACE ";" "." ${name}_VERSION "${${name}_GIT_VERSION}")
  endif()

  list(GET ${name}_VERSION_LIST 0 "${name}_VERSION_MAJOR")
  list(GET ${name}_VERSION_LIST 1 "${name}_VERSION_MINOR")
  list(GET ${name}_VERSION_LIST 2 "${name}_VERSION_RELEASE")
  list(GET ${name}_VERSION_LIST 3 "${name}_VERSION_REVISION")

  if(${parent_scope})
    set(${name}_VERSION_MAJOR "${${name}_VERSION_MAJOR}" PARENT_SCOPE)
    set(${name}_VERSION_MINOR "${${name}_VERSION_MINOR}" PARENT_SCOPE)
    set(${name}_VERSION_RELEASE "${${name}_VERSION_RELEASE}" PARENT_SCOPE)
    set(${name}_VERSION_REVISION "${${name}_VERSION_REVISION}" PARENT_SCOPE)
    set(${name}_VERSION "${${name}_VERSION}" PARENT_SCOPE)

    set(${name}_GIT_DESCRIBE "${${name}_GIT_DESCRIBE}" PARENT_SCOPE)
    set(${name}_GIT_TIMESTAMP "${${name}_GIT_TIMESTAMP}" PARENT_SCOPE)
    set(${name}_GIT_TREE "${${name}_GIT_TREE}" PARENT_SCOPE)
    set(${name}_GIT_COMMIT "${${name}_GIT_COMMIT}" PARENT_SCOPE)
    set(${name}_GIT_REVISION "${${name}_GIT_REVISION}" PARENT_SCOPE)
    set(${name}_GIT_VERSION "${${name}_GIT_VERSION}" PARENT_SCOPE)
  endif()
endmacro(fetch_version)

cmake_policy(POP)
