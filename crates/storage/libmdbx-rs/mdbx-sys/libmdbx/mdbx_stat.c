/// \copyright SPDX-License-Identifier: Apache-2.0
/// \note Please refer to the COPYRIGHT file for explanations license change,
/// credits and acknowledgments.
/// \author Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> \date 2015-2025
///
/// mdbx_stat.c - memory-mapped database status tool
///

/* clang-format off */
#ifdef _MSC_VER
#if _MSC_VER > 1800
#pragma warning(disable : 4464) /* relative include path contains '..' */
#endif
#pragma warning(disable : 4996) /* The POSIX name is deprecated... */
#endif                          /* _MSC_VER (warnings) */

#define xMDBX_TOOLS /* Avoid using internal eASSERT() */
/// \copyright SPDX-License-Identifier: Apache-2.0
/// \author Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> \date 2015-2025

#define MDBX_BUILD_SOURCERY 4df7f8f177aee7f9f94c4e72f0d732384e9a870d7d79b8142abdeb4633e710cd_v0_13_6_0_ga971c76a

#define LIBMDBX_INTERNALS
#define MDBX_DEPRECATED

#ifdef MDBX_CONFIG_H
#include MDBX_CONFIG_H
#endif

/* Undefine the NDEBUG if debugging is enforced by MDBX_DEBUG */
#if (defined(MDBX_DEBUG) && MDBX_DEBUG > 0) || (defined(MDBX_FORCE_ASSERTIONS) && MDBX_FORCE_ASSERTIONS)
#undef NDEBUG
#ifndef MDBX_DEBUG
/* Чтобы избежать включения отладки только из-за включения assert-проверок */
#define MDBX_DEBUG 0
#endif
#endif

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
#if !defined(_FILE_OFFSET_BITS) && !defined(__ANDROID_API__) && !defined(ANDROID)
#define _FILE_OFFSET_BITS 64
#endif /* _FILE_OFFSET_BITS */

#if defined(__APPLE__) && !defined(_DARWIN_C_SOURCE)
#define _DARWIN_C_SOURCE
#endif /* _DARWIN_C_SOURCE */

#if (defined(__MINGW__) || defined(__MINGW32__) || defined(__MINGW64__)) && !defined(__USE_MINGW_ANSI_STDIO)
#define __USE_MINGW_ANSI_STDIO 1
#endif /* MinGW */

#if defined(_WIN32) || defined(_WIN64) || defined(_WINDOWS)

#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0601 /* Windows 7 */
#endif                      /* _WIN32_WINNT */

#if !defined(_CRT_SECURE_NO_WARNINGS)
#define _CRT_SECURE_NO_WARNINGS
#endif /* _CRT_SECURE_NO_WARNINGS */
#if !defined(UNICODE)
#define UNICODE
#endif /* UNICODE */

#if !defined(_NO_CRT_STDIO_INLINE) && MDBX_BUILD_SHARED_LIBRARY && !defined(xMDBX_TOOLS) && MDBX_WITHOUT_MSVC_CRT
#define _NO_CRT_STDIO_INLINE
#endif /* _NO_CRT_STDIO_INLINE */

#elif !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif /* Windows */

#ifdef __cplusplus

#ifndef NOMINMAX
#define NOMINMAX
#endif /* NOMINMAX */

/* Workaround for modern libstdc++ with CLANG < 4.x */
#if defined(__SIZEOF_INT128__) && !defined(__GLIBCXX_TYPE_INT_N_0) && defined(__clang__) && __clang_major__ < 4
#define __GLIBCXX_BITSIZE_INT_N_0 128
#define __GLIBCXX_TYPE_INT_N_0 __int128
#endif /* Workaround for modern libstdc++ with CLANG < 4.x */

#ifdef _MSC_VER
/* Workaround for MSVC' header `extern "C"` vs `std::` redefinition bug */
#if defined(__SANITIZE_ADDRESS__) && !defined(_DISABLE_VECTOR_ANNOTATION)
#define _DISABLE_VECTOR_ANNOTATION
#endif /* _DISABLE_VECTOR_ANNOTATION */
#endif /* _MSC_VER */

#endif /* __cplusplus */

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
#error "At least \"Microsoft C/C++ Compiler\" version 19.00.24234 (Visual Studio 2015 Update 3) is required."
#endif
#if _MSC_VER > 1800
#pragma warning(disable : 4464) /* relative include path contains '..' */
#endif
#if _MSC_VER > 1913
#pragma warning(disable : 5045) /* will insert Spectre mitigation... */
#endif
#if _MSC_VER > 1914
#pragma warning(disable : 5105) /* winbase.h(9531): warning C5105: macro expansion                                     \
                                   producing 'defined' has undefined behavior */
#endif
#if _MSC_VER < 1920
/* avoid "error C2219: syntax error: type qualifier must be after '*'" */
#define __restrict
#endif
#if _MSC_VER > 1930
#pragma warning(disable : 6235) /* <expression> is always a constant */
#pragma warning(disable : 6237) /* <expression> is never evaluated and might                                           \
                                   have side effects */
#endif
#pragma warning(disable : 4710) /* 'xyz': function not inlined */
#pragma warning(disable : 4711) /* function 'xyz' selected for automatic                                               \
                                   inline expansion */
#pragma warning(disable : 4201) /* nonstandard extension used: nameless                                                \
                                   struct/union */
#pragma warning(disable : 4702) /* unreachable code */
#pragma warning(disable : 4706) /* assignment within conditional expression */
#pragma warning(disable : 4127) /* conditional expression is constant */
#pragma warning(disable : 4324) /* 'xyz': structure was padded due to                                                  \
                                   alignment specifier */
#pragma warning(disable : 4310) /* cast truncates constant value */
#pragma warning(disable : 4820) /* bytes padding added after data member for                                           \
                                   alignment */
#pragma warning(disable : 4548) /* expression before comma has no effect;                                              \
                                   expected expression with side - effect */
#pragma warning(disable : 4366) /* the result of the unary '&' operator may be                                         \
                                   unaligned */
#pragma warning(disable : 4200) /* nonstandard extension used: zero-sized                                              \
                                   array in struct/union */
#pragma warning(disable : 4204) /* nonstandard extension used: non-constant                                            \
                                   aggregate initializer */
#pragma warning(disable : 4505) /* unreferenced local function has been removed */
#endif                          /* _MSC_VER (warnings) */

#if defined(__GNUC__) && __GNUC__ < 9
#pragma GCC diagnostic ignored "-Wattributes"
#endif /* GCC < 9 */

/*----------------------------------------------------------------------------*/
/* Microsoft compiler generates a lot of warning for self includes... */

#ifdef _MSC_VER
#pragma warning(push, 1)
#pragma warning(disable : 4548) /* expression before comma has no effect;                                              \
                                   expected expression with side - effect */
#pragma warning(disable : 4530) /* C++ exception handler used, but unwind                                              \
                                 * semantics are not enabled. Specify /EHsc */
#pragma warning(disable : 4577) /* 'noexcept' used with no exception handling                                          \
                                 * mode specified; termination on exception is                                         \
                                 * not guaranteed. Specify /EHsc */
#endif                          /* _MSC_VER (warnings) */

/*----------------------------------------------------------------------------*/
/* basic C99 includes */

#include <inttypes.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

#include <assert.h>
#include <ctype.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

/*----------------------------------------------------------------------------*/
/* feature testing */

#ifndef __has_warning
#define __has_warning(x) (0)
#endif

#ifndef __has_include
#define __has_include(x) (0)
#endif

#ifndef __has_attribute
#define __has_attribute(x) (0)
#endif

#ifndef __has_cpp_attribute
#define __has_cpp_attribute(x) 0
#endif

#ifndef __has_feature
#define __has_feature(x) (0)
#endif

#ifndef __has_extension
#define __has_extension(x) (0)
#endif

#ifndef __has_builtin
#define __has_builtin(x) (0)
#endif

#if __has_feature(thread_sanitizer)
#define __SANITIZE_THREAD__ 1
#endif

#if __has_feature(address_sanitizer)
#define __SANITIZE_ADDRESS__ 1
#endif

#ifndef __GNUC_PREREQ
#if defined(__GNUC__) && defined(__GNUC_MINOR__)
#define __GNUC_PREREQ(maj, min) ((__GNUC__ << 16) + __GNUC_MINOR__ >= ((maj) << 16) + (min))
#else
#define __GNUC_PREREQ(maj, min) (0)
#endif
#endif /* __GNUC_PREREQ */

#ifndef __CLANG_PREREQ
#ifdef __clang__
#define __CLANG_PREREQ(maj, min) ((__clang_major__ << 16) + __clang_minor__ >= ((maj) << 16) + (min))
#else
#define __CLANG_PREREQ(maj, min) (0)
#endif
#endif /* __CLANG_PREREQ */

#ifndef __GLIBC_PREREQ
#if defined(__GLIBC__) && defined(__GLIBC_MINOR__)
#define __GLIBC_PREREQ(maj, min) ((__GLIBC__ << 16) + __GLIBC_MINOR__ >= ((maj) << 16) + (min))
#else
#define __GLIBC_PREREQ(maj, min) (0)
#endif
#endif /* __GLIBC_PREREQ */

/*----------------------------------------------------------------------------*/
/* pre-requirements */

#if (-6 & 5) || CHAR_BIT != 8 || UINT_MAX < 0xffffffff || ULONG_MAX % 0xFFFF
#error "Sanity checking failed: Two's complement, reasonably sized integer types"
#endif

#ifndef SSIZE_MAX
#define SSIZE_MAX INTPTR_MAX
#endif

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
#warning "libmdbx don't compatible with ThreadSanitizer, you will get a lot of false-positive issues."
#endif /* __SANITIZE_THREAD__ */

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

#if !defined(nullptr) && !defined(__cplusplus) || (__cplusplus < 201103L && !defined(_MSC_VER))
#define nullptr NULL
#endif

#if defined(__APPLE__) || defined(_DARWIN_C_SOURCE)
#include <AvailabilityMacros.h>
#include <TargetConditionals.h>
#ifndef MAC_OS_X_VERSION_MIN_REQUIRED
#define MAC_OS_X_VERSION_MIN_REQUIRED 1070 /* Mac OS X 10.7, 2011 */
#endif
#endif /* Apple OSX & iOS */

#if defined(__FreeBSD__) || defined(__NetBSD__) || defined(__OpenBSD__) || defined(__BSD__) || defined(__bsdi__) ||    \
    defined(__DragonFly__) || defined(__APPLE__) || defined(__MACH__)
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
#if !(defined(__sun) || defined(__SVR4) || defined(__svr4__) || defined(_WIN32) || defined(_WIN64))
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
#include <windows.h>
#include <winnt.h>
#include <winternl.h>

/* После подгрузки windows.h, чтобы избежать проблем со сборкой MINGW и т.п. */
#include <excpt.h>
#include <tlhelp32.h>

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

#if defined(i386) || defined(__386) || defined(__i386) || defined(__i386__) || defined(i486) || defined(__i486) ||     \
    defined(__i486__) || defined(i586) || defined(__i586) || defined(__i586__) || defined(i686) || defined(__i686) ||  \
    defined(__i686__) || defined(_M_IX86) || defined(_X86_) || defined(__THW_INTEL__) || defined(__I86__) ||           \
    defined(__INTEL__) || defined(__x86_64) || defined(__x86_64__) || defined(__amd64__) || defined(__amd64) ||        \
    defined(_M_X64) || defined(_M_AMD64) || defined(__IA32__) || defined(__INTEL__)
#ifndef __ia32__
/* LY: define neutral __ia32__ for x86 and x86-64 */
#define __ia32__ 1
#endif /* __ia32__ */
#if !defined(__amd64__) &&                                                                                             \
    (defined(__x86_64) || defined(__x86_64__) || defined(__amd64) || defined(_M_X64) || defined(_M_AMD64))
/* LY: define trusty __amd64__ for all AMD64/x86-64 arch */
#define __amd64__ 1
#endif /* __amd64__ */
#endif /* all x86 */

#if !defined(__BYTE_ORDER__) || !defined(__ORDER_LITTLE_ENDIAN__) || !defined(__ORDER_BIG_ENDIAN__)

#if defined(__GLIBC__) || defined(__GNU_LIBRARY__) || defined(__ANDROID_API__) || defined(HAVE_ENDIAN_H) ||            \
    __has_include(<endian.h>)
#include <endian.h>
#elif defined(__APPLE__) || defined(__MACH__) || defined(__OpenBSD__) || defined(HAVE_MACHINE_ENDIAN_H) ||             \
    __has_include(<machine/endian.h>)
#include <machine/endian.h>
#elif defined(HAVE_SYS_ISA_DEFS_H) || __has_include(<sys/isa_defs.h>)
#include <sys/isa_defs.h>
#elif (defined(HAVE_SYS_TYPES_H) && defined(HAVE_SYS_ENDIAN_H)) ||                                                     \
    (__has_include(<sys/types.h>) && __has_include(<sys/endian.h>))
#include <sys/endian.h>
#include <sys/types.h>
#elif defined(__bsdi__) || defined(__DragonFly__) || defined(__FreeBSD__) || defined(__NetBSD__) ||                    \
    defined(HAVE_SYS_PARAM_H) || __has_include(<sys/param.h>)
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

#if defined(__LITTLE_ENDIAN__) || (defined(_LITTLE_ENDIAN) && !defined(_BIG_ENDIAN)) || defined(__ARMEL__) ||          \
    defined(__THUMBEL__) || defined(__AARCH64EL__) || defined(__MIPSEL__) || defined(_MIPSEL) || defined(__MIPSEL) ||  \
    defined(_M_ARM) || defined(_M_ARM64) || defined(__e2k__) || defined(__elbrus_4c__) || defined(__elbrus_8c__) ||    \
    defined(__bfin__) || defined(__BFIN__) || defined(__ia64__) || defined(_IA64) || defined(__IA64__) ||              \
    defined(__ia64) || defined(_M_IA64) || defined(__itanium__) || defined(__ia32__) || defined(__CYGWIN__) ||         \
    defined(_WIN64) || defined(_WIN32) || defined(__TOS_WIN__) || defined(__WINDOWS__)
#define __BYTE_ORDER__ __ORDER_LITTLE_ENDIAN__

#elif defined(__BIG_ENDIAN__) || (defined(_BIG_ENDIAN) && !defined(_LITTLE_ENDIAN)) || defined(__ARMEB__) ||           \
    defined(__THUMBEB__) || defined(__AARCH64EB__) || defined(__MIPSEB__) || defined(_MIPSEB) || defined(__MIPSEB) ||  \
    defined(__m68k__) || defined(M68000) || defined(__hppa__) || defined(__hppa) || defined(__HPPA__) ||               \
    defined(__sparc__) || defined(__sparc) || defined(__370__) || defined(__THW_370__) || defined(__s390__) ||         \
    defined(__s390x__) || defined(__SYSC_ZARCH__)
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
#elif defined(_M_ARM) || defined(_M_ARM64) || defined(__aarch64__) || defined(__aarch64) || defined(__arm__) ||        \
    defined(__arm) || defined(__CC_ARM)
