/* mdbx_drop.c - memory-mapped database delete tool */

/*
 * Copyright 2021-2023 Leonid Yuriev <leo@yuriev.ru>
 * and other libmdbx authors: please see AUTHORS file.
 *
 * Copyright 2016-2021 Howard Chu, Symas Corp.
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted only as authorized by the OpenLDAP
 * Public License.
 *
 * A copy of this license is available in the file LICENSE in the
 * top-level directory of the distribution or, alternatively, at
 * <http://www.OpenLDAP.org/license.html>. */

#ifdef _MSC_VER
#if _MSC_VER > 1800
#pragma warning(disable : 4464) /* relative include path contains '..' */
#endif
#pragma warning(disable : 4996) /* The POSIX name is deprecated... */
#endif                          /* _MSC_VER (warnings) */

#define xMDBX_TOOLS /* Avoid using internal eASSERT() */
/*
 * Copyright 2015-2023 Leonid Yuriev <leo@yuriev.ru>
 * and other libmdbx authors: please see AUTHORS file.
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted only as authorized by the OpenLDAP
 * Public License.
 *
 * A copy of this license is available in the file LICENSE in the
 * top-level directory of the distribution or, alternatively, at
 * <http://www.OpenLDAP.org/license.html>. */

#define MDBX_BUILD_SOURCERY 30c8f70db1f021dc2bfb201ba04efdcc34fc7495127f517f9624f18c0100b8ab_v0_12_8_0_g02c7cf2a
#ifdef MDBX_CONFIG_H
#include MDBX_CONFIG_H
#endif

#define LIBMDBX_INTERNALS
#ifdef xMDBX_TOOLS
#define MDBX_DEPRECATED
#endif /* xMDBX_TOOLS */

#ifdef xMDBX_ALLOY
/* Amalgamated build */
#define MDBX_INTERNAL_FUNC static
#define MDBX_INTERNAL_VAR static
#else
/* Non-amalgamated build */
#define MDBX_INTERNAL_FUNC
#define MDBX_INTERNAL_VAR extern
#endif /* xMDBX_ALLOY */

/*----------------------------------------------------------------------------*/

/** Disables using GNU/Linux libc extensions.
 * \ingroup build_option
 * \note This option couldn't be moved to the options.h since dependent
 * control macros/defined should be prepared before include the options.h */
#ifndef MDBX_DISABLE_GNU_SOURCE
#define MDBX_DISABLE_GNU_SOURCE 0
#endif
#if MDBX_DISABLE_GNU_SOURCE
#undef _GNU_SOURCE
#elif (defined(__linux__) || defined(__gnu_linux__)) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE
#endif /* MDBX_DISABLE_GNU_SOURCE */

/* Should be defined before any includes */
#if !defined(_FILE_OFFSET_BITS) && !defined(__ANDROID_API__) &&                \
    !defined(ANDROID)
#define _FILE_OFFSET_BITS 64
#endif

#ifdef __APPLE__
#define _DARWIN_C_SOURCE
#endif

#ifdef _MSC_VER
#if _MSC_FULL_VER < 190024234
/* Actually libmdbx was not tested with compilers older than 19.00.24234 (Visual
 * Studio 2015 Update 3). But you could remove this #error and try to continue
 * at your own risk. In such case please don't rise up an issues related ONLY to
 * old compilers.
 *
 * NOTE:
 *   Unfortunately, there are several different builds of "Visual Studio" that
 *   are called "Visual Studio 2015 Update 3".
 *
 *   The 190024234 is used here because it is minimal version of Visual Studio
 *   that was used for build and testing libmdbx in recent years. Soon this
 *   value will be increased to 19.0.24241.7, since build and testing using
 *   "Visual Studio 2015" will be performed only at https://ci.appveyor.com.
 *
 *   Please ask Microsoft (but not us) for information about version differences
 *   and how to and where you can obtain the latest "Visual Studio 2015" build
 *   with all fixes.
 */
#error                                                                         \
    "At least \"Microsoft C/C++ Compiler\" version 19.00.24234 (Visual Studio 2015 Update 3) is required."
#endif
#ifndef _CRT_SECURE_NO_WARNINGS
#define _CRT_SECURE_NO_WARNINGS
#endif /* _CRT_SECURE_NO_WARNINGS */
#if _MSC_VER > 1800
#pragma warning(disable : 4464) /* relative include path contains '..' */
#endif
#if _MSC_VER > 1913
#pragma warning(disable : 5045) /* will insert Spectre mitigation... */
#endif
#if _MSC_VER > 1914
#pragma warning(                                                               \
        disable : 5105) /* winbase.h(9531): warning C5105: macro expansion     \
                           producing 'defined' has undefined behavior */
#endif
#if _MSC_VER > 1930
#pragma warning(disable : 6235) /* <expression> is always a constant */
#pragma warning(disable : 6237) /* <expression> is never evaluated and might   \
                                   have side effects */
#endif
#pragma warning(disable : 4710) /* 'xyz': function not inlined */
#pragma warning(disable : 4711) /* function 'xyz' selected for automatic       \
                                   inline expansion */
#pragma warning(disable : 4201) /* nonstandard extension used: nameless        \
                                   struct/union */
#pragma warning(disable : 4702) /* unreachable code */
#pragma warning(disable : 4706) /* assignment within conditional expression */
#pragma warning(disable : 4127) /* conditional expression is constant */
#pragma warning(disable : 4324) /* 'xyz': structure was padded due to          \
                                   alignment specifier */
#pragma warning(disable : 4310) /* cast truncates constant value */
#pragma warning(disable : 4820) /* bytes padding added after data member for   \
                                   alignment */
#pragma warning(disable : 4548) /* expression before comma has no effect;      \
                                   expected expression with side - effect */
#pragma warning(disable : 4366) /* the result of the unary '&' operator may be \
                                   unaligned */
#pragma warning(disable : 4200) /* nonstandard extension used: zero-sized      \
                                   array in struct/union */
#pragma warning(disable : 4204) /* nonstandard extension used: non-constant    \
                                   aggregate initializer */
#pragma warning(                                                               \
        disable : 4505) /* unreferenced local function has been removed */
#endif                  /* _MSC_VER (warnings) */

#if defined(__GNUC__) && __GNUC__ < 9
#pragma GCC diagnostic ignored "-Wattributes"
#endif /* GCC < 9 */

#if (defined(__MINGW__) || defined(__MINGW32__) || defined(__MINGW64__)) &&    \
    !defined(__USE_MINGW_ANSI_STDIO)
#define __USE_MINGW_ANSI_STDIO 1
#endif /* MinGW */

#if (defined(_WIN32) || defined(_WIN64)) && !defined(UNICODE)
#define UNICODE
#endif /* UNICODE */

#include "mdbx.h"
/*
 * Copyright 2015-2023 Leonid Yuriev <leo@yuriev.ru>
 * and other libmdbx authors: please see AUTHORS file.
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted only as authorized by the OpenLDAP
 * Public License.
 *
 * A copy of this license is available in the file LICENSE in the
 * top-level directory of the distribution or, alternatively, at
 * <http://www.OpenLDAP.org/license.html>.
 */


/*----------------------------------------------------------------------------*/
/* Microsoft compiler generates a lot of warning for self includes... */

#ifdef _MSC_VER
#pragma warning(push, 1)
#pragma warning(disable : 4548) /* expression before comma has no effect;      \
                                   expected expression with side - effect */
#pragma warning(disable : 4530) /* C++ exception handler used, but unwind      \
                                 * semantics are not enabled. Specify /EHsc */
#pragma warning(disable : 4577) /* 'noexcept' used with no exception handling  \
                                 * mode specified; termination on exception is \
                                 * not guaranteed. Specify /EHsc */
#endif                          /* _MSC_VER (warnings) */

#if defined(_WIN32) || defined(_WIN64)
#if !defined(_CRT_SECURE_NO_WARNINGS)
#define _CRT_SECURE_NO_WARNINGS
#endif /* _CRT_SECURE_NO_WARNINGS */
#if !defined(_NO_CRT_STDIO_INLINE) && MDBX_BUILD_SHARED_LIBRARY &&             \
    !defined(xMDBX_TOOLS) && MDBX_WITHOUT_MSVC_CRT
#define _NO_CRT_STDIO_INLINE
#endif
#elif !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif /* Windows */

/*----------------------------------------------------------------------------*/
/* basic C99 includes */
#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

#include <assert.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

#if (-6 & 5) || CHAR_BIT != 8 || UINT_MAX < 0xffffffff || ULONG_MAX % 0xFFFF
#error                                                                         \
    "Sanity checking failed: Two's complement, reasonably sized integer types"
#endif

#ifndef SSIZE_MAX
#define SSIZE_MAX INTPTR_MAX
#endif

#if UINTPTR_MAX > 0xffffFFFFul || ULONG_MAX > 0xffffFFFFul || defined(_WIN64)
#define MDBX_WORDBITS 64
#else
#define MDBX_WORDBITS 32
#endif /* MDBX_WORDBITS */

/*----------------------------------------------------------------------------*/
/* feature testing */

#ifndef __has_warning
#define __has_warning(x) (0)
#endif

#ifndef __has_include
#define __has_include(x) (0)
#endif

#ifndef __has_feature
#define __has_feature(x) (0)
#endif

#ifndef __has_extension
#define __has_extension(x) (0)
#endif

#if __has_feature(thread_sanitizer)
#define __SANITIZE_THREAD__ 1
#endif

#if __has_feature(address_sanitizer)
#define __SANITIZE_ADDRESS__ 1
#endif

#ifndef __GNUC_PREREQ
#if defined(__GNUC__) && defined(__GNUC_MINOR__)
#define __GNUC_PREREQ(maj, min)                                                \
  ((__GNUC__ << 16) + __GNUC_MINOR__ >= ((maj) << 16) + (min))
#else
#define __GNUC_PREREQ(maj, min) (0)
#endif
#endif /* __GNUC_PREREQ */

#ifndef __CLANG_PREREQ
#ifdef __clang__
#define __CLANG_PREREQ(maj, min)                                               \
  ((__clang_major__ << 16) + __clang_minor__ >= ((maj) << 16) + (min))
#else
#define __CLANG_PREREQ(maj, min) (0)
#endif
#endif /* __CLANG_PREREQ */

#ifndef __GLIBC_PREREQ
#if defined(__GLIBC__) && defined(__GLIBC_MINOR__)
#define __GLIBC_PREREQ(maj, min)                                               \
  ((__GLIBC__ << 16) + __GLIBC_MINOR__ >= ((maj) << 16) + (min))
#else
#define __GLIBC_PREREQ(maj, min) (0)
#endif
#endif /* __GLIBC_PREREQ */

/*----------------------------------------------------------------------------*/
/* C11' alignas() */

#if __has_include(<stdalign.h>)
#include <stdalign.h>
#endif
#if defined(alignas) || defined(__cplusplus)
#define MDBX_ALIGNAS(N) alignas(N)
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
#define MDBX_ALIGNAS(N) _Alignas(N)
#elif defined(_MSC_VER)
#define MDBX_ALIGNAS(N) __declspec(align(N))
#elif __has_attribute(__aligned__) || defined(__GNUC__)
#define MDBX_ALIGNAS(N) __attribute__((__aligned__(N)))
#else
#error "FIXME: Required alignas() or equivalent."
#endif /* MDBX_ALIGNAS */

/*----------------------------------------------------------------------------*/
/* Systems macros and includes */

#ifndef __extern_C
#ifdef __cplusplus
#define __extern_C extern "C"
#else
#define __extern_C
#endif
#endif /* __extern_C */

#if !defined(nullptr) && !defined(__cplusplus) ||                              \
    (__cplusplus < 201103L && !defined(_MSC_VER))
#define nullptr NULL
#endif

#if defined(__APPLE__) || defined(_DARWIN_C_SOURCE)
#include <AvailabilityMacros.h>
#include <TargetConditionals.h>
#ifndef MAC_OS_X_VERSION_MIN_REQUIRED
#define MAC_OS_X_VERSION_MIN_REQUIRED 1070 /* Mac OS X 10.7, 2011 */
#endif
#endif /* Apple OSX & iOS */

#if defined(__FreeBSD__) || defined(__NetBSD__) || defined(__OpenBSD__) ||     \
    defined(__BSD__) || defined(__bsdi__) || defined(__DragonFly__) ||         \
    defined(__APPLE__) || defined(__MACH__)
#include <sys/cdefs.h>
#include <sys/mount.h>
#include <sys/sysctl.h>
#include <sys/types.h>
#if defined(__FreeBSD__) || defined(__DragonFly__)
#include <vm/vm_param.h>
#elif defined(__OpenBSD__) || defined(__NetBSD__)
#include <uvm/uvm_param.h>
#else
#define SYSCTL_LEGACY_NONCONST_MIB
#endif
#ifndef __MACH__
#include <sys/vmmeter.h>
#endif
#else
#include <malloc.h>
#if !(defined(__sun) || defined(__SVR4) || defined(__svr4__) ||                \
      defined(_WIN32) || defined(_WIN64))
#include <mntent.h>
#endif /* !Solaris */
#endif /* !xBSD */

#if defined(__FreeBSD__) || __has_include(<malloc_np.h>)
#include <malloc_np.h>
#endif

#if defined(__APPLE__) || defined(__MACH__) || __has_include(<malloc/malloc.h>)
#include <malloc/malloc.h>
#endif /* MacOS */

#if defined(__MACH__)
#include <mach/host_info.h>
#include <mach/mach_host.h>
#include <mach/mach_port.h>
#include <uuid/uuid.h>
#endif

#if defined(__linux__) || defined(__gnu_linux__)
#include <sched.h>
#include <sys/sendfile.h>
#include <sys/statfs.h>
#endif /* Linux */

#ifndef _XOPEN_SOURCE
#define _XOPEN_SOURCE 0
#endif

#ifndef _XOPEN_SOURCE_EXTENDED
#define _XOPEN_SOURCE_EXTENDED 0
#else
#include <utmpx.h>
#endif /* _XOPEN_SOURCE_EXTENDED */

#if defined(__sun) || defined(__SVR4) || defined(__svr4__)
#include <kstat.h>
#include <sys/mnttab.h>
/* On Solaris, it's easier to add a missing prototype rather than find a
 * combination of #defines that break nothing. */
__extern_C key_t ftok(const char *, int);
#endif /* SunOS/Solaris */

#if defined(_WIN32) || defined(_WIN64) /*-------------------------------------*/

#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0601 /* Windows 7 */
#elif _WIN32_WINNT < 0x0500
#error At least 'Windows 2000' API is required for libmdbx.
#endif /* _WIN32_WINNT */
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif /* WIN32_LEAN_AND_MEAN */
#include <excpt.h>
#include <tlhelp32.h>
#include <windows.h>
#include <winnt.h>
#include <winternl.h>

#else /*----------------------------------------------------------------------*/

#include <unistd.h>
#if !defined(_POSIX_MAPPED_FILES) || _POSIX_MAPPED_FILES < 1
#error "libmdbx requires the _POSIX_MAPPED_FILES feature"
#endif /* _POSIX_MAPPED_FILES */

#include <pthread.h>
#include <semaphore.h>
#include <signal.h>
#include <sys/file.h>
#include <sys/ipc.h>
#include <sys/mman.h>
#include <sys/param.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <sys/uio.h>

#endif /*---------------------------------------------------------------------*/

#if defined(__ANDROID_API__) || defined(ANDROID)
#include <android/log.h>
#if __ANDROID_API__ >= 21
#include <sys/sendfile.h>
#endif
#if defined(_FILE_OFFSET_BITS) && _FILE_OFFSET_BITS != MDBX_WORDBITS
#error "_FILE_OFFSET_BITS != MDBX_WORDBITS" (_FILE_OFFSET_BITS != MDBX_WORDBITS)
#elif defined(__FILE_OFFSET_BITS) && __FILE_OFFSET_BITS != MDBX_WORDBITS
#error "__FILE_OFFSET_BITS != MDBX_WORDBITS" (__FILE_OFFSET_BITS != MDBX_WORDBITS)
#endif
#endif /* Android */

#if defined(HAVE_SYS_STAT_H) || __has_include(<sys/stat.h>)
#include <sys/stat.h>
#endif
#if defined(HAVE_SYS_TYPES_H) || __has_include(<sys/types.h>)
#include <sys/types.h>
#endif
#if defined(HAVE_SYS_FILE_H) || __has_include(<sys/file.h>)
#include <sys/file.h>
#endif

/*----------------------------------------------------------------------------*/
/* Byteorder */

#if defined(i386) || defined(__386) || defined(__i386) || defined(__i386__) || \
    defined(i486) || defined(__i486) || defined(__i486__) || defined(i586) ||  \
    defined(__i586) || defined(__i586__) || defined(i686) ||                   \
    defined(__i686) || defined(__i686__) || defined(_M_IX86) ||                \
    defined(_X86_) || defined(__THW_INTEL__) || defined(__I86__) ||            \
    defined(__INTEL__) || defined(__x86_64) || defined(__x86_64__) ||          \
    defined(__amd64__) || defined(__amd64) || defined(_M_X64) ||               \
    defined(_M_AMD64) || defined(__IA32__) || defined(__INTEL__)
#ifndef __ia32__
/* LY: define neutral __ia32__ for x86 and x86-64 */
#define __ia32__ 1
#endif /* __ia32__ */
#if !defined(__amd64__) &&                                                     \
    (defined(__x86_64) || defined(__x86_64__) || defined(__amd64) ||           \
     defined(_M_X64) || defined(_M_AMD64))
/* LY: define trusty __amd64__ for all AMD64/x86-64 arch */
#define __amd64__ 1
#endif /* __amd64__ */
#endif /* all x86 */

#if !defined(__BYTE_ORDER__) || !defined(__ORDER_LITTLE_ENDIAN__) ||           \
    !defined(__ORDER_BIG_ENDIAN__)

#if defined(__GLIBC__) || defined(__GNU_LIBRARY__) ||                          \
    defined(__ANDROID_API__) || defined(HAVE_ENDIAN_H) || __has_include(<endian.h>)
#include <endian.h>
#elif defined(__APPLE__) || defined(__MACH__) || defined(__OpenBSD__) ||       \
    defined(HAVE_MACHINE_ENDIAN_H) || __has_include(<machine/endian.h>)
#include <machine/endian.h>
#elif defined(HAVE_SYS_ISA_DEFS_H) || __has_include(<sys/isa_defs.h>)
#include <sys/isa_defs.h>
#elif (defined(HAVE_SYS_TYPES_H) && defined(HAVE_SYS_ENDIAN_H)) ||             \
    (__has_include(<sys/types.h>) && __has_include(<sys/endian.h>))
#include <sys/endian.h>
#include <sys/types.h>
#elif defined(__bsdi__) || defined(__DragonFly__) || defined(__FreeBSD__) ||   \
    defined(__NetBSD__) || defined(HAVE_SYS_PARAM_H) || __has_include(<sys/param.h>)
#include <sys/param.h>
#endif /* OS */

#if defined(__BYTE_ORDER) && defined(__LITTLE_ENDIAN) && defined(__BIG_ENDIAN)
#define __ORDER_LITTLE_ENDIAN__ __LITTLE_ENDIAN
#define __ORDER_BIG_ENDIAN__ __BIG_ENDIAN
#define __BYTE_ORDER__ __BYTE_ORDER
#elif defined(_BYTE_ORDER) && defined(_LITTLE_ENDIAN) && defined(_BIG_ENDIAN)
#define __ORDER_LITTLE_ENDIAN__ _LITTLE_ENDIAN
#define __ORDER_BIG_ENDIAN__ _BIG_ENDIAN
#define __BYTE_ORDER__ _BYTE_ORDER
#else
#define __ORDER_LITTLE_ENDIAN__ 1234
#define __ORDER_BIG_ENDIAN__ 4321

#if defined(__LITTLE_ENDIAN__) ||                                              \
    (defined(_LITTLE_ENDIAN) && !defined(_BIG_ENDIAN)) ||                      \
    defined(__ARMEL__) || defined(__THUMBEL__) || defined(__AARCH64EL__) ||    \
    defined(__MIPSEL__) || defined(_MIPSEL) || defined(__MIPSEL) ||            \
    defined(_M_ARM) || defined(_M_ARM64) || defined(__e2k__) ||                \
    defined(__elbrus_4c__) || defined(__elbrus_8c__) || defined(__bfin__) ||   \
    defined(__BFIN__) || defined(__ia64__) || defined(_IA64) ||                \
    defined(__IA64__) || defined(__ia64) || defined(_M_IA64) ||                \
    defined(__itanium__) || defined(__ia32__) || defined(__CYGWIN__) ||        \
    defined(_WIN64) || defined(_WIN32) || defined(__TOS_WIN__) ||              \
    defined(__WINDOWS__)
#define __BYTE_ORDER__ __ORDER_LITTLE_ENDIAN__

#elif defined(__BIG_ENDIAN__) ||                                               \
    (defined(_BIG_ENDIAN) && !defined(_LITTLE_ENDIAN)) ||                      \
    defined(__ARMEB__) || defined(__THUMBEB__) || defined(__AARCH64EB__) ||    \
    defined(__MIPSEB__) || defined(_MIPSEB) || defined(__MIPSEB) ||            \
    defined(__m68k__) || defined(M68000) || defined(__hppa__) ||               \
    defined(__hppa) || defined(__HPPA__) || defined(__sparc__) ||              \
    defined(__sparc) || defined(__370__) || defined(__THW_370__) ||            \
    defined(__s390__) || defined(__s390x__) || defined(__SYSC_ZARCH__)
#define __BYTE_ORDER__ __ORDER_BIG_ENDIAN__

#else
#error __BYTE_ORDER__ should be defined.
#endif /* Arch */

#endif
#endif /* __BYTE_ORDER__ || __ORDER_LITTLE_ENDIAN__ || __ORDER_BIG_ENDIAN__ */

/*----------------------------------------------------------------------------*/
/* Availability of CMOV or equivalent */

#ifndef MDBX_HAVE_CMOV
#if defined(__e2k__)
#define MDBX_HAVE_CMOV 1
#elif defined(__thumb2__) || defined(__thumb2)
#define MDBX_HAVE_CMOV 1
#elif defined(__thumb__) || defined(__thumb) || defined(__TARGET_ARCH_THUMB)
#define MDBX_HAVE_CMOV 0
#elif defined(_M_ARM) || defined(_M_ARM64) || defined(__aarch64__) ||          \
    defined(__aarch64) || defined(__arm__) || defined(__arm) ||                \
    defined(__CC_ARM)
#define MDBX_HAVE_CMOV 1
#elif (defined(__riscv__) || defined(__riscv64)) &&                            \
    (defined(__riscv_b) || defined(__riscv_bitmanip))
#define MDBX_HAVE_CMOV 1
#elif defined(i686) || defined(__i686) || defined(__i686__) ||                 \
    (defined(_M_IX86) && _M_IX86 > 600) || defined(__x86_64) ||                \
    defined(__x86_64__) || defined(__amd64__) || defined(__amd64) ||           \
    defined(_M_X64) || defined(_M_AMD64)
#define MDBX_HAVE_CMOV 1
#else
#define MDBX_HAVE_CMOV 0
#endif
#endif /* MDBX_HAVE_CMOV */

/*----------------------------------------------------------------------------*/
/* Compiler's includes for builtins/intrinsics */

#if defined(_MSC_VER) || defined(__INTEL_COMPILER)
#include <intrin.h>
#elif __GNUC_PREREQ(4, 4) || defined(__clang__)
#if defined(__e2k__)
#include <e2kintrin.h>
#include <x86intrin.h>
#endif /* __e2k__ */
#if defined(__ia32__)
#include <cpuid.h>
#include <x86intrin.h>
#endif /* __ia32__ */
#ifdef __ARM_NEON
#include <arm_neon.h>
#endif
#elif defined(__SUNPRO_C) || defined(__sun) || defined(sun)
#include <mbarrier.h>
#elif (defined(_HPUX_SOURCE) || defined(__hpux) || defined(__HP_aCC)) &&       \
    (defined(HP_IA64) || defined(__ia64))
