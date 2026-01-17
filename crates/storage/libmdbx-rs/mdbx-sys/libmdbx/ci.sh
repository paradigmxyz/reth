#!/usr/bin/env bash

function failure() {
	echo "Oops, $* failed ;(" >&2
	exit 2
}

export ci_script_recursion="$((++ci_script_recursion))"
if [ $ci_script_recursion -gt 3 ]; then
     failure "WTF: ci_script_recursion = $ci_script_recursion ?"
fi

IFS='|' read -r -a config_args <<< "${1:-}"
IFS='|' read -r -a build_args <<< "${2:-}"
IFS='|' read -r -a test_args <<< "${3:-}"
set -euxo pipefail

function provide_toolchain {
	set +ex
	export CC="$((which ${CC:-cc} || which gcc || which clang || which true) 2>/dev/null)"
	export CXX="$((which ${CXX:-c++} || which g++ || which clang++ || which true) 2>/dev/null)"
	echo "CC: ${CC} => $($CC --version | head -1)"
	echo "CXX: ${CXX} => $($CXX --version | head -1)"
	CMAKE="$(which cmake 2>/dev/null)"
	if [ -z "${CMAKE}" -o -z "$(which ninja 2>/dev/null)" ]; then
		SUDO=$(which sudo 2>&-)
		if [ -n "$(which apt 2>/dev/null)" ]; then
			${SUDO} apt update && sudo apt install -y cmake ninja-build libgtest-dev
		elif [ -n "$(which dnf 2>/dev/null)" ]; then
			${SUDO} dnf install -y cmake ninja-build gtest-devel
		elif [ -n "$(which yum 2>/dev/null)" ]; then
			${SUDO} yum install -y cmake ninja-build gtest-devel
		fi
		CMAKE="$(which cmake 2>/dev/null) | echo false"
	fi
	CMAKE_VERSION=$(eval expr $("${CMAKE}" --version | sed -n 's/cmake version \([0-9]\{1,\}\)\.\([0-9]\{1,\}\)\.\([0-9]\{1,\}\)/\10000 + \200 + \3/p' || echo '00000'))
	echo "CMAKE: ${CMAKE} => $(""${CMAKE}"" --version | head -1) ($CMAKE_VERSION)"
	set -euxo pipefail
}

function default_test {
	GTEST_SHUFFLE=1 GTEST_RUNTIME_LIMIT=99 MALLOC_CHECK_=7 MALLOC_PERTURB_=42 \
	ctest --output-on-failure --parallel 3 --schedule-random --no-tests=error \
	"${test_args[@]+"${test_args[@]}"}"
}

function default_build {
	local cmake_use_ninja=""
	if "${CMAKE}" --help | grep -iq ninja && [ -n "$(which ninja 2>/dev/null)" ] && echo " ${config_args[@]+"${config_args[@]}"}" | grep -qv -e ' -[GTA] '; then
		echo "NINJA: $(which ninja 2>/dev/null) => $(ninja --version | head -1)"
		cmake_use_ninja="-G Ninja"
	fi
	"${CMAKE}" ${cmake_use_ninja} "${config_args[@]+"${config_args[@]}"}" .. && "${CMAKE}" --build . --verbose "${build_args[@]+"${build_args[@]}"}"
}

function default_ci {
	provide_toolchain
	local skipped=true
	local ok=true
	if [ -e CMakeLists.txt -a $CMAKE_VERSION -ge 30002 ]; then
		skipped=false
		mkdir @ci-cmake-build && (cd @ci-cmake-build && default_build && default_test && echo "Done (cmake)") || ok=false
	fi
	if [ -n "$CC" -a -n "${CI_MAKE_TARGET=test}" ] && [ -e GNUmakefile -o -e Makefile -o -e makefile ]; then
		skipped=false
		make -j2 all && make ${CI_MAKE_TARGET} && echo "Done (make)" || ok=false
	fi
	if [ $skipped = "true" ]; then
		echo "Skipped since CMAKE_VERSION ($CMAKE_VERSION) < 3.0.2 and no Makefile"
	elif [ $ok != "true" ]; then
		exit 1
	fi
}

git clean -x -f -d || echo "ignore 'git clean' error"
git describe --tags || git show --oneline -s

if [ -z "${CI_ACTION:-}" ]; then
	CI_ACTION=default_ci
fi

$CI_ACTION || failure $CI_ACTION