#define MDBX_HAVE_CMOV 1
#elif (defined(__riscv__) || defined(__riscv64)) && (defined(__riscv_b) || defined(__riscv_bitmanip))
#define MDBX_HAVE_CMOV 1
#elif defined(i686) || defined(__i686) || defined(__i686__) || (defined(_M_IX86) && _M_IX86 > 600) ||                  \
    defined(__x86_64) || defined(__x86_64__) || defined(__amd64__) || defined(__amd64) || defined(_M_X64) ||           \
    defined(_M_AMD64)
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
#elif (defined(_HPUX_SOURCE) || defined(__hpux) || defined(__HP_aCC)) && (defined(HP_IA64) || defined(__ia64))
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
#define __noop                                                                                                         \
  do {                                                                                                                 \
  } while (0)
#endif /* __noop */

#if defined(__fallthrough) && (defined(__MINGW__) || defined(__MINGW32__) || defined(__MINGW64__))
#undef __fallthrough
#endif /* __fallthrough workaround for MinGW */

#ifndef __fallthrough
#if defined(__cplusplus) && (__has_cpp_attribute(fallthrough) && (!defined(__clang__) || __clang__ > 4)) ||            \
    __cplusplus >= 201703L
#define __fallthrough [[fallthrough]]
#elif __GNUC_PREREQ(8, 0) && defined(__cplusplus) && __cplusplus >= 201103L
#define __fallthrough [[fallthrough]]
#elif __GNUC_PREREQ(7, 0) && (!defined(__LCC__) || (__LCC__ == 124 && __LCC_MINOR__ >= 12) ||                          \
                              (__LCC__ == 125 && __LCC_MINOR__ >= 5) || (__LCC__ >= 126))
#define __fallthrough __attribute__((__fallthrough__))
#elif defined(__clang__) && defined(__cplusplus) && __cplusplus >= 201103L && __has_feature(cxx_attributes) &&         \
    __has_warning("-Wimplicit-fallthrough")
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
#define __unreachable()                                                                                                \
  do {                                                                                                                 \
  } while (1)
#endif
#endif /* __unreachable */

#ifndef __prefetch
#if defined(__GNUC__) || defined(__clang__) || __has_builtin(__builtin_prefetch)
#define __prefetch(ptr) __builtin_prefetch(ptr)
#else
#define __prefetch(ptr)                                                                                                \
  do {                                                                                                                 \
    (void)(ptr);                                                                                                       \
  } while (0)
#endif
#endif /* __prefetch */

#ifndef offsetof
#define offsetof(type, member) __builtin_offsetof(type, member)
#endif /* offsetof */

#ifndef container_of
#define container_of(ptr, type, member) ((type *)((char *)(ptr) - offsetof(type, member)))
#endif /* container_of */

/*----------------------------------------------------------------------------*/
/* useful attributes */

#ifndef __always_inline
#if defined(__GNUC__) || __has_attribute(__always_inline__)
#define __always_inline __inline __attribute__((__always_inline__))
#elif defined(_MSC_VER)
#define __always_inline __forceinline
#else
#define __always_inline __inline
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
#if defined(__clang__) && !__has_attribute(__hot__) && __has_attribute(__section__) &&                                 \
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
#if defined(__clang__) && !__has_attribute(__cold__) && __has_attribute(__section__) &&                                \
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
#if (defined(__GNUC__) || __has_builtin(__builtin_expect)) && !defined(__COVERITY__)
#define likely(cond) __builtin_expect(!!(cond), 1)
#else
#define likely(x) (!!(x))
#endif
#endif /* likely */

#ifndef unlikely
#if (defined(__GNUC__) || __has_builtin(__builtin_expect)) && !defined(__COVERITY__)
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

#ifndef MDBX_WEAK_IMPORT_ATTRIBUTE
#ifdef WEAK_IMPORT_ATTRIBUTE
#define MDBX_WEAK_IMPORT_ATTRIBUTE WEAK_IMPORT_ATTRIBUTE
#elif __has_attribute(__weak__) && __has_attribute(__weak_import__)
#define MDBX_WEAK_IMPORT_ATTRIBUTE __attribute__((__weak__, __weak_import__))
#elif __has_attribute(__weak__) || (defined(__GNUC__) && __GNUC__ >= 4 && defined(__ELF__))
#define MDBX_WEAK_IMPORT_ATTRIBUTE __attribute__((__weak__))
#else
#define MDBX_WEAK_IMPORT_ATTRIBUTE
#endif
#endif /* MDBX_WEAK_IMPORT_ATTRIBUTE */

#if !defined(__thread) && (defined(_MSC_VER) || defined(__DMC__))
#define __thread __declspec(thread)
#endif /* __thread */

#ifndef MDBX_EXCLUDE_FOR_GPROF
#ifdef ENABLE_GPROF
#define MDBX_EXCLUDE_FOR_GPROF __attribute__((__no_instrument_function__, __no_profile_instrument_function__))
#else
#define MDBX_EXCLUDE_FOR_GPROF
#endif /* ENABLE_GPROF */
#endif /* MDBX_EXCLUDE_FOR_GPROF */

/*----------------------------------------------------------------------------*/

#ifndef expect_with_probability
#if defined(__builtin_expect_with_probability) || __has_builtin(__builtin_expect_with_probability) ||                  \
    __GNUC_PREREQ(9, 0)
#define expect_with_probability(expr, value, prob) __builtin_expect_with_probability(expr, value, prob)
#else
#define expect_with_probability(expr, value, prob) (expr)
#endif
#endif /* expect_with_probability */

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
#define MDBX_SUPPRESS_GOOFY_MSVC_ANALYZER(warn_id) __pragma(prefast(suppress : warn_id))
#else
#define MDBX_SUPPRESS_GOOFY_MSVC_ANALYZER(warn_id) __pragma(warning(suppress : warn_id))
#endif
#else
#define MDBX_ANALYSIS_ASSUME(expr) assert(expr)
#define MDBX_SUPPRESS_GOOFY_MSVC_ANALYZER(warn_id)
#endif /* MDBX_GOOFY_MSVC_STATIC_ANALYZER */

#ifndef FLEXIBLE_ARRAY_MEMBERS
#if (defined(__STDC_VERSION__) && __STDC_VERSION__ >= 199901L) || (!defined(__cplusplus) && defined(_MSC_VER))
#define FLEXIBLE_ARRAY_MEMBERS 1
#else
#define FLEXIBLE_ARRAY_MEMBERS 0
#endif
#endif /* FLEXIBLE_ARRAY_MEMBERS */

/*----------------------------------------------------------------------------*/
/* Valgrind and Address Sanitizer */

#if defined(ENABLE_MEMCHECK)
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
#endif /* ENABLE_MEMCHECK */

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

#define MDBX_TETRAD(a, b, c, d) ((uint32_t)(a) << 24 | (uint32_t)(b) << 16 | (uint32_t)(c) << 8 | (d))

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
#elif (defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L) || __has_feature(c_static_assert)
#define STATIC_ASSERT_MSG(expr, msg) _Static_assert(expr, msg)
#else
#define STATIC_ASSERT_MSG(expr, msg)                                                                                   \
  switch (0) {                                                                                                         \
  case 0:                                                                                                              \
  case (expr):;                                                                                                        \
  }
#endif
#endif /* STATIC_ASSERT */

#ifndef STATIC_ASSERT
#define STATIC_ASSERT(expr) STATIC_ASSERT_MSG(expr, #expr)
#endif

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

/*----------------------------------------------------------------------------*/

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

#ifdef xMDBX_ALLOY
/* Amalgamated build */
#define MDBX_INTERNAL static
#else
/* Non-amalgamated build */
#define MDBX_INTERNAL
#endif /* xMDBX_ALLOY */

#include "mdbx.h"

/*----------------------------------------------------------------------------*/
/* Basic constants and types */

typedef struct iov_ctx iov_ctx_t;
///

/*----------------------------------------------------------------------------*/
/* Memory/Compiler barriers, cache coherence */

#if __has_include(<sys/cachectl.h>)
#include <sys/cachectl.h>
#elif defined(__mips) || defined(__mips__) || defined(__mips64) || defined(__mips64__) || defined(_M_MRX000) ||        \
    defined(_MIPS_) || defined(__MWERKS__) || defined(__sgi)
/* MIPS should have explicit cache control */
#include <sys/cachectl.h>
#endif

MDBX_MAYBE_UNUSED static inline void osal_compiler_barrier(void) {
#if defined(__clang__) || defined(__GNUC__)
  __asm__ __volatile__("" ::: "memory");
#elif defined(_MSC_VER)
  _ReadWriteBarrier();
#elif defined(__INTEL_COMPILER) /* LY: Intel Compiler may mimic GCC and MSC */
  __memory_barrier();
#elif defined(__SUNPRO_C) || defined(__sun) || defined(sun)
  __compiler_barrier();
#elif (defined(_HPUX_SOURCE) || defined(__hpux) || defined(__HP_aCC)) && (defined(HP_IA64) || defined(__ia64))
  _Asm_sched_fence(/* LY: no-arg meaning 'all expect ALU', e.g. 0x3D3D */);
#elif defined(_AIX) || defined(__ppc__) || defined(__powerpc__) || defined(__ppc64__) || defined(__powerpc64__)
  __fence();
#else
#error "Could not guess the kind of compiler, please report to us."
#endif
}

MDBX_MAYBE_UNUSED static inline void osal_memory_barrier(void) {
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
#elif (defined(_HPUX_SOURCE) || defined(__hpux) || defined(__HP_aCC)) && (defined(HP_IA64) || defined(__ia64))
  _Asm_mf();
#elif defined(_AIX) || defined(__ppc__) || defined(__powerpc__) || defined(__ppc64__) || defined(__powerpc64__)
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
#define MAP_FAILED nullptr
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
#define __except(COND) if (/* (void)(COND), */ false)
#endif /* stub for MSVC's __try/__except */

#if MDBX_WITHOUT_MSVC_CRT

#ifndef osal_malloc
static inline void *osal_malloc(size_t bytes) { return HeapAlloc(GetProcessHeap(), 0, bytes); }
#endif /* osal_malloc */

#ifndef osal_calloc
static inline void *osal_calloc(size_t nelem, size_t size) {
  return HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, nelem * size);
}
#endif /* osal_calloc */

#ifndef osal_realloc
static inline void *osal_realloc(void *ptr, size_t bytes) {
  return ptr ? HeapReAlloc(GetProcessHeap(), 0, ptr, bytes) : HeapAlloc(GetProcessHeap(), 0, bytes);
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
#define osal_malloc_usable_size(ptr) malloc_usable_size(ptr)
#elif defined(__APPLE__)
#define osal_malloc_usable_size(ptr) malloc_size(ptr)
#elif defined(_MSC_VER) && !MDBX_WITHOUT_MSVC_CRT
#define osal_malloc_usable_size(ptr) _msize(ptr)
#endif /* osal_malloc_usable_size */

/*----------------------------------------------------------------------------*/
/* OS abstraction layer stuff */

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
    struct shared_lck *lck;
  };
  mdbx_filehandle_t fd;
  size_t limit;   /* mapping length, but NOT a size of file nor DB */
  size_t current; /* mapped region size, i.e. the size of file and DB */
  uint64_t filesize /* in-process cache of a file size */;
#if defined(_WIN32) || defined(_WIN64)
  HANDLE section; /* memory-mapped section handle */
#endif
} osal_mmap_t;

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

#if defined(MAC_OS_X_VERSION_MIN_REQUIRED) && defined(MAC_OS_VERSION_11_0) &&                                          \
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
#define ior_sgv_gap4terminator 1
#define ior_sgv_element FILE_SEGMENT_ELEMENT
#else
  size_t offset;
#if MDBX_HAVE_PWRITEV
  size_t sgvcnt;
#define ior_sgv_gap4terminator 0
#define ior_sgv_element struct iovec
#endif /* MDBX_HAVE_PWRITEV */
#endif /* !Windows */
  union {
    MDBX_val single;
#if defined(ior_sgv_element)
    ior_sgv_element sgv[1 + ior_sgv_gap4terminator];
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

/* Actually this is not ioring for now, but on the way. */
MDBX_INTERNAL int osal_ioring_create(osal_ioring_t *
#if defined(_WIN32) || defined(_WIN64)
                                     ,
                                     bool enable_direct, mdbx_filehandle_t overlapped_fd
#endif /* Windows */
);
MDBX_INTERNAL int osal_ioring_resize(osal_ioring_t *, size_t items);
MDBX_INTERNAL void osal_ioring_destroy(osal_ioring_t *);
MDBX_INTERNAL void osal_ioring_reset(osal_ioring_t *);
MDBX_INTERNAL int osal_ioring_add(osal_ioring_t *ctx, const size_t offset, void *data, const size_t bytes);
typedef struct osal_ioring_write_result {
  int err;
  unsigned wops;
} osal_ioring_write_result_t;
MDBX_INTERNAL osal_ioring_write_result_t osal_ioring_write(osal_ioring_t *ior, mdbx_filehandle_t fd);

MDBX_INTERNAL void osal_ioring_walk(osal_ioring_t *ior, iov_ctx_t *ctx,
                                    void (*callback)(iov_ctx_t *ctx, size_t offset, void *data, size_t bytes));

MDBX_MAYBE_UNUSED static inline unsigned osal_ioring_left(const osal_ioring_t *ior) { return ior->slots_left; }

MDBX_MAYBE_UNUSED static inline unsigned osal_ioring_used(const osal_ioring_t *ior) {
  return ior->allocated - ior->slots_left;
}

MDBX_MAYBE_UNUSED static inline int osal_ioring_prepare(osal_ioring_t *ior, size_t items, size_t bytes) {
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

#if (!defined(__GLIBC__) && __GLIBC_PREREQ(2, 1)) && (defined(_GNU_SOURCE) || defined(_BSD_SOURCE))
#define osal_asprintf asprintf
#define osal_vasprintf vasprintf
#else
MDBX_MAYBE_UNUSED MDBX_INTERNAL MDBX_PRINTF_ARGS(2, 3) int osal_asprintf(char **strp, const char *fmt, ...);
MDBX_INTERNAL int osal_vasprintf(char **strp, const char *fmt, va_list ap);
#endif

#if !defined(MADV_DODUMP) && defined(MADV_CORE)
#define MADV_DODUMP MADV_CORE
#endif /* MADV_CORE -> MADV_DODUMP */

#if !defined(MADV_DONTDUMP) && defined(MADV_NOCORE)
#define MADV_DONTDUMP MADV_NOCORE
#endif /* MADV_NOCORE -> MADV_DONTDUMP */

MDBX_MAYBE_UNUSED MDBX_INTERNAL void osal_jitter(bool tiny);

/* max bytes to write in one call */
#if defined(_WIN64)
#define MAX_WRITE UINT32_C(0x10000000)
#elif defined(_WIN32)
#define MAX_WRITE UINT32_C(0x04000000)
#else
#define MAX_WRITE UINT32_C(0x3f000000)

#if defined(F_GETLK64) && defined(F_SETLK64) && defined(F_SETLKW64) && !defined(__ANDROID_API__)
#define MDBX_F_SETLK F_SETLK64
#define MDBX_F_SETLKW F_SETLKW64
#define MDBX_F_GETLK F_GETLK64
#if (__GLIBC_PREREQ(2, 28) && (defined(__USE_LARGEFILE64) || defined(__LARGEFILE64_SOURCE) ||                          \
                               defined(_USE_LARGEFILE64) || defined(_LARGEFILE64_SOURCE))) ||                          \
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

