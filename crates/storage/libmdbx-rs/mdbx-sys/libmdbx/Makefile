# This is thunk-Makefile for calling GNU Make 3.80 or above

all help options cmake-build ninja \
clean install install-no-strip install-strip strip tools uninstall \
bench bench-clean bench-couple bench-quartet bench-triplet re-bench \
lib libs lib-static lib-shared tools-static \
libmdbx mdbx mdbx_chk mdbx_copy mdbx_drop mdbx_dump mdbx_load mdbx_stat \
check dist memcheck cross-gcc cross-qemu doxygen gcc-analyzer reformat \
release-assets tags build-test mdbx_test \
smoke smoke-fault smoke-singleprocess smoke-assertion smoke-memcheck \
test test-assertion test-long test-long-assertion test-ci test-ci-extra \
test-asan test-leak test-singleprocess test-ubsan test-memcheck:
	@CC=$(CC) \
	CXX=`if test -n "$(CXX)" && which "$(CXX)" > /dev/null; then echo "$(CXX)"; elif test -n "$(CCC)" && which "$(CCC)" > /dev/null; then echo "$(CCC)"; else echo "c++"; fi` \
	`which gmake || which gnumake || echo 'echo "GNU Make 3.80 or above is required"; exit 2;'` \
		$(MAKEFLAGS) -f GNUmakefile $@