#include <machine/sys/inline.h>
#elif defined(__IBMC__) && defined(__powerpc)
#include <atomic.h>
#elif defined(_AIX)
#include <builtins.h>
#include <sys/atomic_op.h>
#elif (defined(__osf__) && defined(__DECC)) || defined(__alpha)
#include <c_asm.h>
#include <machine/builtins.h>
#elif defined(__MWERKS__)
/* CodeWarrior - troubles ? */
#pragma gcc_extensions
#elif defined(__SNC__)
/* Sony PS3 - troubles ? */
#elif defined(__hppa__) || defined(__hppa)
#include <machine/inline.h>
#else
#error Unsupported C compiler, please use GNU C 4.4 or newer
#endif /* Compiler */

#if !defined(__noop) && !defined(_MSC_VER)
#define __noop                                                                 \
  do {                                                                         \
  } while (0)
#endif /* __noop */

#if defined(__fallthrough) &&                                                  \
    (defined(__MINGW__) || defined(__MINGW32__) || defined(__MINGW64__))
#undef __fallthrough
#endif /* __fallthrough workaround for MinGW */

#ifndef __fallthrough
#if defined(__cplusplus) && (__has_cpp_attribute(fallthrough) &&               \
                             (!defined(__clang__) || __clang__ > 4)) ||        \
    __cplusplus >= 201703L
#define __fallthrough [[fallthrough]]
#elif __GNUC_PREREQ(8, 0) && defined(__cplusplus) && __cplusplus >= 201103L
#define __fallthrough [[fallthrough]]
#elif __GNUC_PREREQ(7, 0) &&                                                   \
    (!defined(__LCC__) || (__LCC__ == 124 && __LCC_MINOR__ >= 12) ||           \
     (__LCC__ == 125 && __LCC_MINOR__ >= 5) || (__LCC__ >= 126))
#define __fallthrough __attribute__((__fallthrough__))
#elif defined(__clang__) && defined(__cplusplus) && __cplusplus >= 201103L &&  \
    __has_feature(cxx_attributes) && __has_warning("-Wimplicit-fallthrough")
#define __fallthrough [[clang::fallthrough]]
#else
#define __fallthrough
#endif
#endif /* __fallthrough */

#ifndef __unreachable
#if __GNUC_PREREQ(4, 5) || __has_builtin(__builtin_unreachable)
#define __unreachable() __builtin_unreachable()
#elif defined(_MSC_VER)
#define __unreachable() __assume(0)
#else
#define __unreachable()                                                        \
  do {                                                                         \
  } while (1)
#endif
#endif /* __unreachable */

#ifndef __prefetch
#if defined(__GNUC__) || defined(__clang__) || __has_builtin(__builtin_prefetch)
#define __prefetch(ptr) __builtin_prefetch(ptr)
#else
#define __prefetch(ptr)                                                        \
  do {                                                                         \
    (void)(ptr);                                                               \
  } while (0)
#endif
#endif /* __prefetch */

#ifndef offsetof
#define offsetof(type, member) __builtin_offsetof(type, member)
#endif /* offsetof */

#ifndef container_of
#define container_of(ptr, type, member)                                        \
  ((type *)((char *)(ptr)-offsetof(type, member)))
#endif /* container_of */

/*----------------------------------------------------------------------------*/

#ifndef __always_inline
#if defined(__GNUC__) || __has_attribute(__always_inline__)
#define __always_inline __inline __attribute__((__always_inline__))
#elif defined(_MSC_VER)
#define __always_inline __forceinline
#else
#define __always_inline
#endif
#endif /* __always_inline */

#ifndef __noinline
#if defined(__GNUC__) || __has_attribute(__noinline__)
#define __noinline __attribute__((__noinline__))
#elif defined(_MSC_VER)
#define __noinline __declspec(noinline)
#else
#define __noinline
#endif
#endif /* __noinline */

#ifndef __must_check_result
#if defined(__GNUC__) || __has_attribute(__warn_unused_result__)
#define __must_check_result __attribute__((__warn_unused_result__))
#else
#define __must_check_result
#endif
#endif /* __must_check_result */

#ifndef __nothrow
#if defined(__cplusplus)
#if __cplusplus < 201703L
#define __nothrow throw()
#else
#define __nothrow noexcept(true)
#endif /* __cplusplus */
#elif defined(__GNUC__) || __has_attribute(__nothrow__)
#define __nothrow __attribute__((__nothrow__))
#elif defined(_MSC_VER) && defined(__cplusplus)
#define __nothrow __declspec(nothrow)
#else
#define __nothrow
#endif
#endif /* __nothrow */

#ifndef __hidden
#if defined(__GNUC__) || __has_attribute(__visibility__)
#define __hidden __attribute__((__visibility__("hidden")))
#else
#define __hidden
#endif
#endif /* __hidden */

#ifndef __optimize
#if defined(__OPTIMIZE__)
#if (defined(__GNUC__) && !defined(__clang__)) || __has_attribute(__optimize__)
#define __optimize(ops) __attribute__((__optimize__(ops)))
#else
#define __optimize(ops)
#endif
#else
#define __optimize(ops)
#endif
#endif /* __optimize */

#ifndef __hot
#if defined(__OPTIMIZE__)
#if defined(__clang__) && !__has_attribute(__hot__) &&                         \
    __has_attribute(__section__) &&                                            \
    (defined(__linux__) || defined(__gnu_linux__))
/* just put frequently used functions in separate section */
#define __hot __attribute__((__section__("text.hot"))) __optimize("O3")
#elif defined(__GNUC__) || __has_attribute(__hot__)
#define __hot __attribute__((__hot__))
#else
#define __hot __optimize("O3")
#endif
#else
#define __hot
#endif
#endif /* __hot */

#ifndef __cold
#if defined(__OPTIMIZE__)
#if defined(__clang__) && !__has_attribute(__cold__) &&                        \
    __has_attribute(__section__) &&                                            \
    (defined(__linux__) || defined(__gnu_linux__))
/* just put infrequently used functions in separate section */
#define __cold __attribute__((__section__("text.unlikely"))) __optimize("Os")
#elif defined(__GNUC__) || __has_attribute(__cold__)
#define __cold __attribute__((__cold__))
#else
#define __cold __optimize("Os")
#endif
#else
#define __cold
#endif
#endif /* __cold */

#ifndef __flatten
#if defined(__OPTIMIZE__) && (defined(__GNUC__) || __has_attribute(__flatten__))
#define __flatten __attribute__((__flatten__))
#else
#define __flatten
#endif
#endif /* __flatten */

#ifndef likely
#if (defined(__GNUC__) || __has_builtin(__builtin_expect)) &&                  \
    !defined(__COVERITY__)
#define likely(cond) __builtin_expect(!!(cond), 1)
#else
#define likely(x) (!!(x))
#endif
#endif /* likely */

#ifndef unlikely
#if (defined(__GNUC__) || __has_builtin(__builtin_expect)) &&                  \
    !defined(__COVERITY__)
#define unlikely(cond) __builtin_expect(!!(cond), 0)
#else
#define unlikely(x) (!!(x))
#endif
#endif /* unlikely */

#ifndef __anonymous_struct_extension__
#if defined(__GNUC__)
#define __anonymous_struct_extension__ __extension__
#else
#define __anonymous_struct_extension__
#endif
#endif /* __anonymous_struct_extension__ */

#ifndef expect_with_probability
#if defined(__builtin_expect_with_probability) ||                              \
    __has_builtin(__builtin_expect_with_probability) || __GNUC_PREREQ(9, 0)
#define expect_with_probability(expr, value, prob)                             \
  __builtin_expect_with_probability(expr, value, prob)
#else
#define expect_with_probability(expr, value, prob) (expr)
#endif
#endif /* expect_with_probability */

#ifndef MDBX_WEAK_IMPORT_ATTRIBUTE
#ifdef WEAK_IMPORT_ATTRIBUTE
#define MDBX_WEAK_IMPORT_ATTRIBUTE WEAK_IMPORT_ATTRIBUTE
#elif __has_attribute(__weak__) && __has_attribute(__weak_import__)
#define MDBX_WEAK_IMPORT_ATTRIBUTE __attribute__((__weak__, __weak_import__))
#elif __has_attribute(__weak__) ||                                             \
    (defined(__GNUC__) && __GNUC__ >= 4 && defined(__ELF__))
#define MDBX_WEAK_IMPORT_ATTRIBUTE __attribute__((__weak__))
#else
#define MDBX_WEAK_IMPORT_ATTRIBUTE
#endif
#endif /* MDBX_WEAK_IMPORT_ATTRIBUTE */

#ifndef MDBX_GOOFY_MSVC_STATIC_ANALYZER
#ifdef _PREFAST_
#define MDBX_GOOFY_MSVC_STATIC_ANALYZER 1
#else
#define MDBX_GOOFY_MSVC_STATIC_ANALYZER 0
#endif
#endif /* MDBX_GOOFY_MSVC_STATIC_ANALYZER */

#if MDBX_GOOFY_MSVC_STATIC_ANALYZER || (defined(_MSC_VER) && _MSC_VER > 1919)
#define MDBX_ANALYSIS_ASSUME(expr) __analysis_assume(expr)
#ifdef _PREFAST_
#define MDBX_SUPPRESS_GOOFY_MSVC_ANALYZER(warn_id)                             \
  __pragma(prefast(suppress : warn_id))
#else
#define MDBX_SUPPRESS_GOOFY_MSVC_ANALYZER(warn_id)                             \
  __pragma(warning(suppress : warn_id))
#endif
#else
#define MDBX_ANALYSIS_ASSUME(expr) assert(expr)
#define MDBX_SUPPRESS_GOOFY_MSVC_ANALYZER(warn_id)
#endif /* MDBX_GOOFY_MSVC_STATIC_ANALYZER */

/*----------------------------------------------------------------------------*/

#if defined(MDBX_USE_VALGRIND)
#include <valgrind/memcheck.h>
#ifndef VALGRIND_DISABLE_ADDR_ERROR_REPORTING_IN_RANGE
/* LY: available since Valgrind 3.10 */
#define VALGRIND_DISABLE_ADDR_ERROR_REPORTING_IN_RANGE(a, s)
#define VALGRIND_ENABLE_ADDR_ERROR_REPORTING_IN_RANGE(a, s)
#endif
#elif !defined(RUNNING_ON_VALGRIND)
#define VALGRIND_CREATE_MEMPOOL(h, r, z)
#define VALGRIND_DESTROY_MEMPOOL(h)
#define VALGRIND_MEMPOOL_TRIM(h, a, s)
#define VALGRIND_MEMPOOL_ALLOC(h, a, s)
#define VALGRIND_MEMPOOL_FREE(h, a)
#define VALGRIND_MEMPOOL_CHANGE(h, a, b, s)
#define VALGRIND_MAKE_MEM_NOACCESS(a, s)
#define VALGRIND_MAKE_MEM_DEFINED(a, s)
#define VALGRIND_MAKE_MEM_UNDEFINED(a, s)
#define VALGRIND_DISABLE_ADDR_ERROR_REPORTING_IN_RANGE(a, s)
#define VALGRIND_ENABLE_ADDR_ERROR_REPORTING_IN_RANGE(a, s)
#define VALGRIND_CHECK_MEM_IS_ADDRESSABLE(a, s) (0)
#define VALGRIND_CHECK_MEM_IS_DEFINED(a, s) (0)
#define RUNNING_ON_VALGRIND (0)
#endif /* MDBX_USE_VALGRIND */

#ifdef __SANITIZE_ADDRESS__
#include <sanitizer/asan_interface.h>
#elif !defined(ASAN_POISON_MEMORY_REGION)
#define ASAN_POISON_MEMORY_REGION(addr, size) ((void)(addr), (void)(size))
#define ASAN_UNPOISON_MEMORY_REGION(addr, size) ((void)(addr), (void)(size))
#endif /* __SANITIZE_ADDRESS__ */

/*----------------------------------------------------------------------------*/

#ifndef ARRAY_LENGTH
#ifdef __cplusplus
template <typename T, size_t N> char (&__ArraySizeHelper(T (&array)[N]))[N];
#define ARRAY_LENGTH(array) (sizeof(::__ArraySizeHelper(array)))
#else
#define ARRAY_LENGTH(array) (sizeof(array) / sizeof(array[0]))
#endif
#endif /* ARRAY_LENGTH */

#ifndef ARRAY_END
#define ARRAY_END(array) (&array[ARRAY_LENGTH(array)])
#endif /* ARRAY_END */

#define CONCAT(a, b) a##b
#define XCONCAT(a, b) CONCAT(a, b)

#define MDBX_TETRAD(a, b, c, d)                                                \
  ((uint32_t)(a) << 24 | (uint32_t)(b) << 16 | (uint32_t)(c) << 8 | (d))

#define MDBX_STRING_TETRAD(str) MDBX_TETRAD(str[0], str[1], str[2], str[3])

#define FIXME "FIXME: " __FILE__ ", " MDBX_STRINGIFY(__LINE__)

#ifndef STATIC_ASSERT_MSG
#if defined(static_assert)
#define STATIC_ASSERT_MSG(expr, msg) static_assert(expr, msg)
#elif defined(_STATIC_ASSERT)
#define STATIC_ASSERT_MSG(expr, msg) _STATIC_ASSERT(expr)
#elif defined(_MSC_VER)
#include <crtdbg.h>
#define STATIC_ASSERT_MSG(expr, msg) _STATIC_ASSERT(expr)
#elif (defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L) ||            \
    __has_feature(c_static_assert)
#define STATIC_ASSERT_MSG(expr, msg) _Static_assert(expr, msg)
#else
#define STATIC_ASSERT_MSG(expr, msg)                                           \
  switch (0) {                                                                 \
  case 0:                                                                      \
  case (expr):;                                                                \
  }
#endif
#endif /* STATIC_ASSERT */

#ifndef STATIC_ASSERT
#define STATIC_ASSERT(expr) STATIC_ASSERT_MSG(expr, #expr)
#endif

#ifndef __Wpedantic_format_voidptr
MDBX_MAYBE_UNUSED MDBX_PURE_FUNCTION static __inline const void *
__Wpedantic_format_voidptr(const void *ptr) {
  return ptr;
}
#define __Wpedantic_format_voidptr(ARG) __Wpedantic_format_voidptr(ARG)
#endif /* __Wpedantic_format_voidptr */

#if defined(__GNUC__) && !__GNUC_PREREQ(4, 2)
/* Actually libmdbx was not tested with compilers older than GCC 4.2.
 * But you could ignore this warning at your own risk.
 * In such case please don't rise up an issues related ONLY to old compilers.
 */
#warning "libmdbx required GCC >= 4.2"
#endif

#if defined(__clang__) && !__CLANG_PREREQ(3, 8)
/* Actually libmdbx was not tested with CLANG older than 3.8.
 * But you could ignore this warning at your own risk.
 * In such case please don't rise up an issues related ONLY to old compilers.
 */
#warning "libmdbx required CLANG >= 3.8"
#endif

#if defined(__GLIBC__) && !__GLIBC_PREREQ(2, 12)
/* Actually libmdbx was not tested with something older than glibc 2.12.
 * But you could ignore this warning at your own risk.
 * In such case please don't rise up an issues related ONLY to old systems.
 */
#warning "libmdbx was only tested with GLIBC >= 2.12."
#endif

#ifdef __SANITIZE_THREAD__
#warning                                                                       \
    "libmdbx don't compatible with ThreadSanitizer, you will get a lot of false-positive issues."
#endif /* __SANITIZE_THREAD__ */

#if __has_warning("-Wnested-anon-types")
#if defined(__clang__)
#pragma clang diagnostic ignored "-Wnested-anon-types"
#elif defined(__GNUC__)
#pragma GCC diagnostic ignored "-Wnested-anon-types"
#else
#pragma warning disable "nested-anon-types"
#endif
#endif /* -Wnested-anon-types */

#if __has_warning("-Wconstant-logical-operand")
#if defined(__clang__)
#pragma clang diagnostic ignored "-Wconstant-logical-operand"
#elif defined(__GNUC__)
#pragma GCC diagnostic ignored "-Wconstant-logical-operand"
#else
#pragma warning disable "constant-logical-operand"
#endif
#endif /* -Wconstant-logical-operand */

#if defined(__LCC__) && (__LCC__ <= 121)
/* bug #2798 */
#pragma diag_suppress alignment_reduction_ignored
#elif defined(__ICC)
#pragma warning(disable : 3453 1366)
#elif __has_warning("-Walignment-reduction-ignored")
#if defined(__clang__)
#pragma clang diagnostic ignored "-Walignment-reduction-ignored"
#elif defined(__GNUC__)
#pragma GCC diagnostic ignored "-Walignment-reduction-ignored"
#else
#pragma warning disable "alignment-reduction-ignored"
#endif
#endif /* -Walignment-reduction-ignored */

#ifndef MDBX_EXCLUDE_FOR_GPROF
#ifdef ENABLE_GPROF
#define MDBX_EXCLUDE_FOR_GPROF                                                 \
  __attribute__((__no_instrument_function__,                                   \
                 __no_profile_instrument_function__))
#else
#define MDBX_EXCLUDE_FOR_GPROF
#endif /* ENABLE_GPROF */
#endif /* MDBX_EXCLUDE_FOR_GPROF */

#ifdef __cplusplus
extern "C" {
#endif

/* https://en.wikipedia.org/wiki/Operating_system_abstraction_layer */

/*
 * Copyright 2015-2023 Leonid Yuriev <leo@yuriev.ru>
 * and other libmdbx authors: please see AUTHORS file.
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted only as authorized by the OpenLDAP
 * Public License.
 *
 * A copy of this license is available in the file LICENSE in the
 * top-level directory of the distribution or, alternatively, at
 * <http://www.OpenLDAP.org/license.html>.
 */


/*----------------------------------------------------------------------------*/
/* C11 Atomics */

#if defined(__cplusplus) && !defined(__STDC_NO_ATOMICS__) && __has_include(<cstdatomic>)
#include <cstdatomic>
#define MDBX_HAVE_C11ATOMICS
#elif !defined(__cplusplus) &&                                                 \
    (__STDC_VERSION__ >= 201112L || __has_extension(c_atomic)) &&              \
    !defined(__STDC_NO_ATOMICS__) &&                                           \
    (__GNUC_PREREQ(4, 9) || __CLANG_PREREQ(3, 8) ||                            \
     !(defined(__GNUC__) || defined(__clang__)))
#include <stdatomic.h>
#define MDBX_HAVE_C11ATOMICS
#elif defined(__GNUC__) || defined(__clang__)
#elif defined(_MSC_VER)
#pragma warning(disable : 4163) /* 'xyz': not available as an intrinsic */
#pragma warning(disable : 4133) /* 'function': incompatible types - from       \
                                   'size_t' to 'LONGLONG' */
#pragma warning(disable : 4244) /* 'return': conversion from 'LONGLONG' to     \
                                   'std::size_t', possible loss of data */
#pragma warning(disable : 4267) /* 'function': conversion from 'size_t' to     \
                                   'long', possible loss of data */
#pragma intrinsic(_InterlockedExchangeAdd, _InterlockedCompareExchange)
#pragma intrinsic(_InterlockedExchangeAdd64, _InterlockedCompareExchange64)
#elif defined(__APPLE__)
#include <libkern/OSAtomic.h>
#else
#error FIXME atomic-ops
#endif

/*----------------------------------------------------------------------------*/
/* Memory/Compiler barriers, cache coherence */

#if __has_include(<sys/cachectl.h>)
#include <sys/cachectl.h>
#elif defined(__mips) || defined(__mips__) || defined(__mips64) ||             \
    defined(__mips64__) || defined(_M_MRX000) || defined(_MIPS_) ||            \
    defined(__MWERKS__) || defined(__sgi)
/* MIPS should have explicit cache control */
#include <sys/cachectl.h>
#endif

MDBX_MAYBE_UNUSED static __inline void osal_compiler_barrier(void) {
#if defined(__clang__) || defined(__GNUC__)
  __asm__ __volatile__("" ::: "memory");
#elif defined(_MSC_VER)
  _ReadWriteBarrier();
#elif defined(__INTEL_COMPILER) /* LY: Intel Compiler may mimic GCC and MSC */
  __memory_barrier();
#elif defined(__SUNPRO_C) || defined(__sun) || defined(sun)
  __compiler_barrier();
#elif (defined(_HPUX_SOURCE) || defined(__hpux) || defined(__HP_aCC)) &&       \
    (defined(HP_IA64) || defined(__ia64))
  _Asm_sched_fence(/* LY: no-arg meaning 'all expect ALU', e.g. 0x3D3D */);
#elif defined(_AIX) || defined(__ppc__) || defined(__powerpc__) ||             \
    defined(__ppc64__) || defined(__powerpc64__)
  __fence();
#else
#error "Could not guess the kind of compiler, please report to us."
#endif
}

MDBX_MAYBE_UNUSED static __inline void osal_memory_barrier(void) {
#ifdef MDBX_HAVE_C11ATOMICS
  atomic_thread_fence(memory_order_seq_cst);
#elif defined(__ATOMIC_SEQ_CST)
#ifdef __clang__
  __c11_atomic_thread_fence(__ATOMIC_SEQ_CST);
#else
  __atomic_thread_fence(__ATOMIC_SEQ_CST);
#endif
#elif defined(__clang__) || defined(__GNUC__)
  __sync_synchronize();
#elif defined(_WIN32) || defined(_WIN64)
  MemoryBarrier();
#elif defined(__INTEL_COMPILER) /* LY: Intel Compiler may mimic GCC and MSC */
#if defined(__ia32__)
  _mm_mfence();
#else
  __mf();
#endif
#elif defined(__SUNPRO_C) || defined(__sun) || defined(sun)
  __machine_rw_barrier();
#elif (defined(_HPUX_SOURCE) || defined(__hpux) || defined(__HP_aCC)) &&       \
    (defined(HP_IA64) || defined(__ia64))
  _Asm_mf();
#elif defined(_AIX) || defined(__ppc__) || defined(__powerpc__) ||             \
    defined(__ppc64__) || defined(__powerpc64__)
  __lwsync();
#else
#error "Could not guess the kind of compiler, please report to us."
#endif
}

/*----------------------------------------------------------------------------*/
/* system-depended definitions */

#if defined(_WIN32) || defined(_WIN64)
#define HAVE_SYS_STAT_H
#define HAVE_SYS_TYPES_H
typedef HANDLE osal_thread_t;
typedef unsigned osal_thread_key_t;
#define MAP_FAILED NULL
#define HIGH_DWORD(v) ((DWORD)((sizeof(v) > 4) ? ((uint64_t)(v) >> 32) : 0))
#define THREAD_CALL WINAPI
#define THREAD_RESULT DWORD
typedef struct {
  HANDLE mutex;
  HANDLE event[2];
} osal_condpair_t;
typedef CRITICAL_SECTION osal_fastmutex_t;

#if !defined(_MSC_VER) && !defined(__try)
#define __try
#define __except(COND) if (false)
#endif /* stub for MSVC's __try/__except */

#if MDBX_WITHOUT_MSVC_CRT

#ifndef osal_malloc
static inline void *osal_malloc(size_t bytes) {
  return HeapAlloc(GetProcessHeap(), 0, bytes);
}
#endif /* osal_malloc */

#ifndef osal_calloc
static inline void *osal_calloc(size_t nelem, size_t size) {
  return HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, nelem * size);
}
#endif /* osal_calloc */

#ifndef osal_realloc
static inline void *osal_realloc(void *ptr, size_t bytes) {
  return ptr ? HeapReAlloc(GetProcessHeap(), 0, ptr, bytes)
             : HeapAlloc(GetProcessHeap(), 0, bytes);
}
#endif /* osal_realloc */

#ifndef osal_free
static inline void osal_free(void *ptr) { HeapFree(GetProcessHeap(), 0, ptr); }
#endif /* osal_free */

#else /* MDBX_WITHOUT_MSVC_CRT */

#define osal_malloc malloc
#define osal_calloc calloc
#define osal_realloc realloc
#define osal_free free
#define osal_strdup _strdup

#endif /* MDBX_WITHOUT_MSVC_CRT */

#ifndef snprintf
#define snprintf _snprintf /* ntdll */
#endif

#ifndef vsnprintf
#define vsnprintf _vsnprintf /* ntdll */
#endif

#else /*----------------------------------------------------------------------*/