#if defined(F_OFD_SETLK64) && defined(F_OFD_SETLKW64) && defined(F_OFD_GETLK64) && !defined(__ANDROID_API__)
#define MDBX_F_OFD_SETLK F_OFD_SETLK64
#define MDBX_F_OFD_SETLKW F_OFD_SETLKW64
#define MDBX_F_OFD_GETLK F_OFD_GETLK64
#else
#define MDBX_F_OFD_SETLK F_OFD_SETLK
#define MDBX_F_OFD_SETLKW F_OFD_SETLKW
#define MDBX_F_OFD_GETLK F_OFD_GETLK
#ifndef OFF_T_MAX
#define OFF_T_MAX (((sizeof(off_t) > 4) ? INT64_MAX : INT32_MAX) & ~(size_t)0xFffff)
#endif /* OFF_T_MAX */
#endif /* MDBX_F_OFD_SETLK64, MDBX_F_OFD_SETLKW64, MDBX_F_OFD_GETLK64 */

#endif /* !Windows */

#ifndef osal_strdup
LIBMDBX_API char *osal_strdup(const char *str);
#endif

MDBX_MAYBE_UNUSED static inline int osal_get_errno(void) {
#if defined(_WIN32) || defined(_WIN64)
  DWORD rc = GetLastError();
#else
  int rc = errno;
#endif
  return rc;
}

#ifndef osal_memalign_alloc
MDBX_INTERNAL int osal_memalign_alloc(size_t alignment, size_t bytes, void **result);
#endif
#ifndef osal_memalign_free
MDBX_INTERNAL void osal_memalign_free(void *ptr);
#endif

MDBX_INTERNAL int osal_condpair_init(osal_condpair_t *condpair);
MDBX_INTERNAL int osal_condpair_lock(osal_condpair_t *condpair);
MDBX_INTERNAL int osal_condpair_unlock(osal_condpair_t *condpair);
MDBX_INTERNAL int osal_condpair_signal(osal_condpair_t *condpair, bool part);
MDBX_INTERNAL int osal_condpair_wait(osal_condpair_t *condpair, bool part);
MDBX_INTERNAL int osal_condpair_destroy(osal_condpair_t *condpair);

MDBX_INTERNAL int osal_fastmutex_init(osal_fastmutex_t *fastmutex);
MDBX_INTERNAL int osal_fastmutex_acquire(osal_fastmutex_t *fastmutex);
MDBX_INTERNAL int osal_fastmutex_release(osal_fastmutex_t *fastmutex);
MDBX_INTERNAL int osal_fastmutex_destroy(osal_fastmutex_t *fastmutex);

MDBX_INTERNAL int osal_pwritev(mdbx_filehandle_t fd, struct iovec *iov, size_t sgvcnt, uint64_t offset);
MDBX_INTERNAL int osal_pread(mdbx_filehandle_t fd, void *buf, size_t count, uint64_t offset);
MDBX_INTERNAL int osal_pwrite(mdbx_filehandle_t fd, const void *buf, size_t count, uint64_t offset);
MDBX_INTERNAL int osal_write(mdbx_filehandle_t fd, const void *buf, size_t count);

MDBX_INTERNAL int osal_thread_create(osal_thread_t *thread, THREAD_RESULT(THREAD_CALL *start_routine)(void *),
                                     void *arg);
MDBX_INTERNAL int osal_thread_join(osal_thread_t thread);

enum osal_syncmode_bits {
  MDBX_SYNC_NONE = 0,
  MDBX_SYNC_KICK = 1,
  MDBX_SYNC_DATA = 2,
  MDBX_SYNC_SIZE = 4,
  MDBX_SYNC_IODQ = 8
};

MDBX_INTERNAL int osal_fsync(mdbx_filehandle_t fd, const enum osal_syncmode_bits mode_bits);
MDBX_INTERNAL int osal_ftruncate(mdbx_filehandle_t fd, uint64_t length);
MDBX_INTERNAL int osal_fseek(mdbx_filehandle_t fd, uint64_t pos);
MDBX_INTERNAL int osal_filesize(mdbx_filehandle_t fd, uint64_t *length);

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

MDBX_MAYBE_UNUSED static inline bool osal_isdirsep(pathchar_t c) {
  return
#if defined(_WIN32) || defined(_WIN64)
      c == '\\' ||
#endif
      c == '/';
}

MDBX_INTERNAL bool osal_pathequal(const pathchar_t *l, const pathchar_t *r, size_t len);
MDBX_INTERNAL pathchar_t *osal_fileext(const pathchar_t *pathname, size_t len);
MDBX_INTERNAL int osal_fileexists(const pathchar_t *pathname);
MDBX_INTERNAL int osal_openfile(const enum osal_openfile_purpose purpose, const MDBX_env *env,
                                const pathchar_t *pathname, mdbx_filehandle_t *fd, mdbx_mode_t unix_mode_bits);
MDBX_INTERNAL int osal_closefile(mdbx_filehandle_t fd);
MDBX_INTERNAL int osal_removefile(const pathchar_t *pathname);
MDBX_INTERNAL int osal_removedirectory(const pathchar_t *pathname);
MDBX_INTERNAL int osal_is_pipe(mdbx_filehandle_t fd);
MDBX_INTERNAL int osal_lockfile(mdbx_filehandle_t fd, bool wait);

#define MMAP_OPTION_TRUNCATE 1
#define MMAP_OPTION_SEMAPHORE 2
MDBX_INTERNAL int osal_mmap(const int flags, osal_mmap_t *map, size_t size, const size_t limit, const unsigned options,
                            const pathchar_t *pathname4logging);
MDBX_INTERNAL int osal_munmap(osal_mmap_t *map);
#define MDBX_MRESIZE_MAY_MOVE 0x00000100
#define MDBX_MRESIZE_MAY_UNMAP 0x00000200
MDBX_INTERNAL int osal_mresize(const int flags, osal_mmap_t *map, size_t size, size_t limit);
#if defined(_WIN32) || defined(_WIN64)
typedef struct {
  unsigned limit, count;
  HANDLE handles[31];
} mdbx_handle_array_t;
MDBX_INTERNAL int osal_suspend_threads_before_remap(MDBX_env *env, mdbx_handle_array_t **array);
MDBX_INTERNAL int osal_resume_threads_after_remap(mdbx_handle_array_t *array);
#endif /* Windows */
MDBX_INTERNAL int osal_msync(const osal_mmap_t *map, size_t offset, size_t length, enum osal_syncmode_bits mode_bits);
MDBX_INTERNAL int osal_check_fs_rdonly(mdbx_filehandle_t handle, const pathchar_t *pathname, int err);
MDBX_INTERNAL int osal_check_fs_incore(mdbx_filehandle_t handle);
MDBX_INTERNAL int osal_check_fs_local(mdbx_filehandle_t handle, int flags);

MDBX_MAYBE_UNUSED static inline uint32_t osal_getpid(void) {
  STATIC_ASSERT(sizeof(mdbx_pid_t) <= sizeof(uint32_t));
#if defined(_WIN32) || defined(_WIN64)
  return GetCurrentProcessId();
#else
  STATIC_ASSERT(sizeof(pid_t) <= sizeof(uint32_t));
  return getpid();
#endif
}

