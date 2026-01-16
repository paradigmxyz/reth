/// \copyright SPDX-License-Identifier: Apache-2.0
/// \author Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> \date 2015-2026
/* clang-format off */

#define MDBX_BUILD_SOURCERY 504a6ecaae8fc599ed314a60e624c8d15df1d8bed9a4f417271a7c3e05b56467_v0_14_1_291_g8fcc05f8

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
#define _WIN32_WINNT 0x0A00 /* Windows 10 */
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
#pragma warning(disable : 5286) /* implicit conversion from enum type 'type 1' to enum type 'type 2' */
#pragma warning(disable : 5287) /* operands are different enum types 'type 1' and 'type 2' */
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

#if __GLIBC_PREREQ(2, 16) || __has_include(<sys/auxv.h>)
#include <sys/auxv.h>
#endif /* glibc >= 2.16 */

#endif /*---------------------------------------------------------------------*/

#if defined(__ANDROID_API__) || defined(ANDROID)
#include <android/log.h>
#if __ANDROID_API__ >= 21
#include <sys/sendfile.h>
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

#if UINTPTR_MAX > 0xffffFFFFul || ULONG_MAX > 0xffffFFFFul || defined(_WIN64)
#define MDBX_WORDBITS 64
#define MDBX_WORDBITS_LN2 6
#else
#define MDBX_WORDBITS 32
#define MDBX_WORDBITS_LN2 5
#endif /* MDBX_WORDBITS */

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

#include "mdbx.h++"

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

#ifndef strcasecmp
#define strcasecmp _stricmp /* ntdll */
#endif

#ifndef strncasecmp
#define strncasecmp _strnicmp /* ntdll */
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

MDBX_MAYBE_UNUSED static inline bool osal_yield(void) {
#if defined(_WIN32) || defined(_WIN64)
  return SleepEx(0, true) == WAIT_IO_COMPLETION;
#else
  return sched_yield() != 0;
#endif
}

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

MDBX_INTERNAL int osal_waitstatus2errcode(DWORD result);

#elif defined(__ANDROID_API__)

#if __ANDROID_API__ < 24
/* https://android-developers.googleblog.com/2017/09/introducing-android-native-development.html
 * https://android.googlesource.com/platform/bionic/+/master/docs/32-bit-abi.md */
#define MDBX_HAVE_PWRITEV 0
#if defined(_FILE_OFFSET_BITS) && _FILE_OFFSET_BITS != MDBX_WORDBITS
#error "_FILE_OFFSET_BITS != MDBX_WORDBITS and __ANDROID_API__ < 24" (_FILE_OFFSET_BITS != MDBX_WORDBITS)
#elif defined(__FILE_OFFSET_BITS) && __FILE_OFFSET_BITS != MDBX_WORDBITS
#error "__FILE_OFFSET_BITS != MDBX_WORDBITS and __ANDROID_API__ < 24" (__FILE_OFFSET_BITS != MDBX_WORDBITS)
#endif
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
MDBX_INTERNAL int osal_fsetsize(mdbx_filehandle_t fd, const uint64_t length);
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
  MDBX_OPEN_DELETE,
  MDBX_OPEN_COPY_EXCL,
  MDBX_OPEN_COPY_OVERWRITE,
};

MDBX_MAYBE_UNUSED static inline bool osal_isdirsep(pathchar_t c) {
  return
#if defined(_WIN32) || defined(_WIN64)
      c == '\\' ||
#endif
      c == '/';
}

MDBX_INTERNAL const char *osal_getenv(const char *name, bool secure);
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

#define MMAP_OPTION_SETLENGTH 1
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
MDBX_INTERNAL int osal_msync(const osal_mmap_t *map, size_t length, enum osal_syncmode_bits mode_bits);
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
#if defined(MDBX_BUILD_CXX) && !MDBX_BUILD_CXX && (defined(_WIN32) || defined(_WIN64))
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

/** if enabled then treats the commit of pure (nothing changes) transactions as special
 * cases and return \ref MDBX_RESULT_TRUE instead of \ref MDBX_SUCCESS. */
#ifndef MDBX_NOSUCCESS_PURE_COMMIT
#define MDBX_NOSUCCESS_PURE_COMMIT 0
#elif !(MDBX_NOSUCCESS_PURE_COMMIT == 0 || MDBX_NOSUCCESS_PURE_COMMIT == 1)
#error MDBX_NOSUCCESS_PURE_COMMIT must be defined as 0 or 1
#endif /* MDBX_NOSUCCESS_PURE_COMMIT */

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
    (!defined(__GLIBC__) || __GLIBC_PREREQ(2, 10) /* troubles with Robust mutexes before 2.10 */) &&                   \
    !defined(__OHOS__) /* Harmony OS doesn't support robust mutexes at the end of 2025 */
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

/** Advanced: Using posix_fallocate() or fcntl(F_PREALLOCATE) on OSX (autodetection by default). */
#ifndef MDBX_USE_FALLOCATE
#if defined(__APPLE__)
#define MDBX_USE_FALLOCATE 0 /* Too slow and unclean, but not required to prevent SIGBUS */
#elif (defined(_POSIX_C_SOURCE) && _POSIX_C_SOURCE >= 200112L) || (__GLIBC_PREREQ(2, 10) && defined(_GNU_SOURCE))
#define MDBX_USE_FALLOCATE 1
#else
#define MDBX_USE_FALLOCATE 0
#endif
#define MDBX_USE_FALLOCATE_CONFIG "AUTO=" MDBX_STRINGIFY(MDBX_USE_FALLOCATE)
#elif !(MDBX_USE_FALLOCATE == 0 || MDBX_USE_FALLOCATE == 1)
#error MDBX_USE_FALLOCATE must be defined as 0 or 1
#else
#define MDBX_USE_FALLOCATE_CONFIG MDBX_STRINGIFY(MDBX_USE_FALLOCATE)
#endif /* MDBX_USE_FALLOCATE */

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
  P_TYPE = (P_BRANCH | P_LEAF | P_LARGE | P_META | P_DUPFIX | P_SUBP),
  P_FLAGS = (P_BAD | P_SPILLED | P_LOOSE | P_FROZEN),
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
   * It must be set to MDBX_MAGIC with MDBX_LOCK_VERSION. */
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
#ifdef AT_UCACHEBSIZE
  unsigned sys_unified_cache_block;
#endif /* AT_UCACHEBSIZE */
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
#if !((defined(_WIN32) || defined(_WIN64)) && defined(_DEBUG) && !MDBX_WITHOUT_MSVC_CRT)
MDBX_NORETURN
#endif
__cold void assert_fail(const char *msg, const char *func, unsigned line);
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

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED MDBX_INTERNAL unsigned ceil_log2n(size_t value_uintptr);

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

typedef struct ratio2digits_buffer {
  char string[1 + 20 + 1 + 19 + 1];
} ratio2digits_buffer_t;

char *ratio2digits(const uint64_t v, const uint64_t d, ratio2digits_buffer_t *const buffer, int precision);
char *ratio2percent(const uint64_t v, const uint64_t d, ratio2digits_buffer_t *const buffer);

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

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline size_t pnl_alloclen(const_pnl_t pnl) { return pnl[-1]; }

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline size_t pnl_size(const_pnl_t pnl) { return pnl[0]; }

MDBX_MAYBE_UNUSED static inline void pnl_setsize(pnl_t pnl, size_t len) {
  assert(len < INT_MAX);
  pnl[0] = (pgno_t)len;
}

#define MDBX_PNL_FIRST(pl) ((pl)[1])
#define MDBX_PNL_LAST(pl) ((pl)[pnl_size(pl)])
#define MDBX_PNL_BEGIN(pl) (&(pl)[1])
#define MDBX_PNL_END(pl) (&(pl)[pnl_size(pl) + 1])

#if MDBX_PNL_ASCENDING
#define MDBX_PNL_EDGE(pl) ((pl) + 1)
#define MDBX_PNL_LEAST(pl) MDBX_PNL_FIRST(pl)
#define MDBX_PNL_MOST(pl) MDBX_PNL_LAST(pl)
#define MDBX_PNL_CONTIGUOUS(prev, next, span) ((next) - (prev)) == (span))
#else
#define MDBX_PNL_EDGE(pl) ((pl) + pnl_size(pl))
#define MDBX_PNL_LEAST(pl) MDBX_PNL_LAST(pl)
#define MDBX_PNL_MOST(pl) MDBX_PNL_FIRST(pl)
#define MDBX_PNL_CONTIGUOUS(prev, next, span) (((prev) - (next)) == (span))
#endif

#define MDBX_PNL_SIZEOF(pl) ((pnl_size(pl) + 1) * sizeof(pgno_t))
#define MDBX_PNL_IS_EMPTY(pl) (pnl_size(pl) == 0)

MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline pgno_t pnl_bytes2size(const size_t bytes) {
  size_t size = bytes / sizeof(pgno_t);
  assert(size > 3 && size <= PAGELIST_LIMIT + /* alignment gap */ 65536);
  size -= 3;
#if MDBX_PNL_PREALLOC_FOR_RADIXSORT
  size >>= 1;
#endif /* MDBX_PNL_PREALLOC_FOR_RADIXSORT */
  return (pgno_t)size;
}

MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline size_t pnl_size2bytes(size_t wanna_size) {
  size_t size = wanna_size;
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
  assert(pnl_bytes2size(bytes) >= wanna_size);
  return bytes;
}

MDBX_INTERNAL pnl_t pnl_alloc(size_t size);

MDBX_INTERNAL void pnl_free(pnl_t pnl);

MDBX_MAYBE_UNUSED MDBX_INTERNAL pnl_t pnl_clone(const pnl_t src);

MDBX_INTERNAL int pnl_reserve(pnl_t __restrict *__restrict ppnl, const size_t wanna);

MDBX_MAYBE_UNUSED static inline int __must_check_result pnl_need(pnl_t __restrict *__restrict ppnl, size_t num) {
  assert(pnl_size(*ppnl) <= PAGELIST_LIMIT && pnl_alloclen(*ppnl) >= pnl_size(*ppnl));
  assert(num <= PAGELIST_LIMIT);
  const size_t wanna = pnl_size(*ppnl) + num;
  return likely(pnl_alloclen(*ppnl) >= wanna) ? MDBX_SUCCESS : pnl_reserve(ppnl, wanna);
}

MDBX_MAYBE_UNUSED static inline void pnl_append_prereserved(__restrict pnl_t pnl, pgno_t pgno) {
  assert(pnl_size(pnl) < pnl_alloclen(pnl));
  if (AUDIT_ENABLED()) {
    for (size_t i = pnl_size(pnl); i > 0; --i)
      assert(pgno != pnl[i]);
  }
  *pnl += 1;
  MDBX_PNL_LAST(pnl) = pgno;
}

MDBX_MAYBE_UNUSED static inline int __must_check_result pnl_append(__restrict pnl_t *ppnl, pgno_t pgno) {
  int rc = pnl_need(ppnl, 1);
  if (likely(rc == MDBX_SUCCESS))
    pnl_append_prereserved(*ppnl, pgno);
  return rc;
}

MDBX_INTERNAL void pnl_shrink(pnl_t __restrict *__restrict ppnl);

MDBX_INTERNAL int __must_check_result spill_append_span(__restrict pnl_t *ppnl, pgno_t pgno, size_t n);

MDBX_INTERNAL int __must_check_result pnl_append_span(__restrict pnl_t *ppnl, pgno_t pgno, size_t n);

MDBX_INTERNAL int __must_check_result pnl_insert_span(__restrict pnl_t *ppnl, pgno_t pgno, size_t n);

MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t pnl_search_nochk(const pnl_t pnl, pgno_t pgno);

MDBX_INTERNAL void pnl_sort_nochk(pnl_t pnl);

MDBX_INTERNAL bool pnl_check(const const_pnl_t pnl, const size_t limit);

MDBX_MAYBE_UNUSED static inline bool pnl_check_allocated(const const_pnl_t pnl, const size_t limit) {
  return pnl == nullptr || (pnl_alloclen(pnl) >= pnl_size(pnl) && pnl_check(pnl, limit));
}

MDBX_MAYBE_UNUSED static inline void pnl_sort(pnl_t pnl, size_t limit4check) {
  pnl_sort_nochk(pnl);
  assert(pnl_check(pnl, limit4check));
  (void)limit4check;
}

MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline size_t pnl_search(const pnl_t pnl, pgno_t pgno,
                                                                             size_t limit) {
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

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t pnl_maxspan(const pnl_t pnl);

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

/*----------------------------------------------------------------------------*/

#ifdef _MSC_VER
#pragma warning(push, 1)
#if _MSC_VER > 1913
#pragma warning(disable : 5054) /* deprecated between enumerations of different types */
#endif
#endif /* MSVC */

typedef struct dp dp_t;
typedef struct dpl dpl_t;
typedef struct kvx kvx_t;
typedef struct meta_ptr meta_ptr_t;
typedef struct inner_cursor subcur_t;
typedef struct cursor_couple cursor_couple_t;
typedef struct defer_free_item defer_free_item_t;

typedef struct troika {
  uint8_t fsm, recent, prefer_steady, tail_and_flags;
#if MDBX_WORDBITS > 32 /* Workaround for false-positives from Valgrind */
  uint32_t unused_pad;
#endif
#define TROIKA_HAVE_STEADY(troika) ((troika)->fsm & 7u)
#define TROIKA_STRICT_VALID(troika) ((troika)->tail_and_flags & 64u)
#define TROIKA_VALID(troika) ((troika)->tail_and_flags & 128u)
#define TROIKA_TAIL(troika) ((troika)->tail_and_flags & 3u)
  txnid_t txnid[NUM_METAS];
} troika_t;

typedef struct page_get_result {
  page_t *page;
  int err;
} pgr_t;

typedef struct node_search_result {
  node_t *node;
  bool exact;
} nsr_t;

typedef struct bind_reader_slot_result {
  int err;
  reader_slot_t *slot;
} bsr_t;

#ifndef __cplusplus

MDBX_MAYBE_UNUSED static __always_inline void atomic_yield(void) {
#if defined(_WIN32) || defined(_WIN64)
  YieldProcessor();
#elif defined(__ia32__) || defined(__e2k__)
  __builtin_ia32_pause();
#elif defined(__ia64__)
#if defined(__HP_cc__) || defined(__HP_aCC__)
  _Asm_hint(_HINT_PAUSE);
#else
  __asm__ __volatile__("hint @pause");
#endif
#elif defined(__aarch64__) || (defined(__ARM_ARCH) && __ARM_ARCH > 6) || defined(__ARM_ARCH_6K__)
#ifdef __CC_ARM
  __yield();
#else
  __asm__ __volatile__("yield");
#endif
#elif (defined(__mips64) || defined(__mips64__)) && defined(__mips_isa_rev) && __mips_isa_rev >= 2
  __asm__ __volatile__("pause");
#elif defined(__mips) || defined(__mips__) || defined(__mips64) || defined(__mips64__) || defined(_M_MRX000) ||        \
    defined(_MIPS_) || defined(__MWERKS__) || defined(__sgi)
  __asm__ __volatile__(".word 0x00000140");
#else
  osal_yield();
#endif
}

#ifdef MDBX_HAVE_C11ATOMICS
#define osal_memory_fence(order, write) atomic_thread_fence((write) ? mo_c11_store(order) : mo_c11_load(order))
#else /* MDBX_HAVE_C11ATOMICS */
#define osal_memory_fence(order, write)                                                                                \
  do {                                                                                                                 \
    osal_compiler_barrier();                                                                                           \
    if (write && order > (MDBX_CPU_WRITEBACK_INCOHERENT ? mo_Relaxed : mo_AcquireRelease))                             \
      osal_memory_barrier();                                                                                           \
  } while (0)
#endif /* MDBX_HAVE_C11ATOMICS */

#if defined(MDBX_HAVE_C11ATOMICS) && defined(__LCC__)
#define atomic_store32(p, value, order)                                                                                \
  ({                                                                                                                   \
    const uint32_t value_to_store = (value);                                                                           \
    atomic_store_explicit(MDBX_c11a_rw(uint32_t, p), value_to_store, mo_c11_store(order));                             \
    value_to_store;                                                                                                    \
  })
#define atomic_load32(p, order) atomic_load_explicit(MDBX_c11a_ro(uint32_t, p), mo_c11_load(order))
#define atomic_store64(p, value, order)                                                                                \
  ({                                                                                                                   \
    const uint64_t value_to_store = (value);                                                                           \
    atomic_store_explicit(MDBX_c11a_rw(uint64_t, p), value_to_store, mo_c11_store(order));                             \
    value_to_store;                                                                                                    \
  })
#define atomic_load64(p, order) atomic_load_explicit(MDBX_c11a_ro(uint64_t, p), mo_c11_load(order))
#endif /* LCC && MDBX_HAVE_C11ATOMICS */

#ifndef atomic_store32
MDBX_MAYBE_UNUSED static __always_inline uint32_t atomic_store32(mdbx_atomic_uint32_t *p, const uint32_t value,
                                                                 enum mdbx_memory_order order) {
  STATIC_ASSERT(sizeof(mdbx_atomic_uint32_t) == 4);
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
MDBX_MAYBE_UNUSED static __always_inline uint32_t atomic_load32(const volatile mdbx_atomic_uint32_t *p,
                                                                enum mdbx_memory_order order) {
  STATIC_ASSERT(sizeof(mdbx_atomic_uint32_t) == 4);
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

/*------------------------------------------------------------------------------
 * safe read/write volatile 64-bit fields on 32-bit architectures. */

/* LY: for testing non-atomic 64-bit txnid on 32-bit arches.
 * #define xMDBX_TXNID_STEP (UINT32_MAX / 3) */
#ifndef xMDBX_TXNID_STEP
#if MDBX_64BIT_CAS
#define xMDBX_TXNID_STEP 1u
#else
#define xMDBX_TXNID_STEP 2u
#endif
#endif /* xMDBX_TXNID_STEP */

#ifndef atomic_store64
MDBX_MAYBE_UNUSED static __always_inline uint64_t atomic_store64(mdbx_atomic_uint64_t *p, const uint64_t value,
                                                                 enum mdbx_memory_order order) {
  STATIC_ASSERT(sizeof(mdbx_atomic_uint64_t) == 8);
#if MDBX_64BIT_ATOMIC
#if __GNUC_PREREQ(11, 0)
  STATIC_ASSERT(__alignof__(mdbx_atomic_uint64_t) >= sizeof(uint64_t));
#endif /* GNU C >= 11 */
#ifdef MDBX_HAVE_C11ATOMICS
  assert(atomic_is_lock_free(MDBX_c11a_rw(uint64_t, p)));
  atomic_store_explicit(MDBX_c11a_rw(uint64_t, p), value, mo_c11_store(order));
#else  /* MDBX_HAVE_C11ATOMICS */
  if (order != mo_Relaxed)
    osal_compiler_barrier();
  p->weak = value;
  osal_memory_fence(order, true);
#endif /* MDBX_HAVE_C11ATOMICS */
#else  /* !MDBX_64BIT_ATOMIC */
  osal_compiler_barrier();
  atomic_store32(&p->low, (uint32_t)value, mo_Relaxed);
  jitter4testing(true);
  atomic_store32(&p->high, (uint32_t)(value >> 32), order);
  jitter4testing(true);
#endif /* !MDBX_64BIT_ATOMIC */
  return value;
}
#endif /* atomic_store64 */

#ifndef atomic_load64
MDBX_MAYBE_UNUSED static
#if MDBX_64BIT_ATOMIC
    __always_inline
#endif /* MDBX_64BIT_ATOMIC */
        uint64_t atomic_load64(const volatile mdbx_atomic_uint64_t *p, enum mdbx_memory_order order) {
  STATIC_ASSERT(sizeof(mdbx_atomic_uint64_t) == 8);
#if MDBX_64BIT_ATOMIC
#ifdef MDBX_HAVE_C11ATOMICS
  assert(atomic_is_lock_free(MDBX_c11a_ro(uint64_t, p)));
  return atomic_load_explicit(MDBX_c11a_ro(uint64_t, p), mo_c11_load(order));
#else  /* MDBX_HAVE_C11ATOMICS */
  osal_memory_fence(order, false);
  const uint64_t value = p->weak;
  if (order != mo_Relaxed)
    osal_compiler_barrier();
  return value;
#endif /* MDBX_HAVE_C11ATOMICS */
#else  /* !MDBX_64BIT_ATOMIC */
  osal_compiler_barrier();
  uint64_t value = (uint64_t)atomic_load32(&p->high, order) << 32;
  jitter4testing(true);
  value |= atomic_load32(&p->low, (order == mo_Relaxed) ? mo_Relaxed : mo_AcquireRelease);
  jitter4testing(true);
  for (;;) {
    osal_compiler_barrier();
    uint64_t again = (uint64_t)atomic_load32(&p->high, order) << 32;
    jitter4testing(true);
    again |= atomic_load32(&p->low, (order == mo_Relaxed) ? mo_Relaxed : mo_AcquireRelease);
    jitter4testing(true);
    if (likely(value == again))
      return value;
    value = again;
    atomic_yield();
  }
#endif /* !MDBX_64BIT_ATOMIC */
}
#endif /* atomic_load64 */

#if MDBX_64BIT_CAS
MDBX_MAYBE_UNUSED static __always_inline bool atomic_cas64(mdbx_atomic_uint64_t *p, uint64_t c, uint64_t v) {
#ifdef MDBX_HAVE_C11ATOMICS
  STATIC_ASSERT(sizeof(long long) >= sizeof(uint64_t));
  assert(atomic_is_lock_free(MDBX_c11a_rw(uint64_t, p)));
  return atomic_compare_exchange_strong(MDBX_c11a_rw(uint64_t, p), &c, v);
#elif defined(__GNUC__) || defined(__clang__)
  return __sync_bool_compare_and_swap(&p->weak, c, v);
#elif defined(_MSC_VER)
  return c == (uint64_t)_InterlockedCompareExchange64((volatile __int64 *)&p->weak, v, c);
#elif defined(__APPLE__)
  return OSAtomicCompareAndSwap64Barrier(c, v, &p->weak);
#else
#error FIXME: Unsupported compiler
#endif
}
#endif /* MDBX_64BIT_CAS */

MDBX_MAYBE_UNUSED static __always_inline bool atomic_cas32(mdbx_atomic_uint32_t *p, uint32_t c, uint32_t v) {
#ifdef MDBX_HAVE_C11ATOMICS
  STATIC_ASSERT(sizeof(int) >= sizeof(uint32_t));
  assert(atomic_is_lock_free(MDBX_c11a_rw(uint32_t, p)));
  return atomic_compare_exchange_strong(MDBX_c11a_rw(uint32_t, p), &c, v);
#elif defined(__GNUC__) || defined(__clang__)
  return __sync_bool_compare_and_swap(&p->weak, c, v);
#elif defined(_MSC_VER)
  STATIC_ASSERT(sizeof(volatile long) == sizeof(volatile uint32_t));
  return c == (uint32_t)_InterlockedCompareExchange((volatile long *)&p->weak, v, c);
#elif defined(__APPLE__)
  return OSAtomicCompareAndSwap32Barrier(c, v, &p->weak);
#else
#error FIXME: Unsupported compiler
#endif
}

MDBX_MAYBE_UNUSED static __always_inline uint32_t atomic_add32(mdbx_atomic_uint32_t *p, uint32_t v) {
#ifdef MDBX_HAVE_C11ATOMICS
  STATIC_ASSERT(sizeof(int) >= sizeof(uint32_t));
  assert(atomic_is_lock_free(MDBX_c11a_rw(uint32_t, p)));
  return atomic_fetch_add(MDBX_c11a_rw(uint32_t, p), v);
#elif defined(__GNUC__) || defined(__clang__)
  return __sync_fetch_and_add(&p->weak, v);
#elif defined(_MSC_VER)
  STATIC_ASSERT(sizeof(volatile long) == sizeof(volatile uint32_t));
  return (uint32_t)_InterlockedExchangeAdd((volatile long *)&p->weak, v);
#elif defined(__APPLE__)
  return OSAtomicAdd32Barrier(v, &p->weak);
#else
#error FIXME: Unsupported compiler
#endif
}

#define atomic_sub32(p, v) atomic_add32(p, 0 - (v))

MDBX_MAYBE_UNUSED static __always_inline uint64_t safe64_txnid_next(uint64_t txnid) {
  txnid += xMDBX_TXNID_STEP;
#if !MDBX_64BIT_CAS
  /* avoid overflow of low-part in safe64_reset() */
  txnid += (UINT32_MAX == (uint32_t)txnid);
#endif
  return txnid;
}

/* Atomically make target value >= SAFE64_INVALID_THRESHOLD */
MDBX_MAYBE_UNUSED static __always_inline void safe64_reset(mdbx_atomic_uint64_t *p, bool single_writer) {
  if (single_writer) {
#if MDBX_64BIT_ATOMIC && MDBX_WORDBITS >= 64
    atomic_store64(p, UINT64_MAX, mo_AcquireRelease);
#else
    atomic_store32(&p->high, UINT32_MAX, mo_AcquireRelease);
#endif /* MDBX_64BIT_ATOMIC && MDBX_WORDBITS >= 64 */
  } else {
#if MDBX_64BIT_CAS && MDBX_64BIT_ATOMIC
    /* atomically make value >= SAFE64_INVALID_THRESHOLD by 64-bit operation */
    atomic_store64(p, UINT64_MAX, mo_AcquireRelease);
#elif MDBX_64BIT_CAS
    /* atomically make value >= SAFE64_INVALID_THRESHOLD by 32-bit operation */
    atomic_store32(&p->high, UINT32_MAX, mo_AcquireRelease);
#else
    /* it is safe to increment low-part to avoid ABA, since xMDBX_TXNID_STEP > 1
     * and overflow was preserved in safe64_txnid_next() */
    STATIC_ASSERT(xMDBX_TXNID_STEP > 1);
    atomic_add32(&p->low, 1) /* avoid ABA in safe64_reset_compare() */;
    atomic_store32(&p->high, UINT32_MAX, mo_AcquireRelease);
    atomic_add32(&p->low, 1) /* avoid ABA in safe64_reset_compare() */;
#endif /* MDBX_64BIT_CAS && MDBX_64BIT_ATOMIC */
  }
  assert(p->weak >= SAFE64_INVALID_THRESHOLD);
  jitter4testing(true);
}

MDBX_MAYBE_UNUSED static __always_inline bool safe64_reset_compare(mdbx_atomic_uint64_t *p, uint64_t compare) {
  /* LY: This function is used to reset `txnid` from hsr-handler in case
   *     the asynchronously cancellation of read transaction. Therefore,
   *     there may be a collision between the cleanup performed here and
   *     asynchronous termination and restarting of the read transaction
   *     in another process/thread. In general we MUST NOT reset the `txnid`
   *     if a new transaction was started (i.e. if `txnid` was changed). */
#if MDBX_64BIT_CAS
  bool rc = atomic_cas64(p, compare, UINT64_MAX);
#else
  /* LY: There is no gold ratio here since shared mutex is too costly,
   *     in such way we must acquire/release it for every update of txnid,
   *     i.e. twice for each read transaction). */
  bool rc = false;
  if (likely(atomic_load32(&p->low, mo_AcquireRelease) == (uint32_t)compare &&
             atomic_cas32(&p->high, (uint32_t)(compare >> 32), UINT32_MAX))) {
    if (unlikely(atomic_load32(&p->low, mo_AcquireRelease) != (uint32_t)compare))
      atomic_cas32(&p->high, UINT32_MAX, (uint32_t)(compare >> 32));
    else
      rc = true;
  }
#endif /* MDBX_64BIT_CAS */
  jitter4testing(true);
  return rc;
}

MDBX_MAYBE_UNUSED static __always_inline void safe64_write(mdbx_atomic_uint64_t *p, const uint64_t v) {
  assert(p->weak >= SAFE64_INVALID_THRESHOLD);
#if MDBX_64BIT_ATOMIC && MDBX_64BIT_CAS
  atomic_store64(p, v, mo_AcquireRelease);
#else  /* MDBX_64BIT_ATOMIC */
  osal_compiler_barrier();
  /* update low-part but still value >= SAFE64_INVALID_THRESHOLD */
  atomic_store32(&p->low, (uint32_t)v, mo_Relaxed);
  assert(p->weak >= SAFE64_INVALID_THRESHOLD);
  jitter4testing(true);
  /* update high-part from SAFE64_INVALID_THRESHOLD to actual value */
  atomic_store32(&p->high, (uint32_t)(v >> 32), mo_AcquireRelease);
#endif /* MDBX_64BIT_ATOMIC */
  assert(p->weak == v);
  jitter4testing(true);
}

MDBX_MAYBE_UNUSED static __always_inline uint64_t safe64_read(const mdbx_atomic_uint64_t *p) {
  jitter4testing(true);
  uint64_t v = atomic_load64(p, mo_AcquireRelease);
  while (!MDBX_64BIT_ATOMIC && unlikely(v != p->weak)) {
    atomic_yield();
    v = atomic_load64(p, mo_AcquireRelease);
  }
  return v;
}

#if 0 /* unused for now */
MDBX_MAYBE_UNUSED static __always_inline bool safe64_is_valid(uint64_t v) {
#if MDBX_WORDBITS >= 64
  return v < SAFE64_INVALID_THRESHOLD;
#else
  return (v >> 32) != UINT32_MAX;
#endif /* MDBX_WORDBITS */
}

MDBX_MAYBE_UNUSED static __always_inline bool
 safe64_is_valid_ptr(const mdbx_atomic_uint64_t *p) {
#if MDBX_64BIT_ATOMIC
  return atomic_load64(p, mo_AcquireRelease) < SAFE64_INVALID_THRESHOLD;
#else
  return atomic_load32(&p->high, mo_AcquireRelease) != UINT32_MAX;
#endif /* MDBX_64BIT_ATOMIC */
}
#endif /* unused for now */

/* non-atomic write with safety for reading a half-updated value */
MDBX_MAYBE_UNUSED static __always_inline void safe64_update(mdbx_atomic_uint64_t *p, const uint64_t v) {
#if MDBX_64BIT_ATOMIC
  atomic_store64(p, v, mo_Relaxed);
#else
  safe64_reset(p, true);
  safe64_write(p, v);
#endif /* MDBX_64BIT_ATOMIC */
}

/* non-atomic increment with safety for reading a half-updated value */
MDBX_MAYBE_UNUSED static
#if MDBX_64BIT_ATOMIC
    __always_inline
#endif /* MDBX_64BIT_ATOMIC */
    void safe64_inc(mdbx_atomic_uint64_t *p, const uint64_t v) {
  assert(v > 0);
  safe64_update(p, safe64_read(p) + v);
}

#endif /* !__cplusplus */

/* Internal prototypes */

/* audit.c */
MDBX_INTERNAL int audit_ex(MDBX_txn *txn, size_t retired_stored, bool dont_filter_gc);

/* mvcc-readers.c */
MDBX_INTERNAL bsr_t mvcc_bind_slot(MDBX_env *env);
MDBX_MAYBE_UNUSED MDBX_INTERNAL pgno_t mvcc_largest_this(MDBX_env *env, pgno_t largest);
MDBX_INTERNAL txnid_t mvcc_shapshot_oldest(MDBX_env *const env, const txnid_t steady);
MDBX_INTERNAL pgno_t mvcc_snapshot_largest(const MDBX_env *env, pgno_t last_used_page);
MDBX_INTERNAL int mvcc_cleanup_dead(MDBX_env *env, int rlocked, int *dead);
MDBX_INTERNAL bool mvcc_kick_laggards(MDBX_env *env, const txnid_t laggard);

/* dxb.c */
MDBX_INTERNAL int dxb_setup(MDBX_env *env, const int lck_rc, const mdbx_mode_t mode_bits);
MDBX_INTERNAL int __must_check_result dxb_read_header(MDBX_env *env, meta_t *meta, const int lck_exclusive,
                                                      const mdbx_mode_t mode_bits);
enum resize_mode { implicit_grow, impilict_shrink, explicit_resize };
MDBX_INTERNAL int __must_check_result dxb_resize(MDBX_env *const env, const pgno_t used_pgno, const pgno_t size_pgno,
                                                 pgno_t limit_pgno, const enum resize_mode mode);
MDBX_INTERNAL int dxb_set_readahead(const MDBX_env *env, const pgno_t edge, const bool enable, const bool force_whole);
MDBX_INTERNAL int dxb_msync(const MDBX_env *env, size_t length_pages, enum osal_syncmode_bits mode_bits);
MDBX_INTERNAL int dxb_fsync(const MDBX_env *env, enum osal_syncmode_bits mode_bits);
MDBX_INTERNAL int __must_check_result dxb_sync_locked(MDBX_env *env, unsigned flags, meta_t *const pending,
                                                      troika_t *const troika);
#if defined(ENABLE_MEMCHECK) || defined(__SANITIZE_ADDRESS__)
MDBX_INTERNAL void dxb_sanitize_tail(MDBX_env *env, MDBX_txn *txn);
#else
MDBX_MAYBE_UNUSED static inline void dxb_sanitize_tail(MDBX_env *env, MDBX_txn *txn) {
  (void)env;
  (void)txn;
}
#endif /* ENABLE_MEMCHECK || __SANITIZE_ADDRESS__ */

/* txn.c */
#define TXN_END_NAMES                                                                                                  \
  {"committed", "pure-commit", "abort", "reset", "fail-begin", "fail-begin-nested", "ousted", nullptr}
enum {
  /* txn_end operation number, for logging */
  TXN_END_COMMITTED /* 0 */,
  TXN_END_PURE_COMMIT /* 1 */,
  TXN_END_ABORT /* 2 */,
  TXN_END_RESET /* 3 */,
  TXN_END_FAIL_BEGIN /* 4 */,
  TXN_END_FAIL_BEGIN_NESTED /* 5 */,
  TXN_END_OUSTED /* 6 */,

  TXN_END_OPMASK = 0x07 /* mask for txn_end() operation number */,
  TXN_END_UPDATE = 0x10 /* update env state (DBIs) */,
  TXN_END_FREE = 0x20 /* free txn unless it is env.basal_txn */,
  TXN_END_SLOT = 0x40 /* release any reader slot if NOSTICKYTHREADS */
};

struct commit_timestamp {
  uint64_t start, prep, gc, audit, write, sync, gc_cpu;
};

MDBX_INTERNAL bool txn_refund(MDBX_txn *txn);
MDBX_INTERNAL bool txn_gc_detent(const MDBX_txn *const txn);
MDBX_INTERNAL int txn_check_badbits_parked(const MDBX_txn *txn, int bad_bits);
MDBX_INTERNAL void txn_done_cursors(MDBX_txn *txn);
MDBX_INTERNAL int txn_shadow_cursors(const MDBX_txn *parent, const size_t dbi);

MDBX_INTERNAL MDBX_txn *txn_alloc(const MDBX_txn_flags_t flags, MDBX_env *env);
MDBX_INTERNAL int txn_abort(MDBX_txn *txn);
MDBX_INTERNAL int txn_renew(MDBX_txn *txn, unsigned flags);
MDBX_INTERNAL int txn_end(MDBX_txn *txn, unsigned mode);

MDBX_INTERNAL int txn_nested_create(MDBX_txn *parent, const MDBX_txn_flags_t flags);
MDBX_INTERNAL void txn_nested_abort(MDBX_txn *nested);
MDBX_INTERNAL int txn_nested_join(MDBX_txn *txn, struct commit_timestamp *ts);

MDBX_INTERNAL MDBX_txn *txn_basal_create(const size_t max_dbi);
MDBX_INTERNAL void txn_basal_destroy(MDBX_txn *txn);
MDBX_INTERNAL int txn_basal_start(MDBX_txn *txn, unsigned flags);
MDBX_INTERNAL int txn_basal_commit(MDBX_txn *txn, struct commit_timestamp *ts);
MDBX_INTERNAL int txn_basal_end(MDBX_txn *txn, unsigned mode);

MDBX_INTERNAL int txn_ro_park(MDBX_txn *txn, bool autounpark);
MDBX_INTERNAL int txn_ro_unpark(MDBX_txn *txn);
MDBX_INTERNAL int txn_ro_start(MDBX_txn *txn, unsigned flags);
MDBX_INTERNAL int txn_ro_end(MDBX_txn *txn, unsigned mode);
MDBX_INTERNAL int txn_ro_clone(const MDBX_txn *const source, MDBX_txn *const clone);
MDBX_INTERNAL int txn_ro_reset(MDBX_txn *txn);

/* env.c */
MDBX_INTERNAL int env_open(MDBX_env *env, mdbx_mode_t mode);
MDBX_INTERNAL int env_info(const MDBX_env *env, const MDBX_txn *txn, MDBX_envinfo *out, troika_t *troika);
MDBX_INTERNAL int env_sync(MDBX_env *env, bool force, bool nonblock);
MDBX_INTERNAL int env_close(MDBX_env *env, bool resurrect_after_fork);
MDBX_INTERNAL MDBX_txn *env_owned_wrtxn(const MDBX_env *env);
MDBX_INTERNAL int __must_check_result env_page_auxbuffer(MDBX_env *env);
MDBX_INTERNAL unsigned env_setup_pagesize(MDBX_env *env, const size_t pagesize);

/* api-opt.c */
MDBX_INTERNAL void env_options_init(MDBX_env *env);
MDBX_INTERNAL void env_options_adjust_defaults(MDBX_env *env);
MDBX_INTERNAL void env_options_adjust_dp_limit(MDBX_env *env);
MDBX_INTERNAL pgno_t default_dp_limit(const MDBX_env *env);

/* tree.c */
MDBX_INTERNAL int tree_drop(MDBX_cursor *mc, const bool may_have_tables);
MDBX_INTERNAL int __must_check_result tree_rebalance(MDBX_cursor *mc);
MDBX_INTERNAL int __must_check_result tree_propagate_key(MDBX_cursor *mc, const MDBX_val *key);
MDBX_INTERNAL void recalculate_merge_thresholds(MDBX_env *env);
MDBX_INTERNAL void recalculate_subpage_thresholds(MDBX_env *env);

/* table.c */
MDBX_INTERNAL int __must_check_result tbl_fetch(MDBX_txn *txn, MDBX_cursor *mc, size_t dbi, const MDBX_val *name,
                                                unsigned wanna_flags);
MDBX_INTERNAL int __must_check_result tbl_create(MDBX_txn *txn, MDBX_cursor *mc, size_t slot, const MDBX_val *name,
                                                 unsigned db_flags);
MDBX_INTERNAL int __must_check_result tbl_setup(const MDBX_env *env, volatile kvx_t *const kvx, const tree_t *const db);
MDBX_INTERNAL int __must_check_result tbl_refresh(MDBX_txn *txn, size_t dbi);
MDBX_INTERNAL int __must_check_result tbl_purge(MDBX_cursor *mc);

/* coherency.c */
MDBX_INTERNAL bool coherency_check_meta(const MDBX_env *env, const volatile meta_t *meta, bool report);
MDBX_INTERNAL int coherency_fetch_head(MDBX_txn *txn, const meta_ptr_t head, uint64_t *timestamp);
MDBX_INTERNAL int coherency_check_written(const MDBX_env *env, const txnid_t txnid, const volatile meta_t *meta,
                                          const intptr_t pgno, uint64_t *timestamp);
MDBX_INTERNAL int coherency_timeout(uint64_t *timestamp, intptr_t pgno, const MDBX_env *env);

/* Сортированный набор txnid, использующий внутри комбинацию непрерывного интервала и списка.
 * Обеспечивает хранение id записей при переработке, очистку и обновлении GC, включая возврат остатков переработанных
 * страниц.
 *
 * При переработке GC записи преимущественно выбираются последовательно, но это не гарантируется. В LIFO-режиме
 * переработка и добавление записей в rkl происходит преимущественно в обратном порядке, но из-за завершения читающих
 * транзакций могут быть «скачки» в прямом направлении. В FIFO-режиме записи GC перерабатываются в прямом порядке и при
 * этом линейно, но не обязательно строго последовательно, при этом гарантируется что между добавляемыми в rkl
 * идентификаторами в GC нет записей, т.е. между первой (минимальный id) и последней (максимальный id) в GC нет записей
 * и весь интервал может быть использован для возврата остатков страниц в GC.
 *
 * Таким образом, комбинация линейного интервала и списка (отсортированного в порядке возрастания элементов) является
 * рациональным решением, близким к теоретически оптимальному пределу.
 *
 * Реализация rkl достаточно проста/прозрачная, если не считать неочевидную «магию» обмена непрерывного интервала и
 * образующихся в списке последовательностей. Однако, именно этот автоматически выполняемый без лишних операций обмен
 * оправдывает все накладные расходы. */
typedef struct MDBX_rkl {
  txnid_t solid_begin, solid_end; /* начало и конец непрерывной последовательности solid_begin ... solid_end-1. */
  unsigned list_length;           /* текущая длина списка. */
  unsigned list_limit;    /* размер буфера выделенного под список, равен ARRAY_LENGTH(inplace) когда list == inplace. */
  txnid_t *list;          /* список отдельных элементов в порядке возрастания (наименьший в начале). */
  txnid_t inplace[4 + 8]; /* статический массив для коротких списков, чтобы избавиться от выделения/освобождения памяти
                           * в большинстве случаев. */
} rkl_t;

MDBX_MAYBE_UNUSED MDBX_INTERNAL void rkl_init(rkl_t *rkl);
MDBX_MAYBE_UNUSED MDBX_INTERNAL void rkl_clear(rkl_t *rkl);
MDBX_MAYBE_UNUSED static inline void rkl_clear_and_shrink(rkl_t *rkl) { rkl_clear(rkl); /* TODO */ }
MDBX_MAYBE_UNUSED MDBX_INTERNAL void rkl_destroy(rkl_t *rkl);
MDBX_MAYBE_UNUSED MDBX_INTERNAL void rkl_destructive_move(rkl_t *src, rkl_t *dst);
MDBX_MAYBE_UNUSED MDBX_INTERNAL __must_check_result int rkl_copy(const rkl_t *src, rkl_t *dst);
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool rkl_empty(const rkl_t *rkl) {
  return rkl->solid_begin > rkl->solid_end;
}
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL bool rkl_check(const rkl_t *rkl);
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t rkl_len(const rkl_t *rkl);
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL txnid_t rkl_lowest(const rkl_t *rkl);
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL txnid_t rkl_highest(const rkl_t *rkl);
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline txnid_t rkl_edge(const rkl_t *rkl,
                                                                            const bool highest_not_lowest) {
  return highest_not_lowest ? rkl_highest(rkl) : rkl_lowest(rkl);
}
MDBX_MAYBE_UNUSED MDBX_INTERNAL __must_check_result int rkl_push(rkl_t *rkl, const txnid_t id);
MDBX_MAYBE_UNUSED MDBX_INTERNAL txnid_t rkl_pop(rkl_t *rkl, const bool highest_not_lowest);
MDBX_MAYBE_UNUSED MDBX_INTERNAL __must_check_result int rkl_merge(const rkl_t *src, rkl_t *dst, bool ignore_duplicates);
MDBX_MAYBE_UNUSED MDBX_INTERNAL int rkl_destructive_merge(rkl_t *src, rkl_t *dst, bool ignore_duplicates);

/* Итератор для rkl.
 * Обеспечивает изоляцию внутреннего устройства rkl от остального кода, чем существенно его упрощает.
 * Фактически именно использованием rkl с итераторами ликвидируется "ребус" исторически образовавшийся в gc-update. */
typedef struct MDBX_rkl_iter {
  const rkl_t *rkl;
  unsigned pos;
  unsigned solid_offset;
} rkl_iter_t;

MDBX_MAYBE_UNUSED MDBX_INTERNAL __must_check_result rkl_iter_t rkl_iterator(const rkl_t *rkl, const bool reverse);
MDBX_MAYBE_UNUSED MDBX_INTERNAL __must_check_result txnid_t rkl_turn(rkl_iter_t *iter, const bool reverse);
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t rkl_left(rkl_iter_t *iter, const bool reverse);
MDBX_MAYBE_UNUSED MDBX_INTERNAL bool rkl_find(const rkl_t *rkl, const txnid_t id, rkl_iter_t *iter);
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION __must_check_result MDBX_INTERNAL bool rkl_contain(const rkl_t *rkl,
                                                                                                txnid_t id);

typedef struct MDBX_rkl_hole {
  txnid_t begin;
  txnid_t end;
} rkl_hole_t;
MDBX_MAYBE_UNUSED MDBX_INTERNAL __must_check_result rkl_hole_t rkl_hole(rkl_iter_t *iter, const bool reverse);

/* List of txnid */
typedef txnid_t *txl_t;
typedef const txnid_t *const_txl_t;

enum txl_rules {
  txl_granulate = 32,
  txl_initial = txl_granulate - 2 - MDBX_ASSUME_MALLOC_OVERHEAD / sizeof(txnid_t),
  txl_max = (1u << 26) - 2 - MDBX_ASSUME_MALLOC_OVERHEAD / sizeof(txnid_t)
};

MDBX_MAYBE_UNUSED MDBX_INTERNAL txl_t txl_alloc(void);

MDBX_MAYBE_UNUSED MDBX_INTERNAL void txl_free(txl_t txl);

MDBX_MAYBE_UNUSED MDBX_INTERNAL int __must_check_result txl_append(txl_t __restrict *ptxl, txnid_t id);

MDBX_MAYBE_UNUSED MDBX_INTERNAL void txl_sort(txl_t txl);

MDBX_MAYBE_UNUSED MDBX_INTERNAL bool txl_contain(const txl_t txl, txnid_t id);

MDBX_MAYBE_UNUSED static inline size_t txl_alloclen(const_txl_t txl) { return txl[-1]; }

MDBX_MAYBE_UNUSED static inline size_t txl_size(const_txl_t txl) { return txl[0]; }

/*------------------------------------------------------------------------------
 * Unaligned access */

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED static inline size_t field_alignment(size_t alignment_baseline,
                                                                                   size_t field_offset) {
  size_t merge = alignment_baseline | (size_t)field_offset;
  return merge & -(int)merge;
}

/* read-thunk for UB-sanitizer */
MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline uint8_t peek_u8(const uint8_t *__restrict ptr) {
  return *ptr;
}

/* write-thunk for UB-sanitizer */
MDBX_MAYBE_UNUSED static inline void poke_u8(uint8_t *__restrict ptr, const uint8_t v) { *ptr = v; }

MDBX_MAYBE_UNUSED static inline void *bcopy_2(void *__restrict dst, const void *__restrict src) {
  uint8_t *__restrict d = (uint8_t *)dst;
  const uint8_t *__restrict s = (uint8_t *)src;
  d[0] = s[0];
  d[1] = s[1];
  return d;
}

MDBX_MAYBE_UNUSED static inline void *bcopy_4(void *const __restrict dst, const void *const __restrict src) {
  uint8_t *__restrict d = (uint8_t *)dst;
  const uint8_t *__restrict s = (uint8_t *)src;
  d[0] = s[0];
  d[1] = s[1];
  d[2] = s[2];
  d[3] = s[3];
  return d;
}

MDBX_MAYBE_UNUSED static inline void *bcopy_8(void *const __restrict dst, const void *const __restrict src) {
  uint8_t *__restrict d = (uint8_t *)dst;
  const uint8_t *__restrict s = (uint8_t *)src;
  d[0] = s[0];
  d[1] = s[1];
  d[2] = s[2];
  d[3] = s[3];
  d[4] = s[4];
  d[5] = s[5];
  d[6] = s[6];
  d[7] = s[7];
  return d;
}

MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline uint16_t unaligned_peek_u16(const size_t expected_alignment,
                                                                                       const void *const ptr) {
  assert((uintptr_t)ptr % expected_alignment == 0);
  if (MDBX_UNALIGNED_OK >= 2 || (expected_alignment % sizeof(uint16_t)) == 0)
    return *(const uint16_t *)ptr;
  else {
#if defined(__unaligned) || defined(_M_ARM) || defined(_M_ARM64) || defined(_M_X64) || defined(_M_IA64)
    return *(const __unaligned uint16_t *)ptr;
#else
    uint16_t v;
    bcopy_2((uint8_t *)&v, (const uint8_t *)ptr);
    return v;
#endif /* _MSC_VER || __unaligned */
  }
}

MDBX_MAYBE_UNUSED static inline void unaligned_poke_u16(const size_t expected_alignment, void *const __restrict ptr,
                                                        const uint16_t v) {
  assert((uintptr_t)ptr % expected_alignment == 0);
  if (MDBX_UNALIGNED_OK >= 2 || (expected_alignment % sizeof(v)) == 0)
    *(uint16_t *)ptr = v;
  else {
#if defined(__unaligned) || defined(_M_ARM) || defined(_M_ARM64) || defined(_M_X64) || defined(_M_IA64)
    *((uint16_t __unaligned *)ptr) = v;
#else
    bcopy_2((uint8_t *)ptr, (const uint8_t *)&v);
#endif /* _MSC_VER || __unaligned */
  }
}

MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline uint32_t
unaligned_peek_u32(const size_t expected_alignment, const void *const __restrict ptr) {
  assert((uintptr_t)ptr % expected_alignment == 0);
  if (MDBX_UNALIGNED_OK >= 4 || (expected_alignment % sizeof(uint32_t)) == 0)
    return *(const uint32_t *)ptr;
  else if ((expected_alignment % sizeof(uint16_t)) == 0) {
    const uint16_t lo = ((const uint16_t *)ptr)[__BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__];
    const uint16_t hi = ((const uint16_t *)ptr)[__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__];
    return lo | (uint32_t)hi << 16;
  } else {
#if defined(__unaligned) || defined(_M_ARM) || defined(_M_ARM64) || defined(_M_X64) || defined(_M_IA64)
    return *(const __unaligned uint32_t *)ptr;
#else
    uint32_t v;
    bcopy_4((uint8_t *)&v, (const uint8_t *)ptr);
    return v;
#endif /* _MSC_VER || __unaligned */
  }
}

MDBX_MAYBE_UNUSED static inline void unaligned_poke_u32(const size_t expected_alignment, void *const __restrict ptr,
                                                        const uint32_t v) {
  assert((uintptr_t)ptr % expected_alignment == 0);
  if (MDBX_UNALIGNED_OK >= 4 || (expected_alignment % sizeof(v)) == 0)
    *(uint32_t *)ptr = v;
  else if ((expected_alignment % sizeof(uint16_t)) == 0) {
    ((uint16_t *)ptr)[__BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__] = (uint16_t)v;
    ((uint16_t *)ptr)[__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__] = (uint16_t)(v >> 16);
  } else {
#if defined(__unaligned) || defined(_M_ARM) || defined(_M_ARM64) || defined(_M_X64) || defined(_M_IA64)
    *((uint32_t __unaligned *)ptr) = v;
#else
    bcopy_4((uint8_t *)ptr, (const uint8_t *)&v);
#endif /* _MSC_VER || __unaligned */
  }
}

MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline uint64_t
unaligned_peek_u64(const size_t expected_alignment, const void *const __restrict ptr) {
  assert((uintptr_t)ptr % expected_alignment == 0);
  if (MDBX_UNALIGNED_OK >= 8 || (expected_alignment % sizeof(uint64_t)) == 0)
    return *(const uint64_t *)ptr;
  else if ((expected_alignment % sizeof(uint32_t)) == 0) {
    const uint32_t lo = ((const uint32_t *)ptr)[__BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__];
    const uint32_t hi = ((const uint32_t *)ptr)[__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__];
    return lo | (uint64_t)hi << 32;
  } else {
#if defined(__unaligned) || defined(_M_ARM) || defined(_M_ARM64) || defined(_M_X64) || defined(_M_IA64)
    return *(const __unaligned uint64_t *)ptr;
#else
    uint64_t v;
    bcopy_8((uint8_t *)&v, (const uint8_t *)ptr);
    return v;
#endif /* _MSC_VER || __unaligned */
  }
}

MDBX_MAYBE_UNUSED static inline uint64_t unaligned_peek_u64_volatile(const size_t expected_alignment,
                                                                     const volatile void *const __restrict ptr) {
  assert((uintptr_t)ptr % expected_alignment == 0);
  assert(expected_alignment % sizeof(uint32_t) == 0);
  if (MDBX_UNALIGNED_OK >= 8 || (expected_alignment % sizeof(uint64_t)) == 0)
    return *(const volatile uint64_t *)ptr;
  else {
#if defined(__unaligned) || defined(_M_ARM) || defined(_M_ARM64) || defined(_M_X64) || defined(_M_IA64)
    return *(const volatile __unaligned uint64_t *)ptr;
#else
    const uint32_t lo = ((const volatile uint32_t *)ptr)[__BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__];
    const uint32_t hi = ((const volatile uint32_t *)ptr)[__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__];
    return lo | (uint64_t)hi << 32;
#endif /* _MSC_VER || __unaligned */
  }
}

MDBX_MAYBE_UNUSED static inline void unaligned_poke_u64(const size_t expected_alignment, void *const __restrict ptr,
                                                        const uint64_t v) {
  assert((uintptr_t)ptr % expected_alignment == 0);
  if (MDBX_UNALIGNED_OK >= 8 || (expected_alignment % sizeof(v)) == 0)
    *(uint64_t *)ptr = v;
  else if ((expected_alignment % sizeof(uint32_t)) == 0) {
    ((uint32_t *)ptr)[__BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__] = (uint32_t)v;
    ((uint32_t *)ptr)[__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__] = (uint32_t)(v >> 32);
  } else {
#if defined(__unaligned) || defined(_M_ARM) || defined(_M_ARM64) || defined(_M_X64) || defined(_M_IA64)
    *((uint64_t __unaligned *)ptr) = v;
#else
    bcopy_8((uint8_t *)ptr, (const uint8_t *)&v);
#endif /* _MSC_VER || __unaligned */
  }
}

#define UNALIGNED_PEEK_8(ptr, struct, field) peek_u8(ptr_disp(ptr, offsetof(struct, field)))
#define UNALIGNED_POKE_8(ptr, struct, field, value) poke_u8(ptr_disp(ptr, offsetof(struct, field)), value)

#define UNALIGNED_PEEK_16(ptr, struct, field) unaligned_peek_u16(1, ptr_disp(ptr, offsetof(struct, field)))
#define UNALIGNED_POKE_16(ptr, struct, field, value)                                                                   \
  unaligned_poke_u16(1, ptr_disp(ptr, offsetof(struct, field)), value)

#define UNALIGNED_PEEK_32(ptr, struct, field) unaligned_peek_u32(1, ptr_disp(ptr, offsetof(struct, field)))
#define UNALIGNED_POKE_32(ptr, struct, field, value)                                                                   \
  unaligned_poke_u32(1, ptr_disp(ptr, offsetof(struct, field)), value)

#define UNALIGNED_PEEK_64(ptr, struct, field) unaligned_peek_u64(1, ptr_disp(ptr, offsetof(struct, field)))
#define UNALIGNED_POKE_64(ptr, struct, field, value)                                                                   \
  unaligned_poke_u64(1, ptr_disp(ptr, offsetof(struct, field)), value)

MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline pgno_t peek_pgno(const void *const __restrict ptr) {
  if (sizeof(pgno_t) == sizeof(uint32_t))
    return (pgno_t)unaligned_peek_u32(1, ptr);
  else if (sizeof(pgno_t) == sizeof(uint64_t))
    return (pgno_t)unaligned_peek_u64(1, ptr);
  else {
    pgno_t pgno;
    memcpy(&pgno, ptr, sizeof(pgno));
    return pgno;
  }
}

MDBX_MAYBE_UNUSED static inline void poke_pgno(void *const __restrict ptr, const pgno_t pgno) {
  if (sizeof(pgno) == sizeof(uint32_t))
    unaligned_poke_u32(1, ptr, pgno);
  else if (sizeof(pgno) == sizeof(uint64_t))
    unaligned_poke_u64(1, ptr, pgno);
  else
    memcpy(ptr, &pgno, sizeof(pgno));
}
#if defined(_WIN32) || defined(_WIN64)

typedef union osal_srwlock {
  __anonymous_struct_extension__ struct {
    long volatile readerCount;
    long volatile writerCount;
  };
  RTL_SRWLOCK native;
} osal_srwlock_t;

typedef void(WINAPI *osal_srwlock_t_function)(osal_srwlock_t *);

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

typedef BOOL(WINAPI *MDBX_GetFileInformationByHandleEx)(_In_ HANDLE hFile,
                                                        _In_ FILE_INFO_BY_HANDLE_CLASS FileInformationClass,
                                                        _Out_ LPVOID lpFileInformation, _In_ DWORD dwBufferSize);

typedef BOOL(WINAPI *MDBX_GetVolumeInformationByHandleW)(
    _In_ HANDLE hFile, _Out_opt_ LPWSTR lpVolumeNameBuffer, _In_ DWORD nVolumeNameSize,
    _Out_opt_ LPDWORD lpVolumeSerialNumber, _Out_opt_ LPDWORD lpMaximumComponentLength,
    _Out_opt_ LPDWORD lpFileSystemFlags, _Out_opt_ LPWSTR lpFileSystemNameBuffer, _In_ DWORD nFileSystemNameSize);

typedef DWORD(WINAPI *MDBX_GetFinalPathNameByHandleW)(_In_ HANDLE hFile, _Out_ LPWSTR lpszFilePath,
                                                      _In_ DWORD cchFilePath, _In_ DWORD dwFlags);

typedef BOOL(WINAPI *MDBX_SetFileInformationByHandle)(_In_ HANDLE hFile,
                                                      _In_ FILE_INFO_BY_HANDLE_CLASS FileInformationClass,
                                                      _Out_ LPVOID lpFileInformation, _In_ DWORD dwBufferSize);

typedef NTSTATUS(NTAPI *MDBX_NtFsControlFile)(IN HANDLE FileHandle, IN OUT HANDLE Event,
                                              IN OUT PVOID /* PIO_APC_ROUTINE */ ApcRoutine, IN OUT PVOID ApcContext,
                                              OUT PIO_STATUS_BLOCK IoStatusBlock, IN ULONG FsControlCode,
                                              IN OUT PVOID InputBuffer, IN ULONG InputBufferLength,
                                              OUT OPTIONAL PVOID OutputBuffer, IN ULONG OutputBufferLength);

typedef uint64_t(WINAPI *MDBX_GetTickCount64)(void);

#if !defined(_WIN32_WINNT_WIN8) || _WIN32_WINNT < _WIN32_WINNT_WIN8
typedef struct _WIN32_MEMORY_RANGE_ENTRY {
  PVOID VirtualAddress;
  SIZE_T NumberOfBytes;
} WIN32_MEMORY_RANGE_ENTRY, *PWIN32_MEMORY_RANGE_ENTRY;
#endif /* Windows 8.x */

typedef BOOL(WINAPI *MDBX_PrefetchVirtualMemory)(HANDLE hProcess, ULONG_PTR NumberOfEntries,
                                                 PWIN32_MEMORY_RANGE_ENTRY VirtualAddresses, ULONG Flags);

typedef enum _SECTION_INHERIT { ViewShare = 1, ViewUnmap = 2 } SECTION_INHERIT;

typedef NTSTATUS(NTAPI *MDBX_NtExtendSection)(IN HANDLE SectionHandle, IN PLARGE_INTEGER NewSectionSize);

typedef LSTATUS(WINAPI *MDBX_RegGetValueA)(HKEY hkey, LPCSTR lpSubKey, LPCSTR lpValue, DWORD dwFlags, LPDWORD pdwType,
                                           PVOID pvData, LPDWORD pcbData);

typedef long(WINAPI *MDBX_CoCreateGuid)(bin128_t *guid);

NTSYSAPI ULONG RtlRandomEx(PULONG Seed);

typedef BOOL(WINAPI *MDBX_SetFileIoOverlappedRange)(HANDLE FileHandle, PUCHAR OverlappedRangeStart, ULONG Length);

struct libmdbx_imports {
  osal_srwlock_t_function srwl_Init;
  osal_srwlock_t_function srwl_AcquireShared;
  osal_srwlock_t_function srwl_ReleaseShared;
  osal_srwlock_t_function srwl_AcquireExclusive;
  osal_srwlock_t_function srwl_ReleaseExclusive;
  MDBX_NtExtendSection NtExtendSection;
  MDBX_GetFileInformationByHandleEx GetFileInformationByHandleEx;
  MDBX_GetVolumeInformationByHandleW GetVolumeInformationByHandleW;
  MDBX_GetFinalPathNameByHandleW GetFinalPathNameByHandleW;
  MDBX_SetFileInformationByHandle SetFileInformationByHandle;
  MDBX_NtFsControlFile NtFsControlFile;
  MDBX_PrefetchVirtualMemory PrefetchVirtualMemory;
  MDBX_GetTickCount64 GetTickCount64;
  MDBX_RegGetValueA RegGetValueA;
  MDBX_SetFileIoOverlappedRange SetFileIoOverlappedRange;
  MDBX_CoCreateGuid CoCreateGuid;
};

MDBX_INTERNAL void windows_import(void);
#endif /* Windows */

enum signatures {
  env_signature = INT32_C(0x1A899641),
  txn_signature = INT32_C(0x13D53A31),
  cur_signature_live = INT32_C(0x7E05D5B1),
  cur_signature_ready4dispose = INT32_C(0x2817A047),
  cur_signature_wait4eot = INT32_C(0x10E297A7)
};

/*----------------------------------------------------------------------------*/

/* An dirty-page list item is an pgno/pointer pair. */
struct dp {
  page_t *ptr;
  pgno_t pgno, npages;
};

enum dpl_rules {
  dpl_gap_edging = 2,
  dpl_gap_mergesort = 16,
  dpl_reserve_gap = dpl_gap_mergesort + dpl_gap_edging,
  dpl_insertion_threshold = 42
};

/* An DPL (dirty-page list) is a lazy-sorted array of MDBX_DPs. */
struct dpl {
  size_t sorted;
  size_t length;
  /* number of pages, but not an entries. */
  size_t pages_including_loose;
  /* allocated size excluding the dpl_reserve_gap */
  size_t detent;
  /* dynamic size with holes at zero and after the last */
  dp_t items[dpl_reserve_gap];
};

/*----------------------------------------------------------------------------*/
/* Internal structures */

/* Comparing/ordering and length constraints */
typedef struct clc {
  MDBX_cmp_func *cmp; /* comparator */
  size_t lmin, lmax;  /* min/max length constraints */
} clc_t;

/* Вспомогательная информация о table.
 *
 * Совокупность потребностей:
 * 1. Для транзакций и основного курсора нужны все поля.
 * 2. Для вложенного dupsort-курсора нужен компаратор значений, который изнутри
 *    курсора будет выглядеть как компаратор ключей. Плюс заглушка компаратора
 *    значений, которая не должна использоваться в штатных ситуациях, но
 *    требуется хотя-бы для отслеживания таких обращений.
 * 3. Использование компараторов для курсора и вложенного dupsort-курсора
 *    должно выглядеть одинаково.
 * 4. Желательно минимизировать объём данных размещаемых внутри вложенного
 *    dupsort-курсора.
 * 5. Желательно чтобы объем всей структуры был степенью двойки.
 *
 * Решение:
 *  - не храним в dupsort-курсоре ничего лишнего, а только tree;
 *  - в курсоры помещаем только указатель на clc_t, который будет указывать
 *    на соответствующее clc-поле в общей kvx-таблице привязанной к env;
 *  - компаратор размещаем в начале clc_t, в kvx_t сначала размещаем clc
 *    для ключей, потом для значений, а имя БД в конце kvx_t.
 *  - тогда в курсоре clc[0] будет содержать информацию для ключей,
 *    а clc[1] для значений, причем компаратор значений для dupsort-курсора
 *    будет попадать на MDBX_val с именем, что приведет к SIGSEGV при попытке
 *    использования такого компаратора.
 *  - размер kvx_t становится равным 8 словам.
 *
 * Трюки и прочая экономия на спичках:
 *  - не храним dbi внутри курсора, вместо этого вычисляем его как разницу между
 *    dbi_state курсора и началом таблицы dbi_state в транзакции. Смысл тут в
 *    экономии кол-ва полей при инициализации курсора. Затрат это не создает,
 *    так как dbi требуется для последующего доступа к массивам в транзакции,
 *    т.е. при вычислении dbi разыменовывается тот-же указатель на txn
 *    и читается та же кэш-линия с указателями. */
typedef struct clc2 {
  clc_t k; /* для ключей */
  clc_t v; /* для значений */
} clc2_t;

struct kvx {
  clc2_t clc;
  MDBX_val name; /* имя table */
};

/* Non-shared DBI state flags inside transaction */
enum dbi_state {
  DBI_DIRTY = 0x01 /* table was written in this txn */,
  DBI_STALE = 0x02 /* cached table record is outdated and should be reloaded/refreshed */,
  DBI_FRESH = 0x04 /* table handle opened in this txn */,
  DBI_CREAT = 0x08 /* table handle created in this txn */,
  DBI_VALID = 0x10 /* Handle is valid, see also DB_VALID */,
  DBI_OLDEN = 0x40 /* Handle was closed/reopened outside txn */,
  DBI_LINDO = 0x80 /* Lazy initialization done for DBI-slot */,
};

enum txn_flags {
  txn_ro_begin_flags = MDBX_TXN_RDONLY | MDBX_TXN_RDONLY_PREPARE,
  txn_rw_begin_flags = MDBX_TXN_NOMETASYNC | MDBX_TXN_NOSYNC | MDBX_TXN_TRY,
  txn_shrink_allowed = UINT32_C(0x40000000),
  txn_parked = MDBX_TXN_PARKED,
  txn_gc_drained = 0x80 /* GC was depleted up to oldest reader */,
  txn_may_have_cursors = 0x400,
  txn_state_flags = MDBX_TXN_FINISHED | MDBX_TXN_ERROR | MDBX_TXN_DIRTY | MDBX_TXN_SPILLS | MDBX_TXN_HAS_CHILD |
                    MDBX_TXN_INVALID | txn_gc_drained
};

/* A database transaction.
 * Every operation requires a transaction handle. */
struct MDBX_txn {
  int32_t signature;
  uint32_t flags; /* Transaction Flags */
  size_t n_dbi;
  size_t owner; /* thread ID that owns this transaction */

  MDBX_txn *parent; /* parent of a nested txn */
  MDBX_txn *nested; /* nested txn under this txn,
                       set together with MDBX_TXN_HAS_CHILD */
  geo_t geo;

  /* The ID of this transaction. IDs are integers incrementing from
   * INITIAL_TXNID. Only committed write transactions increment the ID. If a
   * transaction aborts, the ID may be re-used by the next writer. */
  txnid_t txnid, front_txnid;

  MDBX_env *env; /* the DB environment */
  tree_t *dbs;   /* Array of tree_t records for each known DB */

#if MDBX_ENABLE_DBI_SPARSE
  unsigned *__restrict dbi_sparse;
#endif /* MDBX_ENABLE_DBI_SPARSE */

  /* Array of non-shared txn's flags of DBI.
   * Модификатор __restrict тут полезен и безопасен в текущем понимании,
   * так как пересечение возможно только с dbi_state курсоров,
   * и происходит по-чтению до последующего изменения/записи. */
  uint8_t *__restrict dbi_state;

  /* Array of sequence numbers for each DB handle. */
  uint32_t *__restrict dbi_seqs;

  /* Массив с головами односвязных списков отслеживания курсоров. */
  MDBX_cursor **cursors;

  /* "Канареечные" маркеры/счетчики */
  MDBX_canary canary;

  /* User-settable context */
  void *userctx;

  union {
    struct {
      /* For read txns: This thread/txn's slot table slot, or nullptr. */
      reader_slot_t *slot;
    } ro;
    struct {
      troika_t troika;
      pnl_t __restrict repnl; /* Reclaimed GC pages */
      struct {
        rkl_t reclaimed;   /* The list of reclaimed txn-ids from GC, but not cleared/deleted */
        rkl_t ready4reuse; /* The list of reclaimed txn-ids from GC, and cleared/deleted */
        uint64_t spent;    /* Time spent reading and searching GC */
        rkl_t comeback;    /* The list of ids of records returned into GC during commit, etc */
      } gc;
      bool prefault_write_activated;
#if MDBX_ENABLE_REFUND
      pgno_t loose_refund_wl /* FIXME: describe */;
#endif /* MDBX_ENABLE_REFUND */
      /* a sequence to spilling dirty page with LRU policy */
      unsigned dirtylru;
      /* dirtylist room: Dirty array size - dirty pages visible to this txn.
       * Includes ancestor txns' dirty pages not hidden by other txns'
       * dirty/spilled pages. Thus commit(nested txn) has room to merge
       * dirtylist into parent after freeing hidden parent pages. */
      size_t dirtyroom;
      /* For write txns: Modified pages. Sorted when not MDBX_WRITEMAP. */
      dpl_t *__restrict dirtylist;
      /* The list of pages that became unused during this transaction. */
      pnl_t __restrict retired_pages;
      /* The list of loose pages that became unused and may be reused
       * in this transaction, linked through `page_next()`. */
      page_t *__restrict loose_pages;
      /* Number of loose pages (wr.loose_pages) */
      size_t loose_count;
      union {
        struct {
          size_t least_removed;
          /* The sorted list of dirty pages we temporarily wrote to disk
           * because the dirty list was full. page numbers in here are
           * shifted left by 1, deleted slots have the LSB set. */
          pnl_t __restrict list;
        } spilled;
        size_t writemap_dirty_npages;
        size_t writemap_spilled_npages;
      };
      /* In write txns, next is located the array of cursors for each DB */
    } wr;
  };
};

#define CURSOR_STACK_SIZE (16 + MDBX_WORDBITS / 4)

struct MDBX_cursor {
  int32_t signature;
  union {
    /* Тут некоторые трюки/заморочки с тем чтобы во всех основных сценариях
     * проверять состояние курсора одной простой операцией сравнения,
     * и при этом ни на каплю не усложнять код итерации стека курсора.
     *
     * Поэтому решение такое:
     *  - поля flags и top сделаны знаковыми, а их отрицательные значения
     *    используются для обозначения не-установленного/не-инициализированного
     *    состояния курсора;
     *  - для инвалидации/сброса курсора достаточно записать отрицательное
     *    значение в объединенное поле top_and_flags;
     *  - все проверки состояния сводятся к сравнению одного из полей
     *    flags/snum/snum_and_flags, которые в зависимости от сценария,
     *    трактуются либо как знаковые, либо как безнаковые. */
    __anonymous_struct_extension__ struct {
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
      int8_t flags;
      /* индекс вершины стека, меньше нуля для не-инициализированного курсора */
      int8_t top;
#else
      int8_t top;
      int8_t flags;
#endif
    };
    int16_t top_and_flags;
  };
  /* флаги проверки, в том числе биты для проверки типа листовых страниц. */
  uint8_t checking;
  uint8_t pad;

  /* Указывает на txn->dbi_state[] для DBI этого курсора.
   * Модификатор __restrict тут полезен и безопасен в текущем понимании,
   * так как пересечение возможно только с dbi_state транзакции,
   * и происходит по-чтению до последующего изменения/записи. */
  uint8_t *__restrict dbi_state;
  /* Связь списка отслеживания курсоров в транзакции. */
  MDBX_txn *txn;
  /* Указывает на tree->dbs[] для DBI этого курсора. */
  tree_t *tree;
  /* Указывает на env->kvs[] для DBI этого курсора. */
  clc2_t *clc;
  subcur_t *__restrict subcur;
  page_t *pg[CURSOR_STACK_SIZE]; /* stack of pushed pages */
  indx_t ki[CURSOR_STACK_SIZE];  /* stack of page indices */
  MDBX_cursor *next;
  /* Состояние на момент старта вложенной транзакции */
  MDBX_cursor *backup;
};

struct inner_cursor {
  MDBX_cursor cursor;
  tree_t nested_tree;
};

struct cursor_couple {
  MDBX_cursor outer;
  void *userctx; /* User-settable context */
  subcur_t inner;
};

enum env_flags {
  /* Failed to update the meta page. Probably an I/O error. */
  ENV_FATAL_ERROR = INT32_MIN /* 0x80000000 */,
  /* Some fields are initialized. */
  ENV_ACTIVE = UINT32_C(0x20000000),
  /* me_txkey is set */
  ENV_TXKEY = UINT32_C(0x10000000),
  /* Legacy MDBX_MAPASYNC (prior v0.9) */
  DEPRECATED_MAPASYNC = UINT32_C(0x100000),
  /* Legacy MDBX_COALESCE (prior v0.12) */
  DEPRECATED_COALESCE = UINT32_C(0x2000000),
  ENV_INTERNAL_FLAGS = ENV_FATAL_ERROR | ENV_ACTIVE | ENV_TXKEY,
  /* Only a subset of the mdbx_env flags can be changed
   * at runtime. Changing other flags requires closing the
   * environment and re-opening it with the new flags. */
  ENV_CHANGEABLE_FLAGS = MDBX_SAFE_NOSYNC | MDBX_NOMETASYNC | DEPRECATED_MAPASYNC | MDBX_NOMEMINIT |
                         DEPRECATED_COALESCE | MDBX_PAGEPERTURB | MDBX_ACCEDE | MDBX_VALIDATION,
  ENV_CHANGELESS_FLAGS = MDBX_NOSUBDIR | MDBX_RDONLY | MDBX_WRITEMAP | MDBX_NOSTICKYTHREADS | MDBX_NORDAHEAD |
                         MDBX_LIFORECLAIM | MDBX_EXCLUSIVE,
  ENV_USABLE_FLAGS = ENV_CHANGEABLE_FLAGS | ENV_CHANGELESS_FLAGS
};

/* The database environment. */
struct MDBX_env {
  /* ----------------------------------------------------- mostly static part */
  mdbx_atomic_uint32_t signature;
  uint32_t flags;
  unsigned ps;          /* DB page size, initialized from me_os_psize */
  osal_mmap_t dxb_mmap; /* The main data file */
#define lazy_fd dxb_mmap.fd
  mdbx_filehandle_t dsync_fd, fd4meta;
#if defined(_WIN32) || defined(_WIN64)
  HANDLE dxb_lock_event;
  HANDLE lck_lock_event;
#endif                  /* Windows */
  osal_mmap_t lck_mmap; /* The lock file */
  lck_t *lck;

  uint16_t leaf_nodemax;   /* max size of a leaf-node */
  uint16_t branch_nodemax; /* max size of a branch-node */
  uint16_t subpage_limit;
  uint16_t subpage_room_threshold;
  uint16_t subpage_reserve_prereq;
  uint16_t subpage_reserve_limit;
  atomic_pgno_t mlocked_pgno;
  uint8_t ps2ln;              /* log2 of DB page size */
  int8_t stuck_meta;          /* recovery-only: target meta page or less that zero */
  uint16_t merge_threshold;   /* pages emptier than this are candidates for merging */
  unsigned max_readers;       /* size of the reader table */
  MDBX_dbi max_dbi;           /* size of the DB table */
  uint32_t pid;               /* process ID of this env */
  osal_thread_key_t me_txkey; /* thread-key for readers */
  struct {                    /* path to the DB files */
    pathchar_t *lck, *dxb, *specified;
    void *buffer;
  } pathname;
  void *page_auxbuf;              /* scratch area for DUPSORT put() */
  MDBX_txn *basal_txn;            /* preallocated write transaction */
  kvx_t *kvs;                     /* array of auxiliary key-value properties */
  uint8_t *__restrict dbs_flags;  /* array of flags from tree_t.flags */
  mdbx_atomic_uint32_t *dbi_seqs; /* array of dbi sequence numbers */
  unsigned maxgc_large1page;      /* Number of pgno_t fit in a single large page */
  unsigned maxgc_per_branch;
  uint32_t registered_reader_pid; /* have liveness lock in reader table */
  void *userctx;                  /* User-settable context */
  MDBX_hsr_func *hsr_callback;    /* Callback for kicking laggard readers */
  size_t madv_threshold;

  struct {
    unsigned dp_reserve_limit;
    unsigned rp_augment_limit;
    unsigned dp_limit;
    unsigned dp_initial;
    uint64_t gc_time_limit;
    uint8_t dp_loose_limit;
    uint8_t spill_max_denominator;
    uint8_t spill_min_denominator;
    uint8_t spill_parent4child_denominator;
    unsigned merge_threshold_16dot16_percent;
#if !(defined(_WIN32) || defined(_WIN64))
    unsigned writethrough_threshold;
#endif /* Windows */
    bool prefault_write;
    bool prefer_waf_insteadof_balance; /* Strive to minimize WAF instead of
                                          balancing pages fullment */
    bool need_dp_limit_adjust;
    struct {
      uint16_t limit;
      uint16_t room_threshold;
      uint16_t reserve_prereq;
      uint16_t reserve_limit;
    } subpage;

    union {
      unsigned all;
      /* tracks options with non-auto values but tuned by user */
      struct {
        unsigned dp_limit : 1;
        unsigned rp_augment_limit : 1;
        unsigned prefault_write : 1;
      } non_auto;
    } flags;
  } options;

  /* struct geo_in_bytes used for accepting db-geo params from user for the new
   * database creation, i.e. when mdbx_env_set_geometry() was called before
   * mdbx_env_open(). */
  struct {
    size_t lower;  /* minimal size of datafile */
    size_t upper;  /* maximal size of datafile */
    size_t now;    /* current size of datafile */
    size_t grow;   /* step to grow datafile */
    size_t shrink; /* threshold to shrink datafile */
  } geo_in_bytes;

#if MDBX_LOCKING == MDBX_LOCKING_SYSV
  union {
    key_t key;
    int semid;
  } me_sysv_ipc;
#endif /* MDBX_LOCKING == MDBX_LOCKING_SYSV */
  bool incore;

#if MDBX_ENABLE_DBI_LOCKFREE
  defer_free_item_t *defer_free;
#endif /* MDBX_ENABLE_DBI_LOCKFREE */

  /* -------------------------------------------------------------- debugging */

#if MDBX_DEBUG
  MDBX_assert_func *assert_func; /*  Callback for assertion failures */
#endif
#ifdef ENABLE_MEMCHECK
  int valgrind_handle;
#endif
#if defined(ENABLE_MEMCHECK) || defined(__SANITIZE_ADDRESS__)
  pgno_t poison_edge;
#endif /* ENABLE_MEMCHECK || __SANITIZE_ADDRESS__ */

#ifndef xMDBX_DEBUG_SPILLING
#define xMDBX_DEBUG_SPILLING 0
#endif
#if xMDBX_DEBUG_SPILLING == 2
  size_t debug_dirtied_est, debug_dirtied_act;
#endif /* xMDBX_DEBUG_SPILLING */

  /* --------------------------------------------------- mostly volatile part */

  MDBX_txn *txn; /* current write transaction */
  struct {
    txnid_t detent;
  } gc;
  osal_fastmutex_t dbi_lock;
  unsigned n_dbi; /* number of DBs opened */

  unsigned shadow_reserve_len;
  page_t *__restrict shadow_reserve; /* list of malloc'ed blocks for re-use */

  osal_ioring_t ioring;

#if defined(_WIN32) || defined(_WIN64)
  osal_srwlock_t remap_guard;
  /* Workaround for LockFileEx and WriteFile multithread bug */
  CRITICAL_SECTION lck_event_cs;
  CRITICAL_SECTION dxb_event_cs;
  char *pathname_char; /* cache of multi-byte representation of pathname
                             to the DB files */
#else
  osal_fastmutex_t remap_guard;
#endif

  /* ------------------------------------------------- stub for lck-less mode */
  mdbx_atomic_uint64_t lckless_placeholder[(sizeof(lck_t) + MDBX_CACHELINE_SIZE - 1) / sizeof(mdbx_atomic_uint64_t)];
};

/*----------------------------------------------------------------------------*/

/* pseudo-error code, not exposed outside libmdbx */
#define MDBX_NO_ROOT (MDBX_LAST_ADDED_ERRCODE + 33)

/* Number of slots in the reader table.
 * This value was chosen somewhat arbitrarily. The 61 is a prime number,
 * and such readers plus a couple mutexes fit into single 4KB page.
 * Applications should set the table size using mdbx_env_set_maxreaders(). */
#define DEFAULT_READERS 61

enum db_flags {
  DB_PERSISTENT_FLAGS =
      MDBX_REVERSEKEY | MDBX_DUPSORT | MDBX_INTEGERKEY | MDBX_DUPFIXED | MDBX_INTEGERDUP | MDBX_REVERSEDUP,

  /* mdbx_dbi_open() flags */
  DB_USABLE_FLAGS = DB_PERSISTENT_FLAGS | MDBX_CREATE | MDBX_DB_ACCEDE,

  DB_VALID = 0x80u /* DB handle is valid, for dbs_flags */,
  DB_POISON = 0x7fu /* update pending */,
  DB_INTERNAL_FLAGS = DB_VALID
};

#if !defined(__cplusplus) || CONSTEXPR_ENUM_FLAGS_OPERATIONS
MDBX_MAYBE_UNUSED static void static_checks(void) {
  STATIC_ASSERT(MDBX_WORDBITS == sizeof(void *) * CHAR_BIT);
  STATIC_ASSERT(UINT64_C(0x80000000) == (uint32_t)ENV_FATAL_ERROR);
  STATIC_ASSERT_MSG(INT16_MAX - CORE_DBS == MDBX_MAX_DBI, "Oops, MDBX_MAX_DBI or CORE_DBS?");
  STATIC_ASSERT_MSG((unsigned)(MDBX_DB_ACCEDE | MDBX_CREATE) ==
                        ((DB_USABLE_FLAGS | DB_INTERNAL_FLAGS) & (ENV_USABLE_FLAGS | ENV_INTERNAL_FLAGS)),
                    "Oops, some flags overlapped or wrong");
  STATIC_ASSERT_MSG((DB_INTERNAL_FLAGS & DB_USABLE_FLAGS) == 0, "Oops, some flags overlapped or wrong");
  STATIC_ASSERT_MSG((DB_PERSISTENT_FLAGS & ~DB_USABLE_FLAGS) == 0, "Oops, some flags overlapped or wrong");
  STATIC_ASSERT(DB_PERSISTENT_FLAGS <= UINT8_MAX);
  STATIC_ASSERT_MSG((ENV_INTERNAL_FLAGS & ENV_USABLE_FLAGS) == 0, "Oops, some flags overlapped or wrong");

  STATIC_ASSERT_MSG((txn_state_flags & (txn_rw_begin_flags | txn_ro_begin_flags)) == 0,
                    "Oops, some txn flags overlapped or wrong");
  STATIC_ASSERT_MSG(((txn_rw_begin_flags | txn_ro_begin_flags | txn_state_flags) & txn_shrink_allowed) == 0,
                    "Oops, some txn flags overlapped or wrong");

  STATIC_ASSERT(sizeof(reader_slot_t) == 32);
#if MDBX_LOCKING > 0
  STATIC_ASSERT(offsetof(lck_t, wrt_lock) % MDBX_CACHELINE_SIZE == 0);
  STATIC_ASSERT(offsetof(lck_t, rdt_lock) % MDBX_CACHELINE_SIZE == 0);
#else
  STATIC_ASSERT(offsetof(lck_t, cached_oldest) % MDBX_CACHELINE_SIZE == 0);
  STATIC_ASSERT(offsetof(lck_t, rdt_length) % MDBX_CACHELINE_SIZE == 0);
#endif /* MDBX_LOCKING */
#if FLEXIBLE_ARRAY_MEMBERS
  STATIC_ASSERT(offsetof(lck_t, rdt) % MDBX_CACHELINE_SIZE == 0);
#endif /* FLEXIBLE_ARRAY_MEMBERS */

#if FLEXIBLE_ARRAY_MEMBERS
  STATIC_ASSERT(NODESIZE == offsetof(node_t, payload));
  STATIC_ASSERT(PAGEHDRSZ == offsetof(page_t, entries));
#endif /* FLEXIBLE_ARRAY_MEMBERS */
  STATIC_ASSERT(sizeof(clc_t) == 3 * sizeof(void *));
  STATIC_ASSERT(sizeof(kvx_t) == 8 * sizeof(void *));

#define KVX_SIZE_LN2 MDBX_WORDBITS_LN2
  STATIC_ASSERT(sizeof(kvx_t) == (1u << KVX_SIZE_LN2));
}
#endif /* Disabled for MSVC 19.0 (VisualStudio 2015) */

/******************************************************************************/

#ifndef __cplusplus

/* valid flags for mdbx_node_add() */
#define NODE_ADD_FLAGS (N_DUP | N_TREE | MDBX_RESERVE | MDBX_APPEND)

/* Get the page number pointed to by a branch node */
MDBX_NOTHROW_PURE_FUNCTION static inline pgno_t node_pgno(const node_t *const __restrict node) {
  pgno_t pgno = UNALIGNED_PEEK_32(node, node_t, child_pgno);
  return pgno;
}

/* Set the page number in a branch node */
static inline void node_set_pgno(node_t *const __restrict node, pgno_t pgno) {
  assert(pgno >= MIN_PAGENO && pgno <= MAX_PAGENO);
  UNALIGNED_POKE_32(node, node_t, child_pgno, (uint32_t)pgno);
}

/* Get the size of the data in a leaf node */
MDBX_NOTHROW_PURE_FUNCTION static inline size_t node_ds(const node_t *const __restrict node) {
  return UNALIGNED_PEEK_32(node, node_t, dsize);
}

/* Set the size of the data for a leaf node */
static inline void node_set_ds(node_t *const __restrict node, size_t size) {
  assert(size < INT_MAX);
  UNALIGNED_POKE_32(node, node_t, dsize, (uint32_t)size);
}

/* The size of a key in a node */
MDBX_NOTHROW_PURE_FUNCTION static inline size_t node_ks(const node_t *const __restrict node) {
  return UNALIGNED_PEEK_16(node, node_t, ksize);
}

/* Set the size of the key for a leaf node */
static inline void node_set_ks(node_t *const __restrict node, size_t size) {
  assert(size < INT16_MAX);
  UNALIGNED_POKE_16(node, node_t, ksize, (uint16_t)size);
}

MDBX_NOTHROW_PURE_FUNCTION static inline uint8_t node_flags(const node_t *const __restrict node) {
  return UNALIGNED_PEEK_8(node, node_t, flags);
}

static inline void node_set_flags(node_t *const __restrict node, uint8_t flags) {
  UNALIGNED_POKE_8(node, node_t, flags, flags);
}

/* Address of the key for the node */
MDBX_NOTHROW_PURE_FUNCTION static inline void *node_key(const node_t *const __restrict node) {
  return ptr_disp(node, NODESIZE);
}

/* Address of the data for a node */
MDBX_NOTHROW_PURE_FUNCTION static inline void *node_data(const node_t *const __restrict node) {
  return ptr_disp(node_key(node), node_ks(node));
}

/* Size of a node in a leaf page with a given key and data.
 * This is node header plus key plus data size. */
MDBX_NOTHROW_CONST_FUNCTION static inline size_t node_size_len(const size_t key_len, const size_t value_len) {
  return NODESIZE + EVEN_CEIL(key_len + value_len);
}
MDBX_NOTHROW_PURE_FUNCTION static inline size_t node_size(const MDBX_val *key, const MDBX_val *value) {
  return node_size_len(key ? key->iov_len : 0, value ? value->iov_len : 0);
}

MDBX_NOTHROW_PURE_FUNCTION static inline pgno_t node_largedata_pgno(const node_t *const __restrict node) {
  assert(node_flags(node) & N_BIG);
  return peek_pgno(node_data(node));
}

MDBX_INTERNAL int __must_check_result node_read_bigdata(MDBX_cursor *mc, const node_t *node, MDBX_val *data,
                                                        const page_t *mp);

static inline int __must_check_result node_read(MDBX_cursor *mc, const node_t *node, MDBX_val *data, const page_t *mp) {
  data->iov_len = node_ds(node);
  data->iov_base = node_data(node);
  if (likely(node_flags(node) != N_BIG))
    return MDBX_SUCCESS;
  return node_read_bigdata(mc, node, data, mp);
}

/*----------------------------------------------------------------------------*/

MDBX_INTERNAL nsr_t node_search(MDBX_cursor *mc, const MDBX_val *key);

MDBX_INTERNAL int __must_check_result node_add_branch(MDBX_cursor *mc, size_t indx, const MDBX_val *key, pgno_t pgno);

MDBX_INTERNAL int __must_check_result node_add_leaf(MDBX_cursor *mc, size_t indx, const MDBX_val *key, MDBX_val *data,
                                                    unsigned flags);

MDBX_INTERNAL int __must_check_result node_add_dupfix(MDBX_cursor *mc, size_t indx, const MDBX_val *key);

MDBX_INTERNAL void node_del(MDBX_cursor *mc, size_t ksize);

MDBX_INTERNAL node_t *node_shrink(page_t *mp, size_t indx, node_t *node);

#if MDBX_ENABLE_DBI_SPARSE

MDBX_NOTHROW_CONST_FUNCTION MDBX_MAYBE_UNUSED MDBX_INTERNAL size_t dbi_bitmap_ctz_fallback(const MDBX_txn *txn,
                                                                                           intptr_t bmi);

static inline size_t dbi_bitmap_ctz(const MDBX_txn *txn, intptr_t bmi) {
  tASSERT(txn, bmi != 0);
  STATIC_ASSERT(sizeof(bmi) >= sizeof(txn->dbi_sparse[0]));
#if __GNUC_PREREQ(4, 1) || __has_builtin(__builtin_ctzl)
  if (sizeof(txn->dbi_sparse[0]) <= sizeof(int))
    return __builtin_ctz((int)bmi);
  if (sizeof(txn->dbi_sparse[0]) == sizeof(long))
    return __builtin_ctzl((long)bmi);
#if (defined(__SIZEOF_LONG_LONG__) && __SIZEOF_LONG_LONG__ == 8) || __has_builtin(__builtin_ctzll)
  return __builtin_ctzll(bmi);
#endif /* have(long long) && long long == uint64_t */
#endif /* GNU C */

#if defined(_MSC_VER)
  unsigned long index;
  if (sizeof(txn->dbi_sparse[0]) > 4) {
#if defined(_M_AMD64) || defined(_M_ARM64) || defined(_M_X64)
    _BitScanForward64(&index, bmi);
    return index;
#else
    if (bmi > UINT32_MAX) {
      _BitScanForward(&index, (uint32_t)((uint64_t)bmi >> 32));
      return index;
    }
#endif
  }
  _BitScanForward(&index, (uint32_t)bmi);
  return index;
#endif /* MSVC */

  return dbi_bitmap_ctz_fallback(txn, bmi);
}

static inline bool dbi_foreach_step(const MDBX_txn *const txn, size_t *bitmap_item, size_t *dbi) {
  const size_t bitmap_chunk = CHAR_BIT * sizeof(txn->dbi_sparse[0]);
  if (*bitmap_item & 1) {
    *bitmap_item >>= 1;
    return txn->dbi_state[*dbi] != 0;
  }
  if (*bitmap_item) {
    size_t bitmap_skip = dbi_bitmap_ctz(txn, *bitmap_item);
    *bitmap_item >>= bitmap_skip;
    *dbi += bitmap_skip - 1;
  } else {
    *dbi = (*dbi - 1) | (bitmap_chunk - 1);
    *bitmap_item = txn->dbi_sparse[(1 + *dbi) / bitmap_chunk];
    if (*bitmap_item == 0)
      *dbi += bitmap_chunk;
  }
  return false;
}

/* LY: Макрос целенаправленно сделан с одним циклом, чтобы сохранить возможность
 * использования оператора break */
#define TXN_FOREACH_DBI_FROM(TXN, I, FROM)                                                                             \
  for (size_t bitmap_item = TXN->dbi_sparse[0] >> FROM, I = FROM; I < TXN->n_dbi; ++I)                                 \
    if (dbi_foreach_step(TXN, &bitmap_item, &I))

#else

#define TXN_FOREACH_DBI_FROM(TXN, I, FROM)                                                                             \
  for (size_t I = FROM; I < TXN->n_dbi; ++I)                                                                           \
    if (TXN->dbi_state[I])

#endif /* MDBX_ENABLE_DBI_SPARSE */

#define TXN_FOREACH_DBI_ALL(TXN, I) TXN_FOREACH_DBI_FROM(TXN, I, 0)
#define TXN_FOREACH_DBI_USER(TXN, I) TXN_FOREACH_DBI_FROM(TXN, I, CORE_DBS)

MDBX_INTERNAL int dbi_import(MDBX_txn *txn, const size_t dbi);
MDBX_INTERNAL int dbi_gone(MDBX_txn *txn, const size_t dbi, const int rc);

struct dbi_snap_result {
  uint32_t sequence;
  unsigned flags;
};
MDBX_INTERNAL struct dbi_snap_result dbi_snap(const MDBX_env *env, const size_t dbi);

MDBX_INTERNAL int dbi_update(MDBX_txn *txn, bool keep);

static inline uint8_t dbi_state(const MDBX_txn *txn, const size_t dbi) {
  STATIC_ASSERT((int)DBI_DIRTY == MDBX_DBI_DIRTY && (int)DBI_STALE == MDBX_DBI_STALE &&
                (int)DBI_FRESH == MDBX_DBI_FRESH && (int)DBI_CREAT == MDBX_DBI_CREAT);

#if MDBX_ENABLE_DBI_SPARSE
  const size_t bitmap_chunk = CHAR_BIT * sizeof(txn->dbi_sparse[0]);
  const size_t bitmap_indx = dbi / bitmap_chunk;
  const size_t bitmap_mask = (size_t)1 << dbi % bitmap_chunk;
  return likely(dbi < txn->n_dbi && (txn->dbi_sparse[bitmap_indx] & bitmap_mask) != 0) ? txn->dbi_state[dbi] : 0;
#else
  return likely(dbi < txn->n_dbi) ? txn->dbi_state[dbi] : 0;
#endif /* MDBX_ENABLE_DBI_SPARSE */
}

static inline bool dbi_changed(const MDBX_txn *txn, const size_t dbi) {
  const MDBX_env *const env = txn->env;
  eASSERT(env, dbi_state(txn, dbi) & DBI_LINDO);
  const uint32_t snap_seq = atomic_load32(&env->dbi_seqs[dbi], mo_AcquireRelease);
  return unlikely(snap_seq != txn->dbi_seqs[dbi]);
}

static inline int dbi_check(const MDBX_txn *txn, const size_t dbi) {
  const uint8_t state = dbi_state(txn, dbi);
  if (likely((state & DBI_LINDO) != 0 && !dbi_changed(txn, dbi)))
    return (state & DBI_VALID) ? MDBX_SUCCESS : MDBX_BAD_DBI;

  /* Медленный путь: ленивая до-инициализацяи и импорт */
  return dbi_import((MDBX_txn *)txn, dbi);
}

static inline uint32_t dbi_seq_next(const MDBX_env *const env, size_t dbi) {
  uint32_t v = atomic_load32(&env->dbi_seqs[dbi], mo_AcquireRelease) + 1;
  return v ? v : 1;
}

MDBX_INTERNAL int dbi_open(MDBX_txn *txn, const MDBX_val *const name, unsigned user_flags, MDBX_dbi *dbi,
                           MDBX_cmp_func *keycmp, MDBX_cmp_func *datacmp);

MDBX_INTERNAL int dbi_bind(MDBX_txn *txn, const size_t dbi, unsigned user_flags, MDBX_cmp_func *keycmp,
                           MDBX_cmp_func *datacmp);

typedef struct defer_free_item {
  struct defer_free_item *next;
  uint64_t timestamp;
} defer_free_item_t;

MDBX_INTERNAL int dbi_defer_release(MDBX_env *const env, defer_free_item_t *const chain);
MDBX_INTERNAL int dbi_close_release(MDBX_env *env, MDBX_dbi dbi);
MDBX_INTERNAL const tree_t *dbi_dig(const MDBX_txn *txn, const size_t dbi, tree_t *fallback);

struct dbi_rename_result {
  defer_free_item_t *defer;
  int err;
};

MDBX_INTERNAL struct dbi_rename_result dbi_rename_locked(MDBX_txn *txn, MDBX_dbi dbi, MDBX_val new_name);

MDBX_NOTHROW_CONST_FUNCTION MDBX_INTERNAL pgno_t pv2pages(uint16_t pv);

MDBX_NOTHROW_CONST_FUNCTION MDBX_INTERNAL uint16_t pages2pv(size_t pages);

MDBX_MAYBE_UNUSED MDBX_INTERNAL bool pv2pages_verify(void);

/*------------------------------------------------------------------------------
 * Nodes, Keys & Values length limitation factors:
 *
 * BRANCH_NODE_MAX
 *   Branch-page must contain at least two nodes, within each a key and a child
 *   page number. But page can't be split if it contains less that 4 keys,
 *   i.e. a page should not overflow before adding the fourth key. Therefore,
 *   at least 3 branch-node should fit in the single branch-page. Further, the
 *   first node of a branch-page doesn't contain a key, i.e. the first node
 *   is always require space just for itself. Thus:
 *       PAGESPACE = pagesize - page_hdr_len;
 *       BRANCH_NODE_MAX = even_floor(
 *         (PAGESPACE - sizeof(indx_t) - NODESIZE) / (3 - 1) - sizeof(indx_t));
 *       KEYLEN_MAX = BRANCH_NODE_MAX - node_hdr_len;
 *
 * LEAF_NODE_MAX
 *   Leaf-node must fit into single leaf-page, where a value could be placed on
 *   a large/overflow page. However, may require to insert a nearly page-sized
 *   node between two large nodes are already fill-up a page. In this case the
 *   page must be split to two if some pair of nodes fits on one page, or
 *   otherwise the page should be split to the THREE with a single node
 *   per each of ones. Such 1-into-3 page splitting is costly and complex since
 *   requires TWO insertion into the parent page, that could lead to split it
 *   and so on up to the root. Therefore double-splitting is avoided here and
 *   the maximum node size is half of a leaf page space:
 *       LEAF_NODE_MAX = even_floor(PAGESPACE / 2 - sizeof(indx_t));
 *       DATALEN_NO_OVERFLOW = LEAF_NODE_MAX - NODESIZE - KEYLEN_MAX;
 *
 *  - Table-node must fit into one leaf-page:
 *       TABLE_NAME_MAX = LEAF_NODE_MAX - node_hdr_len - sizeof(tree_t);
 *
 *  - Dupsort values itself are a keys in a dupsort-table and couldn't be longer
 *    than the KEYLEN_MAX. But dupsort node must not great than LEAF_NODE_MAX,
 *    since dupsort value couldn't be placed on a large/overflow page:
 *       DUPSORT_DATALEN_MAX = min(KEYLEN_MAX,
 *                                 max(DATALEN_NO_OVERFLOW, sizeof(tree_t));
 */

#define PAGESPACE(pagesize) ((pagesize) - PAGEHDRSZ)

#define BRANCH_NODE_MAX(pagesize)                                                                                      \
  (EVEN_FLOOR((PAGESPACE(pagesize) - sizeof(indx_t) - NODESIZE) / (3 - 1) - sizeof(indx_t)))

#define LEAF_NODE_MAX(pagesize) (EVEN_FLOOR(PAGESPACE(pagesize) / 2) - sizeof(indx_t))

#define MAX_GC1OVPAGE(pagesize) (PAGESPACE(pagesize) / sizeof(pgno_t) - 1)

MDBX_NOTHROW_CONST_FUNCTION static inline size_t keysize_max(size_t pagesize, MDBX_db_flags_t flags) {
  assert(pagesize >= MDBX_MIN_PAGESIZE && pagesize <= MDBX_MAX_PAGESIZE && is_powerof2(pagesize));
  STATIC_ASSERT(BRANCH_NODE_MAX(MDBX_MIN_PAGESIZE) - NODESIZE >= 8);
  if (flags & MDBX_INTEGERKEY)
    return 8 /* sizeof(uint64_t) */;

  const intptr_t max_branch_key = BRANCH_NODE_MAX(pagesize) - NODESIZE;
  STATIC_ASSERT(LEAF_NODE_MAX(MDBX_MIN_PAGESIZE) - NODESIZE -
                    /* sizeof(uint64) as a key */ 8 >
                sizeof(tree_t));
  if (flags & (MDBX_DUPSORT | MDBX_DUPFIXED | MDBX_REVERSEDUP | MDBX_INTEGERDUP)) {
    const intptr_t max_dupsort_leaf_key = LEAF_NODE_MAX(pagesize) - NODESIZE - sizeof(tree_t);
    return (max_branch_key < max_dupsort_leaf_key) ? max_branch_key : max_dupsort_leaf_key;
  }
  return max_branch_key;
}

MDBX_NOTHROW_CONST_FUNCTION static inline size_t env_keysize_max(const MDBX_env *env, MDBX_db_flags_t flags) {
  size_t size_max;
  if (flags & MDBX_INTEGERKEY)
    size_max = 8 /* sizeof(uint64_t) */;
  else {
    const intptr_t max_branch_key = env->branch_nodemax - NODESIZE;
    STATIC_ASSERT(LEAF_NODE_MAX(MDBX_MIN_PAGESIZE) - NODESIZE -
                      /* sizeof(uint64) as a key */ 8 >
                  sizeof(tree_t));
    if (flags & (MDBX_DUPSORT | MDBX_DUPFIXED | MDBX_REVERSEDUP | MDBX_INTEGERDUP)) {
      const intptr_t max_dupsort_leaf_key = env->leaf_nodemax - NODESIZE - sizeof(tree_t);
      size_max = (max_branch_key < max_dupsort_leaf_key) ? max_branch_key : max_dupsort_leaf_key;
    } else
      size_max = max_branch_key;
  }
  eASSERT(env, size_max == keysize_max(env->ps, flags));
  return size_max;
}

MDBX_NOTHROW_CONST_FUNCTION static inline size_t keysize_min(MDBX_db_flags_t flags) {
  return (flags & MDBX_INTEGERKEY) ? 4 /* sizeof(uint32_t) */ : 0;
}

MDBX_NOTHROW_CONST_FUNCTION static inline size_t valsize_min(MDBX_db_flags_t flags) {
  if (flags & MDBX_INTEGERDUP)
    return 4 /* sizeof(uint32_t) */;
  else if (flags & MDBX_DUPFIXED)
    return sizeof(indx_t);
  else
    return 0;
}

MDBX_NOTHROW_CONST_FUNCTION static inline size_t valsize_max(size_t pagesize, MDBX_db_flags_t flags) {
  assert(pagesize >= MDBX_MIN_PAGESIZE && pagesize <= MDBX_MAX_PAGESIZE && is_powerof2(pagesize));

  if (flags & MDBX_INTEGERDUP)
    return 8 /* sizeof(uint64_t) */;

  if (flags & (MDBX_DUPSORT | MDBX_DUPFIXED | MDBX_REVERSEDUP))
    return keysize_max(pagesize, 0);

  const unsigned page_ln2 = log2n_powerof2(pagesize);
  const size_t hard = 0x7FF00000ul;
  const size_t hard_pages = hard >> page_ln2;
  STATIC_ASSERT(PAGELIST_LIMIT <= MAX_PAGENO);
  const size_t pages_limit = PAGELIST_LIMIT / 4;
  const size_t limit = (hard_pages < pages_limit) ? hard : (pages_limit << page_ln2);
  return (limit < MAX_MAPSIZE / 2) ? limit : MAX_MAPSIZE / 2;
}

MDBX_NOTHROW_CONST_FUNCTION static inline size_t env_valsize_max(const MDBX_env *env, MDBX_db_flags_t flags) {
  size_t size_max;
  if (flags & MDBX_INTEGERDUP)
    size_max = 8 /* sizeof(uint64_t) */;
  else if (flags & (MDBX_DUPSORT | MDBX_DUPFIXED | MDBX_REVERSEDUP))
    size_max = env_keysize_max(env, 0);
  else {
    const size_t hard = 0x7FF00000ul;
    const size_t hard_pages = hard >> env->ps2ln;
    STATIC_ASSERT(PAGELIST_LIMIT <= MAX_PAGENO);
    const size_t pages_limit = PAGELIST_LIMIT / 4;
    const size_t limit = (hard_pages < pages_limit) ? hard : (pages_limit << env->ps2ln);
    size_max = (limit < MAX_MAPSIZE / 2) ? limit : MAX_MAPSIZE / 2;
  }
  eASSERT(env, size_max == valsize_max(env->ps, flags));
  return size_max;
}

/*----------------------------------------------------------------------------*/

MDBX_NOTHROW_PURE_FUNCTION static inline size_t leaf_size(const MDBX_env *env, const MDBX_val *key,
                                                          const MDBX_val *data) {
  size_t node_bytes = node_size(key, data);
  if (node_bytes > env->leaf_nodemax)
    /* put on large/overflow page */
    node_bytes = node_size_len(key->iov_len, 0) + sizeof(pgno_t);

  return node_bytes + sizeof(indx_t);
}

MDBX_NOTHROW_PURE_FUNCTION static inline size_t branch_size(const MDBX_env *env, const MDBX_val *key) {
  /* Size of a node in a branch page with a given key.
   * This is just the node header plus the key, there is no data. */
  size_t node_bytes = node_size(key, nullptr);
  if (unlikely(node_bytes > env->branch_nodemax)) {
    /* put on large/overflow page, not implemented */
    mdbx_panic("node_size(key) %zu > %u branch_nodemax", node_bytes, env->branch_nodemax);
    node_bytes = node_size(key, nullptr) + sizeof(pgno_t);
  }

  return node_bytes + sizeof(indx_t);
}

MDBX_NOTHROW_CONST_FUNCTION static inline uint16_t flags_db2sub(uint16_t db_flags) {
  uint16_t sub_flags = db_flags & MDBX_DUPFIXED;

  /* MDBX_INTEGERDUP => MDBX_INTEGERKEY */
#define SHIFT_INTEGERDUP_TO_INTEGERKEY 2
  STATIC_ASSERT((MDBX_INTEGERDUP >> SHIFT_INTEGERDUP_TO_INTEGERKEY) == MDBX_INTEGERKEY);
  sub_flags |= (db_flags & MDBX_INTEGERDUP) >> SHIFT_INTEGERDUP_TO_INTEGERKEY;

  /* MDBX_REVERSEDUP => MDBX_REVERSEKEY */
#define SHIFT_REVERSEDUP_TO_REVERSEKEY 5
  STATIC_ASSERT((MDBX_REVERSEDUP >> SHIFT_REVERSEDUP_TO_REVERSEKEY) == MDBX_REVERSEKEY);
  sub_flags |= (db_flags & MDBX_REVERSEDUP) >> SHIFT_REVERSEDUP_TO_REVERSEKEY;

  return sub_flags;
}

static inline bool check_table_flags(unsigned flags) {
  switch (flags & ~(MDBX_REVERSEKEY | MDBX_INTEGERKEY)) {
  default:
    NOTICE("invalid db-flags 0x%x", flags);
    return false;
  case MDBX_DUPSORT:
  case MDBX_DUPSORT | MDBX_REVERSEDUP:
  case MDBX_DUPSORT | MDBX_DUPFIXED:
  case MDBX_DUPSORT | MDBX_DUPFIXED | MDBX_REVERSEDUP:
  case MDBX_DUPSORT | MDBX_DUPFIXED | MDBX_INTEGERDUP:
  case MDBX_DUPSORT | MDBX_DUPFIXED | MDBX_INTEGERDUP | MDBX_REVERSEDUP:
  case MDBX_DB_DEFAULTS:
    return (flags & (MDBX_REVERSEKEY | MDBX_INTEGERKEY)) != (MDBX_REVERSEKEY | MDBX_INTEGERKEY);
  }
}

MDBX_MAYBE_UNUSED static inline int tbl_setup_ifneed(const MDBX_env *env, volatile kvx_t *const kvx,
                                                     const tree_t *const db) {
  return likely(kvx->clc.v.lmax) ? MDBX_SUCCESS : tbl_setup(env, kvx, db);
}

MDBX_MAYBE_UNUSED static inline int tbl_refresh_absent2baddbi(MDBX_txn *txn, size_t dbi) {
  int rc = tbl_refresh(txn, dbi);
  return likely(rc != MDBX_NOTFOUND) ? rc : MDBX_BAD_DBI;
}

/*----------------------------------------------------------------------------*/

MDBX_NOTHROW_PURE_FUNCTION static inline size_t pgno2bytes(const MDBX_env *env, size_t pgno) {
  eASSERT(env, (1u << env->ps2ln) == env->ps);
  return pgno << env->ps2ln;
}

MDBX_NOTHROW_PURE_FUNCTION static inline page_t *pgno2page(const MDBX_env *env, size_t pgno) {
  return ptr_disp(env->dxb_mmap.base, pgno2bytes(env, pgno));
}

MDBX_NOTHROW_PURE_FUNCTION static inline pgno_t bytes2pgno(const MDBX_env *env, size_t bytes) {
  eASSERT(env, (env->ps >> env->ps2ln) == 1);
  return (pgno_t)(bytes >> env->ps2ln);
}

/* align to system page size */
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t bytes_ceil2sp_bytes(const MDBX_env *env, size_t bytes);
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t bytes_ceil2sp_pgno(const MDBX_env *env, size_t bytes);
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t pgno_ceil2sp_bytes(const MDBX_env *env, size_t pgno);
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL pgno_t pgno_ceil2sp_pgno(const MDBX_env *env, size_t pgno);

/* align to system allocation granularity */
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t bytes_ceil2ag_bytes(const MDBX_env *env, size_t bytes);
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t bytes_ceil2ag_pgno(const MDBX_env *env, size_t bytes);
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL size_t pgno_ceil2ag_bytes(const MDBX_env *env, size_t pgno);
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL pgno_t pgno_ceil2ag_pgno(const MDBX_env *env, size_t pgno);

MDBX_NOTHROW_PURE_FUNCTION static inline pgno_t largechunk_npages(const MDBX_env *env, size_t bytes) {
  return bytes2pgno(env, PAGEHDRSZ - 1 + bytes) + 1;
}

typedef struct alignkey {
  MDBX_val key;
  uint64_t intbuf;
} alignkey_t;

static inline int check_key(const MDBX_cursor *mc, const MDBX_val *key, alignkey_t *alignkey) {
  /* coverity[logical_vs_bitwise] */
  if (unlikely(key->iov_len < mc->clc->k.lmin ||
               (key->iov_len > mc->clc->k.lmax &&
                (mc->clc->k.lmin == mc->clc->k.lmax || MDBX_DEBUG || MDBX_FORCE_ASSERTIONS)))) {
    cASSERT(mc, !"Invalid key-size");
    return MDBX_BAD_VALSIZE;
  }

  alignkey->key = *key;
  if (mc->tree->flags & MDBX_INTEGERKEY) {
    if (alignkey->key.iov_len == 8) {
      if (unlikely(7 & (uintptr_t)alignkey->key.iov_base))
        /* copy instead of return error to avoid break compatibility */
        alignkey->key.iov_base = bcopy_8(&alignkey->intbuf, alignkey->key.iov_base);
    } else if (alignkey->key.iov_len == 4) {
      if (unlikely(3 & (uintptr_t)alignkey->key.iov_base))
        /* copy instead of return error to avoid break compatibility */
        alignkey->key.iov_base = bcopy_4(&alignkey->intbuf, alignkey->key.iov_base);
    } else {
      cASSERT(mc, !"key-size is invalid for MDBX_INTEGERKEY");
      return MDBX_BAD_VALSIZE;
    }
  }
  return MDBX_SUCCESS;
}

MDBX_NOTHROW_PURE_FUNCTION static inline MDBX_val get_key(const node_t *node) {
  MDBX_val key;
  key.iov_len = node_ks(node);
  key.iov_base = node_key(node);
  return key;
}

static inline void get_key_optional(const node_t *node, MDBX_val *keyptr /* __may_null */) {
  if (keyptr)
    *keyptr = get_key(node);
}

MDBX_NOTHROW_PURE_FUNCTION static inline void *page2payload(const page_t *mp) { return ptr_disp(mp, PAGEHDRSZ); }

MDBX_NOTHROW_PURE_FUNCTION static inline const page_t *payload2page(const void *data) {
  return container_of(data, page_t, entries);
}

MDBX_NOTHROW_PURE_FUNCTION MDBX_MAYBE_UNUSED static inline const page_t *ptr2page(const MDBX_env *env,
                                                                                  const void *ptr) {
  eASSERT(env,
          ptr_dist(ptr, env->dxb_mmap.base) >= 0 && (size_t)ptr_dist(ptr, env->dxb_mmap.base) < env->dxb_mmap.limit);
  const uintptr_t mask = env->ps - 1;
  return (page_t *)((uintptr_t)ptr & ~mask);
}

MDBX_NOTHROW_PURE_FUNCTION static inline meta_t *page_meta(page_t *mp) { return (meta_t *)page2payload(mp); }

MDBX_NOTHROW_PURE_FUNCTION static inline size_t page_numkeys(const page_t *mp) {
  assert(mp->lower <= mp->upper);
  return mp->lower >> 1;
}

MDBX_NOTHROW_PURE_FUNCTION static inline size_t page_room(const page_t *mp) {
  assert(mp->lower <= mp->upper);
  return mp->upper - mp->lower;
}

MDBX_NOTHROW_PURE_FUNCTION static inline size_t page_space(const MDBX_env *env) {
  STATIC_ASSERT(PAGEHDRSZ % 2 == 0);
  return env->ps - PAGEHDRSZ;
}

MDBX_NOTHROW_PURE_FUNCTION static inline size_t page_used(const MDBX_env *env, const page_t *mp) {
  return page_space(env) - page_room(mp);
}

/* The percentage of space used in the page, in a percents. */
MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline unsigned page_fill_percentum_x10(const MDBX_env *env,
                                                                                            const page_t *mp) {
  const size_t space = page_space(env);
  return (unsigned)((page_used(env, mp) * 1000 + space / 2) / space);
}

MDBX_NOTHROW_PURE_FUNCTION static inline node_t *page_node(const page_t *mp, size_t i) {
  assert(page_type_compat(mp) == P_LEAF || page_type(mp) == P_BRANCH);
  assert(page_numkeys(mp) > i);
  assert(mp->entries[i] % 2 == 0);
  return ptr_disp(mp, mp->entries[i] + PAGEHDRSZ);
}

MDBX_NOTHROW_PURE_FUNCTION static inline void *page_dupfix_ptr(const page_t *mp, size_t i, size_t keysize) {
  assert(page_type_compat(mp) == (P_LEAF | P_DUPFIX) && i == (indx_t)i && mp->dupfix_ksize == keysize);
  (void)keysize;
  return ptr_disp(mp, PAGEHDRSZ + mp->dupfix_ksize * (indx_t)i);
}

MDBX_NOTHROW_PURE_FUNCTION static inline MDBX_val page_dupfix_key(const page_t *mp, size_t i, size_t keysize) {
  MDBX_val r;
  r.iov_base = page_dupfix_ptr(mp, i, keysize);
  r.iov_len = mp->dupfix_ksize;
  return r;
}

/*----------------------------------------------------------------------------*/

MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL int cmp_int_unaligned(const MDBX_val *a, const MDBX_val *b);

#if MDBX_UNALIGNED_OK < 2 || (MDBX_DEBUG || MDBX_FORCE_ASSERTIONS || !defined(NDEBUG))
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL int
/* Compare two items pointing at 2-byte aligned unsigned int's. */
cmp_int_align2(const MDBX_val *a, const MDBX_val *b);
#else
#define cmp_int_align2 cmp_int_unaligned
#endif /* !MDBX_UNALIGNED_OK || debug */

#if MDBX_UNALIGNED_OK < 4 || (MDBX_DEBUG || MDBX_FORCE_ASSERTIONS || !defined(NDEBUG))
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL int
/* Compare two items pointing at 4-byte aligned unsigned int's. */
cmp_int_align4(const MDBX_val *a, const MDBX_val *b);
#else
#define cmp_int_align4 cmp_int_unaligned
#endif /* !MDBX_UNALIGNED_OK || debug */

/* Compare two items lexically */
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL int cmp_lexical(const MDBX_val *a, const MDBX_val *b);

/* Compare two items in reverse byte order */
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL int cmp_reverse(const MDBX_val *a, const MDBX_val *b);

/* Fast non-lexically comparator */
MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL int cmp_lenfast(const MDBX_val *a, const MDBX_val *b);

MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL bool eq_fast_slowpath(const uint8_t *a, const uint8_t *b, size_t l);

MDBX_NOTHROW_PURE_FUNCTION static inline bool eq_fast(const MDBX_val *a, const MDBX_val *b) {
  return unlikely(a->iov_len == b->iov_len) && eq_fast_slowpath(a->iov_base, b->iov_base, a->iov_len);
}

MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL int cmp_equal_or_greater(const MDBX_val *a, const MDBX_val *b);

MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL int cmp_equal_or_wrong(const MDBX_val *a, const MDBX_val *b);

static inline MDBX_cmp_func *builtin_keycmp(MDBX_db_flags_t flags) {
  return (flags & MDBX_REVERSEKEY) ? cmp_reverse : (flags & MDBX_INTEGERKEY) ? cmp_int_align2 : cmp_lexical;
}

static inline MDBX_cmp_func *builtin_datacmp(MDBX_db_flags_t flags) {
  return !(flags & MDBX_DUPSORT)
             ? cmp_lenfast
             : ((flags & MDBX_INTEGERDUP) ? cmp_int_unaligned
                                          : ((flags & MDBX_REVERSEDUP) ? cmp_reverse : cmp_lexical));
}

/*----------------------------------------------------------------------------*/

MDBX_INTERNAL uint32_t combine_durability_flags(const uint32_t a, const uint32_t b);

MDBX_CONST_FUNCTION static inline lck_t *lckless_stub(const MDBX_env *env) {
  uintptr_t stub = (uintptr_t)&env->lckless_placeholder;
  /* align to avoid false-positive alarm from UndefinedBehaviorSanitizer */
  stub = (stub + MDBX_CACHELINE_SIZE - 1) & ~(MDBX_CACHELINE_SIZE - 1);
  return (lck_t *)stub;
}

#if !(defined(_WIN32) || defined(_WIN64))
MDBX_CONST_FUNCTION static inline int ignore_enosys(int err) {
#ifdef ENOSYS
  if (err == ENOSYS)
    return MDBX_RESULT_TRUE;
#endif /* ENOSYS */
#ifdef ENOIMPL
  if (err == ENOIMPL)
    return MDBX_RESULT_TRUE;
#endif /* ENOIMPL */
#ifdef ENOTSUP
  if (err == ENOTSUP)
    return MDBX_RESULT_TRUE;
#endif /* ENOTSUP */
#ifdef ENOSUPP
  if (err == ENOSUPP)
    return MDBX_RESULT_TRUE;
#endif /* ENOSUPP */
#ifdef EOPNOTSUPP
  if (err == EOPNOTSUPP)
    return MDBX_RESULT_TRUE;
#endif /* EOPNOTSUPP */
  return err;
}

MDBX_MAYBE_UNUSED MDBX_CONST_FUNCTION static inline int ignore_enosys_and_eagain(int err) {
  return (err == EAGAIN) ? MDBX_RESULT_TRUE : ignore_enosys(err);
}

MDBX_MAYBE_UNUSED MDBX_CONST_FUNCTION static inline int ignore_enosys_and_einval(int err) {
  return (err == EINVAL) ? MDBX_RESULT_TRUE : ignore_enosys(err);
}

MDBX_MAYBE_UNUSED MDBX_CONST_FUNCTION static inline int ignore_enosys_and_eremote(int err) {
  return (err == MDBX_EREMOTE) ? MDBX_RESULT_TRUE : ignore_enosys(err);
}

#endif /* defined(_WIN32) || defined(_WIN64) */

static inline int check_env(const MDBX_env *env, const bool wanna_active) {
  if (unlikely(!env))
    return MDBX_EINVAL;

  if (unlikely(env->signature.weak != env_signature))
    return MDBX_EBADSIGN;

  if (unlikely(env->flags & ENV_FATAL_ERROR))
    return MDBX_PANIC;

  if (wanna_active) {
#if MDBX_ENV_CHECKPID
    if (unlikely(env->pid != osal_getpid()) && env->pid) {
      ((MDBX_env *)env)->flags |= ENV_FATAL_ERROR;
      return MDBX_PANIC;
    }
#endif /* MDBX_ENV_CHECKPID */
    if (unlikely((env->flags & ENV_ACTIVE) == 0))
      return MDBX_EPERM;
    eASSERT(env, env->dxb_mmap.base != nullptr);
  }

  return MDBX_SUCCESS;
}

static __always_inline int check_txn_anythread(const MDBX_txn *txn, int bad_bits) {
  if (unlikely(!txn))
    return MDBX_EINVAL;

  if (unlikely(txn->signature != txn_signature))
    return MDBX_EBADSIGN;

  if (bad_bits) {
    if (unlikely(!txn->env->dxb_mmap.base))
      return MDBX_EPERM;

    if (unlikely(txn->flags & bad_bits)) {
      if ((bad_bits & MDBX_TXN_RDONLY) && unlikely(txn->flags & MDBX_TXN_RDONLY))
        return MDBX_EACCESS;
      if ((bad_bits & MDBX_TXN_PARKED) == 0)
        return MDBX_BAD_TXN;
      return txn_check_badbits_parked(txn, bad_bits);
    }
  }

  tASSERT(txn, (txn->flags & MDBX_TXN_FINISHED) ||
                   (txn->flags & MDBX_NOSTICKYTHREADS) == (txn->env->flags & MDBX_NOSTICKYTHREADS));
  return MDBX_SUCCESS;
}

static __always_inline int check_txn(const MDBX_txn *txn, int bad_bits) {
  int err = check_txn_anythread(txn, bad_bits);
#if MDBX_TXN_CHECKOWNER
  if (err == MDBX_SUCCESS && (txn->flags & (MDBX_NOSTICKYTHREADS | MDBX_TXN_FINISHED)) != MDBX_NOSTICKYTHREADS &&
      !(bad_bits /* abort/reset/txn-break */ == 0 &&
        ((txn->flags & (MDBX_TXN_RDONLY | MDBX_TXN_FINISHED)) == (MDBX_TXN_RDONLY | MDBX_TXN_FINISHED))) &&
      unlikely(txn->owner != osal_thread_self()))
    err = txn->owner ? MDBX_THREAD_MISMATCH : MDBX_BAD_TXN;
#endif /* MDBX_TXN_CHECKOWNER */

  return err;
}

static inline int check_txn_rw(const MDBX_txn *txn, int bad_bits) {
  return check_txn(txn, (bad_bits | MDBX_TXN_RDONLY) & ~MDBX_TXN_PARKED);
}

MDBX_NOTHROW_CONST_FUNCTION static inline txnid_t txn_basis_snapshot(const MDBX_txn *txn) {
  STATIC_ASSERT(((MDBX_TXN_RDONLY >> ((xMDBX_TXNID_STEP == 2) ? 16 : 17)) & xMDBX_TXNID_STEP) == xMDBX_TXNID_STEP);
  const txnid_t committed_txnid =
      txn->txnid - xMDBX_TXNID_STEP + ((txn->flags >> ((xMDBX_TXNID_STEP == 2) ? 16 : 17)) & xMDBX_TXNID_STEP);
  tASSERT(txn, committed_txnid == ((txn->flags & MDBX_TXN_RDONLY) ? txn->txnid : txn->txnid - xMDBX_TXNID_STEP));
  return committed_txnid;
}

/*----------------------------------------------------------------------------*/

MDBX_INTERNAL void mincore_clean_cache(const MDBX_env *const env);

MDBX_INTERNAL void update_mlcnt(const MDBX_env *env, const pgno_t new_aligned_mlocked_pgno,
                                const bool lock_not_release);

MDBX_INTERNAL void munlock_after(const MDBX_env *env, const pgno_t aligned_pgno, const size_t end_bytes);

MDBX_INTERNAL void munlock_all(const MDBX_env *env);

/*----------------------------------------------------------------------------*/
/* Cache coherence and mmap invalidation */
#ifndef MDBX_CPU_WRITEBACK_INCOHERENT
#error "The MDBX_CPU_WRITEBACK_INCOHERENT must be defined before"
#elif MDBX_CPU_WRITEBACK_INCOHERENT
#define osal_flush_incoherent_cpu_writeback() osal_memory_barrier()
#else
#define osal_flush_incoherent_cpu_writeback() osal_compiler_barrier()
#endif /* MDBX_CPU_WRITEBACK_INCOHERENT */

MDBX_MAYBE_UNUSED static inline void osal_flush_incoherent_mmap(const void *addr, size_t nbytes,
                                                                const intptr_t pagesize) {
#ifndef MDBX_MMAP_INCOHERENT_FILE_WRITE
#error "The MDBX_MMAP_INCOHERENT_FILE_WRITE must be defined before"
#elif MDBX_MMAP_INCOHERENT_FILE_WRITE
  char *const begin = (char *)(-pagesize & (intptr_t)addr);
  char *const end = (char *)(-pagesize & (intptr_t)((char *)addr + nbytes + pagesize - 1));
  int err = msync(begin, end - begin, MS_SYNC | MS_INVALIDATE) ? errno : 0;
  eASSERT(nullptr, err == 0);
  (void)err;
#else
  (void)pagesize;
#endif /* MDBX_MMAP_INCOHERENT_FILE_WRITE */

#ifndef MDBX_MMAP_INCOHERENT_CPU_CACHE
#error "The MDBX_MMAP_INCOHERENT_CPU_CACHE must be defined before"
#elif MDBX_MMAP_INCOHERENT_CPU_CACHE
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

/* Состояние курсора.
 *
 * плохой/poor:
 *  - неустановленный курсор с незаполненым стеком;
 *  - следует пропускать во всех циклах отслеживания/корректировки
 *    позиций курсоров;
 *  - допускаются только операции предполагающие установку абсолютной позиции;
 *  - в остальных случаях возвращается ENODATA.
 *
 *    У таких курсоров top = -1 и flags < 0, что позволяет дешево проверять и
 *    пропускать такие курсоры в циклах отслеживания/корректировки по условию
 *    probe_cursor->top < this_cursor->top.
 *
 * пустой/hollow:
 *  - частично инициализированный курсор, но без доступной пользователю позиции,
 *    поэтому нельзя выполнить какую-либо операцию без абсолютного (не
 *    относительного) позиционирования;
 *  - ki[top] может быть некорректным, в том числе >= page_numkeys(pg[top]).
 *
 *    У таких курсоров top >= 0, но flags < 0 (есть флажок z_hollow).
 *
 * установленный/pointed:
 *  - полностью инициализированный курсор с конкретной позицией с данными;
 *  - можно прочитать текущую строку, удалить её, либо выполнить
 *    относительное перемещение;
 *  - может иметь флажки z_after_delete, z_eof_hard и z_eof_soft;
 *  - наличие z_eof_soft означает что курсор перемещен за пределы данных,
 *    поэтому нелья прочитать текущие данные, либо удалить их.
 *
 *    У таких курсоров top >= 0 и flags >= 0 (нет флажка z_hollow).
 *
 * наполненный данными/filled:
 *  - это установленный/pointed курсор без флагов z_eof_soft;
 *  - за курсором есть даные, возможны CRUD операции в текущей позиции.
 *
 *    У таких курсоров top >= 0 и (unsigned)flags < z_eof_soft.
 *
 * Изменения состояния.
 *
 *  - Сбрасывается состояние курсора посредством top_and_flags |= z_poor_mark,
 *    что равносильно top = -1 вместе с flags |= z_poor_mark;
 *  - При позиционировании курсора сначала устанавливается top, а flags
 *    только в самом конце при отсутстви ошибок.
 *  - Повторное позиционирование first/last может начинаться
 *    с установки/обнуления только top без сброса flags, что позволяет работать
 *    быстрому пути внутри tree_search_finalize().
 *
 *  - Заморочки с концом данных:
 *     - mdbx_cursor_get(NEXT) выполняет две операции (перемещение и чтение),
 *       поэтому перемещение на последнюю строку строку всегда успешно,
 *       а ошибка возвращается только при последующем next().
 *       Однако, из-за этой двойственности семантика ситуации возврата ошибки
 *       из mdbx_cursor_get(NEXT) допускает разночтение/неопределенность, ибо
 *       не понятно к чему относится ошибка:
 *        - Если к чтению данных, то курсор перемещен и стоит после последней
 *          строки. Соответственно, чтение в текущей позиции запрещено,
 *          а при выполнении prev() курсор вернется на последнюю строку;
 *        - Если же ошибка относится к перемещению, то курсор не перемещен и
 *          остается на последней строке. Соответственно, чтение в текущей
 *          позиции допустимо, а при выполнении prev() курсор встанет
 *          на пред-последнюю строку.
 *        - Пикантность в том, что пользователи (так или иначе) полагаются
 *          на оба варианта поведения, при этом конечно ожидают что после
 *          ошибки MDBX_NEXT функция mdbx_cursor_eof() будет возвращать true.
 *     - далее добавляется схожая ситуация с MDBX_GET_RANGE, MDBX_LOWERBOUND,
 *       MDBX_GET_BOTH_RANGE и MDBX_UPPERBOUND. Тут при неуспехе поиска курсор
 *       может/должен стоять после последней строки.
 *     - далее добавляется MDBX_LAST. Тут курсор должен стоять на последней
 *       строке и допускать чтение в текузщей позиции,
 *       но mdbx_cursor_eof() должен возвращать true.
 *
 *    Решение = делаем два флажка z_eof_soft и z_eof_hard:
 *     - Когда установлен только z_eof_soft,
 *       функция mdbx_cursor_eof() возвращает true, но допускается
 *       чтение данных в текущей позиции, а prev() передвигает курсор
 *       на пред-последнюю строку.
 *     - Когда установлен z_eof_hard, чтение данных в текущей позиции
 *       не допускается, и mdbx_cursor_eof() также возвращает true,
 *       а prev() устанавливает курсора на последюю строку. */
enum cursor_state {
  /* Это вложенный курсор для вложенного дерева/страницы и является
     inner-элементом struct cursor_couple. */
  z_inner = 0x01,

  /* Происходит подготовка к обновлению GC,
     поэтому можно брать страницы из GC даже для FREE_DBI. */
  z_gcu_preparation = 0x02,

  /* Курсор только-что создан, поэтому допускается авто-установка
     в начало/конец, вместо возврата ошибки. */
  z_fresh = 0x04,

  /* Предыдущей операцией было удаление, поэтому курсор уже физически указывает
     на следующий элемент и соответствующая операция перемещения должна
     игнорироваться. */
  z_after_delete = 0x08,

  /* */
  z_disable_tree_search_fastpath = 0x10,

  /* Курсор логически в конце данных, но физически на последней строке,
   * ki[top] == page_numkeys(pg[top]) - 1 и читать данные в текущей позиции. */
  z_eof_soft = 0x20,

  /* Курсор логически за концом данных, поэтому следующий переход "назад"
     должен игнорироваться и/или приводить к установке на последнюю строку.
     В текущем же состоянии нельзя делать CRUD операции. */
  z_eof_hard = 0x40,

  /* За курсором нет данных, логически его позиция не определена,
     нельзя делать CRUD операции в текущей позиции.
     Относительное перемещение запрещено. */
  z_hollow = -128 /* 0x80 */,

  /* Маски для сброса/установки состояния. */
  z_clear_mask = z_inner | z_gcu_preparation,
  z_poor_mark = z_eof_hard | z_hollow | z_disable_tree_search_fastpath,
  z_fresh_mark = z_poor_mark | z_fresh
};

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_inner(const MDBX_cursor *mc) {
  return (mc->flags & z_inner) != 0;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_poor(const MDBX_cursor *mc) {
  const bool r = mc->top < 0;
  cASSERT(mc, r == (mc->top_and_flags < 0));
  if (r && mc->subcur)
    cASSERT(mc, mc->subcur->cursor.flags < 0 && mc->subcur->cursor.top < 0);
  return r;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_pointed(const MDBX_cursor *mc) {
  const bool r = mc->top >= 0;
  cASSERT(mc, r == (mc->top_and_flags >= 0));
  if (!r && mc->subcur)
    cASSERT(mc, is_poor(&mc->subcur->cursor));
  return r;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_hollow(const MDBX_cursor *mc) {
  const bool r = mc->flags < 0;
  if (!r) {
    cASSERT(mc, mc->top >= 0);
    cASSERT(mc, (mc->flags & z_eof_hard) || mc->ki[mc->top] < page_numkeys(mc->pg[mc->top]));
  } else if (mc->subcur)
    cASSERT(mc, is_poor(&mc->subcur->cursor) || (is_pointed(mc) && mc->subcur->cursor.flags < 0));
  return r;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_eof(const MDBX_cursor *mc) {
  const bool r = z_eof_soft <= (uint8_t)mc->flags;
  return r;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_filled(const MDBX_cursor *mc) {
  const bool r = z_eof_hard > (uint8_t)mc->flags;
  return r;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool inner_filled(const MDBX_cursor *mc) {
  return mc->subcur && is_filled(&mc->subcur->cursor);
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool inner_pointed(const MDBX_cursor *mc) {
  return mc->subcur && is_pointed(&mc->subcur->cursor);
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool inner_hollow(const MDBX_cursor *mc) {
  const bool r = !mc->subcur || is_hollow(&mc->subcur->cursor);
#if MDBX_DEBUG || MDBX_FORCE_ASSERTIONS
  if (!r) {
    cASSERT(mc, is_filled(mc));
    const page_t *mp = mc->pg[mc->top];
    const node_t *node = page_node(mp, mc->ki[mc->top]);
    cASSERT(mc, node_flags(node) & N_DUP);
  }
#endif /* MDBX_DEBUG || MDBX_FORCE_ASSERTIONS */
  return r;
}

MDBX_MAYBE_UNUSED static inline void inner_gone(MDBX_cursor *mc) {
  if (mc->subcur) {
    TRACE("reset inner cursor %p", __Wpedantic_format_voidptr(&mc->subcur->cursor));
    mc->subcur->nested_tree.root = 0;
    mc->subcur->cursor.top_and_flags = z_inner | z_poor_mark;
  }
}

MDBX_MAYBE_UNUSED static inline void be_poor(MDBX_cursor *mc) {
  const bool inner = is_inner(mc);
  if (inner) {
    mc->tree->root = 0;
    mc->top_and_flags = z_inner | z_poor_mark;
  } else {
    mc->top_and_flags |= z_poor_mark;
    inner_gone(mc);
  }
  cASSERT(mc, is_poor(mc) && !is_pointed(mc) && !is_filled(mc));
  cASSERT(mc, inner == is_inner(mc));
}

MDBX_MAYBE_UNUSED static inline void be_filled(MDBX_cursor *mc) {
  cASSERT(mc, mc->top >= 0);
  cASSERT(mc, mc->ki[mc->top] < page_numkeys(mc->pg[mc->top]));
  const bool inner = is_inner(mc);
  mc->flags &= z_clear_mask;
  cASSERT(mc, is_filled(mc));
  cASSERT(mc, inner == is_inner(mc));
}

MDBX_MAYBE_UNUSED static inline bool is_related(const MDBX_cursor *base, const MDBX_cursor *scan) {
  cASSERT(base, base->top >= 0);
  return base->top <= scan->top && base != scan;
}

/* Флаги контроля/проверки курсора. */
enum cursor_checking {
  z_branch = 0x01 /* same as P_BRANCH for check_leaf_type() */,
  z_leaf = 0x02 /* same as P_LEAF for check_leaf_type() */,
  z_largepage = 0x04 /* same as P_LARGE for check_leaf_type() */,
  z_updating = 0x08 /* update/rebalance pending */,
  z_ignord = 0x10 /* don't check keys ordering */,
  z_dupfix = 0x20 /* same as P_DUPFIX for check_leaf_type() */,
  z_retiring = 0x40 /* refs to child pages may be invalid */,
  z_pagecheck = 0x80 /* perform page checking, see MDBX_VALIDATION */
};

MDBX_INTERNAL int __must_check_result cursor_validate(const MDBX_cursor *mc);

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline size_t cursor_dbi(const MDBX_cursor *mc) {
  cASSERT(mc, mc->txn->signature == txn_signature);
  size_t dbi = mc->dbi_state - mc->txn->dbi_state;
  cASSERT(mc, dbi < mc->txn->env->n_dbi);
  return dbi;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool cursor_dbi_changed(const MDBX_cursor *mc) {
  return dbi_changed(mc->txn, cursor_dbi(mc));
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline uint8_t *cursor_dbi_state(const MDBX_cursor *mc) {
  return mc->dbi_state;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool cursor_is_gc(const MDBX_cursor *mc) {
  return mc->dbi_state == mc->txn->dbi_state + FREE_DBI;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool cursor_is_main(const MDBX_cursor *mc) {
  return mc->dbi_state == mc->txn->dbi_state + MAIN_DBI;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool cursor_is_core(const MDBX_cursor *mc) {
  return mc->dbi_state < mc->txn->dbi_state + CORE_DBS;
}

MDBX_MAYBE_UNUSED static inline int cursor_dbi_dbg(const MDBX_cursor *mc) {
  /* Debugging output value of a cursor's DBI: Negative for a sub-cursor. */
  const int dbi = cursor_dbi(mc);
  return (mc->flags & z_inner) ? -dbi : dbi;
}

MDBX_MAYBE_UNUSED static inline int __must_check_result cursor_push(MDBX_cursor *mc, page_t *mp, indx_t ki) {
  TRACE("pushing page %" PRIaPGNO " on db %d cursor %p", mp->pgno, cursor_dbi_dbg(mc), __Wpedantic_format_voidptr(mc));
  if (unlikely(mc->top >= CURSOR_STACK_SIZE - 1)) {
    be_poor(mc);
    mc->txn->flags |= MDBX_TXN_ERROR;
    return MDBX_CURSOR_FULL;
  }
  mc->top += 1;
  mc->pg[mc->top] = mp;
  mc->ki[mc->top] = ki;
  return MDBX_SUCCESS;
}

MDBX_MAYBE_UNUSED static inline void cursor_pop(MDBX_cursor *mc) {
  TRACE("popped page %" PRIaPGNO " off db %d cursor %p", mc->pg[mc->top]->pgno, cursor_dbi_dbg(mc),
        __Wpedantic_format_voidptr(mc));
  cASSERT(mc, mc->top >= 0);
  mc->top -= 1;
}

MDBX_NOTHROW_PURE_FUNCTION static inline bool check_leaf_type(const MDBX_cursor *mc, const page_t *mp) {
  return (((page_type(mp) ^ mc->checking) & (z_branch | z_leaf | z_largepage | z_dupfix)) == 0);
}

MDBX_INTERNAL int cursor_check(const MDBX_cursor *mc, int txn_bad_bits);

/* без необходимости доступа к данным, без активации припаркованных транзакций. */
static inline int cursor_check_pure(const MDBX_cursor *mc) {
  return cursor_check(mc, MDBX_TXN_BLOCKED - MDBX_TXN_PARKED);
}

/* для чтения данных, с активацией припаркованных транзакций. */
static inline int cursor_check_ro(const MDBX_cursor *mc) { return cursor_check(mc, MDBX_TXN_BLOCKED); }

/* для записи данных. */
static inline int cursor_check_rw(const MDBX_cursor *mc) {
  return cursor_check(mc, (MDBX_TXN_BLOCKED - MDBX_TXN_PARKED) | MDBX_TXN_RDONLY);
}

MDBX_INTERNAL MDBX_cursor *cursor_eot(MDBX_cursor *cursor, MDBX_txn *txn);
MDBX_INTERNAL int cursor_shadow(MDBX_cursor *cursor, MDBX_txn *nested, const size_t dbi);

MDBX_INTERNAL MDBX_cursor *cursor_cpstk(const MDBX_cursor *csrc, MDBX_cursor *cdst);

MDBX_INTERNAL int __must_check_result cursor_ops(MDBX_cursor *mc, MDBX_val *key, MDBX_val *data,
                                                 const MDBX_cursor_op op);

MDBX_INTERNAL int __must_check_result cursor_check_multiple(MDBX_cursor *mc, const MDBX_val *key, MDBX_val *data,
                                                            unsigned flags);

MDBX_INTERNAL int __must_check_result cursor_put_checklen(MDBX_cursor *mc, const MDBX_val *key, MDBX_val *data,
                                                          unsigned flags);

MDBX_INTERNAL int __must_check_result cursor_put(MDBX_cursor *mc, const MDBX_val *key, MDBX_val *data, unsigned flags);

MDBX_INTERNAL int __must_check_result cursor_validate_updating(MDBX_cursor *mc);

MDBX_INTERNAL int __must_check_result cursor_del(MDBX_cursor *mc, unsigned flags);

MDBX_INTERNAL int __must_check_result cursor_sibling_left(MDBX_cursor *mc);
MDBX_INTERNAL int __must_check_result cursor_sibling_right(MDBX_cursor *mc);

typedef struct cursor_set_result {
  int err;
  bool exact;
} csr_t;

MDBX_INTERNAL csr_t cursor_seek(MDBX_cursor *mc, MDBX_val *key, MDBX_val *data, MDBX_cursor_op op);

MDBX_INTERNAL int __must_check_result inner_first(MDBX_cursor *__restrict mc, MDBX_val *__restrict data);
MDBX_INTERNAL int __must_check_result inner_last(MDBX_cursor *__restrict mc, MDBX_val *__restrict data);
MDBX_INTERNAL int __must_check_result outer_first(MDBX_cursor *__restrict mc, MDBX_val *__restrict key,
                                                  MDBX_val *__restrict data);
MDBX_INTERNAL int __must_check_result outer_last(MDBX_cursor *__restrict mc, MDBX_val *__restrict key,
                                                 MDBX_val *__restrict data);

MDBX_INTERNAL int __must_check_result inner_next(MDBX_cursor *__restrict mc, MDBX_val *__restrict data);
MDBX_INTERNAL int __must_check_result inner_prev(MDBX_cursor *__restrict mc, MDBX_val *__restrict data);
MDBX_INTERNAL int __must_check_result outer_next(MDBX_cursor *__restrict mc, MDBX_val *__restrict key,
                                                 MDBX_val *__restrict data, MDBX_cursor_op op);
MDBX_INTERNAL int __must_check_result outer_prev(MDBX_cursor *__restrict mc, MDBX_val *__restrict key,
                                                 MDBX_val *__restrict data, MDBX_cursor_op op);

MDBX_INTERNAL int cursor_init4walk(cursor_couple_t *couple, const MDBX_txn *const txn, tree_t *const tree,
                                   kvx_t *const kvx);

MDBX_INTERNAL int __must_check_result cursor_init(MDBX_cursor *mc, const MDBX_txn *txn, size_t dbi);

MDBX_INTERNAL int __must_check_result cursor_dupsort_setup(MDBX_cursor *mc, const node_t *node, const page_t *mp);

MDBX_INTERNAL int __must_check_result cursor_touch(MDBX_cursor *const mc, const MDBX_val *key, const MDBX_val *data);

/*----------------------------------------------------------------------------*/

/* Update sub-page pointer, if any, in mc->subcur.
 * Needed when the node which contains the sub-page may have moved.
 * Called with mp = mc->pg[mc->top], ki = mc->ki[mc->top]. */
MDBX_MAYBE_UNUSED static inline void cursor_inner_refresh(const MDBX_cursor *mc, const page_t *mp, unsigned ki) {
  cASSERT(mc, is_leaf(mp));
  const node_t *node = page_node(mp, ki);
  if ((node_flags(node) & (N_DUP | N_TREE)) == N_DUP)
    mc->subcur->cursor.pg[0] = node_data(node);
}

MDBX_MAYBE_UNUSED MDBX_INTERNAL bool cursor_is_tracked(const MDBX_cursor *mc);

static inline void cursor_reset(cursor_couple_t *couple) {
  couple->outer.top_and_flags = z_fresh_mark;
  couple->inner.cursor.top_and_flags = z_fresh_mark | z_inner;
}

static inline void cursor_drown(cursor_couple_t *couple) {
  couple->outer.top_and_flags = z_poor_mark;
  couple->inner.cursor.top_and_flags = z_poor_mark | z_inner;
  couple->outer.txn = nullptr;
  couple->inner.cursor.txn = nullptr;
  couple->outer.tree = nullptr;
  /* сохраняем clc-указатель, так он используется для вычисления dbi в mdbx_cursor_renew(). */
  couple->outer.dbi_state = nullptr;
  couple->inner.cursor.dbi_state = nullptr;
}

static inline size_t dpl_setlen(dpl_t *dl, size_t len) {
  static const page_t dpl_stub_pageE = {INVALID_TXNID,
                                        0,
                                        P_BAD,
                                        {0},
                                        /* pgno */ ~(pgno_t)0};
  assert(dpl_stub_pageE.flags == P_BAD && dpl_stub_pageE.pgno == P_INVALID);
  dl->length = len;
  dl->items[len + 1].ptr = (page_t *)&dpl_stub_pageE;
  dl->items[len + 1].pgno = P_INVALID;
  dl->items[len + 1].npages = 1;
  return len;
}

static inline void dpl_clear(dpl_t *dl) {
  static const page_t dpl_stub_pageB = {INVALID_TXNID,
                                        0,
                                        P_BAD,
                                        {0},
                                        /* pgno */ 0};
  assert(dpl_stub_pageB.flags == P_BAD && dpl_stub_pageB.pgno == 0);
  dl->sorted = dpl_setlen(dl, 0);
  dl->pages_including_loose = 0;
  dl->items[0].ptr = (page_t *)&dpl_stub_pageB;
  dl->items[0].pgno = 0;
  dl->items[0].npages = 1;
  assert(dl->items[0].pgno == 0 && dl->items[dl->length + 1].pgno == P_INVALID);
}

MDBX_INTERNAL int __must_check_result dpl_alloc(MDBX_txn *txn);

MDBX_INTERNAL void dpl_free(MDBX_txn *txn);

MDBX_INTERNAL dpl_t *dpl_reserve(MDBX_txn *txn, size_t size);

MDBX_INTERNAL __noinline dpl_t *dpl_sort_slowpath(const MDBX_txn *txn);

static inline dpl_t *dpl_sort(const MDBX_txn *txn) {
  tASSERT(txn, (txn->flags & MDBX_TXN_RDONLY) == 0);
  tASSERT(txn, (txn->flags & MDBX_WRITEMAP) == 0 || MDBX_AVOID_MSYNC);

  dpl_t *dl = txn->wr.dirtylist;
  tASSERT(txn, dl->length <= PAGELIST_LIMIT);
  tASSERT(txn, dl->sorted <= dl->length);
  tASSERT(txn, dl->items[0].pgno == 0 && dl->items[dl->length + 1].pgno == P_INVALID);
  return likely(dl->sorted == dl->length) ? dl : dpl_sort_slowpath(txn);
}

MDBX_NOTHROW_PURE_FUNCTION MDBX_INTERNAL __noinline size_t dpl_search(const MDBX_txn *txn, pgno_t pgno);

MDBX_MAYBE_UNUSED MDBX_INTERNAL const page_t *debug_dpl_find(const MDBX_txn *txn, const pgno_t pgno);

MDBX_NOTHROW_PURE_FUNCTION static inline unsigned dpl_npages(const dpl_t *dl, size_t i) {
  assert(0 <= (intptr_t)i && i <= dl->length);
  unsigned n = dl->items[i].npages;
  assert(n == (is_largepage(dl->items[i].ptr) ? dl->items[i].ptr->pages : 1));
  return n;
}

MDBX_NOTHROW_PURE_FUNCTION static inline pgno_t dpl_endpgno(const dpl_t *dl, size_t i) {
  return dpl_npages(dl, i) + dl->items[i].pgno;
}

MDBX_NOTHROW_PURE_FUNCTION static inline bool dpl_intersect(const MDBX_txn *txn, pgno_t pgno, size_t npages) {
  tASSERT(txn, (txn->flags & MDBX_TXN_RDONLY) == 0);
  tASSERT(txn, (txn->flags & MDBX_WRITEMAP) == 0 || MDBX_AVOID_MSYNC);

  dpl_t *dl = txn->wr.dirtylist;
  tASSERT(txn, dl->sorted == dl->length);
  tASSERT(txn, dl->items[0].pgno == 0 && dl->items[dl->length + 1].pgno == P_INVALID);
  size_t const n = dpl_search(txn, pgno);
  tASSERT(txn, n >= 1 && n <= dl->length + 1);
  tASSERT(txn, pgno <= dl->items[n].pgno);
  tASSERT(txn, pgno > dl->items[n - 1].pgno);
  const bool rc =
      /* intersection with founded */ pgno + npages > dl->items[n].pgno ||
      /* intersection with prev */ dpl_endpgno(dl, n - 1) > pgno;
  if (ASSERT_ENABLED()) {
    bool check = false;
    for (size_t i = 1; i <= dl->length; ++i) {
      const page_t *const dp = dl->items[i].ptr;
      if (!(dp->pgno /* begin */ >= /* end */ pgno + npages || dpl_endpgno(dl, i) /* end */ <= /* begin */ pgno))
        check |= true;
    }
    tASSERT(txn, check == rc);
  }
  return rc;
}

MDBX_NOTHROW_PURE_FUNCTION static inline size_t dpl_exist(const MDBX_txn *txn, pgno_t pgno) {
  tASSERT(txn, (txn->flags & MDBX_WRITEMAP) == 0 || MDBX_AVOID_MSYNC);
  dpl_t *dl = txn->wr.dirtylist;
  size_t i = dpl_search(txn, pgno);
  tASSERT(txn, (int)i > 0);
  return (dl->items[i].pgno == pgno) ? i : 0;
}

MDBX_INTERNAL void dpl_remove_ex(const MDBX_txn *txn, size_t i, size_t npages);

static inline void dpl_remove(const MDBX_txn *txn, size_t i) {
  dpl_remove_ex(txn, i, dpl_npages(txn->wr.dirtylist, i));
}

MDBX_INTERNAL int __must_check_result dpl_append(MDBX_txn *txn, pgno_t pgno, page_t *page, size_t npages);

MDBX_MAYBE_UNUSED MDBX_INTERNAL bool dpl_check(MDBX_txn *txn);

MDBX_NOTHROW_PURE_FUNCTION static inline uint32_t dpl_age(const MDBX_txn *txn, size_t i) {
  tASSERT(txn, (txn->flags & (MDBX_TXN_RDONLY | MDBX_WRITEMAP)) == 0);
  const dpl_t *dl = txn->wr.dirtylist;
  assert((intptr_t)i > 0 && i <= dl->length);
  size_t *const ptr = ptr_disp(dl->items[i].ptr, -(ptrdiff_t)sizeof(size_t));
  return txn->wr.dirtylru - (uint32_t)*ptr;
}

MDBX_INTERNAL void dpl_lru_reduce(MDBX_txn *txn);

static inline uint32_t dpl_lru_turn(MDBX_txn *txn) {
  txn->wr.dirtylru += 1;
  if (unlikely(txn->wr.dirtylru > UINT32_MAX / 3) && (txn->flags & MDBX_WRITEMAP) == 0)
    dpl_lru_reduce(txn);
  return txn->wr.dirtylru;
}

MDBX_INTERNAL void dpl_sift(MDBX_txn *const txn, pnl_t pl, const bool spilled);

MDBX_INTERNAL void dpl_release_shadows(MDBX_txn *txn);

/* Гистограмма решения нарезки фрагментов для ситуации нехватки идентификаторов/слотов. */
typedef struct gc_dense_histogram {
  /* Размер массива одновременно задаёт максимальный размер последовательностей,
   * с которыми решается задача распределения.
   *
   * Использование длинных последовательностей контрпродуктивно, так как такие последовательности будут
   * создавать/воспроизводить/повторять аналогичные затруднения при последующей переработке. Однако,
   * в редких ситуациях это может быть единственным выходом. */
  unsigned end;
  pgno_t array[31];
} gc_dense_histogram_t;

typedef struct gc_update_context {
  unsigned loop;
  unsigned goodchunk;
  bool dense;
  pgno_t prev_first_unallocated;
  size_t retired_stored;
  size_t return_reserved_lo, return_reserved_hi;
  txnid_t gc_first;
  intptr_t return_left;
#ifndef MDBX_DEBUG_GCU
#define MDBX_DEBUG_GCU 0
#endif
#if MDBX_DEBUG_GCU
  struct {
    txnid_t prev;
    unsigned n;
  } dbg;
#endif /* MDBX_DEBUG_GCU */
  rkl_t sequel;
#if MDBX_ENABLE_BIGFOOT
  txnid_t bigfoot;
#endif /* MDBX_ENABLE_BIGFOOT */
  union {
    MDBX_cursor cursor;
    cursor_couple_t couple;
  };
  gc_dense_histogram_t dense_histogram;
} gcu_t;

MDBX_INTERNAL int gc_put_init(MDBX_txn *txn, gcu_t *ctx);
MDBX_INTERNAL void gc_put_destroy(gcu_t *ctx);

#define ALLOC_DEFAULT 0     /* штатное/обычное выделение страниц */
#define ALLOC_UNIMPORTANT 1 /* запрос неважен, невозможность выделения не приведет к ошибке транзакции */
#define ALLOC_RESERVE 2     /* подготовка резерва для обновления GC, без аллокации */
#define ALLOC_COALESCE 4    /* внутреннее состояние/флажок */
#define ALLOC_SHOULD_SCAN 8 /* внутреннее состояние/флажок */
#define ALLOC_LIFO 16       /* внутреннее состояние/флажок */

MDBX_INTERNAL pgr_t gc_alloc_ex(const MDBX_cursor *const mc, const size_t num, uint8_t flags);

MDBX_INTERNAL pgr_t gc_alloc_single(const MDBX_cursor *const mc);
MDBX_INTERNAL int gc_update(MDBX_txn *txn, gcu_t *ctx);

MDBX_NOTHROW_PURE_FUNCTION static inline size_t gc_stockpile(const MDBX_txn *txn) {
  return pnl_size(txn->wr.repnl) + txn->wr.loose_count;
}

MDBX_NOTHROW_PURE_FUNCTION static inline size_t gc_chunk_bytes(const size_t chunk) {
  return (chunk + 1) * sizeof(pgno_t);
}

MDBX_INTERNAL bool gc_repnl_has_span(const MDBX_txn *txn, const size_t num);

static inline bool gc_is_reclaimed(const MDBX_txn *txn, const txnid_t id) {
  return rkl_contain(&txn->wr.gc.reclaimed, id) || rkl_contain(&txn->wr.gc.comeback, id);
}

static inline txnid_t txnid_min(txnid_t a, txnid_t b) { return (a < b) ? a : b; }

static inline txnid_t txnid_max(txnid_t a, txnid_t b) { return (a > b) ? a : b; }

static inline MDBX_cursor *gc_cursor(MDBX_env *env) { return ptr_disp(env->basal_txn, sizeof(MDBX_txn)); }

MDBX_INTERNAL int lck_setup(MDBX_env *env, mdbx_mode_t mode);
#if MDBX_LOCKING > MDBX_LOCKING_SYSV
MDBX_INTERNAL int lck_ipclock_stubinit(osal_ipclock_t *ipc);
MDBX_INTERNAL int lck_ipclock_destroy(osal_ipclock_t *ipc);
#endif /* MDBX_LOCKING > MDBX_LOCKING_SYSV */

MDBX_INTERNAL int lck_init(MDBX_env *env, MDBX_env *inprocess_neighbor, int global_uniqueness_flag);

MDBX_INTERNAL int lck_destroy(MDBX_env *env, MDBX_env *inprocess_neighbor, const uint32_t current_pid);

MDBX_INTERNAL int lck_seize(MDBX_env *env);

MDBX_INTERNAL int lck_downgrade(MDBX_env *env);

MDBX_MAYBE_UNUSED MDBX_INTERNAL int lck_upgrade(MDBX_env *env, bool dont_wait);

MDBX_INTERNAL int lck_rdt_lock(MDBX_env *env);

MDBX_INTERNAL void lck_rdt_unlock(MDBX_env *env);

MDBX_INTERNAL int lck_txn_lock(MDBX_env *env, bool dont_wait);

MDBX_INTERNAL void lck_txn_unlock(MDBX_env *env);

MDBX_INTERNAL int lck_rpid_set(MDBX_env *env);

MDBX_INTERNAL int lck_rpid_clear(MDBX_env *env);

MDBX_INTERNAL int lck_rpid_check(MDBX_env *env, uint32_t pid);

static inline uint64_t meta_sign_calculate(const meta_t *meta) {
  uint64_t sign = DATASIGN_NONE;
#if 0 /* TODO */
  sign = hippeus_hash64(...);
#else
  (void)meta;
#endif
  /* LY: newer returns DATASIGN_NONE or DATASIGN_WEAK */
  return (sign > DATASIGN_WEAK) ? sign : ~sign;
}

static inline uint64_t meta_sign_get(const volatile meta_t *meta) { return unaligned_peek_u64_volatile(4, meta->sign); }

static inline void meta_sign_as_steady(meta_t *meta) { unaligned_poke_u64(4, meta->sign, meta_sign_calculate(meta)); }

static inline bool meta_is_steady(const volatile meta_t *meta) { return SIGN_IS_STEADY(meta_sign_get(meta)); }

MDBX_INTERNAL troika_t meta_tap(const MDBX_env *env);
MDBX_INTERNAL unsigned meta_eq_mask(const troika_t *troika);
MDBX_INTERNAL bool meta_should_retry(const MDBX_env *env, troika_t *troika);
MDBX_MAYBE_UNUSED MDBX_INTERNAL bool troika_verify_fsm(void);

struct meta_ptr {
  txnid_t txnid;
  union {
    const volatile meta_t *ptr_v;
    const meta_t *ptr_c;
  };
  size_t is_steady;
};

MDBX_INTERNAL meta_ptr_t meta_ptr(const MDBX_env *env, unsigned n);
MDBX_INTERNAL txnid_t meta_txnid(const volatile meta_t *meta);
MDBX_INTERNAL txnid_t recent_committed_txnid(const MDBX_env *env);
MDBX_INTERNAL int meta_sync(const MDBX_env *env, const meta_ptr_t head);

MDBX_INTERNAL const char *durable_caption(const meta_t *const meta);
MDBX_INTERNAL void meta_troika_dump(const MDBX_env *env, const troika_t *troika);

#define METAPAGE(env, n) page_meta(pgno2page(env, n))
#define METAPAGE_END(env) METAPAGE(env, NUM_METAS)

static inline meta_ptr_t meta_recent(const MDBX_env *env, const troika_t *troika) {
  meta_ptr_t r;
  r.txnid = troika->txnid[troika->recent];
  r.ptr_v = METAPAGE(env, troika->recent);
  r.is_steady = (troika->fsm >> troika->recent) & 1;
  return r;
}

static inline meta_ptr_t meta_prefer_steady(const MDBX_env *env, const troika_t *troika) {
  meta_ptr_t r;
  r.txnid = troika->txnid[troika->prefer_steady];
  r.ptr_v = METAPAGE(env, troika->prefer_steady);
  r.is_steady = (troika->fsm >> troika->prefer_steady) & 1;
  return r;
}

static inline meta_ptr_t meta_tail(const MDBX_env *env, const troika_t *troika) {
  const uint8_t tail = troika->tail_and_flags & 3;
  MDBX_ANALYSIS_ASSUME(tail < NUM_METAS);
  meta_ptr_t r;
  r.txnid = troika->txnid[tail];
  r.ptr_v = METAPAGE(env, tail);
  r.is_steady = (troika->fsm >> tail) & 1;
  return r;
}

static inline bool meta_is_used(const troika_t *troika, unsigned n) {
  return n == troika->recent || n == troika->prefer_steady;
}

static inline bool meta_bootid_match(const meta_t *meta) {

  return memcmp(&meta->bootid, &globals.bootid, 16) == 0 && (globals.bootid.x | globals.bootid.y) != 0;
}

static inline bool meta_weak_acceptable(const MDBX_env *env, const meta_t *meta, const int lck_exclusive) {
  return lck_exclusive
             ? /* exclusive lock */ meta_bootid_match(meta)
             : /* db already opened */ env->lck_mmap.lck && (env->lck_mmap.lck->envmode.weak & MDBX_RDONLY) == 0;
}

MDBX_NOTHROW_PURE_FUNCTION static inline txnid_t constmeta_txnid(const meta_t *meta) {
  const txnid_t a = unaligned_peek_u64(4, &meta->txnid_a);
  const txnid_t b = unaligned_peek_u64(4, &meta->txnid_b);
  return likely(a == b) ? a : 0;
}

static inline void meta_update_begin(const MDBX_env *env, meta_t *meta, txnid_t txnid) {
  eASSERT(env, meta >= METAPAGE(env, 0) && meta < METAPAGE_END(env));
  eASSERT(env, unaligned_peek_u64(4, meta->txnid_a) < txnid && unaligned_peek_u64(4, meta->txnid_b) < txnid);
  (void)env;
#if (defined(__amd64__) || defined(__e2k__)) && !defined(ENABLE_UBSAN) && MDBX_UNALIGNED_OK >= 8
  atomic_store64((mdbx_atomic_uint64_t *)&meta->txnid_b, 0, mo_AcquireRelease);
  atomic_store64((mdbx_atomic_uint64_t *)&meta->txnid_a, txnid, mo_AcquireRelease);
#else
  atomic_store32(&meta->txnid_b[__BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__], 0, mo_AcquireRelease);
  atomic_store32(&meta->txnid_b[__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__], 0, mo_AcquireRelease);
  atomic_store32(&meta->txnid_a[__BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__], (uint32_t)txnid, mo_AcquireRelease);
  atomic_store32(&meta->txnid_a[__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__], (uint32_t)(txnid >> 32), mo_AcquireRelease);
#endif
}

static inline void meta_update_end(const MDBX_env *env, meta_t *meta, txnid_t txnid) {
  eASSERT(env, meta >= METAPAGE(env, 0) && meta < METAPAGE_END(env));
  eASSERT(env, unaligned_peek_u64(4, meta->txnid_a) == txnid);
  eASSERT(env, unaligned_peek_u64(4, meta->txnid_b) < txnid);
  (void)env;
  jitter4testing(true);
  memcpy(&meta->bootid, &globals.bootid, 16);
#if (defined(__amd64__) || defined(__e2k__)) && !defined(ENABLE_UBSAN) && MDBX_UNALIGNED_OK >= 8
  atomic_store64((mdbx_atomic_uint64_t *)&meta->txnid_b, txnid, mo_AcquireRelease);
#else
  atomic_store32(&meta->txnid_b[__BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__], (uint32_t)txnid, mo_AcquireRelease);
  atomic_store32(&meta->txnid_b[__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__], (uint32_t)(txnid >> 32), mo_AcquireRelease);
#endif
}

static inline void meta_set_txnid(const MDBX_env *env, meta_t *meta, const txnid_t txnid) {
  eASSERT(env, !env->dxb_mmap.base || meta < METAPAGE(env, 0) || meta >= METAPAGE_END(env));
  (void)env;
  /* update inconsistently since this function used ONLY for filling meta-image
   * for writing, but not the actual meta-page */
  memcpy(&meta->bootid, &globals.bootid, 16);
  unaligned_poke_u64(4, meta->txnid_a, txnid);
  unaligned_poke_u64(4, meta->txnid_b, txnid);
}

static inline uint8_t meta_cmp2int(txnid_t a, txnid_t b, uint8_t s) {
  return unlikely(a == b) ? 1 * s : (a > b) ? 2 * s : 0 * s;
}

static inline uint8_t meta_cmp2recent(uint8_t ab_cmp2int, bool a_steady, bool b_steady) {
  assert(ab_cmp2int < 3 /* && a_steady< 2 && b_steady < 2 */);
  return ab_cmp2int > 1 || (ab_cmp2int == 1 && a_steady > b_steady);
}

static inline uint8_t meta_cmp2steady(uint8_t ab_cmp2int, bool a_steady, bool b_steady) {
  assert(ab_cmp2int < 3 /* && a_steady< 2 && b_steady < 2 */);
  return a_steady > b_steady || (a_steady == b_steady && ab_cmp2int > 1);
}

static inline bool meta_choice_recent(txnid_t a_txnid, bool a_steady, txnid_t b_txnid, bool b_steady) {
  return meta_cmp2recent(meta_cmp2int(a_txnid, b_txnid, 1), a_steady, b_steady);
}

static inline bool meta_choice_steady(txnid_t a_txnid, bool a_steady, txnid_t b_txnid, bool b_steady) {
  return meta_cmp2steady(meta_cmp2int(a_txnid, b_txnid, 1), a_steady, b_steady);
}

MDBX_INTERNAL meta_t *meta_init_triplet(const MDBX_env *env, void *buffer);

MDBX_INTERNAL int meta_validate(MDBX_env *env, meta_t *const meta, const page_t *const page, const unsigned meta_number,
                                unsigned *guess_pagesize);

MDBX_INTERNAL int __must_check_result meta_validate_copy(MDBX_env *env, const meta_t *meta, meta_t *dest);

MDBX_INTERNAL int __must_check_result meta_override(MDBX_env *env, size_t target, txnid_t txnid, const meta_t *shape);

MDBX_INTERNAL int meta_wipe_steady(MDBX_env *env, txnid_t inclusive_upto);

#if !(defined(_WIN32) || defined(_WIN64))
#define MDBX_WRITETHROUGH_THRESHOLD_DEFAULT 2
#endif

struct iov_ctx {
  MDBX_env *env;
  osal_ioring_t *ior;
  mdbx_filehandle_t fd;
  int err;
#ifndef MDBX_NEED_WRITTEN_RANGE
#define MDBX_NEED_WRITTEN_RANGE 1
#endif /* MDBX_NEED_WRITTEN_RANGE */
#if MDBX_NEED_WRITTEN_RANGE
  pgno_t flush_begin;
  pgno_t flush_end;
#endif /* MDBX_NEED_WRITTEN_RANGE */
  uint64_t coherency_timestamp;
};

MDBX_INTERNAL __must_check_result int iov_init(MDBX_txn *const txn, iov_ctx_t *ctx, size_t items, size_t npages,
                                               mdbx_filehandle_t fd, bool check_coherence);

static inline bool iov_empty(const iov_ctx_t *ctx) { return osal_ioring_used(ctx->ior) == 0; }

MDBX_INTERNAL __must_check_result int iov_page(MDBX_txn *txn, iov_ctx_t *ctx, page_t *dp, size_t npages);

MDBX_INTERNAL __must_check_result int iov_write(iov_ctx_t *ctx);

MDBX_INTERNAL void spill_remove(MDBX_txn *txn, size_t idx, size_t npages);
MDBX_INTERNAL pnl_t spill_purge(MDBX_txn *txn);
MDBX_INTERNAL int spill_slowpath(MDBX_txn *const txn, MDBX_cursor *const m0, const intptr_t wanna_spill_entries,
                                 const intptr_t wanna_spill_npages, const size_t need);
/*----------------------------------------------------------------------------*/

static inline size_t spill_search(const MDBX_txn *txn, pgno_t pgno) {
  tASSERT(txn, (txn->flags & MDBX_WRITEMAP) == 0 || MDBX_AVOID_MSYNC);
  const pnl_t pnl = txn->wr.spilled.list;
  if (likely(!pnl))
    return 0;
  pgno <<= 1;
  size_t n = pnl_search(pnl, pgno, (size_t)MAX_PAGENO + MAX_PAGENO + 1);
  return (n <= pnl_size(pnl) && pnl[n] == pgno) ? n : 0;
}

static inline bool spill_intersect(const MDBX_txn *txn, pgno_t pgno, size_t npages) {
  const pnl_t pnl = txn->wr.spilled.list;
  if (likely(!pnl))
    return false;
  const size_t len = pnl_size(pnl);
  if (LOG_ENABLED(MDBX_LOG_EXTRA)) {
    DEBUG_EXTRA("PNL len %zu [", len);
    for (size_t i = 1; i <= len; ++i)
      DEBUG_EXTRA_PRINT(" %li", (pnl[i] & 1) ? -(long)(pnl[i] >> 1) : (long)(pnl[i] >> 1));
    DEBUG_EXTRA_PRINT("%s\n", "]");
  }
  const pgno_t spilled_range_begin = pgno << 1;
  const pgno_t spilled_range_last = ((pgno + (pgno_t)npages) << 1) - 1;
#if MDBX_PNL_ASCENDING
  const size_t n = pnl_search(pnl, spilled_range_begin, (size_t)(MAX_PAGENO + 1) << 1);
  tASSERT(txn, n && (n == pnl_size(pnl) + 1 || spilled_range_begin <= pnl[n]));
  const bool rc = n <= pnl_size(pnl) && pnl[n] <= spilled_range_last;
#else
  const size_t n = pnl_search(pnl, spilled_range_last, (size_t)MAX_PAGENO + MAX_PAGENO + 1);
  tASSERT(txn, n && (n == pnl_size(pnl) + 1 || spilled_range_last >= pnl[n]));
  const bool rc = n <= pnl_size(pnl) && pnl[n] >= spilled_range_begin;
#endif
  if (ASSERT_ENABLED()) {
    bool check = false;
    for (size_t i = 0; i < npages; ++i)
      check |= spill_search(txn, (pgno_t)(pgno + i)) != 0;
    tASSERT(txn, check == rc);
  }
  return rc;
}

static inline int txn_spill(MDBX_txn *const txn, MDBX_cursor *const m0, const size_t need) {
  tASSERT(txn, (txn->flags & MDBX_TXN_RDONLY) == 0);
  tASSERT(txn, !m0 || cursor_is_tracked(m0));

  const intptr_t wanna_spill_entries = txn->wr.dirtylist ? (need - txn->wr.dirtyroom - txn->wr.loose_count) : 0;
  const intptr_t wanna_spill_npages =
      need + (txn->wr.dirtylist ? txn->wr.dirtylist->pages_including_loose : txn->wr.writemap_dirty_npages) -
      txn->wr.loose_count - txn->env->options.dp_limit;

  /* production mode */
  if (likely(wanna_spill_npages < 1 && wanna_spill_entries < 1)
#if xMDBX_DEBUG_SPILLING == 1
      /* debug mode: always try to spill if xMDBX_DEBUG_SPILLING == 1 */
      && txn->txnid % 23 > 11
#endif
  )
    return MDBX_SUCCESS;

  return spill_slowpath(txn, m0, wanna_spill_entries, wanna_spill_npages, need);
}

MDBX_INTERNAL int __must_check_result tree_search_finalize(MDBX_cursor *mc, const MDBX_val *key, int flags);
MDBX_INTERNAL int tree_search_lowest(MDBX_cursor *mc);
MDBX_INTERNAL size_t tree_search_branch(MDBX_cursor *mc, const MDBX_val *key);

enum page_search_flags {
  Z_MODIFY = 1,
  Z_ROOTONLY = 2,
  Z_FIRST = 4,
  Z_LAST = 8,
};
MDBX_INTERNAL int __must_check_result tree_search(MDBX_cursor *mc, const MDBX_val *key, int flags);

#define MDBX_SPLIT_REPLACE MDBX_APPENDDUP /* newkey is not new */
MDBX_INTERNAL int __must_check_result page_split(MDBX_cursor *mc, const MDBX_val *const newkey, MDBX_val *const newdata,
                                                 pgno_t newpgno, const unsigned naf);

/*----------------------------------------------------------------------------*/

MDBX_INTERNAL int MDBX_PRINTF_ARGS(2, 3) bad_page(const page_t *mp, const char *fmt, ...);

MDBX_INTERNAL void MDBX_PRINTF_ARGS(2, 3) poor_page(const page_t *mp, const char *fmt, ...);

MDBX_NOTHROW_PURE_FUNCTION static inline bool is_frozen(const MDBX_txn *txn, const page_t *mp) {
  return mp->txnid < txn->txnid;
}

MDBX_NOTHROW_PURE_FUNCTION static inline bool is_spilled(const MDBX_txn *txn, const page_t *mp) {
  return mp->txnid == txn->txnid;
}

MDBX_NOTHROW_PURE_FUNCTION static inline bool is_shadowed(const MDBX_txn *txn, const page_t *mp) {
  return mp->txnid > txn->txnid;
}

MDBX_MAYBE_UNUSED MDBX_NOTHROW_PURE_FUNCTION static inline bool is_correct(const MDBX_txn *txn, const page_t *mp) {
  return mp->txnid <= txn->front_txnid;
}

MDBX_NOTHROW_PURE_FUNCTION static inline bool is_modifable(const MDBX_txn *txn, const page_t *mp) {
  return mp->txnid == txn->front_txnid;
}

MDBX_INTERNAL int __must_check_result page_check(const MDBX_cursor *const mc, const page_t *const mp);

MDBX_INTERNAL pgr_t page_get_any(const MDBX_cursor *const mc, const pgno_t pgno, const txnid_t front);

MDBX_INTERNAL pgr_t page_get_three(const MDBX_cursor *const mc, const pgno_t pgno, const txnid_t front);

MDBX_INTERNAL pgr_t page_get_large(const MDBX_cursor *const mc, const pgno_t pgno, const txnid_t front);

static inline int __must_check_result page_get(const MDBX_cursor *mc, const pgno_t pgno, page_t **mp,
                                               const txnid_t front) {
  pgr_t ret = page_get_three(mc, pgno, front);
  *mp = ret.page;
  return ret.err;
}

/*----------------------------------------------------------------------------*/

MDBX_INTERNAL int __must_check_result page_dirty(MDBX_txn *txn, page_t *mp, size_t npages);
MDBX_INTERNAL pgr_t page_new(MDBX_cursor *mc, const unsigned flags);
MDBX_INTERNAL pgr_t page_new_large(MDBX_cursor *mc, const size_t npages);
MDBX_INTERNAL int page_touch_modifable(MDBX_txn *txn, const page_t *const mp);
MDBX_INTERNAL int page_touch_unmodifable(MDBX_txn *txn, MDBX_cursor *mc, const page_t *const mp);

static inline int page_touch(MDBX_cursor *mc) {
  page_t *const mp = mc->pg[mc->top];
  MDBX_txn *txn = mc->txn;

  tASSERT(txn, mc->txn->flags & MDBX_TXN_DIRTY);
  tASSERT(txn, F_ISSET(*cursor_dbi_state(mc), DBI_LINDO | DBI_VALID | DBI_DIRTY));
  tASSERT(txn, !is_largepage(mp));
  if (ASSERT_ENABLED()) {
    if (mc->flags & z_inner) {
      subcur_t *mx = container_of(mc->tree, subcur_t, nested_tree);
      cursor_couple_t *couple = container_of(mx, cursor_couple_t, inner);
      tASSERT(txn, mc->tree == &couple->outer.subcur->nested_tree);
      tASSERT(txn, &mc->clc->k == &couple->outer.clc->v);
      tASSERT(txn, *couple->outer.dbi_state & DBI_DIRTY);
    }
    tASSERT(txn, dpl_check(txn));
  }

  if (is_modifable(txn, mp)) {
    if (!txn->wr.dirtylist) {
      tASSERT(txn, (txn->flags & MDBX_WRITEMAP) && !MDBX_AVOID_MSYNC);
      return MDBX_SUCCESS;
    }
    return is_subpage(mp) ? MDBX_SUCCESS : page_touch_modifable(txn, mp);
  }
  return page_touch_unmodifable(txn, mc, mp);
}

MDBX_INTERNAL void page_copy(page_t *const dst, const page_t *const src, const size_t size);
MDBX_INTERNAL pgr_t __must_check_result page_unspill(MDBX_txn *const txn, const page_t *const mp);

MDBX_INTERNAL page_t *page_shadow_alloc(MDBX_txn *txn, size_t num);

MDBX_INTERNAL void page_shadow_release(MDBX_env *env, page_t *dp, size_t npages);

MDBX_INTERNAL int page_retire_ex(MDBX_cursor *mc, const pgno_t pgno, page_t *mp /* maybe null */,
                                 unsigned pageflags /* maybe unknown/zero */);

static inline int page_retire(MDBX_cursor *mc, page_t *mp) { return page_retire_ex(mc, mp->pgno, mp, mp->flags); }

static inline void page_wash(MDBX_txn *txn, size_t di, page_t *const mp, const size_t npages) {
  tASSERT(txn, (txn->flags & MDBX_TXN_RDONLY) == 0);
  mp->txnid = INVALID_TXNID;
  mp->flags = P_BAD;

  if (txn->wr.dirtylist) {
    tASSERT(txn, (txn->flags & MDBX_WRITEMAP) == 0 || MDBX_AVOID_MSYNC);
    tASSERT(txn, MDBX_AVOID_MSYNC || (di && txn->wr.dirtylist->items[di].ptr == mp));
    if (!MDBX_AVOID_MSYNC || di) {
      dpl_remove_ex(txn, di, npages);
      txn->wr.dirtyroom++;
      tASSERT(txn, txn->wr.dirtyroom + txn->wr.dirtylist->length ==
                       (txn->parent ? txn->parent->wr.dirtyroom : txn->env->options.dp_limit));
      if (!MDBX_AVOID_MSYNC || !(txn->flags & MDBX_WRITEMAP)) {
        page_shadow_release(txn->env, mp, npages);
        return;
      }
    }
  } else {
    tASSERT(txn, (txn->flags & MDBX_WRITEMAP) && !MDBX_AVOID_MSYNC && !di);
    txn->wr.writemap_dirty_npages -= (txn->wr.writemap_dirty_npages > npages) ? npages : txn->wr.writemap_dirty_npages;
  }
  VALGRIND_MAKE_MEM_UNDEFINED(mp, PAGEHDRSZ);
  VALGRIND_MAKE_MEM_NOACCESS(page2payload(mp), pgno2bytes(txn->env, npages) - PAGEHDRSZ);
  MDBX_ASAN_POISON_MEMORY_REGION(page2payload(mp), pgno2bytes(txn->env, npages) - PAGEHDRSZ);
}

MDBX_INTERNAL size_t page_subleaf2_reserve(const MDBX_env *env, size_t host_page_room, size_t subpage_len,
                                           size_t item_len);

#define page_next(mp) (*(page_t **)ptr_disp((mp)->entries, sizeof(void *) - sizeof(uint32_t)))

MDBX_INTERNAL void rthc_ctor(void);
MDBX_INTERNAL void rthc_dtor(const uint32_t current_pid);
MDBX_INTERNAL void rthc_lock(void);
MDBX_INTERNAL void rthc_unlock(void);

MDBX_INTERNAL int rthc_register(MDBX_env *const env);
MDBX_INTERNAL int rthc_remove(MDBX_env *const env);
MDBX_INTERNAL int rthc_uniq_check(const osal_mmap_t *pending, MDBX_env **found);

/* dtor called for thread, i.e. for all mdbx's environment objects */
MDBX_INTERNAL void rthc_thread_dtor(void *rthc);

static inline void *thread_rthc_get(osal_thread_key_t key) {
#if defined(_WIN32) || defined(_WIN64)
  return TlsGetValue(key);
#else
  return pthread_getspecific(key);
#endif
}

MDBX_INTERNAL void thread_rthc_set(osal_thread_key_t key, const void *value);

#if !defined(_WIN32) && !defined(_WIN64)
MDBX_INTERNAL void rthc_afterfork(void);
MDBX_INTERNAL void workaround_glibc_bug21031(void);
#endif /* !Windows */

static inline void thread_key_delete(osal_thread_key_t key) {
  TRACE("key = %" PRIuPTR, (uintptr_t)key);
#if defined(_WIN32) || defined(_WIN64)
  ENSURE(nullptr, TlsFree(key));
#else
  ENSURE(nullptr, pthread_key_delete(key) == 0);
  workaround_glibc_bug21031();
#endif
}

typedef struct walk_tbl {
  MDBX_val name;
  tree_t *internal, *nested;
} walk_tbl_t;

typedef int walk_func(const size_t pgno, const unsigned number, void *const ctx, const int deep,
                      const walk_tbl_t *table, const size_t page_size, const page_type_t page_type,
                      const MDBX_error_t err, const size_t nentries, const size_t payload_bytes,
                      const size_t header_bytes, const size_t unused_bytes, const size_t parent_pgno);

typedef enum walk_options { dont_check_keys_ordering = 1 } walk_options_t;

MDBX_INTERNAL int walk_pages(MDBX_txn *txn, walk_func *visitor, void *user, walk_options_t options);

typedef struct walk_ctx {
  void *userctx;
  walk_options_t options;
  int deep;
  walk_func *visitor;
  MDBX_txn *txn;
  MDBX_cursor *cursor;
} walk_ctx_t;

MDBX_INTERNAL int walk_tbl(walk_ctx_t *ctx, walk_tbl_t *tbl);

///

#define MDBX_RADIXSORT_THRESHOLD 142

/* ---------------------------------------------------------------------------
 * LY: State of the art quicksort-based sorting, with internal stack
 * and network-sort for small chunks.
 * Thanks to John M. Gamble for the http://pages.ripco.net/~jgamble/nw.html */

#if MDBX_HAVE_CMOV
#define SORT_CMP_SWAP(TYPE, CMP, a, b)                                                                                 \
  do {                                                                                                                 \
    const TYPE swap_tmp = (a);                                                                                         \
    const bool swap_cmp = expect_with_probability(CMP(swap_tmp, b), 0, .5);                                            \
    (a) = swap_cmp ? swap_tmp : b;                                                                                     \
    (b) = swap_cmp ? b : swap_tmp;                                                                                     \
  } while (0)
#else
#define SORT_CMP_SWAP(TYPE, CMP, a, b)                                                                                 \
  do                                                                                                                   \
    if (expect_with_probability(!CMP(a, b), 0, .5)) {                                                                  \
      const TYPE swap_tmp = (a);                                                                                       \
      (a) = (b);                                                                                                       \
      (b) = swap_tmp;                                                                                                  \
    }                                                                                                                  \
  while (0)
#endif

//  3 comparators, 3 parallel operations
//  o-----^--^--o
//        |  |
//  o--^--|--v--o
//     |  |
//  o--v--v-----o
//
//  [[1,2]]
//  [[0,2]]
//  [[0,1]]
#define SORT_NETWORK_3(TYPE, CMP, begin)                                                                               \
  do {                                                                                                                 \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[1]);                                                                      \
  } while (0)

//  5 comparators, 3 parallel operations
//  o--^--^--------o
//     |  |
//  o--v--|--^--^--o
//        |  |  |
//  o--^--v--|--v--o
//     |     |
//  o--v-----v-----o
//
//  [[0,1],[2,3]]
//  [[0,2],[1,3]]
//  [[1,2]]
#define SORT_NETWORK_4(TYPE, CMP, begin)                                                                               \
  do {                                                                                                                 \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[1]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[2]);                                                                      \
  } while (0)

//  9 comparators, 5 parallel operations
//  o--^--^-----^-----------o
//     |  |     |
//  o--|--|--^--v-----^--^--o
//     |  |  |        |  |
//  o--|--v--|--^--^--|--v--o
//     |     |  |  |  |
//  o--|-----v--|--v--|--^--o
//     |        |     |  |
//  o--v--------v-----v--v--o
//
//  [[0,4],[1,3]]
//  [[0,2]]
//  [[2,4],[0,1]]
//  [[2,3],[1,4]]
//  [[1,2],[3,4]]
#define SORT_NETWORK_5(TYPE, CMP, begin)                                                                               \
  do {                                                                                                                 \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[1]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[4]);                                                                      \
  } while (0)

//  12 comparators, 6 parallel operations
//  o-----^--^--^-----------------o
//        |  |  |
//  o--^--|--v--|--^--------^-----o
//     |  |     |  |        |
//  o--v--v-----|--|--^--^--|--^--o
//              |  |  |  |  |  |
//  o-----^--^--v--|--|--|--v--v--o
//        |  |     |  |  |
//  o--^--|--v-----v--|--v--------o
//     |  |           |
//  o--v--v-----------v-----------o
//
//  [[1,2],[4,5]]
//  [[0,2],[3,5]]
//  [[0,1],[3,4],[2,5]]
//  [[0,3],[1,4]]
//  [[2,4],[1,3]]
//  [[2,3]]
#define SORT_NETWORK_6(TYPE, CMP, begin)                                                                               \
  do {                                                                                                                 \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[4], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[1]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[3]);                                                                      \
  } while (0)

//  16 comparators, 6 parallel operations
//  o--^--------^-----^-----------------o
//     |        |     |
//  o--|--^-----|--^--v--------^--^-----o
//     |  |     |  |           |  |
//  o--|--|--^--v--|--^-----^--|--v-----o
//     |  |  |     |  |     |  |
//  o--|--|--|-----v--|--^--v--|--^--^--o
//     |  |  |        |  |     |  |  |
//  o--v--|--|--^-----v--|--^--v--|--v--o
//        |  |  |        |  |     |
//  o-----v--|--|--------v--v-----|--^--o
//           |  |                 |  |
//  o--------v--v-----------------v--v--o
//
//  [[0,4],[1,5],[2,6]]
//  [[0,2],[1,3],[4,6]]
//  [[2,4],[3,5],[0,1]]
//  [[2,3],[4,5]]
//  [[1,4],[3,6]]
//  [[1,2],[3,4],[5,6]]
#define SORT_NETWORK_7(TYPE, CMP, begin)                                                                               \
  do {                                                                                                                 \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[6]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[4], begin[6]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[1]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[4], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[6]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[5], begin[6]);                                                                      \
  } while (0)

//  19 comparators, 6 parallel operations
//  o--^--------^-----^-----------------o
//     |        |     |
//  o--|--^-----|--^--v--------^--^-----o
//     |  |     |  |           |  |
//  o--|--|--^--v--|--^-----^--|--v-----o
//     |  |  |     |  |     |  |
//  o--|--|--|--^--v--|--^--v--|--^--^--o
//     |  |  |  |     |  |     |  |  |
//  o--v--|--|--|--^--v--|--^--v--|--v--o
//        |  |  |  |     |  |     |
//  o-----v--|--|--|--^--v--v-----|--^--o
//           |  |  |  |           |  |
//  o--------v--|--v--|--^--------v--v--o
//              |     |  |
//  o-----------v-----v--v--------------o
//
//  [[0,4],[1,5],[2,6],[3,7]]
//  [[0,2],[1,3],[4,6],[5,7]]
//  [[2,4],[3,5],[0,1],[6,7]]
//  [[2,3],[4,5]]
//  [[1,4],[3,6]]
//  [[1,2],[3,4],[5,6]]
#define SORT_NETWORK_8(TYPE, CMP, begin)                                                                               \
  do {                                                                                                                 \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[6]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[7]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[4], begin[6]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[5], begin[7]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[1]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[6], begin[7]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[2], begin[3]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[4], begin[5]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[6]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[1], begin[2]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[3], begin[4]);                                                                      \
    SORT_CMP_SWAP(TYPE, CMP, begin[5], begin[6]);                                                                      \
  } while (0)

#define SORT_INNER(TYPE, CMP, begin, end, len)                                                                         \
  switch (len) {                                                                                                       \
  default:                                                                                                             \
    assert(false);                                                                                                     \
    __unreachable();                                                                                                   \
  case 0:                                                                                                              \
  case 1:                                                                                                              \
    break;                                                                                                             \
  case 2:                                                                                                              \
    SORT_CMP_SWAP(TYPE, CMP, begin[0], begin[1]);                                                                      \
    break;                                                                                                             \
  case 3:                                                                                                              \
    SORT_NETWORK_3(TYPE, CMP, begin);                                                                                  \
    break;                                                                                                             \
  case 4:                                                                                                              \
    SORT_NETWORK_4(TYPE, CMP, begin);                                                                                  \
    break;                                                                                                             \
  case 5:                                                                                                              \
    SORT_NETWORK_5(TYPE, CMP, begin);                                                                                  \
    break;                                                                                                             \
  case 6:                                                                                                              \
    SORT_NETWORK_6(TYPE, CMP, begin);                                                                                  \
    break;                                                                                                             \
  case 7:                                                                                                              \
    SORT_NETWORK_7(TYPE, CMP, begin);                                                                                  \
    break;                                                                                                             \
  case 8:                                                                                                              \
    SORT_NETWORK_8(TYPE, CMP, begin);                                                                                  \
    break;                                                                                                             \
  }

#define SORT_SWAP(TYPE, a, b)                                                                                          \
  do {                                                                                                                 \
    const TYPE swap_tmp = (a);                                                                                         \
    (a) = (b);                                                                                                         \
    (b) = swap_tmp;                                                                                                    \
  } while (0)

#define SORT_PUSH(low, high)                                                                                           \
  do {                                                                                                                 \
    top->lo = (low);                                                                                                   \
    top->hi = (high);                                                                                                  \
    ++top;                                                                                                             \
  } while (0)

#define SORT_POP(low, high)                                                                                            \
  do {                                                                                                                 \
    --top;                                                                                                             \
    low = top->lo;                                                                                                     \
    high = top->hi;                                                                                                    \
  } while (0)

#define SORT_IMPL(NAME, EXPECT_LOW_CARDINALITY_OR_PRESORTED, TYPE, CMP)                                                \
                                                                                                                       \
  static inline bool NAME##_is_sorted(const TYPE *first, const TYPE *last) {                                           \
    while (++first <= last)                                                                                            \
      if (expect_with_probability(CMP(first[0], first[-1]), 1, .1))                                                    \
        return false;                                                                                                  \
    return true;                                                                                                       \
  }                                                                                                                    \
                                                                                                                       \
  typedef struct {                                                                                                     \
    TYPE *lo, *hi;                                                                                                     \
  } NAME##_stack;                                                                                                      \
                                                                                                                       \
  __hot static void NAME(TYPE *const __restrict begin, TYPE *const __restrict end) {                                   \
    NAME##_stack stack[sizeof(size_t) * CHAR_BIT], *__restrict top = stack;                                            \
                                                                                                                       \
    TYPE *__restrict hi = end - 1;                                                                                     \
    TYPE *__restrict lo = begin;                                                                                       \
    while (true) {                                                                                                     \
      const ptrdiff_t len = hi - lo;                                                                                   \
      if (len < 8) {                                                                                                   \
        SORT_INNER(TYPE, CMP, lo, hi + 1, len + 1);                                                                    \
        if (unlikely(top == stack))                                                                                    \
          break;                                                                                                       \
        SORT_POP(lo, hi);                                                                                              \
        continue;                                                                                                      \
      }                                                                                                                \
                                                                                                                       \
      TYPE *__restrict mid = lo + (len >> 1);                                                                          \
      SORT_CMP_SWAP(TYPE, CMP, *lo, *mid);                                                                             \
      SORT_CMP_SWAP(TYPE, CMP, *mid, *hi);                                                                             \
      SORT_CMP_SWAP(TYPE, CMP, *lo, *mid);                                                                             \
                                                                                                                       \
      TYPE *right = hi - 1;                                                                                            \
      TYPE *left = lo + 1;                                                                                             \
      while (1) {                                                                                                      \
        while (expect_with_probability(CMP(*left, *mid), 0, .5))                                                       \
          ++left;                                                                                                      \
        while (expect_with_probability(CMP(*mid, *right), 0, .5))                                                      \
          --right;                                                                                                     \
        if (unlikely(left > right)) {                                                                                  \
          if (EXPECT_LOW_CARDINALITY_OR_PRESORTED) {                                                                   \
            if (NAME##_is_sorted(lo, right))                                                                           \
              lo = right + 1;                                                                                          \
            if (NAME##_is_sorted(left, hi))                                                                            \
              hi = left;                                                                                               \
          }                                                                                                            \
          break;                                                                                                       \
        }                                                                                                              \
        SORT_SWAP(TYPE, *left, *right);                                                                                \
        mid = (mid == left) ? right : (mid == right) ? left : mid;                                                     \
        ++left;                                                                                                        \
        --right;                                                                                                       \
      }                                                                                                                \
                                                                                                                       \
      if (right - lo > hi - left) {                                                                                    \
        SORT_PUSH(lo, right);                                                                                          \
        lo = left;                                                                                                     \
      } else {                                                                                                         \
        SORT_PUSH(left, hi);                                                                                           \
        hi = right;                                                                                                    \
      }                                                                                                                \
    }                                                                                                                  \
                                                                                                                       \
    if (AUDIT_ENABLED()) {                                                                                             \
      for (TYPE *scan = begin + 1; scan < end; ++scan)                                                                 \
        assert(CMP(scan[-1], scan[0]));                                                                                \
    }                                                                                                                  \
  }

/*------------------------------------------------------------------------------
 * LY: radix sort for large chunks */

#define RADIXSORT_IMPL(NAME, TYPE, EXTRACT_KEY, BUFFER_PREALLOCATED, END_GAP)                                          \
                                                                                                                       \
  __hot static bool NAME##_radixsort(TYPE *const begin, const size_t length) {                                         \
    TYPE *tmp;                                                                                                         \
    if (BUFFER_PREALLOCATED) {                                                                                         \
      tmp = begin + length + END_GAP;                                                                                  \
      /* memset(tmp, 0xDeadBeef, sizeof(TYPE) * length); */                                                            \
    } else {                                                                                                           \
      tmp = osal_malloc(sizeof(TYPE) * length);                                                                        \
      if (unlikely(!tmp))                                                                                              \
        return false;                                                                                                  \
    }                                                                                                                  \
                                                                                                                       \
    size_t key_shift = 0, key_diff_mask;                                                                               \
    do {                                                                                                               \
      struct {                                                                                                         \
        pgno_t a[256], b[256];                                                                                         \
      } counters;                                                                                                      \
      memset(&counters, 0, sizeof(counters));                                                                          \
                                                                                                                       \
      key_diff_mask = 0;                                                                                               \
      size_t prev_key = EXTRACT_KEY(begin) >> key_shift;                                                               \
      TYPE *r = begin, *end = begin + length;                                                                          \
      do {                                                                                                             \
        const size_t key = EXTRACT_KEY(r) >> key_shift;                                                                \
        counters.a[key & 255]++;                                                                                       \
        counters.b[(key >> 8) & 255]++;                                                                                \
        key_diff_mask |= prev_key ^ key;                                                                               \
        prev_key = key;                                                                                                \
      } while (++r != end);                                                                                            \
                                                                                                                       \
      pgno_t ta = 0, tb = 0;                                                                                           \
      for (size_t i = 0; i < 256; ++i) {                                                                               \
        const pgno_t ia = counters.a[i];                                                                               \
        counters.a[i] = ta;                                                                                            \
        ta += ia;                                                                                                      \
        const pgno_t ib = counters.b[i];                                                                               \
        counters.b[i] = tb;                                                                                            \
        tb += ib;                                                                                                      \
      }                                                                                                                \
                                                                                                                       \
      r = begin;                                                                                                       \
      do {                                                                                                             \
        const size_t key = EXTRACT_KEY(r) >> key_shift;                                                                \
        tmp[counters.a[key & 255]++] = *r;                                                                             \
      } while (++r != end);                                                                                            \
                                                                                                                       \
      if (unlikely(key_diff_mask < 256)) {                                                                             \
        memcpy(begin, tmp, ptr_dist(end, begin));                                                                      \
        break;                                                                                                         \
      }                                                                                                                \
      end = (r = tmp) + length;                                                                                        \
      do {                                                                                                             \
        const size_t key = EXTRACT_KEY(r) >> key_shift;                                                                \
        begin[counters.b[(key >> 8) & 255]++] = *r;                                                                    \
      } while (++r != end);                                                                                            \
                                                                                                                       \
      key_shift += 16;                                                                                                 \
    } while (key_diff_mask >> 16);                                                                                     \
                                                                                                                       \
    if (!(BUFFER_PREALLOCATED))                                                                                        \
      osal_free(tmp);                                                                                                  \
    return true;                                                                                                       \
  }

/*------------------------------------------------------------------------------
 * LY: Binary search */

#if defined(__clang__) && __clang_major__ > 4 && defined(__ia32__)
#define WORKAROUND_FOR_CLANG_OPTIMIZER_BUG(size, flag)                                                                 \
  do                                                                                                                   \
    __asm __volatile(""                                                                                                \
                     : "+r"(size)                                                                                      \
                     : "r" /* the `b` constraint is more suitable here, but                                            \
                              cause CLANG to allocate and push/pop an one more                                         \
                              register, so using the `r` which avoids this. */                                         \
                     (flag));                                                                                          \
  while (0)
#else
#define WORKAROUND_FOR_CLANG_OPTIMIZER_BUG(size, flag)                                                                 \
  do {                                                                                                                 \
    /* nope for non-clang or non-x86 */;                                                                               \
  } while (0)
#endif /* Workaround for CLANG */

#define SEARCH_IMPL(NAME, TYPE_LIST, TYPE_ARG, CMP)                            \
  static __always_inline const TYPE_LIST *NAME(                                \
      const TYPE_LIST *it, size_t length, const TYPE_ARG item) {               \
    const TYPE_LIST *const begin = it, *const end = begin + length;            \
                                                                               \
    if (MDBX_HAVE_CMOV)                                                        \
      do {                                                                     \
        /* Адаптивно-упрощенный шаг двоичного поиска:                          \
         *  - без переходов при наличии cmov или аналога;                      \
         *  - допускает лишние итерации;                                       \
         *  - но ищет пока size > 2, что требует дозавершения поиска           \
         *    среди остающихся 0-1-2 элементов. */                             \
        const TYPE_LIST *const middle = it + (length >> 1);                    \
        length = (length + 1) >> 1;                                            \
        const bool flag = expect_with_probability(CMP(*middle, item), 0, .5);  \
        WORKAROUND_FOR_CLANG_OPTIMIZER_BUG(length, flag);                      \
        it = flag ? middle : it;                                               \
      } while (length > 2);                                                    \
    else                                                                       \
      while (length > 2) {                                                     \
        /* Вариант с использованием условного перехода. Основное отличие в     \
         * том, что при "не равно" (true от компаратора) переход делается на 1 \
         * ближе к концу массива. Алгоритмически это верно и обеспечивает      \
         * чуть-чуть более быструю сходимость, но зато требует больше          \
         * вычислений при true от компаратора. Также ВАЖНО(!) не допускается   \
         * спекулятивное выполнение при size == 0. */                          \
        const TYPE_LIST *const middle = it + (length >> 1);                    \
        length = (length + 1) >> 1;                                            \
        const bool flag = expect_with_probability(CMP(*middle, item), 0, .5);  \
        if (flag) {                                                            \
          it = middle + 1;                                                     \
          length -= 1;                                                         \
        }                                                                      \
      }                                                                        \
    it += length > 1 && expect_with_probability(CMP(*it, item), 0, .5);        \
    it += length > 0 && expect_with_probability(CMP(*it, item), 0, .5);        \
                                                                               \
    if (AUDIT_ENABLED()) {                                                     \
      for (const TYPE_LIST *scan = begin; scan < it; ++scan)                   \
        assert(CMP(*scan, item));                                              \
      for (const TYPE_LIST *scan = it; scan < end; ++scan)                     \
        assert(!CMP(*scan, item));                                             \
      (void)begin, (void)end;                                                  \
    }                                                                          \
                                                                               \
    return it;                                                                 \
  }

#endif /* __cplusplus */

#ifdef _MSC_VER
#pragma warning(pop)
#endif /* MSVC */
/// \copyright SPDX-License-Identifier: Apache-2.0
/// \author Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> \date 2020-2026
///
/// \brief Non-inline part of the libmdbx C++ API
///

#if !defined(MDBX_BUILD_CXX) || MDBX_BUILD_CXX != 1
#error "Build is misconfigured! Expecting MDBX_BUILD_CXX=1 for C++ API."
#endif /* MDBX_BUILD_CXX*/

/* Workaround for MSVC' header `extern "C"` vs `std::` redefinition bug */
#if defined(_MSC_VER)
#if defined(__SANITIZE_ADDRESS__) && !defined(_DISABLE_VECTOR_ANNOTATION)
#define _DISABLE_VECTOR_ANNOTATION
#endif /* _DISABLE_VECTOR_ANNOTATION */
#ifndef _SILENCE_EXPERIMENTAL_FILESYSTEM_DEPRECATION_WARNING
#define _SILENCE_EXPERIMENTAL_FILESYSTEM_DEPRECATION_WARNING
#endif /* #define _SILENCE_EXPERIMENTAL_FILESYSTEM_DEPRECATION_WARNING */
#endif /* _MSC_VER */

#include <array>
#include <atomic>
#include <cctype> // for isxdigit(), etc
#include <system_error>

namespace {

#if 0 /* Unused for now */

class trouble_location {

#ifndef TROUBLE_PROVIDE_LINENO
#define TROUBLE_PROVIDE_LINENO 1
#endif

#ifndef TROUBLE_PROVIDE_CONDITION
#define TROUBLE_PROVIDE_CONDITION 1
#endif

#ifndef TROUBLE_PROVIDE_FUNCTION
#define TROUBLE_PROVIDE_FUNCTION 1
#endif

#ifndef TROUBLE_PROVIDE_FILENAME
#define TROUBLE_PROVIDE_FILENAME 1
#endif

#if TROUBLE_PROVIDE_LINENO
  const unsigned line_;
#endif
#if TROUBLE_PROVIDE_CONDITION
  const char *const condition_;
#endif
#if TROUBLE_PROVIDE_FUNCTION
  const char *const function_;
#endif
#if TROUBLE_PROVIDE_FILENAME
  const char *const filename_;
#endif

public:
  MDBX_CXX11_CONSTEXPR trouble_location(unsigned line, const char *condition, const char *function,
                                        const char *filename)
      :
#if TROUBLE_PROVIDE_LINENO
        line_(line)
#endif
#if TROUBLE_PROVIDE_CONDITION
        ,
        condition_(condition)
#endif
#if TROUBLE_PROVIDE_FUNCTION
        ,
        function_(function)
#endif
#if TROUBLE_PROVIDE_FILENAME
        ,
        filename_(filename)
#endif
  {
#if !TROUBLE_PROVIDE_LINENO
    (void)line;
#endif
#if !TROUBLE_PROVIDE_CONDITION
    (void)condition;
#endif
#if !TROUBLE_PROVIDE_FUNCTION
    (void)function;
#endif
#if !TROUBLE_PROVIDE_FILENAME
    (void)filename;
#endif
  }

  trouble_location(const trouble_location &&) = delete;

  unsigned line() const {
#if TROUBLE_PROVIDE_LINENO
    return line_;
#else
    return 0;
#endif
  }

  const char *condition() const {
#if TROUBLE_PROVIDE_CONDITION
    return condition_;
#else
    return "";
#endif
  }

  const char *function() const {
#if TROUBLE_PROVIDE_FUNCTION
    return function_;
#else
    return "";
#endif
  }

  const char *filename() const {
#if TROUBLE_PROVIDE_FILENAME
    return filename_;
#else
    return "";
#endif
  }
};

//------------------------------------------------------------------------------

__cold std::string format_va(const char *fmt, va_list ap) {
  va_list ones;
  va_copy(ones, ap);
#ifdef _MSC_VER
  int needed = _vscprintf(fmt, ap);
#else
  int needed = vsnprintf(nullptr, 0, fmt, ap);
#endif
  assert(needed >= 0);
  std::string result;
  result.reserve(size_t(needed + 1));
  result.resize(size_t(needed), '\0');
  assert(int(result.capacity()) > needed);
  int actual = vsnprintf(const_cast<char *>(result.data()), result.capacity(), fmt, ones);
  assert(actual == needed);
  (void)actual;
  va_end(ones);
  return result;
}

__cold std::string format(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  std::string result = format_va(fmt, ap);
  va_end(ap);
  return result;
}

class bug : public std::runtime_error {
  const trouble_location &location_;

public:
  bug(const trouble_location &) noexcept;
  /* temporary workaround for "private field 'FOO' is not used" from CLANG
   * and for "function 'BAR' was declared but never referenced" from LCC. */
#ifndef __LCC__
  const trouble_location &location() const noexcept { return location_; }
#endif
  virtual ~bug() noexcept;
};

__cold bug::bug(const trouble_location &location) noexcept
    : std::runtime_error(format("mdbx.bug: %s.%s at %s:%u", location.function(), location.condition(),
                                location.filename(), location.line())),
      location_(location) {}

__cold bug::~bug() noexcept {}

[[maybe_unused, noreturn]] __cold void raise_bug(const trouble_location &what_and_where) { throw bug(what_and_where); }

#define RAISE_BUG(line, condition, function, file)                                                                     \
  do {                                                                                                                 \
    static MDBX_CXX11_CONSTEXPR_VAR trouble_location bug(line, condition, function, file);                             \
    raise_bug(bug);                                                                                                    \
  } while (0)

#undef ENSURE
#define ENSURE(condition)                                                                                              \
  do                                                                                                                   \
    if (MDBX_UNLIKELY(!(condition)))                                                                                   \
      MDBX_CXX20_UNLIKELY RAISE_BUG(__LINE__, #condition, __func__, __FILE__);                                         \
  while (0)

#define NOT_IMPLEMENTED() RAISE_BUG(__LINE__, "not_implemented", __func__, __FILE__);

#endif /* Unused*/

struct line_wrapper {
  char *line, *ptr;
  line_wrapper(char *buf) noexcept : line(buf), ptr(buf) {}
  void put(char c, size_t wrap_width) noexcept {
    *ptr++ = c;
    if (wrap_width && ptr >= wrap_width + line) {
      *ptr++ = '\n';
      line = ptr;
    }
  }
  void put(const ::mdbx::slice &chunk, size_t wrap_width) noexcept {
    if (!wrap_width || wrap_width > (ptr - line) + chunk.length()) {
      memcpy(ptr, chunk.data(), chunk.length());
      ptr += chunk.length();
    } else {
      for (size_t i = 0; i < chunk.length(); ++i)
        put(chunk.char_ptr()[i], wrap_width);
    }
  }
};

template <typename TYPE, unsigned INPLACE_BYTES = unsigned(sizeof(void *) * 64)> struct temp_buffer {
  TYPE inplace[(INPLACE_BYTES + sizeof(TYPE) - 1) / sizeof(TYPE)];
  const size_t size;
  TYPE *const area;
  temp_buffer(size_t bytes)
      : size((bytes + sizeof(TYPE) - 1) / sizeof(TYPE)), area((bytes > sizeof(inplace)) ? new TYPE[size] : inplace) {
    memset(area, 0, sizeof(TYPE) * size);
  }
  ~temp_buffer() {
    if (area != inplace)
      delete[] area;
  }
  TYPE *end() const { return area + size; }
};

} // namespace

#ifndef MDBX_CXX_ENDL
/* Манипулятор std::endl выталкивате буфферизированый вывод, что здесь не
 * требуется.
 *
 * Кроме этого, при сборке libmdbx для символов по-умолчанию выключается
 * видимость вне DSO, из-за чего обращение к std::endl иногда укачивает
 * линковщики, если комплятор ошибочно формируют direct access к global weak
 * symbol, коим является std::endl. */
#if 0
#define MDBX_CXX_ENDL ::std::endl
#else
#define MDBX_CXX_ENDL "\n"
#endif
#endif /* MDBX_CXX_ENDL */

//------------------------------------------------------------------------------

namespace mdbx {

[[noreturn]] __cold void throw_max_length_exceeded() {
  throw std::length_error("mdbx:: Exceeded the maximal length of data/slice/buffer.");
}

[[noreturn]] __cold void throw_too_small_target_buffer() {
  throw std::length_error("mdbx:: The target buffer is too small.");
}

[[noreturn]] __cold void throw_out_range() {
  throw std::out_of_range("mdbx:: Slice or buffer method was called with "
                          "an argument that exceeds the length.");
}

[[noreturn]] __cold void throw_allocators_mismatch() {
  throw std::logic_error("mdbx:: An allocators mismatch, so an object could not be transferred "
                         "into an incompatible memory allocation scheme.");
}

[[noreturn]] __cold void throw_incomparable_cursors() {
  throw std::logic_error("mdbx:: incomparable and/or invalid cursors to compare positions.");
}

[[noreturn]] __cold void throw_bad_value_size() { throw bad_value_size(MDBX_BAD_VALSIZE); }

__cold exception::exception(const ::mdbx::error &error) noexcept : base(error.what()), error_(error) {}

__cold exception::~exception() noexcept {}

static std::atomic_int fatal_countdown;

__cold fatal::fatal(const ::mdbx::error &error) noexcept : base(error) { ++fatal_countdown; }

__cold fatal::~fatal() noexcept {
  if (--fatal_countdown == 0)
    std::terminate();
}

#define DEFINE_EXCEPTION(NAME)                                                                                         \
  __cold NAME::NAME(const ::mdbx::error &rc) : exception(rc) {}                                                        \
  __cold NAME::~NAME() noexcept {}

DEFINE_EXCEPTION(bad_map_id)
DEFINE_EXCEPTION(bad_transaction)
DEFINE_EXCEPTION(bad_value_size)
DEFINE_EXCEPTION(db_corrupted)
DEFINE_EXCEPTION(db_full)
DEFINE_EXCEPTION(db_invalid)
DEFINE_EXCEPTION(db_too_large)
DEFINE_EXCEPTION(db_unable_extend)
DEFINE_EXCEPTION(db_version_mismatch)
DEFINE_EXCEPTION(db_wanna_write_for_recovery)
DEFINE_EXCEPTION(incompatible_operation)
DEFINE_EXCEPTION(internal_page_full)
DEFINE_EXCEPTION(internal_problem)
DEFINE_EXCEPTION(key_exists)
DEFINE_EXCEPTION(key_mismatch)
DEFINE_EXCEPTION(max_maps_reached)
DEFINE_EXCEPTION(max_readers_reached)
DEFINE_EXCEPTION(multivalue)
DEFINE_EXCEPTION(no_data)
DEFINE_EXCEPTION(not_found)
DEFINE_EXCEPTION(operation_not_permitted)
DEFINE_EXCEPTION(permission_denied_or_not_writeable)
DEFINE_EXCEPTION(reader_slot_busy)
DEFINE_EXCEPTION(remote_media)
DEFINE_EXCEPTION(something_busy)
DEFINE_EXCEPTION(thread_mismatch)
DEFINE_EXCEPTION(transaction_full)
DEFINE_EXCEPTION(transaction_overlapping)
DEFINE_EXCEPTION(duplicated_lck_file)
DEFINE_EXCEPTION(dangling_map_id)
DEFINE_EXCEPTION(transaction_ousted)
DEFINE_EXCEPTION(mvcc_retarded)
#undef DEFINE_EXCEPTION

__cold const char *error::what() const noexcept {
  if (is_mdbx_error())
    return mdbx_liberr2str(code());

  switch (code()) {
#define ERROR_CASE(CODE)                                                                                               \
  case CODE:                                                                                                           \
    return MDBX_STRINGIFY(CODE)
    ERROR_CASE(MDBX_ENODATA);
    ERROR_CASE(MDBX_EINVAL);
    ERROR_CASE(MDBX_EACCESS);
    ERROR_CASE(MDBX_ENOMEM);
    ERROR_CASE(MDBX_EROFS);
    ERROR_CASE(MDBX_ENOSYS);
    ERROR_CASE(MDBX_EIO);
    ERROR_CASE(MDBX_EPERM);
    ERROR_CASE(MDBX_EINTR);
    ERROR_CASE(MDBX_ENOFILE);
    ERROR_CASE(MDBX_EREMOTE);
    ERROR_CASE(MDBX_EDEADLK);
#undef ERROR_CASE
  default:
    return "SYSTEM";
  }
}

__cold std::string error::message() const {
  char buf[1024];
  const char *msg = ::mdbx_strerror_r(code(), buf, sizeof(buf));
  return std::string(msg ? msg : "unknown");
}

[[noreturn]] __cold void error::panic(const char *context, const char *func) const noexcept {
  assert(code() != MDBX_SUCCESS);
  ::mdbx_panic("mdbx::%s.%s(): \"%s\" (%d)", context, func, what(), code());
  std::terminate();
}

__cold void error::throw_exception() const {
  switch (code()) {
  case MDBX_EINVAL:
    throw std::invalid_argument("MDBX_EINVAL");
  case MDBX_ENOMEM:
    throw std::bad_alloc();
  case MDBX_SUCCESS:
    static_assert(MDBX_SUCCESS == MDBX_RESULT_FALSE, "WTF?");
    throw std::logic_error("MDBX_SUCCESS (MDBX_RESULT_FALSE)");
  case MDBX_RESULT_TRUE:
    throw std::logic_error("MDBX_RESULT_TRUE");
#define CASE_EXCEPTION(NAME, CODE)                                                                                     \
  case CODE:                                                                                                           \
    throw NAME(code())
    CASE_EXCEPTION(bad_map_id, MDBX_BAD_DBI);
    CASE_EXCEPTION(bad_transaction, MDBX_BAD_TXN);
    CASE_EXCEPTION(bad_value_size, MDBX_BAD_VALSIZE);
    CASE_EXCEPTION(db_corrupted, MDBX_CORRUPTED);
    CASE_EXCEPTION(db_corrupted, MDBX_CURSOR_FULL); /* branch-pages loop */
    CASE_EXCEPTION(db_corrupted, MDBX_PAGE_NOTFOUND);
    CASE_EXCEPTION(db_full, MDBX_MAP_FULL);
    CASE_EXCEPTION(db_invalid, MDBX_INVALID);
    CASE_EXCEPTION(db_too_large, MDBX_TOO_LARGE);
    CASE_EXCEPTION(db_unable_extend, MDBX_UNABLE_EXTEND_MAPSIZE);
    CASE_EXCEPTION(db_version_mismatch, MDBX_VERSION_MISMATCH);
    CASE_EXCEPTION(db_wanna_write_for_recovery, MDBX_WANNA_RECOVERY);
    CASE_EXCEPTION(fatal, MDBX_EBADSIGN);
    CASE_EXCEPTION(fatal, MDBX_PANIC);
    CASE_EXCEPTION(incompatible_operation, MDBX_INCOMPATIBLE);
    CASE_EXCEPTION(internal_page_full, MDBX_PAGE_FULL);
    CASE_EXCEPTION(internal_problem, MDBX_PROBLEM);
    CASE_EXCEPTION(key_exists, MDBX_KEYEXIST);
    CASE_EXCEPTION(key_mismatch, MDBX_EKEYMISMATCH);
    CASE_EXCEPTION(max_maps_reached, MDBX_DBS_FULL);
    CASE_EXCEPTION(max_readers_reached, MDBX_READERS_FULL);
    CASE_EXCEPTION(multivalue, MDBX_EMULTIVAL);
    CASE_EXCEPTION(no_data, MDBX_ENODATA);
    CASE_EXCEPTION(not_found, MDBX_NOTFOUND);
    CASE_EXCEPTION(operation_not_permitted, MDBX_EPERM);
    CASE_EXCEPTION(permission_denied_or_not_writeable, MDBX_EACCESS);
    CASE_EXCEPTION(reader_slot_busy, MDBX_BAD_RSLOT);
    CASE_EXCEPTION(remote_media, MDBX_EREMOTE);
    CASE_EXCEPTION(something_busy, MDBX_BUSY);
    CASE_EXCEPTION(thread_mismatch, MDBX_THREAD_MISMATCH);
    CASE_EXCEPTION(transaction_full, MDBX_TXN_FULL);
    CASE_EXCEPTION(transaction_overlapping, MDBX_TXN_OVERLAPPING);
    CASE_EXCEPTION(duplicated_lck_file, MDBX_DUPLICATED_CLK);
    CASE_EXCEPTION(dangling_map_id, MDBX_DANGLING_DBI);
    CASE_EXCEPTION(transaction_ousted, MDBX_OUSTED);
    CASE_EXCEPTION(mvcc_retarded, MDBX_MVCC_RETARDED);
#undef CASE_EXCEPTION
  default:
    if (is_mdbx_error())
      throw exception(*this);
    throw std::system_error(std::error_code(code(), std::system_category()));
  }
}

//------------------------------------------------------------------------------

bool slice::is_printable(bool disable_utf8) const noexcept {
  enum : byte {
    LS = 4,                     // shift for UTF8 sequence length
    P_ = 1 << LS,               // printable ASCII flag
    X_ = 1 << (LS - 1),         // printable extended ASCII flag
    N_ = 0,                     // non-printable ASCII
    second_range_mask = P_ - 1, // mask for range flag
    r80_BF = 0,                 // flag for UTF8 2nd byte range
    rA0_BF = 1,                 // flag for UTF8 2nd byte range
    r80_9F = 2,                 // flag for UTF8 2nd byte range
    r90_BF = 3,                 // flag for UTF8 2nd byte range
    r80_8F = 4,                 // flag for UTF8 2nd byte range

    // valid utf-8 byte sequences
    // http://www.unicode.org/versions/Unicode6.0.0/ch03.pdf - page 94
    //                        Code               | Bytes  |        |        |
    //                        Points             | 1st    | 2nd    | 3rd    |4th
    //                       --------------------|--------|--------|--------|---
    C2 = 2 << LS | r80_BF, // U+000080..U+0007FF | C2..DF | 80..BF |        |
    E0 = 3 << LS | rA0_BF, // U+000800..U+000FFF | E0     | A0..BF | 80..BF |
    E1 = 3 << LS | r80_BF, // U+001000..U+00CFFF | E1..EC | 80..BF | 80..BF |
    ED = 3 << LS | r80_9F, // U+00D000..U+00D7FF | ED     | 80..9F | 80..BF |
    EE = 3 << LS | r80_BF, // U+00E000..U+00FFFF | EE..EF | 80..BF | 80..BF |
    F0 = 4 << LS | r90_BF, // U+010000..U+03FFFF | F0     | 90..BF | 80..BF |...
    F1 = 4 << LS | r80_BF, // U+040000..U+0FFFFF | F1..F3 | 80..BF | 80..BF |...
    F4 = 4 << LS | r80_BF, // U+100000..U+10FFFF | F4     | 80..8F | 80..BF |...
  };

  static const byte range_from[] = {0x80, 0xA0, 0x80, 0x90, 0x80};
  static const byte range_to[] = {0xBF, 0xBF, 0x9F, 0xBF, 0x8F};

  static const byte map[256] = {
      //  1   2   3   4   5   6   7   8   9   a   b   c   d   e   f
      N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, // 00
      N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, N_, // 10
      P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, // 20
      P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, // 30
      P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, // 40
      P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, // 50
      P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, // 60
      P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, P_, N_, // 70
      N_, N_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, N_, X_, N_, // 80
      N_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, N_, X_, X_, // 90
      X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, // a0
      X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, // b0
      X_, X_, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, // c0
      C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, C2, // df
      E0, E1, E1, E1, E1, E1, E1, E1, E1, E1, E1, E1, E1, ED, EE, EE, // e0
      F0, F1, F1, F1, F4, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_, X_  // f0
  };

  if (MDBX_UNLIKELY(length() < 1))
    MDBX_CXX20_UNLIKELY return false;

  auto src = byte_ptr();
  const auto end = src + length();
  if (MDBX_UNLIKELY(disable_utf8)) {
    do
      if (MDBX_UNLIKELY(((P_ | X_) & map[*src]) == 0))
        MDBX_CXX20_UNLIKELY return false;
    while (++src < end);
    return true;
  }

  do {
    const auto bits = map[*src];
    const auto second_from = range_from[bits & second_range_mask];
    const auto second_to = range_to[bits & second_range_mask];
    switch (bits >> LS) {
    default:
      MDBX_CXX20_UNLIKELY return false;
    case 1:
      src += 1;
      continue;
    case 2:
      if (MDBX_UNLIKELY(src + 1 >= end))
        MDBX_CXX20_UNLIKELY return false;
      if (MDBX_UNLIKELY(src[1] < second_from || src[1] > second_to))
        MDBX_CXX20_UNLIKELY return false;
      src += 2;
      continue;
    case 3:
      if (MDBX_UNLIKELY(src + 3 >= end))
        MDBX_CXX20_UNLIKELY return false;
      if (MDBX_UNLIKELY(src[1] < second_from || src[1] > second_to))
        MDBX_CXX20_UNLIKELY return false;
      if (MDBX_UNLIKELY(src[2] < 0x80 || src[2] > 0xBF))
        MDBX_CXX20_UNLIKELY return false;
      src += 3;
      continue;
    case 4:
      if (MDBX_UNLIKELY(src + 4 >= end))
        MDBX_CXX20_UNLIKELY return false;
      if (MDBX_UNLIKELY(src[1] < second_from || src[1] > second_to))
        MDBX_CXX20_UNLIKELY return false;
      if (MDBX_UNLIKELY(src[2] < 0x80 || src[2] > 0xBF))
        MDBX_CXX20_UNLIKELY return false;
      if (MDBX_UNLIKELY(src[3] < 0x80 || src[3] > 0xBF))
        MDBX_CXX20_UNLIKELY return false;
      src += 4;
      continue;
    }
  } while (src < end);

  return true;
}

#ifdef MDBX_U128_TYPE
MDBX_U128_TYPE slice::as_uint128_adapt() const {
  static_assert(sizeof(MDBX_U128_TYPE) == 16, "WTF?");
  if (size() == 16) {
    MDBX_U128_TYPE r;
    memcpy(&r, data(), sizeof(r));
    return r;
  } else
    return as_uint64_adapt();
}
#endif /* MDBX_U128_TYPE */

uint64_t slice::as_uint64_adapt() const {
  static_assert(sizeof(uint64_t) == 8, "WTF?");
  if (size() == 8) {
    uint64_t r;
    memcpy(&r, data(), sizeof(r));
    return r;
  } else
    return as_uint32_adapt();
}

uint32_t slice::as_uint32_adapt() const {
  static_assert(sizeof(uint32_t) == 4, "WTF?");
  if (size() == 4) {
    uint32_t r;
    memcpy(&r, data(), sizeof(r));
    return r;
  } else
    return as_uint16_adapt();
}

uint16_t slice::as_uint16_adapt() const {
  static_assert(sizeof(uint16_t) == 2, "WTF?");
  if (size() == 2) {
    uint16_t r;
    memcpy(&r, data(), sizeof(r));
    return r;
  } else
    return as_uint8_adapt();
}

uint8_t slice::as_uint8_adapt() const {
  static_assert(sizeof(uint8_t) == 1, "WTF?");
  if (size() == 1)
    return *static_cast<const uint8_t *>(data());
  else if (size() == 0)
    return 0;
  else
    MDBX_CXX20_UNLIKELY throw_bad_value_size();
}

#ifdef MDBX_I128_TYPE
MDBX_I128_TYPE slice::as_int128_adapt() const {
  static_assert(sizeof(MDBX_I128_TYPE) == 16, "WTF?");
  if (size() == 16) {
    MDBX_I128_TYPE r;
    memcpy(&r, data(), sizeof(r));
    return r;
  } else
    return as_int64_adapt();
}
#endif /* MDBX_I128_TYPE */

int64_t slice::as_int64_adapt() const {
  static_assert(sizeof(int64_t) == 8, "WTF?");
  if (size() == 8) {
    uint64_t r;
    memcpy(&r, data(), sizeof(r));
    return r;
  } else
    return as_int32_adapt();
}

int32_t slice::as_int32_adapt() const {
  static_assert(sizeof(int32_t) == 4, "WTF?");
  if (size() == 4) {
    int32_t r;
    memcpy(&r, data(), sizeof(r));
    return r;
  } else
    return as_int16_adapt();
}

int16_t slice::as_int16_adapt() const {
  static_assert(sizeof(int16_t) == 2, "WTF?");
  if (size() == 2) {
    int16_t r;
    memcpy(&r, data(), sizeof(r));
    return r;
  } else
    return as_int8_adapt();
}

int8_t slice::as_int8_adapt() const {
  if (size() == 1)
    return *static_cast<const int8_t *>(data());
  else if (size() == 0)
    return 0;
  else
    MDBX_CXX20_UNLIKELY throw_bad_value_size();
}

//------------------------------------------------------------------------------

char *to_hex::write_bytes(char *__restrict const dest, size_t dest_size) const {
  if (MDBX_UNLIKELY(envisage_result_length() > dest_size))
    MDBX_CXX20_UNLIKELY throw_too_small_target_buffer();

  auto ptr = dest;
  auto src = source.byte_ptr();
  const char alpha_shift = (uppercase ? 'A' : 'a') - '9' - 1;
  auto line = ptr;
  for (const auto end = source.end_byte_ptr(); src != end; ++src) {
    if (wrap_width && size_t(ptr - line) >= wrap_width) {
      *ptr = '\n';
      line = ++ptr;
    }
    const int8_t hi = *src >> 4;
    const int8_t lo = *src & 15;
    ptr[0] = char('0' + hi + (((9 - hi) >> 7) & alpha_shift));
    ptr[1] = char('0' + lo + (((9 - lo) >> 7) & alpha_shift));
    ptr += 2;
    assert(ptr <= dest + dest_size);
  }
  return ptr;
}

::std::ostream &to_hex::output(::std::ostream &out) const {
  if (MDBX_LIKELY(!is_empty()))
    MDBX_CXX20_LIKELY {
      ::std::ostream::sentry sentry(out);
      auto src = source.byte_ptr();
      const char alpha_shift = (uppercase ? 'A' : 'a') - '9' - 1;
      unsigned width = 0;
      for (const auto end = source.end_byte_ptr(); src != end; ++src) {
        if (wrap_width && width >= wrap_width) {
          out << MDBX_CXX_ENDL;
          width = 0;
        }
        const int8_t hi = *src >> 4;
        const int8_t lo = *src & 15;
        out.put(char('0' + hi + (((9 - hi) >> 7) & alpha_shift)));
        out.put(char('0' + lo + (((9 - lo) >> 7) & alpha_shift)));
        width += 2;
      }
    }
  return out;
}

char *from_hex::write_bytes(char *__restrict const dest, size_t dest_size) const {
  if (MDBX_UNLIKELY(source.length() % 2 && !ignore_spaces))
    MDBX_CXX20_UNLIKELY throw std::domain_error("mdbx::from_hex:: odd length of hexadecimal string");
  if (MDBX_UNLIKELY(envisage_result_length() > dest_size))
    MDBX_CXX20_UNLIKELY throw_too_small_target_buffer();

  auto ptr = dest;
  auto src = source.byte_ptr();
  for (auto left = source.length(); left > 0;) {
    if (MDBX_UNLIKELY(*src <= ' ') && MDBX_LIKELY(ignore_spaces && isspace(*src))) {
      ++src;
      --left;
      continue;
    }

    if (MDBX_UNLIKELY(left < 1 || !isxdigit(src[0]) || !isxdigit(src[1])))
      MDBX_CXX20_UNLIKELY throw std::domain_error("mdbx::from_hex:: invalid hexadecimal string");

    int8_t hi = src[0];
    hi = (hi | 0x20) - 'a';
    hi += 10 + ((hi >> 7) & 39);

    int8_t lo = src[1];
    lo = (lo | 0x20) - 'a';
    lo += 10 + ((lo >> 7) & 39);

    *ptr++ = hi << 4 | lo;
    src += 2;
    left -= 2;
    assert(ptr <= dest + dest_size);
  }
  return ptr;
}

bool from_hex::is_erroneous() const noexcept {
  if (MDBX_UNLIKELY(source.length() % 2 && !ignore_spaces))
    MDBX_CXX20_UNLIKELY return true;

  bool got = false;
  auto src = source.byte_ptr();
  for (auto left = source.length(); left > 0;) {
    if (MDBX_UNLIKELY(*src <= ' ') && MDBX_LIKELY(ignore_spaces && isspace(*src))) {
      ++src;
      --left;
      continue;
    }

    if (MDBX_UNLIKELY(left < 1 || !isxdigit(src[0]) || !isxdigit(src[1])))
      MDBX_CXX20_UNLIKELY return true;

    got = true;
    src += 2;
    left -= 2;
  }
  return !got;
}

//------------------------------------------------------------------------------

enum : signed char {
  OO /* ASCII NUL */ = -8,
  EQ /* BASE64 '=' pad */ = -4,
  SP /* SPACE */ = -2,
  IL /* invalid */ = -1
};

#if MDBX_WORDBITS > 32
using b58_uint = uint_fast64_t;
#else
using b58_uint = uint_fast32_t;
#endif

struct b58_buffer : public temp_buffer<b58_uint> {
  b58_buffer(size_t bytes, size_t estimation_ratio_numerator, size_t estimation_ratio_denominator, size_t extra = 0)
      : temp_buffer(
            (/* пересчитываем по указанной пропорции */
             bytes =
                 (bytes * estimation_ratio_numerator + estimation_ratio_denominator - 1) / estimation_ratio_denominator,
             /* учитываем резервный старший байт в каждом слове */
             ((bytes + sizeof(b58_uint) - 2) / (sizeof(b58_uint) - 1) * sizeof(b58_uint) + extra) * sizeof(b58_uint))) {
  }
};

static byte b58_8to11(b58_uint &v) noexcept {
  static const char b58_alphabet[58] = {'1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
                                        'G', 'H', 'J', 'K', 'L', 'M', 'N', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W',
                                        'X', 'Y', 'Z', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'm',
                                        'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z'};

  const auto i = size_t(v % 58);
  v /= 58;
  return b58_alphabet[i];
}

static slice b58_encode(b58_buffer &buf, const byte *begin, const byte *end) {
  auto high = buf.end();
  const auto modulo = b58_uint((sizeof(b58_uint) > 4) ? UINT64_C(0x1A636A90B07A00) /* 58^9 */
                                                      : UINT32_C(0xACAD10) /* 58^4 */);
  static_assert(sizeof(modulo) == 4 || sizeof(modulo) == 8, "WTF?");
  while (begin < end) {
    b58_uint carry = *begin++;
    auto ptr = buf.end();
    do {
      assert(ptr > buf.area);
      carry += *--ptr << CHAR_BIT;
      *ptr = carry % modulo;
      carry /= modulo;
    } while (carry || ptr > high);
    high = ptr;
  }

  byte *output = static_cast<byte *>(static_cast<void *>(buf.area));
  auto ptr = output;
  for (auto porous = high; porous < buf.end();) {
    auto chunk = *porous++;
    static_assert(sizeof(chunk) == 4 || sizeof(chunk) == 8, "WTF?");
    assert(chunk < modulo);
    if (sizeof(chunk) > 4) {
      ptr[8] = b58_8to11(chunk);
      ptr[7] = b58_8to11(chunk);
      ptr[6] = b58_8to11(chunk);
      ptr[5] = b58_8to11(chunk);
      ptr[4] = b58_8to11(chunk);
      ptr[3] = b58_8to11(chunk);
      ptr[2] = b58_8to11(chunk);
      ptr[1] = b58_8to11(chunk);
      ptr[0] = b58_8to11(chunk);
      ptr += 9;
    } else {
      ptr[3] = b58_8to11(chunk);
      ptr[2] = b58_8to11(chunk);
      ptr[1] = b58_8to11(chunk);
      ptr[0] = b58_8to11(chunk);
      ptr += 4;
    }
    assert(static_cast<void *>(ptr) < static_cast<void *>(porous));
  }

  while (output < ptr && *output == '1')
    ++output;
  return slice(output, ptr);
}

char *to_base58::write_bytes(char *__restrict const dest, size_t dest_size) const {
  if (MDBX_UNLIKELY(envisage_result_length() > dest_size))
    MDBX_CXX20_UNLIKELY throw_too_small_target_buffer();

  auto begin = source.byte_ptr();
  auto end = source.end_byte_ptr();
  line_wrapper wrapper(dest);
  while (MDBX_LIKELY(begin < end) && *begin == 0) {
    wrapper.put('1', wrap_width);
    assert(wrapper.ptr <= dest + dest_size);
    ++begin;
  }

  b58_buffer buf(end - begin, 11, 8);
  wrapper.put(b58_encode(buf, begin, end), wrap_width);
  return wrapper.ptr;
}

::std::ostream &to_base58::output(::std::ostream &out) const {
  if (MDBX_LIKELY(!is_empty()))
    MDBX_CXX20_LIKELY {
      ::std::ostream::sentry sentry(out);
      auto begin = source.byte_ptr();
      auto end = source.end_byte_ptr();
      unsigned width = 0;
      while (MDBX_LIKELY(begin < end) && *begin == 0) {
        out.put('1');
        if (wrap_width && ++width >= wrap_width) {
          out << MDBX_CXX_ENDL;
          width = 0;
        }
        ++begin;
      }

      b58_buffer buf(end - begin, 11, 8);
      const auto chunk = b58_encode(buf, begin, end);
      if (!wrap_width || wrap_width > width + chunk.length())
        out.write(chunk.char_ptr(), chunk.length());
      else {
        for (size_t i = 0; i < chunk.length(); ++i) {
          out.put(chunk.char_ptr()[i]);
          if (wrap_width && ++width >= wrap_width) {
            out << MDBX_CXX_ENDL;
            width = 0;
          }
        }
      }
    }
  return out;
}

const signed char b58_map[256] = {
    //   1   2   3   4   5   6   7   8   9   a   b   c   d   e   f
    OO, IL, IL, IL, IL, IL, IL, IL, IL, SP, SP, SP, SP, SP, IL, IL, // 00
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // 10
    SP, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // 20
    IL, 0,  1,  2,  3,  4,  5,  6,  7,  8,  IL, IL, IL, IL, IL, IL, // 30
    IL, 9,  10, 11, 12, 13, 14, 15, 16, IL, 17, 18, 19, 20, 21, IL, // 40
    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, IL, IL, IL, IL, IL, // 50
    IL, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, IL, 44, 45, 46, // 60
    47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, IL, IL, IL, IL, IL, // 70
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // 80
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // 90
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // a0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // b0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // c0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // d0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // e0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL  // f0
};

static slice b58_decode(b58_buffer &buf, const byte *begin, const byte *end, bool ignore_spaces) {
  auto high = buf.end();
  while (begin < end) {
    const auto c = b58_map[*begin++];
    if (MDBX_LIKELY(c >= 0)) {
      b58_uint carry = c;
      auto ptr = buf.end();
      do {
        assert(ptr > buf.area);
        carry += *--ptr * 58;
        *ptr = carry & (~b58_uint(0) >> CHAR_BIT);
        carry >>= CHAR_BIT * (sizeof(carry) - 1);
      } while (carry || ptr > high);
      high = ptr;
    } else if (MDBX_UNLIKELY(!ignore_spaces || !isspace(begin[-1])))
      MDBX_CXX20_UNLIKELY
    throw std::domain_error("mdbx::from_base58:: invalid base58 string");
  }

  byte *output = static_cast<byte *>(static_cast<void *>(buf.area));
  auto ptr = output;
  for (auto porous = high; porous < buf.end(); ++porous) {
    auto chunk = *porous;
    static_assert(sizeof(chunk) == 4 || sizeof(chunk) == 8, "WTF?");
    assert(chunk <= (~b58_uint(0) >> CHAR_BIT));
    if (sizeof(chunk) > 4) {
      *ptr++ = byte(uint_fast64_t(chunk) >> CHAR_BIT * 6);
      *ptr++ = byte(uint_fast64_t(chunk) >> CHAR_BIT * 5);
      *ptr++ = byte(uint_fast64_t(chunk) >> CHAR_BIT * 4);
      *ptr++ = byte(chunk >> CHAR_BIT * 3);
    }
    *ptr++ = byte(chunk >> CHAR_BIT * 2);
    *ptr++ = byte(chunk >> CHAR_BIT * 1);
    *ptr++ = byte(chunk >> CHAR_BIT * 0);
  }

  while (output < ptr && *output == 0)
    ++output;
  return slice(output, ptr);
}

char *from_base58::write_bytes(char *__restrict const dest, size_t dest_size) const {
  if (MDBX_UNLIKELY(envisage_result_length() > dest_size))
    MDBX_CXX20_UNLIKELY throw_too_small_target_buffer();

  auto ptr = dest;
  auto begin = source.byte_ptr();
  auto const end = source.end_byte_ptr();
  while (begin < end && *begin <= '1') {
    if (MDBX_LIKELY(*begin == '1'))
      MDBX_CXX20_LIKELY *ptr++ = 0;
    else if (MDBX_UNLIKELY(!ignore_spaces || !isspace(*begin)))
      MDBX_CXX20_UNLIKELY
    throw std::domain_error("mdbx::from_base58:: invalid base58 string");
    ++begin;
  }

  b58_buffer buf(end - begin, 47, 64);
  auto slice = b58_decode(buf, begin, end, ignore_spaces);
  memcpy(ptr, slice.data(), slice.length());
  return ptr + slice.length();
}

bool from_base58::is_erroneous() const noexcept {
  auto begin = source.byte_ptr();
  auto const end = source.end_byte_ptr();
  while (begin < end) {
    if (MDBX_UNLIKELY(b58_map[*begin] < 0 && !(ignore_spaces && isspace(*begin))))
      return true;
    ++begin;
  }
  return false;
}

//------------------------------------------------------------------------------

static inline void b64_3to4(const byte x, const byte y, const byte z, char *__restrict dest) noexcept {
  static const byte alphabet[64] = {'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P',
                                    'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', 'a', 'b', 'c', 'd', 'e', 'f',
                                    'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v',
                                    'w', 'x', 'y', 'z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '+', '/'};
  dest[0] = alphabet[(x & 0xfc) >> 2];
  dest[1] = alphabet[((x & 0x03) << 4) + ((y & 0xf0) >> 4)];
  dest[2] = alphabet[((y & 0x0f) << 2) + ((z & 0xc0) >> 6)];
  dest[3] = alphabet[z & 0x3f];
}

char *to_base64::write_bytes(char *__restrict const dest, size_t dest_size) const {
  if (MDBX_UNLIKELY(envisage_result_length() > dest_size))
    MDBX_CXX20_UNLIKELY throw_too_small_target_buffer();

  auto ptr = dest;
  auto src = source.byte_ptr();
  size_t left = source.length();
  auto line = ptr;
  while (true) {
    switch (left) {
    default:
      MDBX_CXX20_LIKELY left -= 3;
      b64_3to4(src[0], src[1], src[2], ptr);
      ptr += 4;
      src += 3;
      if (wrap_width && size_t(ptr - line) >= wrap_width && left) {
        *ptr = '\n';
        line = ++ptr;
      }
      assert(ptr <= dest + dest_size);
      continue;
    case 2:
      b64_3to4(src[0], src[1], 0, ptr);
      ptr[3] = '=';
      assert(ptr + 4 <= dest + dest_size);
      return ptr + 4;
    case 1:
      b64_3to4(src[0], 0, 0, ptr);
      ptr[2] = ptr[3] = '=';
      assert(ptr + 4 <= dest + dest_size);
      return ptr + 4;
    case 0:
      return ptr;
    }
  }
}

::std::ostream &to_base64::output(::std::ostream &out) const {
  if (MDBX_LIKELY(!is_empty()))
    MDBX_CXX20_LIKELY {
      ::std::ostream::sentry sentry(out);
      auto src = source.byte_ptr();
      size_t left = source.length();
      unsigned width = 0;
      std::array<char, 4> buf;

      while (true) {
        switch (left) {
        default:
          MDBX_CXX20_LIKELY left -= 3;
          b64_3to4(src[0], src[1], src[2], &buf.front());
          src += 3;
          out.write(&buf.front(), 4);
          if (wrap_width && (width += 4) >= wrap_width && left) {
            out << MDBX_CXX_ENDL;
            width = 0;
          }
          continue;
        case 2:
          b64_3to4(src[0], src[1], 0, &buf.front());
          buf[3] = '=';
          return out.write(&buf.front(), 4);
        case 1:
          b64_3to4(src[0], 0, 0, &buf.front());
          buf[2] = buf[3] = '=';
          return out.write(&buf.front(), 4);
        case 0:
          return out;
        }
      }
    }
  return out;
}

static const signed char b64_map[256] = {
    //   1   2   3   4   5   6   7   8   9   a   b   c   d   e   f
    OO, IL, IL, IL, IL, IL, IL, IL, IL, SP, SP, SP, SP, SP, IL, IL, // 00
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // 10
    SP, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, 62, IL, IL, IL, 63, // 20
    52, 53, 54, 55, 56, 57, 58, 59, 60, 61, IL, IL, IL, EQ, IL, IL, // 30
    IL, 0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, // 40
    15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, IL, IL, IL, IL, IL, // 50
    IL, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, // 60
    41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, IL, IL, IL, IL, IL, // 70
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // 80
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // 90
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // a0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // b0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // c0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // d0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, // e0
    IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL, IL  // f0
};

static inline signed char b64_4to3(signed char a, signed char b, signed char c, signed char d,
                                   char *__restrict dest) noexcept {
  dest[0] = byte((a << 2) + ((b & 0x30) >> 4));
  dest[1] = byte(((b & 0xf) << 4) + ((c & 0x3c) >> 2));
  dest[2] = byte(((c & 0x3) << 6) + d);
  return a | b | c | d;
}

char *from_base64::write_bytes(char *__restrict const dest, size_t dest_size) const {
  if (MDBX_UNLIKELY(source.length() % 4 && !ignore_spaces))
    MDBX_CXX20_UNLIKELY throw std::domain_error("mdbx::from_base64:: odd length of base64 string");
  if (MDBX_UNLIKELY(envisage_result_length() > dest_size))
    MDBX_CXX20_UNLIKELY throw_too_small_target_buffer();

  auto ptr = dest;
  auto src = source.byte_ptr();
  for (auto left = source.length(); left > 0;) {
    if (MDBX_UNLIKELY(*src <= ' ') && MDBX_LIKELY(ignore_spaces && isspace(*src))) {
      ++src;
      --left;
      continue;
    }

    if (MDBX_UNLIKELY(left < 3))
      MDBX_CXX20_UNLIKELY {
      bailout:
        throw std::domain_error("mdbx::from_base64:: invalid base64 string");
      }
    const signed char a = b64_map[src[0]], b = b64_map[src[1]], c = b64_map[src[2]], d = b64_map[src[3]];
    if (MDBX_UNLIKELY(b64_4to3(a, b, c, d, ptr) < 0)) {
      if (left == 4 && (a | b) >= 0 && d == EQ) {
        if (c >= 0) {
          assert(ptr + 2 <= dest + dest_size);
          return ptr + 2;
        }
        if (c == d) {
          assert(ptr + 1 <= dest + dest_size);
          return ptr + 1;
        }
      }
      MDBX_CXX20_UNLIKELY goto bailout;
    }
    src += 4;
    left -= 4;
    ptr += 3;
    assert(ptr <= dest + dest_size);
  }
  return ptr;
}

bool from_base64::is_erroneous() const noexcept {
  if (MDBX_UNLIKELY(source.length() % 4 && !ignore_spaces))
    MDBX_CXX20_UNLIKELY return true;

  bool got = false;
  auto src = source.byte_ptr();
  for (auto left = source.length(); left > 0;) {
    if (MDBX_UNLIKELY(*src <= ' ') && MDBX_LIKELY(ignore_spaces && isspace(*src))) {
      ++src;
      --left;
      continue;
    }

    if (MDBX_UNLIKELY(left < 3))
      MDBX_CXX20_UNLIKELY return false;
    const signed char a = b64_map[src[0]], b = b64_map[src[1]], c = b64_map[src[2]], d = b64_map[src[3]];
    if (MDBX_UNLIKELY((a | b | c | d) < 0))
      MDBX_CXX20_UNLIKELY {
        if (left == 4 && (a | b) >= 0 && d == EQ && (c >= 0 || c == d))
          return false;
        return true;
      }
    got = true;
    src += 4;
    left -= 4;
  }
  return !got;
}

//------------------------------------------------------------------------------

#if defined(_MSC_VER)
#pragma warning(push)
/* warning C4251: 'mdbx::buffer<...>::silo_':
 *   struct 'mdbx::buffer<..>::silo' needs to have dll-interface to be used by clients of class 'mdbx::buffer<...>'
 *
 * Microsoft не хочет признавать ошибки и пересматривать приятные решения, поэтому MSVC продолжает кошмарить
 * и стращать разработчиков предупреждениями, тем самым перекладывая ответственность на их плечи.
 *
 * В данном случае предупреждение выдаётся из-за инстанцирования std::string::allocator_type::pointer и
 * std::pmr::string::allocator_type::pointer внутри mdbx::buffer<..>::silo. А так как эти типы являются частью
 * стандартной библиотеки C++ они всегда будут доступны и без необходимости их инстанцирования и экспорта из libmdbx.
 *
 * Поэтому нет других вариантов как заглушить это предупреждение и еще раз плюнуть в сторону microsoft. */
#pragma warning(disable : 4251)
#endif /* MSVC */

MDBX_INSTALL_API_TEMPLATE(LIBMDBX_API_TYPE, buffer<legacy_allocator, default_capacity_policy>);

#if MDBX_CXX_HAS_POLYMORPHIC_ALLOCATOR
MDBX_INSTALL_API_TEMPLATE(LIBMDBX_API_TYPE, buffer<polymorphic_allocator, default_capacity_policy>);
#endif /* MDBX_CXX_HAS_POLYMORPHIC_ALLOCATOR */

#if defined(_MSC_VER)
#pragma warning(pop)
#endif /* MSVC */

//------------------------------------------------------------------------------

static inline MDBX_env_flags_t mode2flags(env::mode mode) {
  switch (mode) {
  default:
    MDBX_CXX20_UNLIKELY throw std::invalid_argument("db::mode is invalid");
  case env::mode::readonly:
    return MDBX_RDONLY;
  case env::mode::write_file_io:
    return MDBX_ENV_DEFAULTS;
  case env::mode::write_mapped_io:
    return MDBX_WRITEMAP;
  }
}

__cold MDBX_env_flags_t env::operate_parameters::make_flags(bool accede, bool use_subdirectory) const {
  MDBX_env_flags_t flags = mode2flags(mode);
  if (accede)
    flags |= MDBX_ACCEDE;
  if (!use_subdirectory)
    flags |= MDBX_NOSUBDIR;
  if (options.exclusive)
    flags |= MDBX_EXCLUSIVE;
  if (options.no_sticky_threads)
    flags |= MDBX_NOSTICKYTHREADS;
  if (options.disable_readahead)
    flags |= MDBX_NORDAHEAD;
  if (options.disable_clear_memory)
    flags |= MDBX_NOMEMINIT;
  if (options.enable_validation)
    flags |= MDBX_VALIDATION;

  if (mode != readonly) {
    if (options.nested_write_transactions)
      flags &= ~MDBX_WRITEMAP;
    if (reclaiming.coalesce)
      flags |= MDBX_COALESCE;
    if (reclaiming.lifo)
      flags |= MDBX_LIFORECLAIM;
    switch (durability) {
    default:
      MDBX_CXX20_UNLIKELY throw std::invalid_argument("db::durability is invalid");
    case env::durability::robust_synchronous:
      break;
    case env::durability::half_synchronous_weak_last:
      flags |= MDBX_NOMETASYNC;
      break;
    case env::durability::lazy_weak_tail:
      static_assert(MDBX_MAPASYNC == MDBX_SAFE_NOSYNC, "WTF? Obsolete C API?");
      flags |= MDBX_SAFE_NOSYNC;
      break;
    case env::durability::whole_fragile:
      flags |= MDBX_UTTERLY_NOSYNC;
      break;
    }
  }
  return flags;
}

env::mode env::operate_parameters::mode_from_flags(MDBX_env_flags_t flags) noexcept {
  if (flags & MDBX_RDONLY)
    return env::mode::readonly;
  return (flags & MDBX_WRITEMAP) ? env::mode::write_mapped_io : env::mode::write_file_io;
}

env::durability env::operate_parameters::durability_from_flags(MDBX_env_flags_t flags) noexcept {
  if ((flags & MDBX_UTTERLY_NOSYNC) == MDBX_UTTERLY_NOSYNC)
    return env::durability::whole_fragile;
  if (flags & MDBX_SAFE_NOSYNC)
    return env::durability::lazy_weak_tail;
  if (flags & MDBX_NOMETASYNC)
    return env::durability::half_synchronous_weak_last;
  return env::durability::robust_synchronous;
}

env::reclaiming_options::reclaiming_options(MDBX_env_flags_t flags) noexcept
    : lifo((flags & MDBX_LIFORECLAIM) ? true : false), coalesce((flags & MDBX_COALESCE) ? true : false) {}

env::operate_options::operate_options(MDBX_env_flags_t flags) noexcept
    : no_sticky_threads(((flags & (MDBX_NOSTICKYTHREADS | MDBX_EXCLUSIVE)) == MDBX_NOSTICKYTHREADS) ? true : false),
      nested_write_transactions((flags & (MDBX_WRITEMAP | MDBX_RDONLY)) ? false : true),
      exclusive((flags & MDBX_EXCLUSIVE) ? true : false), disable_readahead((flags & MDBX_NORDAHEAD) ? true : false),
      disable_clear_memory((flags & MDBX_NOMEMINIT) ? true : false) {}

bool env::is_pristine() const { return get_stat().ms_mod_txnid == 0 && get_info().mi_recent_txnid == INITIAL_TXNID; }

bool env::is_empty() const { return get_stat().ms_leaf_pages == 0; }

__cold env &env::copy(filehandle fd, bool compactify, bool force_dynamic_size) {
  error::success_or_throw(::mdbx_env_copy2fd(handle_, fd,
                                             (compactify ? MDBX_CP_COMPACT : MDBX_CP_DEFAULTS) |
                                                 (force_dynamic_size ? MDBX_CP_FORCE_DYNAMIC_SIZE : MDBX_CP_DEFAULTS)));
  return *this;
}

__cold env &env::copy(const char *destination, bool compactify, bool force_dynamic_size) {
  error::success_or_throw(::mdbx_env_copy(handle_, destination,
                                          (compactify ? MDBX_CP_COMPACT : MDBX_CP_DEFAULTS) |
                                              (force_dynamic_size ? MDBX_CP_FORCE_DYNAMIC_SIZE : MDBX_CP_DEFAULTS)));
  return *this;
}

__cold env &env::copy(const ::std::string &destination, bool compactify, bool force_dynamic_size) {
  return copy(destination.c_str(), compactify, force_dynamic_size);
}

#if defined(_WIN32) || defined(_WIN64)
__cold env &env::copy(const wchar_t *destination, bool compactify, bool force_dynamic_size) {
  error::success_or_throw(::mdbx_env_copyW(handle_, destination,
                                           (compactify ? MDBX_CP_COMPACT : MDBX_CP_DEFAULTS) |
                                               (force_dynamic_size ? MDBX_CP_FORCE_DYNAMIC_SIZE : MDBX_CP_DEFAULTS)));
  return *this;
}

env &env::copy(const ::std::wstring &destination, bool compactify, bool force_dynamic_size) {
  return copy(destination.c_str(), compactify, force_dynamic_size);
}
#endif /* Windows */

#ifdef MDBX_STD_FILESYSTEM_PATH
__cold env &env::copy(const MDBX_STD_FILESYSTEM_PATH &destination, bool compactify, bool force_dynamic_size) {
  return copy(destination.native(), compactify, force_dynamic_size);
}
#endif /* MDBX_STD_FILESYSTEM_PATH */

__cold path env::get_path() const {
#if defined(_WIN32) || defined(_WIN64)
  const wchar_t *c_wstr = nullptr;
  error::success_or_throw(::mdbx_env_get_pathW(handle_, &c_wstr));
  static_assert(sizeof(path::value_type) == sizeof(wchar_t), "Oops");
  return path(c_wstr);
#else
  const char *c_str = nullptr;
  error::success_or_throw(::mdbx_env_get_path(handle_, &c_str));
  static_assert(sizeof(path::value_type) == sizeof(char), "Oops");
  return path(c_str);
#endif
}

__cold bool env::remove(const char *pathname, const remove_mode mode) {
  return !error::boolean_or_throw(::mdbx_env_delete(pathname, MDBX_env_delete_mode_t(mode)));
}

__cold bool env::remove(const ::std::string &pathname, const remove_mode mode) {
  return remove(pathname.c_str(), mode);
}

#if defined(_WIN32) || defined(_WIN64)
__cold bool env::remove(const wchar_t *pathname, const remove_mode mode) {
  return !error::boolean_or_throw(::mdbx_env_deleteW(pathname, MDBX_env_delete_mode_t(mode)));
}

__cold bool env::remove(const ::std::wstring &pathname, const remove_mode mode) {
  return remove(pathname.c_str(), mode);
}
#endif /* Windows */

#ifdef MDBX_STD_FILESYSTEM_PATH
__cold bool env::remove(const MDBX_STD_FILESYSTEM_PATH &pathname, const remove_mode mode) {
  return remove(pathname.native(), mode);
}
#endif /* MDBX_STD_FILESYSTEM_PATH */

//------------------------------------------------------------------------------

static inline MDBX_env *create_env() {
  MDBX_env *ptr;
  error::success_or_throw(::mdbx_env_create(&ptr));
  assert(ptr != nullptr);
  return ptr;
}

__cold env_managed::~env_managed() noexcept {
  if (MDBX_UNLIKELY(handle_))
    MDBX_CXX20_UNLIKELY error::success_or_panic(::mdbx_env_close(handle_), "mdbx::~env()", "mdbx_env_close");
}

__cold void env_managed::close(bool dont_sync) {
  const error rc = static_cast<MDBX_error_t>(::mdbx_env_close_ex(handle_, dont_sync));
  switch (rc.code()) {
  case MDBX_EBADSIGN:
    MDBX_CXX20_UNLIKELY handle_ = nullptr;
    __fallthrough /* fall through */;
  default:
    MDBX_CXX20_UNLIKELY rc.throw_exception();
  case MDBX_SUCCESS:
    MDBX_CXX20_LIKELY handle_ = nullptr;
  }
}

__cold void env_managed::setup(unsigned max_maps, unsigned max_readers) {
  if (max_readers > 0)
    error::success_or_throw(::mdbx_env_set_maxreaders(handle_, max_readers));
  if (max_maps > 0)
    error::success_or_throw(::mdbx_env_set_maxdbs(handle_, max_maps));
}

__cold env_managed::env_managed(const char *pathname, const operate_parameters &op, bool accede)
    : env_managed(create_env()) {
  setup(op.max_maps, op.max_readers);
  error::success_or_throw(::mdbx_env_open(handle_, pathname, op.make_flags(accede), 0));

  if (op.options.nested_write_transactions && !get_options().nested_write_transactions)
    MDBX_CXX20_UNLIKELY error::throw_exception(MDBX_INCOMPATIBLE);
}

__cold env_managed::env_managed(const char *pathname, const env_managed::create_parameters &cp,
                                const env::operate_parameters &op, bool accede)
    : env_managed(create_env()) {
  setup(op.max_maps, op.max_readers);
  set_geometry(cp.geometry);
  error::success_or_throw(
      ::mdbx_env_open(handle_, pathname, op.make_flags(accede, cp.use_subdirectory), cp.file_mode_bits));

  if (op.options.nested_write_transactions && !get_options().nested_write_transactions)
    MDBX_CXX20_UNLIKELY error::throw_exception(MDBX_INCOMPATIBLE);
}

__cold env_managed::env_managed(const ::std::string &pathname, const operate_parameters &op, bool accede)
    : env_managed(pathname.c_str(), op, accede) {}

__cold env_managed::env_managed(const ::std::string &pathname, const env_managed::create_parameters &cp,
                                const env::operate_parameters &op, bool accede)
    : env_managed(pathname.c_str(), cp, op, accede) {}

#if defined(_WIN32) || defined(_WIN64)
__cold env_managed::env_managed(const wchar_t *pathname, const operate_parameters &op, bool accede)
    : env_managed(create_env()) {
  setup(op.max_maps, op.max_readers);
  error::success_or_throw(::mdbx_env_openW(handle_, pathname, op.make_flags(accede), 0));

  if (op.options.nested_write_transactions && !get_options().nested_write_transactions)
    MDBX_CXX20_UNLIKELY error::throw_exception(MDBX_INCOMPATIBLE);
}

__cold env_managed::env_managed(const wchar_t *pathname, const env_managed::create_parameters &cp,
                                const env::operate_parameters &op, bool accede)
    : env_managed(create_env()) {
  setup(op.max_maps, op.max_readers);
  set_geometry(cp.geometry);
  error::success_or_throw(
      ::mdbx_env_openW(handle_, pathname, op.make_flags(accede, cp.use_subdirectory), cp.file_mode_bits));

  if (op.options.nested_write_transactions && !get_options().nested_write_transactions)
    MDBX_CXX20_UNLIKELY error::throw_exception(MDBX_INCOMPATIBLE);
}

__cold env_managed::env_managed(const ::std::wstring &pathname, const operate_parameters &op, bool accede)
    : env_managed(pathname.c_str(), op, accede) {}

__cold env_managed::env_managed(const ::std::wstring &pathname, const env_managed::create_parameters &cp,
                                const env::operate_parameters &op, bool accede)
    : env_managed(pathname.c_str(), cp, op, accede) {}
#endif /* Windows */

#ifdef MDBX_STD_FILESYSTEM_PATH
__cold env_managed::env_managed(const MDBX_STD_FILESYSTEM_PATH &pathname, const operate_parameters &op, bool accede)
    : env_managed(pathname.native(), op, accede) {}

__cold env_managed::env_managed(const MDBX_STD_FILESYSTEM_PATH &pathname, const env_managed::create_parameters &cp,
                                const env::operate_parameters &op, bool accede)
    : env_managed(pathname.native(), cp, op, accede) {}
#endif /* MDBX_STD_FILESYSTEM_PATH */

//------------------------------------------------------------------------------

txn_managed txn::start_nested() {
  MDBX_txn *nested;
  error::throw_on_nullptr(handle_, MDBX_BAD_TXN);
  error::success_or_throw(::mdbx_txn_begin(mdbx_txn_env(handle_), handle_, MDBX_TXN_READWRITE, &nested));
  assert(nested != nullptr);
  return txn_managed(nested);
}

txn_managed::~txn_managed() noexcept {
  if (MDBX_UNLIKELY(handle_))
    MDBX_CXX20_UNLIKELY error::success_or_panic(::mdbx_txn_abort(handle_), "mdbx::~txn", "mdbx_txn_abort");
}

void txn_managed::abort() {
  const error err = static_cast<MDBX_error_t>(::mdbx_txn_abort(handle_));
  if (MDBX_LIKELY(err.code() != MDBX_THREAD_MISMATCH))
    MDBX_CXX20_LIKELY handle_ = nullptr;
  if (MDBX_UNLIKELY(err.code() != MDBX_SUCCESS))
    MDBX_CXX20_UNLIKELY err.throw_exception();
}

void txn_managed::commit() {
  const error err = static_cast<MDBX_error_t>(::mdbx_txn_commit(handle_));
  if (MDBX_LIKELY(err.code() != MDBX_THREAD_MISMATCH))
    MDBX_CXX20_LIKELY handle_ = nullptr;
  if (MDBX_UNLIKELY(err.code() != MDBX_SUCCESS))
    MDBX_CXX20_UNLIKELY err.throw_exception();
}

void txn_managed::commit(commit_latency *latency) {
  const error err = static_cast<MDBX_error_t>(::mdbx_txn_commit_ex(handle_, latency));
  if (MDBX_LIKELY(err.code() != MDBX_THREAD_MISMATCH))
    MDBX_CXX20_LIKELY handle_ = nullptr;
  if (MDBX_UNLIKELY(err.code() != MDBX_SUCCESS))
    MDBX_CXX20_UNLIKELY err.throw_exception();
}

void txn_managed::commit_embark_read() {
  auto env = handle_->env;
  commit();
  error::success_or_throw(::mdbx_txn_begin(env, nullptr, MDBX_TXN_RDONLY, &handle_));
}

//------------------------------------------------------------------------------

__cold bool txn::drop_map(const char *name, bool throw_if_absent) {
  map_handle map;
  const int err = ::mdbx_dbi_open(handle_, name, MDBX_DB_ACCEDE, &map.dbi);
  switch (err) {
  case MDBX_SUCCESS:
    drop_map(map);
    return true;
  case MDBX_NOTFOUND:
  case MDBX_BAD_DBI:
    if (!throw_if_absent)
      return false;
    MDBX_CXX17_FALLTHROUGH /* fallthrough */;
  default:
    MDBX_CXX20_UNLIKELY error::throw_exception(err);
  }
}

__cold bool txn::clear_map(const char *name, bool throw_if_absent) {
  map_handle map;
  const int err = ::mdbx_dbi_open(handle_, name, MDBX_DB_ACCEDE, &map.dbi);
  switch (err) {
  case MDBX_SUCCESS:
    clear_map(map);
    return true;
  case MDBX_NOTFOUND:
  case MDBX_BAD_DBI:
    if (!throw_if_absent)
      return false;
    MDBX_CXX17_FALLTHROUGH /* fallthrough */;
  default:
    MDBX_CXX20_UNLIKELY error::throw_exception(err);
  }
}

__cold bool txn::rename_map(const char *old_name, const char *new_name, bool throw_if_absent) {
  map_handle map;
  const int err = ::mdbx_dbi_open(handle_, old_name, MDBX_DB_ACCEDE, &map.dbi);
  switch (err) {
  case MDBX_SUCCESS:
    rename_map(map, new_name);
    return true;
  case MDBX_NOTFOUND:
  case MDBX_BAD_DBI:
    if (!throw_if_absent)
      return false;
    MDBX_CXX17_FALLTHROUGH /* fallthrough */;
  default:
    MDBX_CXX20_UNLIKELY error::throw_exception(err);
  }
}

__cold bool txn::drop_map(const ::mdbx::slice &name, bool throw_if_absent) {
  map_handle map;
  const int err = ::mdbx_dbi_open2(handle_, name, MDBX_DB_ACCEDE, &map.dbi);
  switch (err) {
  case MDBX_SUCCESS:
    drop_map(map);
    return true;
  case MDBX_NOTFOUND:
  case MDBX_BAD_DBI:
    if (!throw_if_absent)
      return false;
    MDBX_CXX17_FALLTHROUGH /* fallthrough */;
  default:
    MDBX_CXX20_UNLIKELY error::throw_exception(err);
  }
}

__cold bool txn::clear_map(const ::mdbx::slice &name, bool throw_if_absent) {
  map_handle map;
  const int err = ::mdbx_dbi_open2(handle_, name, MDBX_DB_ACCEDE, &map.dbi);
  switch (err) {
  case MDBX_SUCCESS:
    clear_map(map);
    return true;
  case MDBX_NOTFOUND:
  case MDBX_BAD_DBI:
    if (!throw_if_absent)
      return false;
    MDBX_CXX17_FALLTHROUGH /* fallthrough */;
  default:
    MDBX_CXX20_UNLIKELY error::throw_exception(err);
  }
}

__cold bool txn::rename_map(const ::mdbx::slice &old_name, const ::mdbx::slice &new_name, bool throw_if_absent) {
  map_handle map;
  const int err = ::mdbx_dbi_open2(handle_, old_name, MDBX_DB_ACCEDE, &map.dbi);
  switch (err) {
  case MDBX_SUCCESS:
    rename_map(map, new_name);
    return true;
  case MDBX_NOTFOUND:
  case MDBX_BAD_DBI:
    if (!throw_if_absent)
      return false;
    MDBX_CXX17_FALLTHROUGH /* fallthrough */;
  default:
    MDBX_CXX20_UNLIKELY error::throw_exception(err);
  }
}

__cold bool txn::rename_map(const ::std::string &old_name, const ::std::string &new_name, bool throw_if_absent) {
  return rename_map(::mdbx::slice(old_name), ::mdbx::slice(new_name), throw_if_absent);
}

//------------------------------------------------------------------------------

void cursor::update_current(const slice &value) {
  default_buffer holder;
  auto key = current().key;
  if (error::boolean_or_throw(mdbx_is_dirty(handle_->txn, key.iov_base)))
    key = holder.assign(key);

  update(key, value);
}

slice cursor::reverse_current(size_t value_length) {
  default_buffer holder;
  auto key = current().key;
  if (error::boolean_or_throw(mdbx_is_dirty(handle_->txn, key.iov_base)))
    key = holder.assign(key);

  return update_reserve(key, value_length);
}

//------------------------------------------------------------------------------

__cold ::std::ostream &operator<<(::std::ostream &out, const slice &it) {
  out << "{";
  if (!it.is_valid())
    out << "INVALID." << it.length();
  else if (it.is_null())
    out << "NULL";
  else if (it.empty())
    out << "EMPTY->" << it.data();
  else {
    const slice head(it.head(std::min(it.length(), size_t(64))));
    out << it.length() << ".";
    if (head.is_printable())
      (out << "\"").write(head.char_ptr(), head.length()) << "\"";
    else
      out << to_hex(head);
    if (head.length() < it.length())
      out << "...";
  }
  return out << "}";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const pair &it) {
  return out << "{" << it.key << " => " << it.value << "}";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const pair_result &it) {
  return out << "{" << (it.done ? "done: " : "non-done: ") << it.key << " => " << it.value << "}";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const ::mdbx::env::geometry::size &it) {
  switch (it.bytes) {
  case ::mdbx::env::geometry::default_value:
    return out << "default";
  case ::mdbx::env::geometry::minimal_value:
    return out << "minimal";
  case ::mdbx::env::geometry::maximal_value:
    return out << "maximal";
  }

  const auto bytes = (it.bytes < 0) ? out << "-", size_t(-it.bytes) : size_t(it.bytes);
  struct {
    size_t one;
    const char *suffix;
  } static const scales[] = {
#if MDBX_WORDBITS > 32
      {env_managed::geometry::EiB, "EiB"},
      {env_managed::geometry::EB, "EB"},
      {env_managed::geometry::PiB, "PiB"},
      {env_managed::geometry::PB, "PB"},
      {env_managed::geometry::TiB, "TiB"},
      {env_managed::geometry::TB, "TB"},
#endif
      {env_managed::geometry::GiB, "GiB"},
      {env_managed::geometry::GB, "GB"},
      {env_managed::geometry::MiB, "MiB"},
      {env_managed::geometry::MB, "MB"},
      {env_managed::geometry::KiB, "KiB"},
      {env_managed::geometry::kB, "kB"},
      {1, " bytes"}};

  for (const auto i : scales)
    if (bytes % i.one == 0)
      return out << bytes / i.one << i.suffix;

  assert(false);
  __unreachable();
  return out;
}

__cold ::std::ostream &operator<<(::std::ostream &out, const env::geometry &it) {
  return                                                                //
      out << "\tlower " << env::geometry::size(it.size_lower)           //
          << ",\n\tnow " << env::geometry::size(it.size_now)            //
          << ",\n\tupper " << env::geometry::size(it.size_upper)        //
          << ",\n\tgrowth " << env::geometry::size(it.growth_step)      //
          << ",\n\tshrink " << env::geometry::size(it.shrink_threshold) //
          << ",\n\tpagesize " << env::geometry::size(it.pagesize) << "\n";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const env::operate_parameters &it) {
  return out << "{\n"                                 //
             << "\tmax_maps " << it.max_maps          //
             << ",\n\tmax_readers " << it.max_readers //
             << ",\n\tmode " << it.mode               //
             << ",\n\tdurability " << it.durability   //
             << ",\n\treclaiming " << it.reclaiming   //
             << ",\n\toptions " << it.options         //
             << "\n}";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const env::mode &it) {
  switch (it) {
  case env::mode::readonly:
    return out << "readonly";
  case env::mode::write_file_io:
    return out << "write_file_io";
  case env::mode::write_mapped_io:
    return out << "write_mapped_io";
  default:
    return out << "mdbx::env::mode::invalid";
  }
}

__cold ::std::ostream &operator<<(::std::ostream &out, const env::durability &it) {
  switch (it) {
  case env::durability::robust_synchronous:
    return out << "robust_synchronous";
  case env::durability::half_synchronous_weak_last:
    return out << "half_synchronous_weak_last";
  case env::durability::lazy_weak_tail:
    return out << "lazy_weak_tail";
  case env::durability::whole_fragile:
    return out << "whole_fragile";
  default:
    return out << "mdbx::env::durability::invalid";
  }
}

__cold ::std::ostream &operator<<(::std::ostream &out, const env::reclaiming_options &it) {
  return out << "{"                                            //
             << "lifo: " << (it.lifo ? "yes" : "no")           //
             << ", coalesce: " << (it.coalesce ? "yes" : "no") //
             << "}";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const env::operate_options &it) {
  static const char comma[] = ", ";
  const char *delimiter = "";
  out << "{";
  if (it.no_sticky_threads) {
    out << delimiter << "no_sticky_threads";
    delimiter = comma;
  }
  if (it.nested_write_transactions) {
    out << delimiter << "nested_write_transactions";
    delimiter = comma;
  }
  if (it.exclusive) {
    out << delimiter << "exclusive";
    delimiter = comma;
  }
  if (it.disable_readahead) {
    out << delimiter << "disable_readahead";
    delimiter = comma;
  }
  if (it.disable_clear_memory) {
    out << delimiter << "disable_clear_memory";
    delimiter = comma;
  }
  if (delimiter != comma)
    out << "default";
  return out << "}";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const env_managed::create_parameters &it) {
  return out << "{\n"                                                        //
             << "\tfile_mode " << std::oct << it.file_mode_bits << std::dec  //
             << ",\n\tsubdirectory " << (it.use_subdirectory ? "yes" : "no") //
             << ",\n"
             << it.geometry << "}";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const MDBX_log_level_t &it) {
  switch (it) {
  case MDBX_LOG_FATAL:
    return out << "LOG_FATAL";
  case MDBX_LOG_ERROR:
    return out << "LOG_ERROR";
  case MDBX_LOG_WARN:
    return out << "LOG_WARN";
  case MDBX_LOG_NOTICE:
    return out << "LOG_NOTICE";
  case MDBX_LOG_VERBOSE:
    return out << "LOG_VERBOSE";
  case MDBX_LOG_DEBUG:
    return out << "LOG_DEBUG";
  case MDBX_LOG_TRACE:
    return out << "LOG_TRACE";
  case MDBX_LOG_EXTRA:
    return out << "LOG_EXTRA";
  case MDBX_LOG_DONTCHANGE:
    return out << "LOG_DONTCHANGE";
  default:
    return out << "mdbx::log_level::invalid";
  }
}

__cold ::std::ostream &operator<<(::std::ostream &out, const MDBX_debug_flags_t &it) {
  if (it == MDBX_DBG_DONTCHANGE)
    return out << "DBG_DONTCHANGE";

  static const char comma[] = "|";
  const char *delimiter = "";
  out << "{";
  if (it & MDBX_DBG_ASSERT) {
    out << delimiter << "DBG_ASSERT";
    delimiter = comma;
  }
  if (it & MDBX_DBG_AUDIT) {
    out << delimiter << "DBG_AUDIT";
    delimiter = comma;
  }
  if (it & MDBX_DBG_JITTER) {
    out << delimiter << "DBG_JITTER";
    delimiter = comma;
  }
  if (it & MDBX_DBG_DUMP) {
    out << delimiter << "DBG_DUMP";
    delimiter = comma;
  }
  if (it & MDBX_DBG_LEGACY_MULTIOPEN) {
    out << delimiter << "DBG_LEGACY_MULTIOPEN";
    delimiter = comma;
  }
  if (it & MDBX_DBG_LEGACY_OVERLAP) {
    out << delimiter << "DBG_LEGACY_OVERLAP";
    delimiter = comma;
  }
  if (delimiter != comma)
    out << "DBG_NONE";
  return out << "}";
}

__cold ::std::ostream &operator<<(::std::ostream &out, const ::mdbx::error &err) {
  return out << err.what() << " (" << long(err.code()) << ")";
}

} // namespace mdbx