typedef pthread_t osal_thread_t;
typedef pthread_key_t osal_thread_key_t;
#define INVALID_HANDLE_VALUE (-1)
#define THREAD_CALL
#define THREAD_RESULT void *
typedef struct {
  pthread_mutex_t mutex;
  pthread_cond_t cond[2];
} osal_condpair_t;
typedef pthread_mutex_t osal_fastmutex_t;
#define osal_malloc malloc
#define osal_calloc calloc
#define osal_realloc realloc
#define osal_free free
#define osal_strdup strdup
#endif /* Platform */

#if __GLIBC_PREREQ(2, 12) || defined(__FreeBSD__) || defined(malloc_usable_size)
/* malloc_usable_size() already provided */
#elif defined(__APPLE__)
#define malloc_usable_size(ptr) malloc_size(ptr)
#elif defined(_MSC_VER) && !MDBX_WITHOUT_MSVC_CRT
#define malloc_usable_size(ptr) _msize(ptr)
#endif /* malloc_usable_size */

/*----------------------------------------------------------------------------*/
/* OS abstraction layer stuff */

MDBX_INTERNAL_VAR unsigned sys_pagesize;
MDBX_MAYBE_UNUSED MDBX_INTERNAL_VAR unsigned sys_pagesize_ln2,
    sys_allocation_granularity;

/* Get the size of a memory page for the system.
 * This is the basic size that the platform's memory manager uses, and is
 * fundamental to the use of memory-mapped files. */
MDBX_MAYBE_UNUSED MDBX_NOTHROW_CONST_FUNCTION static __inline size_t
osal_syspagesize(void) {
  assert(sys_pagesize > 0 && (sys_pagesize & (sys_pagesize - 1)) == 0);
  return sys_pagesize;
}

#if defined(_WIN32) || defined(_WIN64)
typedef wchar_t pathchar_t;
#define MDBX_PRIsPATH "ls"
#else
typedef char pathchar_t;
#define MDBX_PRIsPATH "s"
#endif

typedef struct osal_mmap {
  union {
    void *base;
    struct MDBX_lockinfo *lck;
  };
  mdbx_filehandle_t fd;
  size_t limit;   /* mapping length, but NOT a size of file nor DB */
  size_t current; /* mapped region size, i.e. the size of file and DB */
  uint64_t filesize /* in-process cache of a file size */;
#if defined(_WIN32) || defined(_WIN64)
  HANDLE section; /* memory-mapped section handle */
#endif
} osal_mmap_t;

typedef union bin128 {
  __anonymous_struct_extension__ struct {
    uint64_t x, y;
  };
  __anonymous_struct_extension__ struct {
    uint32_t a, b, c, d;
  };
} bin128_t;

#if defined(_WIN32) || defined(_WIN64)
typedef union osal_srwlock {
  __anonymous_struct_extension__ struct {
    long volatile readerCount;
    long volatile writerCount;
  };
  RTL_SRWLOCK native;
} osal_srwlock_t;
#endif /* Windows */

#ifndef MDBX_HAVE_PWRITEV
#if defined(_WIN32) || defined(_WIN64)

#define MDBX_HAVE_PWRITEV 0

#elif defined(__ANDROID_API__)

#if __ANDROID_API__ < 24
#define MDBX_HAVE_PWRITEV 0
#else
#define MDBX_HAVE_PWRITEV 1
#endif

#elif defined(__APPLE__) || defined(__MACH__) || defined(_DARWIN_C_SOURCE)

#if defined(MAC_OS_X_VERSION_MIN_REQUIRED) && defined(MAC_OS_VERSION_11_0) &&  \
    MAC_OS_X_VERSION_MIN_REQUIRED >= MAC_OS_VERSION_11_0
/* FIXME: add checks for IOS versions, etc */
#define MDBX_HAVE_PWRITEV 1
#else
#define MDBX_HAVE_PWRITEV 0
#endif

#elif defined(_SC_IOV_MAX) || (defined(IOV_MAX) && IOV_MAX > 1)
#define MDBX_HAVE_PWRITEV 1
#else
#define MDBX_HAVE_PWRITEV 0
#endif
#endif /* MDBX_HAVE_PWRITEV */

typedef struct ior_item {
#if defined(_WIN32) || defined(_WIN64)
  OVERLAPPED ov;
#define ior_svg_gap4terminator 1
#define ior_sgv_element FILE_SEGMENT_ELEMENT
#else
  size_t offset;
#if MDBX_HAVE_PWRITEV
  size_t sgvcnt;
#define ior_svg_gap4terminator 0
#define ior_sgv_element struct iovec
#endif /* MDBX_HAVE_PWRITEV */
#endif /* !Windows */
  union {
    MDBX_val single;
#if defined(ior_sgv_element)
    ior_sgv_element sgv[1 + ior_svg_gap4terminator];
#endif /* ior_sgv_element */
  };
} ior_item_t;

typedef struct osal_ioring {
  unsigned slots_left;
  unsigned allocated;
#if defined(_WIN32) || defined(_WIN64)
#define IOR_STATE_LOCKED 1
  HANDLE overlapped_fd;
  unsigned pagesize;
  unsigned last_sgvcnt;
  size_t last_bytes;
  uint8_t direct, state, pagesize_ln2;
  unsigned event_stack;
  HANDLE *event_pool;
  volatile LONG async_waiting;
  volatile LONG async_completed;
  HANDLE async_done;

#define ior_last_sgvcnt(ior, item) (ior)->last_sgvcnt
#define ior_last_bytes(ior, item) (ior)->last_bytes
#elif MDBX_HAVE_PWRITEV
  unsigned last_bytes;
#define ior_last_sgvcnt(ior, item) (item)->sgvcnt
#define ior_last_bytes(ior, item) (ior)->last_bytes
#else
#define ior_last_sgvcnt(ior, item) (1)
#define ior_last_bytes(ior, item) (item)->single.iov_len
#endif /* !Windows */
  ior_item_t *last;
  ior_item_t *pool;
  char *boundary;
} osal_ioring_t;

#ifndef __cplusplus

/* Actually this is not ioring for now, but on the way. */
MDBX_INTERNAL_FUNC int osal_ioring_create(osal_ioring_t *
#if defined(_WIN32) || defined(_WIN64)
                                          ,
                                          bool enable_direct,
                                          mdbx_filehandle_t overlapped_fd
#endif /* Windows */
);
MDBX_INTERNAL_FUNC int osal_ioring_resize(osal_ioring_t *, size_t items);
MDBX_INTERNAL_FUNC void osal_ioring_destroy(osal_ioring_t *);
MDBX_INTERNAL_FUNC void osal_ioring_reset(osal_ioring_t *);
MDBX_INTERNAL_FUNC int osal_ioring_add(osal_ioring_t *ctx, const size_t offset,
                                       void *data, const size_t bytes);
typedef struct osal_ioring_write_result {
  int err;
  unsigned wops;
} osal_ioring_write_result_t;
MDBX_INTERNAL_FUNC osal_ioring_write_result_t
osal_ioring_write(osal_ioring_t *ior, mdbx_filehandle_t fd);

typedef struct iov_ctx iov_ctx_t;
MDBX_INTERNAL_FUNC void osal_ioring_walk(
    osal_ioring_t *ior, iov_ctx_t *ctx,
    void (*callback)(iov_ctx_t *ctx, size_t offset, void *data, size_t bytes));

MDBX_MAYBE_UNUSED static inline unsigned
osal_ioring_left(const osal_ioring_t *ior) {
  return ior->slots_left;
}

MDBX_MAYBE_UNUSED static inline unsigned
osal_ioring_used(const osal_ioring_t *ior) {
  return ior->allocated - ior->slots_left;
}

MDBX_MAYBE_UNUSED static inline int
osal_ioring_prepare(osal_ioring_t *ior, size_t items, size_t bytes) {
  items = (items > 32) ? items : 32;
#if defined(_WIN32) || defined(_WIN64)
  if (ior->direct) {
    const size_t npages = bytes >> ior->pagesize_ln2;
    items = (items > npages) ? items : npages;
  }
#else
  (void)bytes;
#endif
  items = (items < 65536) ? items : 65536;
  if (likely(ior->allocated >= items))
    return MDBX_SUCCESS;
  return osal_ioring_resize(ior, items);
}

/*----------------------------------------------------------------------------*/
/* libc compatibility stuff */

#if (!defined(__GLIBC__) && __GLIBC_PREREQ(2, 1)) &&                           \
    (defined(_GNU_SOURCE) || defined(_BSD_SOURCE))
#define osal_asprintf asprintf
#define osal_vasprintf vasprintf
#else
MDBX_MAYBE_UNUSED MDBX_INTERNAL_FUNC
    MDBX_PRINTF_ARGS(2, 3) int osal_asprintf(char **strp, const char *fmt, ...);
MDBX_INTERNAL_FUNC int osal_vasprintf(char **strp, const char *fmt, va_list ap);
#endif

#if !defined(MADV_DODUMP) && defined(MADV_CORE)
#define MADV_DODUMP MADV_CORE
#endif /* MADV_CORE -> MADV_DODUMP */

#if !defined(MADV_DONTDUMP) && defined(MADV_NOCORE)
#define MADV_DONTDUMP MADV_NOCORE
#endif /* MADV_NOCORE -> MADV_DONTDUMP */

MDBX_MAYBE_UNUSED MDBX_INTERNAL_FUNC void osal_jitter(bool tiny);
MDBX_MAYBE_UNUSED static __inline void jitter4testing(bool tiny);

/* max bytes to write in one call */
#if defined(_WIN64)
#define MAX_WRITE UINT32_C(0x10000000)
#elif defined(_WIN32)
#define MAX_WRITE UINT32_C(0x04000000)
#else
#define MAX_WRITE UINT32_C(0x3f000000)

#if defined(F_GETLK64) && defined(F_SETLK64) && defined(F_SETLKW64) &&         \
    !defined(__ANDROID_API__)
#define MDBX_F_SETLK F_SETLK64
#define MDBX_F_SETLKW F_SETLKW64
#define MDBX_F_GETLK F_GETLK64
#if (__GLIBC_PREREQ(2, 28) &&                                                  \
     (defined(__USE_LARGEFILE64) || defined(__LARGEFILE64_SOURCE) ||           \
      defined(_USE_LARGEFILE64) || defined(_LARGEFILE64_SOURCE))) ||           \
    defined(fcntl64)
#define MDBX_FCNTL fcntl64
#else
#define MDBX_FCNTL fcntl
#endif
#define MDBX_STRUCT_FLOCK struct flock64
#ifndef OFF_T_MAX
#define OFF_T_MAX UINT64_C(0x7fffFFFFfff00000)
#endif /* OFF_T_MAX */
#else
#define MDBX_F_SETLK F_SETLK
#define MDBX_F_SETLKW F_SETLKW
#define MDBX_F_GETLK F_GETLK
#define MDBX_FCNTL fcntl
#define MDBX_STRUCT_FLOCK struct flock
#endif /* MDBX_F_SETLK, MDBX_F_SETLKW, MDBX_F_GETLK */

#if defined(F_OFD_SETLK64) && defined(F_OFD_SETLKW64) &&                       \
    defined(F_OFD_GETLK64) && !defined(__ANDROID_API__)
#define MDBX_F_OFD_SETLK F_OFD_SETLK64
#define MDBX_F_OFD_SETLKW F_OFD_SETLKW64
#define MDBX_F_OFD_GETLK F_OFD_GETLK64
#else
#define MDBX_F_OFD_SETLK F_OFD_SETLK
#define MDBX_F_OFD_SETLKW F_OFD_SETLKW
#define MDBX_F_OFD_GETLK F_OFD_GETLK
#ifndef OFF_T_MAX
#define OFF_T_MAX                                                              \
  (((sizeof(off_t) > 4) ? INT64_MAX : INT32_MAX) & ~(size_t)0xFffff)
#endif /* OFF_T_MAX */
#endif /* MDBX_F_OFD_SETLK64, MDBX_F_OFD_SETLKW64, MDBX_F_OFD_GETLK64 */

#endif

#if defined(__linux__) || defined(__gnu_linux__)
MDBX_INTERNAL_VAR uint32_t linux_kernel_version;
MDBX_INTERNAL_VAR bool mdbx_RunningOnWSL1 /* Windows Subsystem 1 for Linux */;
#endif /* Linux */

#ifndef osal_strdup
LIBMDBX_API char *osal_strdup(const char *str);
#endif

MDBX_MAYBE_UNUSED static __inline int osal_get_errno(void) {
#if defined(_WIN32) || defined(_WIN64)
  DWORD rc = GetLastError();
#else
  int rc = errno;
#endif
  return rc;
}

#ifndef osal_memalign_alloc
MDBX_INTERNAL_FUNC int osal_memalign_alloc(size_t alignment, size_t bytes,
                                           void **result);
#endif
#ifndef osal_memalign_free
MDBX_INTERNAL_FUNC void osal_memalign_free(void *ptr);
#endif

MDBX_INTERNAL_FUNC int osal_condpair_init(osal_condpair_t *condpair);
MDBX_INTERNAL_FUNC int osal_condpair_lock(osal_condpair_t *condpair);
MDBX_INTERNAL_FUNC int osal_condpair_unlock(osal_condpair_t *condpair);
MDBX_INTERNAL_FUNC int osal_condpair_signal(osal_condpair_t *condpair,
                                            bool part);
MDBX_INTERNAL_FUNC int osal_condpair_wait(osal_condpair_t *condpair, bool part);
MDBX_INTERNAL_FUNC int osal_condpair_destroy(osal_condpair_t *condpair);

MDBX_INTERNAL_FUNC int osal_fastmutex_init(osal_fastmutex_t *fastmutex);
MDBX_INTERNAL_FUNC int osal_fastmutex_acquire(osal_fastmutex_t *fastmutex);
MDBX_INTERNAL_FUNC int osal_fastmutex_release(osal_fastmutex_t *fastmutex);
MDBX_INTERNAL_FUNC int osal_fastmutex_destroy(osal_fastmutex_t *fastmutex);

MDBX_INTERNAL_FUNC int osal_pwritev(mdbx_filehandle_t fd, struct iovec *iov,
                                    size_t sgvcnt, uint64_t offset);
MDBX_INTERNAL_FUNC int osal_pread(mdbx_filehandle_t fd, void *buf, size_t count,
                                  uint64_t offset);
MDBX_INTERNAL_FUNC int osal_pwrite(mdbx_filehandle_t fd, const void *buf,
                                   size_t count, uint64_t offset);
MDBX_INTERNAL_FUNC int osal_write(mdbx_filehandle_t fd, const void *buf,
                                  size_t count);

MDBX_INTERNAL_FUNC int
osal_thread_create(osal_thread_t *thread,
                   THREAD_RESULT(THREAD_CALL *start_routine)(void *),
                   void *arg);
MDBX_INTERNAL_FUNC int osal_thread_join(osal_thread_t thread);

enum osal_syncmode_bits {
  MDBX_SYNC_NONE = 0,
  MDBX_SYNC_KICK = 1,
  MDBX_SYNC_DATA = 2,
  MDBX_SYNC_SIZE = 4,
  MDBX_SYNC_IODQ = 8
};

MDBX_INTERNAL_FUNC int osal_fsync(mdbx_filehandle_t fd,
                                  const enum osal_syncmode_bits mode_bits);
MDBX_INTERNAL_FUNC int osal_ftruncate(mdbx_filehandle_t fd, uint64_t length);
MDBX_INTERNAL_FUNC int osal_fseek(mdbx_filehandle_t fd, uint64_t pos);
MDBX_INTERNAL_FUNC int osal_filesize(mdbx_filehandle_t fd, uint64_t *length);

enum osal_openfile_purpose {
  MDBX_OPEN_DXB_READ,
  MDBX_OPEN_DXB_LAZY,
  MDBX_OPEN_DXB_DSYNC,
#if defined(_WIN32) || defined(_WIN64)
  MDBX_OPEN_DXB_OVERLAPPED,
  MDBX_OPEN_DXB_OVERLAPPED_DIRECT,
#endif /* Windows */
  MDBX_OPEN_LCK,
  MDBX_OPEN_COPY,
  MDBX_OPEN_DELETE
};

MDBX_MAYBE_UNUSED static __inline bool osal_isdirsep(pathchar_t c) {
  return
#if defined(_WIN32) || defined(_WIN64)
      c == '\\' ||
#endif
      c == '/';
}

MDBX_INTERNAL_FUNC bool osal_pathequal(const pathchar_t *l, const pathchar_t *r,
                                       size_t len);
MDBX_INTERNAL_FUNC pathchar_t *osal_fileext(const pathchar_t *pathname,
                                            size_t len);
MDBX_INTERNAL_FUNC int osal_fileexists(const pathchar_t *pathname);
MDBX_INTERNAL_FUNC int osal_openfile(const enum osal_openfile_purpose purpose,
                                     const MDBX_env *env,
                                     const pathchar_t *pathname,
                                     mdbx_filehandle_t *fd,
                                     mdbx_mode_t unix_mode_bits);
MDBX_INTERNAL_FUNC int osal_closefile(mdbx_filehandle_t fd);
MDBX_INTERNAL_FUNC int osal_removefile(const pathchar_t *pathname);
MDBX_INTERNAL_FUNC int osal_removedirectory(const pathchar_t *pathname);
MDBX_INTERNAL_FUNC int osal_is_pipe(mdbx_filehandle_t fd);
MDBX_INTERNAL_FUNC int osal_lockfile(mdbx_filehandle_t fd, bool wait);

#define MMAP_OPTION_TRUNCATE 1
#define MMAP_OPTION_SEMAPHORE 2
MDBX_INTERNAL_FUNC int osal_mmap(const int flags, osal_mmap_t *map, size_t size,
                                 const size_t limit, const unsigned options);
MDBX_INTERNAL_FUNC int osal_munmap(osal_mmap_t *map);
#define MDBX_MRESIZE_MAY_MOVE 0x00000100
#define MDBX_MRESIZE_MAY_UNMAP 0x00000200
MDBX_INTERNAL_FUNC int osal_mresize(const int flags, osal_mmap_t *map,
                                    size_t size, size_t limit);
#if defined(_WIN32) || defined(_WIN64)
typedef struct {
  unsigned limit, count;
  HANDLE handles[31];
} mdbx_handle_array_t;
MDBX_INTERNAL_FUNC int
osal_suspend_threads_before_remap(MDBX_env *env, mdbx_handle_array_t **array);
MDBX_INTERNAL_FUNC int
osal_resume_threads_after_remap(mdbx_handle_array_t *array);
#endif /* Windows */
MDBX_INTERNAL_FUNC int osal_msync(const osal_mmap_t *map, size_t offset,
                                  size_t length,
                                  enum osal_syncmode_bits mode_bits);
MDBX_INTERNAL_FUNC int osal_check_fs_rdonly(mdbx_filehandle_t handle,
                                            const pathchar_t *pathname,
                                            int err);
MDBX_INTERNAL_FUNC int osal_check_fs_incore(mdbx_filehandle_t handle);

MDBX_MAYBE_UNUSED static __inline uint32_t osal_getpid(void) {
  STATIC_ASSERT(sizeof(mdbx_pid_t) <= sizeof(uint32_t));
#if defined(_WIN32) || defined(_WIN64)
  return GetCurrentProcessId();
#else
  STATIC_ASSERT(sizeof(pid_t) <= sizeof(uint32_t));
  return getpid();
#endif
}

MDBX_MAYBE_UNUSED static __inline uintptr_t osal_thread_self(void) {
  mdbx_tid_t thunk;
  STATIC_ASSERT(sizeof(uintptr_t) >= sizeof(thunk));
#if defined(_WIN32) || defined(_WIN64)
  thunk = GetCurrentThreadId();
#else
  thunk = pthread_self();
#endif
  return (uintptr_t)thunk;
}

#if !defined(_WIN32) && !defined(_WIN64)
#if defined(__ANDROID_API__) || defined(ANDROID) || defined(BIONIC)
MDBX_INTERNAL_FUNC int osal_check_tid4bionic(void);
#else
static __inline int osal_check_tid4bionic(void) { return 0; }
#endif /* __ANDROID_API__ || ANDROID) || BIONIC */

MDBX_MAYBE_UNUSED static __inline int
osal_pthread_mutex_lock(pthread_mutex_t *mutex) {
  int err = osal_check_tid4bionic();
  return unlikely(err) ? err : pthread_mutex_lock(mutex);
}
#endif /* !Windows */

MDBX_INTERNAL_FUNC uint64_t osal_monotime(void);
MDBX_INTERNAL_FUNC uint64_t osal_cputime(size_t *optional_page_faults);
MDBX_INTERNAL_FUNC uint64_t osal_16dot16_to_monotime(uint32_t seconds_16dot16);
MDBX_INTERNAL_FUNC uint32_t osal_monotime_to_16dot16(uint64_t monotime);

MDBX_MAYBE_UNUSED static inline uint32_t
osal_monotime_to_16dot16_noUnderflow(uint64_t monotime) {
  uint32_t seconds_16dot16 = osal_monotime_to_16dot16(monotime);
  return seconds_16dot16 ? seconds_16dot16 : /* fix underflow */ (monotime > 0);
}

MDBX_INTERNAL_FUNC bin128_t osal_bootid(void);
/*----------------------------------------------------------------------------*/
/* lck stuff */

/// \brief Initialization of synchronization primitives linked with MDBX_env
///   instance both in LCK-file and within the current process.
/// \param
///   global_uniqueness_flag = true - denotes that there are no other processes
///     working with DB and LCK-file. Thus the function MUST initialize
///     shared synchronization objects in memory-mapped LCK-file.
///   global_uniqueness_flag = false - denotes that at least one process is
///     already working with DB and LCK-file, including the case when DB
///     has already been opened in the current process. Thus the function
///     MUST NOT initialize shared synchronization objects in memory-mapped
///     LCK-file that are already in use.
/// \return Error code or zero on success.
MDBX_INTERNAL_FUNC int osal_lck_init(MDBX_env *env,
                                     MDBX_env *inprocess_neighbor,
                                     int global_uniqueness_flag);

/// \brief Disconnects from shared interprocess objects and destructs
///   synchronization objects linked with MDBX_env instance
///   within the current process.
/// \param
///   inprocess_neighbor = NULL - if the current process does not have other
///     instances of MDBX_env linked with the DB being closed.
///     Thus the function MUST check for other processes working with DB or
///     LCK-file, and keep or destroy shared synchronization objects in
///     memory-mapped LCK-file depending on the result.
///   inprocess_neighbor = not-NULL - pointer to another instance of MDBX_env
///     (anyone of there is several) working with DB or LCK-file within the
///     current process. Thus the function MUST NOT try to acquire exclusive
///     lock and/or try to destruct shared synchronization objects linked with
///     DB or LCK-file. Moreover, the implementation MUST ensure correct work
///     of other instances of MDBX_env within the current process, e.g.
///     restore POSIX-fcntl locks after the closing of file descriptors.
/// \return Error code (MDBX_PANIC) or zero on success.
MDBX_INTERNAL_FUNC int osal_lck_destroy(MDBX_env *env,
                                        MDBX_env *inprocess_neighbor);

/// \brief Connects to shared interprocess locking objects and tries to acquire
///   the maximum lock level (shared if exclusive is not available)
///   Depending on implementation or/and platform (Windows) this function may
///   acquire the non-OS super-level lock (e.g. for shared synchronization
///   objects initialization), which will be downgraded to OS-exclusive or
///   shared via explicit calling of osal_lck_downgrade().
/// \return
///   MDBX_RESULT_TRUE (-1) - if an exclusive lock was acquired and thus
///     the current process is the first and only after the last use of DB.
///   MDBX_RESULT_FALSE (0) - if a shared lock was acquired and thus
///     DB has already been opened and now is used by other processes.
///   Otherwise (not 0 and not -1) - error code.
MDBX_INTERNAL_FUNC int osal_lck_seize(MDBX_env *env);

/// \brief Downgrades the level of initially acquired lock to
///   operational level specified by argument. The reason for such downgrade:
///    - unblocking of other processes that are waiting for access, i.e.
///      if (env->me_flags & MDBX_EXCLUSIVE) != 0, then other processes
///      should be made aware that access is unavailable rather than
///      wait for it.
///    - freeing locks that interfere file operation (especially for Windows)
///   (env->me_flags & MDBX_EXCLUSIVE) == 0 - downgrade to shared lock.
///   (env->me_flags & MDBX_EXCLUSIVE) != 0 - downgrade to exclusive
///   operational lock.
/// \return Error code or zero on success
MDBX_INTERNAL_FUNC int osal_lck_downgrade(MDBX_env *env);