MDBX_MAYBE_UNUSED static inline uintptr_t osal_thread_self(void) {
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
MDBX_INTERNAL int osal_check_tid4bionic(void);
#else
static inline int osal_check_tid4bionic(void) { return 0; }
#endif /* __ANDROID_API__ || ANDROID) || BIONIC */

MDBX_MAYBE_UNUSED static inline int osal_pthread_mutex_lock(pthread_mutex_t *mutex) {
  int err = osal_check_tid4bionic();
  return unlikely(err) ? err : pthread_mutex_lock(mutex);
}
#endif /* !Windows */

MDBX_INTERNAL uint64_t osal_monotime(void);
MDBX_INTERNAL uint64_t osal_cputime(size_t *optional_page_faults);
MDBX_INTERNAL uint64_t osal_16dot16_to_monotime(uint32_t seconds_16dot16);
MDBX_INTERNAL uint32_t osal_monotime_to_16dot16(uint64_t monotime);

MDBX_MAYBE_UNUSED static inline uint32_t osal_monotime_to_16dot16_noUnderflow(uint64_t monotime) {
  uint32_t seconds_16dot16 = osal_monotime_to_16dot16(monotime);
  return seconds_16dot16 ? seconds_16dot16 : /* fix underflow */ (monotime > 0);
}

/*----------------------------------------------------------------------------*/

MDBX_INTERNAL void osal_ctor(void);
MDBX_INTERNAL void osal_dtor(void);

#if defined(_WIN32) || defined(_WIN64)
MDBX_INTERNAL int osal_mb2w(const char *const src, wchar_t **const pdst);
#endif /* Windows */

typedef union bin128 {
  __anonymous_struct_extension__ struct {
    uint64_t x, y;
  };
  __anonymous_struct_extension__ struct {
    uint32_t a, b, c, d;
  };
} bin128_t;

MDBX_INTERNAL bin128_t osal_guid(const MDBX_env *);

/*----------------------------------------------------------------------------*/

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline uint64_t osal_bswap64(uint64_t v) {
#if __GNUC_PREREQ(4, 4) || __CLANG_PREREQ(4, 0) || __has_builtin(__builtin_bswap64)
  return __builtin_bswap64(v);
#elif defined(_MSC_VER) && !defined(__clang__)
  return _byteswap_uint64(v);
#elif defined(__bswap_64)
  return __bswap_64(v);
#elif defined(bswap_64)
  return bswap_64(v);
#else
  return v << 56 | v >> 56 | ((v << 40) & UINT64_C(0x00ff000000000000)) | ((v << 24) & UINT64_C(0x0000ff0000000000)) |
         ((v << 8) & UINT64_C(0x000000ff00000000)) | ((v >> 8) & UINT64_C(0x00000000ff000000)) |
         ((v >> 24) & UINT64_C(0x0000000000ff0000)) | ((v >> 40) & UINT64_C(0x000000000000ff00));
#endif
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline uint32_t osal_bswap32(uint32_t v) {
#if __GNUC_PREREQ(4, 4) || __CLANG_PREREQ(4, 0) || __has_builtin(__builtin_bswap32)
  return __builtin_bswap32(v);
#elif defined(_MSC_VER) && !defined(__clang__)
  return _byteswap_ulong(v);
#elif defined(__bswap_32)
  return __bswap_32(v);
#elif defined(bswap_32)
  return bswap_32(v);
#else
  return v << 24 | v >> 24 | ((v << 8) & UINT32_C(0x00ff0000)) | ((v >> 8) & UINT32_C(0x0000ff00));
#endif
}

#if UINTPTR_MAX > 0xffffFFFFul || ULONG_MAX > 0xffffFFFFul || defined(_WIN64)
#define MDBX_WORDBITS 64
#else
#define MDBX_WORDBITS 32
#endif /* MDBX_WORDBITS */

/*******************************************************************************
 *******************************************************************************
 *
 * BUILD TIME
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

#ifndef MDBX_APPLE_SPEED_INSTEADOF_DURABILITY
/** Choices \ref MDBX_OSX_WANNA_DURABILITY or \ref MDBX_OSX_WANNA_SPEED
 * for OSX & iOS */
#define MDBX_APPLE_SPEED_INSTEADOF_DURABILITY MDBX_OSX_WANNA_DURABILITY
#endif /* MDBX_APPLE_SPEED_INSTEADOF_DURABILITY */

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
#if defined(__linux__) || defined(__gnu_linux__) || defined(__NetBSD__) || defined(__OpenBSD__)
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
#ifndef MDBX_USE_MINCORE
#if defined(MINCORE_INCORE) || !(defined(_WIN32) || defined(_WIN64))
#define MDBX_USE_MINCORE 1
#else
#define MDBX_USE_MINCORE 0
#endif
#define MDBX_USE_MINCORE_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_USE_MINCORE)
#elif !(MDBX_USE_MINCORE == 0 || MDBX_USE_MINCORE == 1)
#error MDBX_USE_MINCORE must be defined as 0 or 1
#endif /* MDBX_USE_MINCORE */

/** Enables chunking long list of retired pages during huge transactions commit
 * to avoid use sequences of pages. */
#ifndef MDBX_ENABLE_BIGFOOT
#define MDBX_ENABLE_BIGFOOT 1
#elif !(MDBX_ENABLE_BIGFOOT == 0 || MDBX_ENABLE_BIGFOOT == 1)
#error MDBX_ENABLE_BIGFOOT must be defined as 0 or 1
#endif /* MDBX_ENABLE_BIGFOOT */

/** Disable some checks to reduce an overhead and detection probability of
 * database corruption to a values closer to the LMDB. */
#ifndef MDBX_DISABLE_VALIDATION
#define MDBX_DISABLE_VALIDATION 0
#elif !(MDBX_DISABLE_VALIDATION == 0 || MDBX_DISABLE_VALIDATION == 1)
#error MDBX_DISABLE_VALIDATION must be defined as 0 or 1
#endif /* MDBX_DISABLE_VALIDATION */

#ifndef MDBX_PNL_PREALLOC_FOR_RADIXSORT
#define MDBX_PNL_PREALLOC_FOR_RADIXSORT 1
#elif !(MDBX_PNL_PREALLOC_FOR_RADIXSORT == 0 || MDBX_PNL_PREALLOC_FOR_RADIXSORT == 1)
#error MDBX_PNL_PREALLOC_FOR_RADIXSORT must be defined as 0 or 1
#endif /* MDBX_PNL_PREALLOC_FOR_RADIXSORT */

#ifndef MDBX_DPL_PREALLOC_FOR_RADIXSORT
#define MDBX_DPL_PREALLOC_FOR_RADIXSORT 1
#elif !(MDBX_DPL_PREALLOC_FOR_RADIXSORT == 0 || MDBX_DPL_PREALLOC_FOR_RADIXSORT == 1)
#error MDBX_DPL_PREALLOC_FOR_RADIXSORT must be defined as 0 or 1
#endif /* MDBX_DPL_PREALLOC_FOR_RADIXSORT */

/** Controls dirty pages tracking, spilling and persisting in `MDBX_WRITEMAP`
 * mode, i.e. disables in-memory database updating with consequent
 * flush-to-disk/msync syscall.
 *
 * 0/OFF = Don't track dirty pages at all, don't spill ones, and use msync() to
 * persist data. This is by-default on Linux and other systems where kernel
 * provides properly LRU tracking and effective flushing on-demand.
 *
 * 1/ON = Tracking of dirty pages but with LRU labels for spilling and explicit
 * persist ones by write(). This may be reasonable for goofy systems (Windows)
 * which low performance of msync() and/or zany LRU tracking. */
#ifndef MDBX_AVOID_MSYNC
#if defined(_WIN32) || defined(_WIN64)
#define MDBX_AVOID_MSYNC 1
#else
#define MDBX_AVOID_MSYNC 0
#endif
#elif !(MDBX_AVOID_MSYNC == 0 || MDBX_AVOID_MSYNC == 1)
#error MDBX_AVOID_MSYNC must be defined as 0 or 1
#endif /* MDBX_AVOID_MSYNC */

/** Управляет механизмом поддержки разреженных наборов DBI-хендлов для снижения
 * накладных расходов при запуске и обработке транзакций. */
#ifndef MDBX_ENABLE_DBI_SPARSE
#define MDBX_ENABLE_DBI_SPARSE 1
#elif !(MDBX_ENABLE_DBI_SPARSE == 0 || MDBX_ENABLE_DBI_SPARSE == 1)
#error MDBX_ENABLE_DBI_SPARSE must be defined as 0 or 1
#endif /* MDBX_ENABLE_DBI_SPARSE */

/** Управляет механизмом отложенного освобождения и поддержки пути быстрого
 * открытия DBI-хендлов без захвата блокировок. */
#ifndef MDBX_ENABLE_DBI_LOCKFREE
#define MDBX_ENABLE_DBI_LOCKFREE 1
#elif !(MDBX_ENABLE_DBI_LOCKFREE == 0 || MDBX_ENABLE_DBI_LOCKFREE == 1)
#error MDBX_ENABLE_DBI_LOCKFREE must be defined as 0 or 1
#endif /* MDBX_ENABLE_DBI_LOCKFREE */

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
#if defined(MDBX_BUILD_CXX) && !MDBX_BUILD_CXX
#define MDBX_WITHOUT_MSVC_CRT 1
#else
#define MDBX_WITHOUT_MSVC_CRT 0
#endif
#elif !(MDBX_WITHOUT_MSVC_CRT == 0 || MDBX_WITHOUT_MSVC_CRT == 1)
#error MDBX_WITHOUT_MSVC_CRT must be defined as 0 or 1
#endif /* MDBX_WITHOUT_MSVC_CRT */

/** Size of buffer used during copying a environment/database file. */
#ifndef MDBX_ENVCOPY_WRITEBUF
#define MDBX_ENVCOPY_WRITEBUF 1048576u
#elif MDBX_ENVCOPY_WRITEBUF < 65536u || MDBX_ENVCOPY_WRITEBUF > 1073741824u || MDBX_ENVCOPY_WRITEBUF % 65536u
#error MDBX_ENVCOPY_WRITEBUF must be defined in range 65536..1073741824 and be multiple of 65536
#endif /* MDBX_ENVCOPY_WRITEBUF */

/** Forces assertion checking. */
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
#elif MDBX_ASSUME_MALLOC_OVERHEAD < 0 || MDBX_ASSUME_MALLOC_OVERHEAD > 64 || MDBX_ASSUME_MALLOC_OVERHEAD % 4
#error MDBX_ASSUME_MALLOC_OVERHEAD must be defined in range 0..64 and be multiple of 4
#endif /* MDBX_ASSUME_MALLOC_OVERHEAD */

/** If defined then enables integration with Valgrind,
 * a memory analyzing tool. */
#ifndef ENABLE_MEMCHECK
#endif /* ENABLE_MEMCHECK */

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
#elif __has_builtin(__builtin_cpu_supports) || defined(__BUILTIN_CPU_SUPPORTS__) ||                                    \
    (defined(__ia32__) && __GNUC_PREREQ(4, 8) && __GLIBC_PREREQ(2, 23))
#define MDBX_HAVE_BUILTIN_CPU_SUPPORTS 1
#else
#define MDBX_HAVE_BUILTIN_CPU_SUPPORTS 0
#endif
#elif !(MDBX_HAVE_BUILTIN_CPU_SUPPORTS == 0 || MDBX_HAVE_BUILTIN_CPU_SUPPORTS == 1)
#error MDBX_HAVE_BUILTIN_CPU_SUPPORTS must be defined as 0 or 1
#endif /* MDBX_HAVE_BUILTIN_CPU_SUPPORTS */

/** if enabled then instead of the returned error `MDBX_REMOTE`, only a warning is issued, when
 * the database being opened in non-read-only mode is located in a file system exported via NFS. */
#ifndef MDBX_ENABLE_NON_READONLY_EXPORT
#define MDBX_ENABLE_NON_READONLY_EXPORT 0
#elif !(MDBX_ENABLE_NON_READONLY_EXPORT == 0 || MDBX_ENABLE_NON_READONLY_EXPORT == 1)
#error MDBX_ENABLE_NON_READONLY_EXPORT must be defined as 0 or 1
#endif /* MDBX_ENABLE_NON_READONLY_EXPORT */

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

/** Advanced: Choices the locking implementation (autodetection by default). */
#if defined(_WIN32) || defined(_WIN64)
#define MDBX_LOCKING MDBX_LOCKING_WIN32FILES
#else
#ifndef MDBX_LOCKING
#if defined(_POSIX_THREAD_PROCESS_SHARED) && _POSIX_THREAD_PROCESS_SHARED >= 200112L && !defined(__FreeBSD__)

/* Some platforms define the EOWNERDEAD error code even though they
 * don't support Robust Mutexes. If doubt compile with -MDBX_LOCKING=2001. */
#if defined(EOWNERDEAD) && _POSIX_THREAD_PROCESS_SHARED >= 200809L &&                                                  \
    ((defined(_POSIX_THREAD_ROBUST_PRIO_INHERIT) && _POSIX_THREAD_ROBUST_PRIO_INHERIT > 0) ||                          \
     (defined(_POSIX_THREAD_ROBUST_PRIO_PROTECT) && _POSIX_THREAD_ROBUST_PRIO_PROTECT > 0) ||                          \
     defined(PTHREAD_MUTEX_ROBUST) || defined(PTHREAD_MUTEX_ROBUST_NP)) &&                                             \
    (!defined(__GLIBC__) || __GLIBC_PREREQ(2, 10) /* troubles with Robust mutexes before 2.10 */)
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
#if ((defined(F_OFD_SETLK) && defined(F_OFD_SETLKW) && defined(F_OFD_GETLK)) ||                                        \
     (defined(F_OFD_SETLK64) && defined(F_OFD_SETLKW64) && defined(F_OFD_GETLK64))) &&                                 \
    !defined(MDBX_SAFE4QEMU) && !defined(__sun) /* OFD-lock are broken on Solaris */
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
#if ((defined(__linux__) || defined(__gnu_linux__)) && !defined(__ANDROID_API__)) ||                                   \
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

//------------------------------------------------------------------------------

#ifndef MDBX_CPU_WRITEBACK_INCOHERENT
#if defined(__ia32__) || defined(__e2k__) || defined(__hppa) || defined(__hppa__) || defined(DOXYGEN)
#define MDBX_CPU_WRITEBACK_INCOHERENT 0
#else
#define MDBX_CPU_WRITEBACK_INCOHERENT 1
#endif
#elif !(MDBX_CPU_WRITEBACK_INCOHERENT == 0 || MDBX_CPU_WRITEBACK_INCOHERENT == 1)
#error MDBX_CPU_WRITEBACK_INCOHERENT must be defined as 0 or 1
#endif /* MDBX_CPU_WRITEBACK_INCOHERENT */

#ifndef MDBX_MMAP_INCOHERENT_FILE_WRITE
#ifdef __OpenBSD__
#define MDBX_MMAP_INCOHERENT_FILE_WRITE 1
#else
#define MDBX_MMAP_INCOHERENT_FILE_WRITE 0
#endif
#elif !(MDBX_MMAP_INCOHERENT_FILE_WRITE == 0 || MDBX_MMAP_INCOHERENT_FILE_WRITE == 1)
#error MDBX_MMAP_INCOHERENT_FILE_WRITE must be defined as 0 or 1
#endif /* MDBX_MMAP_INCOHERENT_FILE_WRITE */

#ifndef MDBX_MMAP_INCOHERENT_CPU_CACHE
#if defined(__mips) || defined(__mips__) || defined(__mips64) || defined(__mips64__) || defined(_M_MRX000) ||          \
    defined(_MIPS_) || defined(__MWERKS__) || defined(__sgi)
/* MIPS has cache coherency issues. */
#define MDBX_MMAP_INCOHERENT_CPU_CACHE 1
#else
/* LY: assume no relevant mmap/dcache issues. */
#define MDBX_MMAP_INCOHERENT_CPU_CACHE 0
#endif
#elif !(MDBX_MMAP_INCOHERENT_CPU_CACHE == 0 || MDBX_MMAP_INCOHERENT_CPU_CACHE == 1)
#error MDBX_MMAP_INCOHERENT_CPU_CACHE must be defined as 0 or 1
#endif /* MDBX_MMAP_INCOHERENT_CPU_CACHE */

/** Assume system needs explicit syscall to sync/flush/write modified mapped
 * memory. */
#ifndef MDBX_MMAP_NEEDS_JOLT
#if MDBX_MMAP_INCOHERENT_FILE_WRITE || MDBX_MMAP_INCOHERENT_CPU_CACHE || !(defined(__linux__) || defined(__gnu_linux__))
#define MDBX_MMAP_NEEDS_JOLT 1
#else
#define MDBX_MMAP_NEEDS_JOLT 0
#endif
#define MDBX_MMAP_NEEDS_JOLT_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_MMAP_NEEDS_JOLT)
#elif !(MDBX_MMAP_NEEDS_JOLT == 0 || MDBX_MMAP_NEEDS_JOLT == 1)
#error MDBX_MMAP_NEEDS_JOLT must be defined as 0 or 1
#endif /* MDBX_MMAP_NEEDS_JOLT */

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
#if defined(__ALIGNED__) || defined(__SANITIZE_UNDEFINED__) || defined(ENABLE_UBSAN)
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

/* Max length of iov-vector passed to writev() call, used for auxilary writes */
#ifndef MDBX_AUXILARY_IOV_MAX
#define MDBX_AUXILARY_IOV_MAX 64
#endif
#if defined(IOV_MAX) && IOV_MAX < MDBX_AUXILARY_IOV_MAX
#undef MDBX_AUXILARY_IOV_MAX
#define MDBX_AUXILARY_IOV_MAX IOV_MAX
#endif /* MDBX_AUXILARY_IOV_MAX */

/* An extra/custom information provided during library build */
#ifndef MDBX_BUILD_METADATA
#define MDBX_BUILD_METADATA ""
#endif /* MDBX_BUILD_METADATA */
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
#endif
#if MDBX_DEBUG < 0 || MDBX_DEBUG > 2
#error "The MDBX_DEBUG must be defined to 0, 1 or 2"
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
#define MDBX_DEBUG 0...2

/** Disables using of GNU libc extensions. */
#define MDBX_DISABLE_GNU_SOURCE 0 or 1

#endif /* DOXYGEN */

#ifndef MDBX_64BIT_ATOMIC
#error "The MDBX_64BIT_ATOMIC must be defined before"
#endif /* MDBX_64BIT_ATOMIC */

#ifndef MDBX_64BIT_CAS
#error "The MDBX_64BIT_CAS must be defined before"
#endif /* MDBX_64BIT_CAS */

#if defined(__cplusplus) && !defined(__STDC_NO_ATOMICS__) && __has_include(<cstdatomic>)
#include <cstdatomic>
#define MDBX_HAVE_C11ATOMICS
#elif !defined(__cplusplus) && (__STDC_VERSION__ >= 201112L || __has_extension(c_atomic)) &&                           \
    !defined(__STDC_NO_ATOMICS__) &&                                                                                   \
    (__GNUC_PREREQ(4, 9) || __CLANG_PREREQ(3, 8) || !(defined(__GNUC__) || defined(__clang__)))
#include <stdatomic.h>
#define MDBX_HAVE_C11ATOMICS
#elif defined(__GNUC__) || defined(__clang__)
#elif defined(_MSC_VER)
#pragma warning(disable : 4163) /* 'xyz': not available as an intrinsic */
#pragma warning(disable : 4133) /* 'function': incompatible types - from                                               \
                                   'size_t' to 'LONGLONG' */
#pragma warning(disable : 4244) /* 'return': conversion from 'LONGLONG' to                                             \
                                   'std::size_t', possible loss of data */
#pragma warning(disable : 4267) /* 'function': conversion from 'size_t' to                                             \
                                   'long', possible loss of data */
#pragma intrinsic(_InterlockedExchangeAdd, _InterlockedCompareExchange)
#pragma intrinsic(_InterlockedExchangeAdd64, _InterlockedCompareExchange64)
#elif defined(__APPLE__)
#include <libkern/OSAtomic.h>
#else
#error FIXME atomic-ops
#endif

typedef enum mdbx_memory_order {
  mo_Relaxed,
  mo_AcquireRelease
  /* , mo_SequentialConsistency */
} mdbx_memory_order_t;

typedef union {
  volatile uint32_t weak;
#ifdef MDBX_HAVE_C11ATOMICS
  volatile _Atomic uint32_t c11a;
#endif /* MDBX_HAVE_C11ATOMICS */
} mdbx_atomic_uint32_t;

typedef union {
  volatile uint64_t weak;
#if defined(MDBX_HAVE_C11ATOMICS) && (MDBX_64BIT_CAS || MDBX_64BIT_ATOMIC)
  volatile _Atomic uint64_t c11a;
#endif
#if !defined(MDBX_HAVE_C11ATOMICS) || !MDBX_64BIT_CAS || !MDBX_64BIT_ATOMIC
  __anonymous_struct_extension__ struct {
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
    mdbx_atomic_uint32_t low, high;
#elif __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
    mdbx_atomic_uint32_t high, low;
#else
#error "FIXME: Unsupported byte order"
#endif /* __BYTE_ORDER__ */
  };
#endif
} mdbx_atomic_uint64_t;

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

#define mo_c11_store(fence)                                                                                            \
  (((fence) == mo_Relaxed)          ? memory_order_relaxed                                                             \
   : ((fence) == mo_AcquireRelease) ? memory_order_release                                                             \
                                    : memory_order_seq_cst)
#define mo_c11_load(fence)                                                                                             \
  (((fence) == mo_Relaxed)          ? memory_order_relaxed                                                             \
   : ((fence) == mo_AcquireRelease) ? memory_order_acquire                                                             \
                                    : memory_order_seq_cst)

#endif /* MDBX_HAVE_C11ATOMICS */

#define SAFE64_INVALID_THRESHOLD UINT64_C(0xffffFFFF00000000)

#pragma pack(push, 4)

/* A stamp that identifies a file as an MDBX file.
 * There's nothing special about this value other than that it is easily
 * recognizable, and it will reflect any byte order mismatches. */
#define MDBX_MAGIC UINT64_C(/* 56-bit prime */ 0x59659DBDEF4C11)

/* FROZEN: The version number for a database's datafile format. */
#define MDBX_DATA_VERSION 3

#define MDBX_DATA_MAGIC ((MDBX_MAGIC << 8) + MDBX_PNL_ASCENDING * 64 + MDBX_DATA_VERSION)
#define MDBX_DATA_MAGIC_LEGACY_COMPAT ((MDBX_MAGIC << 8) + MDBX_PNL_ASCENDING * 64 + 2)
#define MDBX_DATA_MAGIC_LEGACY_DEVEL ((MDBX_MAGIC << 8) + 255)

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
typedef mdbx_atomic_uint32_t atomic_pgno_t;
#define PRIaPGNO PRIu32
#define MAX_PAGENO UINT32_C(0x7FFFffff)
#define MIN_PAGENO NUM_METAS

/* An invalid page number.
 * Mainly used to denote an empty tree. */
#define P_INVALID (~(pgno_t)0)

/* A transaction ID. */
typedef uint64_t txnid_t;
typedef mdbx_atomic_uint64_t atomic_txnid_t;
#define PRIaTXN PRIi64
#define MIN_TXNID UINT64_C(1)
#define MAX_TXNID (SAFE64_INVALID_THRESHOLD - 1)
#define INITIAL_TXNID (MIN_TXNID + NUM_METAS - 1)
#define INVALID_TXNID UINT64_MAX

/* Used for offsets within a single page. */
typedef uint16_t indx_t;

typedef struct tree {
  uint16_t flags;       /* see mdbx_dbi_open */
  uint16_t height;      /* height of this tree */
  uint32_t dupfix_size; /* key-size for MDBX_DUPFIXED (DUPFIX pages) */
  pgno_t root;          /* the root page of this tree */
  pgno_t branch_pages;  /* number of branch pages */
  pgno_t leaf_pages;    /* number of leaf pages */
  pgno_t large_pages;   /* number of large pages */
  uint64_t sequence;    /* table sequence counter */
  uint64_t items;       /* number of data items */
  uint64_t mod_txnid;   /* txnid of last committed modification */
} tree_t;

/* database size-related parameters */
typedef struct geo {
  uint16_t grow_pv;   /* datafile growth step as a 16-bit packed (exponential
                           quantized) value */
  uint16_t shrink_pv; /* datafile shrink threshold as a 16-bit packed
                           (exponential quantized) value */
  pgno_t lower;       /* minimal size of datafile in pages */
  pgno_t upper;       /* maximal size of datafile in pages */
  union {
    pgno_t now; /* current size of datafile in pages */
    pgno_t end_pgno;
  };
  union {
    pgno_t first_unallocated; /* first unused page in the datafile,
                         but actually the file may be shorter. */
    pgno_t next_pgno;
  };
} geo_t;

/* Meta page content.
 * A meta page is the start point for accessing a database snapshot.
 * Pages 0-2 are meta pages. */
typedef struct meta {
  /* Stamp identifying this as an MDBX file.
   * It must be set to MDBX_MAGIC with MDBX_DATA_VERSION. */
  uint32_t magic_and_version[2];

  /* txnid that committed this meta, the first of a two-phase-update pair */
  union {
    mdbx_atomic_uint32_t txnid_a[2];
    uint64_t unsafe_txnid;
  };

  uint16_t reserve16;   /* extra flags, zero (nothing) for now */
  uint8_t validator_id; /* ID of checksum and page validation method,
                         * zero (nothing) for now */
  int8_t extra_pagehdr; /* extra bytes in the page header,
                         * zero (nothing) for now */

  geo_t geometry; /* database size-related parameters */

  union {
    struct {
      tree_t gc, main;
    } trees;
    __anonymous_struct_extension__ struct {
      uint16_t gc_flags;
      uint16_t gc_height;
      uint32_t pagesize;
    };
  };

  MDBX_canary canary;

#define DATASIGN_NONE 0u
#define DATASIGN_WEAK 1u
#define SIGN_IS_STEADY(sign) ((sign) > DATASIGN_WEAK)
  union {
    uint32_t sign[2];
    uint64_t unsafe_sign;
  };

  /* txnid that committed this meta, the second of a two-phase-update pair */
  mdbx_atomic_uint32_t txnid_b[2];

  /* Number of non-meta pages which were put in GC after COW. May be 0 in case
   * DB was previously handled by libmdbx without corresponding feature.
   * This value in couple with reader.snapshot_pages_retired allows fast
   * estimation of "how much reader is restraining GC recycling". */
  uint32_t pages_retired[2];

  /* The analogue /proc/sys/kernel/random/boot_id or similar to determine
   * whether the system was rebooted after the last use of the database files.
   * If there was no reboot, but there is no need to rollback to the last
   * steady sync point. Zeros mean that no relevant information is available
   * from the system. */
  bin128_t bootid;

  /* GUID базы данных, начиная с v0.13.1 */
  bin128_t dxbid;
} meta_t;

#pragma pack(1)

typedef enum page_type {
  P_BRANCH = 0x01u /* branch page */,
  P_LEAF = 0x02u /* leaf page */,
  P_LARGE = 0x04u /* large/overflow page */,
  P_META = 0x08u /* meta page */,
  P_LEGACY_DIRTY = 0x10u /* legacy P_DIRTY flag prior to v0.10 958fd5b9 */,
  P_BAD = P_LEGACY_DIRTY /* explicit flag for invalid/bad page */,
  P_DUPFIX = 0x20u /* for MDBX_DUPFIXED records */,
  P_SUBP = 0x40u /* for MDBX_DUPSORT sub-pages */,
  P_SPILLED = 0x2000u /* spilled in parent txn */,
  P_LOOSE = 0x4000u /* page was dirtied then freed, can be reused */,
  P_FROZEN = 0x8000u /* used for retire page with known status */,
  P_ILL_BITS = (uint16_t)~(P_BRANCH | P_LEAF | P_DUPFIX | P_LARGE | P_SPILLED),

  page_broken = 0,
  page_large = P_LARGE,
  page_branch = P_BRANCH,
  page_leaf = P_LEAF,
  page_dupfix_leaf = P_DUPFIX,
  page_sub_leaf = P_SUBP | P_LEAF,
  page_sub_dupfix_leaf = P_SUBP | P_DUPFIX,
  page_sub_broken = P_SUBP,
} page_type_t;

/* Common header for all page types. The page type depends on flags.
 *
 * P_BRANCH and P_LEAF pages have unsorted 'node_t's at the end, with
 * sorted entries[] entries referring to them. Exception: P_DUPFIX pages
 * omit entries and pack sorted MDBX_DUPFIXED values after the page header.
 *
 * P_LARGE records occupy one or more contiguous pages where only the
 * first has a page header. They hold the real data of N_BIG nodes.
 *
 * P_SUBP sub-pages are small leaf "pages" with duplicate data.
 * A node with flag N_DUP but not N_TREE contains a sub-page.
 * (Duplicate data can also go in tables, which use normal pages.)
 *
 * P_META pages contain meta_t, the start point of an MDBX snapshot.
 *
 * Each non-metapage up to meta_t.mm_last_pg is reachable exactly once
 * in the snapshot: Either used by a database or listed in a GC record. */
typedef struct page {
  uint64_t txnid;        /* txnid which created page, maybe zero in legacy DB */
  uint16_t dupfix_ksize; /* key size if this is a DUPFIX page */
  uint16_t flags;
  union {
    uint32_t pages; /* number of overflow pages */
    __anonymous_struct_extension__ struct {
      indx_t lower; /* lower bound of free space */
      indx_t upper; /* upper bound of free space */
    };
  };
  pgno_t pgno; /* page number */

#if FLEXIBLE_ARRAY_MEMBERS
  indx_t entries[] /* dynamic size */;
#endif /* FLEXIBLE_ARRAY_MEMBERS */
} page_t;

/* Size of the page header, excluding dynamic data at the end */
#define PAGEHDRSZ 20u

/* Header for a single key/data pair within a page.
 * Used in pages of type P_BRANCH and P_LEAF without P_DUPFIX.
 * We guarantee 2-byte alignment for 'node_t's.
 *
 * Leaf node flags describe node contents.  N_BIG says the node's
 * data part is the page number of an overflow page with actual data.
 * N_DUP and N_TREE can be combined giving duplicate data in
 * a sub-page/table, and named databases (just N_TREE). */
typedef struct node {
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
  union {
    uint32_t dsize;
    uint32_t child_pgno;
  };
  uint8_t flags; /* see node_flags */
  uint8_t extra;
  uint16_t ksize; /* key size */
#else
  uint16_t ksize; /* key size */
  uint8_t extra;
  uint8_t flags; /* see node_flags */
  union {
    uint32_t child_pgno;
    uint32_t dsize;
  };
#endif /* __BYTE_ORDER__ */

#if FLEXIBLE_ARRAY_MEMBERS
  uint8_t payload[] /* key and data are appended here */;
#endif /* FLEXIBLE_ARRAY_MEMBERS */
} node_t;

/* Size of the node header, excluding dynamic data at the end */
#define NODESIZE 8u

typedef enum node_flags {
  N_BIG = 0x01 /* data put on large page */,
  N_TREE = 0x02 /* data is a b-tree */,
  N_DUP = 0x04 /* data has duplicates */
} node_flags_t;

#pragma pack(pop)

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline uint8_t page_type(const page_t *mp) { return mp->flags; }

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline uint8_t page_type_compat(const page_t *mp) {
  /* Drop legacy P_DIRTY flag for sub-pages for compatilibity,
   * for assertions only. */
  return unlikely(mp->flags & P_SUBP) ? mp->flags & ~(P_SUBP | P_LEGACY_DIRTY) : mp->flags;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_leaf(const page_t *mp) {
  return (mp->flags & P_LEAF) != 0;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_dupfix_leaf(const page_t *mp) {
  return (mp->flags & P_DUPFIX) != 0;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_branch(const page_t *mp) {
  return (mp->flags & P_BRANCH) != 0;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_largepage(const page_t *mp) {
  return (mp->flags & P_LARGE) != 0;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_subpage(const page_t *mp) {
  return (mp->flags & P_SUBP) != 0;
}

/* The version number for a database's lockfile format. */
#define MDBX_LOCK_VERSION 6

#if MDBX_LOCKING == MDBX_LOCKING_WIN32FILES

#define MDBX_LCK_SIGN UINT32_C(0xF10C)
typedef void osal_ipclock_t;
#elif MDBX_LOCKING == MDBX_LOCKING_SYSV

#define MDBX_LCK_SIGN UINT32_C(0xF18D)
typedef mdbx_pid_t osal_ipclock_t;

#elif MDBX_LOCKING == MDBX_LOCKING_POSIX2001 || MDBX_LOCKING == MDBX_LOCKING_POSIX2008

#define MDBX_LCK_SIGN UINT32_C(0x8017)
typedef pthread_mutex_t osal_ipclock_t;

#elif MDBX_LOCKING == MDBX_LOCKING_POSIX1988

#define MDBX_LCK_SIGN UINT32_C(0xFC29)
typedef sem_t osal_ipclock_t;

#else
#error "FIXME"
#endif /* MDBX_LOCKING */

/* Статистика профилирования работы GC */
typedef struct gc_prof_stat {
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
  /* Для разборок с pnl_merge() */
  struct {
    uint64_t time;
    uint64_t volume;
    uint32_t calls;
  } pnl_merge;
} gc_prof_stat_t;

/* Statistics of pages operations for all transactions,
 * including incomplete and aborted. */
typedef struct pgops {
  mdbx_atomic_uint64_t newly;   /* Quantity of a new pages added */
  mdbx_atomic_uint64_t cow;     /* Quantity of pages copied for update */
  mdbx_atomic_uint64_t clone;   /* Quantity of parent's dirty pages clones
                                   for nested transactions */
  mdbx_atomic_uint64_t split;   /* Page splits */
  mdbx_atomic_uint64_t merge;   /* Page merges */
  mdbx_atomic_uint64_t spill;   /* Quantity of spilled dirty pages */
  mdbx_atomic_uint64_t unspill; /* Quantity of unspilled/reloaded pages */
  mdbx_atomic_uint64_t wops;    /* Number of explicit write operations (not a pages) to a disk */
  mdbx_atomic_uint64_t msync;   /* Number of explicit msync/flush-to-disk operations */
  mdbx_atomic_uint64_t fsync;   /* Number of explicit fsync/flush-to-disk operations */

  mdbx_atomic_uint64_t prefault; /* Number of prefault write operations */
  mdbx_atomic_uint64_t mincore;  /* Number of mincore() calls */

  mdbx_atomic_uint32_t incoherence; /* number of https://libmdbx.dqdkfa.ru/dead-github/issues/269
                                       caught */
  mdbx_atomic_uint32_t reserved;

  /* Статистика для профилирования GC.
   * Логически эти данные, возможно, стоит вынести в другую структуру,
   * но разница будет сугубо косметическая. */
  struct {
    /* Затраты на поддержку данных пользователя */
    gc_prof_stat_t work;
    /* Затраты на поддержку и обновления самой GC */
    gc_prof_stat_t self;
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

/* Reader Lock Table
 *
 * Readers don't acquire any locks for their data access. Instead, they
 * simply record their transaction ID in the reader table. The reader
 * mutex is needed just to find an empty slot in the reader table. The
 * slot's address is saved in thread-specific data so that subsequent
 * read transactions started by the same thread need no further locking to
 * proceed.
 *
 * If MDBX_NOSTICKYTHREADS is set, the slot address is not saved in
 * thread-specific data. No reader table is used if the database is on a
 * read-only filesystem.
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
typedef struct reader_slot {
  /* Current Transaction ID when this transaction began, or INVALID_TXNID.
   * Multiple readers that start at the same time will probably have the
   * same ID here. Again, it's not important to exclude them from
   * anything; all we need to know is which version of the DB they
   * started from so we can avoid overwriting any data used in that
   * particular version. */
  atomic_txnid_t txnid;

  /* The information we store in a single slot of the reader table.
   * In addition to a transaction ID, we also record the process and
   * thread ID that owns a slot, so that we can detect stale information,
   * e.g. threads or processes that went away without cleaning up.
   *
   * NOTE: We currently don't check for stale records.
   * We simply re-init the table when we know that we're the only process
   * opening the lock file. */

  /* Псевдо thread_id для пометки вытесненных читающих транзакций. */
#define MDBX_TID_TXN_OUSTED (UINT64_MAX - 1)

  /* Псевдо thread_id для пометки припаркованных читающих транзакций. */
#define MDBX_TID_TXN_PARKED UINT64_MAX

  /* The thread ID of the thread owning this txn. */
  mdbx_atomic_uint64_t tid;

  /* The process ID of the process owning this reader txn. */
  mdbx_atomic_uint32_t pid;

  /* The number of pages used in the reader's MVCC snapshot,
   * i.e. the value of meta->geometry.first_unallocated and
   * txn->geo.first_unallocated */
  atomic_pgno_t snapshot_pages_used;
  /* Number of retired pages at the time this reader starts transaction. So,
   * at any time the difference meta.pages_retired -
   * reader.snapshot_pages_retired will give the number of pages which this
   * reader restraining from reuse. */
  mdbx_atomic_uint64_t snapshot_pages_retired;
} reader_slot_t;

/* The header for the reader table (a memory-mapped lock file). */
typedef struct shared_lck {
  /* Stamp identifying this as an MDBX file.
   * It must be set to MDBX_MAGIC with with MDBX_LOCK_VERSION. */
  uint64_t magic_and_version;

  /* Format of this lock file. Must be set to MDBX_LOCK_FORMAT. */
  uint32_t os_and_format;

  /* Flags which environment was opened. */
  mdbx_atomic_uint32_t envmode;

  /* Threshold of un-synced-with-disk pages for auto-sync feature,
   * zero means no-threshold, i.e. auto-sync is disabled. */
  atomic_pgno_t autosync_threshold;

  /* Low 32-bit of txnid with which meta-pages was synced,
   * i.e. for sync-polling in the MDBX_NOMETASYNC mode. */
#define MDBX_NOMETASYNC_LAZY_UNK (UINT32_MAX / 3)
#define MDBX_NOMETASYNC_LAZY_FD (MDBX_NOMETASYNC_LAZY_UNK + UINT32_MAX / 8)
#define MDBX_NOMETASYNC_LAZY_WRITEMAP (MDBX_NOMETASYNC_LAZY_UNK - UINT32_MAX / 8)
  mdbx_atomic_uint32_t meta_sync_txnid;

  /* Period for timed auto-sync feature, i.e. at the every steady checkpoint
   * the mti_unsynced_timeout sets to the current_time + autosync_period.
   * The time value is represented in a suitable system-dependent form, for
   * example clock_gettime(CLOCK_BOOTTIME) or clock_gettime(CLOCK_MONOTONIC).
   * Zero means timed auto-sync is disabled. */
  mdbx_atomic_uint64_t autosync_period;

  /* Marker to distinguish uniqueness of DB/CLK. */
  mdbx_atomic_uint64_t bait_uniqueness;

  /* Paired counter of processes that have mlock()ed part of mmapped DB.
   * The (mlcnt[0] - mlcnt[1]) > 0 means at least one process
   * lock at least one page, so therefore madvise() could return EINVAL. */
  mdbx_atomic_uint32_t mlcnt[2];

  MDBX_ALIGNAS(MDBX_CACHELINE_SIZE) /* cacheline ----------------------------*/

  /* Statistics of costly ops of all (running, completed and aborted)
   * transactions */
  pgop_stat_t pgops;

  MDBX_ALIGNAS(MDBX_CACHELINE_SIZE) /* cacheline ----------------------------*/

#if MDBX_LOCKING > 0
  /* Write transaction lock. */
  osal_ipclock_t wrt_lock;
#endif /* MDBX_LOCKING > 0 */

  atomic_txnid_t cached_oldest;

  /* Timestamp of entering an out-of-sync state. Value is represented in a
   * suitable system-dependent form, for example clock_gettime(CLOCK_BOOTTIME)
   * or clock_gettime(CLOCK_MONOTONIC). */
  mdbx_atomic_uint64_t eoos_timestamp;

  /* Number un-synced-with-disk pages for auto-sync feature. */
  mdbx_atomic_uint64_t unsynced_pages;

  /* Timestamp of the last readers check. */
  mdbx_atomic_uint64_t readers_check_timestamp;

  /* Number of page which was discarded last time by madvise(DONTNEED). */
  atomic_pgno_t discarded_tail;

  /* Shared anchor for tracking readahead edge and enabled/disabled status. */
  pgno_t readahead_anchor;

  /* Shared cache for mincore() results */
  struct {
    pgno_t begin[4];
    uint64_t mask[4];
  } mincore_cache;

  MDBX_ALIGNAS(MDBX_CACHELINE_SIZE) /* cacheline ----------------------------*/

#if MDBX_LOCKING > 0
  /* Readeaders table lock. */
  osal_ipclock_t rdt_lock;
#endif /* MDBX_LOCKING > 0 */

  /* The number of slots that have been used in the reader table.
   * This always records the maximum count, it is not decremented
   * when readers release their slots. */
  mdbx_atomic_uint32_t rdt_length;
  mdbx_atomic_uint32_t rdt_refresh_flag;

#if FLEXIBLE_ARRAY_MEMBERS
  MDBX_ALIGNAS(MDBX_CACHELINE_SIZE) /* cacheline ----------------------------*/
  reader_slot_t rdt[] /* dynamic size */;

/* Lockfile format signature: version, features and field layout */
#define MDBX_LOCK_FORMAT                                                                                               \
  (MDBX_LCK_SIGN * 27733 + (unsigned)sizeof(reader_slot_t) * 13 +                                                      \
   (unsigned)offsetof(reader_slot_t, snapshot_pages_used) * 251 + (unsigned)offsetof(lck_t, cached_oldest) * 83 +      \
   (unsigned)offsetof(lck_t, rdt_length) * 37 + (unsigned)offsetof(lck_t, rdt) * 29)
#endif /* FLEXIBLE_ARRAY_MEMBERS */
} lck_t;

#define MDBX_LOCK_MAGIC ((MDBX_MAGIC << 8) + MDBX_LOCK_VERSION)

#define MDBX_READERS_LIMIT 32767

#define MIN_MAPSIZE (MDBX_MIN_PAGESIZE * MIN_PAGENO)
#if defined(_WIN32) || defined(_WIN64)
#define MAX_MAPSIZE32 UINT32_C(0x38000000)
#else
#define MAX_MAPSIZE32 UINT32_C(0x7f000000)
#endif
#define MAX_MAPSIZE64 ((MAX_PAGENO + 1) * (uint64_t)MDBX_MAX_PAGESIZE)

#if MDBX_WORDBITS >= 64
#define MAX_MAPSIZE MAX_MAPSIZE64
#define PAGELIST_LIMIT ((size_t)MAX_PAGENO)
#else
#define MAX_MAPSIZE MAX_MAPSIZE32
#define PAGELIST_LIMIT (MAX_MAPSIZE32 / MDBX_MIN_PAGESIZE)
#endif /* MDBX_WORDBITS */

#define MDBX_GOLD_RATIO_DBL 1.6180339887498948482
#define MEGABYTE ((size_t)1 << 20)

/*----------------------------------------------------------------------------*/

union logger_union {
  void *ptr;
  MDBX_debug_func *fmt;
  MDBX_debug_func_nofmt *nofmt;
};

struct libmdbx_globals {
  bin128_t bootid;
  unsigned sys_pagesize, sys_allocation_granularity;
  uint8_t sys_pagesize_ln2;
  uint8_t runtime_flags;
  uint8_t loglevel;
#if defined(_WIN32) || defined(_WIN64)
  bool running_under_Wine;
#elif defined(__linux__) || defined(__gnu_linux__)
  bool running_on_WSL1 /* Windows Subsystem 1 for Linux */;
  uint32_t linux_kernel_version;
#endif /* Linux */
  union logger_union logger;
  osal_fastmutex_t debug_lock;
  size_t logger_buffer_size;
  char *logger_buffer;
};

#ifdef __cplusplus
extern "C" {
#endif /* __cplusplus */

extern struct libmdbx_globals globals;
#if defined(_WIN32) || defined(_WIN64)
extern struct libmdbx_imports imports;
#endif /* Windows */

#ifndef __Wpedantic_format_voidptr
MDBX_MAYBE_UNUSED static inline const void *__Wpedantic_format_voidptr(const void *ptr) { return ptr; }
#define __Wpedantic_format_voidptr(ARG) __Wpedantic_format_voidptr(ARG)
#endif /* __Wpedantic_format_voidptr */

MDBX_INTERNAL void MDBX_PRINTF_ARGS(4, 5) debug_log(int level, const char *function, int line, const char *fmt, ...)
    MDBX_PRINTF_ARGS(4, 5);
MDBX_INTERNAL void debug_log_va(int level, const char *function, int line, const char *fmt, va_list args);

#if MDBX_DEBUG
#define LOG_ENABLED(LVL) unlikely(LVL <= globals.loglevel)
#define AUDIT_ENABLED() unlikely((globals.runtime_flags & (unsigned)MDBX_DBG_AUDIT))
#else /* MDBX_DEBUG */
#define LOG_ENABLED(LVL) (LVL < MDBX_LOG_VERBOSE && LVL <= globals.loglevel)
#define AUDIT_ENABLED() (0)
#endif /* LOG_ENABLED() & AUDIT_ENABLED() */

#if MDBX_FORCE_ASSERTIONS
#define ASSERT_ENABLED() (1)
#elif MDBX_DEBUG
#define ASSERT_ENABLED() likely((globals.runtime_flags & (unsigned)MDBX_DBG_ASSERT))
#else
#define ASSERT_ENABLED() (0)
#endif /* ASSERT_ENABLED() */

#define DEBUG_EXTRA(fmt, ...)                                                                                          \
  do {                                                                                                                 \
    if (LOG_ENABLED(MDBX_LOG_EXTRA))                                                                                   \
      debug_log(MDBX_LOG_EXTRA, __func__, __LINE__, fmt, __VA_ARGS__);                                                 \
  } while (0)

#define DEBUG_EXTRA_PRINT(fmt, ...)                                                                                    \
  do {                                                                                                                 \
    if (LOG_ENABLED(MDBX_LOG_EXTRA))                                                                                   \
      debug_log(MDBX_LOG_EXTRA, nullptr, 0, fmt, __VA_ARGS__);                                                         \
  } while (0)

#define TRACE(fmt, ...)                                                                                                \
  do {                                                                                                                 \
    if (LOG_ENABLED(MDBX_LOG_TRACE))                                                                                   \
      debug_log(MDBX_LOG_TRACE, __func__, __LINE__, fmt "\n", __VA_ARGS__);                                            \
  } while (0)

#define DEBUG(fmt, ...)                                                                                                \
  do {                                                                                                                 \
    if (LOG_ENABLED(MDBX_LOG_DEBUG))                                                                                   \
      debug_log(MDBX_LOG_DEBUG, __func__, __LINE__, fmt "\n", __VA_ARGS__);                                            \
  } while (0)

#define VERBOSE(fmt, ...)                                                                                              \
  do {                                                                                                                 \
    if (LOG_ENABLED(MDBX_LOG_VERBOSE))                                                                                 \
      debug_log(MDBX_LOG_VERBOSE, __func__, __LINE__, fmt "\n", __VA_ARGS__);                                          \
  } while (0)

#define NOTICE(fmt, ...)                                                                                               \
  do {                                                                                                                 \
    if (LOG_ENABLED(MDBX_LOG_NOTICE))                                                                                  \
      debug_log(MDBX_LOG_NOTICE, __func__, __LINE__, fmt "\n", __VA_ARGS__);                                           \
  } while (0)

#define WARNING(fmt, ...)                                                                                              \
  do {                                                                                                                 \
    if (LOG_ENABLED(MDBX_LOG_WARN))                                                                                    \
      debug_log(MDBX_LOG_WARN, __func__, __LINE__, fmt "\n", __VA_ARGS__);                                             \
  } while (0)

#undef ERROR /* wingdi.h                                                                                               \
  Yeah, morons from M$ put such definition to the public header. */

#define ERROR(fmt, ...)                                                                                                \
  do {                                                                                                                 \
    if (LOG_ENABLED(MDBX_LOG_ERROR))                                                                                   \
      debug_log(MDBX_LOG_ERROR, __func__, __LINE__, fmt "\n", __VA_ARGS__);                                            \
  } while (0)

#define FATAL(fmt, ...) debug_log(MDBX_LOG_FATAL, __func__, __LINE__, fmt "\n", __VA_ARGS__);

#if MDBX_DEBUG
#define ASSERT_FAIL(env, msg, func, line) mdbx_assert_fail(env, msg, func, line)
#else /* MDBX_DEBUG */
MDBX_NORETURN __cold void assert_fail(const char *msg, const char *func, unsigned line);
#define ASSERT_FAIL(env, msg, func, line)                                                                              \
  do {                                                                                                                 \
    (void)(env);                                                                                                       \
    assert_fail(msg, func, line);                                                                                      \
  } while (0)
#endif /* MDBX_DEBUG */

#define ENSURE_MSG(env, expr, msg)                                                                                     \
  do {                                                                                                                 \
    if (unlikely(!(expr)))                                                                                             \
      ASSERT_FAIL(env, msg, __func__, __LINE__);                                                                       \
  } while (0)

#define ENSURE(env, expr) ENSURE_MSG(env, expr, #expr)

/* assert(3) variant in environment context */
#define eASSERT(env, expr)                                                                                             \
  do {                                                                                                                 \
    if (ASSERT_ENABLED())                                                                                              \
      ENSURE(env, expr);                                                                                               \
  } while (0)

/* assert(3) variant in cursor context */
#define cASSERT(mc, expr) eASSERT((mc)->txn->env, expr)

/* assert(3) variant in transaction context */
#define tASSERT(txn, expr) eASSERT((txn)->env, expr)

#ifndef xMDBX_TOOLS /* Avoid using internal eASSERT() */
#undef assert
#define assert(expr) eASSERT(nullptr, expr)
#endif

MDBX_MAYBE_UNUSED static inline void jitter4testing(bool tiny) {
#if MDBX_DEBUG
  if (globals.runtime_flags & (unsigned)MDBX_DBG_JITTER)
    osal_jitter(tiny);
#else
  (void)tiny;
#endif
}

MDBX_MAYBE_UNUSED MDBX_INTERNAL void page_list(page_t *mp);

MDBX_INTERNAL const char *pagetype_caption(const uint8_t type, char buf4unknown[16]);
/* Key size which fits in a DKBUF (debug key buffer). */
#define DKBUF_MAX 127
#define DKBUF char dbg_kbuf[DKBUF_MAX * 4 + 2]
#define DKEY(x) mdbx_dump_val(x, dbg_kbuf, DKBUF_MAX * 2 + 1)
#define DVAL(x) mdbx_dump_val(x, dbg_kbuf + DKBUF_MAX * 2 + 1, DKBUF_MAX * 2 + 1)

#if MDBX_DEBUG
#define DKBUF_DEBUG DKBUF
#define DKEY_DEBUG(x) DKEY(x)
#define DVAL_DEBUG(x) DVAL(x)
#else
#define DKBUF_DEBUG ((void)(0))
#define DKEY_DEBUG(x) ("-")
#define DVAL_DEBUG(x) ("-")
#endif

MDBX_INTERNAL void log_error(const int err, const char *func, unsigned line);

MDBX_MAYBE_UNUSED static inline int log_if_error(const int err, const char *func, unsigned line) {
  if (unlikely(err != MDBX_SUCCESS))
    log_error(err, func, line);
  return err;
}

#define LOG_IFERR(err) log_if_error((err), __func__, __LINE__)

/* Test if the flags f are set in a flag word w. */
#define F_ISSET(w, f) (((w) & (f)) == (f))

/* Round n up to an even number. */
#define EVEN_CEIL(n) (((n) + 1UL) & -2L) /* sign-extending -2 to match n+1U */

/* Round n down to an even number. */
#define EVEN_FLOOR(n) ((n) & ~(size_t)1)

/*
 *                /
 *                | -1, a < b
 * CMP2INT(a,b) = <  0, a == b
 *                |  1, a > b
 *                \
 */
#define CMP2INT(a, b) (((a) != (b)) ? (((a) < (b)) ? -1 : 1) : 0)

/* Pointer displacement without casting to char* to avoid pointer-aliasing */
#define ptr_disp(ptr, disp) ((void *)(((intptr_t)(ptr)) + ((intptr_t)(disp))))

/* Pointer distance as signed number of bytes */
#define ptr_dist(more, less) (((intptr_t)(more)) - ((intptr_t)(less)))

#define MDBX_ASAN_POISON_MEMORY_REGION(addr, size)                                                                     \
  do {                                                                                                                 \
    TRACE("POISON_MEMORY_REGION(%p, %zu) at %u", (void *)(addr), (size_t)(size), __LINE__);                            \
    ASAN_POISON_MEMORY_REGION(addr, size);                                                                             \
  } while (0)

#define MDBX_ASAN_UNPOISON_MEMORY_REGION(addr, size)                                                                   \
  do {                                                                                                                 \
    TRACE("UNPOISON_MEMORY_REGION(%p, %zu) at %u", (void *)(addr), (size_t)(size), __LINE__);                          \
    ASAN_UNPOISON_MEMORY_REGION(addr, size);                                                                           \
  } while (0)

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED static inline size_t branchless_abs(intptr_t value) {
  assert(value > INT_MIN);
  const size_t expanded_sign = (size_t)(value >> (sizeof(value) * CHAR_BIT - 1));
  return ((size_t)value + expanded_sign) ^ expanded_sign;
}

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED static inline bool is_powerof2(size_t x) { return (x & (x - 1)) == 0; }

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED static inline size_t floor_powerof2(size_t value, size_t granularity) {
  assert(is_powerof2(granularity));
  return value & ~(granularity - 1);
}

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED static inline size_t ceil_powerof2(size_t value, size_t granularity) {
  return floor_powerof2(value + granularity - 1, granularity);
}

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED MDBX_INTERNAL unsigned log2n_powerof2(size_t value_uintptr);

MDBX_NOTHROW_CONST_FUNCTION MDBX_INTERNAL uint64_t rrxmrrxmsx_0(uint64_t v);

struct monotime_cache {
  uint64_t value;
  int expire_countdown;
};

MDBX_MAYBE_UNUSED static inline uint64_t monotime_since_cached(uint64_t begin_timestamp, struct monotime_cache *cache) {
  if (cache->expire_countdown)
    cache->expire_countdown -= 1;
  else {
    cache->value = osal_monotime();
    cache->expire_countdown = 42 / 3;
  }
  return cache->value - begin_timestamp;
}

/* An PNL is an Page Number List, a sorted array of IDs.
 *
 * The first element of the array is a counter for how many actual page-numbers
 * are in the list. By default PNLs are sorted in descending order, this allow
 * cut off a page with lowest pgno (at the tail) just truncating the list. The
 * sort order of PNLs is controlled by the MDBX_PNL_ASCENDING build option. */
typedef pgno_t *pnl_t;
typedef const pgno_t *const_pnl_t;

#if MDBX_PNL_ASCENDING
#define MDBX_PNL_ORDERED(first, last) ((first) < (last))
#define MDBX_PNL_DISORDERED(first, last) ((first) >= (last))
#else
#define MDBX_PNL_ORDERED(first, last) ((first) > (last))
#define MDBX_PNL_DISORDERED(first, last) ((first) <= (last))
#endif

#define MDBX_PNL_GRANULATE_LOG2 10
#define MDBX_PNL_GRANULATE (1 << MDBX_PNL_GRANULATE_LOG2)
#define MDBX_PNL_INITIAL (MDBX_PNL_GRANULATE - 2 - MDBX_ASSUME_MALLOC_OVERHEAD / sizeof(pgno_t))

#define MDBX_PNL_ALLOCLEN(pl) ((pl)[-1])
#define MDBX_PNL_GETSIZE(pl) ((size_t)((pl)[0]))
#define MDBX_PNL_SETSIZE(pl, size)                                                                                     \
  do {                                                                                                                 \
    const size_t __size = size;                                                                                        \
    assert(__size < INT_MAX);                                                                                          \
    (pl)[0] = (pgno_t)__size;                                                                                          \
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

MDBX_MAYBE_UNUSED static inline size_t pnl_size2bytes(size_t size) {
  assert(size > 0 && size <= PAGELIST_LIMIT);
#if MDBX_PNL_PREALLOC_FOR_RADIXSORT

  size += size;
#endif /* MDBX_PNL_PREALLOC_FOR_RADIXSORT */
  STATIC_ASSERT(MDBX_ASSUME_MALLOC_OVERHEAD +
                    (PAGELIST_LIMIT * (MDBX_PNL_PREALLOC_FOR_RADIXSORT + 1) + MDBX_PNL_GRANULATE + 3) * sizeof(pgno_t) <
                SIZE_MAX / 4 * 3);
  size_t bytes =
      ceil_powerof2(MDBX_ASSUME_MALLOC_OVERHEAD + sizeof(pgno_t) * (size + 3), MDBX_PNL_GRANULATE * sizeof(pgno_t)) -
      MDBX_ASSUME_MALLOC_OVERHEAD;
  return bytes;
}

MDBX_MAYBE_UNUSED static inline pgno_t pnl_bytes2size(const size_t bytes) {
  size_t size = bytes / sizeof(pgno_t);
  assert(size > 3 && size <= PAGELIST_LIMIT + /* alignment gap */ 65536);
  size -= 3;
#if MDBX_PNL_PREALLOC_FOR_RADIXSORT
  size >>= 1;
#endif /* MDBX_PNL_PREALLOC_FOR_RADIXSORT */
  return (pgno_t)size;
}

MDBX_INTERNAL pnl_t pnl_alloc(size_t size);

MDBX_INTERNAL void pnl_free(pnl_t pnl);

MDBX_INTERNAL int pnl_reserve(pnl_t __restrict *__restrict ppnl, const size_t wanna);

MDBX_MAYBE_UNUSED static inline int __must_check_result pnl_need(pnl_t __restrict *__restrict ppnl, size_t num) {
  assert(MDBX_PNL_GETSIZE(*ppnl) <= PAGELIST_LIMIT && MDBX_PNL_ALLOCLEN(*ppnl) >= MDBX_PNL_GETSIZE(*ppnl));
  assert(num <= PAGELIST_LIMIT);
  const size_t wanna = MDBX_PNL_GETSIZE(*ppnl) + num;
  return likely(MDBX_PNL_ALLOCLEN(*ppnl) >= wanna) ? MDBX_SUCCESS : pnl_reserve(ppnl, wanna);
}

MDBX_MAYBE_UNUSED static inline void pnl_append_prereserved(__restrict pnl_t pnl, pgno_t pgno) {
  assert(MDBX_PNL_GETSIZE(pnl) < MDBX_PNL_ALLOCLEN(pnl));
  if (AUDIT_ENABLED()) {
    for (size_t i = MDBX_PNL_GETSIZE(pnl); i > 0; --i)
      assert(pgno != pnl[i]);
  }
  *pnl += 1;
  MDBX_PNL_LAST(pnl) = pgno;
}

MDBX_INTERNAL void pnl_shrink(pnl_t __restrict *__restrict ppnl);

MDBX_INTERNAL int __must_check_result spill_append_span(__restrict pnl_t *ppnl, pgno_t pgno, size_t n);

MDBX_INTERNAL int __must_check_result pnl_append_span(__restrict pnl_t *ppnl, pgno_t pgno, size_t n);

MDBX_INTERNAL int __must_check_result pnl_insert_span(__restrict pnl_t *ppnl, pgno_t pgno, size_t n);

MDBX_INTERNAL size_t pnl_search_nochk(const pnl_t pnl, pgno_t pgno);

MDBX_INTERNAL void pnl_sort_nochk(pnl_t pnl);

MDBX_INTERNAL bool pnl_check(const const_pnl_t pnl, const size_t limit);

MDBX_MAYBE_UNUSED static inline bool pnl_check_allocated(const const_pnl_t pnl, const size_t limit) {
  return pnl == nullptr || (MDBX_PNL_ALLOCLEN(pnl) >= MDBX_PNL_GETSIZE(pnl) && pnl_check(pnl, limit));
}

MDBX_MAYBE_UNUSED static inline void pnl_sort(pnl_t pnl, size_t limit4check) {
  pnl_sort_nochk(pnl);
  assert(pnl_check(pnl, limit4check));
  (void)limit4check;
}

MDBX_MAYBE_UNUSED static inline size_t pnl_search(const pnl_t pnl, pgno_t pgno, size_t limit) {
  assert(pnl_check_allocated(pnl, limit));
  if (MDBX_HAVE_CMOV) {
    /* cmov-ускоренный бинарный поиск может читать (но не использовать) один
     * элемент за концом данных, этот элемент в пределах выделенного участка
     * памяти, но не инициализирован. */
    VALGRIND_MAKE_MEM_DEFINED(MDBX_PNL_END(pnl), sizeof(pgno_t));
  }
  assert(pgno < limit);
  (void)limit;
  size_t n = pnl_search_nochk(pnl, pgno);
  if (MDBX_HAVE_CMOV) {
    VALGRIND_MAKE_MEM_UNDEFINED(MDBX_PNL_END(pnl), sizeof(pgno_t));
  }
  return n;
}

MDBX_INTERNAL size_t pnl_merge(pnl_t dst, const pnl_t src);

#ifdef __cplusplus
}
#endif /* __cplusplus */

#define mdbx_sourcery_anchor XCONCAT(mdbx_sourcery_, MDBX_BUILD_SOURCERY)
#if defined(xMDBX_TOOLS)
extern LIBMDBX_API const char *const mdbx_sourcery_anchor;
#endif

#define MDBX_IS_ERROR(rc) ((rc) != MDBX_RESULT_TRUE && (rc) != MDBX_RESULT_FALSE)

/*----------------------------------------------------------------------------*/

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED static inline pgno_t int64pgno(int64_t i64) {
  if (likely(i64 >= (int64_t)MIN_PAGENO && i64 <= (int64_t)MAX_PAGENO + 1))
    return (pgno_t)i64;
  return (i64 < (int64_t)MIN_PAGENO) ? MIN_PAGENO : MAX_PAGENO;
}

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED static inline pgno_t pgno_add(size_t base, size_t augend) {
  assert(base <= MAX_PAGENO + 1 && augend < MAX_PAGENO);
  return int64pgno((int64_t)base + (int64_t)augend);
}

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED static inline pgno_t pgno_sub(size_t base, size_t subtrahend) {
  assert(base >= MIN_PAGENO && base <= MAX_PAGENO + 1 && subtrahend < MAX_PAGENO);
  return int64pgno((int64_t)base - (int64_t)subtrahend);
}

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
#pragma warning(disable : 4548) /* expression before comma has no effect;                                              \
                                   expected expression with side - effect */
#pragma warning(disable : 4530) /* C++ exception handler used, but unwind                                              \
                                 * semantics are not enabled. Specify /EHsc */
#pragma warning(disable : 4577) /* 'noexcept' used with no exception handling                                          \
                                 * mode specified; termination on exception is                                         \
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
      fprintf(stderr, "%s: %s -- %c\n", argv[0], "option requires an argument", c);
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

static void print_stat(MDBX_stat *ms) {
  printf("  Pagesize: %u\n", ms->ms_psize);
  printf("  Tree depth: %u\n", ms->ms_depth);
  printf("  Branch pages: %" PRIu64 "\n", ms->ms_branch_pages);
  printf("  Leaf pages: %" PRIu64 "\n", ms->ms_leaf_pages);
  printf("  Overflow pages: %" PRIu64 "\n", ms->ms_overflow_pages);
  printf("  Entries: %" PRIu64 "\n", ms->ms_entries);
}

static void usage(const char *prog) {
  fprintf(stderr,
          "usage: %s [-V] [-q] [-e] [-f[f[f]]] [-r[r]] [-a|-s table] dbpath\n"
          "  -V\t\tprint version and exit\n"
          "  -q\t\tbe quiet\n"
          "  -p\t\tshow statistics of page operations for current session\n"
          "  -e\t\tshow whole DB info\n"
          "  -f\t\tshow GC info\n"
          "  -r\t\tshow readers\n"
          "  -a\t\tprint stat of main DB and all tables\n"
          "  -s table\tprint stat of only the specified named table\n"
          "  \t\tby default print stat of only the main DB\n",
          prog);
  exit(EXIT_FAILURE);
}

static int reader_list_func(void *ctx, int num, int slot, mdbx_pid_t pid, mdbx_tid_t thread, uint64_t txnid,
                            uint64_t lag, size_t bytes_used, size_t bytes_retained) {
  (void)ctx;
  if (num == 1)
    printf("Reader Table\n"
           "   #\tslot\t%6s %*s %20s %10s %13s %13s\n",
           "pid", (int)sizeof(size_t) * 2, "thread", "txnid", "lag", "used", "retained");

  if (thread < (mdbx_tid_t)((intptr_t)MDBX_TID_TXN_OUSTED))
    printf(" %3d)\t[%d]\t%6" PRIdSIZE " %*" PRIxPTR, num, slot, (size_t)pid, (int)sizeof(size_t) * 2,
           (uintptr_t)thread);
  else
    printf(" %3d)\t[%d]\t%6" PRIdSIZE " %sed", num, slot, (size_t)pid,
           (thread == (mdbx_tid_t)((uintptr_t)MDBX_TID_TXN_PARKED)) ? "park" : "oust");

  if (txnid)
    printf(" %20" PRIu64 " %10" PRIu64 " %12.1fM %12.1fM\n", txnid, lag, bytes_used / 1048576.0,
           bytes_retained / 1048576.0);
  else
    printf(" %20s %10s %13s %13s\n", "-", "0", "0", "0");

  return user_break ? MDBX_RESULT_TRUE : MDBX_RESULT_FALSE;
}

const char *prog;
bool quiet = false;
static void error(const char *func, int rc) {
  if (!quiet)
    fprintf(stderr, "%s: %s() error %d %s\n", prog, func, rc, mdbx_strerror(rc));
}

static void logger(MDBX_log_level_t level, const char *function, int line, const char *fmt, va_list args) {
  static const char *const prefixes[] = {
      "!!!fatal: ", // 0 fatal
      " ! ",        // 1 error
      " ~ ",        // 2 warning
      "   ",        // 3 notice
      "   //",      // 4 verbose
  };
  if (level < MDBX_LOG_DEBUG) {
    if (function && line)
      fprintf(stderr, "%s", prefixes[level]);
    vfprintf(stderr, fmt, args);
  }
}

int main(int argc, char *argv[]) {
  int opt, rc;
  MDBX_env *env;
  MDBX_txn *txn;
  MDBX_dbi dbi;
  MDBX_envinfo mei;
  prog = argv[0];
  char *envname;
  char *table = nullptr;
  bool alldbs = false, envinfo = false, pgop = false;
  int freinfo = 0, rdrinfo = 0;

  if (argc < 2)
    usage(prog);

  while ((opt = getopt(argc, argv,
                       "V"
                       "q"
                       "p"
                       "a"
                       "e"
                       "f"
                       "n"
                       "r"
                       "s:")) != EOF) {
    switch (opt) {
    case 'V':
      printf("mdbx_stat version %d.%d.%d.%d\n"
             " - source: %s %s, commit %s, tree %s\n"
             " - anchor: %s\n"
             " - build: %s for %s by %s\n"
             " - flags: %s\n"
             " - options: %s\n",
             mdbx_version.major, mdbx_version.minor, mdbx_version.patch, mdbx_version.tweak, mdbx_version.git.describe,
             mdbx_version.git.datetime, mdbx_version.git.commit, mdbx_version.git.tree, mdbx_sourcery_anchor,
             mdbx_build.datetime, mdbx_build.target, mdbx_build.compiler, mdbx_build.flags, mdbx_build.options);
      return EXIT_SUCCESS;
    case 'q':
      quiet = true;
      break;
    case 'p':
      pgop = true;
      break;
    case 'a':
      if (table)
        usage(prog);
      alldbs = true;
      break;
    case 'e':
      envinfo = true;
      break;
    case 'f':
      freinfo += 1;
      break;
    case 'n':
      break;
    case 'r':
      rdrinfo += 1;
      break;
    case 's':
      if (alldbs)
        usage(prog);
      table = optarg;
      break;
    default:
      usage(prog);
    }
  }

  if (optind != argc - 1)
    usage(prog);

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
  envname = argv[optind];
  if (!quiet) {
    printf("mdbx_stat %s (%s, T-%s)\nRunning for %s...\n", mdbx_version.git.describe, mdbx_version.git.datetime,
           mdbx_version.git.tree, envname);
    fflush(nullptr);
    mdbx_setup_debug(MDBX_LOG_NOTICE, MDBX_DBG_DONTCHANGE, logger);
  }

  rc = mdbx_env_create(&env);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_env_create", rc);
    return EXIT_FAILURE;
  }

  if (alldbs || table) {
    rc = mdbx_env_set_maxdbs(env, 2);
    if (unlikely(rc != MDBX_SUCCESS)) {
      error("mdbx_env_set_maxdbs", rc);
      goto env_close;
    }
  }

  rc = mdbx_env_open(env, envname, MDBX_RDONLY, 0);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_env_open", rc);
    goto env_close;
  }

  rc = mdbx_txn_begin(env, nullptr, MDBX_TXN_RDONLY, &txn);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_txn_begin", rc);
    goto txn_abort;
  }

  if (envinfo || freinfo || pgop) {
    rc = mdbx_env_info_ex(env, txn, &mei, sizeof(mei));
    if (unlikely(rc != MDBX_SUCCESS)) {
      error("mdbx_env_info_ex", rc);
      goto txn_abort;
    }
  } else {
    /* LY: zap warnings from gcc */
    memset(&mei, 0, sizeof(mei));
  }

  if (pgop) {
    printf("Page Operations (for current session):\n");
    printf("      New: %8" PRIu64 "\t// quantity of a new pages added\n", mei.mi_pgop_stat.newly);
    printf("      CoW: %8" PRIu64 "\t// quantity of pages copied for altering\n", mei.mi_pgop_stat.cow);
    printf("    Clone: %8" PRIu64 "\t// quantity of parent's dirty pages "
           "clones for nested transactions\n",
           mei.mi_pgop_stat.clone);
    printf("    Split: %8" PRIu64 "\t// page splits during insertions or updates\n", mei.mi_pgop_stat.split);
    printf("    Merge: %8" PRIu64 "\t// page merges during deletions or updates\n", mei.mi_pgop_stat.merge);
    printf("    Spill: %8" PRIu64 "\t// quantity of spilled/ousted `dirty` "
           "pages during large transactions\n",
           mei.mi_pgop_stat.spill);
    printf("  Unspill: %8" PRIu64 "\t// quantity of unspilled/redone `dirty` "
           "pages during large transactions\n",
           mei.mi_pgop_stat.unspill);
    printf("      WOP: %8" PRIu64 "\t// number of explicit write operations (not a pages) to a disk\n",
           mei.mi_pgop_stat.wops);
    printf(" PreFault: %8" PRIu64 "\t// number of prefault write operations (not a pages)\n",
           mei.mi_pgop_stat.prefault);
    printf("  mInCore: %8" PRIu64 "\t// number of mincore() calls\n", mei.mi_pgop_stat.mincore);
    printf("    mSync: %8" PRIu64 "\t// number of explicit msync-to-disk operations (not a pages)\n",
           mei.mi_pgop_stat.msync);
    printf("    fSync: %8" PRIu64 "\t// number of explicit fsync-to-disk operations (not a pages)\n",
           mei.mi_pgop_stat.fsync);
  }

  if (envinfo) {
    printf("Environment Info\n");
    printf("  Pagesize: %u\n", mei.mi_dxb_pagesize);
    if (mei.mi_geo.lower != mei.mi_geo.upper) {
      printf("  Dynamic datafile: %" PRIu64 "..%" PRIu64 " bytes (+%" PRIu64 "/-%" PRIu64 "), %" PRIu64 "..%" PRIu64
             " pages (+%" PRIu64 "/-%" PRIu64 ")\n",
             mei.mi_geo.lower, mei.mi_geo.upper, mei.mi_geo.grow, mei.mi_geo.shrink,
             mei.mi_geo.lower / mei.mi_dxb_pagesize, mei.mi_geo.upper / mei.mi_dxb_pagesize,
             mei.mi_geo.grow / mei.mi_dxb_pagesize, mei.mi_geo.shrink / mei.mi_dxb_pagesize);
      printf("  Current mapsize: %" PRIu64 " bytes, %" PRIu64 " pages \n", mei.mi_mapsize,
             mei.mi_mapsize / mei.mi_dxb_pagesize);
      printf("  Current datafile: %" PRIu64 " bytes, %" PRIu64 " pages\n", mei.mi_geo.current,
             mei.mi_geo.current / mei.mi_dxb_pagesize);
#if defined(_WIN32) || defined(_WIN64)
      if (mei.mi_geo.shrink && mei.mi_geo.current != mei.mi_geo.upper)
        printf("                    WARNING: Due Windows system limitations a "
               "file couldn't\n                    be truncated while database "
               "is opened. So, the size of\n                    database file "
               "may by large than the database itself,\n                    "
               "until it will be closed or reopened in read-write mode.\n");
#endif
    } else {
      printf("  Fixed datafile: %" PRIu64 " bytes, %" PRIu64 " pages\n", mei.mi_geo.current,
             mei.mi_geo.current / mei.mi_dxb_pagesize);
    }
    printf("  Last transaction ID: %" PRIu64 "\n", mei.mi_recent_txnid);
    printf("  Latter reader transaction ID: %" PRIu64 " (%" PRIi64 ")\n", mei.mi_latter_reader_txnid,
           mei.mi_latter_reader_txnid - mei.mi_recent_txnid);
    printf("  Max readers: %u\n", mei.mi_maxreaders);
    printf("  Number of reader slots uses: %u\n", mei.mi_numreaders);
  }

  if (rdrinfo) {
    rc = mdbx_reader_list(env, reader_list_func, nullptr);
    if (MDBX_IS_ERROR(rc)) {
      error("mdbx_reader_list", rc);
      goto txn_abort;
    }
    if (rc == MDBX_RESULT_TRUE)
      printf("Reader Table is absent\n");
    else if (rc == MDBX_SUCCESS && rdrinfo > 1) {
      int dead;
      rc = mdbx_reader_check(env, &dead);
      if (MDBX_IS_ERROR(rc)) {
        error("mdbx_reader_check", rc);
        goto txn_abort;
      }
      if (rc == MDBX_RESULT_TRUE) {
        printf("  %d stale readers cleared.\n", dead);
        rc = mdbx_reader_list(env, reader_list_func, nullptr);
        if (rc == MDBX_RESULT_TRUE)
          printf("  Now Reader Table is empty\n");
      } else
        printf("  No stale readers.\n");
    }
    if (!(table || alldbs || freinfo))
      goto txn_abort;
  }

  if (freinfo) {
    printf("Garbage Collection\n");
    dbi = 0;
    MDBX_cursor *cursor;
    rc = mdbx_cursor_open(txn, dbi, &cursor);
    if (unlikely(rc != MDBX_SUCCESS)) {
      error("mdbx_cursor_open", rc);
      goto txn_abort;
    }

    MDBX_stat mst;
    rc = mdbx_dbi_stat(txn, dbi, &mst, sizeof(mst));
    if (unlikely(rc != MDBX_SUCCESS)) {
      error("mdbx_dbi_stat", rc);
      goto txn_abort;
    }
    print_stat(&mst);

    pgno_t pages = 0, *iptr;
    pgno_t reclaimable = 0;
    MDBX_val key, data;
    while (MDBX_SUCCESS == (rc = mdbx_cursor_get(cursor, &key, &data, MDBX_NEXT))) {
      if (user_break) {
        rc = MDBX_EINTR;
        break;
      }
      iptr = data.iov_base;
      const pgno_t number = *iptr++;

      pages += number;
      if (envinfo && mei.mi_latter_reader_txnid > *(txnid_t *)key.iov_base)
        reclaimable += number;

      if (freinfo > 1) {
        char *bad = "";
        pgno_t prev = MDBX_PNL_ASCENDING ? NUM_METAS - 1 : (pgno_t)mei.mi_last_pgno + 1;
        pgno_t span = 1;
        for (unsigned i = 0; i < number; ++i) {
          pgno_t pg = iptr[i];
          if (MDBX_PNL_DISORDERED(prev, pg))
            bad = " [bad sequence]";
          prev = pg;
          while (i + span < number && iptr[i + span] == (MDBX_PNL_ASCENDING ? pgno_add(pg, span) : pgno_sub(pg, span)))
            ++span;
        }
        printf("    Transaction %" PRIaTXN ", %" PRIaPGNO " pages, maxspan %" PRIaPGNO "%s\n", *(txnid_t *)key.iov_base,
               number, span, bad);
        if (freinfo > 2) {
          for (unsigned i = 0; i < number; i += span) {
            const pgno_t pg = iptr[i];
            for (span = 1;
                 i + span < number && iptr[i + span] == (MDBX_PNL_ASCENDING ? pgno_add(pg, span) : pgno_sub(pg, span));
                 ++span)
              ;
            if (span > 1)
              printf("     %9" PRIaPGNO "[%" PRIaPGNO "]\n", pg, span);
            else
              printf("     %9" PRIaPGNO "\n", pg);
          }
        }
      }
    }
    mdbx_cursor_close(cursor);
    cursor = nullptr;

    switch (rc) {
    case MDBX_SUCCESS:
    case MDBX_NOTFOUND:
      break;
    case MDBX_EINTR:
      if (!quiet)
        fprintf(stderr, "Interrupted by signal/user\n");
      goto txn_abort;
    default:
      error("mdbx_cursor_get", rc);
      goto txn_abort;
    }

    if (envinfo) {
      uint64_t value = mei.mi_mapsize / mei.mi_dxb_pagesize;
      double percent = value / 100.0;
      printf("Page Usage\n");
      printf("  Total: %" PRIu64 " 100%%\n", value);

      value = mei.mi_geo.current / mei.mi_dxb_pagesize;
      printf("  Backed: %" PRIu64 " %.1f%%\n", value, value / percent);

      value = mei.mi_last_pgno + 1;
      printf("  Allocated: %" PRIu64 " %.1f%%\n", value, value / percent);

      value = mei.mi_mapsize / mei.mi_dxb_pagesize - (mei.mi_last_pgno + 1);
      printf("  Remained: %" PRIu64 " %.1f%%\n", value, value / percent);

      value = mei.mi_last_pgno + 1 - pages;
      printf("  Used: %" PRIu64 " %.1f%%\n", value, value / percent);

      value = pages;
      printf("  GC: %" PRIu64 " %.1f%%\n", value, value / percent);

      value = pages - reclaimable;
      printf("  Retained: %" PRIu64 " %.1f%%\n", value, value / percent);

      value = reclaimable;
      printf("  Reclaimable: %" PRIu64 " %.1f%%\n", value, value / percent);

      value = mei.mi_mapsize / mei.mi_dxb_pagesize - (mei.mi_last_pgno + 1) + reclaimable;
      printf("  Available: %" PRIu64 " %.1f%%\n", value, value / percent);
    } else
      printf("  GC: %" PRIaPGNO " pages\n", pages);
  }

  rc = mdbx_dbi_open(txn, table, MDBX_DB_ACCEDE, &dbi);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_dbi_open", rc);
    goto txn_abort;
  }

  MDBX_stat mst;
  rc = mdbx_dbi_stat(txn, dbi, &mst, sizeof(mst));
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_dbi_stat", rc);
    goto txn_abort;
  }
  printf("Status of %s\n", table ? table : "Main DB");
  print_stat(&mst);

  if (alldbs) {
    MDBX_cursor *cursor;
    rc = mdbx_cursor_open(txn, dbi, &cursor);
    if (unlikely(rc != MDBX_SUCCESS)) {
      error("mdbx_cursor_open", rc);
      goto txn_abort;
    }

    MDBX_val key;
    while (MDBX_SUCCESS == (rc = mdbx_cursor_get(cursor, &key, nullptr, MDBX_NEXT_NODUP))) {
      MDBX_dbi xdbi;
      if (memchr(key.iov_base, '\0', key.iov_len))
        continue;
      table = osal_malloc(key.iov_len + 1);
      memcpy(table, key.iov_base, key.iov_len);
      table[key.iov_len] = '\0';
      rc = mdbx_dbi_open(txn, table, MDBX_DB_ACCEDE, &xdbi);
      if (rc == MDBX_SUCCESS)
        printf("Status of %s\n", table);
      osal_free(table);
      if (unlikely(rc != MDBX_SUCCESS)) {
        if (rc == MDBX_INCOMPATIBLE)
          continue;
        error("mdbx_dbi_open", rc);
        goto txn_abort;
      }

      rc = mdbx_dbi_stat(txn, xdbi, &mst, sizeof(mst));
      if (unlikely(rc != MDBX_SUCCESS)) {
        error("mdbx_dbi_stat", rc);
        goto txn_abort;
      }
      print_stat(&mst);

      rc = mdbx_dbi_close(env, xdbi);
      if (unlikely(rc != MDBX_SUCCESS)) {
        error("mdbx_dbi_close", rc);
        goto txn_abort;
      }
    }
    mdbx_cursor_close(cursor);
    cursor = nullptr;
  }

  switch (rc) {
  case MDBX_SUCCESS:
  case MDBX_NOTFOUND:
    break;
  case MDBX_EINTR:
    if (!quiet)
      fprintf(stderr, "Interrupted by signal/user\n");
    break;
  default:
    if (unlikely(rc != MDBX_SUCCESS))
      error("mdbx_cursor_get", rc);
  }

  mdbx_dbi_close(env, dbi);
txn_abort:
  mdbx_txn_abort(txn);
env_close:
  mdbx_env_close(env);

  return rc ? EXIT_FAILURE : EXIT_SUCCESS;
}