/// \brief Locks LCK-file or/and table of readers for (de)registering.
/// \return Error code or zero on success
MDBX_INTERNAL_FUNC int osal_rdt_lock(MDBX_env *env);

/// \brief Unlocks LCK-file or/and table of readers after (de)registering.
MDBX_INTERNAL_FUNC void osal_rdt_unlock(MDBX_env *env);

/// \brief Acquires lock for DB change (on writing transaction start)
///   Reading transactions will not be blocked.
///   Declared as LIBMDBX_API because it is used in mdbx_chk.
/// \return Error code or zero on success
LIBMDBX_API int mdbx_txn_lock(MDBX_env *env, bool dont_wait);

/// \brief Releases lock once DB changes is made (after writing transaction
///   has finished).
///   Declared as LIBMDBX_API because it is used in mdbx_chk.
LIBMDBX_API void mdbx_txn_unlock(MDBX_env *env);

/// \brief Sets alive-flag of reader presence (indicative lock) for PID of
///   the current process. The function does no more than needed for
///   the correct working of osal_rpid_check() in other processes.
/// \return Error code or zero on success
MDBX_INTERNAL_FUNC int osal_rpid_set(MDBX_env *env);

/// \brief Resets alive-flag of reader presence (indicative lock)
///   for PID of the current process. The function does no more than needed
///   for the correct working of osal_rpid_check() in other processes.
/// \return Error code or zero on success
MDBX_INTERNAL_FUNC int osal_rpid_clear(MDBX_env *env);

/// \brief Checks for reading process status with the given pid with help of
///   alive-flag of presence (indicative lock) or using another way.
/// \return
///   MDBX_RESULT_TRUE (-1) - if the reader process with the given PID is alive
///     and working with DB (indicative lock is present).
///   MDBX_RESULT_FALSE (0) - if the reader process with the given PID is absent
///     or not working with DB (indicative lock is not present).
///   Otherwise (not 0 and not -1) - error code.
MDBX_INTERNAL_FUNC int osal_rpid_check(MDBX_env *env, uint32_t pid);

#if defined(_WIN32) || defined(_WIN64)

MDBX_INTERNAL_FUNC int osal_mb2w(const char *const src, wchar_t **const pdst);

typedef void(WINAPI *osal_srwlock_t_function)(osal_srwlock_t *);
MDBX_INTERNAL_VAR osal_srwlock_t_function osal_srwlock_Init,
    osal_srwlock_AcquireShared, osal_srwlock_ReleaseShared,
    osal_srwlock_AcquireExclusive, osal_srwlock_ReleaseExclusive;

#if _WIN32_WINNT < 0x0600 /* prior to Windows Vista */
typedef enum _FILE_INFO_BY_HANDLE_CLASS {
  FileBasicInfo,
  FileStandardInfo,
  FileNameInfo,
  FileRenameInfo,
  FileDispositionInfo,
  FileAllocationInfo,
  FileEndOfFileInfo,
  FileStreamInfo,
  FileCompressionInfo,
  FileAttributeTagInfo,
  FileIdBothDirectoryInfo,
  FileIdBothDirectoryRestartInfo,
  FileIoPriorityHintInfo,
  FileRemoteProtocolInfo,
  MaximumFileInfoByHandleClass
} FILE_INFO_BY_HANDLE_CLASS,
    *PFILE_INFO_BY_HANDLE_CLASS;

typedef struct _FILE_END_OF_FILE_INFO {
  LARGE_INTEGER EndOfFile;
} FILE_END_OF_FILE_INFO, *PFILE_END_OF_FILE_INFO;

#define REMOTE_PROTOCOL_INFO_FLAG_LOOPBACK 0x00000001
#define REMOTE_PROTOCOL_INFO_FLAG_OFFLINE 0x00000002

typedef struct _FILE_REMOTE_PROTOCOL_INFO {
  USHORT StructureVersion;
  USHORT StructureSize;
  DWORD Protocol;
  USHORT ProtocolMajorVersion;
  USHORT ProtocolMinorVersion;
  USHORT ProtocolRevision;
  USHORT Reserved;
  DWORD Flags;
  struct {
    DWORD Reserved[8];
  } GenericReserved;
  struct {
    DWORD Reserved[16];
  } ProtocolSpecificReserved;
} FILE_REMOTE_PROTOCOL_INFO, *PFILE_REMOTE_PROTOCOL_INFO;

#endif /* _WIN32_WINNT < 0x0600 (prior to Windows Vista) */

typedef BOOL(WINAPI *MDBX_GetFileInformationByHandleEx)(
    _In_ HANDLE hFile, _In_ FILE_INFO_BY_HANDLE_CLASS FileInformationClass,
    _Out_ LPVOID lpFileInformation, _In_ DWORD dwBufferSize);
MDBX_INTERNAL_VAR MDBX_GetFileInformationByHandleEx
    mdbx_GetFileInformationByHandleEx;

typedef BOOL(WINAPI *MDBX_GetVolumeInformationByHandleW)(
    _In_ HANDLE hFile, _Out_opt_ LPWSTR lpVolumeNameBuffer,
    _In_ DWORD nVolumeNameSize, _Out_opt_ LPDWORD lpVolumeSerialNumber,
    _Out_opt_ LPDWORD lpMaximumComponentLength,
    _Out_opt_ LPDWORD lpFileSystemFlags,
    _Out_opt_ LPWSTR lpFileSystemNameBuffer, _In_ DWORD nFileSystemNameSize);
MDBX_INTERNAL_VAR MDBX_GetVolumeInformationByHandleW
    mdbx_GetVolumeInformationByHandleW;

typedef DWORD(WINAPI *MDBX_GetFinalPathNameByHandleW)(_In_ HANDLE hFile,
                                                      _Out_ LPWSTR lpszFilePath,
                                                      _In_ DWORD cchFilePath,
                                                      _In_ DWORD dwFlags);
MDBX_INTERNAL_VAR MDBX_GetFinalPathNameByHandleW mdbx_GetFinalPathNameByHandleW;

typedef BOOL(WINAPI *MDBX_SetFileInformationByHandle)(
    _In_ HANDLE hFile, _In_ FILE_INFO_BY_HANDLE_CLASS FileInformationClass,
    _Out_ LPVOID lpFileInformation, _In_ DWORD dwBufferSize);
MDBX_INTERNAL_VAR MDBX_SetFileInformationByHandle
    mdbx_SetFileInformationByHandle;

typedef NTSTATUS(NTAPI *MDBX_NtFsControlFile)(
    IN HANDLE FileHandle, IN OUT HANDLE Event,
    IN OUT PVOID /* PIO_APC_ROUTINE */ ApcRoutine, IN OUT PVOID ApcContext,
    OUT PIO_STATUS_BLOCK IoStatusBlock, IN ULONG FsControlCode,
    IN OUT PVOID InputBuffer, IN ULONG InputBufferLength,
    OUT OPTIONAL PVOID OutputBuffer, IN ULONG OutputBufferLength);
MDBX_INTERNAL_VAR MDBX_NtFsControlFile mdbx_NtFsControlFile;

typedef uint64_t(WINAPI *MDBX_GetTickCount64)(void);
MDBX_INTERNAL_VAR MDBX_GetTickCount64 mdbx_GetTickCount64;

#if !defined(_WIN32_WINNT_WIN8) || _WIN32_WINNT < _WIN32_WINNT_WIN8
typedef struct _WIN32_MEMORY_RANGE_ENTRY {
  PVOID VirtualAddress;
  SIZE_T NumberOfBytes;
} WIN32_MEMORY_RANGE_ENTRY, *PWIN32_MEMORY_RANGE_ENTRY;
#endif /* Windows 8.x */

typedef BOOL(WINAPI *MDBX_PrefetchVirtualMemory)(
    HANDLE hProcess, ULONG_PTR NumberOfEntries,
    PWIN32_MEMORY_RANGE_ENTRY VirtualAddresses, ULONG Flags);
MDBX_INTERNAL_VAR MDBX_PrefetchVirtualMemory mdbx_PrefetchVirtualMemory;

typedef enum _SECTION_INHERIT { ViewShare = 1, ViewUnmap = 2 } SECTION_INHERIT;

typedef NTSTATUS(NTAPI *MDBX_NtExtendSection)(IN HANDLE SectionHandle,
                                              IN PLARGE_INTEGER NewSectionSize);
MDBX_INTERNAL_VAR MDBX_NtExtendSection mdbx_NtExtendSection;

static __inline bool mdbx_RunningUnderWine(void) {
  return !mdbx_NtExtendSection;
}

typedef LSTATUS(WINAPI *MDBX_RegGetValueA)(HKEY hkey, LPCSTR lpSubKey,
                                           LPCSTR lpValue, DWORD dwFlags,
                                           LPDWORD pdwType, PVOID pvData,
                                           LPDWORD pcbData);
MDBX_INTERNAL_VAR MDBX_RegGetValueA mdbx_RegGetValueA;

NTSYSAPI ULONG RtlRandomEx(PULONG Seed);

typedef BOOL(WINAPI *MDBX_SetFileIoOverlappedRange)(HANDLE FileHandle,
                                                    PUCHAR OverlappedRangeStart,
                                                    ULONG Length);
MDBX_INTERNAL_VAR MDBX_SetFileIoOverlappedRange mdbx_SetFileIoOverlappedRange;

#endif /* Windows */

#endif /* !__cplusplus */

/*----------------------------------------------------------------------------*/

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static __always_inline uint64_t
osal_bswap64(uint64_t v) {
#if __GNUC_PREREQ(4, 4) || __CLANG_PREREQ(4, 0) ||                             \
    __has_builtin(__builtin_bswap64)
  return __builtin_bswap64(v);
#elif defined(_MSC_VER) && !defined(__clang__)
  return _byteswap_uint64(v);
#elif defined(__bswap_64)
  return __bswap_64(v);
#elif defined(bswap_64)
  return bswap_64(v);
#else
  return v << 56 | v >> 56 | ((v << 40) & UINT64_C(0x00ff000000000000)) |
         ((v << 24) & UINT64_C(0x0000ff0000000000)) |
         ((v << 8) & UINT64_C(0x000000ff00000000)) |
         ((v >> 8) & UINT64_C(0x00000000ff000000)) |
         ((v >> 24) & UINT64_C(0x0000000000ff0000)) |
         ((v >> 40) & UINT64_C(0x000000000000ff00));
#endif
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static __always_inline uint32_t
osal_bswap32(uint32_t v) {
#if __GNUC_PREREQ(4, 4) || __CLANG_PREREQ(4, 0) ||                             \
    __has_builtin(__builtin_bswap32)
  return __builtin_bswap32(v);
#elif defined(_MSC_VER) && !defined(__clang__)
  return _byteswap_ulong(v);
#elif defined(__bswap_32)
  return __bswap_32(v);
#elif defined(bswap_32)
  return bswap_32(v);
#else
  return v << 24 | v >> 24 | ((v << 8) & UINT32_C(0x00ff0000)) |
         ((v >> 8) & UINT32_C(0x0000ff00));
#endif
}

/*----------------------------------------------------------------------------*/

#if defined(_MSC_VER) && _MSC_VER >= 1900
/* LY: MSVC 2015/2017/2019 has buggy/inconsistent PRIuPTR/PRIxPTR macros
 * for internal format-args checker. */
#undef PRIuPTR
#undef PRIiPTR
#undef PRIdPTR
#undef PRIxPTR
#define PRIuPTR "Iu"
#define PRIiPTR "Ii"
#define PRIdPTR "Id"
#define PRIxPTR "Ix"
#define PRIuSIZE "zu"
#define PRIiSIZE "zi"
#define PRIdSIZE "zd"
#define PRIxSIZE "zx"
#endif /* fix PRI*PTR for _MSC_VER */

#ifndef PRIuSIZE
#define PRIuSIZE PRIuPTR
#define PRIiSIZE PRIiPTR
#define PRIdSIZE PRIdPTR
#define PRIxSIZE PRIxPTR
#endif /* PRI*SIZE macros for MSVC */

#ifdef _MSC_VER
#pragma warning(pop)
#endif

#define mdbx_sourcery_anchor XCONCAT(mdbx_sourcery_, MDBX_BUILD_SOURCERY)
#if defined(xMDBX_TOOLS)
extern LIBMDBX_API const char *const mdbx_sourcery_anchor;
#endif

/*******************************************************************************
 *******************************************************************************
 *******************************************************************************
 *
 *
 *         ####   #####    #####     #     ####   #    #   ####
 *        #    #  #    #     #       #    #    #  ##   #  #
 *        #    #  #    #     #       #    #    #  # #  #   ####
 *        #    #  #####      #       #    #    #  #  # #       #
 *        #    #  #          #       #    #    #  #   ##  #    #
 *         ####   #          #       #     ####   #    #   ####
 *
 *
 */

/** \defgroup build_option Build options
 * The libmdbx build options.
 @{ */

/** Using fcntl(F_FULLFSYNC) with 5-10 times slowdown */
#define MDBX_OSX_WANNA_DURABILITY 0
/** Using fsync() with chance of data lost on power failure */
#define MDBX_OSX_WANNA_SPEED 1

#ifndef MDBX_OSX_SPEED_INSTEADOF_DURABILITY
/** Choices \ref MDBX_OSX_WANNA_DURABILITY or \ref MDBX_OSX_WANNA_SPEED
 * for OSX & iOS */
#define MDBX_OSX_SPEED_INSTEADOF_DURABILITY MDBX_OSX_WANNA_DURABILITY
#endif /* MDBX_OSX_SPEED_INSTEADOF_DURABILITY */

/** Controls checking PID against reuse DB environment after the fork() */
#ifndef MDBX_ENV_CHECKPID
#if defined(MADV_DONTFORK) || defined(_WIN32) || defined(_WIN64)
/* PID check could be omitted:
 *  - on Linux when madvise(MADV_DONTFORK) is available, i.e. after the fork()
 *    mapped pages will not be available for child process.
 *  - in Windows where fork() not available. */
#define MDBX_ENV_CHECKPID 0
#else
#define MDBX_ENV_CHECKPID 1
#endif
#define MDBX_ENV_CHECKPID_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_ENV_CHECKPID)
#elif !(MDBX_ENV_CHECKPID == 0 || MDBX_ENV_CHECKPID == 1)
#error MDBX_ENV_CHECKPID must be defined as 0 or 1
#else
#define MDBX_ENV_CHECKPID_CONFIG MDBX_STRINGIFY(MDBX_ENV_CHECKPID)
#endif /* MDBX_ENV_CHECKPID */

/** Controls checking transaction owner thread against misuse transactions from
 * other threads. */
#ifndef MDBX_TXN_CHECKOWNER
#define MDBX_TXN_CHECKOWNER 1
#define MDBX_TXN_CHECKOWNER_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_TXN_CHECKOWNER)
#elif !(MDBX_TXN_CHECKOWNER == 0 || MDBX_TXN_CHECKOWNER == 1)
#error MDBX_TXN_CHECKOWNER must be defined as 0 or 1
#else
#define MDBX_TXN_CHECKOWNER_CONFIG MDBX_STRINGIFY(MDBX_TXN_CHECKOWNER)
#endif /* MDBX_TXN_CHECKOWNER */

/** Does a system have battery-backed Real-Time Clock or just a fake. */
#ifndef MDBX_TRUST_RTC
#if defined(__linux__) || defined(__gnu_linux__) || defined(__NetBSD__) ||     \
    defined(__OpenBSD__)
#define MDBX_TRUST_RTC 0 /* a lot of embedded systems have a fake RTC */
#else
#define MDBX_TRUST_RTC 1
#endif
#define MDBX_TRUST_RTC_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_TRUST_RTC)
#elif !(MDBX_TRUST_RTC == 0 || MDBX_TRUST_RTC == 1)
#error MDBX_TRUST_RTC must be defined as 0 or 1
#else
#define MDBX_TRUST_RTC_CONFIG MDBX_STRINGIFY(MDBX_TRUST_RTC)
#endif /* MDBX_TRUST_RTC */

/** Controls online database auto-compactification during write-transactions. */
#ifndef MDBX_ENABLE_REFUND
#define MDBX_ENABLE_REFUND 1
#elif !(MDBX_ENABLE_REFUND == 0 || MDBX_ENABLE_REFUND == 1)
#error MDBX_ENABLE_REFUND must be defined as 0 or 1
#endif /* MDBX_ENABLE_REFUND */

/** Controls profiling of GC search and updates. */
#ifndef MDBX_ENABLE_PROFGC
#define MDBX_ENABLE_PROFGC 0
#elif !(MDBX_ENABLE_PROFGC == 0 || MDBX_ENABLE_PROFGC == 1)
#error MDBX_ENABLE_PROFGC must be defined as 0 or 1
#endif /* MDBX_ENABLE_PROFGC */

/** Controls gathering statistics for page operations. */
#ifndef MDBX_ENABLE_PGOP_STAT
#define MDBX_ENABLE_PGOP_STAT 1
#elif !(MDBX_ENABLE_PGOP_STAT == 0 || MDBX_ENABLE_PGOP_STAT == 1)
#error MDBX_ENABLE_PGOP_STAT must be defined as 0 or 1
#endif /* MDBX_ENABLE_PGOP_STAT */

/** Controls using Unix' mincore() to determine whether DB-pages
 * are resident in memory. */
#ifndef MDBX_ENABLE_MINCORE
#if MDBX_ENABLE_PREFAULT &&                                                    \
    (defined(MINCORE_INCORE) || !(defined(_WIN32) || defined(_WIN64)))
#define MDBX_ENABLE_MINCORE 1
#else
#define MDBX_ENABLE_MINCORE 0
#endif
#elif !(MDBX_ENABLE_MINCORE == 0 || MDBX_ENABLE_MINCORE == 1)
#error MDBX_ENABLE_MINCORE must be defined as 0 or 1
#endif /* MDBX_ENABLE_MINCORE */

/** Enables chunking long list of retired pages during huge transactions commit
 * to avoid use sequences of pages. */
#ifndef MDBX_ENABLE_BIGFOOT
#if MDBX_WORDBITS >= 64 || defined(DOXYGEN)
#define MDBX_ENABLE_BIGFOOT 1
#else
#define MDBX_ENABLE_BIGFOOT 0
#endif
#elif !(MDBX_ENABLE_BIGFOOT == 0 || MDBX_ENABLE_BIGFOOT == 1)
#error MDBX_ENABLE_BIGFOOT must be defined as 0 or 1
#endif /* MDBX_ENABLE_BIGFOOT */

/** Controls using of POSIX' madvise() and/or similar hints. */
#ifndef MDBX_ENABLE_MADVISE
#define MDBX_ENABLE_MADVISE 1
#elif !(MDBX_ENABLE_MADVISE == 0 || MDBX_ENABLE_MADVISE == 1)
#error MDBX_ENABLE_MADVISE must be defined as 0 or 1
#endif /* MDBX_ENABLE_MADVISE */

/** Disable some checks to reduce an overhead and detection probability of
 * database corruption to a values closer to the LMDB. */
#ifndef MDBX_DISABLE_VALIDATION
#define MDBX_DISABLE_VALIDATION 0
#elif !(MDBX_DISABLE_VALIDATION == 0 || MDBX_DISABLE_VALIDATION == 1)
#error MDBX_DISABLE_VALIDATION must be defined as 0 or 1
#endif /* MDBX_DISABLE_VALIDATION */

#ifndef MDBX_PNL_PREALLOC_FOR_RADIXSORT
#define MDBX_PNL_PREALLOC_FOR_RADIXSORT 1
#elif !(MDBX_PNL_PREALLOC_FOR_RADIXSORT == 0 ||                                \
        MDBX_PNL_PREALLOC_FOR_RADIXSORT == 1)
#error MDBX_PNL_PREALLOC_FOR_RADIXSORT must be defined as 0 or 1
#endif /* MDBX_PNL_PREALLOC_FOR_RADIXSORT */

#ifndef MDBX_DPL_PREALLOC_FOR_RADIXSORT
#define MDBX_DPL_PREALLOC_FOR_RADIXSORT 1
#elif !(MDBX_DPL_PREALLOC_FOR_RADIXSORT == 0 ||                                \
        MDBX_DPL_PREALLOC_FOR_RADIXSORT == 1)
#error MDBX_DPL_PREALLOC_FOR_RADIXSORT must be defined as 0 or 1
#endif /* MDBX_DPL_PREALLOC_FOR_RADIXSORT */

/** Controls dirty pages tracking, spilling and persisting in MDBX_WRITEMAP
 * mode. 0/OFF = Don't track dirty pages at all, don't spill ones, and use
 * msync() to persist data. This is by-default on Linux and other systems where
 * kernel provides properly LRU tracking and effective flushing on-demand. 1/ON
 * = Tracking of dirty pages but with LRU labels for spilling and explicit
 * persist ones by write(). This may be reasonable for systems which low
 * performance of msync() and/or LRU tracking. */
#ifndef MDBX_AVOID_MSYNC
#if defined(_WIN32) || defined(_WIN64)
#define MDBX_AVOID_MSYNC 1
#else
#define MDBX_AVOID_MSYNC 0
#endif
#elif !(MDBX_AVOID_MSYNC == 0 || MDBX_AVOID_MSYNC == 1)
#error MDBX_AVOID_MSYNC must be defined as 0 or 1
#endif /* MDBX_AVOID_MSYNC */

/** Controls sort order of internal page number lists.
 * This mostly experimental/advanced option with not for regular MDBX users.
 * \warning The database format depend on this option and libmdbx built with
 * different option value are incompatible. */
#ifndef MDBX_PNL_ASCENDING
#define MDBX_PNL_ASCENDING 0
#elif !(MDBX_PNL_ASCENDING == 0 || MDBX_PNL_ASCENDING == 1)
#error MDBX_PNL_ASCENDING must be defined as 0 or 1
#endif /* MDBX_PNL_ASCENDING */

/** Avoid dependence from MSVC CRT and use ntdll.dll instead. */
#ifndef MDBX_WITHOUT_MSVC_CRT
#define MDBX_WITHOUT_MSVC_CRT 1
#elif !(MDBX_WITHOUT_MSVC_CRT == 0 || MDBX_WITHOUT_MSVC_CRT == 1)
#error MDBX_WITHOUT_MSVC_CRT must be defined as 0 or 1
#endif /* MDBX_WITHOUT_MSVC_CRT */

/** Size of buffer used during copying a environment/database file. */
#ifndef MDBX_ENVCOPY_WRITEBUF
#define MDBX_ENVCOPY_WRITEBUF 1048576u
#elif MDBX_ENVCOPY_WRITEBUF < 65536u || MDBX_ENVCOPY_WRITEBUF > 1073741824u || \
    MDBX_ENVCOPY_WRITEBUF % 65536u
#error MDBX_ENVCOPY_WRITEBUF must be defined in range 65536..1073741824 and be multiple of 65536
#endif /* MDBX_ENVCOPY_WRITEBUF */

/** Forces assertion checking */
#ifndef MDBX_FORCE_ASSERTIONS
#define MDBX_FORCE_ASSERTIONS 0
#elif !(MDBX_FORCE_ASSERTIONS == 0 || MDBX_FORCE_ASSERTIONS == 1)
#error MDBX_FORCE_ASSERTIONS must be defined as 0 or 1
#endif /* MDBX_FORCE_ASSERTIONS */

/** Presumed malloc size overhead for each allocation
 * to adjust allocations to be more aligned. */
#ifndef MDBX_ASSUME_MALLOC_OVERHEAD
#ifdef __SIZEOF_POINTER__
#define MDBX_ASSUME_MALLOC_OVERHEAD (__SIZEOF_POINTER__ * 2u)
#else
#define MDBX_ASSUME_MALLOC_OVERHEAD (sizeof(void *) * 2u)
#endif
#elif MDBX_ASSUME_MALLOC_OVERHEAD < 0 || MDBX_ASSUME_MALLOC_OVERHEAD > 64 ||   \
    MDBX_ASSUME_MALLOC_OVERHEAD % 4
#error MDBX_ASSUME_MALLOC_OVERHEAD must be defined in range 0..64 and be multiple of 4
#endif /* MDBX_ASSUME_MALLOC_OVERHEAD */

/** If defined then enables integration with Valgrind,
 * a memory analyzing tool. */
#ifndef MDBX_USE_VALGRIND
#endif /* MDBX_USE_VALGRIND */

/** If defined then enables use C11 atomics,
 *  otherwise detects ones availability automatically. */
#ifndef MDBX_HAVE_C11ATOMICS
#endif /* MDBX_HAVE_C11ATOMICS */

/** If defined then enables use the GCC's `__builtin_cpu_supports()`
 * for runtime dispatching depending on the CPU's capabilities.
 * \note Defining `MDBX_HAVE_BUILTIN_CPU_SUPPORTS` to `0` should avoided unless
 * build for particular single-target platform, since on AMD64/x86 this disables
 * dynamic choice (at runtime) of SSE2 / AVX2 / AVX512 instructions
 * with fallback to non-accelerated baseline code. */
#ifndef MDBX_HAVE_BUILTIN_CPU_SUPPORTS
#if defined(__APPLE__) || defined(BIONIC)
/* Never use any modern features on Apple's or Google's OSes
 * since a lot of troubles with compatibility and/or performance */
#define MDBX_HAVE_BUILTIN_CPU_SUPPORTS 0
#elif defined(__e2k__)
#define MDBX_HAVE_BUILTIN_CPU_SUPPORTS 0
#elif __has_builtin(__builtin_cpu_supports) ||                                 \
    defined(__BUILTIN_CPU_SUPPORTS__) ||                                       \
    (defined(__ia32__) && __GNUC_PREREQ(4, 8) && __GLIBC_PREREQ(2, 23))
#define MDBX_HAVE_BUILTIN_CPU_SUPPORTS 1
#else
#define MDBX_HAVE_BUILTIN_CPU_SUPPORTS 0
#endif
#elif !(MDBX_HAVE_BUILTIN_CPU_SUPPORTS == 0 ||                                 \
        MDBX_HAVE_BUILTIN_CPU_SUPPORTS == 1)
#error MDBX_HAVE_BUILTIN_CPU_SUPPORTS must be defined as 0 or 1
#endif /* MDBX_HAVE_BUILTIN_CPU_SUPPORTS */

//------------------------------------------------------------------------------

/** Win32 File Locking API for \ref MDBX_LOCKING */
#define MDBX_LOCKING_WIN32FILES -1

/** SystemV IPC semaphores for \ref MDBX_LOCKING */
#define MDBX_LOCKING_SYSV 5

/** POSIX-1 Shared anonymous semaphores for \ref MDBX_LOCKING */
#define MDBX_LOCKING_POSIX1988 1988

/** POSIX-2001 Shared Mutexes for \ref MDBX_LOCKING */
#define MDBX_LOCKING_POSIX2001 2001

/** POSIX-2008 Robust Mutexes for \ref MDBX_LOCKING */
#define MDBX_LOCKING_POSIX2008 2008

/** BeOS Benaphores, aka Futexes for \ref MDBX_LOCKING */
#define MDBX_LOCKING_BENAPHORE 1995

/** Advanced: Choices the locking implementation (autodetection by default). */
#if defined(_WIN32) || defined(_WIN64)
#define MDBX_LOCKING MDBX_LOCKING_WIN32FILES
#else
#ifndef MDBX_LOCKING
#if defined(_POSIX_THREAD_PROCESS_SHARED) &&                                   \
    _POSIX_THREAD_PROCESS_SHARED >= 200112L && !defined(__FreeBSD__)

/* Some platforms define the EOWNERDEAD error code even though they
 * don't support Robust Mutexes. If doubt compile with -MDBX_LOCKING=2001. */
#if defined(EOWNERDEAD) && _POSIX_THREAD_PROCESS_SHARED >= 200809L &&          \
    ((defined(_POSIX_THREAD_ROBUST_PRIO_INHERIT) &&                            \
      _POSIX_THREAD_ROBUST_PRIO_INHERIT > 0) ||                                \
     (defined(_POSIX_THREAD_ROBUST_PRIO_PROTECT) &&                            \
      _POSIX_THREAD_ROBUST_PRIO_PROTECT > 0) ||                                \
     defined(PTHREAD_MUTEX_ROBUST) || defined(PTHREAD_MUTEX_ROBUST_NP)) &&     \
    (!defined(__GLIBC__) ||                                                    \
     __GLIBC_PREREQ(2, 10) /* troubles with Robust mutexes before 2.10 */)
#define MDBX_LOCKING MDBX_LOCKING_POSIX2008
#else
#define MDBX_LOCKING MDBX_LOCKING_POSIX2001
#endif
#elif defined(__sun) || defined(__SVR4) || defined(__svr4__)
#define MDBX_LOCKING MDBX_LOCKING_POSIX1988
#else
#define MDBX_LOCKING MDBX_LOCKING_SYSV
#endif
#define MDBX_LOCKING_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_LOCKING)
#else
#define MDBX_LOCKING_CONFIG MDBX_STRINGIFY(MDBX_LOCKING)
#endif /* MDBX_LOCKING */
#endif /* !Windows */

/** Advanced: Using POSIX OFD-locks (autodetection by default). */
#ifndef MDBX_USE_OFDLOCKS
#if ((defined(F_OFD_SETLK) && defined(F_OFD_SETLKW) &&                         \
      defined(F_OFD_GETLK)) ||                                                 \
     (defined(F_OFD_SETLK64) && defined(F_OFD_SETLKW64) &&                     \
      defined(F_OFD_GETLK64))) &&                                              \
    !defined(MDBX_SAFE4QEMU) &&                                                \
    !defined(__sun) /* OFD-lock are broken on Solaris */
#define MDBX_USE_OFDLOCKS 1
#else
#define MDBX_USE_OFDLOCKS 0
#endif
#define MDBX_USE_OFDLOCKS_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_USE_OFDLOCKS)
#elif !(MDBX_USE_OFDLOCKS == 0 || MDBX_USE_OFDLOCKS == 1)
#error MDBX_USE_OFDLOCKS must be defined as 0 or 1
#else
#define MDBX_USE_OFDLOCKS_CONFIG MDBX_STRINGIFY(MDBX_USE_OFDLOCKS)
#endif /* MDBX_USE_OFDLOCKS */

/** Advanced: Using sendfile() syscall (autodetection by default). */
#ifndef MDBX_USE_SENDFILE
#if ((defined(__linux__) || defined(__gnu_linux__)) &&                         \
     !defined(__ANDROID_API__)) ||                                             \
    (defined(__ANDROID_API__) && __ANDROID_API__ >= 21)
#define MDBX_USE_SENDFILE 1
#else
#define MDBX_USE_SENDFILE 0
#endif
#elif !(MDBX_USE_SENDFILE == 0 || MDBX_USE_SENDFILE == 1)
#error MDBX_USE_SENDFILE must be defined as 0 or 1
#endif /* MDBX_USE_SENDFILE */

/** Advanced: Using copy_file_range() syscall (autodetection by default). */
#ifndef MDBX_USE_COPYFILERANGE
#if __GLIBC_PREREQ(2, 27) && defined(_GNU_SOURCE)
#define MDBX_USE_COPYFILERANGE 1
#else
#define MDBX_USE_COPYFILERANGE 0
#endif
#elif !(MDBX_USE_COPYFILERANGE == 0 || MDBX_USE_COPYFILERANGE == 1)
#error MDBX_USE_COPYFILERANGE must be defined as 0 or 1
#endif /* MDBX_USE_COPYFILERANGE */

/** Advanced: Using sync_file_range() syscall (autodetection by default). */
#ifndef MDBX_USE_SYNCFILERANGE
#if ((defined(__linux__) || defined(__gnu_linux__)) &&                         \
     defined(SYNC_FILE_RANGE_WRITE) && !defined(__ANDROID_API__)) ||           \
    (defined(__ANDROID_API__) && __ANDROID_API__ >= 26)
#define MDBX_USE_SYNCFILERANGE 1
#else
#define MDBX_USE_SYNCFILERANGE 0
#endif
#elif !(MDBX_USE_SYNCFILERANGE == 0 || MDBX_USE_SYNCFILERANGE == 1)
#error MDBX_USE_SYNCFILERANGE must be defined as 0 or 1
#endif /* MDBX_USE_SYNCFILERANGE */

//------------------------------------------------------------------------------

#ifndef MDBX_CPU_WRITEBACK_INCOHERENT
#if defined(__ia32__) || defined(__e2k__) || defined(__hppa) ||                \
    defined(__hppa__) || defined(DOXYGEN)
#define MDBX_CPU_WRITEBACK_INCOHERENT 0
#else
#define MDBX_CPU_WRITEBACK_INCOHERENT 1
#endif
#elif !(MDBX_CPU_WRITEBACK_INCOHERENT == 0 ||                                  \
        MDBX_CPU_WRITEBACK_INCOHERENT == 1)
#error MDBX_CPU_WRITEBACK_INCOHERENT must be defined as 0 or 1
#endif /* MDBX_CPU_WRITEBACK_INCOHERENT */

#ifndef MDBX_MMAP_INCOHERENT_FILE_WRITE
#ifdef __OpenBSD__
#define MDBX_MMAP_INCOHERENT_FILE_WRITE 1
#else
#define MDBX_MMAP_INCOHERENT_FILE_WRITE 0
#endif
#elif !(MDBX_MMAP_INCOHERENT_FILE_WRITE == 0 ||                                \
        MDBX_MMAP_INCOHERENT_FILE_WRITE == 1)
#error MDBX_MMAP_INCOHERENT_FILE_WRITE must be defined as 0 or 1
#endif /* MDBX_MMAP_INCOHERENT_FILE_WRITE */

#ifndef MDBX_MMAP_INCOHERENT_CPU_CACHE
#if defined(__mips) || defined(__mips__) || defined(__mips64) ||               \
    defined(__mips64__) || defined(_M_MRX000) || defined(_MIPS_) ||            \
    defined(__MWERKS__) || defined(__sgi)
/* MIPS has cache coherency issues. */
#define MDBX_MMAP_INCOHERENT_CPU_CACHE 1
#else
/* LY: assume no relevant mmap/dcache issues. */
#define MDBX_MMAP_INCOHERENT_CPU_CACHE 0
#endif
#elif !(MDBX_MMAP_INCOHERENT_CPU_CACHE == 0 ||                                 \
        MDBX_MMAP_INCOHERENT_CPU_CACHE == 1)
#error MDBX_MMAP_INCOHERENT_CPU_CACHE must be defined as 0 or 1
#endif /* MDBX_MMAP_INCOHERENT_CPU_CACHE */

#ifndef MDBX_MMAP_USE_MS_ASYNC
#if MDBX_MMAP_INCOHERENT_FILE_WRITE || MDBX_MMAP_INCOHERENT_CPU_CACHE
#define MDBX_MMAP_USE_MS_ASYNC 1
#else
#define MDBX_MMAP_USE_MS_ASYNC 0
#endif
#elif !(MDBX_MMAP_USE_MS_ASYNC == 0 || MDBX_MMAP_USE_MS_ASYNC == 1)
#error MDBX_MMAP_USE_MS_ASYNC must be defined as 0 or 1
#endif /* MDBX_MMAP_USE_MS_ASYNC */

#ifndef MDBX_64BIT_ATOMIC
#if MDBX_WORDBITS >= 64 || defined(DOXYGEN)
#define MDBX_64BIT_ATOMIC 1
#else
#define MDBX_64BIT_ATOMIC 0
#endif
#define MDBX_64BIT_ATOMIC_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_64BIT_ATOMIC)
#elif !(MDBX_64BIT_ATOMIC == 0 || MDBX_64BIT_ATOMIC == 1)
#error MDBX_64BIT_ATOMIC must be defined as 0 or 1
#else
#define MDBX_64BIT_ATOMIC_CONFIG MDBX_STRINGIFY(MDBX_64BIT_ATOMIC)
#endif /* MDBX_64BIT_ATOMIC */

#ifndef MDBX_64BIT_CAS
#if defined(__GCC_ATOMIC_LLONG_LOCK_FREE)
#if __GCC_ATOMIC_LLONG_LOCK_FREE > 1
#define MDBX_64BIT_CAS 1
#else
#define MDBX_64BIT_CAS 0
#endif
#elif defined(__CLANG_ATOMIC_LLONG_LOCK_FREE)
#if __CLANG_ATOMIC_LLONG_LOCK_FREE > 1
#define MDBX_64BIT_CAS 1
#else
#define MDBX_64BIT_CAS 0
#endif
#elif defined(ATOMIC_LLONG_LOCK_FREE)
#if ATOMIC_LLONG_LOCK_FREE > 1
#define MDBX_64BIT_CAS 1
#else
#define MDBX_64BIT_CAS 0
#endif
#elif defined(_MSC_VER) || defined(__APPLE__) || defined(DOXYGEN)
#define MDBX_64BIT_CAS 1
#elif !(MDBX_64BIT_CAS == 0 || MDBX_64BIT_CAS == 1)
#error MDBX_64BIT_CAS must be defined as 0 or 1
#else
#define MDBX_64BIT_CAS MDBX_64BIT_ATOMIC
#endif
#define MDBX_64BIT_CAS_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_64BIT_CAS)
#else
#define MDBX_64BIT_CAS_CONFIG MDBX_STRINGIFY(MDBX_64BIT_CAS)
#endif /* MDBX_64BIT_CAS */

#ifndef MDBX_UNALIGNED_OK
#if defined(__ALIGNED__) || defined(__SANITIZE_UNDEFINED__) ||                 \
    defined(ENABLE_UBSAN)
#define MDBX_UNALIGNED_OK 0 /* no unaligned access allowed */
#elif defined(__ARM_FEATURE_UNALIGNED)
#define MDBX_UNALIGNED_OK 4 /* ok unaligned for 32-bit words */
#elif defined(__e2k__) || defined(__elbrus__)
#if __iset__ > 4
#define MDBX_UNALIGNED_OK 8 /* ok unaligned for 64-bit words */
#else
#define MDBX_UNALIGNED_OK 4 /* ok unaligned for 32-bit words */
#endif
#elif defined(__ia32__)
#define MDBX_UNALIGNED_OK 8 /* ok unaligned for 64-bit words */
#elif __CLANG_PREREQ(5, 0) || __GNUC_PREREQ(5, 0)
/* expecting an optimization will well done, also this
 * hushes false-positives from UBSAN (undefined behaviour sanitizer) */
#define MDBX_UNALIGNED_OK 0
#else
#define MDBX_UNALIGNED_OK 0 /* no unaligned access allowed */
#endif
#elif MDBX_UNALIGNED_OK == 1
#undef MDBX_UNALIGNED_OK
#define MDBX_UNALIGNED_OK 32 /* any unaligned access allowed */
#endif                       /* MDBX_UNALIGNED_OK */

#ifndef MDBX_CACHELINE_SIZE
#if defined(SYSTEM_CACHE_ALIGNMENT_SIZE)
#define MDBX_CACHELINE_SIZE SYSTEM_CACHE_ALIGNMENT_SIZE
#elif defined(__ia64__) || defined(__ia64) || defined(_M_IA64)
#define MDBX_CACHELINE_SIZE 128
#else
#define MDBX_CACHELINE_SIZE 64
#endif
#endif /* MDBX_CACHELINE_SIZE */

/** @} end of build options */
/*******************************************************************************
 *******************************************************************************
 ******************************************************************************/

#ifndef DOXYGEN

/* In case the MDBX_DEBUG is undefined set it corresponding to NDEBUG */
#ifndef MDBX_DEBUG
#ifdef NDEBUG
#define MDBX_DEBUG 0
#else
#define MDBX_DEBUG 1
#endif
#endif /* MDBX_DEBUG */

#else

/* !!! Actually this is a fake definitions for Doxygen !!! */

/** Controls enabling of debugging features.
 *
 *  - `MDBX_DEBUG = 0` (by default) Disables any debugging features at all,
 *                     including logging and assertion controls.
 *                     Logging level and corresponding debug flags changing
 *                     by \ref mdbx_setup_debug() will not have effect.
 *  - `MDBX_DEBUG > 0` Enables code for the debugging features (logging,
 *                     assertions checking and internal audit).
 *                     Simultaneously sets the default logging level
 *                     to the `MDBX_DEBUG` value.
 *                     Also enables \ref MDBX_DBG_AUDIT if `MDBX_DEBUG >= 2`.
 *
 * \ingroup build_option */
#define MDBX_DEBUG 0...7

/** Disables using of GNU libc extensions. */
#define MDBX_DISABLE_GNU_SOURCE 0 or 1

#endif /* DOXYGEN */

/* Undefine the NDEBUG if debugging is enforced by MDBX_DEBUG */
#if MDBX_DEBUG
#undef NDEBUG
#endif

#ifndef __cplusplus
/*----------------------------------------------------------------------------*/
/* Debug and Logging stuff */

#define MDBX_RUNTIME_FLAGS_INIT                                                \
  ((MDBX_DEBUG) > 0) * MDBX_DBG_ASSERT + ((MDBX_DEBUG) > 1) * MDBX_DBG_AUDIT

extern uint8_t runtime_flags;
extern uint8_t loglevel;
extern MDBX_debug_func *debug_logger;

MDBX_MAYBE_UNUSED static __inline void jitter4testing(bool tiny) {
#if MDBX_DEBUG
  if (MDBX_DBG_JITTER & runtime_flags)
    osal_jitter(tiny);
#else
  (void)tiny;
#endif
}

MDBX_INTERNAL_FUNC void MDBX_PRINTF_ARGS(4, 5)
    debug_log(int level, const char *function, int line, const char *fmt, ...)
        MDBX_PRINTF_ARGS(4, 5);
MDBX_INTERNAL_FUNC void debug_log_va(int level, const char *function, int line,
                                     const char *fmt, va_list args);

#if MDBX_DEBUG
#define LOG_ENABLED(msg) unlikely(msg <= loglevel)
#define AUDIT_ENABLED() unlikely((runtime_flags & MDBX_DBG_AUDIT))
#else /* MDBX_DEBUG */
#define LOG_ENABLED(msg) (msg < MDBX_LOG_VERBOSE && msg <= loglevel)
#define AUDIT_ENABLED() (0)
#endif /* MDBX_DEBUG */

#if MDBX_FORCE_ASSERTIONS
#define ASSERT_ENABLED() (1)
#elif MDBX_DEBUG
#define ASSERT_ENABLED() likely((runtime_flags & MDBX_DBG_ASSERT))
#else
#define ASSERT_ENABLED() (0)
#endif /* assertions */

#define DEBUG_EXTRA(fmt, ...)                                                  \
  do {                                                                         \
    if (LOG_ENABLED(MDBX_LOG_EXTRA))                                           \
      debug_log(MDBX_LOG_EXTRA, __func__, __LINE__, fmt, __VA_ARGS__);         \
  } while (0)

#define DEBUG_EXTRA_PRINT(fmt, ...)                                            \
  do {                                                                         \
    if (LOG_ENABLED(MDBX_LOG_EXTRA))                                           \
      debug_log(MDBX_LOG_EXTRA, NULL, 0, fmt, __VA_ARGS__);                    \
  } while (0)

#define TRACE(fmt, ...)                                                        \
  do {                                                                         \
    if (LOG_ENABLED(MDBX_LOG_TRACE))                                           \
      debug_log(MDBX_LOG_TRACE, __func__, __LINE__, fmt "\n", __VA_ARGS__);    \
  } while (0)

#define DEBUG(fmt, ...)                                                        \
  do {                                                                         \
    if (LOG_ENABLED(MDBX_LOG_DEBUG))                                           \
      debug_log(MDBX_LOG_DEBUG, __func__, __LINE__, fmt "\n", __VA_ARGS__);    \
  } while (0)

#define VERBOSE(fmt, ...)                                                      \
  do {                                                                         \
    if (LOG_ENABLED(MDBX_LOG_VERBOSE))                                         \
      debug_log(MDBX_LOG_VERBOSE, __func__, __LINE__, fmt "\n", __VA_ARGS__);  \
  } while (0)

#define NOTICE(fmt, ...)                                                       \
  do {                                                                         \
    if (LOG_ENABLED(MDBX_LOG_NOTICE))                                          \
      debug_log(MDBX_LOG_NOTICE, __func__, __LINE__, fmt "\n", __VA_ARGS__);   \
  } while (0)

#define WARNING(fmt, ...)                                                      \
  do {                                                                         \
    if (LOG_ENABLED(MDBX_LOG_WARN))                                            \
      debug_log(MDBX_LOG_WARN, __func__, __LINE__, fmt "\n", __VA_ARGS__);     \
  } while (0)

#undef ERROR /* wingdi.h                                                       \
  Yeah, morons from M$ put such definition to the public header. */

#define ERROR(fmt, ...)                                                        \
  do {                                                                         \
    if (LOG_ENABLED(MDBX_LOG_ERROR))                                           \
      debug_log(MDBX_LOG_ERROR, __func__, __LINE__, fmt "\n", __VA_ARGS__);    \
  } while (0)

#define FATAL(fmt, ...)                                                        \
  debug_log(MDBX_LOG_FATAL, __func__, __LINE__, fmt "\n", __VA_ARGS__);

#if MDBX_DEBUG
#define ASSERT_FAIL(env, msg, func, line) mdbx_assert_fail(env, msg, func, line)
#else /* MDBX_DEBUG */
MDBX_NORETURN __cold void assert_fail(const char *msg, const char *func,
                                      unsigned line);
#define ASSERT_FAIL(env, msg, func, line)                                      \
  do {                                                                         \
    (void)(env);                                                               \
    assert_fail(msg, func, line);                                              \
  } while (0)
#endif /* MDBX_DEBUG */

#define ENSURE_MSG(env, expr, msg)                                             \
  do {                                                                         \
    if (unlikely(!(expr)))                                                     \
      ASSERT_FAIL(env, msg, __func__, __LINE__);                               \
  } while (0)

#define ENSURE(env, expr) ENSURE_MSG(env, expr, #expr)

/* assert(3) variant in environment context */
#define eASSERT(env, expr)                                                     \
  do {                                                                         \
    if (ASSERT_ENABLED())                                                      \
      ENSURE(env, expr);                                                       \
  } while (0)

/* assert(3) variant in cursor context */
#define cASSERT(mc, expr) eASSERT((mc)->mc_txn->mt_env, expr)

/* assert(3) variant in transaction context */
#define tASSERT(txn, expr) eASSERT((txn)->mt_env, expr)

#ifndef xMDBX_TOOLS /* Avoid using internal eASSERT() */
#undef assert
#define assert(expr) eASSERT(NULL, expr)
#endif

#endif /* __cplusplus */

/*----------------------------------------------------------------------------*/
/* Atomics */

enum MDBX_memory_order {
  mo_Relaxed,
  mo_AcquireRelease
  /* , mo_SequentialConsistency */
};

typedef union {
  volatile uint32_t weak;
#ifdef MDBX_HAVE_C11ATOMICS
  volatile _Atomic uint32_t c11a;
#endif /* MDBX_HAVE_C11ATOMICS */
} MDBX_atomic_uint32_t;

typedef union {
  volatile uint64_t weak;
#if defined(MDBX_HAVE_C11ATOMICS) && (MDBX_64BIT_CAS || MDBX_64BIT_ATOMIC)
  volatile _Atomic uint64_t c11a;
#endif
#if !defined(MDBX_HAVE_C11ATOMICS) || !MDBX_64BIT_CAS || !MDBX_64BIT_ATOMIC
  __anonymous_struct_extension__ struct {
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
    MDBX_atomic_uint32_t low, high;
#elif __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
    MDBX_atomic_uint32_t high, low;
#else
#error "FIXME: Unsupported byte order"
#endif /* __BYTE_ORDER__ */
  };
#endif
} MDBX_atomic_uint64_t;

#ifdef MDBX_HAVE_C11ATOMICS

/* Crutches for C11 atomic compiler's bugs */
#if defined(__e2k__) && defined(__LCC__) && __LCC__ < /* FIXME */ 127
#define MDBX_c11a_ro(type, ptr) (&(ptr)->weak)
#define MDBX_c11a_rw(type, ptr) (&(ptr)->weak)
#elif defined(__clang__) && __clang__ < 8
#define MDBX_c11a_ro(type, ptr) ((volatile _Atomic(type) *)&(ptr)->c11a)
#define MDBX_c11a_rw(type, ptr) (&(ptr)->c11a)
#else
#define MDBX_c11a_ro(type, ptr) (&(ptr)->c11a)
#define MDBX_c11a_rw(type, ptr) (&(ptr)->c11a)
#endif /* Crutches for C11 atomic compiler's bugs */

#define mo_c11_store(fence)                                                    \
  (((fence) == mo_Relaxed)          ? memory_order_relaxed                     \
   : ((fence) == mo_AcquireRelease) ? memory_order_release                     \
                                    : memory_order_seq_cst)
#define mo_c11_load(fence)                                                     \
  (((fence) == mo_Relaxed)          ? memory_order_relaxed                     \
   : ((fence) == mo_AcquireRelease) ? memory_order_acquire                     \
                                    : memory_order_seq_cst)

#endif /* MDBX_HAVE_C11ATOMICS */

#ifndef __cplusplus

#ifdef MDBX_HAVE_C11ATOMICS
#define osal_memory_fence(order, write)                                        \
  atomic_thread_fence((write) ? mo_c11_store(order) : mo_c11_load(order))
#else /* MDBX_HAVE_C11ATOMICS */
#define osal_memory_fence(order, write)                                        \
  do {                                                                         \
    osal_compiler_barrier();                                                   \
    if (write && order > (MDBX_CPU_WRITEBACK_INCOHERENT ? mo_Relaxed           \
                                                        : mo_AcquireRelease))  \
      osal_memory_barrier();                                                   \
  } while (0)
#endif /* MDBX_HAVE_C11ATOMICS */

#if defined(MDBX_HAVE_C11ATOMICS) && defined(__LCC__)
#define atomic_store32(p, value, order)                                        \
  ({                                                                           \
    const uint32_t value_to_store = (value);                                   \
    atomic_store_explicit(MDBX_c11a_rw(uint32_t, p), value_to_store,           \
                          mo_c11_store(order));                                \
    value_to_store;                                                            \
  })
#define atomic_load32(p, order)                                                \
  atomic_load_explicit(MDBX_c11a_ro(uint32_t, p), mo_c11_load(order))
#define atomic_store64(p, value, order)                                        \
  ({                                                                           \
    const uint64_t value_to_store = (value);                                   \
    atomic_store_explicit(MDBX_c11a_rw(uint64_t, p), value_to_store,           \
                          mo_c11_store(order));                                \
    value_to_store;                                                            \
  })
#define atomic_load64(p, order)                                                \
  atomic_load_explicit(MDBX_c11a_ro(uint64_t, p), mo_c11_load(order))
#endif /* LCC && MDBX_HAVE_C11ATOMICS */

#ifndef atomic_store32
MDBX_MAYBE_UNUSED static __always_inline uint32_t
atomic_store32(MDBX_atomic_uint32_t *p, const uint32_t value,
               enum MDBX_memory_order order) {
  STATIC_ASSERT(sizeof(MDBX_atomic_uint32_t) == 4);
#ifdef MDBX_HAVE_C11ATOMICS
  assert(atomic_is_lock_free(MDBX_c11a_rw(uint32_t, p)));
  atomic_store_explicit(MDBX_c11a_rw(uint32_t, p), value, mo_c11_store(order));
#else  /* MDBX_HAVE_C11ATOMICS */
  if (order != mo_Relaxed)
    osal_compiler_barrier();
  p->weak = value;
  osal_memory_fence(order, true);
#endif /* MDBX_HAVE_C11ATOMICS */
  return value;
}
#endif /* atomic_store32 */

#ifndef atomic_load32
MDBX_MAYBE_UNUSED static __always_inline uint32_t atomic_load32(
    const volatile MDBX_atomic_uint32_t *p, enum MDBX_memory_order order) {
  STATIC_ASSERT(sizeof(MDBX_atomic_uint32_t) == 4);
#ifdef MDBX_HAVE_C11ATOMICS
  assert(atomic_is_lock_free(MDBX_c11a_ro(uint32_t, p)));
  return atomic_load_explicit(MDBX_c11a_ro(uint32_t, p), mo_c11_load(order));
#else  /* MDBX_HAVE_C11ATOMICS */
  osal_memory_fence(order, false);
  const uint32_t value = p->weak;
  if (order != mo_Relaxed)
    osal_compiler_barrier();
  return value;
#endif /* MDBX_HAVE_C11ATOMICS */
}
#endif /* atomic_load32 */

#endif /* !__cplusplus */

/*----------------------------------------------------------------------------*/
/* Basic constants and types */

/* A stamp that identifies a file as an MDBX file.
 * There's nothing special about this value other than that it is easily
 * recognizable, and it will reflect any byte order mismatches. */
#define MDBX_MAGIC UINT64_C(/* 56-bit prime */ 0x59659DBDEF4C11)

/* FROZEN: The version number for a database's datafile format. */
#define MDBX_DATA_VERSION 3
/* The version number for a database's lockfile format. */
#define MDBX_LOCK_VERSION 5

/* handle for the DB used to track free pages. */
#define FREE_DBI 0
/* handle for the default DB. */
#define MAIN_DBI 1
/* Number of DBs in metapage (free and main) - also hardcoded elsewhere */
#define CORE_DBS 2

/* Number of meta pages - also hardcoded elsewhere */
#define NUM_METAS 3

/* A page number in the database.
 *
 * MDBX uses 32 bit for page numbers. This limits database
 * size up to 2^44 bytes, in case of 4K pages. */
typedef uint32_t pgno_t;
typedef MDBX_atomic_uint32_t atomic_pgno_t;
#define PRIaPGNO PRIu32
#define MAX_PAGENO UINT32_C(0x7FFFffff)
#define MIN_PAGENO NUM_METAS

#define SAFE64_INVALID_THRESHOLD UINT64_C(0xffffFFFF00000000)

/* A transaction ID. */
typedef uint64_t txnid_t;
typedef MDBX_atomic_uint64_t atomic_txnid_t;
#define PRIaTXN PRIi64
#define MIN_TXNID UINT64_C(1)
#define MAX_TXNID (SAFE64_INVALID_THRESHOLD - 1)
#define INITIAL_TXNID (MIN_TXNID + NUM_METAS - 1)
#define INVALID_TXNID UINT64_MAX
/* LY: for testing non-atomic 64-bit txnid on 32-bit arches.
 * #define xMDBX_TXNID_STEP (UINT32_MAX / 3) */
#ifndef xMDBX_TXNID_STEP
#if MDBX_64BIT_CAS
#define xMDBX_TXNID_STEP 1u
#else
#define xMDBX_TXNID_STEP 2u
#endif
#endif /* xMDBX_TXNID_STEP */

/* Used for offsets within a single page.
 * Since memory pages are typically 4 or 8KB in size, 12-13 bits,
 * this is plenty. */
typedef uint16_t indx_t;

#define MEGABYTE ((size_t)1 << 20)

/*----------------------------------------------------------------------------*/
/* Core structures for database and shared memory (i.e. format definition) */
#pragma pack(push, 4)

/* Information about a single database in the environment. */
typedef struct MDBX_db {
  uint16_t md_flags;        /* see mdbx_dbi_open */
  uint16_t md_depth;        /* depth of this tree */
  uint32_t md_xsize;        /* key-size for MDBX_DUPFIXED (LEAF2 pages) */
  pgno_t md_root;           /* the root page of this tree */
  pgno_t md_branch_pages;   /* number of internal pages */
  pgno_t md_leaf_pages;     /* number of leaf pages */
  pgno_t md_overflow_pages; /* number of overflow pages */
  uint64_t md_seq;          /* table sequence counter */
  uint64_t md_entries;      /* number of data items */
  uint64_t md_mod_txnid;    /* txnid of last committed modification */
} MDBX_db;

/* database size-related parameters */
typedef struct MDBX_geo {
  uint16_t grow_pv;   /* datafile growth step as a 16-bit packed (exponential
                           quantized) value */
  uint16_t shrink_pv; /* datafile shrink threshold as a 16-bit packed
                           (exponential quantized) value */
  pgno_t lower;       /* minimal size of datafile in pages */
  pgno_t upper;       /* maximal size of datafile in pages */
  pgno_t now;         /* current size of datafile in pages */
  pgno_t next;        /* first unused page in the datafile,
                         but actually the file may be shorter. */
} MDBX_geo;

/* Meta page content.
 * A meta page is the start point for accessing a database snapshot.
 * Pages 0-1 are meta pages. Transaction N writes meta page (N % 2). */
typedef struct MDBX_meta {
  /* Stamp identifying this as an MDBX file.
   * It must be set to MDBX_MAGIC with MDBX_DATA_VERSION. */
  uint32_t mm_magic_and_version[2];

  /* txnid that committed this page, the first of a two-phase-update pair */
  union {
    MDBX_atomic_uint32_t mm_txnid_a[2];
    uint64_t unsafe_txnid;
  };

  uint16_t mm_extra_flags;  /* extra DB flags, zero (nothing) for now */
  uint8_t mm_validator_id;  /* ID of checksum and page validation method,
                             * zero (nothing) for now */
  uint8_t mm_extra_pagehdr; /* extra bytes in the page header,
                             * zero (nothing) for now */

  MDBX_geo mm_geo; /* database size-related parameters */

  MDBX_db mm_dbs[CORE_DBS]; /* first is free space, 2nd is main db */
                            /* The size of pages used in this DB */
#define mm_psize mm_dbs[FREE_DBI].md_xsize
  MDBX_canary mm_canary;

#define MDBX_DATASIGN_NONE 0u
#define MDBX_DATASIGN_WEAK 1u
#define SIGN_IS_STEADY(sign) ((sign) > MDBX_DATASIGN_WEAK)
#define META_IS_STEADY(meta)                                                   \
  SIGN_IS_STEADY(unaligned_peek_u64_volatile(4, (meta)->mm_sign))
  union {
    uint32_t mm_sign[2];
    uint64_t unsafe_sign;
  };

  /* txnid that committed this page, the second of a two-phase-update pair */
  MDBX_atomic_uint32_t mm_txnid_b[2];

  /* Number of non-meta pages which were put in GC after COW. May be 0 in case
   * DB was previously handled by libmdbx without corresponding feature.
   * This value in couple with mr_snapshot_pages_retired allows fast estimation
   * of "how much reader is restraining GC recycling". */
  uint32_t mm_pages_retired[2];

  /* The analogue /proc/sys/kernel/random/boot_id or similar to determine
   * whether the system was rebooted after the last use of the database files.
   * If there was no reboot, but there is no need to rollback to the last
   * steady sync point. Zeros mean that no relevant information is available
   * from the system. */
  bin128_t mm_bootid;

} MDBX_meta;

#pragma pack(1)

/* Common header for all page types. The page type depends on mp_flags.
 *
 * P_BRANCH and P_LEAF pages have unsorted 'MDBX_node's at the end, with
 * sorted mp_ptrs[] entries referring to them. Exception: P_LEAF2 pages
 * omit mp_ptrs and pack sorted MDBX_DUPFIXED values after the page header.
 *
 * P_OVERFLOW records occupy one or more contiguous pages where only the
 * first has a page header. They hold the real data of F_BIGDATA nodes.
 *
 * P_SUBP sub-pages are small leaf "pages" with duplicate data.
 * A node with flag F_DUPDATA but not F_SUBDATA contains a sub-page.
 * (Duplicate data can also go in sub-databases, which use normal pages.)
 *
 * P_META pages contain MDBX_meta, the start point of an MDBX snapshot.
 *
 * Each non-metapage up to MDBX_meta.mm_last_pg is reachable exactly once
 * in the snapshot: Either used by a database or listed in a GC record. */
typedef struct MDBX_page {
#define IS_FROZEN(txn, p) ((p)->mp_txnid < (txn)->mt_txnid)
#define IS_SPILLED(txn, p) ((p)->mp_txnid == (txn)->mt_txnid)
#define IS_SHADOWED(txn, p) ((p)->mp_txnid > (txn)->mt_txnid)
#define IS_VALID(txn, p) ((p)->mp_txnid <= (txn)->mt_front)
#define IS_MODIFIABLE(txn, p) ((p)->mp_txnid == (txn)->mt_front)
  uint64_t mp_txnid; /* txnid which created page, maybe zero in legacy DB */
  uint16_t mp_leaf2_ksize;   /* key size if this is a LEAF2 page */
#define P_BRANCH 0x01u       /* branch page */
#define P_LEAF 0x02u         /* leaf page */
#define P_OVERFLOW 0x04u     /* overflow page */
#define P_META 0x08u         /* meta page */
#define P_LEGACY_DIRTY 0x10u /* legacy P_DIRTY flag prior to v0.10 958fd5b9 */
#define P_BAD P_LEGACY_DIRTY /* explicit flag for invalid/bad page */
#define P_LEAF2 0x20u        /* for MDBX_DUPFIXED records */
#define P_SUBP 0x40u         /* for MDBX_DUPSORT sub-pages */
#define P_SPILLED 0x2000u    /* spilled in parent txn */
#define P_LOOSE 0x4000u      /* page was dirtied then freed, can be reused */
#define P_FROZEN 0x8000u     /* used for retire page with known status */
#define P_ILL_BITS                                                             \
  ((uint16_t) ~(P_BRANCH | P_LEAF | P_LEAF2 | P_OVERFLOW | P_SPILLED))
  uint16_t mp_flags;
  union {
    uint32_t mp_pages; /* number of overflow pages */
    __anonymous_struct_extension__ struct {
      indx_t mp_lower; /* lower bound of free space */
      indx_t mp_upper; /* upper bound of free space */
    };
  };
  pgno_t mp_pgno; /* page number */

#if (defined(__STDC_VERSION__) && __STDC_VERSION__ >= 199901L) ||              \
    (!defined(__cplusplus) && defined(_MSC_VER))
  indx_t mp_ptrs[] /* dynamic size */;
#endif /* C99 */
} MDBX_page;

#define PAGETYPE_WHOLE(p) ((uint8_t)(p)->mp_flags)

/* Drop legacy P_DIRTY flag for sub-pages for compatilibity */
#define PAGETYPE_COMPAT(p)                                                     \
  (unlikely(PAGETYPE_WHOLE(p) & P_SUBP)                                        \
       ? PAGETYPE_WHOLE(p) & ~(P_SUBP | P_LEGACY_DIRTY)                        \
       : PAGETYPE_WHOLE(p))

/* Size of the page header, excluding dynamic data at the end */
#define PAGEHDRSZ offsetof(MDBX_page, mp_ptrs)

/* Pointer displacement without casting to char* to avoid pointer-aliasing */
#define ptr_disp(ptr, disp) ((void *)(((intptr_t)(ptr)) + ((intptr_t)(disp))))

/* Pointer distance as signed number of bytes */
#define ptr_dist(more, less) (((intptr_t)(more)) - ((intptr_t)(less)))

#define mp_next(mp)                                                            \
  (*(MDBX_page **)ptr_disp((mp)->mp_ptrs, sizeof(void *) - sizeof(uint32_t)))

#pragma pack(pop)

typedef struct profgc_stat {
  /* Монотонное время по "настенным часам"
   * затраченное на чтение и поиск внутри GC */
  uint64_t rtime_monotonic;
  /* Процессорное время в режим пользователя
   * на подготовку страниц извлекаемых из GC, включая подкачку с диска. */
  uint64_t xtime_cpu;
  /* Количество итераций чтения-поиска внутри GC при выделении страниц */
  uint32_t rsteps;
  /* Количество запросов на выделение последовательностей страниц,
   * т.е. когда запрашивает выделение больше одной страницы */
  uint32_t xpages;
  /* Счетчик выполнения по медленному пути (slow path execution count) */
  uint32_t spe_counter;
  /* page faults (hard page faults) */
  uint32_t majflt;
} profgc_stat_t;

/* Statistics of page operations overall of all (running, completed and aborted)
 * transactions */
typedef struct pgop_stat {
  MDBX_atomic_uint64_t newly;   /* Quantity of a new pages added */
  MDBX_atomic_uint64_t cow;     /* Quantity of pages copied for update */
  MDBX_atomic_uint64_t clone;   /* Quantity of parent's dirty pages clones
                                   for nested transactions */
  MDBX_atomic_uint64_t split;   /* Page splits */
  MDBX_atomic_uint64_t merge;   /* Page merges */
  MDBX_atomic_uint64_t spill;   /* Quantity of spilled dirty pages */
  MDBX_atomic_uint64_t unspill; /* Quantity of unspilled/reloaded pages */
  MDBX_atomic_uint64_t
      wops; /* Number of explicit write operations (not a pages) to a disk */
  MDBX_atomic_uint64_t
      msync; /* Number of explicit msync/flush-to-disk operations */
  MDBX_atomic_uint64_t
      fsync; /* Number of explicit fsync/flush-to-disk operations */

  MDBX_atomic_uint64_t prefault; /* Number of prefault write operations */
  MDBX_atomic_uint64_t mincore;  /* Number of mincore() calls */

  MDBX_atomic_uint32_t
      incoherence; /* number of https://libmdbx.dqdkfa.ru/dead-github/issues/269
                      caught */
  MDBX_atomic_uint32_t reserved;

  /* Статистика для профилирования GC.
   * Логически эти данные может быть стоит вынести в другую структуру,
   * но разница будет сугубо косметическая. */
  struct {
    /* Затраты на поддержку данных пользователя */
    profgc_stat_t work;
    /* Затраты на поддержку и обновления самой GC */
    profgc_stat_t self;
    /* Итераций обновления GC,
     * больше 1 если были повторы/перезапуски */
    uint32_t wloops;
    /* Итерации слияния записей GC */
    uint32_t coalescences;
    /* Уничтожения steady-точек фиксации в MDBX_UTTERLY_NOSYNC */
    uint32_t wipes;
    /* Сбросы данные на диск вне MDBX_UTTERLY_NOSYNC */
    uint32_t flushes;
    /* Попытки пнуть тормозящих читателей */
    uint32_t kicks;
  } gc_prof;
} pgop_stat_t;

#if MDBX_LOCKING == MDBX_LOCKING_WIN32FILES
#define MDBX_CLOCK_SIGN UINT32_C(0xF10C)
typedef void osal_ipclock_t;
#elif MDBX_LOCKING == MDBX_LOCKING_SYSV

#define MDBX_CLOCK_SIGN UINT32_C(0xF18D)
typedef mdbx_pid_t osal_ipclock_t;
#ifndef EOWNERDEAD
#define EOWNERDEAD MDBX_RESULT_TRUE
#endif

#elif MDBX_LOCKING == MDBX_LOCKING_POSIX2001 ||                                \
    MDBX_LOCKING == MDBX_LOCKING_POSIX2008
#define MDBX_CLOCK_SIGN UINT32_C(0x8017)
typedef pthread_mutex_t osal_ipclock_t;
#elif MDBX_LOCKING == MDBX_LOCKING_POSIX1988
#define MDBX_CLOCK_SIGN UINT32_C(0xFC29)
typedef sem_t osal_ipclock_t;
#else
#error "FIXME"
#endif /* MDBX_LOCKING */

#if MDBX_LOCKING > MDBX_LOCKING_SYSV && !defined(__cplusplus)
MDBX_INTERNAL_FUNC int osal_ipclock_stub(osal_ipclock_t *ipc);
MDBX_INTERNAL_FUNC int osal_ipclock_destroy(osal_ipclock_t *ipc);
#endif /* MDBX_LOCKING */

/* Reader Lock Table
 *
 * Readers don't acquire any locks for their data access. Instead, they
 * simply record their transaction ID in the reader table. The reader
 * mutex is needed just to find an empty slot in the reader table. The
 * slot's address is saved in thread-specific data so that subsequent
 * read transactions started by the same thread need no further locking to
 * proceed.
 *
 * If MDBX_NOTLS is set, the slot address is not saved in thread-specific data.
 * No reader table is used if the database is on a read-only filesystem.
 *
 * Since the database uses multi-version concurrency control, readers don't
 * actually need any locking. This table is used to keep track of which
 * readers are using data from which old transactions, so that we'll know
 * when a particular old transaction is no longer in use. Old transactions
 * that have discarded any data pages can then have those pages reclaimed
 * for use by a later write transaction.
 *
 * The lock table is constructed such that reader slots are aligned with the
 * processor's cache line size. Any slot is only ever used by one thread.
 * This alignment guarantees that there will be no contention or cache
 * thrashing as threads update their own slot info, and also eliminates
 * any need for locking when accessing a slot.
 *
 * A writer thread will scan every slot in the table to determine the oldest
 * outstanding reader transaction. Any freed pages older than this will be
 * reclaimed by the writer. The writer doesn't use any locks when scanning
 * this table. This means that there's no guarantee that the writer will
 * see the most up-to-date reader info, but that's not required for correct
 * operation - all we need is to know the upper bound on the oldest reader,
 * we don't care at all about the newest reader. So the only consequence of
 * reading stale information here is that old pages might hang around a
 * while longer before being reclaimed. That's actually good anyway, because
 * the longer we delay reclaiming old pages, the more likely it is that a
 * string of contiguous pages can be found after coalescing old pages from
 * many old transactions together. */

/* The actual reader record, with cacheline padding. */
typedef struct MDBX_reader {
  /* Current Transaction ID when this transaction began, or (txnid_t)-1.
   * Multiple readers that start at the same time will probably have the
   * same ID here. Again, it's not important to exclude them from
   * anything; all we need to know is which version of the DB they
   * started from so we can avoid overwriting any data used in that
   * particular version. */
  MDBX_atomic_uint64_t /* txnid_t */ mr_txnid;

  /* The information we store in a single slot of the reader table.
   * In addition to a transaction ID, we also record the process and
   * thread ID that owns a slot, so that we can detect stale information,
   * e.g. threads or processes that went away without cleaning up.
   *
   * NOTE: We currently don't check for stale records.
   * We simply re-init the table when we know that we're the only process
   * opening the lock file. */

  /* The thread ID of the thread owning this txn. */
  MDBX_atomic_uint64_t mr_tid;

  /* The process ID of the process owning this reader txn. */
  MDBX_atomic_uint32_t mr_pid;

  /* The number of pages used in the reader's MVCC snapshot,
   * i.e. the value of meta->mm_geo.next and txn->mt_next_pgno */
  atomic_pgno_t mr_snapshot_pages_used;
  /* Number of retired pages at the time this reader starts transaction. So,
   * at any time the difference mm_pages_retired - mr_snapshot_pages_retired
   * will give the number of pages which this reader restraining from reuse. */
  MDBX_atomic_uint64_t mr_snapshot_pages_retired;
} MDBX_reader;

/* The header for the reader table (a memory-mapped lock file). */
typedef struct MDBX_lockinfo {
  /* Stamp identifying this as an MDBX file.
   * It must be set to MDBX_MAGIC with with MDBX_LOCK_VERSION. */
  uint64_t mti_magic_and_version;

  /* Format of this lock file. Must be set to MDBX_LOCK_FORMAT. */
  uint32_t mti_os_and_format;

  /* Flags which environment was opened. */
  MDBX_atomic_uint32_t mti_envmode;

  /* Threshold of un-synced-with-disk pages for auto-sync feature,
   * zero means no-threshold, i.e. auto-sync is disabled. */
  atomic_pgno_t mti_autosync_threshold;

  /* Low 32-bit of txnid with which meta-pages was synced,
   * i.e. for sync-polling in the MDBX_NOMETASYNC mode. */
#define MDBX_NOMETASYNC_LAZY_UNK (UINT32_MAX / 3)
#define MDBX_NOMETASYNC_LAZY_FD (MDBX_NOMETASYNC_LAZY_UNK + UINT32_MAX / 8)
#define MDBX_NOMETASYNC_LAZY_WRITEMAP                                          \
  (MDBX_NOMETASYNC_LAZY_UNK - UINT32_MAX / 8)
  MDBX_atomic_uint32_t mti_meta_sync_txnid;

  /* Period for timed auto-sync feature, i.e. at the every steady checkpoint
   * the mti_unsynced_timeout sets to the current_time + mti_autosync_period.
   * The time value is represented in a suitable system-dependent form, for
   * example clock_gettime(CLOCK_BOOTTIME) or clock_gettime(CLOCK_MONOTONIC).
   * Zero means timed auto-sync is disabled. */
  MDBX_atomic_uint64_t mti_autosync_period;

  /* Marker to distinguish uniqueness of DB/CLK. */
  MDBX_atomic_uint64_t mti_bait_uniqueness;

  /* Paired counter of processes that have mlock()ed part of mmapped DB.
   * The (mti_mlcnt[0] - mti_mlcnt[1]) > 0 means at least one process
   * lock at least one page, so therefore madvise() could return EINVAL. */
  MDBX_atomic_uint32_t mti_mlcnt[2];

  MDBX_ALIGNAS(MDBX_CACHELINE_SIZE) /* cacheline ----------------------------*/

  /* Statistics of costly ops of all (running, completed and aborted)
   * transactions */
  pgop_stat_t mti_pgop_stat;

  MDBX_ALIGNAS(MDBX_CACHELINE_SIZE) /* cacheline ----------------------------*/

  /* Write transaction lock. */
#if MDBX_LOCKING > 0
  osal_ipclock_t mti_wlock;
#endif /* MDBX_LOCKING > 0 */

  atomic_txnid_t mti_oldest_reader;

  /* Timestamp of entering an out-of-sync state. Value is represented in a
   * suitable system-dependent form, for example clock_gettime(CLOCK_BOOTTIME)
   * or clock_gettime(CLOCK_MONOTONIC). */
  MDBX_atomic_uint64_t mti_eoos_timestamp;

  /* Number un-synced-with-disk pages for auto-sync feature. */
  MDBX_atomic_uint64_t mti_unsynced_pages;

  /* Timestamp of the last readers check. */
  MDBX_atomic_uint64_t mti_reader_check_timestamp;

  /* Number of page which was discarded last time by madvise(DONTNEED). */
  atomic_pgno_t mti_discarded_tail;

  /* Shared anchor for tracking readahead edge and enabled/disabled status. */
  pgno_t mti_readahead_anchor;

  /* Shared cache for mincore() results */
  struct {
    pgno_t begin[4];
    uint64_t mask[4];
  } mti_mincore_cache;

  MDBX_ALIGNAS(MDBX_CACHELINE_SIZE) /* cacheline ----------------------------*/

  /* Readeaders registration lock. */
#if MDBX_LOCKING > 0
  osal_ipclock_t mti_rlock;
#endif /* MDBX_LOCKING > 0 */

  /* The number of slots that have been used in the reader table.
   * This always records the maximum count, it is not decremented
   * when readers release their slots. */
  MDBX_atomic_uint32_t mti_numreaders;
  MDBX_atomic_uint32_t mti_readers_refresh_flag;

#if (defined(__STDC_VERSION__) && __STDC_VERSION__ >= 199901L) ||              \
    (!defined(__cplusplus) && defined(_MSC_VER))
  MDBX_ALIGNAS(MDBX_CACHELINE_SIZE) /* cacheline ----------------------------*/
  MDBX_reader mti_readers[] /* dynamic size */;
#endif /* C99 */
} MDBX_lockinfo;

/* Lockfile format signature: version, features and field layout */
#define MDBX_LOCK_FORMAT                                                       \
  (MDBX_CLOCK_SIGN * 27733 + (unsigned)sizeof(MDBX_reader) * 13 +              \
   (unsigned)offsetof(MDBX_reader, mr_snapshot_pages_used) * 251 +             \
   (unsigned)offsetof(MDBX_lockinfo, mti_oldest_reader) * 83 +                 \
   (unsigned)offsetof(MDBX_lockinfo, mti_numreaders) * 37 +                    \
   (unsigned)offsetof(MDBX_lockinfo, mti_readers) * 29)

#define MDBX_DATA_MAGIC                                                        \
  ((MDBX_MAGIC << 8) + MDBX_PNL_ASCENDING * 64 + MDBX_DATA_VERSION)

#define MDBX_DATA_MAGIC_LEGACY_COMPAT                                          \
  ((MDBX_MAGIC << 8) + MDBX_PNL_ASCENDING * 64 + 2)

#define MDBX_DATA_MAGIC_LEGACY_DEVEL ((MDBX_MAGIC << 8) + 255)

#define MDBX_LOCK_MAGIC ((MDBX_MAGIC << 8) + MDBX_LOCK_VERSION)

/* The maximum size of a database page.
 *
 * It is 64K, but value-PAGEHDRSZ must fit in MDBX_page.mp_upper.
 *
 * MDBX will use database pages < OS pages if needed.
 * That causes more I/O in write transactions: The OS must
 * know (read) the whole page before writing a partial page.
 *
 * Note that we don't currently support Huge pages. On Linux,
 * regular data files cannot use Huge pages, and in general
 * Huge pages aren't actually pageable. We rely on the OS
 * demand-pager to read our data and page it out when memory
 * pressure from other processes is high. So until OSs have
 * actual paging support for Huge pages, they're not viable. */
#define MAX_PAGESIZE MDBX_MAX_PAGESIZE
#define MIN_PAGESIZE MDBX_MIN_PAGESIZE

#define MIN_MAPSIZE (MIN_PAGESIZE * MIN_PAGENO)
#if defined(_WIN32) || defined(_WIN64)
#define MAX_MAPSIZE32 UINT32_C(0x38000000)
#else
#define MAX_MAPSIZE32 UINT32_C(0x7f000000)
#endif
#define MAX_MAPSIZE64 ((MAX_PAGENO + 1) * (uint64_t)MAX_PAGESIZE)

#if MDBX_WORDBITS >= 64
#define MAX_MAPSIZE MAX_MAPSIZE64
#define MDBX_PGL_LIMIT ((size_t)MAX_PAGENO)
#else
#define MAX_MAPSIZE MAX_MAPSIZE32
#define MDBX_PGL_LIMIT (MAX_MAPSIZE32 / MIN_PAGESIZE)
#endif /* MDBX_WORDBITS */

#define MDBX_READERS_LIMIT 32767
#define MDBX_RADIXSORT_THRESHOLD 142
#define MDBX_GOLD_RATIO_DBL 1.6180339887498948482

/*----------------------------------------------------------------------------*/

/* An PNL is an Page Number List, a sorted array of IDs.
 * The first element of the array is a counter for how many actual page-numbers
 * are in the list. By default PNLs are sorted in descending order, this allow
 * cut off a page with lowest pgno (at the tail) just truncating the list. The
 * sort order of PNLs is controlled by the MDBX_PNL_ASCENDING build option. */
typedef pgno_t *MDBX_PNL;

#if MDBX_PNL_ASCENDING
#define MDBX_PNL_ORDERED(first, last) ((first) < (last))
#define MDBX_PNL_DISORDERED(first, last) ((first) >= (last))
#else
#define MDBX_PNL_ORDERED(first, last) ((first) > (last))
#define MDBX_PNL_DISORDERED(first, last) ((first) <= (last))
#endif

/* List of txnid, only for MDBX_txn.tw.lifo_reclaimed */
typedef txnid_t *MDBX_TXL;

/* An Dirty-Page list item is an pgno/pointer pair. */
typedef struct MDBX_dp {
  MDBX_page *ptr;
  pgno_t pgno, npages;
} MDBX_dp;

/* An DPL (dirty-page list) is a sorted array of MDBX_DPs. */
typedef struct MDBX_dpl {
  size_t sorted;
  size_t length;
  size_t pages_including_loose; /* number of pages, but not an entries. */
  size_t detent; /* allocated size excluding the MDBX_DPL_RESERVE_GAP */
#if (defined(__STDC_VERSION__) && __STDC_VERSION__ >= 199901L) ||              \
    (!defined(__cplusplus) && defined(_MSC_VER))
  MDBX_dp items[] /* dynamic size with holes at zero and after the last */;
#endif
} MDBX_dpl;

/* PNL sizes */
#define MDBX_PNL_GRANULATE_LOG2 10
#define MDBX_PNL_GRANULATE (1 << MDBX_PNL_GRANULATE_LOG2)
#define MDBX_PNL_INITIAL                                                       \
  (MDBX_PNL_GRANULATE - 2 - MDBX_ASSUME_MALLOC_OVERHEAD / sizeof(pgno_t))

#define MDBX_TXL_GRANULATE 32
#define MDBX_TXL_INITIAL                                                       \
  (MDBX_TXL_GRANULATE - 2 - MDBX_ASSUME_MALLOC_OVERHEAD / sizeof(txnid_t))
#define MDBX_TXL_MAX                                                           \
  ((1u << 26) - 2 - MDBX_ASSUME_MALLOC_OVERHEAD / sizeof(txnid_t))

#define MDBX_PNL_ALLOCLEN(pl) ((pl)[-1])
#define MDBX_PNL_GETSIZE(pl) ((size_t)((pl)[0]))
#define MDBX_PNL_SETSIZE(pl, size)                                             \
  do {                                                                         \
    const size_t __size = size;                                                \
    assert(__size < INT_MAX);                                                  \
    (pl)[0] = (pgno_t)__size;                                                  \
  } while (0)
#define MDBX_PNL_FIRST(pl) ((pl)[1])
#define MDBX_PNL_LAST(pl) ((pl)[MDBX_PNL_GETSIZE(pl)])
#define MDBX_PNL_BEGIN(pl) (&(pl)[1])
#define MDBX_PNL_END(pl) (&(pl)[MDBX_PNL_GETSIZE(pl) + 1])

#if MDBX_PNL_ASCENDING
#define MDBX_PNL_EDGE(pl) ((pl) + 1)
#define MDBX_PNL_LEAST(pl) MDBX_PNL_FIRST(pl)
#define MDBX_PNL_MOST(pl) MDBX_PNL_LAST(pl)
#else
#define MDBX_PNL_EDGE(pl) ((pl) + MDBX_PNL_GETSIZE(pl))
#define MDBX_PNL_LEAST(pl) MDBX_PNL_LAST(pl)
#define MDBX_PNL_MOST(pl) MDBX_PNL_FIRST(pl)
#endif

#define MDBX_PNL_SIZEOF(pl) ((MDBX_PNL_GETSIZE(pl) + 1) * sizeof(pgno_t))
#define MDBX_PNL_IS_EMPTY(pl) (MDBX_PNL_GETSIZE(pl) == 0)

/*----------------------------------------------------------------------------*/
/* Internal structures */

/* Auxiliary DB info.
 * The information here is mostly static/read-only. There is
 * only a single copy of this record in the environment. */
typedef struct MDBX_dbx {
  MDBX_val md_name;                /* name of the database */
  MDBX_cmp_func *md_cmp;           /* function for comparing keys */
  MDBX_cmp_func *md_dcmp;          /* function for comparing data items */
  size_t md_klen_min, md_klen_max; /* min/max key length for the database */
  size_t md_vlen_min,
      md_vlen_max; /* min/max value/data length for the database */
} MDBX_dbx;

typedef struct troika {
  uint8_t fsm, recent, prefer_steady, tail_and_flags;
#if MDBX_WORDBITS > 32 /* Workaround for false-positives from Valgrind */
  uint32_t unused_pad;
#endif
#define TROIKA_HAVE_STEADY(troika) ((troika)->fsm & 7)
#define TROIKA_STRICT_VALID(troika) ((troika)->tail_and_flags & 64)
#define TROIKA_VALID(troika) ((troika)->tail_and_flags & 128)
#define TROIKA_TAIL(troika) ((troika)->tail_and_flags & 3)
  txnid_t txnid[NUM_METAS];
} meta_troika_t;

/* A database transaction.
 * Every operation requires a transaction handle. */
struct MDBX_txn {
#define MDBX_MT_SIGNATURE UINT32_C(0x93D53A31)
  uint32_t mt_signature;

  /* Transaction Flags */
  /* mdbx_txn_begin() flags */
#define MDBX_TXN_RO_BEGIN_FLAGS (MDBX_TXN_RDONLY | MDBX_TXN_RDONLY_PREPARE)
#define MDBX_TXN_RW_BEGIN_FLAGS                                                \
  (MDBX_TXN_NOMETASYNC | MDBX_TXN_NOSYNC | MDBX_TXN_TRY)
  /* Additional flag for sync_locked() */
#define MDBX_SHRINK_ALLOWED UINT32_C(0x40000000)

#define MDBX_TXN_DRAINED_GC 0x20 /* GC was depleted up to oldest reader */

#define TXN_FLAGS                                                              \
  (MDBX_TXN_FINISHED | MDBX_TXN_ERROR | MDBX_TXN_DIRTY | MDBX_TXN_SPILLS |     \
   MDBX_TXN_HAS_CHILD | MDBX_TXN_INVALID | MDBX_TXN_DRAINED_GC)

#if (TXN_FLAGS & (MDBX_TXN_RW_BEGIN_FLAGS | MDBX_TXN_RO_BEGIN_FLAGS)) ||       \
    ((MDBX_TXN_RW_BEGIN_FLAGS | MDBX_TXN_RO_BEGIN_FLAGS | TXN_FLAGS) &         \
     MDBX_SHRINK_ALLOWED)
#error "Oops, some txn flags overlapped or wrong"
#endif
  uint32_t mt_flags;

  MDBX_txn *mt_parent; /* parent of a nested txn */
  /* Nested txn under this txn, set together with flag MDBX_TXN_HAS_CHILD */
  MDBX_txn *mt_child;
  MDBX_geo mt_geo;
  /* next unallocated page */
#define mt_next_pgno mt_geo.next
  /* corresponding to the current size of datafile */
#define mt_end_pgno mt_geo.now

  /* The ID of this transaction. IDs are integers incrementing from
   * INITIAL_TXNID. Only committed write transactions increment the ID. If a
   * transaction aborts, the ID may be re-used by the next writer. */
  txnid_t mt_txnid;
  txnid_t mt_front;

  MDBX_env *mt_env; /* the DB environment */
  /* Array of records for each DB known in the environment. */
  MDBX_dbx *mt_dbxs;
  /* Array of MDBX_db records for each known DB */
  MDBX_db *mt_dbs;
  /* Array of sequence numbers for each DB handle */
  MDBX_atomic_uint32_t *mt_dbiseqs;

  /* Transaction DBI Flags */
#define DBI_DIRTY MDBX_DBI_DIRTY /* DB was written in this txn */
#define DBI_STALE MDBX_DBI_STALE /* Named-DB record is older than txnID */
#define DBI_FRESH MDBX_DBI_FRESH /* Named-DB handle opened in this txn */
#define DBI_CREAT MDBX_DBI_CREAT /* Named-DB handle created in this txn */
#define DBI_VALID 0x10           /* DB handle is valid, see also DB_VALID */
#define DBI_USRVALID 0x20        /* As DB_VALID, but not set for FREE_DBI */
#define DBI_AUDITED 0x40         /* Internal flag for accounting during audit */
  /* Array of flags for each DB */
  uint8_t *mt_dbistate;
  /* Number of DB records in use, or 0 when the txn is finished.
   * This number only ever increments until the txn finishes; we
   * don't decrement it when individual DB handles are closed. */
  MDBX_dbi mt_numdbs;
  size_t mt_owner; /* thread ID that owns this transaction */
  MDBX_canary mt_canary;
  void *mt_userctx; /* User-settable context */
  MDBX_cursor **mt_cursors;

  union {
    struct {
      /* For read txns: This thread/txn's reader table slot, or NULL. */
      MDBX_reader *reader;
    } to;
    struct {
      meta_troika_t troika;
      /* In write txns, array of cursors for each DB */
      MDBX_PNL relist;        /* Reclaimed GC pages */
      txnid_t last_reclaimed; /* ID of last used record */
#if MDBX_ENABLE_REFUND
      pgno_t loose_refund_wl /* FIXME: describe */;
#endif /* MDBX_ENABLE_REFUND */
      /* a sequence to spilling dirty page with LRU policy */
      unsigned dirtylru;
      /* dirtylist room: Dirty array size - dirty pages visible to this txn.
       * Includes ancestor txns' dirty pages not hidden by other txns'
       * dirty/spilled pages. Thus commit(nested txn) has room to merge
       * dirtylist into mt_parent after freeing hidden mt_parent pages. */
      size_t dirtyroom;
      /* For write txns: Modified pages. Sorted when not MDBX_WRITEMAP. */
      MDBX_dpl *dirtylist;
      /* The list of reclaimed txns from GC */
      MDBX_TXL lifo_reclaimed;
      /* The list of pages that became unused during this transaction. */
      MDBX_PNL retired_pages;
      /* The list of loose pages that became unused and may be reused
       * in this transaction, linked through `mp_next`. */
      MDBX_page *loose_pages;
      /* Number of loose pages (tw.loose_pages) */
      size_t loose_count;
      union {
        struct {
          size_t least_removed;
          /* The sorted list of dirty pages we temporarily wrote to disk
           * because the dirty list was full. page numbers in here are
           * shifted left by 1, deleted slots have the LSB set. */
          MDBX_PNL list;
        } spilled;
        size_t writemap_dirty_npages;
        size_t writemap_spilled_npages;
      };
    } tw;
  };
};

#if MDBX_WORDBITS >= 64
#define CURSOR_STACK 32
#else
#define CURSOR_STACK 24
#endif

struct MDBX_xcursor;

/* Cursors are used for all DB operations.
 * A cursor holds a path of (page pointer, key index) from the DB
 * root to a position in the DB, plus other state. MDBX_DUPSORT
 * cursors include an xcursor to the current data item. Write txns
 * track their cursors and keep them up to date when data moves.
 * Exception: An xcursor's pointer to a P_SUBP page can be stale.
 * (A node with F_DUPDATA but no F_SUBDATA contains a subpage). */
struct MDBX_cursor {
#define MDBX_MC_LIVE UINT32_C(0xFE05D5B1)
#define MDBX_MC_READY4CLOSE UINT32_C(0x2817A047)
#define MDBX_MC_WAIT4EOT UINT32_C(0x90E297A7)
  uint32_t mc_signature;
  /* The database handle this cursor operates on */
  MDBX_dbi mc_dbi;
  /* Next cursor on this DB in this txn */
  MDBX_cursor *mc_next;
  /* Backup of the original cursor if this cursor is a shadow */
  MDBX_cursor *mc_backup;
  /* Context used for databases with MDBX_DUPSORT, otherwise NULL */
  struct MDBX_xcursor *mc_xcursor;
  /* The transaction that owns this cursor */
  MDBX_txn *mc_txn;
  /* The database record for this cursor */
  MDBX_db *mc_db;
  /* The database auxiliary record for this cursor */
  MDBX_dbx *mc_dbx;
  /* The mt_dbistate for this database */
  uint8_t *mc_dbistate;
  uint8_t mc_snum; /* number of pushed pages */
  uint8_t mc_top;  /* index of top page, normally mc_snum-1 */

  /* Cursor state flags. */
#define C_INITIALIZED 0x01 /* cursor has been initialized and is valid */
#define C_EOF 0x02         /* No more data */
#define C_SUB 0x04         /* Cursor is a sub-cursor */
#define C_DEL 0x08         /* last op was a cursor_del */
#define C_UNTRACK 0x10     /* Un-track cursor when closing */
#define C_GCU                                                                                  \
  0x20 /* Происходит подготовка к обновлению GC, поэтому \
        * можно брать страницы из GC даже для FREE_DBI */
  uint8_t mc_flags;

  /* Cursor checking flags. */
#define CC_BRANCH 0x01    /* same as P_BRANCH for CHECK_LEAF_TYPE() */
#define CC_LEAF 0x02      /* same as P_LEAF for CHECK_LEAF_TYPE() */
#define CC_OVERFLOW 0x04  /* same as P_OVERFLOW for CHECK_LEAF_TYPE() */
#define CC_UPDATING 0x08  /* update/rebalance pending */
#define CC_SKIPORD 0x10   /* don't check keys ordering */
#define CC_LEAF2 0x20     /* same as P_LEAF2 for CHECK_LEAF_TYPE() */
#define CC_RETIRING 0x40  /* refs to child pages may be invalid */
#define CC_PAGECHECK 0x80 /* perform page checking, see MDBX_VALIDATION */
  uint8_t mc_checking;

  MDBX_page *mc_pg[CURSOR_STACK]; /* stack of pushed pages */
  indx_t mc_ki[CURSOR_STACK];     /* stack of page indices */
};

#define CHECK_LEAF_TYPE(mc, mp)                                                \
  (((PAGETYPE_WHOLE(mp) ^ (mc)->mc_checking) &                                 \
    (CC_BRANCH | CC_LEAF | CC_OVERFLOW | CC_LEAF2)) == 0)

/* Context for sorted-dup records.
 * We could have gone to a fully recursive design, with arbitrarily
 * deep nesting of sub-databases. But for now we only handle these
 * levels - main DB, optional sub-DB, sorted-duplicate DB. */
typedef struct MDBX_xcursor {
  /* A sub-cursor for traversing the Dup DB */
  MDBX_cursor mx_cursor;
  /* The database record for this Dup DB */
  MDBX_db mx_db;
  /* The auxiliary DB record for this Dup DB */
  MDBX_dbx mx_dbx;
} MDBX_xcursor;

typedef struct MDBX_cursor_couple {
  MDBX_cursor outer;
  void *mc_userctx; /* User-settable context */
  MDBX_xcursor inner;
} MDBX_cursor_couple;

/* The database environment. */
struct MDBX_env {
  /* ----------------------------------------------------- mostly static part */
#define MDBX_ME_SIGNATURE UINT32_C(0x9A899641)
  MDBX_atomic_uint32_t me_signature;
  /* Failed to update the meta page. Probably an I/O error. */
#define MDBX_FATAL_ERROR UINT32_C(0x80000000)
  /* Some fields are initialized. */
#define MDBX_ENV_ACTIVE UINT32_C(0x20000000)
  /* me_txkey is set */
#define MDBX_ENV_TXKEY UINT32_C(0x10000000)
  /* Legacy MDBX_MAPASYNC (prior v0.9) */
#define MDBX_DEPRECATED_MAPASYNC UINT32_C(0x100000)
  /* Legacy MDBX_COALESCE (prior v0.12) */
#define MDBX_DEPRECATED_COALESCE UINT32_C(0x2000000)
#define ENV_INTERNAL_FLAGS (MDBX_FATAL_ERROR | MDBX_ENV_ACTIVE | MDBX_ENV_TXKEY)
  uint32_t me_flags;
  osal_mmap_t me_dxb_mmap; /* The main data file */
#define me_map me_dxb_mmap.base
#define me_lazy_fd me_dxb_mmap.fd
  mdbx_filehandle_t me_dsync_fd, me_fd4meta;
#if defined(_WIN32) || defined(_WIN64)
#define me_overlapped_fd me_ioring.overlapped_fd
  HANDLE me_data_lock_event;
#endif                     /* Windows */
  osal_mmap_t me_lck_mmap; /* The lock file */
#define me_lfd me_lck_mmap.fd
  struct MDBX_lockinfo *me_lck;

  unsigned me_psize;          /* DB page size, initialized from me_os_psize */
  unsigned me_leaf_nodemax;   /* max size of a leaf-node */
  unsigned me_branch_nodemax; /* max size of a branch-node */
  atomic_pgno_t me_mlocked_pgno;
  uint8_t me_psize2log; /* log2 of DB page size */
  int8_t me_stuck_meta; /* recovery-only: target meta page or less that zero */
  uint16_t me_merge_threshold,
      me_merge_threshold_gc;  /* pages emptier than this are candidates for
                                 merging */
  unsigned me_os_psize;       /* OS page size, from osal_syspagesize() */
  unsigned me_maxreaders;     /* size of the reader table */
  MDBX_dbi me_maxdbs;         /* size of the DB table */
  uint32_t me_pid;            /* process ID of this env */
  osal_thread_key_t me_txkey; /* thread-key for readers */
  pathchar_t *me_pathname;    /* path to the DB files */
  void *me_pbuf;              /* scratch area for DUPSORT put() */
  MDBX_txn *me_txn0;          /* preallocated write transaction */

  MDBX_dbx *me_dbxs;                /* array of static DB info */
  uint16_t *me_dbflags;             /* array of flags from MDBX_db.md_flags */
  MDBX_atomic_uint32_t *me_dbiseqs; /* array of dbi sequence numbers */
  unsigned
      me_maxgc_ov1page; /* Number of pgno_t fit in a single overflow page */
  unsigned me_maxgc_per_branch;
  uint32_t me_live_reader;        /* have liveness lock in reader table */
  void *me_userctx;               /* User-settable context */
  MDBX_hsr_func *me_hsr_callback; /* Callback for kicking laggard readers */
  size_t me_madv_threshold;

  struct {
    unsigned dp_reserve_limit;
    unsigned rp_augment_limit;
    unsigned dp_limit;
    unsigned dp_initial;
    uint8_t dp_loose_limit;
    uint8_t spill_max_denominator;
    uint8_t spill_min_denominator;
    uint8_t spill_parent4child_denominator;
    unsigned merge_threshold_16dot16_percent;
#if !(defined(_WIN32) || defined(_WIN64))
    unsigned writethrough_threshold;
#endif /* Windows */
    bool prefault_write;
    union {
      unsigned all;
      /* tracks options with non-auto values but tuned by user */
      struct {
        unsigned dp_limit : 1;
        unsigned rp_augment_limit : 1;
        unsigned prefault_write : 1;
      } non_auto;
    } flags;
  } me_options;

  /* struct me_dbgeo used for accepting db-geo params from user for the new
   * database creation, i.e. when mdbx_env_set_geometry() was called before
   * mdbx_env_open(). */
  struct {
    size_t lower;  /* minimal size of datafile */
    size_t upper;  /* maximal size of datafile */
    size_t now;    /* current size of datafile */
    size_t grow;   /* step to grow datafile */
    size_t shrink; /* threshold to shrink datafile */
  } me_dbgeo;

#if MDBX_LOCKING == MDBX_LOCKING_SYSV
  union {
    key_t key;
    int semid;
  } me_sysv_ipc;
#endif /* MDBX_LOCKING == MDBX_LOCKING_SYSV */
  bool me_incore;

  MDBX_env *me_lcklist_next;

  /* --------------------------------------------------- mostly volatile part */

  MDBX_txn *me_txn; /* current write transaction */
  osal_fastmutex_t me_dbi_lock;
  MDBX_dbi me_numdbs; /* number of DBs opened */
  bool me_prefault_write;

  MDBX_page *me_dp_reserve; /* list of malloc'ed blocks for re-use */
  unsigned me_dp_reserve_len;
  /* PNL of pages that became unused in a write txn */
  MDBX_PNL me_retired_pages;
  osal_ioring_t me_ioring;

#if defined(_WIN32) || defined(_WIN64)
  osal_srwlock_t me_remap_guard;
  /* Workaround for LockFileEx and WriteFile multithread bug */
  CRITICAL_SECTION me_windowsbug_lock;
  char *me_pathname_char; /* cache of multi-byte representation of pathname
                             to the DB files */
#else
  osal_fastmutex_t me_remap_guard;
#endif

  /* -------------------------------------------------------------- debugging */

#if MDBX_DEBUG
  MDBX_assert_func *me_assert_func; /*  Callback for assertion failures */
#endif
#ifdef MDBX_USE_VALGRIND
  int me_valgrind_handle;
#endif
#if defined(MDBX_USE_VALGRIND) || defined(__SANITIZE_ADDRESS__)
  MDBX_atomic_uint32_t me_ignore_EDEADLK;
  pgno_t me_poison_edge;
#endif /* MDBX_USE_VALGRIND || __SANITIZE_ADDRESS__ */

#ifndef xMDBX_DEBUG_SPILLING
#define xMDBX_DEBUG_SPILLING 0
#endif
#if xMDBX_DEBUG_SPILLING == 2
  size_t debug_dirtied_est, debug_dirtied_act;
#endif /* xMDBX_DEBUG_SPILLING */

  /* ------------------------------------------------- stub for lck-less mode */
  MDBX_atomic_uint64_t
      x_lckless_stub[(sizeof(MDBX_lockinfo) + MDBX_CACHELINE_SIZE - 1) /
                     sizeof(MDBX_atomic_uint64_t)];
};

#ifndef __cplusplus
/*----------------------------------------------------------------------------*/
/* Cache coherence and mmap invalidation */

#if MDBX_CPU_WRITEBACK_INCOHERENT
#define osal_flush_incoherent_cpu_writeback() osal_memory_barrier()
#else
#define osal_flush_incoherent_cpu_writeback() osal_compiler_barrier()
#endif /* MDBX_CPU_WRITEBACK_INCOHERENT */

MDBX_MAYBE_UNUSED static __inline void
osal_flush_incoherent_mmap(const void *addr, size_t nbytes,
                           const intptr_t pagesize) {
#if MDBX_MMAP_INCOHERENT_FILE_WRITE
  char *const begin = (char *)(-pagesize & (intptr_t)addr);
  char *const end =
      (char *)(-pagesize & (intptr_t)((char *)addr + nbytes + pagesize - 1));
  int err = msync(begin, end - begin, MS_SYNC | MS_INVALIDATE) ? errno : 0;
  eASSERT(nullptr, err == 0);
  (void)err;
#else
  (void)pagesize;
#endif /* MDBX_MMAP_INCOHERENT_FILE_WRITE */

#if MDBX_MMAP_INCOHERENT_CPU_CACHE
#ifdef DCACHE
  /* MIPS has cache coherency issues.
   * Note: for any nbytes >= on-chip cache size, entire is flushed. */
  cacheflush((void *)addr, nbytes, DCACHE);
#else
#error "Oops, cacheflush() not available"
#endif /* DCACHE */
#endif /* MDBX_MMAP_INCOHERENT_CPU_CACHE */

#if !MDBX_MMAP_INCOHERENT_FILE_WRITE && !MDBX_MMAP_INCOHERENT_CPU_CACHE
  (void)addr;
  (void)nbytes;
#endif
}

/*----------------------------------------------------------------------------*/
/* Internal prototypes */

MDBX_INTERNAL_FUNC int cleanup_dead_readers(MDBX_env *env, int rlocked,
                                            int *dead);
MDBX_INTERNAL_FUNC int rthc_alloc(osal_thread_key_t *key, MDBX_reader *begin,
                                  MDBX_reader *end);
MDBX_INTERNAL_FUNC void rthc_remove(const osal_thread_key_t key);

MDBX_INTERNAL_FUNC void global_ctor(void);
MDBX_INTERNAL_FUNC void osal_ctor(void);
MDBX_INTERNAL_FUNC void global_dtor(void);
MDBX_INTERNAL_FUNC void osal_dtor(void);
MDBX_INTERNAL_FUNC void thread_dtor(void *ptr);

#endif /* !__cplusplus */

#define MDBX_IS_ERROR(rc)                                                      \
  ((rc) != MDBX_RESULT_TRUE && (rc) != MDBX_RESULT_FALSE)

/* Internal error codes, not exposed outside libmdbx */
#define MDBX_NO_ROOT (MDBX_LAST_ADDED_ERRCODE + 10)

/* Debugging output value of a cursor DBI: Negative in a sub-cursor. */
#define DDBI(mc)                                                               \
  (((mc)->mc_flags & C_SUB) ? -(int)(mc)->mc_dbi : (int)(mc)->mc_dbi)

/* Key size which fits in a DKBUF (debug key buffer). */
#define DKBUF_MAX 511
#define DKBUF char _kbuf[DKBUF_MAX * 4 + 2]
#define DKEY(x) mdbx_dump_val(x, _kbuf, DKBUF_MAX * 2 + 1)
#define DVAL(x) mdbx_dump_val(x, _kbuf + DKBUF_MAX * 2 + 1, DKBUF_MAX * 2 + 1)

#if MDBX_DEBUG
#define DKBUF_DEBUG DKBUF
#define DKEY_DEBUG(x) DKEY(x)
#define DVAL_DEBUG(x) DVAL(x)
#else
#define DKBUF_DEBUG ((void)(0))
#define DKEY_DEBUG(x) ("-")
#define DVAL_DEBUG(x) ("-")
#endif

/* An invalid page number.
 * Mainly used to denote an empty tree. */
#define P_INVALID (~(pgno_t)0)

/* Test if the flags f are set in a flag word w. */
#define F_ISSET(w, f) (((w) & (f)) == (f))

/* Round n up to an even number. */
#define EVEN(n) (((n) + 1UL) & -2L) /* sign-extending -2 to match n+1U */

/* Default size of memory map.
 * This is certainly too small for any actual applications. Apps should
 * always set the size explicitly using mdbx_env_set_geometry(). */
#define DEFAULT_MAPSIZE MEGABYTE

/* Number of slots in the reader table.
 * This value was chosen somewhat arbitrarily. The 61 is a prime number,
 * and such readers plus a couple mutexes fit into single 4KB page.
 * Applications should set the table size using mdbx_env_set_maxreaders(). */
#define DEFAULT_READERS 61

/* Test if a page is a leaf page */
#define IS_LEAF(p) (((p)->mp_flags & P_LEAF) != 0)
/* Test if a page is a LEAF2 page */
#define IS_LEAF2(p) unlikely(((p)->mp_flags & P_LEAF2) != 0)
/* Test if a page is a branch page */
#define IS_BRANCH(p) (((p)->mp_flags & P_BRANCH) != 0)
/* Test if a page is an overflow page */
#define IS_OVERFLOW(p) unlikely(((p)->mp_flags & P_OVERFLOW) != 0)
/* Test if a page is a sub page */
#define IS_SUBP(p) (((p)->mp_flags & P_SUBP) != 0)

/* Header for a single key/data pair within a page.
 * Used in pages of type P_BRANCH and P_LEAF without P_LEAF2.
 * We guarantee 2-byte alignment for 'MDBX_node's.
 *
 * Leaf node flags describe node contents.  F_BIGDATA says the node's
 * data part is the page number of an overflow page with actual data.
 * F_DUPDATA and F_SUBDATA can be combined giving duplicate data in
 * a sub-page/sub-database, and named databases (just F_SUBDATA). */
typedef struct MDBX_node {
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
  union {
    uint32_t mn_dsize;
    uint32_t mn_pgno32;
  };
  uint8_t mn_flags; /* see mdbx_node flags */
  uint8_t mn_extra;
  uint16_t mn_ksize; /* key size */
#else
  uint16_t mn_ksize; /* key size */
  uint8_t mn_extra;
  uint8_t mn_flags; /* see mdbx_node flags */
  union {
    uint32_t mn_pgno32;
    uint32_t mn_dsize;
  };
#endif /* __BYTE_ORDER__ */

  /* mdbx_node Flags */
#define F_BIGDATA 0x01 /* data put on overflow page */
#define F_SUBDATA 0x02 /* data is a sub-database */
#define F_DUPDATA 0x04 /* data has duplicates */

  /* valid flags for mdbx_node_add() */
#define NODE_ADD_FLAGS (F_DUPDATA | F_SUBDATA | MDBX_RESERVE | MDBX_APPEND)

#if (defined(__STDC_VERSION__) && __STDC_VERSION__ >= 199901L) ||              \
    (!defined(__cplusplus) && defined(_MSC_VER))
  uint8_t mn_data[] /* key and data are appended here */;
#endif /* C99 */
} MDBX_node;

#define DB_PERSISTENT_FLAGS                                                    \
  (MDBX_REVERSEKEY | MDBX_DUPSORT | MDBX_INTEGERKEY | MDBX_DUPFIXED |          \
   MDBX_INTEGERDUP | MDBX_REVERSEDUP)

/* mdbx_dbi_open() flags */
#define DB_USABLE_FLAGS (DB_PERSISTENT_FLAGS | MDBX_CREATE | MDBX_DB_ACCEDE)

#define DB_VALID 0x8000 /* DB handle is valid, for me_dbflags */
#define DB_INTERNAL_FLAGS DB_VALID

#if DB_INTERNAL_FLAGS & DB_USABLE_FLAGS
#error "Oops, some flags overlapped or wrong"
#endif
#if DB_PERSISTENT_FLAGS & ~DB_USABLE_FLAGS
#error "Oops, some flags overlapped or wrong"
#endif

/* Max length of iov-vector passed to writev() call, used for auxilary writes */
#define MDBX_AUXILARY_IOV_MAX 64
#if defined(IOV_MAX) && IOV_MAX < MDBX_AUXILARY_IOV_MAX
#undef MDBX_AUXILARY_IOV_MAX
#define MDBX_AUXILARY_IOV_MAX IOV_MAX
#endif /* MDBX_AUXILARY_IOV_MAX */

/*
 *                /
 *                | -1, a < b
 * CMP2INT(a,b) = <  0, a == b
 *                |  1, a > b
 *                \
 */
#define CMP2INT(a, b) (((a) != (b)) ? (((a) < (b)) ? -1 : 1) : 0)

MDBX_MAYBE_UNUSED MDBX_NOTHROW_CONST_FUNCTION static __inline pgno_t
int64pgno(int64_t i64) {
  if (likely(i64 >= (int64_t)MIN_PAGENO && i64 <= (int64_t)MAX_PAGENO + 1))
    return (pgno_t)i64;
  return (i64 < (int64_t)MIN_PAGENO) ? MIN_PAGENO : MAX_PAGENO;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_CONST_FUNCTION static __inline pgno_t
pgno_add(size_t base, size_t augend) {
  assert(base <= MAX_PAGENO + 1 && augend < MAX_PAGENO);
  return int64pgno((int64_t)base + (int64_t)augend);
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_CONST_FUNCTION static __inline pgno_t
pgno_sub(size_t base, size_t subtrahend) {
  assert(base >= MIN_PAGENO && base <= MAX_PAGENO + 1 &&
         subtrahend < MAX_PAGENO);
  return int64pgno((int64_t)base - (int64_t)subtrahend);
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_CONST_FUNCTION static __always_inline bool
is_powerof2(size_t x) {
  return (x & (x - 1)) == 0;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_CONST_FUNCTION static __always_inline size_t
floor_powerof2(size_t value, size_t granularity) {
  assert(is_powerof2(granularity));
  return value & ~(granularity - 1);
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_CONST_FUNCTION static __always_inline size_t
ceil_powerof2(size_t value, size_t granularity) {
  return floor_powerof2(value + granularity - 1, granularity);
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_CONST_FUNCTION static unsigned
log2n_powerof2(size_t value_uintptr) {
  assert(value_uintptr > 0 && value_uintptr < INT32_MAX &&
         is_powerof2(value_uintptr));
  assert((value_uintptr & -(intptr_t)value_uintptr) == value_uintptr);
  const uint32_t value_uint32 = (uint32_t)value_uintptr;
#if __GNUC_PREREQ(4, 1) || __has_builtin(__builtin_ctz)
  STATIC_ASSERT(sizeof(value_uint32) <= sizeof(unsigned));
  return __builtin_ctz(value_uint32);
#elif defined(_MSC_VER)
  unsigned long index;
  STATIC_ASSERT(sizeof(value_uint32) <= sizeof(long));
  _BitScanForward(&index, value_uint32);
  return index;
#else
  static const uint8_t debruijn_ctz32[32] = {
      0,  1,  28, 2,  29, 14, 24, 3, 30, 22, 20, 15, 25, 17, 4,  8,
      31, 27, 13, 23, 21, 19, 16, 7, 26, 12, 18, 6,  11, 5,  10, 9};
  return debruijn_ctz32[(uint32_t)(value_uint32 * 0x077CB531ul) >> 27];
#endif
}

/* Only a subset of the mdbx_env flags can be changed
 * at runtime. Changing other flags requires closing the
 * environment and re-opening it with the new flags. */
#define ENV_CHANGEABLE_FLAGS                                                   \
  (MDBX_SAFE_NOSYNC | MDBX_NOMETASYNC | MDBX_DEPRECATED_MAPASYNC |             \
   MDBX_NOMEMINIT | MDBX_COALESCE | MDBX_PAGEPERTURB | MDBX_ACCEDE |           \
   MDBX_VALIDATION)
#define ENV_CHANGELESS_FLAGS                                                   \
  (MDBX_NOSUBDIR | MDBX_RDONLY | MDBX_WRITEMAP | MDBX_NOTLS | MDBX_NORDAHEAD | \
   MDBX_LIFORECLAIM | MDBX_EXCLUSIVE)
#define ENV_USABLE_FLAGS (ENV_CHANGEABLE_FLAGS | ENV_CHANGELESS_FLAGS)

#if !defined(__cplusplus) || CONSTEXPR_ENUM_FLAGS_OPERATIONS
MDBX_MAYBE_UNUSED static void static_checks(void) {
  STATIC_ASSERT_MSG(INT16_MAX - CORE_DBS == MDBX_MAX_DBI,
                    "Oops, MDBX_MAX_DBI or CORE_DBS?");
  STATIC_ASSERT_MSG((unsigned)(MDBX_DB_ACCEDE | MDBX_CREATE) ==
                        ((DB_USABLE_FLAGS | DB_INTERNAL_FLAGS) &
                         (ENV_USABLE_FLAGS | ENV_INTERNAL_FLAGS)),
                    "Oops, some flags overlapped or wrong");
  STATIC_ASSERT_MSG((ENV_INTERNAL_FLAGS & ENV_USABLE_FLAGS) == 0,
                    "Oops, some flags overlapped or wrong");
}
#endif /* Disabled for MSVC 19.0 (VisualStudio 2015) */

#ifdef __cplusplus
}
#endif

#define MDBX_ASAN_POISON_MEMORY_REGION(addr, size)                             \
  do {                                                                         \
    TRACE("POISON_MEMORY_REGION(%p, %zu) at %u", (void *)(addr),               \
          (size_t)(size), __LINE__);                                           \
    ASAN_POISON_MEMORY_REGION(addr, size);                                     \
  } while (0)

#define MDBX_ASAN_UNPOISON_MEMORY_REGION(addr, size)                           \
  do {                                                                         \
    TRACE("UNPOISON_MEMORY_REGION(%p, %zu) at %u", (void *)(addr),             \
          (size_t)(size), __LINE__);                                           \
    ASAN_UNPOISON_MEMORY_REGION(addr, size);                                   \
  } while (0)

#include <ctype.h>

#if defined(_WIN32) || defined(_WIN64)
/*
 * POSIX getopt for Windows
 *
 * AT&T Public License
 *
 * Code given out at the 1985 UNIFORUM conference in Dallas.
 */

/*----------------------------------------------------------------------------*/
/* Microsoft compiler generates a lot of warning for self includes... */

#ifdef _MSC_VER
#pragma warning(push, 1)
#pragma warning(disable : 4548) /* expression before comma has no effect;      \
                                   expected expression with side - effect */
#pragma warning(disable : 4530) /* C++ exception handler used, but unwind      \
                                 * semantics are not enabled. Specify /EHsc */
#pragma warning(disable : 4577) /* 'noexcept' used with no exception handling  \
                                 * mode specified; termination on exception is \
                                 * not guaranteed. Specify /EHsc */
#if !defined(_CRT_SECURE_NO_WARNINGS)
#define _CRT_SECURE_NO_WARNINGS
#endif
#endif /* _MSC_VER (warnings) */

#include <stdio.h>
#include <string.h>

#ifdef _MSC_VER
#pragma warning(pop)
#endif
/*----------------------------------------------------------------------------*/

#ifndef NULL
#define NULL 0
#endif

#ifndef EOF
#define EOF (-1)
#endif

int optind = 1;
int optopt;
char *optarg;

int getopt(int argc, char *const argv[], const char *opts) {
  static int sp = 1;
  int c;
  const char *cp;

  if (sp == 1) {
    if (optind >= argc || argv[optind][0] != '-' || argv[optind][1] == '\0')
      return EOF;
    else if (strcmp(argv[optind], "--") == 0) {
      optind++;
      return EOF;
    }
  }
  optopt = c = argv[optind][sp];
  if (c == ':' || (cp = strchr(opts, c)) == NULL) {
    fprintf(stderr, "%s: %s -- %c\n", argv[0], "illegal option", c);
    if (argv[optind][++sp] == '\0') {
      optind++;
      sp = 1;
    }
    return '?';
  }
  if (*++cp == ':') {
    if (argv[optind][sp + 1] != '\0')
      optarg = &argv[optind++][sp + 1];
    else if (++optind >= argc) {
      fprintf(stderr, "%s: %s -- %c\n", argv[0], "option requires an argument",
              c);
      sp = 1;
      return '?';
    } else
      optarg = argv[optind++];
    sp = 1;
  } else {
    if (argv[optind][++sp] == '\0') {
      sp = 1;
      optind++;
    }
    optarg = NULL;
  }
  return c;
}

static volatile BOOL user_break;
static BOOL WINAPI ConsoleBreakHandlerRoutine(DWORD dwCtrlType) {
  (void)dwCtrlType;
  user_break = true;
  return true;
}

#else /* WINDOWS */

static volatile sig_atomic_t user_break;
static void signal_handler(int sig) {
  (void)sig;
  user_break = 1;
}

#endif /* !WINDOWS */

static char *prog;
bool quiet = false;
static void usage(void) {
  fprintf(stderr,
          "usage: %s [-V] [-q] [-d] [-s name] dbpath\n"
          "  -V\t\tprint version and exit\n"
          "  -q\t\tbe quiet\n"
          "  -d\t\tdelete the specified database, don't just empty it\n"
          "  -s name\tdrop the specified named subDB\n"
          "  \t\tby default empty the main DB\n",
          prog);
  exit(EXIT_FAILURE);
}

static void error(const char *func, int rc) {
  if (!quiet)
    fprintf(stderr, "%s: %s() error %d %s\n", prog, func, rc,
            mdbx_strerror(rc));
}

int main(int argc, char *argv[]) {
  int i, rc;
  MDBX_env *env;
  MDBX_txn *txn;
  MDBX_dbi dbi;
  char *envname = nullptr;
  char *subname = nullptr;
  int envflags = MDBX_ACCEDE;
  bool delete = false;

  prog = argv[0];
  if (argc < 2)
    usage();

  while ((i = getopt(argc, argv,
                     "d"
                     "s:"
                     "n"
                     "q"
                     "V")) != EOF) {
    switch (i) {
    case 'V':
      printf("mdbx_drop version %d.%d.%d.%d\n"
             " - source: %s %s, commit %s, tree %s\n"
             " - anchor: %s\n"
             " - build: %s for %s by %s\n"
             " - flags: %s\n"
             " - options: %s\n",
             mdbx_version.major, mdbx_version.minor, mdbx_version.release,
             mdbx_version.revision, mdbx_version.git.describe,
             mdbx_version.git.datetime, mdbx_version.git.commit,
             mdbx_version.git.tree, mdbx_sourcery_anchor, mdbx_build.datetime,
             mdbx_build.target, mdbx_build.compiler, mdbx_build.flags,
             mdbx_build.options);
      return EXIT_SUCCESS;
    case 'q':
      quiet = true;
      break;
    case 'd':
      delete = true;
      break;
    case 'n':
      break;
    case 's':
      subname = optarg;
      break;
    default:
      usage();
    }
  }

  if (optind != argc - 1)
    usage();

#if defined(_WIN32) || defined(_WIN64)
  SetConsoleCtrlHandler(ConsoleBreakHandlerRoutine, true);
#else
#ifdef SIGPIPE
  signal(SIGPIPE, signal_handler);
#endif
#ifdef SIGHUP
  signal(SIGHUP, signal_handler);
#endif
  signal(SIGINT, signal_handler);
  signal(SIGTERM, signal_handler);
#endif /* !WINDOWS */

  envname = argv[optind];
  if (!quiet) {
    printf("mdbx_drop %s (%s, T-%s)\nRunning for %s/%s...\n",
           mdbx_version.git.describe, mdbx_version.git.datetime,
           mdbx_version.git.tree, envname, subname ? subname : "@MAIN");
    fflush(nullptr);
  }

  rc = mdbx_env_create(&env);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_env_create", rc);
    return EXIT_FAILURE;
  }

  if (subname) {
    rc = mdbx_env_set_maxdbs(env, 2);
    if (unlikely(rc != MDBX_SUCCESS)) {
      error("mdbx_env_set_maxdbs", rc);
      goto env_close;
    }
  }

  rc = mdbx_env_open(env, envname, envflags, 0);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_env_open", rc);
    goto env_close;
  }

  rc = mdbx_txn_begin(env, NULL, 0, &txn);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_txn_begin", rc);
    goto env_close;
  }

  rc = mdbx_dbi_open(txn, subname, MDBX_DB_ACCEDE, &dbi);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_dbi_open", rc);
    goto txn_abort;
  }

  rc = mdbx_drop(txn, dbi, delete);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_drop", rc);
    goto txn_abort;
  }

  rc = mdbx_txn_commit(txn);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_txn_commit", rc);
    goto txn_abort;
  }
  txn = nullptr;

txn_abort:
  if (txn)
    mdbx_txn_abort(txn);
env_close:
  mdbx_env_close(env);

  return rc ? EXIT_FAILURE : EXIT_SUCCESS;
}
