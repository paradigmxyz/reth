/** This file is part of the libmdbx amalgamated source code (v0.14.2-8-gcfb319f8 at 2026-06-08T23:38:47+03:00).

\file mdbx.h
\brief The libmdbx C API header file.

\details _libmdbx_ (aka MDBX) is an extremely fast, compact, powerful, embeddable,
transactional [key-value
store](https://en.wikipedia.org/wiki/Key-value_database), with [Apache 2.0
license](./LICENSE). _MDBX_ has a specific set of properties and capabilities,
focused on creating unique lightweight solutions with extraordinary performance.

_libmdbx_ is superior to [LMDB](https://bit.ly/26ts7tL) in terms of features
and reliability, not inferior in performance. In comparison to LMDB, _libmdbx_
makes many things just work perfectly, not silently and catastrophically
break down. _libmdbx_ supports Linux, Windows, MacOS, OSX, Harmony, iOS, Android,
FreeBSD, DragonFly, Solaris, OpenSolaris, OpenIndiana, NetBSD, OpenBSD and other
systems compliant with POSIX.1-2008.

Please visit https://libmdbx.dqdkfa.ru for more information, documentation,
C++ API description and links to the origin git repo with the source code.
Questions, feedback and suggestions are welcome to the Telegram' group
https://t.me/libmdbx, MAX' chat https://max.ru/join/dKckvyuARxp1vRK-wnPur8zYCEkbR3OUOmpPWkWxp78.

Donations are welcome to ETH `0xD104d8f8B2dC312aaD74899F83EBf3EEBDC1EA3A`,
BTC `bc1qzvl9uegf2ea6cwlytnanrscyv8snwsvrc0xfsu`, SOL `FTCTgbHajoLVZGr8aEFWMzx3NDMyS5wXJgfeMTmJznRi`.
Всё будет хорошо!

The _libmdbx_ project has been completely relocated to the jurisdiction of the Russian Federation.
\note _libmdbx_ is still open and provided with first-class free support.

\section copyright LICENSE & COPYRIGHT
\copyright SPDX-License-Identifier: Apache-2.0
Please refer to the COPYRIGHT file for explanations license change, credits and acknowledgments.
\author Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> \date 2015-2026

*******************************************************************************/

#pragma once
/*
 * Tested with, since 2026:
 *  - Elbrus LCC >= 1.28 (http://www.mcst.ru/lcc);
 *  - GNU C >= 11.3;
 *  - CLANG >= 14.0;
 *  - MSVC >= 19.44 (Visual Studio 2022 toolchain v143),
 * before 2026:
 *  - Elbrus LCC >= 1.23 (http://www.mcst.ru/lcc);
 *  - GNU C >= 4.8;
 *  - CLANG >= 3.9;
 *  - MSVC >= 14.0 (Visual Studio 2015),
 *    but 19.2x could hang due optimizer bug;
 *  - AppleClang.
 */

#ifndef LIBMDBX_H
#define LIBMDBX_H

#if defined(__riscv) || defined(__riscv__) || defined(__RISCV) || defined(__RISCV__)
#warning "The RISC-V architecture is intentionally insecure by design. \
  Please delete this admonition at your own risk, \
  if you make such decision informed and consciously. \
  Refer to https://clck.ru/32d9xH for more information."
#endif /* RISC-V */

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

/* *INDENT-OFF* */
/* clang-format off */
/**
 \file mdbx.h
 \brief The libmdbx C API header file.

 \defgroup c_api C API
 @{
 \defgroup c_err Error handling
 \defgroup c_opening Opening & Closing
 \defgroup c_transactions Transactions
 \defgroup c_dbi Tables
 \defgroup c_crud Create/Read/Update/Delete (see Quick Reference in details)

 \details
 \anchor c_crud_hints
# Quick Reference for Insert/Update/Delete operations

Historically, libmdbx inherits the API basis from LMDB, where it is often
difficult to select flags/options and functions for the desired operation.
So it is recommend using this hints.

## Tables with UNIQUE keys

In tables created without the \ref MDBX_DUPSORT option, keys are always
unique. Thus always a single value corresponds to the each key, and so there
are only a few cases of changing data.

| Case                                        | Flags to use        | Result                 |
|---------------------------------------------|---------------------|------------------------|
| _INSERTING_|||
|Key is absent → Insertion                    |\ref MDBX_NOOVERWRITE|Insertion               |
|Key exist → Error since key present          |\ref MDBX_NOOVERWRITE|Error \ref MDBX_KEYEXIST and return Present value|
| _UPSERTING_|||
|Key is absent → Insertion                    |\ref MDBX_UPSERT     |Insertion               |
|Key exist → Update                           |\ref MDBX_UPSERT     |Update                  |
|  _UPDATING_|||
|Key is absent → Error since no such key      |\ref MDBX_CURRENT    |Error \ref MDBX_NOTFOUND|
|Key exist → Update                           |\ref MDBX_CURRENT    |Update value            |
| _DELETING_|||
|Key is absent → Error since no such key      |\ref mdbx_del() or \ref mdbx_replace()|Error \ref MDBX_NOTFOUND|
|Key exist → Delete by key                    |\ref mdbx_del() with the parameter `data = NULL`|Deletion|
|Key exist → Delete by key with data matching check|\ref mdbx_del() with the parameter `data` filled with the value which should be match for deletion|Deletion or \ref MDBX_NOTFOUND if the value does not match|
|Delete at the current cursor position        |\ref mdbx_cursor_del() with \ref MDBX_CURRENT flag|Deletion|
|Extract (read & delete) value by the key     |\ref mdbx_replace() with zero flag and parameter `new_data = NULL`|Returning a deleted value|

## Tables with NON-UNIQUE keys

In tables created with the \ref MDBX_DUPSORT (Sorted Duplicates) option, keys
may be non unique. Such non-unique keys in a key-value table may be treated
as a duplicates or as like a multiple values corresponds to keys.

| Case                                        | Flags to use        | Result                 |
|---------------------------------------------|---------------------|------------------------|
| _INSERTING_|||
|Key is absent → Insertion                    |\ref MDBX_NOOVERWRITE|Insertion|
|Key exist → Needn't to add new values        |\ref MDBX_NOOVERWRITE|Error \ref MDBX_KEYEXIST with returning the first value from those already present|
| _UPSERTING_|||
|Key is absent → Insertion                    |\ref MDBX_UPSERT     |Insertion|
|Key exist → Wanna to add new values          |\ref MDBX_UPSERT     |Add one more value to the key|
|Key exist → Replace all values with a new one|\ref MDBX_UPSERT + \ref MDBX_ALLDUPS|Overwrite by single new value|
|  _UPDATING_|||
|Key is absent → Error since no such key      |\ref MDBX_CURRENT    |Error \ref MDBX_NOTFOUND|
|Key exist, Single value → Update             |\ref MDBX_CURRENT    |Update single value    |
|Key exist, Multiple values → Replace all values with a new one|\ref MDBX_CURRENT + \ref MDBX_ALLDUPS|Overwrite by single new value|
|Key exist, Multiple values → Error since it is unclear which of the values should be updated|\ref mdbx_put() with \ref MDBX_CURRENT|Error \ref MDBX_EMULTIVAL|
|Key exist, Multiple values → Update particular entry of multi-value|\ref mdbx_replace() with \ref MDBX_CURRENT + \ref MDBX_NOOVERWRITE and the parameter `old_value` filled with the value that wanna to update|Update one multi-value entry|
|Key exist, Multiple values → Update the current entry of multi-value|\ref mdbx_cursor_put() with \ref MDBX_CURRENT|Update one multi-value entry|
| _DELETING_|||
|Key is absent → Error since no such key      |\ref mdbx_del() or \ref mdbx_replace()|Error \ref MDBX_NOTFOUND|
|Key exist → Delete all values corresponds given key|\ref mdbx_del() with the parameter `data = NULL`|Deletion|
|Key exist → Delete particular value corresponds given key|\ref mdbx_del() with the parameter `data` filled with the value that wanna to delete, or \ref mdbx_replace() with \ref MDBX_CURRENT + \ref MDBX_NOOVERWRITE and the `old_value` parameter filled with the value that wanna to delete and `new_data = NULL`| Deletion or \ref MDBX_NOTFOUND if no such key-value pair|
|Delete one value at the current cursor position|\ref mdbx_cursor_del() with \ref MDBX_CURRENT flag|Deletion only the current entry|
|Delete all values of key at the current cursor position|\ref mdbx_cursor_del() with \ref MDBX_ALLDUPS flag|Deletion all duplicates of key (all multi-values) at the current cursor position|

 \defgroup c_cursors Cursors
 \defgroup c_statinfo Statistics & Information
 \defgroup c_settings Settings
 \defgroup c_debug Logging and runtime debug
 \defgroup c_rqest Range query estimation
 \defgroup c_extra Extra operations
*/
/* *INDENT-ON* */
/* clang-format on */

#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#if !defined(assert)
#include <assert.h>
#endif /* assert */

#if defined(_WIN32) || defined(_WIN64)
#include <windows.h>
#include <winnt.h>
#ifndef __mode_t_defined
typedef unsigned short mdbx_mode_t;
#else
typedef mode_t mdbx_mode_t;
#endif /* __mode_t_defined */
typedef HANDLE mdbx_filehandle_t;
typedef DWORD mdbx_pid_t;
typedef DWORD mdbx_tid_t;
#else                  /* Windows */
#include <errno.h>     /* for error codes */
#include <pthread.h>   /* for pthread_t */
#include <sys/types.h> /* for pid_t */
#include <sys/uio.h>   /* for struct iovec */
#define HAVE_STRUCT_IOVEC 1
typedef int mdbx_filehandle_t;
typedef pid_t mdbx_pid_t;
typedef pthread_t mdbx_tid_t;
typedef mode_t mdbx_mode_t;
#endif /* !Windows */

#ifdef _MSC_VER
#pragma warning(pop)
#endif

#define MDBX_AMALGAMATED_SOURCE 1

/** end of c_api @}
 *
 * \defgroup api_macros Common Macros
 * @{ */

/*----------------------------------------------------------------------------*/

#ifndef __has_attribute
#define __has_attribute(x) (0)
#endif /* __has_attribute */

#ifndef __has_c_attribute
#define __has_c_attribute(x) (0)
#define __has_c_attribute_qualified(x) 0
#elif !defined(__STDC_VERSION__) || __STDC_VERSION__ < 202311L
#define __has_c_attribute_qualified(x) 0
#elif defined(_MSC_VER)
/* MSVC don't support `namespace::attr` syntax */
#define __has_c_attribute_qualified(x) 0
#else
#define __has_c_attribute_qualified(x) __has_c_attribute(x)
#endif /* __has_c_attribute */

#ifndef __has_cpp_attribute
#define __has_cpp_attribute(x) 0
#define __has_cpp_attribute_qualified(x) 0
#elif defined(_MSC_VER) || (__clang__ && __clang__ < 14)
/* MSVC don't support `namespace::attr` syntax */
#define __has_cpp_attribute_qualified(x) 0
#else
#define __has_cpp_attribute_qualified(x) __has_cpp_attribute(x)
#endif /* __has_cpp_attribute */

#ifndef __has_C23_or_CXX_attribute
#if defined(__cplusplus)
#define __has_C23_or_CXX_attribute(x) __has_cpp_attribute_qualified(x)
#else
#define __has_C23_or_CXX_attribute(x) __has_c_attribute_qualified(x)
#endif
#endif /* __has_C23_or_CXX_attribute */

#ifndef __has_feature
#define __has_feature(x) (0)
#define __has_exceptions_disabled (0)
#elif !defined(__has_exceptions_disabled)
#define __has_exceptions_disabled (__has_feature(cxx_noexcept) && !__has_feature(cxx_exceptions))
#endif /* __has_feature */

#ifndef __has_extension
#define __has_extension(x) __has_feature(x)
#endif /* __has_extension */

#ifndef __has_builtin
#define __has_builtin(x) (0)
#endif /* __has_builtin */

/** \brief The `pure` function attribute for optimization.
 * \details Many functions have no effects except the return value and their
 * return value depends only on the parameters and/or global variables.
 * Such a function can be subject to common subexpression elimination
 * and loop optimization just as an arithmetic operator would be.
 * These functions should be declared with the attribute pure. */
#if defined(DOXYGEN)
#define MDBX_PURE_FUNCTION [[gnu::pure]]
#elif __has_C23_or_CXX_attribute(gnu::pure)
#define MDBX_PURE_FUNCTION [[gnu::pure]]
#elif (defined(__GNUC__) || __has_attribute(__pure__)) &&                                                              \
    (!defined(__clang__) /* https://bugs.llvm.org/show_bug.cgi?id=43275 */ || !defined(__cplusplus) ||                 \
     __has_exceptions_disabled)
#define MDBX_PURE_FUNCTION __attribute__((__pure__))
#else
#define MDBX_PURE_FUNCTION
#endif /* MDBX_PURE_FUNCTION */

/** \brief The `pure nothrow` function attribute for optimization.
 * \details Like \ref MDBX_PURE_FUNCTION with addition `noexcept` restriction
 * that is compatible to CLANG and proposed [[pure]]. */
#if defined(DOXYGEN)
#define MDBX_NOTHROW_PURE_FUNCTION [[gnu::pure, gnu::nothrow]]
#elif __has_C23_or_CXX_attribute(gnu::pure)
#if __has_C23_or_CXX_attribute(gnu::nothrow)
#define MDBX_NOTHROW_PURE_FUNCTION [[gnu::pure, gnu::nothrow]]
#else
#define MDBX_NOTHROW_PURE_FUNCTION [[gnu::pure]]
#endif
#elif defined(__GNUC__) || (__has_attribute(__pure__) && __has_attribute(__nothrow__))
#define MDBX_NOTHROW_PURE_FUNCTION __attribute__((__pure__, __nothrow__))
#elif __has_cpp_attribute(pure)
#define MDBX_NOTHROW_PURE_FUNCTION [[pure]]
#else
#define MDBX_NOTHROW_PURE_FUNCTION
#endif /* MDBX_NOTHROW_PURE_FUNCTION */

/** \brief The `const` function attribute for optimization.
 * \details Many functions do not examine any values except their arguments,
 * and have no effects except the return value. Basically this is just
 * slightly more strict class than the PURE attribute, since function
 * is not allowed to read global memory.
 *
 * Note that a function that has pointer arguments and examines the
 * data pointed to must not be declared const. Likewise, a function
 * that calls a non-const function usually must not be const.
 * It does not make sense for a const function to return void. */
#if defined(DOXYGEN)
#define MDBX_CONST_FUNCTION [[gnu::const]]
#elif __has_C23_or_CXX_attribute(gnu::const)
#define MDBX_CONST_FUNCTION [[gnu::const]]
#elif (defined(__GNUC__) || __has_attribute(__const__)) &&                                                             \
    (!defined(__clang__) /* https://bugs.llvm.org/show_bug.cgi?id=43275 */ || !defined(__cplusplus) ||                 \
     __has_exceptions_disabled)
#define MDBX_CONST_FUNCTION __attribute__((__const__))
#else
#define MDBX_CONST_FUNCTION MDBX_PURE_FUNCTION
#endif /* MDBX_CONST_FUNCTION */

/** \brief The `const nothrow` function attribute for optimization.
 * \details Like \ref MDBX_CONST_FUNCTION with addition `noexcept` restriction
 * that is compatible to CLANG and future [[const]]. */
#if defined(DOXYGEN)
#define MDBX_NOTHROW_CONST_FUNCTION [[gnu::const, gnu::nothrow]]
#elif __has_C23_or_CXX_attribute(gnu::const)
#if __has_C23_or_CXX_attribute(gnu::nothrow)
#define MDBX_NOTHROW_CONST_FUNCTION [[gnu::const, gnu::nothrow]]
#else
#define MDBX_NOTHROW_CONST_FUNCTION [[gnu::const]]
#endif
#elif defined(__GNUC__) || (__has_attribute(__const__) && __has_attribute(__nothrow__))
#define MDBX_NOTHROW_CONST_FUNCTION __attribute__((__const__, __nothrow__))
#elif __has_cpp_attribute_qualified(const)
#define MDBX_NOTHROW_CONST_FUNCTION [[const]]
#else
#define MDBX_NOTHROW_CONST_FUNCTION MDBX_NOTHROW_PURE_FUNCTION
#endif /* MDBX_NOTHROW_CONST_FUNCTION */

/** \brief The `deprecated` attribute to produce warnings when used.
 * \note This macro may be predefined as empty to avoid "deprecated" warnings.
 */
#ifndef MDBX_DEPRECATED
#ifdef __deprecated
#define MDBX_DEPRECATED __deprecated
#elif defined(DOXYGEN) || ((!defined(__GNUC__) || (defined(__clang__) && __clang__ > 19) || __GNUC__ > 5) &&           \
                           ((defined(__cplusplus) && __cplusplus >= 201403L && __has_cpp_attribute(deprecated) &&      \
                             __has_cpp_attribute(deprecated) >= 201309L) ||                                            \
                            (!defined(__cplusplus) && defined(__STDC_VERSION__) && __STDC_VERSION__ >= 202304L)))
#define MDBX_DEPRECATED [[deprecated]]
#elif (defined(__GNUC__) && __GNUC__ > 5) ||                                                                           \
    (__has_attribute(__deprecated__) && (!defined(__GNUC__) || defined(__clang__) || __GNUC__ > 5))
#define MDBX_DEPRECATED __attribute__((__deprecated__))
#elif defined(_MSC_VER)
#define MDBX_DEPRECATED __declspec(deprecated)
#else
#define MDBX_DEPRECATED
#endif
#endif /* MDBX_DEPRECATED */

#ifndef MDBX_DEPRECATED_ENUM
#ifdef __deprecated_enum
#define MDBX_DEPRECATED_ENUM __deprecated_enum
#elif defined(DOXYGEN) ||                                                                                              \
    (!defined(_MSC_VER) || (defined(__cplusplus) && __cplusplus >= 201403L && __has_cpp_attribute(deprecated) &&       \
                            __has_cpp_attribute(deprecated) >= 201309L))
#define MDBX_DEPRECATED_ENUM MDBX_DEPRECATED
#else
#define MDBX_DEPRECATED_ENUM /* avoid madness MSVC */
#endif
#endif /* MDBX_DEPRECATED_ENUM */

#ifndef __dll_export
#if defined(_WIN32) || defined(_WIN64) || defined(__CYGWIN__) || defined(__MINGW__) || defined(__MINGW32__) ||         \
    defined(__MINGW64__)
#if defined(__GNUC__) || __has_attribute(__dllexport__)
#define __dll_export __attribute__((__dllexport__))
#elif defined(_MSC_VER)
#define __dll_export __declspec(dllexport)
#else
#define __dll_export
#endif
#elif defined(__GNUC__) || defined(__clang__) || __has_attribute(__visibility__)
#define __dll_export __attribute__((__visibility__("default")))
#else
#define __dll_export
#endif
#endif /* __dll_export */

#ifndef __dll_import
#if defined(_WIN32) || defined(_WIN64) || defined(__CYGWIN__) || defined(__MINGW__) || defined(__MINGW32__) ||         \
    defined(__MINGW64__)
#if defined(__GNUC__) || __has_attribute(__dllimport__)
#define __dll_import __attribute__((__dllimport__))
#elif defined(_MSC_VER)
#define __dll_import __declspec(dllimport)
#else
#define __dll_import
#endif
#else
#define __dll_import
#endif
#endif /* __dll_import */

/** \brief Auxiliary macro for robustly define the both inline version of API
 * function and non-inline fallback dll-exported version for applications linked
 * with old version of libmdbx, with a strictly ODR-common implementation. Thus,
 * we emulate __extern_inline for all compilers, including non-GNU ones. */
#if defined(LIBMDBX_INTERNALS) && !defined(LIBMDBX_NO_EXPORTS_LEGACY_API)
#define LIBMDBX_INLINE_API(TYPE, NAME, ARGS)                                                                           \
  /* proto of exported which uses common impl */ LIBMDBX_API TYPE NAME ARGS;                                           \
  /* definition of common impl */ static __inline TYPE __inline_##NAME ARGS
#else
#define LIBMDBX_INLINE_API(TYPE, NAME, ARGS) static __inline TYPE NAME ARGS
#endif /* LIBMDBX_INLINE_API */

/** \brief Converts a macro argument into a string constant. */
#ifndef MDBX_STRINGIFY
#define MDBX_STRINGIFY_HELPER(x) #x
#define MDBX_STRINGIFY(x) MDBX_STRINGIFY_HELPER(x)
#endif /* MDBX_STRINGIFY */

/*----------------------------------------------------------------------------*/

#ifndef __cplusplus
#ifndef bool
#define bool _Bool
#endif
#ifndef true
#define true (1)
#endif
#ifndef false
#define false (0)
#endif
#endif /* bool without __cplusplus */

/** Workaround for old compilers without support for C++17 `noexcept`. */
#if defined(DOXYGEN)
#define MDBX_CXX17_NOEXCEPT noexcept
#elif !defined(__cpp_noexcept_function_type) || __cpp_noexcept_function_type < 201510L
#define MDBX_CXX17_NOEXCEPT
#else
#define MDBX_CXX17_NOEXCEPT noexcept
#endif /* MDBX_CXX17_NOEXCEPT */

/** Workaround for old compilers without support for any kind of `constexpr`. */
#if defined(DOXYGEN)
#define MDBX_CXX01_CONSTEXPR constexpr
#define MDBX_CXX01_CONSTEXPR_VAR constexpr
#elif !defined(__cplusplus)
#define MDBX_CXX01_CONSTEXPR __inline
#define MDBX_CXX01_CONSTEXPR_VAR const
#elif !defined(DOXYGEN) &&                                                                                             \
    ((__cplusplus < 201103L && defined(__cpp_constexpr) && __cpp_constexpr < 200704L) ||                               \
     (defined(__LCC__) && __LCC__ < 124) ||                                                                            \
     (defined(__GNUC__) && (__GNUC__ * 100 + __GNUC_MINOR__ < 407) && !defined(__clang__) && !defined(__LCC__)) ||     \
     (defined(_MSC_VER) && _MSC_VER < 1910) || (defined(__clang__) && __clang_major__ < 4))
#define MDBX_CXX01_CONSTEXPR inline
#define MDBX_CXX01_CONSTEXPR_VAR const
#else
#define MDBX_CXX01_CONSTEXPR constexpr
#define MDBX_CXX01_CONSTEXPR_VAR constexpr
#endif /* MDBX_CXX01_CONSTEXPR */

/** Workaround for old compilers without properly support for C++11 `constexpr`.
 */
#if defined(DOXYGEN)
#define MDBX_CXX11_CONSTEXPR constexpr
#define MDBX_CXX11_CONSTEXPR_VAR constexpr
#elif !defined(__cplusplus)
#define MDBX_CXX11_CONSTEXPR __inline
#define MDBX_CXX11_CONSTEXPR_VAR const
#elif !defined(DOXYGEN) &&                                                                                             \
    (!defined(__cpp_constexpr) || __cpp_constexpr < 201304L || (defined(__LCC__) && __LCC__ < 124) ||                  \
     (defined(__GNUC__) && __GNUC__ < 6 && !defined(__clang__) && !defined(__LCC__)) ||                                \
     (defined(_MSC_VER) && _MSC_VER < 1910) || (defined(__clang__) && __clang_major__ < 5))
#define MDBX_CXX11_CONSTEXPR inline
#define MDBX_CXX11_CONSTEXPR_VAR const
#else
#define MDBX_CXX11_CONSTEXPR constexpr
#define MDBX_CXX11_CONSTEXPR_VAR constexpr
#endif /* MDBX_CXX11_CONSTEXPR */

/** Workaround for old compilers without properly support for C++14 `constexpr`.
 */
#if defined(DOXYGEN)
#define MDBX_CXX14_CONSTEXPR constexpr
#define MDBX_CXX14_CONSTEXPR_VAR constexpr
#elif !defined(__cplusplus)
#define MDBX_CXX14_CONSTEXPR __inline
#define MDBX_CXX14_CONSTEXPR_VAR const
#elif defined(DOXYGEN) ||                                                                                              \
    defined(__cpp_constexpr) && __cpp_constexpr >= 201304L &&                                                          \
        ((defined(_MSC_VER) && _MSC_VER >= 1910) || (defined(__clang__) && __clang_major__ > 4) ||                     \
         (defined(__GNUC__) && __GNUC__ > 6) || (!defined(__GNUC__) && !defined(__clang__) && !defined(_MSC_VER)))
#define MDBX_CXX14_CONSTEXPR constexpr
#define MDBX_CXX14_CONSTEXPR_VAR constexpr
#else
#define MDBX_CXX14_CONSTEXPR inline
#define MDBX_CXX14_CONSTEXPR_VAR const
#endif /* MDBX_CXX14_CONSTEXPR */

#if defined(__noreturn)
#define MDBX_NORETURN __noreturn
#elif defined(_Noreturn)
#define MDBX_NORETURN _Noreturn
#elif defined(DOXYGEN) || (defined(__cplusplus) && __cplusplus >= 201103L) ||                                          \
    (!defined(__cplusplus) && defined(__STDC_VERSION__) && __STDC_VERSION__ > 202005L)
#define MDBX_NORETURN [[noreturn]]
#elif defined(__GNUC__) || __has_attribute(__noreturn__)
#define MDBX_NORETURN __attribute__((__noreturn__))
#elif defined(_MSC_VER) && !defined(__clang__)
#define MDBX_NORETURN __declspec(noreturn)
#else
#define MDBX_NORETURN
#endif /* MDBX_NORETURN */

#ifndef MDBX_PRINTF_ARGS
#if defined(__GNUC__) || __has_attribute(__format__) || defined(DOXYGEN)
#if defined(__MINGW__) || defined(__MINGW32__) || defined(__MINGW64__)
#define MDBX_PRINTF_ARGS(format_index, first_arg) __attribute__((__format__(__gnu_printf__, format_index, first_arg)))
#else
#define MDBX_PRINTF_ARGS(format_index, first_arg) __attribute__((__format__(__printf__, format_index, first_arg)))
#endif /* MinGW */
#else
#define MDBX_PRINTF_ARGS(format_index, first_arg)
#endif
#endif /* MDBX_PRINTF_ARGS */

#if defined(DOXYGEN) ||                                                                                                \
    (defined(__cplusplus) && __cplusplus >= 201603L && __has_cpp_attribute(maybe_unused) &&                            \
     __has_cpp_attribute(maybe_unused) >= 201603L && (!defined(__clang__) || __clang__ > 19)) ||                       \
    (!defined(__cplusplus) && defined(__STDC_VERSION__) && __STDC_VERSION__ > 202005L)
#define MDBX_MAYBE_UNUSED [[maybe_unused]]
#elif defined(__GNUC__) || __has_attribute(__unused__)
#define MDBX_MAYBE_UNUSED __attribute__((__unused__))
#else
#define MDBX_MAYBE_UNUSED
#endif /* MDBX_MAYBE_UNUSED */

#if __has_attribute(no_sanitize) || defined(DOXYGEN)
#define MDBX_NOSANITIZE_ENUM __attribute((__no_sanitize__("enum")))
#else
#define MDBX_NOSANITIZE_ENUM
#endif /* MDBX_NOSANITIZE_ENUM */

/* Oh, below are some songs and dances since:
 *  - C++ requires explicit definition of the necessary operators.
 *  - the proper implementation of DEFINE_ENUM_FLAG_OPERATORS for C++ required
 *    the constexpr feature which is broken in most old compilers;
 *  - DEFINE_ENUM_FLAG_OPERATORS may be defined broken as in the Windows SDK. */
#if !defined(DEFINE_ENUM_FLAG_OPERATORS) && !defined(DOXYGEN)

#ifdef __cplusplus
#if !defined(__cpp_constexpr) || __cpp_constexpr < 200704L || (defined(__LCC__) && __LCC__ < 124) ||                   \
    (defined(__GNUC__) && (__GNUC__ * 100 + __GNUC_MINOR__ < 407) && !defined(__clang__) && !defined(__LCC__)) ||      \
    (defined(_MSC_VER) && _MSC_VER < 1910) || (defined(__clang__) && __clang_major__ < 4)
/* The constexpr feature is not available or (may be) broken */
#define CONSTEXPR_ENUM_FLAGS_OPERATIONS 0
#else
/* C always allows these operators for enums */
#define CONSTEXPR_ENUM_FLAGS_OPERATIONS 1
#endif /* __cpp_constexpr */

/// Define operator overloads to enable bit operations on enum values that are
/// used to define flags (based on Microsoft's DEFINE_ENUM_FLAG_OPERATORS).
#define DEFINE_ENUM_FLAG_OPERATORS(ENUM)                                                                               \
  extern "C++" {                                                                                                       \
  MDBX_NOSANITIZE_ENUM MDBX_CXX01_CONSTEXPR ENUM operator|(ENUM a, ENUM b) { return ENUM(unsigned(a) | unsigned(b)); } \
  MDBX_NOSANITIZE_ENUM MDBX_CXX14_CONSTEXPR ENUM &operator|=(ENUM &a, ENUM b) { return a = a | b; }                    \
  MDBX_NOSANITIZE_ENUM MDBX_CXX01_CONSTEXPR ENUM operator&(ENUM a, ENUM b) { return ENUM(unsigned(a) & unsigned(b)); } \
  MDBX_NOSANITIZE_ENUM MDBX_CXX01_CONSTEXPR ENUM operator&(ENUM a, unsigned b) { return ENUM(unsigned(a) & b); }       \
  MDBX_NOSANITIZE_ENUM MDBX_CXX01_CONSTEXPR ENUM operator&(unsigned a, ENUM b) { return ENUM(a & unsigned(b)); }       \
  MDBX_NOSANITIZE_ENUM MDBX_CXX14_CONSTEXPR ENUM &operator&=(ENUM &a, ENUM b) { return a = a & b; }                    \
  MDBX_NOSANITIZE_ENUM MDBX_CXX14_CONSTEXPR ENUM &operator&=(ENUM &a, unsigned b) { return a = a & b; }                \
  MDBX_CXX01_CONSTEXPR unsigned operator~(ENUM a) { return ~unsigned(a); }                                             \
  MDBX_NOSANITIZE_ENUM MDBX_CXX01_CONSTEXPR ENUM operator^(ENUM a, ENUM b) { return ENUM(unsigned(a) ^ unsigned(b)); } \
  MDBX_NOSANITIZE_ENUM MDBX_CXX14_CONSTEXPR ENUM &operator^=(ENUM &a, ENUM b) { return a = a ^ b; }                    \
  }
#else /* __cplusplus */
/* nope for C since it always allows these operators for enums */
#define DEFINE_ENUM_FLAG_OPERATORS(ENUM)
#define CONSTEXPR_ENUM_FLAGS_OPERATIONS 1
#endif /* !__cplusplus */

#elif !defined(CONSTEXPR_ENUM_FLAGS_OPERATIONS)

#ifdef __cplusplus
/* DEFINE_ENUM_FLAG_OPERATORS may be defined broken as in the Windows SDK */
#define CONSTEXPR_ENUM_FLAGS_OPERATIONS 0
#else
/* C always allows these operators for enums */
#define CONSTEXPR_ENUM_FLAGS_OPERATIONS 1
#endif

#endif /* DEFINE_ENUM_FLAG_OPERATORS */

#ifndef MDBX_LIKELY
#if defined(DOXYGEN) || (defined(__GNUC__) || __has_builtin(__builtin_expect)) && !defined(__COVERITY__)
#define MDBX_LIKELY(cond) __builtin_expect(!!(cond), 1)
#else
#define MDBX_LIKELY(x) (x)
#endif
#endif /* MDBX_LIKELY */

#ifndef MDBX_UNLIKELY
#if defined(DOXYGEN) || (defined(__GNUC__) || __has_builtin(__builtin_expect)) && !defined(__COVERITY__)
#define MDBX_UNLIKELY(cond) __builtin_expect(!!(cond), 0)
#else
#define MDBX_UNLIKELY(x) (x)
#endif
#endif /* MDBX_UNLIKELY */

#if defined(DOXYGEN) || (defined(MDBX_CHECKING) && MDBX_CHECKING > 0) || (defined(MDBX_DEBUG) && MDBX_DEBUG > 0)
#define MDBX_INLINE_API_ASSERT(expr)                                                                                   \
  do {                                                                                                                 \
    if (MDBX_UNLIKELY(!(expr)))                                                                                        \
      mdbx_assert_fail(#expr, __func__, __LINE__);                                                                     \
  } while (0)
#else
/* clang-format off */
#define MDBX_INLINE_API_ASSERT(expr) do {} while(0)
/* clang-format on */
#endif /* MDBX_INLINE_API_ASSERT */

/** end of api_macros @} */

/*----------------------------------------------------------------------------*/

/** \addtogroup c_api
 * @{ */

#ifdef __cplusplus
extern "C" {
#endif

/* MDBX version 0.14.x, but it is unstable/under-development yet. */
#define MDBX_VERSION_UNSTABLE
#define MDBX_VERSION_MAJOR 0
#define MDBX_VERSION_MINOR 14

#ifndef LIBMDBX_API
#if defined(LIBMDBX_EXPORTS) || defined(DOXYGEN)
#define LIBMDBX_API __dll_export
#elif defined(LIBMDBX_IMPORTS)
#define LIBMDBX_API __dll_import
#else
#define LIBMDBX_API
#endif
#endif /* LIBMDBX_API */

#ifdef __cplusplus
#if defined(__clang__) || __has_attribute(type_visibility) || defined(DOXYGEN)
#define LIBMDBX_API_TYPE LIBMDBX_API __attribute__((type_visibility("default")))
#else
#define LIBMDBX_API_TYPE LIBMDBX_API
#endif
#else
#define LIBMDBX_API_TYPE
#endif /* LIBMDBX_API_TYPE */

#if defined(LIBMDBX_IMPORTS)
#define LIBMDBX_VERINFO_API __dll_import
#else
#define LIBMDBX_VERINFO_API __dll_export
#endif /* LIBMDBX_VERINFO_API */

/** \brief libmdbx version information, \see https://semver.org/ */
extern LIBMDBX_VERINFO_API const struct MDBX_version_info {
  uint16_t major;                /**< Major version number */
  uint16_t minor;                /**< Minor version number */
  uint16_t patch;                /**< Patch number */
  uint16_t tweak;                /**< Tweak number */
  const char *semver_prerelease; /**< Semantic Versioning `pre-release` */
  struct {
    const char *datetime; /**< committer date, strict ISO-8601 format */
    const char *tree;     /**< commit hash (hexadecimal digits) */
    const char *commit;   /**< tree hash, i.e. digest of the source code */
    const char *describe; /**< git-describe string */
  } git;                  /**< source information from git */
  const char *sourcery;   /**< sourcery anchor for pinning */
} /** \brief libmdbx version information */ mdbx_version;

/** \brief libmdbx build information
 * \attention Some strings could be NULL in case no corresponding information
 *            was provided at build time (i.e. flags). */
extern LIBMDBX_VERINFO_API const struct MDBX_build_info {
  const char *datetime; /**< build timestamp (ISO-8601 or __DATE__ __TIME__) */
  const char *target;   /**< cpu/arch-system-config triplet */
  const char *options;  /**< mdbx-related options */
  const char *compiler; /**< compiler */
  const char *flags;    /**< CFLAGS and CXXFLAGS */
  const char *metadata; /**< an extra/custom information provided via
                             the MDBX_BUILD_METADATA definition
                             during library build */
} /** \brief libmdbx build information */ mdbx_build;

#if (defined(_WIN32) || defined(_WIN64)) && !MDBX_BUILD_SHARED_LIBRARY
/* MDBX internally uses global and thread local storage destructors to
 * automatically (de)initialization, releasing reader lock table slots
 * and so on.
 *
 * If MDBX built as a DLL this is done out-of-the-box by DllEntry() function,
 * which called automatically by Windows core with passing corresponding reason
 * argument.
 *
 * Otherwise, if MDBX was built not as a DLL, some black magic
 * may be required depending of Windows version:
 *
 *  - Modern Windows versions, including Windows Vista and later, provides
 *    support for "TLS Directory" (e.g .CRT$XL[A-Z] sections in executable
 *    or dll file). In this case, MDBX capable of doing all automatically,
 *    therefore you DON'T NEED to call mdbx_module_handler()
 *    so the MDBX_MANUAL_MODULE_HANDLER defined as 0.
 *
 *  - Obsolete versions of Windows, prior to Windows Vista, REQUIRES calling
 *    mdbx_module_handler() manually from corresponding DllMain() or WinMain()
 *    of your DLL or application,
 *    so the MDBX_MANUAL_MODULE_HANDLER defined as 1.
 *
 * Therefore, building MDBX as a DLL is recommended for all version of Windows.
 * So, if you doubt, just build MDBX as the separate DLL and don't care about
 * the MDBX_MANUAL_MODULE_HANDLER. */

#ifndef _WIN32_WINNT
#error Non-dll build libmdbx requires target Windows version \
  to be explicitly defined via _WIN32_WINNT for properly \
  handling thread local storage destructors.
#endif /* _WIN32_WINNT */

#if _WIN32_WINNT >= 0x0600 /* Windows Vista */
/* As described above mdbx_module_handler() is NOT needed for Windows Vista
 * and later. */
#define MDBX_MANUAL_MODULE_HANDLER 0
#else
/* As described above mdbx_module_handler() IS REQUIRED for Windows versions
 * prior to Windows Vista. */
#define MDBX_MANUAL_MODULE_HANDLER 1
void LIBMDBX_API NTAPI mdbx_module_handler(PVOID module, DWORD reason, PVOID reserved);
#endif

#endif /* Windows && !DLL && MDBX_MANUAL_MODULE_HANDLER */

/* OPACITY STRUCTURES *********************************************************/

/** \brief Opaque structure for a database environment.
 * \details An environment supports multiple key-value tables (aka key-value
 * maps, spaces or sub-databases), all residing in the same database file.
 * \see mdbx_env_create() \see mdbx_env_close() */
#ifndef __cplusplus
typedef struct MDBX_env MDBX_env;
#else
struct MDBX_env;
#endif

/** \brief Opaque structure for a transaction handle.
 * \ingroup c_transactions
 * \details All table operations require a transaction handle. Transactions
 * may be read-only or read-write.
 * \see mdbx_txn_begin() \see mdbx_txn_commit() \see mdbx_txn_abort() */
#ifndef __cplusplus
typedef struct MDBX_txn MDBX_txn;
#else
struct MDBX_txn;
#endif

/** \brief A handle for an individual table (key-value spaces) in the
 * environment.
 * \ingroup c_dbi
 * \details Zero handle is used internally (hidden Garbage Collection table).
 * So, any valid DBI-handle great than 0 and less than or equal
 * \ref MDBX_MAX_DBI.
 * \see mdbx_dbi_open() \see mdbx_dbi_close() */
typedef uint32_t MDBX_dbi;

/** \brief Opaque structure for navigating through a table
 * \ingroup c_cursors
 * \see mdbx_cursor_create() \see mdbx_cursor_bind() \see mdbx_cursor_close()
 */
#ifndef __cplusplus
typedef struct MDBX_cursor MDBX_cursor;
#else
struct MDBX_cursor;
#endif

/** \brief Generic structure used for passing keys and data in and out of the table.
 * \anchor MDBX_val \see mdbx::slice \see mdbx::buffer
 *
 * \details Values returned from the table are valid only until a subsequent
 * update operation, or the end of the transaction. Do not modify or
 * free them, they commonly point into the database itself.
 *
 * Key sizes must be between 0 and \ref mdbx_env_get_maxkeysize() inclusive.
 * The same applies to data sizes in tables with the \ref MDBX_DUPSORT flag.
 * Other data items can in theory be from 0 to \ref MDBX_MAXDATASIZE bytes long.
 *
 * \note The notable difference between MDBX and LMDB is that MDBX support zero
 * length keys. */
#ifndef HAVE_STRUCT_IOVEC
struct iovec {
  void *iov_base; /**< pointer to some data */
  size_t iov_len; /**< the length of data in bytes */
};
#define HAVE_STRUCT_IOVEC
#endif /* HAVE_STRUCT_IOVEC */

#if defined(__sun) || defined(__SVR4) || defined(__svr4__)
/* The `iov_len` is signed on Sun/Solaris.
 * So define custom MDBX_val to avoid a lot of warnings. */
struct MDBX_val {
  void *iov_base; /**< pointer to some data */
  size_t iov_len; /**< the length of data in bytes */
};
#ifndef __cplusplus
typedef struct MDBX_val MDBX_val;
#endif
#else  /* SunOS */
typedef struct iovec MDBX_val;
#endif /* ! SunOS */

enum MDBX_constants {
  /** The hard limit for DBI handles. */
  MDBX_MAX_DBI = UINT32_C(32765),

  /** The maximum size of a data item. */
  MDBX_MAXDATASIZE = UINT32_C(0x7fff0000),

  /** The minimal database page size in bytes. */
  MDBX_MIN_PAGESIZE = 256,

  /** The maximal database page size in bytes. */
  MDBX_MAX_PAGESIZE = 65536,
};

/* THE FILES *******************************************************************
 * At the file system level, the environment corresponds to a pair of files. */

#ifndef MDBX_LOCKNAME
/** \brief The name of the lock file in the environment without using \ref MDBX_NOSUBDIR */
#if !(defined(_WIN32) || defined(_WIN64))
#define MDBX_LOCKNAME "/mdbx.lck"
#else
#define MDBX_LOCKNAME_W L"\\mdbx.lck"
#define MDBX_LOCKNAME_A "\\mdbx.lck"
#ifdef UNICODE
#define MDBX_LOCKNAME MDBX_LOCKNAME_W
#else
#define MDBX_LOCKNAME MDBX_LOCKNAME_A
#endif /* UNICODE */
#endif /* Windows */
#endif /* MDBX_LOCKNAME */
#ifndef MDBX_DATANAME
/** \brief The name of the data file in the environment
 * without using \ref MDBX_NOSUBDIR */
#if !(defined(_WIN32) || defined(_WIN64))
#define MDBX_DATANAME "/mdbx.dat"
#else
#define MDBX_DATANAME_W L"\\mdbx.dat"
#define MDBX_DATANAME_A "\\mdbx.dat"
#ifdef UNICODE
#define MDBX_DATANAME MDBX_DATANAME_W
#else
#define MDBX_DATANAME MDBX_DATANAME_A
#endif /* UNICODE */
#endif /* Windows */
#endif /* MDBX_DATANAME */

#ifndef MDBX_LOCK_SUFFIX
/** \brief The suffix of the lock file when \ref MDBX_NOSUBDIR is used */
#if !(defined(_WIN32) || defined(_WIN64))
#define MDBX_LOCK_SUFFIX "-lck"
#else
#define MDBX_LOCK_SUFFIX_W L"-lck"
#define MDBX_LOCK_SUFFIX_A "-lck"
#ifdef UNICODE
#define MDBX_LOCK_SUFFIX MDBX_LOCK_SUFFIX_W
#else
#define MDBX_LOCK_SUFFIX MDBX_LOCK_SUFFIX_A
#endif /* UNICODE */
#endif /* Windows */
#endif /* MDBX_LOCK_SUFFIX */

/* DEBUG & LOGGING ************************************************************/

/** \addtogroup c_debug
 * \note Most of debug feature enabled only when libmdbx built with
 * \ref MDBX_DEBUG build option. @{ */

/** Log level
 * \note Levels detailed than (great than) \ref MDBX_LOG_NOTICE
 * requires build libmdbx with \ref MDBX_DEBUG option.
 *
 * \see mdbx_setup_debug() \see MDBX_log_level_t */
typedef enum MDBX_log_level {
  /** Critical conditions, i.e. assertion failures.
   * \note libmdbx always produces such messages regardless
   * of \ref MDBX_DEBUG build option. */
  MDBX_LOG_FATAL = 0,

  /** Enables logging for error conditions
   * and \ref MDBX_LOG_FATAL.
   * \note libmdbx always produces such messages regardless
   * of \ref MDBX_DEBUG build option. */
  MDBX_LOG_ERROR = 1,

  /** Enables logging for warning conditions
   * and \ref MDBX_LOG_ERROR ... \ref MDBX_LOG_FATAL.
   * \note libmdbx always produces such messages regardless
   * of \ref MDBX_DEBUG build option. */
  MDBX_LOG_WARN = 2,

  /** Enables logging for normal but significant condition
   * and \ref MDBX_LOG_WARN ... \ref MDBX_LOG_FATAL.
   * \note libmdbx always produces such messages regardless
   * of \ref MDBX_DEBUG build option. */
  MDBX_LOG_NOTICE = 3,

  /** Enables logging for verbose informational
   * and \ref MDBX_LOG_NOTICE ... \ref MDBX_LOG_FATAL.
   * \note Requires build libmdbx with \ref MDBX_DEBUG option. */
  MDBX_LOG_VERBOSE = 4,

  /** Enables logging for debug-level messages
   * and \ref MDBX_LOG_VERBOSE ... \ref MDBX_LOG_FATAL.
   * \note Requires build libmdbx with \ref MDBX_DEBUG option. */
  MDBX_LOG_DEBUG = 5,

  /** Enables logging for trace debug-level messages
   * and \ref MDBX_LOG_DEBUG ... \ref MDBX_LOG_FATAL.
   * \note Requires build libmdbx with \ref MDBX_DEBUG option. */
  MDBX_LOG_TRACE = 6,

  /** Enables extra debug-level messages (dump pgno lists)
   * and all other log-messages.
   * \note Requires build libmdbx with \ref MDBX_DEBUG option. */
  MDBX_LOG_EXTRA = 7,

#ifdef ENABLE_UBSAN
  MDBX_LOG_MAX = 7 /* avoid UBSAN false-positive trap by a tests */,
#endif /* ENABLE_UBSAN */

  /** for \ref mdbx_setup_debug() only: Don't change current settings */
  MDBX_LOG_DONTCHANGE = -1
} MDBX_log_level_t;

/** \brief Runtime debug flags
 *
 * \details `MDBX_DBG_DUMP` and `MDBX_DBG_LEGACY_MULTIOPEN` always have an
 * effect, but `MDBX_DBG_ASSERT`, `MDBX_DBG_AUDIT` and `MDBX_DBG_JITTER` only if
 * libmdbx built with \ref MDBX_DEBUG.
 *
 * \see mdbx_setup_debug() \see MDBX_debug_flags_t */
typedef enum MDBX_debug_flags {
  MDBX_DBG_NONE = 0,

  /** Enables costly check of debug-like assertions.
   * \note Has effect only for debugging builds with the build option \ref MDBX_CHECKING >= 2. */
  MDBX_DBG_ASSERT = 1,

  /** Enables extra costly checks and deep verification of page lists,
   * including page usage audit at commit transactions.
   * \note Has effect only for debugging builds with the build option \ref MDBX_CHECKING >= 3. */
  MDBX_DBG_AUDIT = 2,

  /** Enables small random delays in critical points.
   * \note Requires build with \ref MDBX_DEBUG > 0 */
  MDBX_DBG_JITTER = 4,

  /** Controls including of a database(s) meta-pages in coredump files.
   * \note May affect performance while inspecting or dumping process memory. */
  MDBX_DBG_DUMP = 8,

  /** Allow multi-opening environment(s) */
  MDBX_DBG_LEGACY_MULTIOPEN = 16,

  /** Allow read and write transactions overlapping for the same thread. */
  MDBX_DBG_LEGACY_OVERLAP = 32,

  /** Disables automatic updating of the database format signature, i.e. upgrade database format on a media.
   * \note Nonetheless a new write transactions will use and store the last signature regardless this flag */
  MDBX_DBG_DONT_UPGRADE = 64,

#ifdef ENABLE_UBSAN
  MDBX_DBG_MAX = ((unsigned)MDBX_LOG_MAX) << 16 | 127 /* avoid UBSAN false-positive trap by a tests */,
#endif /* ENABLE_UBSAN */

  /** for mdbx_setup_debug() only: Don't change current settings */
  MDBX_DBG_DONTCHANGE = -1
} MDBX_debug_flags_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_debug_flags)

/** \brief A debug-logger callback function,
 * called before printing the message and aborting.
 * \see mdbx_setup_debug()
 *
 * \param [in] loglevel  The severity of message.
 * \param [in] function  The function name which emits message,
 *                       may be NULL.
 * \param [in] line      The source code line number which emits message,
 *                       may be zero.
 * \param [in] fmt       The printf-like format string with message.
 * \param [in] args      The variable argument list respectively for the
 *                       format-message string passed by `fmt` argument.
 *                       Maybe NULL or invalid if the format-message string
 *                       don't contain `%`-specification of arguments. */
typedef void (*MDBX_debug_func)(MDBX_log_level_t loglevel, const char *function, int line, const char *fmt,
                                va_list args) MDBX_CXX17_NOEXCEPT;

/** \brief The "don't change `logger`" value for mdbx_setup_debug() */
#define MDBX_LOGGER_DONTCHANGE ((MDBX_debug_func)(intptr_t)-1)
#define MDBX_LOGGER_NOFMT_DONTCHANGE ((MDBX_debug_func_nofmt)(intptr_t)-1)

/** \brief Setup global log-level, debug options and debug logger.
 * \returns The previously `debug_flags` in the 0-15 bits
 *          and `log_level` in the 16-31 bits.
 *
 * \see MDBX_log_level_t \see MDBX_debug_flags_t */
LIBMDBX_API int mdbx_setup_debug(MDBX_log_level_t log_level, MDBX_debug_flags_t debug_flags, MDBX_debug_func logger);

typedef void (*MDBX_debug_func_nofmt)(MDBX_log_level_t loglevel, const char *function, int line, const char *msg,
                                      unsigned length) MDBX_CXX17_NOEXCEPT;

LIBMDBX_API int mdbx_setup_debug_nofmt(MDBX_log_level_t log_level, MDBX_debug_flags_t debug_flags,
                                       MDBX_debug_func_nofmt logger, char *logger_buffer, size_t logger_buffer_size);

/** \brief A callback function for most assertion failures,
 * called before printing the message and aborting.
 * \see mdbx_env_set_panic()
 *
 * \param [in] msg       The assertion message, not including newline.
 * \param [in] function  The function name where the assertion check failed,
 *                       may be NULL.
 * \param [in] line      The line number in the source file
 *                       where the assertion check failed, may be zero.
 * \param [in] obj       A handle of object associated with the assertion,
 *                       it could be MDBX_env, MDBX_txn,
 *                       MDBX_cursor or an internal page structure.
 * \param [in] obj_class A value corresponding to the object type:
 *                       `env`, `txn`, `cursor`, etc. */
typedef void (*MDBX_panic_func)(const char *msg, const char *function, unsigned line, const void *obj,
                                const char *obj_class) MDBX_CXX17_NOEXCEPT;

/** \brief Auxiliary function for MDBX_INLINE_API_ASSERT(). */
MDBX_NORETURN LIBMDBX_API void mdbx_assert_fail(const char *msg, const char *func, unsigned line);

/** \brief Sets or reset the callback for panic() and assert() for the current process.
 *
 * \param [in] func  An MDBX_assert_func function, or 0. */
LIBMDBX_API void mdbx_set_panic(MDBX_panic_func func);

/** \brief Dump given MDBX_val to the buffer
 *
 * Dumps it as string if value is printable (all bytes in the range 0x20..0x7E),
 * otherwise made hexadecimal dump. Requires at least 4 byte length buffer.
 *
 * \returns One of:
 *  - NULL if given buffer size less than 4 bytes;
 *  - pointer to constant string if given value NULL or empty;
 *  - otherwise pointer to given buffer. */
LIBMDBX_API const char *mdbx_dump_val(const MDBX_val *key, char *const buf, const size_t bufsize);

/** end of c_debug @} */

/** \brief Environment flags
 * \ingroup c_opening
 * \anchor env_flags
 * \see mdbx_env_open() \see mdbx_env_set_flags() */
typedef enum MDBX_env_flags {
  MDBX_ENV_DEFAULTS = 0,

  /** Extra validation of DB structure and pages content.
   *
   * The `MDBX_VALIDATION` enabled the simple safe/careful mode for working
   * with damaged or untrusted DB. However, a notable performance
   * degradation should be expected. */
  MDBX_VALIDATION = UINT32_C(0x00002000),

  /** No environment directory.
   *
   * By default, MDBX creates its environment in a directory whose pathname is
   * given in path, and creates its data and lock files under that directory.
   * With this option, path is used as-is for the database main data file.
   * The database lock file is the path with "-lck" appended.
   *
   * - with `MDBX_NOSUBDIR` = in a filesystem we have the pair of MDBX-files
   *   which names derived from given pathname by appending predefined suffixes.
   *
   * - without `MDBX_NOSUBDIR` = in a filesystem we have the MDBX-directory with
   *   given pathname, within that a pair of MDBX-files with predefined names.
   *
   * This flag affects only at new environment creating by \ref mdbx_env_open(),
   * otherwise at opening an existing environment libmdbx will choice this
   * automatically. */
  MDBX_NOSUBDIR = UINT32_C(0x4000),

  /** Read only mode.
   *
   * Open the environment in read-only mode. No write operations will be
   * allowed. MDBX will still modify the lock file - except on read-only
   * filesystems, where MDBX does not use locks.
   *
   * - with `MDBX_RDONLY` = open environment in read-only mode.
   *   MDBX supports pure read-only mode (i.e. without opening LCK-file) only
   *   when environment directory and/or both files are not writable (and the
   *   LCK-file may be missing). In such case allowing file(s) to be placed
   *   on a network read-only share.
   *
   * - without `MDBX_RDONLY` = open environment in read-write mode.
   *
   * This flag affects only at environment opening but can't be changed after.
   */
  MDBX_RDONLY = UINT32_C(0x20000),

  /** Open environment in exclusive/monopolistic mode.
   *
   * `MDBX_EXCLUSIVE` flag can be used as a replacement for `MDB_NOLOCK`,
   * which don't supported by MDBX.
   * In this way, you can get the minimal overhead, but with the correct
   * multi-process and multi-thread locking.
   *
   * - with `MDBX_EXCLUSIVE` = open environment in exclusive/monopolistic mode
   *   or return \ref MDBX_BUSY if environment already used by other process.
   *   The main feature of the exclusive mode is the ability to open the
   *   environment placed on a network share.
   *
   * - without `MDBX_EXCLUSIVE` = open environment in cooperative mode,
   *   i.e. for multi-process access/interaction/cooperation.
   *   The main requirements of the cooperative mode are:
   *
   *   1. data files MUST be placed in the LOCAL file system,
   *      but NOT on a network share.
   *   2. environment MUST be opened only by LOCAL processes,
   *      but NOT over a network.
   *   3. OS kernel (i.e. file system and lock-file memory mapping
   *      implementation) and all processes that open the given environment
   *      MUST be running in the physically single RAM with cache-coherency.
   *      The only exception for cache-consistency requirement is Linux on MIPS
   *      architecture, but this case has not been tested for a long time).
   *
   * This flag affects only at environment opening but can't be changed after.
   */
  MDBX_EXCLUSIVE = UINT32_C(0x400000),

  /** Using database/environment which already opened by another process(es).
   *
   * The `MDBX_ACCEDE` flag is useful to avoid \ref MDBX_INCOMPATIBLE error
   * while opening the database/environment which is already used by another
   * process(es) with unknown mode/flags. In such cases, if there is a
   * difference in the specified flags (\ref MDBX_NOMETASYNC,
   * \ref MDBX_SAFE_NOSYNC, \ref MDBX_UTTERLY_NOSYNC, \ref MDBX_LIFORECLAIM
   * and \ref MDBX_NORDAHEAD), instead of returning an error,
   * the database will be opened in a compatibility with the already used mode.
   *
   * `MDBX_ACCEDE` has no effect if the current process is the only one either
   * opening the DB in read-only mode or other process(es) uses the DB in
   * read-only mode. */
  MDBX_ACCEDE = UINT32_C(0x40000000),

  /** Legacy writable data mapping mode.
   *
   * `MDBX_WRITEMAP` used to map the data file with write permission and modify
   * database pages directly in that mapping. The explicit-I/O backend does not
   * provide writable data-file mmap semantics, so enabling this flag for a
   * writable environment is rejected with \ref MDBX_INCOMPATIBLE by
   * \ref mdbx_env_open() and \ref mdbx_env_set_flags().
   *
   * Read-only opens silently ignore this flag together with other write-only
   * options.
   *
   * This flag cannot be enabled for writable explicit-I/O environments.
   */
  MDBX_WRITEMAP = UINT32_C(0x80000),

  /** Unlinks transactions from threads as much as possible.
   *
   * This option is intended for applications that multiplex multiple user-defined lightweight execution threads across
   * separate operating system threads, such as in the GoLang and Rust runtimes. It is also recommended for such
   * applications to serialize write transactions in a single operating system thread, since MDBX write locking uses
   * basic synchronization system primitives and knows nothing about user threads and/or lightweight runtime threads.
   * Anyway, at a minimum, it is absolutely necessary to ensure that each writing transaction is finished strictly in
   * the same thread of the operating system where it was started.
   *
   * \note Starting from version 0.13, the `MDBX_NOSTICKYTHREADS` option completely replaces the \ref MDBX_NOTLS option.
   *
   * When using `MDBX_NOSTICKYTHREADS`, transactions become unrelated to system threads that created ones. Therefore,
   * the API functions do not check the correspondence between the transaction and the current execution thread. Most
   * functions that work with transactions and cursors can be called from any execution thread. However, it also becomes
   * impossible to detect mistakes when transactions and/or cursors are used simultaneously in different threads.
   *
   * Using `MDBX_NOSTICKYTHREADS` also narrows down the possibilities for resizing the database, as it loses the ability
   * to track execution threads working with the database and suspend ones while the database is unmapped in RAM. Thus
   * unmapping  and remapping of a database file becomes impossible while at least one transaction in still present. In
   * particular, for this reason, on Windows, reducing the database file is not possible until the database is closed by
   * the last process working with it or until the database is subsequently opened in read-write mode.
   *
   * \warning Regardless of \ref MDBX_NOSTICKYTHREADS and \ref MDBX_NOTLS, it is not allowed to use API objects from
   * different execution threads at the same time! It is entirely your responsibility to ensure that API objects are not
   * used simultaneously from different execution threads!
   *
   * \warning Write transactions can only be finished in the same system execution thread where ones were started. This
   * restriction follows from the requirements of most operating systems that the acquired synchronization primitive
   * (mutex, semaphore, critical section) should be released only by the same system execution thread which acquires it.
   *
   * \warning Creating a cursor in the context of a transaction, binding a cursor to a transaction, unlinking a cursor
   * from a transaction, and closing a cursor bound to a transaction are operations that use both a cursor itself and a
   * corresponding transaction. Similarly, completing or aborting a transaction is an operation that uses both a
   * transaction itself and all a cursors associated with it. In order to avoid damage to internal data structures,
   * unpredictable behavior, database destruction and data loss, it is necessary to avoid the possibility of
   * simultaneous use of any cursor or transactions from different execution threads.
   *
   * When using `MDBX_NOSTICKYTHREADS`, reading transactions do not use TLS (Thread Local Storage), and the
   * MVCC-snapshot lock slots in a readers table are becomes linked only to transactions. The completion of any threads
   * does not lead to the release of MVCC snapshot locks until the transactions are explicitly completed, or until the
   * corresponding process as a whole is completed. For writing transactions, there is no checking of the correspondence
   * between the current execution thread and the thread that created the transaction. However, the commit or
   * interruption of writing transactions must be performed strictly in the thread that started the transaction, since
   * these operations are associated with the acquire and release of synchronization primitives (mutexes, semaphores,
   * critical sections), for which most operating systems require the release only by the thread that acquired the
   * resource.
   *
   * This flag takes effect when the environment is opened and cannot be changed after. */
  MDBX_NOSTICKYTHREADS = UINT32_C(0x200000),

  /** \deprecated Please use \ref MDBX_NOSTICKYTHREADS instead. */
  MDBX_NOTLS MDBX_DEPRECATED_ENUM = MDBX_NOSTICKYTHREADS,

  /** Don't do readahead.
   *
   * Turn off readahead. Most operating systems perform readahead on read
   * requests by default. This option turns it off if the OS supports it.
   * Turning it off may help random read performance when the DB is larger
   * than RAM and system RAM is full.
   *
   * By default libmdbx dynamically enables/disables readahead depending on
   * the actual database size and currently available memory. On the other
   * hand, such automation has some limitation, i.e. could be performed only
   * when DB size changing but can't tracks and reacts changing a free RAM
   * availability, since it changes independently and asynchronously.
   *
   * \note The mdbx_is_readahead_reasonable() function allows to quickly find
   * out whether to use readahead or not based on the size of the data and the
   * amount of available memory.
   *
   * This flag affects only at environment opening and can't be changed after.
   */
  MDBX_NORDAHEAD = UINT32_C(0x800000),

  /** Don't initialize malloc'ed memory before writing to datafile.
   *
   * Don't initialize malloc'ed memory before writing to unused spaces in the
   * data file. By default, memory for pages written to the data file is
   * obtained using malloc. While these pages may be reused in subsequent
   * transactions, freshly malloc'ed pages will be initialized to zeroes before
   * use. This avoids persisting leftover data from other code (that used the
   * heap and subsequently freed the memory) into the data file.
   *
   * Note that many other system libraries may allocate and free memory from
   * the heap for arbitrary uses. E.g., stdio may use the heap for file I/O
   * buffers. This initialization step has a modest performance cost so some
   * applications may want to disable it using this flag. This option can be a
   * problem for applications which handle sensitive data like passwords, and
   * it makes memory checkers like Valgrind noisy. The initialization is skipped
   * if \ref MDBX_RESERVE is used; the caller is expected to overwrite all of
   * the memory that was reserved in that case.
   *
   * This flag may be changed at any time using `mdbx_env_set_flags()`. */
  MDBX_NOMEMINIT = UINT32_C(0x1000000),

  /** Aims to coalesce a Garbage Collection items.
   * \deprecated Always enabled since v0.12 and deprecated since v0.13.
   *
   * With `MDBX_COALESCE` flag MDBX will aims to coalesce items while recycling
   * a Garbage Collection. Technically, when possible short lists of pages
   * will be combined into longer ones, but to fit on one database page. As a
   * result, there will be fewer items in Garbage Collection and a page lists
   * are longer, which slightly increases the likelihood of returning pages to
   * Unallocated space and reducing the database file.
   *
   * This flag may be changed at any time using mdbx_env_set_flags(). */
  MDBX_COALESCE MDBX_DEPRECATED_ENUM = UINT32_C(0x2000000),

  /** LIFO policy for recycling a Garbage Collection items.
   *
   * `MDBX_LIFORECLAIM` flag turns on LIFO policy for recycling a Garbage
   * Collection items, instead of FIFO by default. On systems with a disk
   * write-back cache, this can significantly increase write performance, up
   * to several times in a best case scenario.
   *
   * LIFO recycling policy means that for reuse pages will be taken which became
   * unused the lastest (i.e. just now or most recently). Therefore the loop of
   * database pages circulation becomes as short as possible. In other words,
   * the number of pages, that are overwritten in memory and on disk during a
   * series of write transactions, will be as small as possible. Thus creates
   * ideal conditions for the efficient operation of the disk write-back cache.
   *
   * \ref MDBX_LIFORECLAIM is compatible with all no-sync flags, but gives NO
   * noticeable impact in combination with \ref MDBX_SAFE_NOSYNC or
   * \ref MDBX_UTTERLY_NOSYNC. Because MDBX will reused pages only before the
   * last "steady" MVCC-snapshot, i.e. the loop length of database pages
   * circulation will be mostly defined by frequency of calling
   * \ref mdbx_env_sync() rather than LIFO and FIFO difference.
   *
   * This flag may be changed at any time using mdbx_env_set_flags(). */
  MDBX_LIFORECLAIM = UINT32_C(0x4000000),

  /** Debugging option, fill/perturb released pages. */
  MDBX_PAGEPERTURB = UINT32_C(0x8000000),

  /* SYNC MODES****************************************************************/
  /** \defgroup sync_modes SYNC MODES
   *
   * \attention Using any combination of \ref MDBX_SAFE_NOSYNC, \ref
   * MDBX_NOMETASYNC and especially \ref MDBX_UTTERLY_NOSYNC is always a deal to
   * reduce durability for gain write performance. You must know exactly what
   * you are doing and what risks you are taking!
   *
   * \note for LMDB users: \ref MDBX_SAFE_NOSYNC is NOT similar to LMDB_NOSYNC,
   * but \ref MDBX_UTTERLY_NOSYNC is exactly match LMDB_NOSYNC. See details
   * below.
   *
   * THE SCENE:
   * - The DAT-file contains several MVCC-snapshots of B-tree at same time,
   *   each of those B-tree has its own root page.
   * - Each of meta pages at the beginning of the DAT file contains a
   *   pointer to the root page of B-tree which is the result of the particular
   *   transaction, and a number of this transaction.
   * - For data durability, MDBX must first write all MVCC-snapshot data
   *   pages and ensure that are written to the disk, then update a meta page
   *   with the new transaction number and a pointer to the corresponding new
   *   root page, and flush any buffers yet again.
   * - Thus during commit a I/O buffers should be flushed to the disk twice;
   *   i.e. fdatasync(), FlushFileBuffers() or similar syscall should be
   *   called twice for each commit. This is very expensive for performance,
   *   but guaranteed durability even on unexpected system failure or power
   *   outage. Of course, provided that the operating system and the
   *   underlying hardware (e.g. disk) work correctly.
   *
   * TRADE-OFF:
   * By skipping some stages described above, you can significantly benefit in
   * speed, while partially or completely losing in the guarantee of data
   * durability and/or consistency in the event of system or power failure.
   * Moreover, if for any reason disk write order is not preserved, then at
   * moment of a system crash, a meta-page with a pointer to the new B-tree may
   * be written to disk, while the itself B-tree not yet. In that case, the
   * database will be corrupted!
   *
   * \see MDBX_SYNC_DURABLE \see MDBX_NOMETASYNC \see MDBX_SAFE_NOSYNC
   * \see MDBX_UTTERLY_NOSYNC
   *
   * @{ */

  /** Default robust and durable sync mode.
   *
   * Metadata is written and flushed to disk after a data is written and
   * flushed, which guarantees the integrity of the database in the event
   * of a crash at any time.
   *
   * \attention Please do not use other modes until you have studied all the
   * details and are sure. Otherwise, you may lose your users' data, as happens
   * in [Miranda NG](https://www.miranda-ng.org/) messenger. */
  MDBX_SYNC_DURABLE = 0,

  /** Don't sync the meta-page after commit.
   *
   * Flush system buffers to disk only once per transaction commit, omit the
   * metadata flush. Defer that until the system flushes files to disk,
   * or next non-\ref MDBX_RDONLY commit or \ref mdbx_env_sync(). Depending on
   * the platform and hardware, with \ref MDBX_NOMETASYNC you may get a doubling
   * of write performance.
   *
   * This trade-off maintains database integrity, but a system crash may
   * undo the last committed transaction. I.e. it preserves the ACI
   * (atomicity, consistency, isolation) but not D (durability) database
   * property.
   *
   * `MDBX_NOMETASYNC` flag may be changed at any time using
   * \ref mdbx_env_set_flags() or by passing to \ref mdbx_txn_begin() for
   * particular write transaction. \see sync_modes */
  MDBX_NOMETASYNC = UINT32_C(0x40000),

  /** Don't sync anything but keep previous steady commits.
   *
   * Like \ref MDBX_UTTERLY_NOSYNC the `MDBX_SAFE_NOSYNC` flag disable similarly
   * flush system buffers to disk when committing a transaction. But there is a
   * huge difference in how are recycled the MVCC snapshots corresponding to
   * previous "steady" transactions (see below).
   *
   * Depending on the platform and hardware, with `MDBX_SAFE_NOSYNC` you may get
   * a multiple increase of write performance, even 10 times or more.
   *
   * In contrast to \ref MDBX_UTTERLY_NOSYNC mode, with `MDBX_SAFE_NOSYNC` flag
   * MDBX will keeps untouched pages within B-tree of the last transaction
   * "steady" which was synced to disk completely. This has big implications for
   * both data durability and (unfortunately) performance:
   *  - a system crash can't corrupt the database, but you will lose the last
   *    transactions; because MDBX will rollback to last steady commit since it
   *    kept explicitly.
   *  - the last steady transaction makes an effect similar to "long-lived" read
   *    transaction (see above in the \ref restrictions section) since prevents
   *    reuse of pages freed by newer write transactions, thus the any data
   *    changes will be placed in newly allocated pages.
   *  - to avoid rapid database growth, the system will sync data and issue
   *    a steady commit-point to resume reuse pages, each time there is
   *    insufficient space and before increasing the size of the file on disk.
   *
   * In other words, with `MDBX_SAFE_NOSYNC` flag MDBX ensures you from the
   * whole database corruption, at the cost increasing database size and/or
   * number of disk IOPs. So, `MDBX_SAFE_NOSYNC` flag could be used with
   * \ref mdbx_env_sync() as alternatively for batch committing or nested
   * transaction (in some cases). As well, auto-sync feature exposed by
   * \ref mdbx_env_set_syncbytes() and \ref mdbx_env_set_syncperiod() functions
   * could be very useful with `MDBX_SAFE_NOSYNC` flag.
   *
   * The number and volume of disk IOPs with MDBX_SAFE_NOSYNC flag will
   * exactly the as without any no-sync flags. However, you should expect a
   * larger process's [work set](https://bit.ly/2kA2tFX) and significantly worse
   * a [locality of reference](https://bit.ly/2mbYq2J), due to the more
   * intensive allocation of previously unused pages and increase the size of
   * the database.
   *
   * `MDBX_SAFE_NOSYNC` flag may be changed at any time using
   * \ref mdbx_env_set_flags() or by passing to \ref mdbx_txn_begin() for
   * particular write transaction. */
  MDBX_SAFE_NOSYNC = UINT32_C(0x10000),

  /** \deprecated Please use \ref MDBX_SAFE_NOSYNC instead of `MDBX_MAPASYNC`.
   *
   * Since version 0.9.x the public `MDBX_MAPASYNC` name is deprecated and is
   * an alias for \ref MDBX_SAFE_NOSYNC. It no longer implies writable data-file
   * mmap, because \ref MDBX_WRITEMAP is unsupported by the explicit-I/O
   * backend. */
  MDBX_MAPASYNC = MDBX_SAFE_NOSYNC,

  /** Don't sync anything and wipe previous steady commits.
   *
   * Don't flush system buffers to disk when committing a transaction. This
   * optimization means a system crash can corrupt the database, if buffers are
   * not yet flushed to disk. Depending on the platform and hardware, with
   * `MDBX_UTTERLY_NOSYNC` you may get a multiple increase of write performance,
   * even 100 times or more.
   *
   * If the filesystem preserves write order (which is rare and never provided
   * unless explicitly noted) and the \ref MDBX_LIFORECLAIM flag is not used,
   * then a system crash can't corrupt the database, but you can lose the last
   * transactions, if at least one buffer is not yet flushed to disk. The risk
   * is governed by how often the system flushes dirty buffers to disk and how
   * often \ref mdbx_env_sync() is called. So, transactions exhibit ACI
   * (atomicity, consistency, isolation) properties and only lose `D`
   * (durability). I.e. database integrity is maintained, but a system crash may
   * undo the final transactions.
   *
   * Otherwise, if the filesystem not preserves write order (which is
   * typically) or \ref MDBX_LIFORECLAIM flag is used, you should expect the
   * corrupted database after a system crash.
   *
   * So, most important thing about `MDBX_UTTERLY_NOSYNC`:
   *  - a system crash immediately after commit the write transaction
   *    high likely lead to database corruption.
   *  - successful completion of mdbx_env_sync(force = true) after one or
   *    more committed transactions guarantees consistency and durability.
   *  - BUT by committing two or more transactions you back database into
   *    a weak state, in which a system crash may lead to database corruption!
   *    In case single transaction after mdbx_env_sync, you may lose transaction
   *    itself, but not a whole database.
   *
   * Nevertheless, `MDBX_UTTERLY_NOSYNC` provides "weak" durability in case
   * of an application crash (but no durability on system failure), and
   * therefore may be very useful in scenarios where data durability is
   * not required over a system failure (e.g for short-lived data), or if you
   * can take such risk.
   *
   * `MDBX_UTTERLY_NOSYNC` flag may be changed at any time using
   * \ref mdbx_env_set_flags(), but don't has effect if passed to
   * \ref mdbx_txn_begin() for particular write transaction. \see sync_modes */
  MDBX_UTTERLY_NOSYNC = MDBX_SAFE_NOSYNC | UINT32_C(0x100000),

  /** end of sync_modes @} */
} MDBX_env_flags_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_env_flags)

/** Transaction flags
 * \ingroup c_transactions
 * \anchor txn_flags
 * \see mdbx_txn_begin() \see mdbx_txn_flags() */
typedef enum MDBX_txn_flags {
  /** Start read-write transaction.
   *
   * Only one write transaction may be active at a time. Writes are fully
   * serialized, which guarantees that writers can never deadlock. */
  MDBX_TXN_READWRITE = 0,

  /** Start read-only transaction.
   *
   * There can be multiple read-only transactions simultaneously that do not
   * block each other and a write transactions. */
  MDBX_TXN_RDONLY = MDBX_RDONLY,

/** Prepare but not start read-only transaction.
 *
 * Transaction will not be started immediately, but created transaction handle
 * will be ready for use with \ref mdbx_txn_renew(). This flag allows to
 * preallocate memory and assign a reader slot, thus avoiding these operations
 * at the next start of the transaction. */
#if CONSTEXPR_ENUM_FLAGS_OPERATIONS || defined(DOXYGEN)
  MDBX_TXN_RDONLY_PREPARE = MDBX_RDONLY | MDBX_NOMEMINIT,
#else
  MDBX_TXN_RDONLY_PREPARE = uint32_t(MDBX_RDONLY) | uint32_t(MDBX_NOMEMINIT),
#endif

  /** Do not block when starting a write transaction. */
  MDBX_TXN_TRY = UINT32_C(0x10000000),

  /** Do not weakening durability for a transaction but using mode of the environment. */
  MDBX_TXN_NOWEAKING = 0,

  /** Exactly the same as \ref MDBX_NOMETASYNC,
   * but for this transaction only. */
  MDBX_TXN_NOMETASYNC = MDBX_NOMETASYNC,

  /** Exactly the same as \ref MDBX_SAFE_NOSYNC,
   * but for this transaction only. */
  MDBX_TXN_NOSYNC = MDBX_SAFE_NOSYNC,

  /* Transaction state flags ---------------------------------------------- */

  /** Transaction is invalid.
   * \note Transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_INVALID = INT32_MIN,

  /** Transaction is finished or never began.
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_FINISHED = 0x01,

  /** Transaction is unusable after an error.
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_ERROR = 0x02,

  /** Transaction must write, even if dirty list is empty.
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_DIRTY = 0x04,

  /** Transaction or a parent has spilled pages.
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_SPILLS = 0x08,

  /** Transaction has a nested child transaction.
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_HAS_CHILD = 0x10,

  /** Transaction is parked by \ref mdbx_txn_park().
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_PARKED = 0x20,

  /** Transaction is parked by \ref mdbx_txn_park() with `autounpark=true`,
   * and therefore it can be used without explicitly calling
   * \ref mdbx_txn_unpark() first.
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_AUTOUNPARK = 0x40,

  /** The transaction was blocked using the \ref mdbx_txn_park() function,
   * and then ousted by a write transaction because
   * this transaction was interfered with garbage recycling.
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_OUSTED = 0x80,

  /** Most operations on the transaction are currently illegal.
   * \note This is a transaction state flag. Returned from \ref mdbx_txn_flags()
   * but can't be used with \ref mdbx_txn_begin(). */
  MDBX_TXN_BLOCKED = MDBX_TXN_FINISHED | MDBX_TXN_ERROR | MDBX_TXN_HAS_CHILD | MDBX_TXN_PARKED
} MDBX_txn_flags_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_txn_flags)

/** \brief Table flags
 * \ingroup c_dbi
 * \anchor db_flags
 * \see mdbx_dbi_open() */
typedef enum MDBX_db_flags {
  /** Variable length unique keys with usual byte-by-byte string comparison. */
  MDBX_DB_DEFAULTS = 0,

  /** Use reverse string comparison for keys. */
  MDBX_REVERSEKEY = UINT32_C(0x02),

  /** Use sorted duplicates, i.e. allow multi-values for a keys. */
  MDBX_DUPSORT = UINT32_C(0x04),

  /** Numeric keys in native byte order either uint32_t or uint64_t
   * (must be one of uint32_t or uint64_t, other integer types, for example,
   * signed integer or uint16_t will not work).
   * The keys must all be of the same size and must be aligned while passing as
   * arguments. */
  MDBX_INTEGERKEY = UINT32_C(0x08),

  /** With \ref MDBX_DUPSORT; sorted dup items have fixed size. The data values
   * must all be of the same size. */
  MDBX_DUPFIXED = UINT32_C(0x10),

  /** With \ref MDBX_DUPSORT and with \ref MDBX_DUPFIXED; dups are fixed size
   * like \ref MDBX_INTEGERKEY -style integers. The data values must all be of
   * the same size and must be aligned while passing as arguments. */
  MDBX_INTEGERDUP = UINT32_C(0x20),

  /** With \ref MDBX_DUPSORT; use reverse string comparison for data values. */
  MDBX_REVERSEDUP = UINT32_C(0x40),

  /** Create DB if not already existing. */
  MDBX_CREATE = UINT32_C(0x40000),

  /** Opens an existing table created with unknown flags.
   *
   * The `MDBX_DB_ACCEDE` flag is intend to open a existing table which
   * was created with unknown flags (\ref MDBX_REVERSEKEY, \ref MDBX_DUPSORT,
   * \ref MDBX_INTEGERKEY, \ref MDBX_DUPFIXED, \ref MDBX_INTEGERDUP and
   * \ref MDBX_REVERSEDUP).
   *
   * In such cases, instead of returning the \ref MDBX_INCOMPATIBLE error, the
   * table will be opened with flags which it was created, and then an
   * application could determine the actual flags by \ref mdbx_dbi_flags(). */
  MDBX_DB_ACCEDE = MDBX_ACCEDE
} MDBX_db_flags_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_db_flags)

/** \brief Data changing flags
 * \ingroup c_crud
 * \see \ref c_crud_hints "Quick reference for Insert/Update/Delete operations"
 * \see mdbx_put() \see mdbx_cursor_put() \see mdbx_replace() */
typedef enum MDBX_put_flags {
  /** Upsertion by default (without any other flags) */
  MDBX_UPSERT = 0,

  /** For insertion: Don't write if the key already exists. */
  MDBX_NOOVERWRITE = UINT32_C(0x10),

  /** Has effect only for \ref MDBX_DUPSORT tables.
   * For upsertion: don't write if the key-value pair already exist. */
  MDBX_NODUPDATA = UINT32_C(0x20),

  /** For upsertion: overwrite the current key/data pair.
   * MDBX allows this flag for \ref mdbx_put() for explicit overwrite/update
   * without insertion.
   * For deletion: remove only single entry at the current cursor position. */
  MDBX_CURRENT = UINT32_C(0x40),

  /** Has effect only for \ref MDBX_DUPSORT tables.
   * For deletion: remove all multi-values (aka duplicates) for given key.
   * For upsertion: replace all multi-values for given key with a new one. */
  MDBX_ALLDUPS = UINT32_C(0x80),

  /** For upsertion: Just reserve space for data, don't copy it.
   * Return a pointer to the reserved space. */
  MDBX_RESERVE = UINT32_C(0x10000),

  /** Data is being appended.
   * Don't split full pages, continue on a new instead. */
  MDBX_APPEND = UINT32_C(0x20000),

  /** Has effect only for \ref MDBX_DUPSORT tables.
   * Duplicate data is being appended.
   * Don't split full pages, continue on a new instead. */
  MDBX_APPENDDUP = UINT32_C(0x40000),

  /** Only for \ref MDBX_DUPFIXED.
   * Store multiple data items in one call. */
  MDBX_MULTIPLE = UINT32_C(0x80000)
} MDBX_put_flags_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_put_flags)

/** \brief Environment copy flags
 * \ingroup c_extra
 * \see mdbx_env_copy() \see mdbx_env_copy2fd() \see mdbx_txn_copy2pathname() */
typedef enum MDBX_copy_flags {
  MDBX_CP_DEFAULTS = 0,

  /** Copy with compactification: Omit free space from copy and renumber all
   * pages sequentially */
  MDBX_CP_COMPACT = 1u,

  /** Force to make resizable copy, i.e. dynamic size instead of fixed */
  MDBX_CP_FORCE_DYNAMIC_SIZE = 2u,

  /** Don't explicitly flush the written data to an output media */
  MDBX_CP_DONT_FLUSH = 4u,

  /** Use read transaction parking during copying MVCC-snapshot
   * \see mdbx_txn_park() */
  MDBX_CP_THROTTLE_MVCC = 8u,

  /** Abort/dispose passed transaction after copy
   * \see mdbx_txn_copy2fd() \see mdbx_txn_copy2pathname() */
  MDBX_CP_DISPOSE_TXN = 16u,

  /** Enable renew/restart read transaction in case it use outdated
   * MVCC shapshot, otherwise the \ref MDBX_MVCC_RETARDED will be returned
   * \see mdbx_txn_copy2fd() \see mdbx_txn_copy2pathname() */
  MDBX_CP_RENEW_TXN = 32u,

  /** Silently overwrite the target file, if it exists, instead of returning an error
   * \see mdbx_txn_copy2pathname() \see mdbx_env_copy() */
  MDBX_CP_OVERWRITE = 64u

} MDBX_copy_flags_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_copy_flags)

/** \brief Cursor operations
 * \ingroup c_cursors
 * This is the set of all operations for retrieving data using a cursor.
 * \see mdbx_cursor_get() */
typedef enum MDBX_cursor_op {
  /** Position at first key/data item */
  MDBX_FIRST,

  /** \ref MDBX_DUPSORT -only: Position at first data item of current key. */
  MDBX_FIRST_DUP,

  /** \ref MDBX_DUPSORT -only: Position at key/data pair. */
  MDBX_GET_BOTH,

  /** \ref MDBX_DUPSORT -only: Position at given key and at first data greater
   * than or equal to specified data. */
  MDBX_GET_BOTH_RANGE,

  /** Return key/data at current cursor position */
  MDBX_GET_CURRENT,

  /** \ref MDBX_DUPFIXED -only: Return up to a page of duplicate data items
   * from current cursor position. Move cursor to prepare
   * for \ref MDBX_NEXT_MULTIPLE. \see MDBX_SEEK_AND_GET_MULTIPLE */
  MDBX_GET_MULTIPLE,

  /** Position at last key/data item */
  MDBX_LAST,

  /** \ref MDBX_DUPSORT -only: Position at last data item of current key. */
  MDBX_LAST_DUP,

  /** Position at next data item */
  MDBX_NEXT,

  /** \ref MDBX_DUPSORT -only: Position at next data item of current key. */
  MDBX_NEXT_DUP,

  /** \ref MDBX_DUPFIXED -only: Return up to a page of duplicate data items
   * from next cursor position. Move cursor to prepare for `MDBX_NEXT_MULTIPLE`.
   * \see MDBX_SEEK_AND_GET_MULTIPLE \see MDBX_GET_MULTIPLE */
  MDBX_NEXT_MULTIPLE,

  /** Position at first data item of next key */
  MDBX_NEXT_NODUP,

  /** Position at previous data item */
  MDBX_PREV,

  /** \ref MDBX_DUPSORT -only: Position at previous data item of current key. */
  MDBX_PREV_DUP,

  /** Position at last data item of previous key */
  MDBX_PREV_NODUP,

  /** Position at specified key */
  MDBX_SET,

  /** Position at specified key, return both key and data */
  MDBX_SET_KEY,

  /** Position at first key greater than or equal to specified key. */
  MDBX_SET_RANGE,

  /** \ref MDBX_DUPFIXED -only: Position at previous page and return up to
   * a page of duplicate data items.
   * \see MDBX_SEEK_AND_GET_MULTIPLE \see MDBX_GET_MULTIPLE */
  MDBX_PREV_MULTIPLE,

  /** Positions cursor at first key-value pair greater than or equal to
   * specified, return both key and data, and the return code depends on whether
   * a exact match.
   *
   * For non DUPSORT-ed collections this work the same to \ref MDBX_SET_RANGE,
   * but returns \ref MDBX_SUCCESS if key found exactly or
   * \ref MDBX_RESULT_TRUE if greater key was found.
   *
   * For DUPSORT-ed a data value is taken into account for duplicates,
   * i.e. for a pairs/tuples of a key and an each data value of duplicates.
   * Returns \ref MDBX_SUCCESS if key-value pair found exactly or
   * \ref MDBX_RESULT_TRUE if the next pair was returned. */
  MDBX_SET_LOWERBOUND,

  /** Positions cursor at first key-value pair greater than specified,
   * return both key and data, and the return code depends on whether a
   * upper-bound was found.
   *
   * For non DUPSORT-ed collections this work like \ref MDBX_SET_RANGE,
   * but returns \ref MDBX_SUCCESS if the greater key was found or
   * \ref MDBX_NOTFOUND otherwise.
   *
   * For DUPSORT-ed a data value is taken into account for duplicates,
   * i.e. for a pairs/tuples of a key and an each data value of duplicates.
   * Returns \ref MDBX_SUCCESS if the greater pair was returned or
   * \ref MDBX_NOTFOUND otherwise. */
  MDBX_SET_UPPERBOUND,

  /** Doubtless cursor positioning at a specified key. */
  MDBX_TO_KEY_LESSER_THAN,
  MDBX_TO_KEY_LESSER_OR_EQUAL /** \copydoc MDBX_TO_KEY_LESSER_THAN */,
  MDBX_TO_KEY_EQUAL /** \copydoc MDBX_TO_KEY_LESSER_THAN */,
  MDBX_TO_KEY_GREATER_OR_EQUAL /** \copydoc MDBX_TO_KEY_LESSER_THAN */,
  MDBX_TO_KEY_GREATER_THAN /** \copydoc MDBX_TO_KEY_LESSER_THAN */,

  /** Doubtless cursor positioning at a specified key-value pair
   * for dupsort/multi-value hives. */
  MDBX_TO_EXACT_KEY_VALUE_LESSER_THAN,
  MDBX_TO_EXACT_KEY_VALUE_LESSER_OR_EQUAL /** \copydoc MDBX_TO_EXACT_KEY_VALUE_LESSER_THAN */,
  MDBX_TO_EXACT_KEY_VALUE_EQUAL /** \copydoc MDBX_TO_EXACT_KEY_VALUE_LESSER_THAN */,
  MDBX_TO_EXACT_KEY_VALUE_GREATER_OR_EQUAL /** \copydoc MDBX_TO_EXACT_KEY_VALUE_LESSER_THAN */,
  MDBX_TO_EXACT_KEY_VALUE_GREATER_THAN /** \copydoc MDBX_TO_EXACT_KEY_VALUE_LESSER_THAN */,

  /** Doubtless cursor positioning at a specified key-value pair
   * for dupsort/multi-value hives. */
  MDBX_TO_PAIR_LESSER_THAN,
  MDBX_TO_PAIR_LESSER_OR_EQUAL /** \copydoc MDBX_TO_PAIR_LESSER_THAN */,
  MDBX_TO_PAIR_EQUAL /** \copydoc MDBX_TO_PAIR_LESSER_THAN */,
  MDBX_TO_PAIR_GREATER_OR_EQUAL /** \copydoc MDBX_TO_PAIR_LESSER_THAN */,
  MDBX_TO_PAIR_GREATER_THAN /** \copydoc MDBX_TO_PAIR_LESSER_THAN */,

  /** \ref MDBX_DUPFIXED -only: Seek to given key and return up to a page of
   * duplicate data items from current cursor position. Move cursor to prepare
   * for \ref MDBX_NEXT_MULTIPLE. \see MDBX_GET_MULTIPLE */
  MDBX_SEEK_AND_GET_MULTIPLE
} MDBX_cursor_op;

/** \brief Errors and return codes
 * \ingroup c_err
 *
 * BerkeleyDB uses -30800 to -30999, we'll go under them
 * \see mdbx_strerror() \see mdbx_strerror_r() \see mdbx_liberr2str() */
typedef enum MDBX_error {
  /** Successful result */
  MDBX_SUCCESS = 0,

  /** Alias for \ref MDBX_SUCCESS */
  MDBX_RESULT_FALSE = MDBX_SUCCESS,

  /** Successful result with special meaning or a flag */
  MDBX_RESULT_TRUE = -1,

  /** key/data pair already exists */
  MDBX_KEYEXIST = -30799,

  /** The first LMDB-compatible defined error code */
  MDBX_FIRST_LMDB_ERRCODE = MDBX_KEYEXIST,

  /** key/data pair not found (EOF) */
  MDBX_NOTFOUND = -30798,

  /** Requested page not found - this usually indicates corruption */
  MDBX_PAGE_NOTFOUND = -30797,

  /** Database is corrupted (page was wrong type and so on) */
  MDBX_CORRUPTED = -30796,

  /** Environment had fatal error,
   * i.e. update of meta page failed and so on. */
  MDBX_PANIC = -30795,

  /** DB file version mismatch with libmdbx */
  MDBX_VERSION_MISMATCH = -30794,

  /** File is not a valid MDBX file */
  MDBX_INVALID = -30793,

  /** Environment mapsize reached */
  MDBX_MAP_FULL = -30792,

  /** Environment maxdbs reached */
  MDBX_DBS_FULL = -30791,

  /** Environment maxreaders reached */
  MDBX_READERS_FULL = -30790,

  /** Transaction has too many dirty pages, i.e transaction too big */
  MDBX_TXN_FULL = -30788,

  /** Cursor stack too deep - this usually indicates corruption,
   * i.e branch-pages loop */
  MDBX_CURSOR_FULL = -30787,

  /** Page has not enough space - internal error */
  MDBX_PAGE_FULL = -30786,

  /** Database engine was unable to extend mapping, e.g. since address space
   * is unavailable or busy. This can mean:
   *  - Database size extended by other process beyond to environment mapsize
   *    and engine was unable to extend mapping while starting read
   *    transaction. Environment should be reopened to continue.
   *  - Engine was unable to extend mapping during write transaction
   *    or explicit call of \ref mdbx_env_set_geometry(). */
  MDBX_UNABLE_EXTEND_MAPSIZE = -30785,

  /** Environment or table is not compatible with the requested operation
   * or the specified flags. This can mean:
   *  - The operation expects an \ref MDBX_DUPSORT / \ref MDBX_DUPFIXED table.
   *  - Opening a named DB when the unnamed DB has \ref MDBX_DUPSORT / \ref MDBX_INTEGERKEY.
   *  - Accessing a data record as a named table, or vice versa.
   *  - The table was dropped and recreated with different flags. */
  MDBX_INCOMPATIBLE = -30784,

  /** Reader locktable slot was unexpectly reused or cleared by an enemy thread */
  MDBX_BAD_RSLOT = -30783,

  /** Transaction is not valid for requested operation,
   * e.g. had errored and be must aborted, has a child/nested transaction,
   * or is invalid */
  MDBX_BAD_TXN = -30782,

  /** Invalid size or alignment of key or data for target table,
   * either invalid table name */
  MDBX_BAD_VALSIZE = -30781,

  /** The specified DBI-handle is invalid
   * or changed by another thread/transaction */
  MDBX_BAD_DBI = -30780,

  /** Unexpected internal error, transaction should be aborted */
  MDBX_PROBLEM = -30779,

  /** The last LMDB-compatible defined error code */
  MDBX_LAST_LMDB_ERRCODE = MDBX_PROBLEM,

  /** Another write transaction is running or environment is already used while
   * opening with \ref MDBX_EXCLUSIVE flag */
  MDBX_BUSY = -30778,

  /** The first of MDBX-added error codes */
  MDBX_FIRST_ADDED_ERRCODE = MDBX_BUSY,

  /** The specified key has more than one associated value */
  MDBX_EMULTIVAL = -30421,

  /** Bad signature of a runtime object(s), this can mean:
   *  - memory corruption or double-free;
   *  - ABI version mismatch (rare case); */
  MDBX_EBADSIGN = -30420,

  /** Database should be recovered, but this could NOT be done for now
   * since it opened in read-only mode */
  MDBX_WANNA_RECOVERY = -30419,

  /** The given key value is mismatched to the current cursor position */
  MDBX_EKEYMISMATCH = -30418,

  /** Database is too large for current system,
   * e.g. could NOT be mapped into RAM. */
  MDBX_TOO_LARGE = -30417,

  /** A thread has attempted to use a not owned object,
   * e.g. a transaction that started by another thread */
  MDBX_THREAD_MISMATCH = -30416,

  /** Overlapping read and write transactions for the current thread */
  MDBX_TXN_OVERLAPPING = -30415,

  /** An internal error returned if there is a shortage of available pages when updating the GC.
   *  It is used as an auxiliary tool for debugging.
   * \note From the user's point of view, it is semantically equivalent to \ref MDBX_PROBLEM. */
  MDBX_BACKLOG_DEPLETED = -30414,

  /** Alternative/Duplicate LCK-file is exists and should be removed manually */
  MDBX_DUPLICATED_CLK = -30413,

  /** Some cursors and/or other resources should be closed before table or
   *  corresponding DBI-handle could be (re)used and/or closed. */
  MDBX_DANGLING_DBI = -30412,

  /** The parked read transaction was outed for the sake of
   * recycling old MVCC snapshots. */
  MDBX_OUSTED = -30411,

  /** MVCC snapshot used by parked transaction was bygone. */
  MDBX_MVCC_RETARDED = -30410,

  /** An operation cannot continue because a lagging reader is interfering
   *  with the reclaiming of GC and old MVCC-snapshots. */
  MDBX_LAGGARD_READER = -30409,

  /* The last of MDBX-added error codes */
  MDBX_LAST_ADDED_ERRCODE = MDBX_LAGGARD_READER,

#if defined(_WIN32) || defined(_WIN64)
  MDBX_ENODATA = ERROR_HANDLE_EOF,
  MDBX_EINVAL = ERROR_INVALID_PARAMETER,
  MDBX_EACCESS = ERROR_ACCESS_DENIED,
  MDBX_ENOMEM = ERROR_OUTOFMEMORY,
  MDBX_EROFS = ERROR_FILE_READ_ONLY,
  MDBX_ENOSYS = ERROR_NOT_SUPPORTED,
  MDBX_EIO = ERROR_WRITE_FAULT,
  MDBX_EPERM = ERROR_INVALID_FUNCTION,
  MDBX_EINTR = ERROR_CANCELLED,
  MDBX_ENOFILE = ERROR_FILE_NOT_FOUND,
  MDBX_EREMOTE = ERROR_REMOTE_STORAGE_MEDIA_ERROR,
  MDBX_EDEADLK = ERROR_POSSIBLE_DEADLOCK
#else /* Windows */
#if defined(ENODATA) || defined(DOXYGEN)
  MDBX_ENODATA = ENODATA,
#else
  MDBX_ENODATA = 9919 /* for compatibility with LLVM's C++ libraries/headers */,
#endif /* ENODATA */
  MDBX_EINVAL = EINVAL,
  MDBX_EACCESS = EACCES,
  MDBX_ENOMEM = ENOMEM,
  MDBX_EROFS = EROFS,
#if defined(ENOTSUP) || defined(DOXYGEN)
  MDBX_ENOSYS = ENOTSUP,
#else
  MDBX_ENOSYS = ENOSYS,
#endif /* ENOTSUP */
  MDBX_EIO = EIO,
  MDBX_EPERM = EPERM,
  MDBX_EINTR = EINTR,
  MDBX_ENOFILE = ENOENT,
#if defined(EREMOTEIO) || defined(DOXYGEN)
  /** Cannot use the database on a network file system or when exporting it via NFS. */
  MDBX_EREMOTE = EREMOTEIO,
#else
  MDBX_EREMOTE = ENOTBLK,
#endif /* EREMOTEIO */
  MDBX_EDEADLK = EDEADLK
#endif /* !Windows */
} MDBX_error_t;

/** MDBX_MAP_RESIZED
 * \ingroup c_err
 * \deprecated Please review your code to use MDBX_UNABLE_EXTEND_MAPSIZE
 * instead. */
MDBX_DEPRECATED static __inline int MDBX_MAP_RESIZED_is_deprecated(void) { return MDBX_UNABLE_EXTEND_MAPSIZE; }
#define MDBX_MAP_RESIZED MDBX_MAP_RESIZED_is_deprecated()

/** \brief Return a string describing a given error code.
 * \ingroup c_err
 *
 * This function is a superset of the ANSI C X3.159-1989 (ANSI C) `strerror()`
 * function. If the error code is greater than or equal to 0, then the string
 * returned by the system function `strerror()` is returned. If the error code
 * is less than 0, an error string corresponding to the MDBX library error is
 * returned. See errors for a list of MDBX-specific error codes.
 *
 * `mdbx_strerror()` is NOT thread-safe because may share common internal buffer
 * for system messages. The returned string must NOT be modified by the
 * application, but MAY be modified by a subsequent call to
 * \ref mdbx_strerror(), `strerror()` and other related functions.
 * \see mdbx_strerror_r()
 *
 * \param [in] errnum  The error code.
 *
 * \returns "error message" The description of the error. */
LIBMDBX_API const char *mdbx_strerror(int errnum);

/** \brief Return a string describing a given error code.
 * \ingroup c_err
 *
 * This function is a superset of the ANSI C X3.159-1989 (ANSI C) `strerror()`
 * function. If the error code is greater than or equal to 0, then the string
 * returned by the system function `strerror()` is returned. If the error code
 * is less than 0, an error string corresponding to the MDBX library error is
 * returned. See errors for a list of MDBX-specific error codes.
 *
 * `mdbx_strerror_r()` is thread-safe since uses user-supplied buffer where
 * appropriate. The returned string must NOT be modified by the application,
 * since it may be pointer to internal constant string. However, there is no
 * restriction if the returned string points to the supplied buffer.
 * \see mdbx_strerror()
 *
 * mdbx_liberr2str() returns string describing only MDBX error numbers but NULL
 * for non-MDBX error codes. This function is thread-safe since return pointer
 * to constant non-localized strings.
 *
 * \param [in] errnum  The error code.
 * \param [in,out] buf Buffer to store the error message.
 * \param [in] buflen The size of buffer to store the message.
 *
 * \returns "error message" The description of the error. */
LIBMDBX_API const char *mdbx_strerror_r(int errnum, char *buf, size_t buflen);
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API const char *mdbx_liberr2str(int errnum);

#if defined(_WIN32) || defined(_WIN64) || defined(DOXYGEN)
/** Bit of Windows' madness. The similar to \ref mdbx_strerror() but returns
 * Windows error-messages in the OEM-encoding for console utilities.
 * \ingroup c_err
 * \see mdbx_strerror_r_ANSI2OEM() */
LIBMDBX_API const char *mdbx_strerror_ANSI2OEM(int errnum);

/** Bit of Windows' madness. The similar to \ref mdbx_strerror_r() but returns
 * Windows error-messages in the OEM-encoding for console utilities.
 * \ingroup c_err
 * \see mdbx_strerror_ANSI2OEM() */
LIBMDBX_API const char *mdbx_strerror_r_ANSI2OEM(int errnum, char *buf, size_t buflen);
#endif /* Bit of Windows' madness */

/** \brief Create an MDBX environment instance.
 * \ingroup c_opening
 *
 * This function allocates memory for a \ref MDBX_env structure. To release
 * the allocated memory and discard the handle, call \ref mdbx_env_close().
 * Before the handle may be used, it must be opened using \ref mdbx_env_open().
 *
 * Various other options may also need to be set before opening the handle,
 * e.g. \ref mdbx_env_set_geometry(), \ref mdbx_env_set_maxreaders(),
 * \ref mdbx_env_set_maxdbs(), depending on usage requirements.
 *
 * \param [out] penv  The address where the new handle will be stored.
 *
 * \returns a non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_create(MDBX_env **penv);

/** \brief MDBX environment extra runtime options.
 * \ingroup c_settings
 * \see mdbx_env_set_option() \see mdbx_env_get_option() */
typedef enum MDBX_option {
  /** \brief Controls the maximum number of named tables for the environment.
   *
   * \details By default only unnamed key-value table could used and
   * appropriate value should set by `MDBX_opt_max_db` to using any more named
   * table(s). To reduce overhead, use the minimum sufficient value. This option
   * may only set after \ref mdbx_env_create() and before \ref mdbx_env_open().
   *
   * \see mdbx_env_set_maxdbs() \see mdbx_env_get_maxdbs() */
  MDBX_opt_max_db,

  /** \brief Defines the maximum number of threads/reader slots
   * for all processes interacting with the database.
   *
   * \details This defines the number of slots in the lock table that is used to
   * track readers in the environment. The default is about 100 for 4K
   * system page size. Starting a read-only transaction normally ties a lock
   * table slot to the current thread until the environment closes or the thread
   * exits. If \ref MDBX_NOSTICKYTHREADS is in use, \ref mdbx_txn_begin()
   * instead ties the slot to the \ref MDBX_txn object until it or the \ref
   * MDBX_env object is destroyed. This option may only set after \ref
   * mdbx_env_create() and before \ref mdbx_env_open(), and has an effect only
   * when the database is opened by the first process interacts with the
   * database.
   *
   * \see mdbx_env_set_maxreaders() \see mdbx_env_get_maxreaders() */
  MDBX_opt_max_readers,

  /** \brief Controls interprocess/shared threshold to force flush the data
   * buffers to disk, if \ref MDBX_SAFE_NOSYNC is used.
   *
   * \see mdbx_env_set_syncbytes() \see mdbx_env_get_syncbytes() */
  MDBX_opt_sync_bytes,

  /** \brief Controls interprocess/shared relative period since the last
   * unsteady commit to force flush the data buffers to disk,
   * if \ref MDBX_SAFE_NOSYNC is used.
   * \see mdbx_env_set_syncperiod() \see mdbx_env_get_syncperiod() */
  MDBX_opt_sync_period,

  /** \brief Controls the in-process limit to grow a list of reclaimed/recycled
   * page's numbers for finding a sequence of contiguous pages for large data
   * items.
   * \see MDBX_opt_gc_time_limit
   *
   * \details A long values requires allocation of contiguous database pages.
   * To find such sequences, it may be necessary to accumulate very large lists,
   * especially when placing very long values (more than a megabyte) in a large
   * databases (several tens of gigabytes), which is much expensive in extreme
   * cases. This threshold allows you to avoid such costs by allocating new
   * pages at the end of the database (with its possible growth on disk),
   * instead of further accumulating/reclaiming Garbage Collection records.
   *
   * On the other hand, too small threshold will lead to unreasonable database
   * growth, or/and to the inability of put long values.
   *
   * The `MDBX_opt_rp_augment_limit` controls described limit for the current
   * process. By default this limit adjusted dynamically to 1/3 of current
   * quantity of DB pages, which is usually enough for most cases. */
  MDBX_opt_rp_augment_limit,

  /** \brief Controls the in-process limit to grow a cache of dirty
   * pages for reuse in the current transaction.
   *
   * \details A `dirty page` refers to a page that has been updated in memory
   * only, the changes to a dirty page are not yet stored on disk.
   * To reduce overhead, it is reasonable to release not all such pages
   * immediately, but to leave some ones in cache for reuse in the current
   * transaction.
   *
   * The `MDBX_opt_loose_limit` allows you to set a limit for such cache inside
   * the current process. Should be in the range 0..255, default is 64. */
  MDBX_opt_loose_limit,

  /** \brief Controls the in-process limit of a pre-allocated memory items
   * for dirty pages.
   *
   * \details A `dirty page` refers to a page that has been updated in memory
   * only, the changes to a dirty page are not yet stored on disk.
   * Dirty pages are allocated from memory and released when a transaction is
   * committed. To reduce overhead, it is reasonable to release not all ones,
   * but to leave some allocations in reserve for reuse in the next
   * transaction(s).
   *
   * The `MDBX_opt_dp_reserve_limit` allows you to set a limit for such reserve
   * inside the current process. Default is 1024. */
  MDBX_opt_dp_reserve_limit,

  /** \brief Controls the in-process limit of dirty pages
   * for a write transaction.
   *
   * \details A `dirty page` refers to a page that has been updated in memory
   * only, the changes to a dirty page are not yet stored on disk.
   * Dirty pages are allocated from memory and will be busy until are written to
   * disk. Therefore for a large transactions is reasonable to limit dirty pages
   * collecting above an some threshold but spill to disk instead.
   *
   * The `MDBX_opt_txn_dp_limit` controls described threshold for the current
   * process. Default is 1/42 of the sum of whole and currently available RAM
   * size, which the same ones are reported by \ref mdbx_get_sysraminfo(). */
  MDBX_opt_txn_dp_limit,

  /** \brief Controls the in-process initial allocation size for dirty pages
   * list of a write transaction. Default is 1024. */
  MDBX_opt_txn_dp_initial,

  /** \brief Controls the in-process how maximal part of the dirty pages may be
   * spilled when necessary.
   *
   * \details The `MDBX_opt_spill_max_denominator` defines the denominator for
   * limiting from the top for part of the current dirty pages may be spilled
   * when the free room for a new dirty pages (i.e. distance to the
   * `MDBX_opt_txn_dp_limit` threshold) is not enough to perform requested
   * operation.
   * Exactly `max_pages_to_spill = dirty_pages - dirty_pages / N`,
   * where `N` is the value set by `MDBX_opt_spill_max_denominator`.
   *
   * Should be in the range 0..255, where zero means no limit, i.e. all dirty
   * pages could be spilled. Default is 8, i.e. no more than 7/8 of the current
   * dirty pages may be spilled when reached the condition described above. */
  MDBX_opt_spill_max_denominator,

  /** \brief Controls the in-process how minimal part of the dirty pages should
   * be spilled when necessary.
   *
   * \details The `MDBX_opt_spill_min_denominator` defines the denominator for
   * limiting from the bottom for part of the current dirty pages should be
   * spilled when the free room for a new dirty pages (i.e. distance to the
   * `MDBX_opt_txn_dp_limit` threshold) is not enough to perform requested
   * operation.
   * Exactly `min_pages_to_spill = dirty_pages / N`,
   * where `N` is the value set by `MDBX_opt_spill_min_denominator`.
   *
   * Should be in the range 0..255, where zero means no restriction at the
   * bottom. Default is 8, i.e. at least the 1/8 of the current dirty pages
   * should be spilled when reached the condition described above. */
  MDBX_opt_spill_min_denominator,

  /** \brief Controls the in-process how much of the parent transaction dirty
   * pages will be spilled while start each child transaction.
   *
   * \details The `MDBX_opt_spill_parent4child_denominator` defines the
   * denominator to determine how much of parent transaction dirty pages will be
   * spilled explicitly while start each child transaction.
   * Exactly `pages_to_spill = dirty_pages / N`,
   * where `N` is the value set by `MDBX_opt_spill_parent4child_denominator`.
   *
   * For a stack of nested transactions each dirty page could be spilled only
   * once, and parent's dirty pages couldn't be spilled while child
   * transaction(s) are running. Therefore a child transaction could reach
   * \ref MDBX_TXN_FULL when parent(s) transaction has  spilled too less (and
   * child reach the limit of dirty pages), either when parent(s) has spilled
   * too more (since child can't spill already spilled pages). So there is no
   * universal golden ratio.
   *
   * Should be in the range 0..255, where zero means no explicit spilling will
   * be performed during starting nested transactions.
   * Default is 0, i.e. by default no spilling performed during starting nested
   * transactions, that correspond historically behaviour. */
  MDBX_opt_spill_parent4child_denominator,

  /** \brief Controls the in-process threshold of semi-empty pages merge.
   * \details This option controls the in-process threshold of minimum page
   * fill, as used space of percentage of a page. Neighbour pages emptier than
   * this value are candidates for merging. The threshold value is specified
   * in 1/65536 points of a whole page, which is equivalent to the 16-dot-16
   * fixed point format.
   * The specified value must be in the range from 12.5% (almost empty page)
   * to 50% (half empty page) which corresponds to the range from 8192 and
   * to 32768 in units respectively.
   * \see MDBX_opt_prefer_waf_insteadof_balance */
  MDBX_opt_merge_threshold,

  /** \brief Controls the choosing between use write-through disk writes and
   * usual ones with followed flush by the `fdatasync()` syscall.
   * \details Depending on the operating system, storage subsystem
   * characteristics and the use case, higher performance can be achieved by
   * either using write-through or a serie of usual/lazy writes followed by
   * the flush-to-disk.
   *
   * Basically for N chunks the latency/cost of write-through is:
   *  latency = N * (emit_cost + round-trip-to-storage + storage-execution);
   * And for serie of lazy writes with flush is:
   *  latency = N * (emit_cost + storage-execution) + flush_cost + round-trip-to-storage.
   *
   * So, for large N and/or noteable round-trip-to-storage the write+flush
   * approach is win. But for small N and/or near-zero NVMe-like latency
   * the write-through is better.
   *
   * To solve this issue libmdbx provide `MDBX_opt_writethrough_threshold`:
   *  - when N described above less or equal specified threshold,
   *    a write-through approach will be used;
   *  - otherwise, when N great than specified threshold,
   *    a write-and-flush approach will be used.
   *
   * \note MDBX_opt_writethrough_threshold affects only \ref MDBX_SYNC_DURABLE
   * mode and is not supported on Windows.
   * On Windows a write-through is used always but \ref MDBX_NOMETASYNC could
   * be used for switching to write-and-flush. */
  MDBX_opt_writethrough_threshold,

  /** \brief Controls prevention of page-faults of reclaimed and allocated pages
   * by clearing ones through file handle before touching. */
  MDBX_opt_prefault_write_enable,

  /** \brief Controls the in-process spending time limit of searching consecutive pages inside GC.
   * \see MDBX_opt_rp_augment_limit
   *
   * \details Sets the time limit in 1/65536 fractions of a second that can be spent during a writing transaction
   * searching for page sequences inside GC/freelist after reaching the limit set by the \ref MDBX_opt_rp_augment_limit
   * option. Time control is not performed when searching/allocating single pages and allocating pages to the needs of
   * the GC (when updating the GC during transaction commit).
   *
   * The set time limit is calculated according to the "wall clock" and is controlled within a transaction, inherited
   * for nested transactions and accumulated in a parent when ones are finished. Time control is performed only when the
   * limit set by the \ref MDBX_opt_rp_augment_limit option is reached. This allows you to flexibly control the behavior
   * using both options.
   *
   * By default, the limit is set to 0, which immediately stops the GC search when \ref MDBX_opt_rp_augment_limit is
   * reached in the internal state of the transaction and corresponds to the behavior until the `MDBX_opt_gc_time_limit`
   * option appears. On the other hand, with the minimum value (including 0) of `MDBX_opt_rp_augment_limit`, GC
   * processing will be limited mainly by a time spent. */
  MDBX_opt_gc_time_limit,

  /** \brief Controls the choice between striving for uniformity of page filling/density, either for reducing the number
   * of modified and written-to-filesystem pages.
   *
   * \details After deletion operations, pages containing less than the minimum number of keys, or those emptied before
   * \ref MDBX_opt_merge_threshold, must be merged with one of the neighboring ones. If a pages to the
   * right and left of the current one are both "dirty" (it were modified during the transaction and must be written to
   * filesystem) or both are "clean" (it were not changed in the current transaction), then the less populated page is
   * always chosen as the target for merging. When only one of the neighboring ones is "dirty" and the other is "clean",
   * then two tactics of choosing a target for merging are possible:
   *
   *  - If `MDBX_opt_prefer_waf_insteadof_balance = True`, then an already modified page will be selected, which will
   *    NOT INCREASE the number of modified pages and the amount of writing to filesystem when the current transaction
   *    is committed (aka WAF or Write Amplification Factor), but on average will INCREASE the unevenness of page
   *    filling/density. This is the default behaviour since 2026-01-04.
   *
   *  - If `MDBX_opt_prefer_waf_insteadof_balance = False`, then a less populated page will be selected, which will
   *    INCREASE the number of modified pages and the amount of writing to filesystem when the current transaction is
   *    committed, but on average it will REDUCE the unevenness of pages filling/density.
   *
   * \see MDBX_opt_merge_threshold */
  MDBX_opt_prefer_waf_insteadof_balance,

  /** \brief Specifies the maximum size of nested pages used to accommodate a small number of
   * multi-values associated with a single key.
   *
   * Using nested pages, instead of putting values on separate pages of a nested tree, allows to reduce the amount of
   * unused space and thereby increase the density of data placement.
   *
   * On the other hand, as the size of a nested pages increases, more leaf pages of a main tree are required, which also
   * increases the height of a main tree. In addition, changing data on nested pages requires additional copies, so the
   * cost may be higher in many scenarios.
   *
   * The option value is specified in units of 1/65536 of the page size: minimal 0% (0), maximal 100% (65535),
   * default is 100% (65535). */
  MDBX_opt_subpage_limit,

  /** \brief Sets the minimum amount of free space on a leaf page in the absence of which the nested pages are
   * placed in a separate tree.
   *
   * The option value is specified in units of 1/65536 of the page size: minimal 0, maximal 100% (65535),
   * default is 0. */
  MDBX_opt_subpage_room_threshold,

  /** \brief Sets the minimum amount of free space on the main page, if available, to reserve space in the subpage.
   *
   * If there is not enough free space on a leaf page, then the nested page will be the minimum size. In turn, if there
   * is no reserve in the nested page, each addition of elements to it will require the reform of a leaf page with
   * transfer of all data nodes.
   *
   * Therefore, reserving space is usually advantageous in scenarios with intensive addition of short multi-values, such
   * as indexing. But it reduces the density of data placement, respectively, it increases the volume of databases and
   * I/O operations.
   *
   * The option value is specified in units of 1/65536 of the page size: minimal 0, maximal 100% (65535),
   * default is 42% (27525). */
  MDBX_opt_subpage_reserve_prereq,

  /** \brief Sets the limit for reserving space on nested pages.
   *
   * The option value is specified in units of 1/65536 of the page size: minimal 0, maximal 100% (65535),
   * default is 4.2% (2753). */
  MDBX_opt_subpage_reserve_limit,

  /** \brief Sets the space reservation in 1/65536 of page size when splitting page along the edge.
   *
   * By default, pages are split along the edges when multiple entries are inserted strictly in ascending or descending
   * (in reverse) order, as this leads to dense page filling in the case of mass ordered inserts and, consequently, to a
   * smaller increase in the size of the database. For example, when loading data from a dump or an ordered external
   * source, the most dense filling of the pages and the minimum size of the database will be ensured.
   * However, with such dense padding, any subsequent inserts will require splitting pages immediately, which will lead
   * to a doubling of ones, an increase in database size, and a decrease in performance. In other words, initially dense
   * padding greatly slows down subsequent inserts, as it requires splitting each database page.
   *
   * This option allows you to set additional space in % of the page size, which will be reserved when splitting the
   * page, which helps to smooth out the effect described above of slowing down subsequent inserts:
   *  - with the minimum/zero value, the most densely filled pages will be formed during a mass ordered inserts;
   *  - with the maximum/32768 (means the 50% reservation), a pages will be split in the middle, not on the edge.
   *
   * Thus this option also allows to minor manage the trade-off between volume and balance of the b-tree forming while
   * inserting data.
   *
   * The option value is specified in units of 1/65536 of the page size: minimal 0, maximal 50% (32768),
   * default is 0. */
  MDBX_opt_split_reserve
} MDBX_option_t;

/** \brief Sets the value of a extra runtime options for an environment.
 * \ingroup c_settings
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 * \param [in] option  The option from \ref MDBX_option_t to set value of it.
 * \param [in] value   The value of option to be set.
 *
 * \see MDBX_option_t
 * \see mdbx_env_get_option()
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_set_option(MDBX_env *env, const MDBX_option_t option, uint64_t value);

/** \brief Gets the value of extra runtime options from an environment.
 * \ingroup c_settings
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 * \param [in] option  The option from \ref MDBX_option_t to get value of it.
 * \param [out] pvalue The address where the option's value will be stored.
 *
 * \see MDBX_option_t
 * \see mdbx_env_get_option()
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_get_option(const MDBX_env *env, const MDBX_option_t option, uint64_t *pvalue);

/** \brief Open an environment instance.
 * \ingroup c_opening
 *
 * Indifferently this function will fails or not, the \ref mdbx_env_close() must
 * be called later to discard the \ref MDBX_env handle and release associated
 * resources.
 *
 * \todo Добавить в API возможность установки обратного вызова для ревизии опций
 * работы с БД в процессе её открытия (при удержании блокировок).
 *
 * \note On Windows the \ref mdbx_env_openW() is recommended to use.
 *
 * \param [in] env       An environment handle returned
 *                       by \ref mdbx_env_create()
 *
 * \param [in] pathname  The pathname for the database or the directory in which
 *                       the database files reside. In the case of directory it
 *                       must already exist and be writable.
 *
 * \param [in] flags     Specifies options for this environment.
 *                       This parameter must be bitwise OR'ing together
 *                       any constants described above in the \ref env_flags
 *                       and \ref sync_modes sections.
 *
 * Flags set by mdbx_env_set_flags() are also used:
 *  - \ref MDBX_ENV_DEFAULTS, \ref MDBX_NOSUBDIR, \ref MDBX_RDONLY,
 *    \ref MDBX_EXCLUSIVE, \ref MDBX_WRITEMAP, \ref MDBX_NOSTICKYTHREADS,
 *    \ref MDBX_NORDAHEAD, \ref MDBX_NOMEMINIT, \ref MDBX_COALESCE,
 *    \ref MDBX_LIFORECLAIM. See \ref env_flags section.
 *
 *  - \ref MDBX_SYNC_DURABLE, \ref MDBX_NOMETASYNC, \ref MDBX_SAFE_NOSYNC,
 *    \ref MDBX_UTTERLY_NOSYNC. See \ref sync_modes section.
 *
 * \note `MDB_NOLOCK` flag don't supported by MDBX,
 *       try use \ref MDBX_EXCLUSIVE as a replacement.
 *
 * \note MDBX don't allow to mix processes with different \ref MDBX_SAFE_NOSYNC
 *       flags on the same environment.
 *       In such case \ref MDBX_INCOMPATIBLE will be returned.
 *
 * \note The explicit-I/O backend rejects \ref MDBX_WRITEMAP for writable
 *       environments with \ref MDBX_INCOMPATIBLE. Read-only opens ignore it as
 *       an irrelevant write-only flag.
 *
 * If the database is already exist and parameters specified early by
 * \ref mdbx_env_set_geometry() are incompatible (i.e. for instance, different
 * page size) then \ref mdbx_env_open() will return \ref MDBX_INCOMPATIBLE
 * error.
 *
 * \param [in] mode   The UNIX permissions to set on created files.
 *                    Zero value means to open existing, but do not create.
 *
 * \return A non-zero error value on failure and 0 on success,
 *         some possible errors are:
 * \retval MDBX_VERSION_MISMATCH The version of the MDBX library doesn't match
 *                            the version that created the database environment.
 * \retval MDBX_INVALID       The environment file headers are corrupted.
 * \retval MDBX_ENOENT        The directory specified by the path parameter
 *                            doesn't exist.
 * \retval MDBX_EACCES        The user didn't have permission to access
 *                            the environment files.
 * \retval MDBX_BUSY          The \ref MDBX_EXCLUSIVE flag was specified and the
 *                            environment is in use by another process,
 *                            or the current process tries to open environment
 *                            more than once.
 * \retval MDBX_INCOMPATIBLE  Environment is already opened by another process,
 *                            but with different set of \ref MDBX_SAFE_NOSYNC,
 *                            \ref MDBX_UTTERLY_NOSYNC flags.
 *                            Or \ref MDBX_WRITEMAP was requested for a
 *                            writable explicit-I/O environment.
 *                            Or if the database is already exist and parameters
 *                            specified early by \ref mdbx_env_set_geometry()
 *                            are incompatible (i.e. different pagesize, etc).
 *
 * \retval MDBX_WANNA_RECOVERY The \ref MDBX_RDONLY flag was specified but
 *                             read-write access is required to rollback
 *                             inconsistent state after a system crash.
 *
 * \retval MDBX_TOO_LARGE      Database is too large for this process,
 *                             i.e. 32-bit process tries to open >4Gb database.
 */
LIBMDBX_API int mdbx_env_open(MDBX_env *env, const char *pathname, MDBX_env_flags_t flags, mdbx_mode_t mode);

#if defined(_WIN32) || defined(_WIN64) || defined(DOXYGEN)
/** \copydoc mdbx_env_open()
 * \note Available only on Windows.
 * \see mdbx_env_open() */
LIBMDBX_API int mdbx_env_openW(MDBX_env *env, const wchar_t *pathname, MDBX_env_flags_t flags, mdbx_mode_t mode);
#define mdbx_env_openT(env, pathname, flags, mode) mdbx_env_openW(env, pathname, flags, mode)
#else
#define mdbx_env_openT(env, pathname, flags, mode) mdbx_env_open(env, pathname, flags, mode)
#endif /* Windows */

/** \brief Deletion modes for \ref mdbx_env_delete().
 * \ingroup c_extra
 * \see mdbx_env_delete() */
typedef enum MDBX_env_delete_mode {
  /** \brief Just delete the environment's files and directory if any.
   * \note On POSIX systems, processes already working with the database will
   * continue to work without interference until it close the environment.
   * \note On Windows, the behavior of `MDBX_ENV_JUST_DELETE` is different
   * because the system does not support deleting files that still have mapped
   * lock-file views. */
  MDBX_ENV_JUST_DELETE = 0,
  /** \brief Make sure that the environment is not being used by other
   * processes, or return an error otherwise. */
  MDBX_ENV_ENSURE_UNUSED = 1,
  /** \brief Wait until other processes closes the environment before deletion. */
  MDBX_ENV_WAIT_FOR_UNUSED = 2,
} MDBX_env_delete_mode_t;

/** \brief Delete the environment's files in a proper and multiprocess-safe way.
 * \ingroup c_extra
 *
 * \note On Windows the \ref mdbx_env_deleteW() is recommended to use.
 *
 * \param [in] pathname  The pathname for the database or the directory in which
 *                       the database files reside.
 *
 * \param [in] mode      Specifies deletion mode for the environment. This
 *                       parameter must be set to one of the constants described
 *                       above in the \ref MDBX_env_delete_mode_t section.
 *
 * \note The \ref MDBX_ENV_JUST_DELETE don't supported on Windows since system
 * unable to delete files that still have mapped lock-file views.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_RESULT_TRUE   No corresponding files or directories were found,
 *                            so no deletion was performed. */
LIBMDBX_API int mdbx_env_delete(const char *pathname, MDBX_env_delete_mode_t mode);

#if defined(_WIN32) || defined(_WIN64) || defined(DOXYGEN)
/** \copydoc mdbx_env_delete()
 * \ingroup c_extra
 * \note Available only on Windows.
 * \see mdbx_env_delete() */
LIBMDBX_API int mdbx_env_deleteW(const wchar_t *pathname, MDBX_env_delete_mode_t mode);
#define mdbx_env_deleteT(pathname, mode) mdbx_env_deleteW(pathname, mode)
#else
#define mdbx_env_deleteT(pathname, mode) mdbx_env_delete(pathname, mode)
#endif /* Windows */

/** \brief Copy an MDBX environment to the specified path, with options.
 * \ingroup c_extra
 *
 * This function may be used to make a backup of an existing environment.
 * No lockfile is created, since it gets recreated at need.
 * \note This call can trigger significant file size growth if run in
 * parallel with write transactions, because it employs a read-only
 * transaction. See long-lived transactions under \ref restrictions section.
 *
 * \note On Windows the \ref mdbx_env_copyW() is recommended to use.
 * \see mdbx_env_copy2fd()
 * \see mdbx_txn_copy2pathname()
 *
 * \param [in] env    An environment handle returned by mdbx_env_create().
 *                    It must have already been opened successfully.
 * \param [in] dest   The pathname of a file in which the copy will reside.
 *                    This file must not be already exist, but parent directory
 *                    must be writable.
 * \param [in] flags  Specifies options for this operation. This parameter
 *                    must be bitwise OR'ing together any of the constants
 *                    described here:
 *
 *  - \ref MDBX_CP_DEFAULTS
 *      Perform copy as-is without compaction, etc.
 *
 *  - \ref MDBX_CP_COMPACT
 *      Perform compaction while copying: omit free pages and sequentially
 *      renumber all pages in output. This option consumes little bit more
 *      CPU for processing, but may running quickly than the default, on
 *      account skipping free pages.
 *
 *  - \ref MDBX_CP_FORCE_DYNAMIC_SIZE
 *      Force to make resizable copy, i.e. dynamic size instead of fixed.
 *
 *  - \ref MDBX_CP_DONT_FLUSH
 *      Don't explicitly flush the written data to an output media to reduce
 *      the time of the operation and the duration of the transaction.
 *
 *  - \ref MDBX_CP_THROTTLE_MVCC
 *      Use read transaction parking during copying MVCC-snapshot
 *      to avoid stopping recycling and overflowing the database.
 *      This allows the writing transaction to oust the read
 *      transaction used to copy the database if copying takes so long
 *      that it will interfere with the recycling old MVCC snapshots
 *      and may lead to an overflow of the database.
 *      However, if the reading transaction is ousted the copy will
 *      be aborted until successful completion. Thus, this option
 *      allows copy the database without interfering with write
 *      transactions and a threat of database overflow, but at the cost
 *      that copying will be aborted to prevent such conditions.
 *      \see mdbx_txn_park()
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_copy(MDBX_env *env, const char *dest, MDBX_copy_flags_t flags);

/** \brief Copy an MDBX environment by given read transaction to the specified path, with options.
 * \ingroup c_extra
 *
 * This function may be used to make a backup of an existing environment.
 * No lockfile is created, since it gets recreated at need.
 * \note This call can trigger significant file size growth if run in
 * parallel with write transactions, because it employs a read-only
 * transaction. See long-lived transactions under \ref restrictions section.
 *
 * \note On Windows the \ref mdbx_txn_copy2pathnameW() is recommended to use.
 * \see mdbx_txn_copy2fd()
 * \see mdbx_env_copy()
 *
 * \param [in] txn    A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dest   The pathname of a file in which the copy will reside.
 *                    This file must not be already exist, but parent directory
 *                    must be writable.
 * \param [in] flags  Specifies options for this operation. This parameter
 *                    must be bitwise OR'ing together any of the constants
 *                    described here:
 *
 *  - \ref MDBX_CP_DEFAULTS
 *      Perform copy as-is without compaction, etc.
 *
 *  - \ref MDBX_CP_COMPACT
 *      Perform compaction while copying: omit free pages and sequentially
 *      renumber all pages in output. This option consumes little bit more
 *      CPU for processing, but may running quickly than the default, on
 *      account skipping free pages.
 *
 *  - \ref MDBX_CP_FORCE_DYNAMIC_SIZE
 *      Force to make resizable copy, i.e. dynamic size instead of fixed.
 *
 *  - \ref MDBX_CP_DONT_FLUSH
 *      Don't explicitly flush the written data to an output media to reduce
 *      the time of the operation and the duration of the transaction.
 *
 *  - \ref MDBX_CP_THROTTLE_MVCC
 *      Use read transaction parking during copying MVCC-snapshot
 *      to avoid stopping recycling and overflowing the database.
 *      This allows the writing transaction to oust the read
 *      transaction used to copy the database if copying takes so long
 *      that it will interfere with the recycling old MVCC snapshots
 *      and may lead to an overflow of the database.
 *      However, if the reading transaction is ousted the copy will
 *      be aborted until successful completion. Thus, this option
 *      allows copy the database without interfering with write
 *      transactions and a threat of database overflow, but at the cost
 *      that copying will be aborted to prevent such conditions.
 *      \see mdbx_txn_park()
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_txn_copy2pathname(MDBX_txn *txn, const char *dest, MDBX_copy_flags_t flags);

#if defined(_WIN32) || defined(_WIN64) || defined(DOXYGEN)
/** \copydoc mdbx_env_copy()
 * \ingroup c_extra
 * \note Available only on Windows.
 * \see mdbx_env_copy() */
LIBMDBX_API int mdbx_env_copyW(MDBX_env *env, const wchar_t *dest, MDBX_copy_flags_t flags);
#define mdbx_env_copyT(env, dest, flags) mdbx_env_copyW(env, dest, flags)

/** \copydoc mdbx_txn_copy2pathname()
 * \ingroup c_extra
 * \note Available only on Windows.
 * \see mdbx_txn_copy2pathname() */
LIBMDBX_API int mdbx_txn_copy2pathnameW(MDBX_txn *txn, const wchar_t *dest, MDBX_copy_flags_t flags);
#define mdbx_txn_copy2pathnameT(txn, dest, flags) mdbx_txn_copy2pathnameW(txn, dest, path)
#else
#define mdbx_env_copyT(env, dest, flags) mdbx_env_copy(env, dest, flags)
#define mdbx_txn_copy2pathnameT(txn, dest, flags) mdbx_txn_copy2pathname(txn, dest, path)
#endif /* Windows */

/** \brief Copy an environment to the specified file descriptor, with options.
 * \ingroup c_extra
 *
 * This function may be used to make a backup of an existing environment.
 * No lockfile is created, since it gets recreated at need.
 * \see mdbx_env_copy()
 * \see mdbx_txn_copy2fd()
 *
 * \note This call can trigger significant file size growth if run in
 *       parallel with write transactions, because it employs a read-only
 *       transaction. See long-lived transactions under \ref restrictions
 *       section.
 *
 * \note Fails if the environment has suffered a page leak and the destination
 *       file descriptor is associated with a pipe, socket, or FIFO.
 *
 * \param [in] env     An environment handle returned by mdbx_env_create().
 *                     It must have already been opened successfully.
 * \param [in] fd      The file descriptor to write the copy to. It must have
 *                     already been opened for Write access.
 * \param [in] flags   Special options for this operation. \see mdbx_env_copy()
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_copy2fd(MDBX_env *env, mdbx_filehandle_t fd, MDBX_copy_flags_t flags);

/** \brief Copy an environment by given read transaction to the specified file descriptor, with options.
 * \ingroup c_extra
 *
 * This function may be used to make a backup of an existing environment.
 * No lockfile is created, since it gets recreated at need.
 * \see mdbx_txn_copy2pathname()
 * \see mdbx_env_copy2fd()
 *
 * \note This call can trigger significant file size growth if run in
 *       parallel with write transactions, because it employs a read-only
 *       transaction. See long-lived transactions under \ref restrictions
 *       section.
 *
 * \note Fails if the environment has suffered a page leak and the destination
 *       file descriptor is associated with a pipe, socket, or FIFO.
 *
 * \param [in] txn     A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] fd      The file descriptor to write the copy to. It must have
 *                     already been opened for Write access.
 * \param [in] flags   Special options for this operation. \see mdbx_env_copy()
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_txn_copy2fd(MDBX_txn *txn, mdbx_filehandle_t fd, MDBX_copy_flags_t flags);

/** \brief Statistics for a table in the environment
 * \ingroup c_statinfo
 * \see mdbx_env_stat_ex() \see mdbx_dbi_stat() */
struct MDBX_stat {
  uint32_t ms_psize;          /**< Size of a table page. This is the same for all tables
                                 in a database. */
  uint32_t ms_depth;          /**< Depth (height) of the B-tree */
  uint64_t ms_branch_pages;   /**< Number of internal (non-leaf) pages */
  uint64_t ms_leaf_pages;     /**< Number of leaf pages */
  uint64_t ms_overflow_pages; /**< Number of large/overflow pages */
  uint64_t ms_entries;        /**< Number of data items */
  uint64_t ms_mod_txnid;      /**< Transaction ID of committed last modification */
};
#ifndef __cplusplus
/** \ingroup c_statinfo */
typedef struct MDBX_stat MDBX_stat;
#endif

/** \brief Return statistics about the MDBX environment.
 * \ingroup c_statinfo
 *
 * At least one of `env` or `txn` argument must be non-null. If txn is passed
 * non-null then stat will be filled accordingly to the given transaction.
 * Otherwise, if txn is null, then stat will be populated by a snapshot from
 * the last committed write transaction, and at next time, other information
 * can be returned.
 *
 * Legacy mdbx_env_stat() correspond to calling \ref mdbx_env_stat_ex() with the
 * null `txn` argument.
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 * \param [in] txn     A transaction handle returned by \ref mdbx_txn_begin().
 * \param [out] stat   The address of an \ref MDBX_stat structure where
 *                     the statistics will be copied.
 * \param [in] bytes   The size of \ref MDBX_stat.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_stat_ex(const MDBX_env *env, const MDBX_txn *txn, MDBX_stat *stat, size_t bytes);

/** \brief Return statistics about the MDBX environment.
 * \ingroup c_statinfo
 * \deprecated Please use mdbx_env_stat_ex() instead. */
MDBX_DEPRECATED LIBMDBX_INLINE_API(int, mdbx_env_stat, (const MDBX_env *env, MDBX_stat *stat, size_t bytes)) {
  return mdbx_env_stat_ex(env, NULL, stat, bytes);
}

/** \brief Information about the environment.
 * \ingroup c_statinfo
 * \see mdbx_env_info_ex() */
struct MDBX_envinfo {
  struct {
    uint64_t lower;   /**< Lower limit for datafile size */
    uint64_t upper;   /**< Upper limit for datafile size */
    uint64_t current; /**< Current datafile size */
    uint64_t shrink;  /**< Shrink threshold for datafile */
    uint64_t grow;    /**< Growth step for datafile */
  } mi_geo;
  uint64_t mi_mapsize;                  /**< Configured maximum database size, kept for ABI compatibility */
  uint64_t mi_dxb_fsize;                /**< Current database file size */
  uint64_t mi_dxb_fallocated;           /**< Space allocated for the database file in a filesystem */
  uint64_t mi_last_pgno;                /**< Number of the last used page */
  uint64_t mi_recent_txnid;             /**< ID of the last committed transaction */
  uint64_t mi_latter_reader_txnid;      /**< ID of the last reader transaction */
  uint64_t mi_self_latter_reader_txnid; /**< ID of the last reader transaction of this/current process */
  uint64_t mi_meta_txnid[3], mi_meta_sign[3];
  uint32_t mi_maxreaders;   /**< Total reader slots in the environment */
  uint32_t mi_numreaders;   /**< Max reader slots used in the environment */
  uint32_t mi_dxb_pagesize; /**< Database pagesize */
  uint32_t mi_sys_pagesize; /**< System pagesize */
  uint32_t mi_sys_upcblk;   /**< System "Unified Page Cache" block size */
  uint32_t mi_sys_ioblk;    /**< Filesystem I/O block size */

  /** \brief A mostly unique ID that is regenerated on each boot.

   As such it can be used to identify the local machine's current boot. MDBX
   uses such when open the database to determine whether rollback required to
   the last steady sync point or not. I.e. if current bootid is differ from the
   value within a database then the system was rebooted and all changes since
   last steady sync must be reverted for data integrity. Zeros mean that no
   relevant information is available from the system. */
  struct {
    struct {
      uint64_t x, y;
    } current, meta[3];
  } mi_bootid;

  /** Bytes not explicitly synchronized to disk */
  uint64_t mi_unsync_volume;
  /** Current auto-sync threshold, see \ref mdbx_env_set_syncbytes(). */
  uint64_t mi_autosync_threshold;
  /** Time since entering to a "dirty" out-of-sync state in units of 1/65536 of
   * second. In other words, this is the time since the last non-steady commit
   * or zero if it was steady. */
  uint32_t mi_since_sync_seconds16dot16;
  /** Current auto-sync period in 1/65536 of second,
   * see \ref mdbx_env_set_syncperiod(). */
  uint32_t mi_autosync_period_seconds16dot16;
  /** Time since the last readers check in 1/65536 of second,
   * see \ref mdbx_reader_check(). */
  uint32_t mi_since_reader_check_seconds16dot16;
  /** Current environment mode.
   * The same as \ref mdbx_env_get_flags() returns. */
  uint32_t mi_mode;

  /** Statistics of page operations.
   * \details Overall statistics of page operations of all (running, completed
   * and aborted) transactions in the current multi-process session (since the
   * first process opened the database after everyone had previously closed it).
   */
  struct {
    uint64_t newly;    /**< Quantity of a new pages added */
    uint64_t cow;      /**< Quantity of pages copied for update */
    uint64_t clone;    /**< Quantity of parent's dirty pages clones
                            for nested transactions */
    uint64_t split;    /**< Page splits */
    uint64_t merge;    /**< Page merges */
    uint64_t spill;    /**< Quantity of spilled dirty pages */
    uint64_t unspill;  /**< Quantity of unspilled/reloaded pages */
    uint64_t wops;     /**< Number of explicit write operations (not a pages)
                            to a disk */
    uint64_t prefault; /**< Number of prefault write operations (not a pages) */
    uint64_t mincore;  /**< Number of mincore() calls */
    uint64_t msync;    /**< Number of explicit msync-to-disk operations (not a pages) */
    uint64_t fsync;    /**< Number of explicit fsync-to-disk operations (not a pages) */
  } mi_pgop_stat;

  /* GUID of the database DXB file. */
  struct {
    uint64_t x, y;
  } mi_dxbid;
};
#ifndef __cplusplus
/** \ingroup c_statinfo */
typedef struct MDBX_envinfo MDBX_envinfo;
#endif

/** \brief Return information about the MDBX environment.
 * \ingroup c_statinfo
 *
 * At least one of `env` or `txn` argument must be non-null. If txn is passed
 * non-null then stat will be filled accordingly to the given transaction.
 * Otherwise, if txn is null, then stat will be populated by a snapshot from
 * the last committed write transaction, and at next time, other information
 * can be returned.
 *
 * Legacy \ref mdbx_env_info() correspond to calling \ref mdbx_env_info_ex()
 * with the null `txn` argument.
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create()
 * \param [in] txn     A transaction handle returned by \ref mdbx_txn_begin()
 * \param [out] info   The address of an \ref MDBX_envinfo structure
 *                     where the information will be provided.
 * \param [in] bytes   The actual size of \ref MDBX_envinfo,
 *                     this value is used to provide ABI compatibility.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_info_ex(const MDBX_env *env, const MDBX_txn *txn, MDBX_envinfo *info, size_t bytes);
/** \brief Return information about the MDBX environment.
 * \ingroup c_statinfo
 * \deprecated Please use mdbx_env_info_ex() instead. */
MDBX_DEPRECATED LIBMDBX_INLINE_API(int, mdbx_env_info, (const MDBX_env *env, MDBX_envinfo *info, size_t bytes)) {
  return mdbx_env_info_ex(env, NULL, info, bytes);
}

/** \brief Flush the environment data buffers to disk.
 * \ingroup c_extra
 *
 * Unless the environment was opened with no-sync flags (\ref MDBX_NOMETASYNC,
 * \ref MDBX_SAFE_NOSYNC and \ref MDBX_UTTERLY_NOSYNC), then
 * data is always written an flushed to disk when \ref mdbx_txn_commit() is
 * called. Otherwise \ref mdbx_env_sync() may be called to manually write and
 * flush unsynced data to disk.
 *
 * Besides, \ref mdbx_env_sync_ex() with argument `force=false` may be used to
 * provide polling mode for lazy/asynchronous sync in conjunction with
 * \ref mdbx_env_set_syncbytes() and/or \ref mdbx_env_set_syncperiod().
 *
 * \note This call is not valid if the environment was opened with MDBX_RDONLY.
 *
 * \param [in] env      An environment handle returned by \ref mdbx_env_create()
 * \param [in] force    If non-zero, force a flush. Otherwise, If force is
 *                      zero, then will run in polling mode,
 *                      i.e. it will check the thresholds that were
 *                      set \ref mdbx_env_set_syncbytes()
 *                      and/or \ref mdbx_env_set_syncperiod() and perform flush
 *                      if at least one of the thresholds is reached.
 *
 * \param [in] nonblock Don't wait if write transaction
 *                      is running by other thread.
 *
 * \returns A non-zero error value on failure and \ref MDBX_RESULT_TRUE or 0 on
 *     success. The \ref MDBX_RESULT_TRUE means no data pending for flush
 *     to disk, and 0 otherwise. Some possible errors are:
 *
 * \retval MDBX_EACCES   The environment is read-only.
 * \retval MDBX_BUSY     The environment is used by other thread
 *                       and `nonblock=true`.
 * \retval MDBX_EINVAL   An invalid parameter was specified.
 * \retval MDBX_EIO      An error occurred during the flushing/writing data
 *                       to a storage medium/disk. */
LIBMDBX_API int mdbx_env_sync_ex(MDBX_env *env, bool force, bool nonblock);

/** \brief The shortcut to calling \ref mdbx_env_sync_ex() with
 * the `force=true` and `nonblock=false` arguments.
 * \ingroup c_extra */
LIBMDBX_INLINE_API(int, mdbx_env_sync, (MDBX_env * env)) { return mdbx_env_sync_ex(env, true, false); }

/** \brief The shortcut to calling \ref mdbx_env_sync_ex() with
 * the `force=false` and `nonblock=true` arguments.
 * \ingroup c_extra */
LIBMDBX_INLINE_API(int, mdbx_env_sync_poll, (MDBX_env * env)) { return mdbx_env_sync_ex(env, false, true); }

/** \brief Sets threshold to force flush the data buffers to disk, even any of
 * \ref MDBX_SAFE_NOSYNC flag in the environment.
 * \ingroup c_settings
 * \see mdbx_env_get_syncbytes \see MDBX_opt_sync_bytes
 *
 * The threshold value affects all processes which operates with given
 * environment until the last process close environment or a new value will be
 * settled.
 *
 * Data is always written to disk when \ref mdbx_txn_commit() is called, but
 * the operating system may keep it buffered. MDBX always flushes the OS buffers
 * upon commit as well, unless the environment was opened with
 * \ref MDBX_SAFE_NOSYNC, \ref MDBX_UTTERLY_NOSYNC
 * or in part \ref MDBX_NOMETASYNC.
 *
 * The default is 0, than mean no any threshold checked, and no additional
 * flush will be made.
 *
 * \param [in] env         An environment handle returned by mdbx_env_create().
 * \param [in] threshold   The size in bytes of summary changes when
 *                         a synchronous flush would be made.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_INLINE_API(int, mdbx_env_set_syncbytes, (MDBX_env * env, size_t threshold)) {
  return mdbx_env_set_option(env, MDBX_opt_sync_bytes, threshold);
}

/** \brief Get threshold to force flush the data buffers to disk, even any of
 * \ref MDBX_SAFE_NOSYNC flag in the environment.
 * \ingroup c_statinfo
 * \see mdbx_env_set_syncbytes() \see MDBX_opt_sync_bytes
 *
 * \param [in] env       An environment handle returned
 *                       by \ref mdbx_env_create().
 * \param [out] threshold  Address of an size_t to store
 *                         the number of bytes of summary changes when
 *                         a synchronous flush would be made.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL   An invalid parameter was specified. */
LIBMDBX_INLINE_API(int, mdbx_env_get_syncbytes, (const MDBX_env *env, size_t *threshold)) {
  int rc = MDBX_EINVAL;
  if (threshold) {
    uint64_t proxy = 0;
    rc = mdbx_env_get_option(env, MDBX_opt_sync_bytes, &proxy);
    MDBX_INLINE_API_ASSERT(proxy <= SIZE_MAX);
    *threshold = (size_t)proxy;
  }
  return rc;
}

/** \brief Sets relative period since the last unsteady commit to force flush
 * the data buffers to disk, even of \ref MDBX_SAFE_NOSYNC flag in the
 * environment.
 * \ingroup c_settings
 * \see mdbx_env_get_syncperiod \see MDBX_opt_sync_period
 *
 * The relative period value affects all processes which operates with given
 * environment until the last process close environment or a new value will be
 * settled.
 *
 * Data is always written to disk when \ref mdbx_txn_commit() is called, but the
 * operating system may keep it buffered. MDBX always flushes the OS buffers
 * upon commit as well, unless the environment was opened with
 * \ref MDBX_SAFE_NOSYNC or in part \ref MDBX_NOMETASYNC.
 *
 * Settled period don't checked asynchronously, but only by the
 * \ref mdbx_txn_commit() and \ref mdbx_env_sync() functions. Therefore, in
 * cases where transactions are committed infrequently and/or irregularly,
 * polling by \ref mdbx_env_sync() may be a reasonable solution to timeout
 * enforcement.
 *
 * The default is 0, than mean no any timeout checked, and no additional
 * flush will be made.
 *
 * \param [in] env   An environment handle returned by \ref mdbx_env_create().
 * \param [in] seconds_16dot16  The period in 1/65536 of second when
 *                              a synchronous flush would be made since
 *                              the last unsteady commit.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_INLINE_API(int, mdbx_env_set_syncperiod, (MDBX_env * env, unsigned seconds_16dot16)) {
  return mdbx_env_set_option(env, MDBX_opt_sync_period, seconds_16dot16);
}

/** \brief Get relative period since the last unsteady commit to force flush
 * the data buffers to disk, even of \ref MDBX_SAFE_NOSYNC flag in the
 * environment.
 * \ingroup c_statinfo
 * \see mdbx_env_set_syncperiod() \see MDBX_opt_sync_period
 *
 * \param [in] env       An environment handle returned
 *                       by \ref mdbx_env_create().
 * \param [out] period_seconds_16dot16  Address of an size_t to store
 *                                      the period in 1/65536 of second when
 *                                      a synchronous flush would be made since
 *                                      the last unsteady commit.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL   An invalid parameter was specified. */
LIBMDBX_INLINE_API(int, mdbx_env_get_syncperiod, (const MDBX_env *env, unsigned *period_seconds_16dot16)) {
  int rc = MDBX_EINVAL;
  if (period_seconds_16dot16) {
    uint64_t proxy = 0;
    rc = mdbx_env_get_option(env, MDBX_opt_sync_period, &proxy);
    MDBX_INLINE_API_ASSERT(proxy <= UINT32_MAX);
    *period_seconds_16dot16 = (unsigned)proxy;
  }
  return rc;
}

/** \brief Close the environment and release associated resources.
 * \ingroup c_opening
 *
 * Only a single thread may call this function. All transactions, tables,
 * and cursors must already be closed before calling this function. Attempts
 * to use any such handles after calling this function is UB and would cause
 * a `SIGSEGV`. The environment handle will be freed and must not be used again
 * after this call.
 *
 * \param [in] env        An environment handle returned by
 *                        \ref mdbx_env_create().
 *
 * \param [in] dont_sync  A dont'sync flag, if non-zero the last checkpoint
 *                        will be kept "as is" and may be still "weak" in the
 *                        \ref MDBX_SAFE_NOSYNC or \ref MDBX_UTTERLY_NOSYNC
 *                        modes. Such "weak" checkpoint will be ignored on
 *                        opening next time, and transactions since the last
 *                        non-weak checkpoint (meta-page update) will rolledback
 *                        for consistency guarantee.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_BUSY   The write transaction is running by other thread,
 *                     in such case \ref MDBX_env instance has NOT be destroyed
 *                     not released!
 *                     \note If any OTHER error code was returned then
 *                     given MDBX_env instance has been destroyed and released.
 *
 * \retval MDBX_EBADSIGN  Environment handle already closed or not valid,
 *                        i.e. \ref mdbx_env_close() was already called for the
 *                        `env` or was not created by \ref mdbx_env_create().
 *
 * \retval MDBX_PANIC  If \ref mdbx_env_close_ex() was called in the child
 *                     process after `fork()`. In this case \ref MDBX_PANIC
 *                     is expected, i.e. \ref MDBX_env instance was freed in
 *                     proper manner.
 *
 * \retval MDBX_EIO    An error occurred during the flushing/writing data
 *                     to a storage medium/disk. */
LIBMDBX_API int mdbx_env_close_ex(MDBX_env *env, bool dont_sync);

/** \brief The shortcut to calling \ref mdbx_env_close_ex() with
 * the `dont_sync=false` argument.
 * \ingroup c_opening */
LIBMDBX_INLINE_API(int, mdbx_env_close, (MDBX_env * env)) { return mdbx_env_close_ex(env, false); }

#if defined(DOXYGEN) || !(defined(_WIN32) || defined(_WIN64))
/** \brief Restores an instance of the environment in a child process after forking the parent process using `fork()`
 *  or similar system calls.
 * \ingroup c_extra
 *
 * Without calling \ref mdbx_env_resurrect_after_fork(), it is not possible to use an open instance of the environment
 * in a child process, including all transactions running at the moment of forking.
 *
 * The actions performed by the function can be considered as reopening the database in a child process, while
 * preserving the set options and addresses of already created instances of most objects accesible via the API.
 *
 * \note This function is not available in the Windows OS family due to the lack of process forking functionality in the
 * operating system API.
 *
 * Forking does not affect the state of the MDBX environment in the parent process. All transactions that were in the
 * parent process at the moment of forking will continue to be performed without interference after forking in the
 * parent process. However, in a child process, all relevant transactions are no longer valid, and an attempt to use
 * ones will result in an error being returned or sending the `SIGSEGV` signal by the OS kernel.
 *
 * Using an instance of the environment in a child process is not possible until calling
 * \ref mdbx_env_resurrect_after_fork(), because as a result of forking, the process's PID changes, the value of
 * which is used to organize collaboration with the database, including to track processes/threads performing reading
 * transactions related to the corresponding MVCC-snapshots. All transactions active at the moment of forking cannot
 * continue in the child process, as ones do not own any locks or any MVCC-snapshot and do not keep it from being
 * recycled during garbage collection.
 *
 * The \ref mdbx_env_resurrect_after_fork() function restores the transferred instance of the environment in the child
 * process after forking, namely: updates the system identifiers used, reopens file descriptors, acquires the necessary
 * locks associated with LCK and DXB database files, restores the lock-file mapping, and reinitializes auxiliary
 * process-local state. However, transactions inherited from the parent process are not restored, and writing and
 * reading transactions are handled differently:
 *
 *  - The writing transaction, if there was one at the moment of forking, is aborted in the child process with the
 *    release of its associated resources, including all nested transactions.
 *
 *  - The reading transactions, if any in the parent process, are logically aborted in the child process, but without
 *    releasing resources. Therefore, it is necessary to provide a call to \ref mdbx_txn_abort() for each such reading
 *    transaction in the child process, or accept resource leakage until the child process is termitaned.
 *
 * The reason for not releasing the resources of reading transactions is that historically MDBX does not maintain any
 * general list of reading transaction instances, as this is not required for normal operation, but requires using of
 * atomic operations or additional synchronization objects when creating/destroying instances \ref MDBX_txn.
 *
 * Calling \ref mdbx_env_resurrect_after_fork() without forking, or not in a child process, or repeated calls
 * do not lead to any actions or changes.
 *
 * \param [in,out] env   An instance of the environment created by the \ref mdbx_env_create() function.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 *
 * \retval MDBX_BUSY      The database was opened in \ref MDBX_EXCLUSIVE mode.
 *
 * \retval MDBX_EBADSIGN  If the signature of an object instance is corrupted,
 *                        as well as if \ref mdbx_env_resurrect_after_fork() is called simultaneously
 *                        from different threads.
 *
 * \retval MDBX_PANIC     A critical error occurred when restoring an instance of the environment,
 *                        or there was already such an error before calling the function. */
LIBMDBX_API int mdbx_env_resurrect_after_fork(MDBX_env *env);
#endif /* Windows */

/** \brief Warming up options
 * \ingroup c_settings
 * \anchor warmup_flags
 * \see mdbx_env_warmup() */
typedef enum MDBX_warmup_flags {
  /** By default \ref mdbx_env_warmup() just ask OS kernel to asynchronously
   * prefetch database pages. */
  MDBX_warmup_default = 0,

  /** Peeking all pages of allocated portion of the database
   * to force ones to be loaded into memory. However, the pages are just peeks
   * sequentially, so unused pages that are in GC will be loaded in the same
   * way as those that contain payload. */
  MDBX_warmup_force = 1,

  /** Using system calls to peeks pages instead of directly accessing ones,
   * which at the cost of additional overhead avoids killing the current
   * process by OOM-killer in a lack of memory condition.
   * \note Has effect only on POSIX (non-Windows) systems with conjunction
   * to \ref MDBX_warmup_force option. */
  MDBX_warmup_oomsafe = 2,

  /** Try to lock database pages in memory by `mlock()` on POSIX-systems
   * or `VirtualLock()` on Windows. Please refer to description of these
   * functions for reasonability of such locking and the information of
   * effects, including the system as a whole.
   *
   * Such locking in memory requires that the corresponding resource limits
   * (e.g. `RLIMIT_RSS`, `RLIMIT_MEMLOCK` or process working set size)
   * and the availability of system RAM are sufficiently high.
   *
   * On successful, all currently allocated pages, both unused in GC and
   * containing payload, will be locked in memory until the environment closes,
   * or explicitly unblocked by using \ref MDBX_warmup_release, or the
   * database geometry will changed, including its auto-shrinking. */
  MDBX_warmup_lock = 4,

  /** Alters corresponding current resource limits to be enough for lock pages
   * by \ref MDBX_warmup_lock. However, this option should be used in simpler
   * applications since takes into account only current size of this environment
   * disregarding all other factors. For real-world database application you
   * will need full-fledged management of resources and their limits with
   * respective engineering. */
  MDBX_warmup_touchlimit = 8,

  /** Release the lock that was performed before by \ref MDBX_warmup_lock. */
  MDBX_warmup_release = 16,
} MDBX_warmup_flags_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_warmup_flags)

/** \brief Warms up the database by loading pages into memory, optionally lock ones.
 * \ingroup c_settings
 *
 * Depending on the specified flags, notifies OS kernel about following access,
 * force loads the database pages, including locks ones in memory or releases
 * such a lock. However, the function does not analyze the b-tree nor the GC.
 * Therefore an unused pages that are in GC handled (i.e. will be loaded) in
 * the same way as those that contain payload.
 *
 * At least one of `env` or `txn` argument must be non-null.
 *
 * \param [in] env              An environment handle returned
 *                              by \ref mdbx_env_create().
 * \param [in] txn              A transaction handle returned
 *                              by \ref mdbx_txn_begin().
 * \param [in] flags            The \ref warmup_flags, bitwise OR'ed together.
 *
 * \param [in] timeout_seconds_16dot16  Optional timeout which checking only
 *                              during explicitly peeking database pages
 *                              for loading ones if the \ref MDBX_warmup_force
 *                              option was specified.
 *
 * \returns A non-zero error value on failure and 0 on success.
 * Some possible errors are:
 *
 * \retval MDBX_ENOSYS        The system does not support requested
 * operation(s).
 *
 * \retval MDBX_RESULT_TRUE   The specified timeout is reached during load
 *                            data into memory. */
LIBMDBX_API int mdbx_env_warmup(const MDBX_env *env, const MDBX_txn *txn, MDBX_warmup_flags_t flags,
                                unsigned timeout_seconds_16dot16);

/** \brief Set environment flags.
 * \ingroup c_settings
 *
 * This may be used to set some flags in addition to those from
 * mdbx_env_open(), or to unset these flags.
 * \see mdbx_env_get_flags()
 *
 * \note In contrast to LMDB, the MDBX serialize threads via mutex while
 * changing the flags. Therefore this function will be blocked while a write
 * transaction running by other thread, or \ref MDBX_BUSY will be returned if
 * function called within a write transaction.
 *
 * \param [in] env      An environment handle returned
 *                      by \ref mdbx_env_create().
 * \param [in] flags    The \ref env_flags to change, bitwise OR'ed together.
 * \param [in] onoff    A non-zero value sets the flags, zero clears them.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL        An invalid parameter was specified.
 * \retval MDBX_EPERM         The requested flag cannot be changed for the
 *                            current environment state.
 * \retval MDBX_EACCESS       The environment is read-only.
 * \retval MDBX_INCOMPATIBLE  \ref MDBX_WRITEMAP was requested for a writable
 *                            explicit-I/O environment. */
LIBMDBX_API int mdbx_env_set_flags(MDBX_env *env, MDBX_env_flags_t flags, bool onoff);

/** \brief Get environment flags.
 * \ingroup c_statinfo
 * \see mdbx_env_set_flags()
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 * \param [out] flags  The address of an integer to store the flags.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL An invalid parameter was specified. */
LIBMDBX_API int mdbx_env_get_flags(const MDBX_env *env, unsigned *flags);

/** \brief Return the path that was used in mdbx_env_open().
 * \ingroup c_statinfo
 *
 * \note On Windows the \ref mdbx_env_get_pathW() is recommended to use.
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create()
 * \param [out] dest   Address of a string pointer to contain the path.
 *                     This is the actual string in the environment, not a
 *                     copy. It should not be altered in any way.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL  An invalid parameter was specified. */
LIBMDBX_API int mdbx_env_get_path(const MDBX_env *env, const char **dest);

#if defined(_WIN32) || defined(_WIN64) || defined(DOXYGEN)
/** \copydoc mdbx_env_get_path()
 * \ingroup c_statinfo
 * \note Available only on Windows.
 * \see mdbx_env_get_path() */
LIBMDBX_API int mdbx_env_get_pathW(const MDBX_env *env, const wchar_t **dest);
#define mdbx_env_get_pathT(env, dest) mdbx_env_get_pathW(env, dest)
#else
#define mdbx_env_get_pathT(env, dest) mdbx_env_get_path(env, dest)
#endif /* Windows */

/** \brief Return the file descriptor for the given environment.
 * \ingroup c_statinfo
 *
 * \note All MDBX file descriptors have `FD_CLOEXEC` and
 *       couldn't be used after exec() and or `fork()`.
 *
 * \param [in] env   An environment handle returned by \ref mdbx_env_create().
 * \param [out] fd   Address of a int to contain the descriptor.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL  An invalid parameter was specified. */
LIBMDBX_API int mdbx_env_get_fd(const MDBX_env *env, mdbx_filehandle_t *fd);

/** \brief Set all size-related parameters of environment, including page size
 * and the min/max size of the database file.
 * \ingroup c_settings
 *
 * In contrast to LMDB, the MDBX provide automatic size management of an
 * database according the given parameters, including shrinking and resizing
 * on the fly. From user point of view all of these just working. Nevertheless,
 * it is reasonable to know some details in order to make optimal decisions
 * when choosing parameters.
 *
 * \see mdbx_env_info_ex()
 *
 * Both \ref mdbx_env_set_geometry() and legacy \ref mdbx_env_set_mapsize() are
 * inapplicable to read-only opened environment.
 *
 * Both \ref mdbx_env_set_geometry() and legacy \ref mdbx_env_set_mapsize()
 * could be called either before or after \ref mdbx_env_open(), either within
 * the write transaction running by current thread or not:
 *
 *  - In case \ref mdbx_env_set_geometry() or legacy \ref mdbx_env_set_mapsize()
 *    was called BEFORE \ref mdbx_env_open(), i.e. for closed environment, then
 *    the specified parameters will be used for new database creation,
 *    or will be applied during opening if database exists and no other process
 *    using it.
 *
 *    If the database is already exist, opened with \ref MDBX_EXCLUSIVE or not
 *    used by any other process, and parameters specified by
 *    \ref mdbx_env_set_geometry() are incompatible (i.e. for instance,
 *    different page size) then \ref mdbx_env_open() will return
 *    \ref MDBX_INCOMPATIBLE error.
 *
 *    In another way, if database will opened read-only or will used by other
 *    process during calling \ref mdbx_env_open() that specified parameters will
 *    silently discarded (open the database with \ref MDBX_EXCLUSIVE flag
 *    to avoid this).
 *
 *  - In case \ref mdbx_env_set_geometry() or legacy \ref mdbx_env_set_mapsize()
 *    was called after \ref mdbx_env_open() WITHIN the write transaction running
 *    by current thread, then specified parameters will be applied as a part of
 *    write transaction, i.e. will not be completely visible to any others
 *    processes until the current write transaction has been committed by the
 *    current process. However, if transaction will be aborted, then the
 *    database file will be reverted to the previous size not immediately, but
 *    when a next transaction will be committed or when the database will be
 *    opened next time.
 *
 *  - In case \ref mdbx_env_set_geometry() or legacy \ref mdbx_env_set_mapsize()
 *    was called after \ref mdbx_env_open() but OUTSIDE a write transaction,
 *    then MDBX will execute internal pseudo-transaction to apply new parameters
 *    (but only if anything has been changed), and changes be visible to any
 *    others processes immediately after successful completion of function.
 *
 * Essentially a concept of "automatic size management" is simple and useful:
 *  - There are the lower and upper bounds of the database file size;
 *  - There is the growth step by which the database file will be increased,
 *    in case of lack of space;
 *  - There is the threshold for unused space, beyond which the database file
 *    will be shrunk;
 *  - The upper size is also the maximum size of the database;
 *  - MDBX will automatically manage the size of the database file according to
 *    the given parameters.
 *
 * So, there some considerations about choosing these parameters:
 *  - The lower bound allows you to prevent database shrinking below certain
 *    reasonable size to avoid unnecessary resizing costs.
 *  - The upper bound allows you to prevent database growth above certain
 *    reasonable size. Therefore this value should be chosen reasonable large,
 *    to accommodate future growth of the database, while still matching
 *    application and filesystem limits.
 *  - The growth step must be greater than zero to allow the database to grow,
 *    but also reasonable not too small, since increasing the size by little
 *    steps will result a large overhead.
 *  - The shrink threshold must be greater than zero to allow the database
 *    to shrink but also reasonable not too small (to avoid extra overhead) and
 *    not less than growth step to avoid up-and-down flouncing.
 *  - The current size (i.e. `size_now` argument) is an auxiliary parameter for
 *    simulation legacy \ref mdbx_env_set_mapsize() and as workaround Windows
 *    issues (see below).
 *
 * MDBX coordinates online resize with active readers and writers, but at a
 * cost:
 *  - Ability to resize database on the fly requires an additional lock
 *    and release `SlimReadWriteLock` during each read-only transaction.
 *  - During resize all in-process threads should be paused and then resumed.
 *  - Shrinking of database file is performed only when it used by single
 *    process, i.e. when a database closes by the last process or opened
 *    by the first.
 *  = Therefore, the size_now argument may be useful to set database size
 *    by the first process which open a database, and thus avoid expensive
 *    remapping further.
 *
 * For create a new database with particular parameters, including the page
 * size, \ref mdbx_env_set_geometry() should be called after
 * \ref mdbx_env_create() and before \ref mdbx_env_open(). Once the database is
 * created, the page size cannot be changed. If you do not specify all or some
 * of the parameters, the corresponding default values will be used. For
 * instance, the default for database size is 10485760 bytes.
 *
 * If the mapsize is increased by another process, MDBX silently and
 * transparently adopt these changes at next transaction start. However,
 * \ref mdbx_txn_begin() will return \ref MDBX_UNABLE_EXTEND_MAPSIZE if new
 * mapping size could not be applied for current process (for instance if
 * address space is busy).  Therefore, in the case of
 * \ref MDBX_UNABLE_EXTEND_MAPSIZE error you need close and reopen the
 * environment to resolve error.
 *
 * \note Actual values may be different than your have specified because of
 * rounding to specified database page size, the system page size and/or the
 * size of the system virtual memory management unit. You can get actual values
 * by \ref mdbx_env_info_ex() or see by using the tool `mdbx_chk` with the `-v`
 * option.
 *
 * Legacy \ref mdbx_env_set_mapsize() correspond to calling
 * \ref mdbx_env_set_geometry() with the arguments `size_lower`, `size_now`,
 * `size_upper` equal to the `size` and `-1` (i.e. default) for all other
 * parameters.
 *
 * \param [in] env         An environment handle returned
 *                         by \ref mdbx_env_create()
 *
 * \param [in] size_lower  The lower bound of database size in bytes.
 *                         Zero value means "minimal acceptable",
 *                         and negative means "keep current or use default".
 *
 * \param [in] size_now    The size in bytes to setup the database size for
 *                         now. Zero value means "minimal acceptable", and
 *                         negative means "keep current or use default". So,
 *                         it is recommended always pass -1 in this argument
 *                         except some special cases.
 *
 * \param [in] size_upper The upper bound of database size in bytes.
 *                        Zero value means "minimal acceptable",
 *                        and negative means "keep current or use default".
 *                        It is recommended to avoid change upper bound while
 *                        database is used by other processes or threaded
 *                        (i.e. just pass -1 in this argument except absolutely
 *                        necessary). Otherwise you must be ready for
 *                        \ref MDBX_UNABLE_EXTEND_MAPSIZE error(s), unexpected
 *                        pauses during remapping and/or system errors like
 *                        "address busy", and so on. In other words, there
 *                        is no way to handle a growth of the upper bound
 *                        robustly because there may be a lack of appropriate
 *                        system resources (which are extremely volatile in
 *                        a multi-process multi-threaded environment).
 *
 * \param [in] growth_step  The growth step in bytes, must be greater than
 *                          zero to allow the database to grow. Negative value
 *                          means "keep current or use default".
 *
 * \param [in] shrink_threshold  The shrink threshold in bytes, must be greater
 *                               than zero to allow the database to shrink and
 *                               greater than growth_step to avoid shrinking
 *                               right after grow.
 *                               Negative value means "keep current
 *                               or use default". Default is 2*growth_step.
 *
 * \param [in] pagesize          The database page size for new database
 *                               creation or -1 otherwise. Once the database
 *                               is created, the page size cannot be changed.
 *                               Must be power of 2 in the range between
 *                               \ref MDBX_MIN_PAGESIZE and
 *                               \ref MDBX_MAX_PAGESIZE. Zero value means
 *                               "minimal acceptable", and negative means
 *                               "keep current or use default".
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL    An invalid parameter was specified,
 *                        or the environment has an active write transaction.
 * \retval MDBX_EPERM     Two specific cases for Windows:
 *                        1) Shrinking was disabled before via geometry settings
 *                        and now it enabled, but there are reading threads that
 *                        don't use the additional `SRWL` (which is required to
 *                        avoid Windows issues).
 *                        2) Geometry change requires pausing in-process read
 *                        transaction(s), but no corresponding thread(s) could
 *                        be suspended since the \ref MDBX_NOSTICKYTHREADS mode
 *                        is used.
 * \retval MDBX_EACCESS   The environment opened in read-only.
 * \retval MDBX_MAP_FULL  Specified size smaller than the space already
 *                        consumed by the environment.
 * \retval MDBX_TOO_LARGE Specified size is too large, i.e. too many pages for
 *                        given size, or a 32-bit process requests too much
 *                        bytes for the 32-bit address space. */
LIBMDBX_API int mdbx_env_set_geometry(MDBX_env *env, intptr_t size_lower, intptr_t size_now, intptr_t size_upper,
                                      intptr_t growth_step, intptr_t shrink_threshold, intptr_t pagesize);

/** \deprecated Please use \ref mdbx_env_set_geometry() instead.
 * \ingroup c_settings */
MDBX_DEPRECATED LIBMDBX_INLINE_API(int, mdbx_env_set_mapsize, (MDBX_env * env, size_t size)) {
  return mdbx_env_set_geometry(env, size, size, size, -1, -1, -1);
}

/** \brief Find out whether to use readahead or not, based on the given database
 * size and the amount of available memory.
 * \ingroup c_extra
 *
 * \param [in] volume      The expected database size in bytes.
 * \param [in] redundancy  Additional reserve or overload in case of negative
 *                         value.
 *
 * \returns A \ref MDBX_RESULT_TRUE or \ref MDBX_RESULT_FALSE value,
 *          otherwise the error code.
 * \retval MDBX_RESULT_TRUE   Readahead is reasonable.
 * \retval MDBX_RESULT_FALSE  Readahead is NOT reasonable,
 *                            i.e. \ref MDBX_NORDAHEAD is useful to
 *                            open environment by \ref mdbx_env_open().
 * \retval OTHERWISE the error code. */
LIBMDBX_API int mdbx_is_readahead_reasonable(size_t volume, intptr_t redundancy);

/** \brief Returns the minimal database page size in bytes.
 * \ingroup c_statinfo */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_INLINE_API(intptr_t, mdbx_limits_pgsize_min, (void)) { return MDBX_MIN_PAGESIZE; }

/** \brief Returns the maximal database page size in bytes.
 * \ingroup c_statinfo */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_INLINE_API(intptr_t, mdbx_limits_pgsize_max, (void)) { return MDBX_MAX_PAGESIZE; }

/** \brief Returns minimal database size in bytes for given page size,
 * or -1 if pagesize is invalid.
 * \ingroup c_statinfo */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_dbsize_min(intptr_t pagesize);

/** \brief Returns maximal database size in bytes for given page size,
 * or -1 if pagesize is invalid.
 * \ingroup c_statinfo */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_dbsize_max(intptr_t pagesize);

/** \brief Returns maximal key size in bytes for given page size
 * and table flags, or -1 if pagesize is invalid.
 * \ingroup c_statinfo
 * \see db_flags */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_keysize_max(intptr_t pagesize, MDBX_db_flags_t flags);

/** \brief Returns minimal key size in bytes for given table flags.
 * \ingroup c_statinfo
 * \see db_flags */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_keysize_min(MDBX_db_flags_t flags);

/** \brief Returns maximal data size in bytes for given page size
 * and table flags, or -1 if pagesize is invalid.
 * \ingroup c_statinfo
 * \see db_flags */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_valsize_max(intptr_t pagesize, MDBX_db_flags_t flags);

/** \brief Returns minimal data size in bytes for given table flags.
 * \ingroup c_statinfo
 * \see db_flags */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_valsize_min(MDBX_db_flags_t flags);

/** \brief Returns maximal size of key-value pair to fit in a single page with
 * the given size and table flags, or -1 if pagesize is invalid.
 * \ingroup c_statinfo
 * \see db_flags */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_pairsize4page_max(intptr_t pagesize,
                                                                               MDBX_db_flags_t flags);

/** \brief Returns maximal data size in bytes to fit in a leaf-page or
 * single large/overflow-page with the given page size and table flags,
 * or -1 if pagesize is invalid.
 * \ingroup c_statinfo
 * \see db_flags */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_valsize4page_max(intptr_t pagesize, MDBX_db_flags_t flags);

/** \brief Returns maximal write transaction size (i.e. limit for summary volume
 * of dirty pages) in bytes for given page size, or -1 if pagesize is invalid.
 * \ingroup c_statinfo */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API intptr_t mdbx_limits_txnsize_max(intptr_t pagesize);

/** \brief Set the maximum number of threads/reader slots for for all processes
 * interacts with the database.
 * \ingroup c_settings
 *
 * \details This defines the number of slots in the lock table that is used to
 * track readers in the environment. The default is about 100 for 4K system
 * page size. Starting a read-only transaction normally ties a lock table slot
 * to the current thread until the environment closes or the thread exits. If
 * \ref MDBX_NOSTICKYTHREADS is in use, \ref mdbx_txn_begin() instead ties the
 * slot to the \ref MDBX_txn object until it or the \ref MDBX_env object is
 * destroyed. This function may only be called after \ref mdbx_env_create() and
 * before \ref mdbx_env_open(), and has an effect only when the database is
 * opened by the first process interacts with the database.
 * \see mdbx_env_get_maxreaders()
 *
 * \param [in] env       An environment handle returned
 *                       by \ref mdbx_env_create().
 * \param [in] readers   The maximum number of reader lock table slots.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL   An invalid parameter was specified.
 * \retval MDBX_EPERM    The environment is already open. */
LIBMDBX_INLINE_API(int, mdbx_env_set_maxreaders, (MDBX_env * env, unsigned readers)) {
  return mdbx_env_set_option(env, MDBX_opt_max_readers, readers);
}

/** \brief Get the maximum number of threads/reader slots for the environment.
 * \ingroup c_statinfo
 * \see mdbx_env_set_maxreaders()
 *
 * \param [in] env       An environment handle returned
 *                       by \ref mdbx_env_create().
 * \param [out] readers  Address of an integer to store the number of readers.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL   An invalid parameter was specified. */
LIBMDBX_INLINE_API(int, mdbx_env_get_maxreaders, (const MDBX_env *env, unsigned *readers)) {
  int rc = MDBX_EINVAL;
  if (readers) {
    uint64_t proxy = 0;
    rc = mdbx_env_get_option(env, MDBX_opt_max_readers, &proxy);
    *readers = (unsigned)proxy;
  }
  return rc;
}

/** \brief Set the maximum number of named tables for the environment.
 * \ingroup c_settings
 *
 * This function is only needed if multiple tables will be used in the
 * environment. Simpler applications that use the environment as a single
 * unnamed table can ignore this option.
 * This function may only be called after \ref mdbx_env_create() and before
 * \ref mdbx_env_open().
 *
 * Currently a moderate number of slots are cheap but a huge number gets
 * expensive: 7-120 words per transaction, and every \ref mdbx_dbi_open()
 * does a linear search of the opened slots.
 * \see mdbx_env_get_maxdbs()
 *
 * \param [in] env   An environment handle returned by \ref mdbx_env_create().
 * \param [in] dbs   The maximum number of tables.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL   An invalid parameter was specified.
 * \retval MDBX_EPERM    The environment is already open. */
LIBMDBX_INLINE_API(int, mdbx_env_set_maxdbs, (MDBX_env * env, MDBX_dbi dbs)) {
  return mdbx_env_set_option(env, MDBX_opt_max_db, dbs);
}

/** \brief Get the maximum number of named tables for the environment.
 * \ingroup c_statinfo
 * \see mdbx_env_set_maxdbs()
 *
 * \param [in] env   An environment handle returned by \ref mdbx_env_create().
 * \param [out] dbs  Address to store the maximum number of tables.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL   An invalid parameter was specified. */
LIBMDBX_INLINE_API(int, mdbx_env_get_maxdbs, (const MDBX_env *env, MDBX_dbi *dbs)) {
  int rc = MDBX_EINVAL;
  if (dbs) {
    uint64_t proxy = 0;
    rc = mdbx_env_get_option(env, MDBX_opt_max_db, &proxy);
    *dbs = (MDBX_dbi)proxy;
  }
  return rc;
}

/** \brief Returns the default size of database page for the current system.
 * \ingroup c_statinfo
 * \details Default size of database page depends on the size of the system
 * page and usually exactly match it. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API size_t mdbx_default_pagesize(void);

/** \brief Returns basic information about system RAM.
 * This function provides a portable way to get information about available RAM
 * and can be useful in that it returns the same information that libmdbx uses
 * internally to adjust various options and control readahead.
 * \ingroup c_statinfo
 *
 * \param [out] page_size     Optional address where the system page size
 *                            will be stored.
 * \param [out] total_pages   Optional address where the number of total RAM
 *                            pages will be stored.
 * \param [out] avail_pages   Optional address where the number of
 *                            available/free RAM pages will be stored.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_get_sysraminfo(intptr_t *page_size, intptr_t *total_pages, intptr_t *avail_pages);

/** \brief Returns the maximum size of keys can put.
 * \ingroup c_statinfo
 *
 * \param [in] env    An environment handle returned by \ref mdbx_env_create().
 * \param [in] flags  Table options (\ref MDBX_DUPSORT, \ref MDBX_INTEGERKEY
 *                    and so on). \see db_flags
 *
 * \returns The maximum size of a key can write,
 *          or -1 if something is wrong. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_env_get_maxkeysize_ex(const MDBX_env *env, MDBX_db_flags_t flags);

/** \brief Returns the maximum size of data we can put.
 * \ingroup c_statinfo
 *
 * \param [in] env    An environment handle returned by \ref mdbx_env_create().
 * \param [in] flags  Table options (\ref MDBX_DUPSORT, \ref MDBX_INTEGERKEY
 *                    and so on). \see db_flags
 *
 * \returns The maximum size of a data can write,
 *          or -1 if something is wrong. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_env_get_maxvalsize_ex(const MDBX_env *env, MDBX_db_flags_t flags);

/** \deprecated Please use \ref mdbx_env_get_maxkeysize_ex()
 *              and/or \ref mdbx_env_get_maxvalsize_ex()
 * \ingroup c_statinfo */
MDBX_NOTHROW_PURE_FUNCTION MDBX_DEPRECATED LIBMDBX_API int mdbx_env_get_maxkeysize(const MDBX_env *env);

/** \brief Returns maximal size of key-value pair to fit in a single page for specified table flags.
 * \ingroup c_statinfo
 *
 * \param [in] env    An environment handle returned by \ref mdbx_env_create().
 * \param [in] flags  Table options (\ref MDBX_DUPSORT, \ref MDBX_INTEGERKEY
 *                    and so on). \see db_flags
 *
 * \returns The maximum size of a data can write,
 *          or -1 if something is wrong. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_env_get_pairsize4page_max(const MDBX_env *env, MDBX_db_flags_t flags);

/** \brief Returns maximal data size in bytes to fit in a leaf-page or
 * single large/overflow-page for specified table flags.
 * \ingroup c_statinfo
 *
 * \param [in] env    An environment handle returned by \ref mdbx_env_create().
 * \param [in] flags  Table options (\ref MDBX_DUPSORT, \ref MDBX_INTEGERKEY
 *                    and so on). \see db_flags
 *
 * \returns The maximum size of a data can write,
 *          or -1 if something is wrong. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_env_get_valsize4page_max(const MDBX_env *env, MDBX_db_flags_t flags);

/** \brief Sets application information (a context pointer) associated with the environment.
 * \see mdbx_env_get_userctx()
 * \ingroup c_settings
 *
 * \param [in] env  An environment handle returned by \ref mdbx_env_create().
 * \param [in] ctx  An arbitrary pointer for whatever the application needs.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_set_userctx(MDBX_env *env, void *ctx);

/** \brief Returns an application information (a context pointer) associated with the environment.
 * \see mdbx_env_set_userctx()
 * \ingroup c_statinfo
 *
 * \param [in] env An environment handle returned by \ref mdbx_env_create()
 * \returns The pointer set by \ref mdbx_env_set_userctx()
 *          or `NULL` if something wrong. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API void *mdbx_env_get_userctx(const MDBX_env *env);

/** \brief Create a transaction with a user provided context pointer for use with the environment.
 * \ingroup c_transactions
 *
 * The transaction handle may be discarded using \ref mdbx_txn_abort()
 * or \ref mdbx_txn_commit().
 * \see mdbx_txn_begin()
 *
 * \note A transaction and its cursors must only be used by a single thread,
 * and a thread may only have a single transaction at a time unless
 * the \ref MDBX_NOSTICKYTHREADS is used.
 *
 * \note Cursors may not span transactions.
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 *
 * \param [in] parent  If this parameter is non-NULL, the new transaction will
 *                     be a nested transaction, with the transaction indicated
 *                     by parent as its parent. Transactions may be nested
 *                     to any level. A parent transaction and its cursors may
 *                     not issue any other operations than mdbx_txn_commit and
 *                     \ref mdbx_txn_abort() while it has active child
 *                     transactions.
 *
 * \param [in] flags   Special options for this transaction. This parameter
 *                     must be set to 0 or by bitwise OR'ing together one
 *                     or more of the values described here:
 *                      - \ref MDBX_TXN_RDONLY This transaction will not perform
 *                                             any write operations.
 *
 *                      - \ref MDBX_TXN_TRY    Do not block when starting
 *                                             a write transaction.
 *
 *                      - \ref MDBX_SAFE_NOSYNC, \ref MDBX_NOMETASYNC.
 *                        Do not sync data to disk corresponding
 *                        to \ref MDBX_NOMETASYNC or \ref MDBX_SAFE_NOSYNC
 *                        description. \see sync_modes
 *
 * \param [out] txn    Address where the new \ref MDBX_txn handle
 *                     will be stored.
 *
 * \param [in] context A pointer to application context to be associated with
 *                     created transaction and could be retrieved by
 *                     \ref mdbx_txn_get_userctx() until transaction finished.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_PANIC         A fatal error occurred earlier and the
 *                            environment must be shut down.
 * \retval MDBX_UNABLE_EXTEND_MAPSIZE  Another process wrote data beyond
 *                                     this MDBX_env's mapsize and this
 *                                     environment map must be resized as well.
 *                                     See \ref mdbx_env_set_mapsize().
 * \retval MDBX_READERS_FULL  A read-only transaction was requested and
 *                            the reader lock table is full.
 *                            See \ref mdbx_env_set_maxreaders().
 * \retval MDBX_ENOMEM        Out of memory.
 * \retval MDBX_BUSY          The write transaction is already started by the
 *                            current thread. */
LIBMDBX_API int mdbx_txn_begin_ex(MDBX_env *env, MDBX_txn *parent, MDBX_txn_flags_t flags, MDBX_txn **txn,
                                  void *context);

/** \brief Create a transaction for use with the environment.
 * \ingroup c_transactions
 *
 * The transaction handle may be discarded using \ref mdbx_txn_abort()
 * or \ref mdbx_txn_commit().
 * \see mdbx_txn_begin_ex()
 *
 * \note A transaction and its cursors must only be used by a single thread,
 * and a thread may only have a single transaction at a time unless
 * the \ref MDBX_NOSTICKYTHREADS is used.
 *
 * \note Cursors may not span transactions.
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 *
 * \param [in] parent  If this parameter is non-NULL, the new transaction will
 *                     be a nested transaction, with the transaction indicated
 *                     by parent as its parent. Transactions may be nested
 *                     to any level. A parent transaction and its cursors may
 *                     not issue any other operations than mdbx_txn_commit and
 *                     \ref mdbx_txn_abort() while it has active child
 *                     transactions.
 *
 * \param [in] flags   Special options for this transaction. This parameter
 *                     must be set to 0 or by bitwise OR'ing together one
 *                     or more of the values described here:
 *                      - \ref MDBX_RDONLY   This transaction will not perform
 *                                           any write operations.
 *
 *                      - \ref MDBX_TXN_TRY  Do not block when starting
 *                                           a write transaction.
 *
 *                      - \ref MDBX_SAFE_NOSYNC, \ref MDBX_NOMETASYNC.
 *                        Do not sync data to disk corresponding
 *                        to \ref MDBX_NOMETASYNC or \ref MDBX_SAFE_NOSYNC
 *                        description. \see sync_modes
 *
 * \param [out] txn    Address where the new \ref MDBX_txn handle
 *                     will be stored.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_PANIC         A fatal error occurred earlier and the
 *                            environment must be shut down.
 * \retval MDBX_UNABLE_EXTEND_MAPSIZE  Another process wrote data beyond
 *                                     this MDBX_env's mapsize and this
 *                                     environment map must be resized as well.
 *                                     See \ref mdbx_env_set_mapsize().
 * \retval MDBX_READERS_FULL  A read-only transaction was requested and
 *                            the reader lock table is full.
 *                            See \ref mdbx_env_set_maxreaders().
 * \retval MDBX_ENOMEM        Out of memory.
 * \retval MDBX_BUSY          The write transaction is already started by the
 *                            current thread. */
LIBMDBX_INLINE_API(int, mdbx_txn_begin, (MDBX_env * env, MDBX_txn *parent, MDBX_txn_flags_t flags, MDBX_txn **txn)) {
  return mdbx_txn_begin_ex(env, parent, flags, txn, NULL);
}

/** \brief Starts a read-only clone of a given transaction.
 * \ingroup c_transactions
 *
 * \note Cloning read-only transactions without \ref MDBX_NOSTICKYTHREADS mode is pointless and is not allowed.
 *
 * Cloning a read-only transaction generates a transaction reading the same MVCC snapshot of the database.
 * However, cloning a read-write transaction is also explicitly allowed and generates a read transaction that
 * reads the original MVCC-snapshot of the database without uncommitted changes. This feature is a specialized
 * tool for implementing database compactification, data transformation, replication and other specific scenarios.
 *
 * In the non- \ref MDBX_NOSTICKYTHREADS operation mode, the cloned transaction becomes binded to the current thread.
 * At the same time, it is clearly assumed that the origin transaction is linked to another thread.
 *
 * \warning It is required to ensure that the original transaction is not used, much less interrupted
 *          or restarted by another thread, until this function done.
 *
 * When cloning read-only transactions, the parking state of original is preserved and inherited by cloned transaction.
 * Cursors and other objects associated with the original transaction are not affected and are not copied into
 * the cloned transaction.
 *
 * The function provides for both the creation of a new transaction handle
 * and the reuse of a transaction previously stopped by \ref mdbx_txn_reset().
 *
 * \note Cloning of a write transactions with pending changes (aka dirtied) is prohibited to avoid confusion.
 *
 * \param [in] origin             An transaction handle returned by \ref mdbx_txn_begin_ex() or \ref mdbx_txn_begin().
 *
 * \param [in, out] in_out_clone  Address of the \ref MDBX_txn handle that wants to be reused, or where to store the
 *                                handle of the new transaction. The handle passed by the pointer must be initialized
 *                                at the input anycase. If the pointed handle at input is NULL, then a new transaction
 *                                will be created and returned there, otherwise it must be a valid handle of read-only
 *                                transaction for reuse.
 *
 * \param [in] context            A pointer to application context to be associated with started transaction and could
 *                                be retrieved by \ref mdbx_txn_get_userctx() until transaction finished.
 *                                Just use NULL if doubt.
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL        An invalid parameter was specified, i.e. the `in_out_clone` is NULL.
 * \retval MDBX_BAD_TXN       Origin transaction is already finished or never began,
 *                            or handle referenced by `in_out_clone` is invalid or not a read-only transaction.
 *                            Cloning of a write transactions with pending changes (aka dirtied) is also
 *                            prohibited to avoid confusion.
 * \retval MDBX_EBADSIGN      Origin transaction object or reference by `in_out_clone`, has invalid signature,
 *                            e.g. transaction was already terminated or memory was corrupted.
 * \retval MDBX_OUSTED        Cloned was outed immediately for the sake of recycling old MVCC snapshots.
 * \retval MDBX_MVCC_RETARDED The MVCC snapshot used by origin transaction was bygone.
 * \retval MDBX_READERS_FULL  A read-only transaction was requested and
 *                            the reader lock table is full.
 *                            See \ref mdbx_env_set_maxreaders().
 * \retval MDBX_ENOMEM        Out of memory during creating new transaction. */
LIBMDBX_API int mdbx_txn_clone(const MDBX_txn *origin, MDBX_txn **in_out_clone, void *context);

/** \brief Sets application information associated (a context pointer) with the transaction.
 * \ingroup c_transactions
 * \see mdbx_txn_get_userctx()
 *
 * \param [in] txn  An transaction handle returned by \ref mdbx_txn_begin_ex()
 *                  or \ref mdbx_txn_begin().
 * \param [in] ctx  An arbitrary pointer for whatever the application needs.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_txn_set_userctx(MDBX_txn *txn, void *ctx);

/** \brief Returns an application information (a context pointer) associated
 * with the transaction.
 * \ingroup c_transactions
 * \see mdbx_txn_set_userctx()
 *
 * \param [in] txn  An transaction handle returned by \ref mdbx_txn_begin_ex()
 *                  or \ref mdbx_txn_begin().
 * \returns The pointer which was passed via the `context` parameter
 *          of `mdbx_txn_begin_ex()` or set by \ref mdbx_txn_set_userctx(),
 *          or `NULL` if something wrong. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API void *mdbx_txn_get_userctx(const MDBX_txn *txn);

/** \brief Information about the transaction
 * \ingroup c_statinfo
 * \see mdbx_txn_info */
struct MDBX_txn_info {
  /** The ID of the transaction. For a READ-ONLY transaction, this corresponds
      to the snapshot being read. */
  uint64_t txn_id;

  /** For READ-ONLY transaction: the lag from a recent MVCC-snapshot, i.e. the
     number of committed transaction since read transaction started.
     For WRITE transaction (provided if `scan_rlt=true`): the lag of the oldest
     reader from current transaction (i.e. at least 1 if any reader running). */
  uint64_t txn_reader_lag;

  /** Used space by this transaction, i.e. corresponding to the last used
   * database page. */
  uint64_t txn_space_used;

  /** Current size of database file. */
  uint64_t txn_space_limit_soft;

  /** Upper bound for size the database file, i.e. the value `size_upper`
     argument of the appropriate call of \ref mdbx_env_set_geometry(). */
  uint64_t txn_space_limit_hard;

  /** For READ-ONLY transaction: The total size of the database pages that were
     retired by committed write transactions after the reader's MVCC-snapshot,
     i.e. the space which would be freed after the Reader releases the
     MVCC-snapshot for reuse by completion read transaction.
     For WRITE transaction: The summarized size of the database pages that were
     retired for now due Copy-On-Write during this transaction. */
  uint64_t txn_space_retired;

  /** For READ-ONLY transaction: the space available for writer(s) and that
     must be exhausted for reason to call the Handle-Slow-Readers callback for
     this read transaction.
     For WRITE transaction: the space inside transaction
     that left to `MDBX_TXN_FULL` error. */
  uint64_t txn_space_leftover;

  /** For READ-ONLY transaction (provided if `scan_rlt=true`): The space that
     actually become available for reuse when only this transaction will be
     finished.
     For WRITE transaction: The summarized size of the dirty database
     pages that generated during this transaction. */
  uint64_t txn_space_dirty;

  /** Number of page get operations within this transaction
     if corresponding statistics enabled via \ref MDBX_ENABLE_PGET_STAT build option. */
  uint64_t txn_pget;
};
#ifndef __cplusplus
/** \ingroup c_statinfo */
typedef struct MDBX_txn_info MDBX_txn_info;
#endif

/** \brief Return information about the MDBX transaction.
 * \ingroup c_statinfo
 *
 * \param [in] txn        A transaction handle returned by \ref mdbx_txn_begin()
 * \param [out] info      The address of an \ref MDBX_txn_info structure
 *                        where the information will be copied.
 * \param [in] scan_rlt   The boolean flag controls the scan of the read lock
 *                        table to provide complete information. Such scan
 *                        is relatively expensive and you can avoid it
 *                        if corresponding fields are not needed.
 *                        See description of \ref MDBX_txn_info.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_txn_info(const MDBX_txn *txn, MDBX_txn_info *info, bool scan_rlt);

/** \brief Returns the transaction's MDBX_env.
 * \ingroup c_transactions
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin() */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API MDBX_env *mdbx_txn_env(const MDBX_txn *txn);

/** \brief Return the transaction's flags.
 * \ingroup c_transactions
 *
 * This returns the flags, including internal, associated with this transaction.
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \returns A transaction flags, valid if input is an valid transaction,
 *          otherwise \ref MDBX_TXN_INVALID. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API MDBX_txn_flags_t mdbx_txn_flags(const MDBX_txn *txn);

/** \brief Return the transaction's ID.
 * \ingroup c_statinfo
 *
 * This returns the identifier associated with this transaction. For a
 * read-only transaction, this corresponds to the snapshot being read;
 * concurrent readers will frequently have the same transaction ID.
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \returns A transaction ID, valid if input is an active transaction,
 *          otherwise 0. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API uint64_t mdbx_txn_id(const MDBX_txn *txn);

/** \brief Latency of commit stages in 1/65536 of seconds units.
 * \warning This structure may be changed in future releases.
 * \ingroup c_statinfo
 * \see mdbx_txn_commit_ex() */
struct MDBX_commit_latency {
  /** \brief Duration of preparation (commit child transactions, update
   * table's records and cursors destroying). */
  uint32_t preparation;
  /** \brief Duration of GC update by wall clock. */
  uint32_t gc_wallclock;
  /** \brief Duration of internal audit if enabled. */
  uint32_t audit;
  /** \brief Duration of writing dirty/modified data pages to a filesystem,
   * i.e. the summary duration of a `write()` syscalls during commit. */
  uint32_t write;
  /** \brief Duration of syncing written data to the disk/storage, i.e.
   * the duration of a `fdatasync()` or a `msync()` syscall during commit. */
  uint32_t sync;
  /** \brief Duration of transaction ending (releasing resources). */
  uint32_t ending;
  /** \brief The total duration of a commit. */
  uint32_t whole;
  /** \brief User-mode CPU time spent on GC update. */
  uint32_t gc_cputime;

  /** \brief Information for GC profiling.
   * \note This data is shared for all processes working with a given database and stored in LCK-file.
   *
   * Statistic is accumulated when all transactions are committed,
   * but only in libmdbx builds with the \ref MDBX_ENABLE_PROFGC option enabled.
   * The collected statistics are returned to any process when using \ref mdbx_txn_commit_ex()
   * or \ref mdbx_txn_checkpoint(), and at the same time they are reset to zero when top-level
   * transactions (not nested) are committed. */
  struct {
    /** \brief The number of GC update iterations is greater than 1 if there were repeats/restarts. */
    uint32_t wloops;
    /** \brief The number of iterations of merging GC items. */
    uint32_t coalescences;
    /** \brief The number of previous reliable/stable committed points erased
     *  when working in \ref MDBX_UTTERLY_NOSYNC mode. */
    uint32_t wipes;
    /** \brief The number of forced commits to disk to avoid the database growth
     *  when working outside of \ref MDBX_UTTERLY_NOSYNC mode. */
    uint32_t flushes;
    /** \brief The number of accesses to the Handle-Slow-Readers mechanism to avoid void the database growth.
     *  \see MDBX_hsr_func */
    uint32_t kicks;

    /** \brief Number of slow/deep path GC search for the sake of placement user's data. */
    uint32_t work_counter;
    /** \brief The time "by the wall clock" spent reading and searching inside the GC for the user's data. */
    uint32_t work_rtime_monotonic;
    /** \brief The CPU time in user mode spent for preparing pages taken from the GC for user data,
     *  including paging ones from disk. */
    uint32_t work_xtime_cpu;
    /** \brief The number of search iterations inside GC when allocating pages for the sake of user's data. */
    uint32_t work_rsteps;
    /** \brief The number of requests to allocate page sequences for the sake of user's data. */
    uint32_t work_xpages;
    /** \brief The number of page faults inside the GC when allocating and preparing pages for user's data. */
    uint32_t work_majflt;

    /** \brief The GC's slow path execution count is for the purposes of maintaining and updating the GC itself. */
    uint32_t self_counter;
    /** \brief The time "by the wall clock" spent reading and searching inside the GC
     *  for the purposes of maintaining and updating the GC itself. */
    uint32_t self_rtime_monotonic;
    /** \brief The CPU time in user mode spent preparing pages taken from the GC for the purposes
     *  of maintaining and updating the GC itself, including swapping from disk. */
    uint32_t self_xtime_cpu;
    /** \brief The number of search iterations inside the GC when allocating pages for the purposes
     *  of maintaining and updating the GC itself. */
    uint32_t self_rsteps;
    /** \brief The number of page sequences allocation requests for the GC itself. */
    uint32_t self_xpages;
    /** \brief The number of page faults within the GC when allocating and preparing pages for the GC itself. */
    uint32_t self_majflt;
    /** \brief Metrics of the amount of work and cost of merging lists of pages. */
    struct {
      uint32_t time;
      uint64_t volume;
      uint32_t calls;
    } pnl_merge_work, pnl_merge_self;
    /** \brief The maximum observed difference between the latest and oldest readed MVCC-snapshots. */
    uint32_t max_reader_lag;
    /** \brief The maximum noticed number of pages withheld from reclaimed due to reading old MVCC-snapshots. */
    uint32_t max_retained_pages;
  } gc_prof;
};
#ifndef __cplusplus
/** \ingroup c_statinfo */
typedef struct MDBX_commit_latency MDBX_commit_latency;
#endif

/** \brief Commits all changes of the transaction into a database with collecting latencies information.
 * \ingroup c_transactions
 *
 * \see mdbx_txn_commit_embark_read()
 * \see mdbx_txn_checkpoint()
 * \see mdbx_txn_commit_ex()
 * \see mdbx_txn_refresh()
 * \see mdbx_txn_begin_ex()
 * \see mdbx_txn_abort()
 * \see mdbx_txn_abort_ex()
 * \see mdbx_txn_rollback()
 *
 * If the current thread is not eligible to manage the transaction then
 * the \ref MDBX_THREAD_MISMATCH error will returned. Otherwise the transaction
 * will be committed and its handle is freed. If the transaction cannot
 * be committed, it will be aborted with the corresponding error returned.
 *
 * Thus, a result other than \ref MDBX_THREAD_MISMATCH means that the
 * transaction is terminated:
 *  - Resources are released;
 *  - Transaction handle is invalid;
 *  - Cursor(s) associated with transaction must not be used, except with
 *    mdbx_cursor_renew() and \ref mdbx_cursor_close().
 *    Such cursor(s) must be closed explicitly by \ref mdbx_cursor_close()
 *    before or after transaction commit, either can be reused with
 *    \ref mdbx_cursor_renew() until it will be explicitly closed by
 *    \ref mdbx_cursor_close().
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 * \param [out] latency             An optional pointer for getting information of latencies during the commit stages.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_RESULT_TRUE      Transaction was aborted since it should
 *                               be aborted due to previous errors,
 *                               either no changes were made during the transaction,
 *                               and the build time option
 *                               \ref MDBX_NOSUCCESS_PURE_COMMIT was enabled.
 * \retval MDBX_PANIC            A fatal error occurred earlier
 *                               and the environment must be shut down.
 * \retval MDBX_BAD_TXN          Transaction is already finished or never began.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL.
 * \retval MDBX_ENOSPC           No more disk space.
 * \retval MDBX_EIO              An error occurred during the flushing/writing
 *                               data to a storage medium/disk.
 * \retval MDBX_ENOMEM           Out of memory.
 * \warning This function may be changed in future releases. */
LIBMDBX_API int mdbx_txn_commit_ex(MDBX_txn *txn, MDBX_commit_latency *latency);

/** \brief Commits all the operations of the transaction and immediately starts next without releasing any locks.
 * \ingroup c_transactions
 *
 * \details The function's actions are similar to the sequence of calls \ref mdbx_txn_commit_ex() and then
 * \ref mdbx_txn_begin(\ref MDBX_TXN_READWRITE) if the first one is successful, but without releasing the locks,
 * which ensures that there are no other changes after the current changes are committed and the transaction begins.
 *
 * \see mdbx_txn_commit_embark_read()
 * \see mdbx_txn_amend()
 * \see mdbx_txn_commit_ex()
 * \see mdbx_txn_refresh()
 * \see mdbx_txn_begin()
 * \see mdbx_txn_rollback()
 *
 * \param [in, out] txn             A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] weakening_durability Additional flags to weaken durability for committing changes.
 * \param [out] latency             An optional pointer for getting information of latencies during the commit stages.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possibilities are:
 * \retval MDBX_RESULT_TRUE      The transaction does not contain any changes to commit,
 *                               no actions have been performed.
 * \retval MDBX_PANIC            A fatal error occurred earlier and
 *                               the environment must be shut down.
 * \retval MDBX_BAD_TXN          Unexpected or wrong transaction state.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL
 *                               or weakening_durability is invalid.
 *
 * \warning This function may be changed in future releases. */
LIBMDBX_API int mdbx_txn_checkpoint(MDBX_txn *txn, MDBX_txn_flags_t weakening_durability, MDBX_commit_latency *latency);

/** \brief Commits all the operations of the transaction and immediately starts read transaction before release locks.
 * \ingroup c_transactions
 *
 * \details The function's actions are similar to the sequence of calls \ref mdbx_txn_commit_ex() and then
 * \ref mdbx_txn_begin(\ref MDBX_TXN_RDONLY) if the first one is successful, but before release the locks, which
 * ensures that there are no other changes after the current changes are committed and the read transaction begins.
 *
 * \note In future versions of libmdbx, it is planned to implement the transfer of the cursors state from a finished
 * transaction to a new one that is being launched. The relevance of such an opportunity is currently being studied,
 * please contact the developers if you need such.
 *
 * \see mdbx_txn_amend()
 * \see mdbx_txn_checkpoint()
 * \see mdbx_txn_commit_ex()
 * \see mdbx_txn_refresh()
 * \see mdbx_txn_begin()
 *
 * \param [in, out] ptxn         A pointer to the transaction handle returned by \ref mdbx_txn_begin().
 * \param [out] latency          An optional pointer for getting information of latencies during the commit stages.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possibilities are:
 * \retval MDBX_RESULT_TRUE      The transaction does not contain any changes to commit,
 *                               no actions have been performed.
 * \retval MDBX_PANIC            A fatal error occurred earlier and
 *                               the environment must be shut down.
 * \retval MDBX_BAD_TXN          Unexpected or wrong transaction state.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL.
 *
 * \warning This function may be changed in future releases. */
LIBMDBX_API int mdbx_txn_commit_embark_read(MDBX_txn **ptxn, MDBX_commit_latency *latency);

/** \brief Starts a writing transaction to amending data in the MVCC-snapshot used by the read-only transaction.
 * \ingroup c_transactions
 *
 * \details The function tries to start a writing transaction to amend the snapshot of a data associated with a given
 * reading transaction. However, such an operation is not possible if at least one writing transaction has been
 * committed after the start of the specified read-only transaction. In this case, no action is performed and
 * the \ref MDBX_RESULT_TRUE code is returned.
 *
 * If successful, the handle of the new writing transaction is returned, and the previous reading transaction is
 * finished. With this if \ref MDBX_TXN_RDONLY_PREPARE is present in the flags, the handle of the previous reading
 * transaction will be preserved for subsequent reuse via \ref mdbx_txn_renew(), otherwise it will be released and will
 * become unavailable.
 *
 * \note In future versions of libmdbx, it is planned to implement the transfer of the cursors state from a finished
 * transaction to a new one that is being launched. The relevance of such an opportunity is currently being studied,
 * please contact the developers if you need such.
 *
 * \see mdbx_txn_commit_embark_read()
 * \see mdbx_txn_checkpoint()
 * \see mdbx_txn_rollback()
 * \see mdbx_txn_commit_ex()
 * \see mdbx_txn_refresh()
 * \see mdbx_txn_begin_ex()
 *
 * \param [in] read_txn            A read-only transaction handle returned by \ref mdbx_txn_begin().
 * \param [in, out] ptr_write_txn  A pointer for returning the \ref MDBX_txn handle of newly
 *                                 started writing transaction.
 *
 * \param [in] flags               Special options for new write transaction. This parameter
 *                                 must be set to 0 or by bitwise OR'ing together one
 *                                 or more of the values described here:
 *                                  - \ref MDBX_TXN_RDONLY_PREPARE.
 *                                    Do not release the source read-only transaction,
 *                                    but preserve it handle to be reused by \ref mdbx_txn_renew().
 *
 *                                  - \ref MDBX_TXN_TRY.
 *                                    Do not block when starting a write transaction.
 *
 *                                  - \ref MDBX_SAFE_NOSYNC, \ref MDBX_NOMETASYNC.
 *                                    Do not sync data to disk corresponding
 *                                    to \ref MDBX_NOMETASYNC or \ref MDBX_SAFE_NOSYNC
 *                                    description. \see sync_modes
 *
 * \param [in] context             A pointer to application context to be associated with
 *                                 created transaction and could be retrieved by
 *                                 \ref mdbx_txn_get_userctx() until transaction finished.
 *
 * \returns A non-zero error value on failure and 0 on success, some possibilities are:
 * \retval MDBX_RESULT_TRUE      A more recent MVCC-snapshot has been committed after reading transaction
 *                               was started and data cannot be amended based on the desired data snapshot,
 *                               no actions have been performed.
 * \retval MDBX_TXN_OVERLAPPING  The current thread is already executing a write transaction.
 * \retval MDBX_PANIC            A fatal error occurred earlier and
 *                               the environment must be shut down.
 * \retval MDBX_BAD_TXN          Unexpected or wrong transaction state.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL.
 *
 * \warning This function may be changed in future releases. */
LIBMDBX_API int mdbx_txn_amend(MDBX_txn *read_txn, MDBX_txn **ptr_write_txn, MDBX_txn_flags_t flags, void *context);

/** \brief Rolls back all uncommitted changes within the write transaction and keeps it running.
 * \ingroup c_transactions
 *
 * Aborts and then restarts the transaction, rolling back all uncommitted changes.
 * If the current thread is not eligible to manage the transaction then
 * the \ref MDBX_THREAD_MISMATCH error will returned.
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \returns A non-zero error value on failure and 0 on success, some possibilities are:
 * \retval MDBX_RESULT_TRUE      A more recent MVCC-snapshot has been committed after reading transaction
 *                               was started and data cannot be amended based on the desired data snapshot,
 *                               no actions have been performed.
 * \retval MDBX_TXN_OVERLAPPING  The current thread is already executing a write transaction.
 * \retval MDBX_PANIC            A fatal error occurred earlier and
 *                               the environment must be shut down.
 * \retval MDBX_BAD_TXN          Unexpected or wrong transaction state.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL.
 *
 * \warning This function may be changed in future releases. */
LIBMDBX_API int mdbx_txn_rollback(MDBX_txn *txn);

/** \brief Commits all the operations of the transaction into the database.
 * \ingroup c_transactions
 *
 * \see mdbx_txn_commit_embark_read()
 * \see mdbx_txn_checkpoint()
 * \see mdbx_txn_rollback()
 * \see mdbx_txn_commit_ex()
 * \see mdbx_txn_refresh()
 * \see mdbx_txn_begin_ex()
 * \see mdbx_txn_abort()
 * \see mdbx_txn_abort_ex()
 *
 * If the current thread is not eligible to manage the transaction then
 * the \ref MDBX_THREAD_MISMATCH error will returned. Otherwise the transaction
 * will be committed and its handle is freed. If the transaction cannot
 * be committed, it will be aborted with the corresponding error returned.
 *
 * Thus, a result other than \ref MDBX_THREAD_MISMATCH means that the
 * transaction is terminated:
 *  - Resources are released;
 *  - Transaction handle is invalid;
 *  - Cursor(s) associated with transaction must not be used, except with
 *    mdbx_cursor_renew() and \ref mdbx_cursor_close().
 *    Such cursor(s) must be closed explicitly by \ref mdbx_cursor_close()
 *    before or after transaction commit, either can be reused with
 *    \ref mdbx_cursor_renew() until it will be explicitly closed by
 *    \ref mdbx_cursor_close().
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_RESULT_TRUE      Transaction was aborted since it should
 *                               be aborted due to previous errors,
 *                               either no changes were made during the transaction,
 *                               and the build time option
 *                               \ref MDBX_NOSUCCESS_PURE_COMMIT was enabled.
 * \retval MDBX_PANIC            A fatal error occurred earlier
 *                               and the environment must be shut down.
 * \retval MDBX_BAD_TXN          Transaction is already finished or never began.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL.
 * \retval MDBX_ENOSPC           No more disk space.
 * \retval MDBX_EIO              An error occurred during the flushing/writing
 *                               data to a storage medium/disk.
 * \retval MDBX_ENOMEM           Out of memory. */
LIBMDBX_INLINE_API(int, mdbx_txn_commit, (MDBX_txn * txn)) { return mdbx_txn_commit_ex(txn, NULL); }

/** \brief Abandons all the operations of the transaction instead of saving ones with collecting latencies information.
 * \ingroup c_transactions
 * \see mdbx_txn_abort()
 * \see mdbx_txn_refresh()
 * \see mdbx_txn_reset()
 * \see mdbx_txn_commit_ex()
 * \see mdbx_txn_checkpoint()
 * \see mdbx_txn_commit_embark_read()
 * \see mdbx_txn_rollback()
 * \see mdbx_txn_amend()
 * \warning This function may be changed in future releases. */
LIBMDBX_API int mdbx_txn_abort_ex(MDBX_txn *txn, MDBX_commit_latency *latency);

/** \brief Abandons all the operations of the transaction instead of saving ones.
 * \ingroup c_transactions
 *
 * The transaction handle is freed. It and its cursors must not be used again
 * after this call, except with \ref mdbx_cursor_renew() and
 * \ref mdbx_cursor_close().
 *
 * If the current thread is not eligible to manage the transaction then
 * the \ref MDBX_THREAD_MISMATCH error will returned. Otherwise the transaction
 * will be aborted and its handle is freed. Thus, a result other than
 * \ref MDBX_THREAD_MISMATCH means that the transaction is terminated:
 *  - Resources are released;
 *  - Transaction handle is invalid;
 *  - Cursor(s) associated with transaction must not be used, except with
 *    \ref mdbx_cursor_renew() and \ref mdbx_cursor_close().
 *    Such cursor(s) must be closed explicitly by \ref mdbx_cursor_close()
 *    before or after transaction abort, either can be reused with
 *    \ref mdbx_cursor_renew() until it will be explicitly closed by
 *    \ref mdbx_cursor_close().
 *
 * \see mdbx_txn_abort_ex()
 * \see mdbx_txn_amend()
 * \see mdbx_txn_commit_embark_read()
 * \see mdbx_txn_checkpoint()
 * \see mdbx_txn_commit()
 * \see mdbx_txn_refresh()
 * \see mdbx_txn_reset()
 * \see mdbx_txn_rollback()
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_PANIC            A fatal error occurred earlier and
 *                               the environment must be shut down.
 * \retval MDBX_BAD_TXN          Transaction is already finished or never began.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL. */
LIBMDBX_INLINE_API(int, mdbx_txn_abort, (MDBX_txn * txn)) { return mdbx_txn_abort_ex(txn, NULL); }

/** \brief Marks transaction as broken to prevent further operations.
 * \ingroup c_transactions
 *
 * Function keeps the transaction handle and corresponding locks, but makes
 * impossible to perform any operations within a broken transaction.
 * Broken transaction must then be aborted explicitly later.
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \see mdbx_txn_abort() \see mdbx_txn_reset() \see mdbx_txn_commit()
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_txn_break(MDBX_txn *txn);

/** \brief Reset a read-only transaction.
 * \ingroup c_transactions
 *
 * Abort the read-only transaction like \ref mdbx_txn_abort(), but keep the
 * transaction handle. Therefore \ref mdbx_txn_renew() may reuse the handle.
 * This saves allocation overhead if the process will start a new read-only
 * transaction soon, and also locking overhead if \ref MDBX_NOSTICKYTHREADS is
 * in use. The reader table lock is released, but the table slot stays tied to
 * its thread or \ref MDBX_txn. Use \ref mdbx_txn_abort() to discard a reset
 * handle, and to free its lock table slot if \ref MDBX_NOSTICKYTHREADS
 * is in use.
 *
 * Cursors opened within the transaction must not be used again after this
 * call, except with \ref mdbx_cursor_renew() and \ref mdbx_cursor_close().
 *
 * Reader locks generally don't interfere with writers, but they keep old
 * versions of database pages allocated. Thus they prevent the old pages from
 * being reused when writers commit new data, and so under heavy load the
 * database size may grow much more rapidly than otherwise.
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_PANIC            A fatal error occurred earlier and
 *                               the environment must be shut down.
 * \retval MDBX_BAD_TXN          Transaction is already finished or never began.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL. */
LIBMDBX_API int mdbx_txn_reset(MDBX_txn *txn);

/** \brief Puts the reading transaction in a "parked" state.
 * \ingroup c_transactions
 *
 * Running read transactions do not allow recycling old MVCC snapshots of data, starting with the oldest used/readable
 * version and all subsequent ones. A parked transaction can be ousted by a write transaction if it interferes with the
 * recycling of garbage (old MVCC snapshots of data). But  if no such ousting occurs, then restoring (restoring to a
 * working state and continuing execution) of the reading transaction will be significantly cheaper. Thus, parking
 * transactions allows you to prevent the negative consequences associated with stopping garbage recycling, while
 * keeping overhead costs at a minimum.
 *
 * To continue execution (reading and/or using data), the parked transaction must be restored using
 * \ref mdbx_txn_unpark(). For ease of use and to prevent unnecessary API calls, using the `autounpark` parameter,
 * automatic "un-parking" is provided when using a parked transaction in API functions involving data reading.
 *
 * \warning Before restoring/un-parking a transaction, regardless of the `autounpark` argument, it is forbidden to
 * dereference pointers received earlier when reading data within a parked transaction, since the MVCC-snapshot in which
 * this data is placed is not retained and can be recycled at any time.
 *
 * A parked transaction without "un-parking" can be aborted, reset, or restarted at any time by using
 * \ref mdbx_txn_abort(), \ref mdbx_txn_reset(), and \ref mdbx_txn_renew(), respectively.
 *
 * \see mdbx_txn_unpark()
 * \see mdbx_txn_flags()
 * \see mdbx_env_set_hsr()
 * \see <a href="intro.html#long-lived-read">Long-lived read transactions</a>
 *
 * \param [in] txn          A read transaction started by \ref mdbx_txn_begin().
 *
 * \param [in] autounpark   Allows you to enable automatic un-parking/restoring of a transaction
 * when calling API functions that involve reading data.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_txn_park(MDBX_txn *txn, bool autounpark);

/** \brief Unparks a previously parked reading transaction.
 * \ingroup c_transactions
 *
 * The function tries to restore a previously parked transaction. If a parked transaction has been ousted in order to
 * recycle old MVCC snapshots, then depending on the `restart_if_ousted` argument, it is restarted in the same way as
 * \ref mdbx_txn_renew(), either the transaction is reset and the error code \ref MDBX_OUSTED is returned.
 *
 * \see mdbx_txn_park()
 * \see mdbx_txn_flags()
 * \see <a href="intro.html#long-lived-read">Long-lived read transactions</a>
 *
 * \param [in] txn     A read transaction started by \ref mdbx_txn_begin() and then parked by \ref mdbx_txn_park.
 *
 * \param [in] restart_if_ousted   Allows you to immediately restart a transaction if it has been ousted.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 *
 * \retval MDBX_SUCCESS      The parked transaction was successfully restored, or it was not parked.
 *
 * \retval MDBX_OUSTED       The reading transaction was ousted by the writing transaction in order to recycle
 *                           old MVCC-snapshots, and the `restart_if_ousted` argument was set to `false`.
 *                           The transaction is reset to a similar state after calling \ref mdbx_txn_reset(),
 *                           but the instance (handle) is not released and can be reused using \ref mdbx_txn_renew(),
 *                           either released using \ref mdbx_txn_abort().
 *
 * \retval MDBX_RESULT_TRUE  The reading transaction was ousted but has now been restarted to read recent
 *                           MVCC-snapshot, since `restart_if_ousted` was set to `true`.
 *
 * \retval MDBX_BAD_TXN      The transaction has already been finished either it has not been started,
 *                           or it is not a reading transaction. */
LIBMDBX_API int mdbx_txn_unpark(MDBX_txn *txn, bool restart_if_ousted);

/** \brief Renew a read-only transaction.
 * \ingroup c_transactions
 *
 * This acquires a new reader lock for a transaction handle that had been
 * released by \ref mdbx_txn_reset(). It must be called before a reset
 * transaction may be used again.
 *
 * \see mdbx_txn_refresh()
 * \see mdbx_txn_amend()
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_PANIC            A fatal error occurred earlier and
 *                               the environment must be shut down.
 * \retval MDBX_BAD_TXN          Unexpected or wrong transaction state.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL. */
LIBMDBX_API int mdbx_txn_renew(MDBX_txn *txn);

/** \brief Refresh a read-only transaction for a recent data.
 * \ingroup c_transactions
 *
 * \see mdbx_txn_renew()
 * \see mdbx_txn_amend()
 * \see mdbx_txn_checkpoint()
 * \see mdbx_txn_commit_embark_read()
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possibilities are:
 * \retval MDBX_RESULT_TRUE      The transaction is already reading
 *                               the most recent version of the data,
 *                               no actions have been performed.
 * \retval MDBX_PANIC            A fatal error occurred earlier and
 *                               the environment must be shut down.
 * \retval MDBX_BAD_TXN          Unexpected or wrong transaction state.
 * \retval MDBX_EBADSIGN         Transaction object has invalid signature,
 *                               e.g. transaction was already terminated
 *                               or memory was corrupted.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL           Transaction handle is NULL. */
LIBMDBX_API int mdbx_txn_refresh(MDBX_txn *txn);

/** \brief The fours integers markers (aka "canary") associated with the
 * environment.
 * \ingroup c_crud
 * \see mdbx_canary_put()
 * \see mdbx_canary_get()
 *
 * The `x`, `y` and `z` values could be set by \ref mdbx_canary_put(), while the
 * `v` will be always set to the transaction number. Updated values becomes
 * visible outside the current transaction only after it was committed. Current
 * values could be retrieved by \ref mdbx_canary_get(). */
struct MDBX_canary {
  uint64_t x, y, z, v;
};
#ifndef __cplusplus
/** \ingroup c_crud */
typedef struct MDBX_canary MDBX_canary;
#endif

/** \brief Set integers markers (aka "canary") associated with the environment.
 * \ingroup c_crud
 * \see mdbx_canary_get()
 *
 * \param [in] txn     A transaction handle returned by \ref mdbx_txn_begin()
 * \param [in] canary  A optional pointer to \ref MDBX_canary structure for `x`,
 *              `y` and `z` values from.
 *            - If canary is NOT NULL then the `x`, `y` and `z` values will be
 *              updated from given canary argument, but the `v` be always set
 *              to the current transaction number if at least one `x`, `y` or
 *              `z` values have changed (i.e. if `x`, `y` and `z` have the same
 *              values as currently present then nothing will be changes or
 *              updated).
 *            - if canary is NULL then the `v` value will be explicitly update
 *              to the current transaction number without changes `x`, `y` nor
 *              `z`.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_canary_put(MDBX_txn *txn, const MDBX_canary *canary);

/** \brief Returns fours integers markers (aka "canary") associated with the
 * environment.
 * \ingroup c_crud
 * \see mdbx_canary_put()
 *
 * \param [in] txn     A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] canary  The address of an \ref MDBX_canary structure where the
 *                     information will be copied.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_canary_get(const MDBX_txn *txn, MDBX_canary *canary);

/** \brief A callback function used to compare two keys in a table
 * \ingroup c_crud
 * \see mdbx_cmp() \see mdbx_get_keycmp()
 * \see mdbx_get_datacmp \see mdbx_dcmp()
 *
 * \anchor avoid_custom_comparators
 * \deprecated It is recommend not using custom comparison functions, but
 * instead converting the keys to one of the forms that are suitable for
 * built-in comparators (for instance take look to the \ref value2key).
 * The reasons to not using custom comparators are:
 *   - The order of records could not be validated without your code.
 *     So `mdbx_chk` utility will reports "wrong order" errors
 *     and the `-i` option is required to suppress ones.
 *   - A records could not be ordered or sorted without your code.
 *     So `mdbx_load` utility should be used with `-a` option to preserve
 *     input data order.
 *   - However, the custom comparators feature will never be removed.
 *     You have been warned but still can use custom comparators knowing
 *     about the issues noted above. In this case you should ignore `deprecated`
 *     warnings or define `MDBX_DEPRECATED` macro to empty to avoid ones. */
typedef int (*MDBX_cmp_func)(const MDBX_val *a, const MDBX_val *b) MDBX_CXX17_NOEXCEPT;

/** \brief Open or Create a named table in the environment.
 * \ingroup c_dbi
 *
 * A table handle denotes the name and parameters of a table,
 * independently of whether such a table exists. The table handle may be
 * discarded by calling \ref mdbx_dbi_close(). The old table handle is
 * returned if the table was already open. The handle may only be closed
 * once.
 *
 * \note A notable difference between MDBX and LMDB is that MDBX make handles
 * opened for existing tables immediately available for other transactions,
 * regardless this transaction will be aborted or reset. The REASON for this is
 * to avoiding the requirement for multiple opening a same handles in
 * concurrent read transactions, and tracking of such open but hidden handles
 * until the completion of read transactions which opened them.
 *
 * Nevertheless, the handle for the NEWLY CREATED table will be invisible
 * for other transactions until the this write transaction is successfully
 * committed. If the write transaction is aborted the handle will be closed
 * automatically. After a successful commit the such handle will reside in the
 * shared environment, and may be used by other transactions.
 *
 * In contrast to LMDB, the MDBX allow this function to be called from multiple
 * concurrent transactions or threads in the same process.
 *
 * To use named table (with name != NULL), \ref mdbx_env_set_maxdbs()
 * must be called before opening the environment. Table names are
 * keys in the internal unnamed table, and may be read but not written.
 *
 * \param [in] txn    transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] name   The name of the table to open. If only a single
 *                    table is needed in the environment,
 *                    this value may be NULL.
 * \param [in] flags  Special options for this table. This parameter must
 *                    be bitwise OR'ing together any of the constants
 *                    described here:
 *
 *  - \ref MDBX_DB_DEFAULTS
 *      Keys are arbitrary byte strings and compared from beginning to end.
 *  - \ref MDBX_REVERSEKEY
 *      Keys are arbitrary byte strings to be compared in reverse order,
 *      from the end of the strings to the beginning.
 *  - \ref MDBX_INTEGERKEY
 *      Keys are binary integers in native byte order, either uint32_t or
 *      uint64_t, and will be sorted as such. The keys must all be of the
 *      same size and must be aligned while passing as arguments.
 *  - \ref MDBX_DUPSORT
 *      Duplicate keys may be used in the table. Or, from another point of
 *      view, keys may have multiple data items, stored in sorted order. By
 *      default keys must be unique and may have only a single data item.
 *  - \ref MDBX_DUPFIXED
 *      This flag may only be used in combination with \ref MDBX_DUPSORT. This
 *      option tells the library that the data items for this table are
 *      all the same size, which allows further optimizations in storage and
 *      retrieval. When all data items are the same size, the
 *      \ref MDBX_GET_MULTIPLE, \ref MDBX_NEXT_MULTIPLE and
 *      \ref MDBX_PREV_MULTIPLE cursor operations may be used to retrieve
 *      multiple items at once.
 *  - \ref MDBX_INTEGERDUP
 *      This option specifies that duplicate data items are binary integers,
 *      similar to \ref MDBX_INTEGERKEY keys. The data values must all be of the
 *      same size and must be aligned while passing as arguments.
 *  - \ref MDBX_REVERSEDUP
 *      This option specifies that duplicate data items should be compared as
 *      strings in reverse order (the comparison is performed in the direction
 *      from the last byte to the first).
 *  - \ref MDBX_CREATE
 *      Create the named table if it doesn't exist. This option is not
 *      allowed in a read-only transaction or a read-only environment.
 *
 * \param [out] dbi     Address where the new \ref MDBX_dbi handle
 *                      will be stored.
 *
 * The name in \ref mdbx_dbi_open() is a null terminated string. While
 * \ref mdbx_dbi_open2() supports arbitrary length keys which are not
 * truncated, for example to support a fixed width integer type.
 *
 * For \ref mdbx_dbi_open_ex() additional arguments allow you to set custom
 * comparison functions for keys and values (for multimaps).
 * \see avoid_custom_comparators
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_NOTFOUND   The specified table doesn't exist in the
 *                         environment and \ref MDBX_CREATE was not specified.
 * \retval MDBX_DBS_FULL   Too many tables have been opened.
 *                         \see mdbx_env_set_maxdbs()
 * \retval MDBX_INCOMPATIBLE  Table is incompatible with given flags,
 *                         i.e. the passed flags is different with which the
 *                         table was created, or the table was already
 *                         opened with a different comparison function(s).
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread. */
LIBMDBX_API int mdbx_dbi_open(MDBX_txn *txn, const char *name, MDBX_db_flags_t flags, MDBX_dbi *dbi);
/** \copydoc mdbx_dbi_open()
 * \ingroup c_dbi */
LIBMDBX_API int mdbx_dbi_open2(MDBX_txn *txn, const MDBX_val *name, MDBX_db_flags_t flags, MDBX_dbi *dbi);

/** \brief Open or Create a named table in the environment
 * with using custom comparison functions.
 * \ingroup c_dbi
 *
 * \deprecated Please \ref avoid_custom_comparators
 * "avoid using custom comparators" and use \ref mdbx_dbi_open() instead.
 *
 * \param [in] txn    A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] name   The name of the table to open. If only a single
 *                    table is needed in the environment,
 *                    this value may be NULL.
 *                    The name in \ref mdbx_dbi_open_ex() is null terminated,
 *                    while \ref mdbx_dbi_open_ex2() supports an arbitrary length.
 * \param [in] flags  Special options for this table.
 * \param [in] keycmp  Optional custom key comparison function for a table.
 * \param [in] datacmp Optional custom data comparison function for a table.
 * \param [out] dbi    Address where the new MDBX_dbi handle will be stored.
 * \returns A non-zero error value on failure and 0 on success. */
MDBX_DEPRECATED LIBMDBX_API int mdbx_dbi_open_ex(MDBX_txn *txn, const char *name, MDBX_db_flags_t flags, MDBX_dbi *dbi,
                                                 MDBX_cmp_func keycmp, MDBX_cmp_func datacmp);
/** \copydoc mdbx_dbi_open_ex()
 * \ingroup c_dbi */
MDBX_DEPRECATED LIBMDBX_API int mdbx_dbi_open_ex2(MDBX_txn *txn, const MDBX_val *name, MDBX_db_flags_t flags,
                                                  MDBX_dbi *dbi, MDBX_cmp_func keycmp, MDBX_cmp_func datacmp);

/** \brief Renames the table using the DBI descriptor.
 *
 * \ingroup c_dbi
 *
 * Renames the user's named table associated with the given DBI descriptor.
 *
 * \param [in,out] txn   A writing transaction started by \ref mdbx_txn_begin().
 *
 * \param [in]     dbi   The table descriptor is opened using \ref mdbx_dbi_open().
 *
 * \param [in]     name  A new name to rename.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_dbi_rename(MDBX_txn *txn, MDBX_dbi dbi, const char *name);
/** \copydoc mdbx_dbi_rename()
 * \ingroup c_dbi */
LIBMDBX_API int mdbx_dbi_rename2(MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *name);

/** \brief A callback function for listing user's named tables.
 *
 * \ingroup c_statinfo
 * \see mdbx_enumerate_tables()
 *
 * \param [in] ctx       A pointer to the context passed by a similar parameter in \ref mdbx_enumerate_tables().
 * \param [in] txn       A transaction handle.
 * \param [in] name      The name of a table.
 * \param [in] flags     The \ref MDBX_db_flags_t of a table
 * \param [in] stat      Basic statistics \ref MDBX_stat of a table.
 * \param [in] dbi       The value of the DBI descriptor other than 0, if one was opened for this table.
 *                       Either 0 if there is no such open descriptor.
 *
 * \returns Zero if an enumeration step is successful and should be continues,
 * if another value is returned, it will be immediately returned to the caller without continuing an enumeration. */
typedef int (*MDBX_table_enum_func)(void *ctx, const MDBX_txn *txn, const MDBX_val *name, MDBX_db_flags_t flags,
                                    const struct MDBX_stat *stat, MDBX_dbi dbi) MDBX_CXX17_NOEXCEPT;

/** \brief Enumerates user's named tables in a database.
 *
 * \details Enumerates user-created named tables by calling a user-specified visitor function for each named table. The
 * enumeration continues until the named tables are exhausted, or until a result other than zero is returned from a
 * user-defined callback function, which will be returned immediately as a result.
 *
 * \ingroup c_statinfo
 * \see MDBX_table_enum_func
 *
 * \param [in] txn     A transaction started by \ref mdbx_txn_begin().
 *
 * \param [in] func    A custom callback function with the signature \ref MDBX_table_enum_func,
 *                     which will be called for each table.
 *
 * \param [in] ctx     A pointer to some context that will be passed to the `func()` function as it is.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_enumerate_tables(const MDBX_txn *txn, MDBX_table_enum_func func, void *ctx);

/** \defgroup value2key Value-to-Key functions
 * \brief Value-to-Key functions to
 * \ref avoid_custom_comparators "avoid using custom comparators"
 * \see key2value
 * @{
 *
 * The \ref mdbx_key_from_jsonInteger() build a keys which are comparable with
 * keys created by \ref mdbx_key_from_double(). So this allows mixing `int64_t`
 * and IEEE754 double values in one index for JSON-numbers with restriction for
 * integer numbers range corresponding to RFC-7159, i.e. \f$[-2^{53}+1,
 * 2^{53}-1]\f$. See bottom of page 6 at https://tools.ietf.org/html/rfc7159 */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API uint64_t mdbx_key_from_jsonInteger(const int64_t json_integer);

MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API uint64_t mdbx_key_from_double(const double ieee754_64bit);

MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API uint64_t mdbx_key_from_ptrdouble(const double *const ieee754_64bit);

MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API uint32_t mdbx_key_from_float(const float ieee754_32bit);

MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API uint32_t mdbx_key_from_ptrfloat(const float *const ieee754_32bit);

MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_INLINE_API(uint64_t, mdbx_key_from_int64, (const int64_t i64)) {
  return UINT64_C(0x8000000000000000) + i64;
}

MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_INLINE_API(uint32_t, mdbx_key_from_int32, (const int32_t i32)) {
  return UINT32_C(0x80000000) + i32;
}
/** end of value2key @} */

/** \defgroup key2value Key-to-Value functions
 * \brief Key-to-Value functions to
 * \ref avoid_custom_comparators "avoid using custom comparators"
 * \see value2key
 * @{ */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int64_t mdbx_jsonInteger_from_key(const MDBX_val);

MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API double mdbx_double_from_key(const MDBX_val);

MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API float mdbx_float_from_key(const MDBX_val);

MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int32_t mdbx_int32_from_key(const MDBX_val);

MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int64_t mdbx_int64_from_key(const MDBX_val);
/** end of value2key @} */

/** \brief Retrieve statistics for a table.
 * \ingroup c_statinfo
 *
 * \param [in] txn     A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi     A table handle returned by \ref mdbx_dbi_open().
 * \param [out] stat   The address of an \ref MDBX_stat structure where
 *                     the statistics will be copied.
 * \param [in] bytes   The size of \ref MDBX_stat.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL   An invalid parameter was specified. */
LIBMDBX_API int mdbx_dbi_stat(const MDBX_txn *txn, MDBX_dbi dbi, MDBX_stat *stat, size_t bytes);

/** \brief Retrieve depth (bitmask) information of nested dupsort (multi-value)
 * B+trees for given table.
 * \ingroup c_statinfo
 *
 * \param [in] txn     A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi     A table handle returned by \ref mdbx_dbi_open().
 * \param [out] mask   The address of an uint32_t value where the bitmask
 *                     will be stored.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL       An invalid parameter was specified.
 * \retval MDBX_RESULT_TRUE  The dbi isn't a dupsort (multi-value) table. */
LIBMDBX_API int mdbx_dbi_dupsort_depthmask(const MDBX_txn *txn, MDBX_dbi dbi, uint32_t *mask);

/** \brief DBI state bits returted by \ref mdbx_dbi_flags_ex()
 * \ingroup c_statinfo
 * \see mdbx_dbi_flags_ex() */
typedef enum MDBX_dbi_state {
  /** DB was written in this txn */
  MDBX_DBI_DIRTY = 0x01,
  /** Cached Named-DB record is older than txnID */
  MDBX_DBI_STALE = 0x02,
  /** Named-DB handle opened in this txn */
  MDBX_DBI_FRESH = 0x04,
  /** Named-DB handle created in this txn */
  MDBX_DBI_CREAT = 0x08,
} MDBX_dbi_state_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_dbi_state)

/** \brief Retrieve the DB flags and status for a table handle.
 * \ingroup c_statinfo
 * \see MDBX_db_flags_t
 * \see MDBX_dbi_state_t
 *
 * \param [in] txn     A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi     A table handle returned by \ref mdbx_dbi_open().
 * \param [out] flags  Address where the flags will be returned.
 * \param [out] state  Address where the state will be returned.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_dbi_flags_ex(const MDBX_txn *txn, MDBX_dbi dbi, unsigned *flags, unsigned *state);
/** \brief The shortcut to calling \ref mdbx_dbi_flags_ex() with `state=NULL`
 * for discarding it result.
 * \ingroup c_statinfo
 * \see MDBX_db_flags_t */
LIBMDBX_INLINE_API(int, mdbx_dbi_flags, (const MDBX_txn *txn, MDBX_dbi dbi, unsigned *flags)) {
  unsigned state;
  return mdbx_dbi_flags_ex(txn, dbi, flags, &state);
}

/** \brief Close a table handle. Normally unnecessary.
 * \ingroup c_dbi
 *
 * Closing a table handle is not necessary, but lets \ref mdbx_dbi_open()
 * reuse the handle value. Usually it's better to set a bigger
 * \ref mdbx_env_set_maxdbs(), unless that value would be large.
 *
 * \note Use with care.
 * This call is synchronized via mutex with \ref mdbx_dbi_open(), but NOT with
 * any transaction(s) running by other thread(s).
 * So the `mdbx_dbi_close()` MUST NOT be called in-parallel/concurrently
 * with any transactions using the closing dbi-handle, nor during other thread
 * commit/abort a write transacton(s). The "next" version of libmdbx (\ref
 * MithrilDB) will solve this issue.
 *
 * Handles should only be closed if no other threads are going to reference
 * the table handle or one of its cursors any further. Do not close a handle
 * if an existing transaction has modified its table. Doing so can cause
 * misbehavior from table corruption to errors like \ref MDBX_BAD_DBI
 * (since the DB name is gone).
 *
 * \param [in] env  An environment handle returned by \ref mdbx_env_create().
 * \param [in] dbi  A table handle returned by \ref mdbx_dbi_open().
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_dbi_close(MDBX_env *env, MDBX_dbi dbi);

/** \brief Empty or delete and close a table.
 * \ingroup c_crud
 *
 * \see mdbx_dbi_close() \see mdbx_dbi_open()
 *
 * \param [in] txn  A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi  A table handle returned by \ref mdbx_dbi_open().
 * \param [in] del  `false` to empty the DB, `true` to delete it
 *                  from the environment and close the DB handle.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_drop(MDBX_txn *txn, MDBX_dbi dbi, bool del);

/** \brief Get items from a table.
 * \ingroup c_crud
 *
 * This function retrieves key/data pairs from the table. The address
 * and length of the data associated with the specified key are returned
 * in the structure to which data refers.
 * If the table supports duplicate keys (\ref MDBX_DUPSORT) then the
 * first data item for the key will be returned. Retrieval of other
 * items requires the use of \ref mdbx_cursor_get().
 *
 * \note The memory pointed to by the returned values is owned by the
 * table. The caller MUST not dispose of the memory, and MUST not modify it
 * in any way regardless in a read-only nor read-write transactions!
 * Modification attempts have undefined behavior. Values may be backed by
 * explicit read-cache pages or by dirty pages in a read-write transaction; in
 * the latter case modifying returned memory can corrupt data committed by the
 * transaction.
 *
 * \note Values returned from the table are valid only until a
 * subsequent update operation, or the end of the transaction.
 *
 * \see mdbx_cache_get()
 *
 * \param [in] txn       A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi       A table handle returned by \ref mdbx_dbi_open().
 * \param [in] key       The key to search for in the table.
 * \param [in,out] data  The data corresponding to the key.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_NOTFOUND  The key was not in the table.
 * \retval MDBX_EINVAL    An invalid parameter was specified. */
LIBMDBX_API int mdbx_get(const MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *key, MDBX_val *data);

/** \brief Get items from a table  and optionally number of data items for a given key.
 *
 * \ingroup c_crud
 *
 * Briefly this function does the same as \ref mdbx_get() with a few
 * differences:
 *  1. If values_count is NOT NULL, then returns the count
 *     of multi-values/duplicates for a given key.
 *  2. Updates BOTH the key and the data for pointing to the actual key-value
 *     pair inside the table.
 *
 * \param [in] txn           A transaction handle returned
 *                           by \ref mdbx_txn_begin().
 * \param [in] dbi           A table handle returned by \ref mdbx_dbi_open().
 * \param [in,out] key       The key to search for in the table.
 * \param [in,out] data      The data corresponding to the key.
 * \param [out] values_count The optional address to return number of values
 *                           associated with given key:
 *                            = 0 - in case \ref MDBX_NOTFOUND error;
 *                            = 1 - exactly for tables
 *                                  WITHOUT \ref MDBX_DUPSORT;
 *                            >= 1 for tables WITH \ref MDBX_DUPSORT.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_NOTFOUND  The key was not in the table.
 * \retval MDBX_EINVAL    An invalid parameter was specified. */
LIBMDBX_API int mdbx_get_ex(const MDBX_txn *txn, MDBX_dbi dbi, MDBX_val *key, MDBX_val *data, size_t *values_count);

/** \brief Get equal or great item from a table.
 * \ingroup c_crud
 *
 * Briefly this function does the same as \ref mdbx_get() with a few
 * differences:
 * 1. Return equal or great (due comparison function) key-value
 *    pair, but not only exactly matching with the key.
 * 2. On success return \ref MDBX_SUCCESS if key found exactly,
 *    and \ref MDBX_RESULT_TRUE otherwise. Moreover, for tables with
 *    \ref MDBX_DUPSORT flag the data argument also will be used to match over
 *    multi-value/duplicates, and \ref MDBX_SUCCESS will be returned only when
 *    BOTH the key and the data match exactly.
 * 3. Updates BOTH the key and the data for pointing to the actual key-value
 *    pair inside the table.
 *
 * \param [in] txn           A transaction handle returned
 *                           by \ref mdbx_txn_begin().
 * \param [in] dbi           A table handle returned by \ref mdbx_dbi_open().
 * \param [in,out] key       The key to search for in the table.
 * \param [in,out] data      The data corresponding to the key.
 *
 * \returns A non-zero error value on failure and \ref MDBX_RESULT_FALSE
 *          or \ref MDBX_RESULT_TRUE on success (as described above).
 *          Some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_NOTFOUND      The key was not in the table.
 * \retval MDBX_EINVAL        An invalid parameter was specified. */
LIBMDBX_API int mdbx_get_equal_or_great(const MDBX_txn *txn, MDBX_dbi dbi, MDBX_val *key, MDBX_val *data);

/** \brief Lightweight transparent cache entry structure used by \ref mdbx_cache_get().
 * \ingroup c_crud
 *
 * The approach of these caching is to preserve address of a value retrieved from the database with an extremely fast
 * check of relevance it based on a transaction ID within an internal b-tree structures. Event a b-tree was modified
 * then the search for the corresponding key from the root of the b-tree to leaf pages stops as soon as reaches a page
 * that has not been modified after the last check of given cache entry. This way, the minimum actions is performed,
 * which is no slower than a usual key search in the worst case, and at best, only a few lightweight checks will be do.
 *
 * \note The cache structure allows it to be placed in shared memory and used by multiple processes.
 * However, such interaction and management are not provided by libmdbx in any way yet now.
 *
 * \note An each cache entry must be initialized by \ref mdbx_cache_init() before first use. */
typedef struct MDBX_cache_entry {
  uint64_t trunk_txnid;          /**< The transaction/MVCC-snapshot ID of a page or other internal DB structure
                                  *   that hold the cached data or reflect it state. */
  uint64_t last_confirmed_txnid; /**< The recent transaction/MVCC-snapshot ID wherein the cache entry
                                  *   was checked and confirmed. */
  size_t offset;                 /**< The data-file offset of cached data value for a corresponding key.
                                  *   The zero value means \ref MDBX_NOTFOUND. */
  uint32_t length;               /**< The length of cached data value for a corresponding key. */
} MDBX_cache_entry_t;

/** \brief Initializes the cache entry before the first use.
 * \ingroup c_crud
 * \see MDBX_cache_entry
 * \see mdbx_cache_get() */
LIBMDBX_INLINE_API(void, mdbx_cache_init, (MDBX_cache_entry_t * entry)) {
  entry->offset = 0;
  entry->length = 0;
  entry->trunk_txnid = 0;
  entry->last_confirmed_txnid = 0;
}

/** \brief Cache entry status returned by \ref mdbx_cache_get().
 * \ingroup c_crud
 * \see MDBX_cache_entry
 * \see mdbx_cache_init() */
typedef enum MDBX_cache_status {
  /** \brief The error other than \ref MDBX_NOTFOUND has occurred.
   *  \details There is no correct result since an error has occurred that is not related
   *  to the absence of the desired key-value pair.
   *  The given cache entry has not been changed. */
  MDBX_CACHE_ERROR = -3,

  /** \brief The result was obtained by bypassing the cache, because
   *  the transaction is too old to using the cache entry.
   *  \details The cache entry reflects a newer version of the data that is unavailable within
   *  an MVCC-snapshot used by current transaction.
   *  The given cache entry has not been changed.
   *  The result of getting a value is correct until the transaction end. */
  MDBX_CACHE_BEHIND = -2,

  /** \brief The result of getting a value is correct, but it cannot be cached since there
   *  the ABA-like issue is in the data history, either other similar reason.
   *  \details When a cache entry is used by different threads reading different MVCC snapshots,
   *  there may be a situation in which the key and associated value are missing from the old
   *  and new MVCC snapshots, but are present in one of the MVCC snapshots between ones.
   *  In such circumstances the result from the cache may be false negative, therefore,
   *  in order to avoid an incorrect result, a search is performed bypassing cache.
   *  The given cache entry has not been changed.
   *  The result of getting a value is correct until the transaction end. */
  MDBX_CACHE_UNABLE = -1,

  /** \brief The result was obtained by bypassing the cache, because
   *  the given cache entry being updated by another thread.
   *  \details When accessing the cache entry, a race condition was detected with its update by another thread.
   *  Therefore, the result was obtained without using the cache entry and without affecting an operation of other
   *  threads using it, including the ones performing an update. For a read transaction, the result is correct until
   *  the transaction end. For a write transactions, the result is correct until the value is explicitly changed or
   *  the transaction is completed. */
  MDBX_CACHE_RACE = 0,

  /** \brief The result of getting a value is correct, but it cannot be cached since
   *  the changes have not been committed.
   *  \details The requested value of a pair is in a dirty state itself or on a dirty page with other updated items.
   *  This cache entry has not been changed because the corresponding data changes have not yet been committed
   *  and could be aborted.
   *  The result of the get operation and data value are valid within the current write transaction
   *  until any next modification. */
  MDBX_CACHE_DIRTY = 1,

  /** \brief The result of getting a value is correct and was retrieved from the cache entry which is untouched.
   *  \details There were no changes in the cached data after the last check.
   *  The given cache entry was not altered as it is complete up-to-date.
   *  For a read transaction, the result is correct until the transaction end.
   *  For a write transactions, the result is correct until the value is explicitly changed
   *  or the transaction is completed. */
  MDBX_CACHE_HIT = 2,

  /** \brief The result of getting a value is correct and has been retrieved from the cache, which has been
   *  altered to reflect recently committed transactions.
   *  \details There were no changes in the cached data after the last check.
   *  The given cache entry has been slightly updated to reflect the relevance of the data for recent committed
   * transaction(s). For a read transaction, the result is correct until the transaction end. For a write transactions,
   * the result is correct until the value is explicitly changed or the transaction is completed. */
  MDBX_CACHE_CONFIRMED = 3,

  /** \brief The result of getting a value is correct and corresponds to the fresh data readed from the database,
   *  which also putted into the cache entry.
   *  \details After the last check, either the value of the requested pair itself changed,
   *  or it was moved to a new page due to the updating of neighboring items.
   *  The given cache entry has been completely updated to reflect the actual data.
   *  For a read transaction, the result is correct until the transaction end.
   *  For a write transactions, the result is correct until the value is explicitly changed
   *  or the transaction is completed. */
  MDBX_CACHE_REFRESHED = 4
} MDBX_cache_status_t;

/** \brief Pair of error code and cache status as a result of \ref mdbx_cache_get().
 * \ingroup c_crud
 * \see mdbx_cache_get()
 * \see mdbx_cache_get_SingleThreaded() */
typedef struct MDBX_cache_result {
  /** The error code of getting data same as from \ref mdbx_get(). */
  MDBX_error_t errcode;
  /** The result of cache operation as the value of \ref MDBX_cache_status_t. */
  MDBX_cache_status_t status;
} MDBX_cache_result_t;

/** \brief Gets items from a table using cache including multithreaded cases.
 * \ingroup c_crud
 * \details The essence of this "caching" is using a cached information to check as quickly as possible whether the data
 * has changed or not, with early exit when searching though a DB. For this a petty version information is stored in
 * a \ref MDBX_cache_entry_t structure, along with the data-file offset to the "cached" data. Instead of a full B-tree
 * search it stops when reaches a DB page that has not been modified after the last check. Thus a minimum number of
 * steps are performed which provides dramatic acceleration in many cases.
 *
 * \note This function is supports multi-threaded cases and automatically resolves collisions using lockfree approach,
 * nonetheless \ref MDBX_NOSTICKYTHREADS mode is required to use it within a different threads.
 *
 * \see mdbx_cache_get_SingleThreaded()
 * \see MDBX_cache_entry_t
 * \see mdbx_cache_init()
 * \see mdbx_get()
 *
 * \param [in] txn        A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi        A table handle returned by \ref mdbx_dbi_open().
 * \param [in] key        The key to search for in the table.
 * \param [in,out] data   The data corresponding to the key.
 * \param [in,out] entry  The cache entry corresponding to the key.
 *
 * \returns The \ref MDBX_cache_result_t with a pair of the error codes for getting a data
 * and the cache entry processing both. */
LIBMDBX_API MDBX_cache_result_t mdbx_cache_get(const MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *key, MDBX_val *data,
                                               volatile MDBX_cache_entry_t *entry);

/** \brief Gets items from a table using cache within single-thread cases only.
 * \ingroup c_crud
 * \details The essence of this "caching" is using a cached information to check as quickly as possible whether the data
 * has changed or not, with early exit when searching though a DB. For this a petty version information is stored in
 * a \ref MDBX_cache_entry_t structure, along with the data-file offset to the "cached" data. Instead of a full B-tree
 * search it stops when reaches a DB page that has not been modified after the last check. Thus a minimum number of
 * steps are performed which provides dramatic acceleration in many cases.
 *
 * \note This function is intended to be used with a given cache entry only in single-threaded cases, otherwise
 * behaviour is undefined.
 *
 * \see mdbx_cache_get()
 * \see MDBX_cache_entry_t
 * \see mdbx_cache_init()
 * \see mdbx_get()
 *
 * \param [in] txn        A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi        A table handle returned by \ref mdbx_dbi_open().
 * \param [in] key        The key to search for in the table.
 * \param [in,out] data   The data corresponding to the key.
 * \param [in,out] entry  The cache entry corresponding to the key.
 *
 * \returns The \ref MDBX_cache_result_t with a pair of the error codes for getting a data
 * and the cache entry processing both. */
LIBMDBX_API MDBX_cache_result_t mdbx_cache_get_SingleThreaded(const MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *key,
                                                              MDBX_val *data, MDBX_cache_entry_t *entry);

/** \brief Store items into a table.
 * \ingroup c_crud
 *
 * This function stores key/data pairs in the table. The default behavior
 * is to enter the new key/data pair, replacing any previously existing key
 * if duplicates are disallowed, or adding a duplicate data item if
 * duplicates are allowed (see \ref MDBX_DUPSORT).
 *
 * \param [in] txn        A transaction handle returned
 *                        by \ref mdbx_txn_begin().
 * \param [in] dbi        A table handle returned by \ref mdbx_dbi_open().
 * \param [in] key        The key to store in the table.
 * \param [in,out] data   The data to store.
 * \param [in] flags      Special options for this operation.
 *                        This parameter must be set to 0 or by bitwise OR'ing
 *                        together one or more of the values described here:
 *   - \ref MDBX_NODUPDATA
 *      Enter the new key-value pair only if it does not already appear
 *      in the table. This flag may only be specified if the table
 *      was opened with \ref MDBX_DUPSORT. The function will return
 *      \ref MDBX_KEYEXIST if the key/data pair already appears in the table.
 *
 *  - \ref MDBX_NOOVERWRITE
 *      Enter the new key/data pair only if the key does not already appear
 *      in the table. The function will return \ref MDBX_KEYEXIST if the key
 *      already appears in the table, even if the table supports
 *      duplicates (see \ref  MDBX_DUPSORT). The data parameter will be set
 *      to point to the existing item.
 *
 *  - \ref MDBX_CURRENT
 *      Update an single existing entry, but not add new ones. The function will
 *      return \ref MDBX_NOTFOUND if the given key not exist in the table.
 *      In case multi-values for the given key, with combination of
 *      the \ref MDBX_ALLDUPS will replace all multi-values,
 *      otherwise return the \ref MDBX_EMULTIVAL.
 *
 *  - \ref MDBX_RESERVE
 *      Reserve space for data of the given size, but don't copy the given
 *      data. Instead, return a pointer to the reserved space, which the
 *      caller can fill in later - before the next update operation or the
 *      transaction ends. This saves an extra memcpy if the data is being
 *      generated later. MDBX does nothing else with this memory, the caller
 *      is expected to modify all of the space requested. This flag must not
 *      be specified if the table was opened with \ref MDBX_DUPSORT.
 *
 *  - \ref MDBX_APPEND
 *      Append the given key/data pair to the end of the table. This option
 *      allows fast bulk loading when keys are already known to be in the
 *      correct order. Loading unsorted keys with this flag will cause
 *      a \ref MDBX_EKEYMISMATCH error.
 *
 *  - \ref MDBX_APPENDDUP
 *      As above, but for sorted dup data.
 *
 *  - \ref MDBX_MULTIPLE
 *      Store multiple contiguous data elements in a single request. This flag
 *      may only be specified if the table was opened with
 *      \ref MDBX_DUPFIXED. With combination the \ref MDBX_ALLDUPS
 *      will replace all multi-values.
 *      The data argument must be an array of two \ref MDBX_val. The `iov_len`
 *      of the first \ref MDBX_val must be the size of a single data element.
 *      The `iov_base` of the first \ref MDBX_val must point to the beginning
 *      of the array of contiguous data elements which must be properly aligned
 *      in case of table with \ref MDBX_INTEGERDUP flag.
 *      The `iov_len` of the second \ref MDBX_val must be the count of the
 *      number of data elements to store. On return this field will be set to
 *      the count of the number of elements actually written. The `iov_base` of
 *      the second \ref MDBX_val is unused.
 *
 * \see \ref c_crud_hints "Quick reference for Insert/Update/Delete operations"
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_KEYEXIST  The key/value pair already exists in the table.
 * \retval MDBX_MAP_FULL  The database is full, see \ref mdbx_env_set_mapsize().
 * \retval MDBX_TXN_FULL  The transaction has too many dirty pages.
 * \retval MDBX_EACCES    An attempt was made to write
 *                        in a read-only transaction.
 * \retval MDBX_EINVAL    An invalid parameter was specified. */
LIBMDBX_API int mdbx_put(MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *key, MDBX_val *data, MDBX_put_flags_t flags);

/** \brief Replaces item in a table.
 * \ingroup c_crud
 *
 * This function allows to update or delete an existing value at the same time
 * as the previous value is retrieved. If the argument new_data equal is NULL
 * zero, the removal is performed, otherwise the update/insert.
 *
 * The current value may be in an already changed (aka dirty) page. In this
 * case, the page will be overwritten during the update, and the old value will
 * be lost. Therefore, an additional buffer must be passed via old_data
 * argument initially to copy the old value. If the buffer passed in is too
 * small, the function will return \ref MDBX_RESULT_TRUE by setting iov_len
 * field pointed by old_data argument to the appropriate value, without
 * performing any changes.
 *
 * For tables with non-unique keys (i.e. with \ref MDBX_DUPSORT flag),
 * another use case is also possible, when by old_data argument selects a
 * specific item from multi-value/duplicates with the same key for deletion or
 * update. To select this scenario in flags should simultaneously specify
 * \ref MDBX_CURRENT and \ref MDBX_NOOVERWRITE. This combination is chosen
 * because it makes no sense, and thus allows you to identify the request of
 * such a scenario.
 *
 * \param [in] txn           A transaction handle returned
 *                           by \ref mdbx_txn_begin().
 * \param [in] dbi           A table handle returned by \ref mdbx_dbi_open().
 * \param [in] key           The key to store in the table.
 * \param [in] new_data      The data to store, if NULL then deletion will
 *                           be performed.
 * \param [in,out] old_data  The buffer for retrieve previous value as describe
 *                           above.
 * \param [in] flags         Special options for this operation.
 *                           This parameter must be set to 0 or by bitwise
 *                           OR'ing together one or more of the values
 *                           described in \ref mdbx_put() description above,
 *                           and additionally
 *                           (\ref MDBX_CURRENT | \ref MDBX_NOOVERWRITE)
 *                           combination for selection particular item from
 *                           multi-value/duplicates.
 *
 * \see mdbx_replace_ex()
 * \see \ref c_crud_hints "Quick reference for Insert/Update/Delete operations"
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_replace(MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *key, MDBX_val *new_data, MDBX_val *old_data,
                             MDBX_put_flags_t flags);

/** \brief A data preservation callback for using within \ref mdbx_replace_ex().
 * \ingroup c_crud */
typedef int (*MDBX_preserve_func)(void *context, MDBX_val *target, const void *src, size_t bytes);

/** \brief Replaces item in a table using preservation callback for an original data.
 * \ingroup c_crud
 *
 * This function allows to update or delete an existing value at the same time
 * as the previous value is retrieved. If the argument new_data equal is NULL
 * zero, the removal is performed, otherwise the update/insert.
 *
 * The current value may be in an already changed (aka dirty) page. In this
 * case, the page will be overwritten during the update, and the old value will
 * be lost. In such cases, the given preservation callback will be used to save
 * the source data, that, at your discretion, can perform copying, other necessary
 * actions, or return a special error code.
 *
 * If an original data needs to be saved then the passed preservation callback will be called
 * with `old_data` as a `target` parameter, and `src` with `bytes` for original data.
 * Such callback should check necessary conditions, perform appropriate action and return
 * corresponding error code, which will be returned from function as is.
 * For example, for behavior similar to \ref mdbx_replace(), the callback should check if there
 * provided buffer size is enough, then either copy the data or return \ref MDBX_RESULT_TRUE.
 *
 * For tables with non-unique keys (i.e. with \ref MDBX_DUPSORT flag),
 * another use case is also possible, when by old_data argument selects a
 * specific item from multi-value/duplicates with the same key for deletion or
 * update. To select this scenario in flags should simultaneously specify
 * \ref MDBX_CURRENT and \ref MDBX_NOOVERWRITE. This combination is chosen
 * because it makes no sense, and thus allows you to identify the request of
 * such a scenario.
 *
 * \param [in] txn           A transaction handle returned
 *                           by \ref mdbx_txn_begin().
 * \param [in] dbi           A table handle returned by \ref mdbx_dbi_open().
 * \param [in] key           The key to store in the table.
 * \param [in] new_data      The data to store, if NULL then deletion will
 *                           be performed.
 * \param [in,out] old_data  The buffer for retrieve previous value as describe
 *                           above.
 * \param [in] flags         Special options for this operation.
 *                           This parameter must be set to 0 or by bitwise
 *                           OR'ing together one or more of the values
 *                           described in \ref mdbx_put() description above,
 *                           and additionally
 *                           (\ref MDBX_CURRENT | \ref MDBX_NOOVERWRITE)
 *                           combination for selection particular item from
 *                           multi-value/duplicates.
 * \param [in] preserver     The callback to preserve an original data in case
 *                           it is on a dirty page and could be overwritten.
 * \param [in] preserver_context The optional context pointer for use within the preserving callback.
 *
 * \see mdbx_replace_ex()
 * \see MDBX_preserve_func
 * \see \ref c_crud_hints "Quick reference for Insert/Update/Delete operations"
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_replace_ex(MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *key, MDBX_val *new_data,
                                MDBX_val *old_data, MDBX_put_flags_t flags, MDBX_preserve_func preserver,
                                void *preserver_context);

/** \brief Delete items from a table.
 * \ingroup c_crud
 *
 * This function removes key/data pairs from the table.
 *
 * \note The data parameter is NOT ignored regardless the table does
 * support sorted duplicate data items or not. If the data parameter
 * is non-NULL only the matching data item will be deleted. Otherwise, if data
 * parameter is NULL, any/all value(s) for specified key will be deleted.
 *
 * This function will return \ref MDBX_NOTFOUND if the specified key/data
 * pair is not in the table.
 *
 * \see \ref c_crud_hints "Quick reference for Insert/Update/Delete operations"
 *
 * \param [in] txn   A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi   A table handle returned by \ref mdbx_dbi_open().
 * \param [in] key   The key to delete from the table.
 * \param [in] data  The data to delete.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EACCES   An attempt was made to write
 *                       in a read-only transaction.
 * \retval MDBX_EINVAL   An invalid parameter was specified. */
LIBMDBX_API int mdbx_del(MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *key, const MDBX_val *data);

/** \brief Create a cursor handle but not bind it to transaction nor DBI-handle.
 * \ingroup c_cursors
 *
 * A cursor cannot be used when its table handle is closed. Nor when its
 * transaction has ended, except with \ref mdbx_cursor_bind() and \ref
 * mdbx_cursor_renew(). Also it can be discarded with \ref mdbx_cursor_close().
 *
 * A cursor must be closed explicitly always, before or after its transaction
 * ends. It can be reused with \ref mdbx_cursor_bind()
 * or \ref mdbx_cursor_renew() before finally closing it.
 *
 * \note In contrast to LMDB, the MDBX required that any opened cursors can be
 * reused and must be freed explicitly, regardless ones was opened in a
 * read-only or write transaction. The REASON for this is eliminates ambiguity
 * which helps to avoid errors such as: use-after-free, double-free, i.e.
 * memory corruption and segfaults.
 *
 * \param [in] context A pointer to application context to be associated with
 *                     created cursor and could be retrieved by
 *                     \ref mdbx_cursor_get_userctx() until cursor closed.
 *
 * \returns Created cursor handle or NULL in case out of memory. */
LIBMDBX_API MDBX_cursor *mdbx_cursor_create(void *context);

/** \brief Set application information associated with the cursor.
 * \ingroup c_cursors
 * \see mdbx_cursor_get_userctx()
 *
 * \param [in] cursor  An cursor handle returned by \ref mdbx_cursor_create()
 *                     or \ref mdbx_cursor_open().
 * \param [in] ctx     An arbitrary pointer for whatever the application needs.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_cursor_set_userctx(MDBX_cursor *cursor, void *ctx);

/** \brief Get the application information associated with the MDBX_cursor.
 * \ingroup c_cursors
 * \see mdbx_cursor_set_userctx()
 *
 * \param [in] cursor  An cursor handle returned by \ref mdbx_cursor_create()
 *                     or \ref mdbx_cursor_open().
 * \returns The pointer which was passed via the `context` parameter
 *          of `mdbx_cursor_create()` or set by \ref mdbx_cursor_set_userctx(),
 *          or `NULL` if something wrong. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API void *mdbx_cursor_get_userctx(const MDBX_cursor *cursor);

/** \brief Bind cursor to specified transaction and DBI-handle.
 * \ingroup c_cursors
 *
 * Using of the `mdbx_cursor_bind()` is equivalent to calling
 * \ref mdbx_cursor_renew() but with specifying an arbitrary DBI-handle.
 *
 * A cursor may be associated with a new transaction, and referencing a new or
 * the same table handle as it was created with. This may be done whether the
 * previous transaction is live or dead.
 *
 * If the transaction is nested, then the cursor should not be used in its parent transaction.
 * Otherwise it is no way to restore state if this nested transaction will be aborted,
 * nor impossible to define the expected behavior.
 *
 * \note In contrast to LMDB, the MDBX required that any opened cursors can be
 * reused and must be freed explicitly, regardless ones was opened in a
 * read-only or write transaction. The REASON for this is eliminates ambiguity
 * which helps to avoid errors such as: use-after-free, double-free, i.e.
 * memory corruption and segfaults.
 *
 * \param [in] txn      A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi      A table handle returned by \ref mdbx_dbi_open().
 * \param [in] cursor   A cursor handle returned by \ref mdbx_cursor_create().
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL  An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_bind(MDBX_txn *txn, MDBX_cursor *cursor, MDBX_dbi dbi);

/** \brief Unbind cursor from a transaction.
 * \ingroup c_cursors
 *
 * Unbinded cursor is disassociated with any transactions but still holds
 * the original DBI-handle internally. Thus it could be renewed with any running
 * transaction or closed.
 *
 * If the transaction is nested, then the cursor should not be used in its parent transaction.
 * Otherwise it is no way to restore state if this nested transaction will be aborted,
 * nor impossible to define the expected behavior.
 *
 * \see mdbx_cursor_renew()
 * \see mdbx_cursor_bind()
 * \see mdbx_cursor_close()
 * \see mdbx_cursor_reset()
 *
 * \note In contrast to LMDB, the MDBX required that any opened cursors can be
 * reused and must be freed explicitly, regardless ones was opened in a
 * read-only or write transaction. The REASON for this is eliminates ambiguity
 * which helps to avoid errors such as: use-after-free, double-free, i.e.
 * memory corruption and segfaults.
 *
 * \param [in] cursor   A cursor handle returned by \ref mdbx_cursor_open().
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_cursor_unbind(MDBX_cursor *cursor);

/** \brief Resets the cursor state.
 * \ingroup c_cursors
 *
 * \details As a result of the reset, the cursor becomes unpositioned and does not allow relative positioning
 * operations, getting or changing data until cursor is set to a position independent of the current one.
 * This allows to stop further operations without first positioning the cursor.
 *
 * \param [in] cursor   A cursor handle returned by \ref mdbx_cursor_open().
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_cursor_reset(MDBX_cursor *cursor);

/** \brief Create a cursor handle for the specified transaction and DBI handle.
 * \ingroup c_cursors
 *
 * Using of the `mdbx_cursor_open()` is equivalent to calling
 * \ref mdbx_cursor_create() and then \ref mdbx_cursor_bind() functions.
 *
 * A cursor cannot be used when its table handle is closed. Nor when its
 * transaction has ended, except with \ref mdbx_cursor_bind() and \ref
 * mdbx_cursor_renew(). Also it can be discarded with \ref mdbx_cursor_close().
 *
 * A cursor must be closed explicitly always, before or after its transaction
 * ends. It can be reused with \ref mdbx_cursor_bind()
 * or \ref mdbx_cursor_renew() before finally closing it.
 *
 * \note In contrast to LMDB, the MDBX required that any opened cursors can be
 * reused and must be freed explicitly, regardless ones was opened in a
 * read-only or write transaction. The REASON for this is eliminates ambiguity
 * which helps to avoid errors such as: use-after-free, double-free, i.e.
 * memory corruption and segfaults.
 *
 * \param [in] txn      A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi      A table handle returned by \ref mdbx_dbi_open().
 * \param [out] cursor  Address where the new \ref MDBX_cursor handle will be
 *                      stored.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL  An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_open(MDBX_txn *txn, MDBX_dbi dbi, MDBX_cursor **cursor);

/** \brief Closes a cursor handle without returning error code.
 * \ingroup c_cursors
 *
 * The cursor handle will be freed and must not be used again after this call,
 * but its transaction may still be live.
 *
 * This function returns `void` but panic in case of error. Use \ref mdbx_cursor_close2()
 * if you need to receive an error code instead of an app crash.
 *
 * \see mdbx_cursor_close2
 *
 * \note In contrast to LMDB, the MDBX required that any opened cursors can be
 * reused and must be freed explicitly, regardless ones was opened in a
 * read-only or write transaction. The REASON for this is eliminates ambiguity
 * which helps to avoid errors such as: use-after-free, double-free, i.e.
 * memory corruption and segfaults.
 *
 * \param [in] cursor  A cursor handle returned by \ref mdbx_cursor_open()
 *                     or \ref mdbx_cursor_create(). */
LIBMDBX_API void mdbx_cursor_close(MDBX_cursor *cursor);

/** \brief Closes a cursor handle with returning error code.
 * \ingroup c_cursors
 *
 * The cursor handle will be freed and must not be used again after this call,
 * but its transaction may still be live.
 *
 * \see mdbx_cursor_close
 *
 * \note In contrast to LMDB, the MDBX required that any opened cursors can be
 * reused and must be freed explicitly, regardless ones was opened in a
 * read-only or write transaction. The REASON for this is eliminates ambiguity
 * which helps to avoid errors such as: use-after-free, double-free, i.e.
 * memory corruption and segfaults.
 *
 * \param [in] cursor  A cursor handle returned by \ref mdbx_cursor_open()
 *                     or \ref mdbx_cursor_create().
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL  An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_close2(MDBX_cursor *cursor);

/** \brief Unbind or closes all cursors of a given transaction and of all
 * its parent transactions if ones are.
 * \ingroup c_cursors
 *
 * Unbinds either closes all cursors associated (opened, renewed or binded) with
 * the given transaction in a bulk with minimal overhead.
 *
 * \see mdbx_cursor_unbind()
 * \see mdbx_cursor_close()
 *
 * \param [in] txn        A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] unbind     If non-zero, unbinds cursors and leaves ones reusable.
 *                        Otherwise close and dispose cursors.
 * \param [in,out] count  An optional pointer to return the number of cursors
 *                        processed by the requested operation.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_BAD_TXN          Given transaction is invalid or has
 *                               a child/nested transaction transaction. */
LIBMDBX_API int mdbx_txn_release_all_cursors_ex(const MDBX_txn *txn, bool unbind, size_t *count);

/** \brief Unbind or closes all cursors of a given transaction and of all
 * its parent transactions if ones are.
 * \ingroup c_cursors
 *
 * Unbinds either closes all cursors associated (opened, renewed or binded) with
 * the given transaction in a bulk with minimal overhead.
 *
 * \see mdbx_cursor_unbind()
 * \see mdbx_cursor_close()
 *
 * \param [in] txn      A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] unbind   If non-zero, unbinds cursors and leaves ones reusable.
 *                      Otherwise close and dispose cursors.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_BAD_TXN          Given transaction is invalid or has
 *                               a child/nested transaction transaction. */
LIBMDBX_INLINE_API(int, mdbx_txn_release_all_cursors, (const MDBX_txn *txn, bool unbind)) {
  return mdbx_txn_release_all_cursors_ex(txn, unbind, NULL);
}

/** \brief Renew a cursor handle for use within the given transaction.
 * \ingroup c_cursors
 *
 * A cursor may be associated with a new transaction whether the previous
 * transaction is running or finished.
 *
 * Using of the `mdbx_cursor_renew()` is equivalent to calling
 * \ref mdbx_cursor_bind() with the DBI-handle that previously
 * the cursor was used with.
 *
 * \note In contrast to LMDB, the MDBX allow any cursor to be re-used by using
 * \ref mdbx_cursor_renew(), to avoid unnecessary malloc/free overhead until it
 * freed by \ref mdbx_cursor_close().
 *
 * \param [in] txn      A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] cursor   A cursor handle returned by \ref mdbx_cursor_open().
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL  An invalid parameter was specified.
 * \retval MDBX_BAD_DBI The cursor was not bound to a DBI-handle
 *                      or such a handle became invalid. */
LIBMDBX_API int mdbx_cursor_renew(MDBX_txn *txn, MDBX_cursor *cursor);

/** \brief Return the cursor's transaction handle.
 * \ingroup c_cursors
 *
 * \param [in] cursor A cursor handle returned by \ref mdbx_cursor_open(). */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API MDBX_txn *mdbx_cursor_txn(const MDBX_cursor *cursor);

/** \brief Return the cursor's table handle.
 * \ingroup c_cursors
 *
 * \param [in] cursor  A cursor handle returned by \ref mdbx_cursor_open(). */
LIBMDBX_API MDBX_dbi mdbx_cursor_dbi(const MDBX_cursor *cursor);

/** \brief Copy cursor position and state.
 * \ingroup c_cursors
 *
 * \param [in] src       A source cursor handle returned
 * by \ref mdbx_cursor_create() or \ref mdbx_cursor_open().
 *
 * \param [in,out] dest  A destination cursor handle returned
 * by \ref mdbx_cursor_create() or \ref mdbx_cursor_open().
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_cursor_copy(const MDBX_cursor *src, MDBX_cursor *dest);

/** \brief Compares the position of the cursors.
 * \ingroup c_cursors
 *
 * This function is intended to compare the positions of two  cursors associated with the same transaction and the same
 * table (DBI descriptor). If the cursors are associated with different transactions, or with different tables, or one
 * of them is not initialized, then the result of the comparison is undefined (the behavior may be changed in subsequent
 * versions).
 *
 * \param [in] left             A left cursor for comparing positions.
 * \param [in] right            A right cursor for comparing positions.
 * \param [in] ignore_multival  A boolean option that affects the result only when comparing cursors for tables with
 *                              multi-values, i.e. with the \ref MDBX_DUPSORT flag. In the case of `true`,
 *                              cursor positions are compared only by keys, without taking into account positioning
 *                              among multi-values. Otherwise, in the case of `false`, if the key positions match,
 *                              the multi-value positions are also compared.
 *
 * \retval A signed value in the semantics of the operator `<=>` (less than zero, zero, or greater than zero) as
 * a result of comparing cursor positions. */
LIBMDBX_API int mdbx_cursor_compare(const MDBX_cursor *left, const MDBX_cursor *right, bool ignore_multival);

/** \brief Retrieve by cursor.
 * \ingroup c_crud
 *
 * This function retrieves key/data pairs from the table. The address and
 * length of the key are returned in the object to which key refers (except
 * for the case of the \ref MDBX_SET option, in which the key object is
 * unchanged), and the address and length of the data are returned in the object
 * to which data refers.
 * \see mdbx_get()
 *
 * \note The memory pointed to by the returned values is owned by the
 * database. The caller MUST not dispose of the memory, and MUST not modify it
 * in any way regardless in a read-only nor read-write transactions!
 * Modification attempts have undefined behavior. Values may be backed by
 * explicit read-cache pages or by dirty pages in a read-write transaction; in
 * the latter case modifying returned memory can corrupt data committed by the
 * transaction.
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 * \param [in,out] key   The key for a retrieved item.
 * \param [in,out] data  The data of a retrieved item.
 * \param [in] op        A cursor operation \ref MDBX_cursor_op.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_NOTFOUND  No matching key found.
 * \retval MDBX_EINVAL    An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_get(MDBX_cursor *cursor, MDBX_val *key, MDBX_val *data, MDBX_cursor_op op);

/** \brief An auxiliary function for use in tools.
 * \ingroup c_extra
 *
 * When using user-defined comparison functions, checking the order of keys or values will lead to incorrect results and
 * return the error \ref MDBX_CORRUPTED.
 *
 * This function disables the control of the order of keys when reading database pages for this cursor, and thus allows
 * to access data in the absence/unavailability of the comparison functions used.
 *
 * \see avoid_custom_comparators
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_cursor_ignord(MDBX_cursor *cursor);

/** \brief The type of predicative callback functions used by \ref mdbx_cursor_scan() and \ref mdbx_cursor_scan_from()
 * to probing key-value pairs.
 *
 * \ingroup c_crud
 *
 * \param [in,out] context  A pointer to the context with the necessary information for evaluation,
 *                          which is fully prepared and controlled by you.
 * \param [in] key          The key for evaluation by a callback function.
 * \param [in] value        The value for evaluation by a callback function.
 * \param [in,out] arg      An auxiliary argument to the predicative function,
 *                          which is fully prepared and controlled by you.
 *
 * \returns The result of checking whether the transmitted key-value pair matches the desired goal.
 * Either, an error code that interrupts the scan and is returned unchanged as a result from the \ref mdbx_cursor_scan()
 * or \ref mdbx_cursor_scan_from() functions.
 *
 * \retval MDBX_RESULT_TRUE if the given key-value pair matches the one you are looking for, andthe scan should be
 * completed.
 *
 * \retval MDBX_RESULT_FALSE if the given key-value pair does NOT match the one you are looking for, and scanning should
 * continue.
 *
 * \retval OTHERWISE any other value other than \ref MDBX_RESULT_TRUE and \ref MDBX_RESULT_FALSE is considered an error
 * indicator and is returned unchanged as a scan result.
 *
 * \see mdbx_cursor_scan()
 * \see mdbx_cursor_scan_from() */
typedef int (*MDBX_predicate_func)(void *context, MDBX_val *key, MDBX_val *value, void *arg) MDBX_CXX17_NOEXCEPT;

/** \brief Scans the table using the passed predicate, reducing the associated overhead.
 * \ingroup c_crud
 *
 * Implements functionality similar to the `std::find_if<>()` template using a cursor and a custom predicative function,
 * while reducing on related overhead costs, including not performing some checks inside the record iteration cycle
 * and potentially reducing the number of DSO cross-border calls.
 *
 * The function accepts a cursor, which should be bound to some transaction and a table DBI-descriptor,
 * performs the initial cursor positioning determined by the `start_op` argument. Next, each key-value pair
 * is probed using the predicative function `predict` provided by you, and then, if necessary, move on to
 * the next using the `turn_op` operation, until one of the four events occurs:
 *  - the end of data is reached;
 *  - an error occurs when positioning the cursor;
 *  - the probing function returns \ref MDBX_RESULT_TRUE,
 *    signaling the need to stop further scanning.;
 *  - the probing function returns a value other than \ref MDBX_RESULT_FALSE or \ref MDBX_RESULT_TRUE,
 *    signaling an error.
 *
 * \param [in,out] cursor   The cursor for performing the scan operation associated with the active transaction
 *                          and the table's DBI descriptor. For instance,
 *                          a cursor created using \ref mdbx_cursor_open().
 * \param [in] predicate    A predicative function for probing key-value pairs,
 *                          see \ref MDBX_predicate_func for more details.
 * \param [in,out] context  A pointer to the context with an auxiliary information for probing,
 *                          which is fully prepared and controlled by you.
 * \param [in] start_op     The initial cursor positioning operation, see \ref MDBX_cursor_op for more details.
 *                          To scan without changing the initial cursor position, use \ref MDBX_GET_CURRENT.
 *                          Acceptable values are \ref MDBX_FIRST, \ref MDBX_FIRST_DUP, \ref MDBX_LAST,
 *                          \ref MDBX_LAST_DUP, \ref MDBX_GET_CURRENT, and also \ref MDBX_GET_MULTIPLE.
 * \param [in] turn_op      The operation of positioning the cursor to move to the next element.
 *                          Acceptable values are \ref MDBX_NEXT, \ref MDBX_NEXT_DUP,
 *                          \ref MDBX_NEXT_NODUP, \ref MDBX_PREV,
 *                          \ref MDBX_PREV_DUP, \ref MDBX_PREV_NODUP, and also
 *                          \ref MDBX_NEXT_MULTIPLE и \ref MDBX_PREV_MULTIPLE.
 * \param [in,out] arg      An auxiliary argument to the predicative function,
 *                          which is fully prepared and controlled by you.
 *
 * \note When using \ref MDBX_GET_MULTIPLE, \ref MDBX_NEXT_MULTIPLE, or \ref MDBX_PREV_MULTIPLE,
 * carefully consider the batch specifics of passing values through the parameters of the predicative function.
 *
 * \see MDBX_predicate_func
 * \see mdbx_cursor_scan_from
 *
 * \returns The result of the scan operation, or an error code.
 *
 * \retval MDBX_RESULT_TRUE  if a key-value pair is found for which the predicative function
 *                           returned \ref MDBX_RESULT_TRUE.
 * \retval MDBX_RESULT_FALSE if a suitable key-value pair is NOT found,
 *                           the end of the data has been reached during the search, or there is no data to search for.
 * \retval OTHERWISE any other value other than \ref MDBX_RESULT_TRUE and \ref MDBX_RESULT_FALSE is an error code
 *         during positioning the cursor or a user-defined code for stopping the search or an user-defined error. */
LIBMDBX_API int mdbx_cursor_scan(MDBX_cursor *cursor, MDBX_predicate_func predicate, void *context,
                                 MDBX_cursor_op start_op, MDBX_cursor_op turn_op, void *arg);

/** Scans a table using the given predicate, starting with the given key-value pair,
 *  and reduces an associated overhead.
 * \ingroup c_crud
 *
 * \details The function accepts a cursor, which should be bound to some transaction and a table DBI-descriptor,
 * performs the initial cursor positioning determined by the `from_op` argument, as well as the arguments `from_key`
 * and `from_value`. Next, each key-value pair is probed using the given predicative function `predict`, and then,
 * if necessary, move on to the next using the `turn_op` operation, until one of the four events occurs:
 *
 *  - the end of data is reached;
 *  - an error occurs when positioning the cursor;
 *  - the probing function returns \ref MDBX_RESULT_TRUE,
 *    signaling the need to stop further scanning;
 *  - the probing function returns a value other than \ref MDBX_RESULT_FALSE or \ref MDBX_RESULT_TRUE,
 *    signaling an error.
 *
 * \param [in,out] cursor    The cursor for performing the scan operation associated with the active transaction
 *                           and the table's DBI descriptor. For instance,
 *                           a cursor created using \ref mdbx_cursor_open().
 * \param [in] predicate     A predicative function for probing key-value pairs,
 *                           see \ref MDBX_predicate_func for more details.
 * \param [in,out] context   A pointer to the context with an auxiliary information for probing,
 *                           which is fully prepared and controlled by you.
 * \param [in] from_op       The operation of positioning the cursor to the initial position, for more details,
 *                           see \ref MDBX_cursor_op. Acceptable values are \ref MDBX_GET_BOTH,
 *                           \ref MDBX_GET_BOTH_RANGE, \ref MDBX_SET_KEY,
 *                           \ref MDBX_SET_LOWERBOUND, \ref MDBX_SET_UPPERBOUND,
 *                           \ref MDBX_TO_KEY_LESSER_THAN,
 *                           \ref MDBX_TO_KEY_LESSER_OR_EQUAL,
 *                           \ref MDBX_TO_KEY_EQUAL,
 *                           \ref MDBX_TO_KEY_GREATER_OR_EQUAL,
 *                           \ref MDBX_TO_KEY_GREATER_THAN,
 *                           \ref MDBX_TO_EXACT_KEY_VALUE_LESSER_THAN,
 *                           \ref MDBX_TO_EXACT_KEY_VALUE_LESSER_OR_EQUAL,
 *                           \ref MDBX_TO_EXACT_KEY_VALUE_EQUAL,
 *                           \ref MDBX_TO_EXACT_KEY_VALUE_GREATER_OR_EQUAL,
 *                           \ref MDBX_TO_EXACT_KEY_VALUE_GREATER_THAN,
 *                           \ref MDBX_TO_PAIR_LESSER_THAN,
 *                           \ref MDBX_TO_PAIR_LESSER_OR_EQUAL,
 *                           \ref MDBX_TO_PAIR_EQUAL,
 *                           \ref MDBX_TO_PAIR_GREATER_OR_EQUAL,
 *                           \ref MDBX_TO_PAIR_GREATER_THAN,
 *                           and also \ref MDBX_GET_MULTIPLE.
 * \param [in,out] from_key  A pointer to the key used both for initial positioning
 *                           and for subsequent steps.
 * \param [in,out] from_value A pointer to the value used both for initial positioning
 *                           and for subsequent steps.
 * \param [in] turn_op       The operation of positioning the cursor to move to the next element.
 *                           Acceptable values are \ref MDBX_NEXT, \ref MDBX_NEXT_DUP,
 *                           \ref MDBX_NEXT_NODUP, \ref MDBX_PREV,
 *                           \ref MDBX_PREV_DUP, \ref MDBX_PREV_NODUP, and also
 *                           \ref MDBX_NEXT_MULTIPLE и \ref MDBX_PREV_MULTIPLE.
 * \param [in,out] arg       An auxiliary argument to the predicative function,
 *                           which is fully prepared and controlled by you.
 *
 * \note When using \ref MDBX_GET_MULTIPLE, \ref MDBX_NEXT_MULTIPLE, or \ref MDBX_PREV_MULTIPLE,
 * carefully consider the batch specifics of passing values through the parameters of the predicative function.
 *
 * \see MDBX_predicate_func
 * \see mdbx_cursor_scan
 *
 * \returns The result of the scan operation, or an error code.
 *
 * \retval MDBX_RESULT_TRUE  if a key-value pair is found for which the predicative function
 *                           returned \ref MDBX_RESULT_TRUE.
 * \retval MDBX_RESULT_FALSE if a suitable key-value pair is NOT found,
 *                           the end of the data has been reached during the search, or there is no data to search for.
 * \retval OTHERWISE any other value other than \ref MDBX_RESULT_TRUE and \ref MDBX_RESULT_FALSE is an error code
 *         during positioning the cursor or a user-defined code for stopping the search or an user-defined error. */
LIBMDBX_API int mdbx_cursor_scan_from(MDBX_cursor *cursor, MDBX_predicate_func predicate, void *context,
                                      MDBX_cursor_op from_op, MDBX_val *from_key, MDBX_val *from_value,
                                      MDBX_cursor_op turn_op, void *arg);

/** \brief Retrieve multiple non-dupsort key/value pairs by cursor.
 * \ingroup c_crud
 *
 * This function retrieves multiple key/data pairs from the table without
 * \ref MDBX_DUPSORT option. For `MDBX_DUPSORT` tables please
 * use \ref MDBX_GET_MULTIPLE and \ref MDBX_NEXT_MULTIPLE.
 *
 * The number of key and value items is returned in the `count`
 * refers. The addresses and lengths of the keys and values are returned in the
 * array to which `pairs` refers.
 * \see mdbx_cursor_get()
 *
 * \note The memory pointed to by the returned values is owned by the
 * database. The caller MUST not dispose of the memory, and MUST not modify it
 * in any way regardless in a read-only nor read-write transactions!
 * Modification attempts have undefined behavior. Values may be backed by
 * explicit read-cache pages or by dirty pages in a read-write transaction; in
 * the latter case modifying returned memory can corrupt data committed by the
 * transaction.
 *
 * \param [in] cursor     A cursor handle returned by \ref mdbx_cursor_open().
 * \param [out] count     The number of key and value item returned, on success
 *                        it always be the even because the key-value
 *                        pairs are returned.
 * \param [in,out] pairs  A pointer to the array of key value pairs.
 * \param [in] limit      The size of pairs buffer as the number of items,
 *                        but not a pairs.
 * \param [in] op         A cursor operation \ref MDBX_cursor_op (only
 *                        \ref MDBX_FIRST and \ref MDBX_NEXT are supported).
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_NOTFOUND         No any key-value pairs are available.
 * \retval MDBX_ENODATA          The cursor is already at the end of data.
 * \retval MDBX_RESULT_TRUE      The returned chunk is the last one,
 *                               and there are no pairs left.
 * \retval MDBX_EINVAL           An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_get_batch(MDBX_cursor *cursor, size_t *count, MDBX_val *pairs, size_t limit,
                                      MDBX_cursor_op op);

/** \brief Store by cursor.
 * \ingroup c_crud
 *
 * This function stores key/data pairs into the table. The cursor is
 * positioned at the new item, or on failure usually near it.
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 * \param [in] key       The key operated on.
 * \param [in,out] data  The data operated on.
 * \param [in] flags     Options for this operation. This parameter
 *                       must be set to 0 or by bitwise OR'ing together
 *                       one or more of the values described here:
 *  - \ref MDBX_CURRENT
 *      Replace the item at the current cursor position. The key parameter
 *      must still be provided, and must match it, otherwise the function
 *      return \ref MDBX_EKEYMISMATCH. With combination the
 *      \ref MDBX_ALLDUPS will replace all multi-values.
 *
 *      \note MDBX allows (unlike LMDB) you to change the size of the data and
 *      automatically handles reordering for sorted duplicates
 *      (see \ref MDBX_DUPSORT).
 *
 *  - \ref MDBX_NODUPDATA
 *      Enter the new key-value pair only if it does not already appear in the
 *      table. This flag may only be specified if the table was opened
 *      with \ref MDBX_DUPSORT. The function will return \ref MDBX_KEYEXIST
 *      if the key/data pair already appears in the table.
 *
 *  - \ref MDBX_NOOVERWRITE
 *      Enter the new key/data pair only if the key does not already appear
 *      in the table. The function will return \ref MDBX_KEYEXIST if the key
 *      already appears in the table, even if the table supports
 *      duplicates (\ref MDBX_DUPSORT).
 *
 *  - \ref MDBX_RESERVE
 *      Reserve space for data of the given size, but don't copy the given
 *      data. Instead, return a pointer to the reserved space, which the
 *      caller can fill in later - before the next update operation or the
 *      transaction ends. This saves an extra memcpy if the data is being
 *      generated later. This flag must not be specified if the table
 *      was opened with \ref MDBX_DUPSORT.
 *
 *  - \ref MDBX_APPEND
 *      Append the given key/data pair to the end of the table. No key
 *      comparisons are performed. This option allows fast bulk loading when
 *      keys are already known to be in the correct order. Loading unsorted
 *      keys with this flag will cause a \ref MDBX_KEYEXIST error.
 *
 *  - \ref MDBX_APPENDDUP
 *      As above, but for sorted dup data.
 *
 *  - \ref MDBX_MULTIPLE
 *      Store multiple contiguous data elements in a single request. This flag
 *      may only be specified if the table was opened with
 *      \ref MDBX_DUPFIXED. With combination the \ref MDBX_ALLDUPS
 *      will replace all multi-values.
 *      The data argument must be an array of two \ref MDBX_val. The `iov_len`
 *      of the first \ref MDBX_val must be the size of a single data element.
 *      The `iov_base` of the first \ref MDBX_val must point to the beginning
 *      of the array of contiguous data elements which must be properly aligned
 *      in case of table with \ref MDBX_INTEGERDUP flag.
 *      The `iov_len` of the second \ref MDBX_val must be the count of the
 *      number of data elements to store. On return this field will be set to
 *      the count of the number of elements actually written. The `iov_base` of
 *      the second \ref MDBX_val is unused.
 *
 * \see \ref c_crud_hints "Quick reference for Insert/Update/Delete operations"
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EKEYMISMATCH  The given key value is mismatched to the current
 *                            cursor position
 * \retval MDBX_MAP_FULL      The database is full,
 *                             see \ref mdbx_env_set_mapsize().
 * \retval MDBX_TXN_FULL      The transaction has too many dirty pages.
 * \retval MDBX_EACCES        An attempt was made to write in a read-only
 *                            transaction.
 * \retval MDBX_EINVAL        An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_put(MDBX_cursor *cursor, const MDBX_val *key, MDBX_val *data, MDBX_put_flags_t flags);

/** \brief Delete current key/data pair.
 * \ingroup c_crud
 *
 * This function deletes the key/data pair to which the cursor refers. This
 * does not invalidate the cursor, so operations such as \ref MDBX_NEXT can
 * still be used on it. Both \ref MDBX_NEXT and \ref MDBX_GET_CURRENT will
 * return the same record after this operation.
 *
 * \param [in] cursor  A cursor handle returned by mdbx_cursor_open().
 * \param [in] flags   Options for this operation. This parameter must be set
 * to one of the values described here.
 *
 *  - \ref MDBX_CURRENT Delete only single entry at current cursor position.
 *  - \ref MDBX_ALLDUPS
 *    or \ref MDBX_NODUPDATA (supported for compatibility)
 *      Delete all of the data items for the current key. This flag has effect
 *      only for table(s) was created with \ref MDBX_DUPSORT.
 *
 * \see \ref c_crud_hints "Quick reference for Insert/Update/Delete operations"
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_MAP_FULL      The database is full,
 *                            see \ref mdbx_env_set_mapsize().
 * \retval MDBX_TXN_FULL      The transaction has too many dirty pages.
 * \retval MDBX_EACCES        An attempt was made to write in a read-only
 *                            transaction.
 * \retval MDBX_EINVAL        An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_del(MDBX_cursor *cursor, MDBX_put_flags_t flags);

/** \brief Quickly removes given range of items.
 * \ingroup c_crud
 *
 * Performs mass deletion of elements between positions of given cursors pair much faster,
 * cutting out entire pages and branches from the B+ tree structure.
 *
 * \param [in] begin                Defines the beginning of the range to delete,
 *                                  or can be NULL to delete starting the first item.
 *
 * \param [in] end                  Defines the ending of the range to delete,
 *                                  or can be NULL to delete up to the last item.
 *
 * \param [in] end_including        The boolean flag determines whether the end of the given
 *                                  interval should be included in the range to be deleted.
 *
 * \param [out] number_of_affected  Address to store the result number of removed items.
 *
 * \see mdbx_cursor_bunch_delete()
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned by current thread.
 * \retval MDBX_ENODATA          One or both of the given cursor(s) is not positioned to a data,
 *                               or position of `begin` cursor is after the `end` cursor.
 * \retval MDBX_TXN_FULL         The transaction has too many dirty pages.
 * \retval MDBX_EACCES           An attempt was made to write in a read-only transaction.
 * \retval MDBX_EINVAL           An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_delete_range(MDBX_cursor *begin, MDBX_cursor *end, bool end_including,
                                         uint64_t *number_of_affected);

/** \brief Calculates the distance between the cursors at the specified B-tree level.
 * \ingroup c_cursors
 *
 * The value of the `deepness` parameter has a fundamental impact on the result since it limits the level of a B-tree,
 * at which the difference between the cursor positions is calculated, where zero corresponds to the root of a B-tree
 * and increases to a leaves. Lower `deepness` values allows to quickly get a rough result, avoiding reading the pages
 * of a B-tree. Large enough `deepness` values allows to find out the exact number of elements between the cursors, but
 * this will require reading all the leaf pages between ones. In order for the returned result to match the number of
 * keys and values, the `deepness` must be at least the height of a B-tree, adding the height of nested B-trees for any
 * kinds of "dupsort" tables. If in doubt, use a deliberately large value such as `INT_MAX` or just the `42`.
 *
 * \param [in] first             Cursor pointing to the first element or NULL to using the begin of a table.
 *                               Either the `first` or the `last` must not be NULL.
 *
 * \param [in] last              Cursor pointing to the end of the range or NULL to using the end of a table.
 *                               Either the `first` or the `last` must not be NULL.
 *
 * \param [out] distance         The address for storing the result calculated distance.
 *
 * \param [in] deepness          Limits the level of a B-tree, at which the difference between the cursor positions
 *                               is calculated, where zero corresponds to the root of a B-tree
 *                               and increases to a leaves.
 *
 * \see mdbx_cursor_scroll()
 * \see mdbx_cursor_distribute()
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_ENODATA          One or both of the given cursor(s) is not positioned to a data.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned by current thread.
 * \retval MDBX_EINVAL           An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_distance(const MDBX_cursor *first, const MDBX_cursor *last, intptr_t *distance,
                                     unsigned deepness);

/** \brief Scrolls the cursor to the specified number of positions at the specified B-tree level.
 * \ingroup c_cursors
 *
 * The value of the `deepness` parameter has a fundamental effect on the result, since it determines the level of the
 * B-tree at which the cursor movement steps are performed, where zero corresponds to the root of the B-tree and
 * increases to a leaves. In order for the performed cursor movement to match the number of keys and values, the
 * `deepness` must be at least the height of a B-tree, adding the height of nested B-trees for any kinds of "dupsort"
 * tables. If in doubt, use a deliberately large value such as `INT_MAX` or just the `42`.
 *
 * \param [in, out] cursor       The cursor handle to scroll.
 *
 * \param [in] amount            The number of logical steps by which the cursor will be moved at the specified
 *                               level of a b-tree. A positive value corresponds to moving forward in the order
 *                               of keys and values, to the end of a table, and a negative value means moving
 *                               in the backward order.
 *
 * \param [in] deepness          Defines the level of a B-tree, at which the cursor movement steps are performed,
 *                               where zero corresponds to the root of a B-tree and increases to a leaves.
 *
 * \see mdbx_cursor_distance()
 * \see mdbx_cursor_distribute()
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_ENODATA          Given cursor is not positioned to a data.
 * \retval MDBX_NOTFOUND         The end of the data was reached before the cursor moved
 *                               by the requested number of steps.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned by current thread.
 * \retval MDBX_EINVAL           An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_scroll(MDBX_cursor *cursor, intptr_t amount, unsigned deepness);

/** \brief Distributes cursors for multithreaded range scanning.
 * \ingroup c_cursors
 *
 * The value of the `deepness` parameter has a fundamental effect on the result, since it determines the level of the
 * B-tree at which the cursors distribution are performed, where zero corresponds to the root of the B-tree and
 * increases to a leaves. In order for the performed cursor movement to match the number of keys and values, the
 * `deepness` must be at least the height of a B-tree, adding the height of nested B-trees for any kinds of "dupsort"
 * tables. If in doubt, use a deliberately large value such as `INT_MAX` or just the `42`.
 *
 * \param [in] first             Cursor pointing to the first element or NULL to using the begin of a table.
 *                               Either the `first` or the `last` must not be NULL.
 *
 * \param [in] last              Cursor pointing to the end of the range or NULL to using the end of a table.
 *                               Either the `first` or the `last` must not be NULL.
 *
 * \param [in, out] array        The pointer to an array of cursors for distribute over a given range.
 *
 * \param [in] count             Number of element in the cursors array.
 *
 * \param [in] deepness          Defines the level of a B-tree, at which the cursors distribution is performed,
 *                               where zero corresponds to the root of a B-tree and increases to a leaves.
 *
 * \see mdbx_cursor_distance()
 * \see mdbx_cursor_scroll()
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_ENODATA          The `first` or `last` cursor(s) is not positioned to a data.
 * \retval MDBX_RESULT_TRUE      The available positions in the specified range were not enough to distribute
 *                               over all the cursors, some cursors remained unset and \ref mdbx_cursor_eof()
 *                               will return \ref MDBX_RESULT_TRUE for ones.
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned by current thread.
 * \retval MDBX_EINVAL           An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_distribute(const MDBX_cursor *first, const MDBX_cursor *last, MDBX_cursor **array,
                                       intptr_t count, unsigned deepness);

/** \brief Modes for deleting bunches of neighboring items with self-documenting names.
 *
 * The EXCLUDING and INCLUDING suffixes mean correspondingly
 * excluding and including deletion items in the current cursor position, and so forth.
 *
 * \ingroup c_crud
 * \see mdbx_cursor_bunch_delete() */
typedef enum MDBX_bunch_action {
  MDBX_DELETE_CURRENT_VALUE,
  MDBX_DELETE_CURRENT_MULTIVAL_BEFORE_EXCLUDING,
  MDBX_DELETE_CURRENT_MULTIVAL_BEFORE_INCLUDING,
  MDBX_DELETE_CURRENT_MULTIVAL_AFTER_INCLUDING,
  MDBX_DELETE_CURRENT_MULTIVAL_AFTER_EXCLUDING,
  MDBX_DELETE_CURRENT_MULTIVAL_ALL,
  MDBX_DELETE_BEFORE_EXCLUDING,
  MDBX_DELETE_BEFORE_INCLUDING,
  MDBX_DELETE_AFTER_INCLUDING,
  MDBX_DELETE_AFTER_EXCLUDING,
  MDBX_DELETE_WHOLE,
} MDBX_bunch_action_t;

/** \brief Quickly removes bunches of neighboring items.
 * \ingroup c_crud
 *
 * Performs massive deletion much faster by cutting whole pages and branches
 * with will deleted elements from the B+tree structure.
 * \see mdbx_cursor_delete_range()
 * \see MDBX_bunch_action_t
 *
 * \param [in] cursor  A cursor handle returned by mdbx_cursor_open().
 * \param [in] action  The requested deletion action as the one
 *                     value of \ref MDBX_bunch_action_t.
 * \param [out] number_of_affected  Address to store the result
 *                                  number of removed items.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_ENODATA       The given cursor is not positioned to a data.
 * \retval MDBX_TXN_FULL      The transaction has too many dirty pages.
 * \retval MDBX_EACCES        An attempt was made to write in a read-only transaction.
 * \retval MDBX_EINVAL        An invalid parameter was specified. */
LIBMDBX_API int mdbx_cursor_bunch_delete(MDBX_cursor *cursor, MDBX_bunch_action_t action, uint64_t *number_of_affected);

/** \brief Return count values (aka duplicates) for current key.
 * \ingroup c_crud
 *
 * \see mdbx_cursor_count_ex()
 *
 * This call is valid for all tables, but reasonable only for that support
 * sorted duplicate data items \ref MDBX_DUPSORT.
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 * \param [out] count    Address where the count will be stored.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL   Cursor is not initialized, or an invalid parameter
 *                       was specified. */
LIBMDBX_API int mdbx_cursor_count(const MDBX_cursor *cursor, size_t *count);

/** \brief Return count values (aka duplicates) and nested b-tree statistics for current key.
 * \ingroup c_crud
 *
 * \see mdbx_dbi_stat
 * \see mdbx_dbi_dupsort_depthmask
 * \see mdbx_cursor_count
 *
 * This call is valid for all tables, but reasonable only for that support
 * sorted duplicate data items \ref MDBX_DUPSORT.
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 * \param [out] count    Address where the count will be stored.
 * \param [out] stat     The address of an \ref MDBX_stat structure where
 *                       the statistics of a nested b-tree will be copied.
 * \param [in] bytes     The size of \ref MDBX_stat.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_THREAD_MISMATCH  Given transaction is not owned
 *                               by current thread.
 * \retval MDBX_EINVAL   Cursor is not initialized, or an invalid parameter
 *                       was specified. */
LIBMDBX_API int mdbx_cursor_count_ex(const MDBX_cursor *cursor, size_t *count, MDBX_stat *stat, size_t bytes);

/** \brief Determines whether the cursor is pointed to a key-value pair or not,
 * i.e. was not positioned or points to the end of data.
 * \ingroup c_cursors
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 *
 * \returns A \ref MDBX_RESULT_TRUE or \ref MDBX_RESULT_FALSE value,
 *          otherwise the error code.
 * \retval MDBX_RESULT_TRUE    No more data available or cursor not
 *                             positioned
 * \retval MDBX_RESULT_FALSE   A data is available
 * \retval OTHERWISE the error code */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_cursor_eof(const MDBX_cursor *cursor);

/** \brief Determines whether the cursor is pointed to the first key-value pair
 * or not.
 * \ingroup c_cursors
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 *
 * \returns A MDBX_RESULT_TRUE or MDBX_RESULT_FALSE value,
 *          otherwise the error code.
 * \retval MDBX_RESULT_TRUE   Cursor positioned to the first key-value pair
 * \retval MDBX_RESULT_FALSE  Cursor NOT positioned to the first key-value pair
 * \retval OTHERWISE the error code */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_cursor_on_first(const MDBX_cursor *cursor);

/** \brief Determines whether the cursor is on the first or single multi-value corresponding to the key.
 *
 * \ingroup c_cursors
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 *
 * \returns A \ref MDBX_RESULT_TRUE or \ref MDBX_RESULT_FALSE value, otherwise the error code.
 *
 * \retval MDBX_RESULT_TRUE   The cursor is positioned to the first or single multi-value corresponding to the key.
 *
 * \retval MDBX_RESULT_FALSE  The cursor is NOT positioned to the first or single multi-value corresponding to the key.
 *
 * \retval OTHERWISE the error code */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_cursor_on_first_dup(const MDBX_cursor *cursor);

/** \brief Determines whether the cursor is pointed to the last key-value pair or not.
 * \ingroup c_cursors
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 *
 * \returns A \ref MDBX_RESULT_TRUE or \ref MDBX_RESULT_FALSE value, otherwise the error code.
 * \retval MDBX_RESULT_TRUE   Cursor positioned to the last key-value pair
 * \retval MDBX_RESULT_FALSE  Cursor NOT positioned to the last key-value pair
 * \retval OTHERWISE the error code */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_cursor_on_last(const MDBX_cursor *cursor);

/** \brief Determines whether the cursor is on the last or single multi-value corresponding to the key.
 * \ingroup c_cursors
 *
 * \param [in] cursor    A cursor handle returned by \ref mdbx_cursor_open().
 *
 * \returns A \ref MDBX_RESULT_TRUE or \ref MDBX_RESULT_FALSE value, otherwise the error code.
 *
 * \retval MDBX_RESULT_TRUE   The cursor is positioned to the last or single multi-value corresponding to the key.
 *
 * \retval MDBX_RESULT_FALSE  The cursor is NOT positioned to the last or single multi-value corresponding to the key.
 *
 * \retval OTHERWISE the error code */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_cursor_on_last_dup(const MDBX_cursor *cursor);

/** \addtogroup c_rqest
 * \details \note The estimation result varies greatly depending on the filling
 * of specific pages and the overall balance of the b-tree:
 *
 * 1. The number of items is estimated by analyzing the height and fullness of
 * the b-tree. The accuracy of the result directly depends on the balance of
 * the b-tree, which in turn is determined by the history of previous
 * insert/delete operations and the nature of the data (i.e. variability of
 * keys length and so on). Therefore, the accuracy of the estimation can vary
 * greatly in a particular situation.
 *
 * 2. To understand the potential spread of results, you should consider a
 * possible situations basing on the general criteria for splitting and merging
 * b-tree pages:
 *  - the page is split into two when there is no space for added data;
 *  - two pages merge if the result fits in half a page;
 *  - thus, the b-tree can consist of an arbitrary combination of pages filled
 *    both completely and only 1/4. Therefore, in the worst case, the result
 *    can diverge 4 times for each level of the b-tree excepting the first and
 *    the last.
 *
 * 3. In practice, the probability of extreme cases of the above situation is
 * close to zero and in most cases the error does not exceed a few percent. On
 * the other hand, it's just a chance you shouldn't overestimate. */

/** \brief Estimates the distance between cursors as a number of elements.
 * \ingroup c_rqest
 *
 * This function performs a rough estimate based only on b-tree pages that are
 * common for the both cursor's stacks. The results of such estimation can be
 * used to build and/or optimize query execution plans.
 *
 * Please see notes on accuracy of the result in the details
 * of \ref c_rqest section.
 *
 * Both cursors must be initialized for the same table and the same
 * transaction.
 *
 * \param [in] first            The first cursor for estimation.
 * \param [in] last             The second cursor for estimation.
 * \param [out] distance_items  The pointer to store estimated distance value,
 *                              i.e. `*distance_items = distance(first, last)`.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_estimate_distance(const MDBX_cursor *first, const MDBX_cursor *last, ptrdiff_t *distance_items);

/** \brief Estimates the move distance.
 * \ingroup c_rqest
 *
 * This function performs a rough estimate distance between the current
 * cursor position and next position after the specified move-operation with
 * given key and data. The results of such estimation can be used to build
 * and/or optimize query execution plans. Current cursor position and state are
 * preserved.
 *
 * Please see notes on accuracy of the result in the details
 * of \ref c_rqest section.
 *
 * \param [in] cursor            Cursor for estimation.
 * \param [in,out] key           The key for a retrieved item.
 * \param [in,out] data          The data of a retrieved item.
 * \param [in] move_op           A cursor operation \ref MDBX_cursor_op.
 * \param [out] distance_items   A pointer to store estimated move distance
 *                               as the number of elements.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_estimate_move(const MDBX_cursor *cursor, MDBX_val *key, MDBX_val *data, MDBX_cursor_op move_op,
                                   ptrdiff_t *distance_items);

/** \brief Estimates the size of a range as a number of elements.
 * \ingroup c_rqest
 *
 * The results of such estimation can be used to build and/or optimize query
 * execution plans.
 *
 * Please see notes on accuracy of the result in the details
 * of \ref c_rqest section.
 *
 *
 * \param [in] txn        A transaction handle returned
 *                        by \ref mdbx_txn_begin().
 * \param [in] dbi        A table handle returned by  \ref mdbx_dbi_open().
 * \param [in] begin_key  The key of range beginning or NULL for explicit FIRST.
 * \param [in] begin_data Optional additional data to seeking among sorted
 *                        duplicates.
 *                        Only for \ref MDBX_DUPSORT, NULL otherwise.
 * \param [in] end_key    The key of range ending or NULL for explicit LAST.
 * \param [in] end_data   Optional additional data to seeking among sorted
 *                        duplicates.
 *                        Only for \ref MDBX_DUPSORT, NULL otherwise.
 * \param [out] distance_items  A pointer to store range estimation result.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_estimate_range(const MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *begin_key,
                                    const MDBX_val *begin_data, const MDBX_val *end_key, const MDBX_val *end_data,
                                    ptrdiff_t *distance_items);

/** \brief The EPSILON value for mdbx_estimate_range()
 * \ingroup c_rqest */
#define MDBX_EPSILON ((MDBX_val *)((ptrdiff_t)-1))

/** \brief Determines whether the given address is on a dirty database page of
 * the transaction or not.
 * \ingroup c_statinfo
 *
 * Ultimately, this allows to avoid copy data from non-dirty pages.
 *
 * "Dirty" pages are those that have already been changed during a write
 * transaction. Accordingly, any further changes may result in such pages being
 * overwritten. Therefore, all functions libmdbx performing changes inside the
 * database as arguments should NOT get pointers to data in those pages. In
 * turn, "not dirty" pages before modification will be copied.
 *
 * In other words, data from dirty pages must either be copied before being
 * passed as arguments for further processing or rejected at the argument
 * validation stage. Thus, `mdbx_is_dirty()` allows you to get rid of
 * unnecessary copying, and perform a more complete check of the arguments.
 *
 * \note The address passed must point to the beginning of the data. This is
 * the only way to ensure that the actual page header is physically located in
 * the same memory page, including for multi-pages with long data.
 *
 * \note In rare cases the function may return a false positive answer
 * (\ref MDBX_RESULT_TRUE when data is NOT on a dirty page), but never a false
 * negative if the arguments are correct.
 *
 * \param [in] txn      A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] ptr      The address of data to check.
 *
 * \returns A MDBX_RESULT_TRUE or MDBX_RESULT_FALSE value,
 *          otherwise the error code.
 * \retval MDBX_RESULT_TRUE    Given address is on the dirty page.
 * \retval MDBX_RESULT_FALSE   Given address is NOT on the dirty page.
 * \retval OTHERWISE the error code. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_is_dirty(const MDBX_txn *txn, const void *ptr);

/** \brief Sequence generation for a table.
 * \ingroup c_crud
 *
 * The function provides a linear sequence of unique positive integers for each table with acquire/allocate semantics.
 * The function can be called for a read transaction to retrieve the current sequence value while the increment must be
 * zero. Sequence changes become visible outside the current write transaction after it is committed, and discarded on
 * abort.
 *
 * \param [in] txn        A transaction handle returned
 *                        by \ref mdbx_txn_begin().
 * \param [in] dbi        A table handle returned by \ref mdbx_dbi_open().
 * \param [out] result    The optional address where the value of sequence
 *                        before the change will be stored.
 * \param [in] increment  Value to increase the sequence,
 *                        must be 0 for read-only transactions.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_RESULT_TRUE   Increasing the sequence has resulted in an
 *                            overflow and therefore cannot be performed. */
LIBMDBX_API int mdbx_dbi_sequence(MDBX_txn *txn, MDBX_dbi dbi, uint64_t *result, uint64_t increment);

/** \brief Compare two keys according to a particular table.
 * \ingroup c_crud
 * \see MDBX_cmp_func
 *
 * This returns a comparison as if the two data items were keys in the
 * specified table.
 *
 * \warning There is a Undefined behavior if one of arguments is invalid.
 *
 * \param [in] txn   A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi   A table handle returned by \ref mdbx_dbi_open().
 * \param [in] a     The first item to compare.
 * \param [in] b     The second item to compare.
 *
 * \returns < 0 if a < b, 0 if a == b, > 0 if a > b */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_cmp(const MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *a,
                                                    const MDBX_val *b);

/** \brief Returns default internal key's comparator for given table flags.
 * \ingroup c_extra */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API MDBX_cmp_func mdbx_get_keycmp(MDBX_db_flags_t flags);

/** \brief Compare two data items according to a particular table.
 * \ingroup c_crud
 * \see MDBX_cmp_func
 *
 * This returns a comparison as if the two items were data items of the
 * specified table.
 *
 * \warning There is a Undefined behavior if one of arguments is invalid.
 *
 * \param [in] txn   A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] dbi   A table handle returned by \ref mdbx_dbi_open().
 * \param [in] a     The first item to compare.
 * \param [in] b     The second item to compare.
 *
 * \returns < 0 if a < b, 0 if a == b, > 0 if a > b */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API int mdbx_dcmp(const MDBX_txn *txn, MDBX_dbi dbi, const MDBX_val *a,
                                                     const MDBX_val *b);

/** \brief Returns default internal data's comparator for given table flags
 * \ingroup c_extra */
MDBX_NOTHROW_CONST_FUNCTION LIBMDBX_API MDBX_cmp_func mdbx_get_datacmp(MDBX_db_flags_t flags);

/** \brief A callback function used to enumerate the reader lock table.
 * \ingroup c_statinfo
 *
 * \param [in] ctx            An arbitrary context pointer for the callback.
 * \param [in] num            The serial number during enumeration,
 *                            starting from 1.
 * \param [in] slot           The reader lock table slot number.
 * \param [in] txnid          The ID of the transaction being read,
 *                            i.e. the MVCC-snapshot number.
 * \param [in] lag            The lag from a recent MVCC-snapshot,
 *                            i.e. the number of committed write transactions
 *                            since the current read transaction started.
 * \param [in] pid            The reader process ID.
 * \param [in] thread         The reader thread ID.
 * \param [in] bytes_used     The number of last used page
 *                            in the MVCC-snapshot which being read,
 *                            i.e. database file can't be shrunk beyond this.
 * \param [in] bytes_retained The total size of the database pages that were
 *                            retired by committed write transactions after
 *                            the reader's MVCC-snapshot,
 *                            i.e. the space which would be freed after
 *                            the Reader releases the MVCC-snapshot
 *                            for reuse by completion read transaction.
 *
 * \returns < 0 on failure, >= 0 on success. \see mdbx_reader_list() */
typedef int (*MDBX_reader_list_func)(void *ctx, int num, int slot, mdbx_pid_t pid, mdbx_tid_t thread, uint64_t txnid,
                                     uint64_t lag, size_t bytes_used, size_t bytes_retained) MDBX_CXX17_NOEXCEPT;

/** \brief Enumerate the entries in the reader lock table.
 *
 * \ingroup c_statinfo
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 * \param [in] func    A \ref MDBX_reader_list_func function.
 * \param [in] ctx     An arbitrary context pointer for the enumeration
 *                     function.
 *
 * \returns A non-zero error value on failure and 0 on success,
 * or \ref MDBX_RESULT_TRUE if the reader lock table is empty. */
LIBMDBX_API int mdbx_reader_list(const MDBX_env *env, MDBX_reader_list_func func, void *ctx);

/** \brief Check for stale entries in the reader lock table.
 * \ingroup c_extra
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 * \param [out] dead   Number of stale slots that were cleared.
 *
 * \returns A non-zero error value on failure and 0 on success,
 * or \ref MDBX_RESULT_TRUE if a dead reader(s) found or mutex was recovered. */
LIBMDBX_API int mdbx_reader_check(MDBX_env *env, int *dead);

/** \brief Returns a lag of the reading for the given transaction.
 * \ingroup c_statinfo
 *
 * Returns an information for estimate how much given read-only
 * transaction is lagging relative the to actual head.
 * \deprecated Please use \ref mdbx_txn_info() instead.
 *
 * \param [in] txn       A transaction handle returned by \ref mdbx_txn_begin().
 * \param [out] percent  Percentage of page allocation in the database.
 *
 * \returns Number of transactions committed after the given was started for
 *          read, or negative value on failure. */
MDBX_DEPRECATED LIBMDBX_API int mdbx_txn_straggler(const MDBX_txn *txn, int *percent);

/** \brief Registers the current thread as a reader for the environment.
 * \ingroup c_extra
 *
 * To perform read operations without blocking, a reader slot must be assigned for each thread. However, this assignment
 * requires a short-term lock acquisition which is performed automatically. This function allows you to assign the
 * reader slot in advance and thus avoid acquiring a lock when the reading transaction starts firstly from the current
 * thread.
 *
 * \see mdbx_thread_unregister()
 *
 * \note Threads are registered automatically the first time a read transaction
 *       starts. Therefore, there is no need to use this function, except in
 *       special cases.
 *
 * \param [in] env   An environment handle returned by \ref mdbx_env_create().
 *
 * \returns A non-zero error value on failure and 0 on success,
 * or \ref MDBX_RESULT_TRUE if thread is already registered. */
LIBMDBX_API int mdbx_thread_register(const MDBX_env *env);

/** \brief Unregisters the current thread as a reader for the environment.
 * \ingroup c_extra
 *
 * To perform read operations without blocking, a reader slot must be assigned
 * for each thread. However, the assigned reader slot will remain occupied until
 * the thread ends or the environment closes. This function allows you to
 * explicitly release the assigned reader slot.
 * \see mdbx_thread_register()
 *
 * \param [in] env   An environment handle returned by \ref mdbx_env_create().
 *
 * \returns A non-zero error value on failure and 0 on success, or
 * \ref MDBX_RESULT_TRUE if thread is not registered or already unregistered. */
LIBMDBX_API int mdbx_thread_unregister(const MDBX_env *env);

/** \brief A Handle-Slow-Readers callback function to resolve database
 * full/overflow issue due to a reader(s) which prevents the old data from being
 * recycled.
 * \ingroup c_err
 *
 * Read transactions prevent reuse of pages freed by newer write transactions,
 * thus the database can grow quickly. This callback will be called when there
 * is not enough space in the database (i.e. before increasing the database size
 * or before \ref MDBX_MAP_FULL error) and thus can be used to resolve issues
 * with a "long-lived" read transactions.
 * \see mdbx_env_set_hsr()
 * \see mdbx_env_get_hsr()
 * \see mdbx_txn_park()
 * \see <a href="intro.html#long-lived-read">Long-lived read transactions</a>
 *
 * Using this callback you can choose how to resolve the situation:
 *   - abort the write transaction with an error;
 *   - wait for the read transaction(s) to complete;
 *   - notify a thread performing a long-lived read transaction
 *     and wait for an effect;
 *   - kill the thread or whole process that performs the long-lived read
 *     transaction;
 *
 * Depending on the arguments and needs, your implementation may wait,
 * terminate a process or thread that is performing a long read, or perform
 * some other action. In doing so it is important that the returned code always
 * corresponds to the performed action.
 *
 * \param [in] env     An environment handle returned by \ref mdbx_env_create().
 * \param [in] txn     The current write transaction which internally at
 *                     the \ref MDBX_MAP_FULL condition.
 * \param [in] pid     A pid of the reader process.
 * \param [in] tid     A thread_id of the reader thread.
 * \param [in] laggard An oldest read transaction number on which stalled.
 * \param [in] gap     A lag from the last committed txn.
 * \param [in] space   A space that actually become available for reuse after
 *                     this reader finished. The callback function can take
 *                     this value into account to evaluate the impact that
 *                     a long-running transaction has.
 * \param [in] retry   A retry number starting from 0.
 *                     If callback has returned 0 at least once, then at end of
 *                     current handling loop the callback function will be
 *                     called additionally with negative `retry` value to notify
 *                     about the end of loop. The callback function can use this
 *                     fact to implement timeout reset logic while waiting for
 *                     a readers.
 *
 * \returns The RETURN CODE determines the further actions libmdbx and must
 *          match the action which was executed by the callback:
 *
 * \retval -2 or less  An error condition and the reader was not killed.
 *
 * \retval -1          The callback was unable to solve the problem and
 *                     agreed on \ref MDBX_MAP_FULL error;
 *                     libmdbx should increase the database size or
 *                     return \ref MDBX_MAP_FULL error.
 *
 * \retval 0 (zero)    The callback solved the problem or just waited for
 *                     a while, libmdbx should rescan the reader lock table and
 *                     retry. This also includes a situation when corresponding
 *                     transaction terminated in normal way by
 *                     \ref mdbx_txn_abort() or \ref mdbx_txn_reset(),
 *                     and my be restarted. I.e. reader slot don't needed
 *                     to be cleaned from transaction.
 *
 * \retval 1           Transaction aborted asynchronous and reader slot
 *                     should be cleared immediately, i.e. read transaction
 *                     will not continue but \ref mdbx_txn_abort()
 *                     nor \ref mdbx_txn_reset() will be called later.
 *
 * \retval 2 or great  The reader process was terminated or killed,
 *                     and libmdbx should entirely reset reader registration.
 */
typedef int (*MDBX_hsr_func)(const MDBX_env *env, const MDBX_txn *txn, mdbx_pid_t pid, mdbx_tid_t tid, uint64_t laggard,
                             unsigned gap, size_t space, int retry) MDBX_CXX17_NOEXCEPT;

/** \brief Sets a Handle-Slow-Readers callback to resolve database full/overflow
 * issue due to a reader(s) which prevents the old data from being recycled.
 * \ingroup c_err
 *
 * The callback will only be triggered when the database is full due to a
 * reader(s) prevents the old data from being recycled.
 *
 * \see MDBX_hsr_func
 * \see mdbx_env_get_hsr()
 * \see mdbx_txn_park()
 * \see <a href="intro.html#long-lived-read">Long-lived read transactions</a>
 *
 * \param [in] env             An environment handle returned
 *                             by \ref mdbx_env_create().
 * \param [in] hsr_callback    A \ref MDBX_hsr_func function
 *                             or NULL to disable.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_set_hsr(MDBX_env *env, MDBX_hsr_func hsr_callback);

/** \brief Gets current Handle-Slow-Readers callback used to resolve database
 * full/overflow issue due to a reader(s) which prevents the old data from being
 * recycled.
 * \see MDBX_hsr_func
 * \see mdbx_env_set_hsr()
 * \see mdbx_txn_park()
 * \see <a href="intro.html#long-lived-read">Long-lived read transactions</a>
 *
 * \param [in] env   An environment handle returned by \ref mdbx_env_create().
 *
 * \returns A MDBX_hsr_func function or NULL if disabled
 *          or something wrong. */
MDBX_NOTHROW_PURE_FUNCTION LIBMDBX_API MDBX_hsr_func mdbx_env_get_hsr(const MDBX_env *env);

/** \defgroup chk Checking and Recovery
 * Basically this is internal API for `mdbx_chk` tool, etc.
 * You should avoid to use it, except some extremal special cases.
 * \ingroup c_extra
 * @{ */

/** \brief Acquires write-transaction lock.
 * Provided for custom and/or complex locking scenarios.
 * \ingroup c_extra
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_txn_lock(MDBX_env *env, bool dont_wait);

/** \brief Releases write-transaction lock.
 * Provided for custom and/or complex locking scenarios.
 * \ingroup c_extra
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_txn_unlock(MDBX_env *env);

/** \brief Open an environment instance using specific meta-page
 * for checking and recovery.
 * \ingroup c_extra
 *
 * This function mostly of internal API for `mdbx_chk` utility and subject to
 * change at any time. Do not use this function to avoid shooting your own
 * leg(s).
 *
 * \note On Windows the \ref mdbx_env_open_for_recoveryW() is recommended
 * to use. */
LIBMDBX_API int mdbx_env_open_for_recovery(MDBX_env *env, const char *pathname, unsigned target_meta, bool writeable);

#if defined(_WIN32) || defined(_WIN64) || defined(DOXYGEN)
/** \copydoc mdbx_env_open_for_recovery()
 * \ingroup c_extra
 * \note Available only on Windows.
 * \see mdbx_env_open_for_recovery() */
LIBMDBX_API int mdbx_env_open_for_recoveryW(MDBX_env *env, const wchar_t *pathname, unsigned target_meta,
                                            bool writeable);
#define mdbx_env_open_for_recoveryT(env, pathname, target_mets, writeable)                                             \
  mdbx_env_open_for_recoveryW(env, pathname, target_mets, writeable)
#else
#define mdbx_env_open_for_recoveryT(env, pathname, target_mets, writeable)                                             \
  mdbx_env_open_for_recovery(env, pathname, target_mets, writeable)
#endif /* Windows */

/** \brief Turn database to the specified meta-page.
 *
 * This function mostly of internal API for `mdbx_chk` utility and subject to
 * change at any time. Do not use this function to avoid shooting your own
 * leg(s). */
LIBMDBX_API int mdbx_env_turn_for_recovery(MDBX_env *env, unsigned target_meta);

/** \brief Gets basic information about the database without opening it.
 * \ingroup c_opening
 *
 * The purpose of the function is to obtain basic information without opening the database nor mapping data to memory,
 * which can be quite a costly action for the OS kernel. The information obtained in this way can be useful for
 * adjusting the options for working with the database before opening it, as well as in scripts, file managers and other
 * auxiliary utilities.
 *
 * \todo Добавить в API возможность установки обратного вызова для ревизии опций
 * работы с БД в процессе её открытия (при удержании блокировок).
 *
 * \param [in]  pathname  The path to the directory or database file.
 * \param [out] info      A pointer to the \ref MDBX_envinfo structure to get information.
 * \param [in] bytes      The size of the \ref MDBX_envinfo structure,
 *                        which is used to ensure ABI compatibility.
 *
 * \note Only some fields of the \ref MDBX_envinfo structure will be provided/filled-in, the values of which can be
 * obtained without mapping database files to memory and without acquiring locks: the size of the database page, the
 * geometry of the database, the size of the allocated space (the number of the last distributed page), the number of
 * the last transaction, and the boot-id stored in the corresponding meta-page.
 *
 * \warning The information received is a snapshot for the moment of the function call and can be changed at any time by
 * a process working with the database. In particular, there is no obstacle to another process deleting the database and
 * recreating it with a different page size and/or changing any other parameters.
 *
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_preopen_snapinfo(const char *pathname, MDBX_envinfo *info, size_t bytes);
#if defined(_WIN32) || defined(_WIN64) || defined(DOXYGEN)
/** \copydoc mdbx_preopen_snapinfo()
 * \ingroup c_opening
 * \note Available only on Windows.
 * \see mdbx_preopen_snapinfo() */
LIBMDBX_API int mdbx_preopen_snapinfoW(const wchar_t *pathname, MDBX_envinfo *info, size_t bytes);
#define mdbx_preopen_snapinfoT(pathname, info, bytes) mdbx_preopen_snapinfoW(pathname, info, bytes)
#else
#define mdbx_preopen_snapinfoT(pathname, info, bytes) mdbx_preopen_snapinfo(pathname, info, bytes)
#endif /* Windows */

/** \brief Flags/options for checking the integrity of a database.
 * \ingroup c_extra
 * \note This API has not been frozen yet, there may be improvements and changes in subsequent versions.
 * \see mdbx_env_chk() */
typedef enum MDBX_chk_flags {
  /** The check mode by default, including read-only mode. */
  MDBX_CHK_DEFAULTS = 0,

  /** Checking in read-write mode, with locking and suspending writing transactions. */
  MDBX_CHK_READWRITE = 1,

  /** Skip the page tree crawl. */
  MDBX_CHK_SKIP_BTREE_TRAVERSAL = 2,

  /** Skip iterating and viewing key-value records. */
  MDBX_CHK_SKIP_KV_TRAVERSAL = 4,

  /** Ignore the order of keys and values.
   * \note This options is required when checking databases created using non-standard (custom) key and/or value
   * comparison functions. */
  MDBX_CHK_IGNORE_ORDER = 8
} MDBX_chk_flags_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_chk_flags)

/** \brief Levels of logging/detailing of information supplied via callbacks during a database integrity check.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
typedef enum MDBX_chk_severity {
  MDBX_chk_severity_prio_shift = 4,
  MDBX_chk_severity_kind_mask = 0xF,
  MDBX_chk_fatal = 0x00u,
  MDBX_chk_error = 0x11u,
  MDBX_chk_warning = 0x22u,
  MDBX_chk_notice = 0x33u,
  MDBX_chk_result = 0x44u,
  MDBX_chk_resolution = 0x55u,
  MDBX_chk_processing = 0x56u,
  MDBX_chk_info = 0x67u,
  MDBX_chk_verbose = 0x78u,
  MDBX_chk_details = 0x89u,
  MDBX_chk_extra = 0x9Au
} MDBX_chk_severity_t;

/** \brief The verification stages reported via callbacks during a database integrity check.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
typedef enum MDBX_chk_stage {
  MDBX_chk_none,
  MDBX_chk_init,
  MDBX_chk_lock,
  MDBX_chk_meta,
  MDBX_chk_tree,
  MDBX_chk_gc,
  MDBX_chk_space,
  MDBX_chk_maindb,
  MDBX_chk_tables,
  MDBX_chk_conclude,
  MDBX_chk_unlock,
  MDBX_chk_finalize
} MDBX_chk_stage_t;

/** \brief A virtual row of the report generated during a database integrity check.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
typedef struct MDBX_chk_line {
  struct MDBX_chk_context *ctx;
  uint8_t severity, scope_depth, empty;
  char *begin, *end, *out;
} MDBX_chk_line_t;

/** \brief An issue problem was discovered during a database integrity check.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
typedef struct MDBX_chk_issue {
  struct MDBX_chk_issue *next;
  size_t count;
  const char *caption;
} MDBX_chk_issue_t;

/** \brief A hierarchical context during a database integrity check.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
typedef struct MDBX_chk_scope {
  MDBX_chk_issue_t *issues;
  struct MDBX_chk_internal *internal;
  const void *object;
  MDBX_chk_stage_t stage;
  MDBX_chk_severity_t verbosity;
  size_t subtotal_issues;
  union {
    void *ptr;
    size_t number;
  } usr_z, usr_v, usr_o;
} MDBX_chk_scope_t;

/** \brief A custom type for binding additional data associated with a certain key-value table
 *   during a database integrity check.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
typedef struct MDBX_chk_user_table_cookie MDBX_chk_user_table_cookie_t;

/** \brief A histogram with some statistical information collected during a database integrity check.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
struct MDBX_chk_histogram {
  size_t amount, count, le1_amount, le1_count;
  struct {
    size_t begin, end, amount, count;
  } ranges[9];
};

/** \brief Information about a certain key-value table during a database integrity check.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
typedef struct MDBX_chk_table {
  MDBX_chk_user_table_cookie_t *cookie;

/** \brief Pseudo-name for MainDB */
#define MDBX_CHK_MAIN ((void *)((ptrdiff_t)0))
/** \brief Pseudo-name for GarbageCollectorDB */
#define MDBX_CHK_GC ((void *)((ptrdiff_t)-1))
/** \brief Pseudo-name for MetaPages */
#define MDBX_CHK_META ((void *)((ptrdiff_t)-2))

  MDBX_val name;
  MDBX_db_flags_t flags;
  int id;

  size_t payload_bytes, lost_bytes;
  struct {
    size_t all, empty, broken;
    size_t branch, leaf;
    size_t nested_branch, nested_leaf, nested_subleaf;
  } pages;
  struct {
    /** Tree deep histogram */
    struct MDBX_chk_histogram height;
    /** Histogram of large/overflow pages length */
    struct MDBX_chk_histogram large_pages;
    /** Histogram of nested trees height, span length for GC */
    struct MDBX_chk_histogram nested_height_or_gc_span_length;
    /** Keys length histogram */
    struct MDBX_chk_histogram key_len;
    /** Values length histogram */
    struct MDBX_chk_histogram val_len;
    /** Number of multi-values (aka duplicates) histogram */
    struct MDBX_chk_histogram multival;
    /** Histogram of branch and leaf pages filling in percents */
    struct MDBX_chk_histogram tree_density;
    /** Histogram of nested tree(s) branch and leaf pages filling in percents */
    struct MDBX_chk_histogram large_or_nested_density;
    /** Histogram of pages age */
    struct MDBX_chk_histogram page_age;
    /** Histogram of used pgno */
    struct MDBX_chk_histogram pgno;
  } histogram;
} MDBX_chk_table_t;

/** \brief The context for checking the integrity of a database.
 * \ingroup c_extra
 * \see mdbx_env_chk() */
typedef struct MDBX_chk_context {
  struct MDBX_chk_internal *internal;
  MDBX_env *env;
  MDBX_txn *txn;
  MDBX_chk_scope_t *scope;
  uint8_t scope_nesting;
  struct {
    size_t total_payload_bytes;
    size_t table_total, table_processed;
    size_t total_unused_bytes, unused_pages;
    size_t processed_pages, reclaimable_pages, gc_pages, alloc_pages, backed_pages;
    size_t problems_meta, tree_problems, gc_tree_problems, kv_tree_problems, problems_gc, problems_kv, total_problems;
    uint64_t steady_txnid, recent_txnid;
    /** Histogram of pages age */
    struct MDBX_chk_histogram histogram_page_age;
    /** Histogram of pgno retained by readers */
    struct MDBX_chk_histogram histogram_pgno_payload;
    /** Histogram of pgno used by all payload */
    struct MDBX_chk_histogram histogram_pgno_retained;
    /** A pointer to the array of `table_total` pointers to instances of \ref MDBX_chk_table_t structures with
     * information about all key-value tables, including `MainDB` and `GC`. */
    const MDBX_chk_table_t *const *tables;
  } result;
} MDBX_chk_context_t;

/** \brief A set of callback functions used for checking the integrity of a database.
 * \ingroup c_extra
 *
 * \details The callback functions are designed to organize interaction with the application code. This includes the
 * integration of application logic that verifies the integrity of a data structure above the key-value level, the
 * preparation and structured output of information about both the progress and the results of verification.
 *
 * All callback functions are optional, unused ones must be set to `nullptr`.
 *
 * \note This API has not been frozen yet, and there may be improvements and changes in subsequent versions.
 *
 * \see mdbx_env_chk() */
typedef struct MDBX_chk_callbacks {
  bool (*check_break)(MDBX_chk_context_t *ctx);
  int (*scope_push)(MDBX_chk_context_t *ctx, MDBX_chk_scope_t *outer, MDBX_chk_scope_t *inner, const char *fmt,
                    va_list args);
  int (*scope_conclude)(MDBX_chk_context_t *ctx, MDBX_chk_scope_t *outer, MDBX_chk_scope_t *inner, int err);
  void (*scope_pop)(MDBX_chk_context_t *ctx, MDBX_chk_scope_t *outer, MDBX_chk_scope_t *inner);
  void (*issue)(MDBX_chk_context_t *ctx, const char *object, uint64_t entry_number, const char *issue,
                const char *extra_fmt, va_list extra_args);
  MDBX_chk_user_table_cookie_t *(*table_filter)(MDBX_chk_context_t *ctx, const MDBX_val *name, MDBX_db_flags_t flags);
  int (*table_conclude)(MDBX_chk_context_t *ctx, const MDBX_chk_table_t *table, MDBX_cursor *cursor, int err);
  void (*table_dispose)(MDBX_chk_context_t *ctx, const MDBX_chk_table_t *table);

  int (*table_handle_kv)(MDBX_chk_context_t *ctx, const MDBX_chk_table_t *table, size_t entry_number,
                         const MDBX_val *key, const MDBX_val *value);

  int (*stage_begin)(MDBX_chk_context_t *ctx, MDBX_chk_stage_t);
  int (*stage_end)(MDBX_chk_context_t *ctx, MDBX_chk_stage_t, int err);

  MDBX_chk_line_t *(*print_begin)(MDBX_chk_context_t *ctx, MDBX_chk_severity_t severity);
  void (*print_flush)(MDBX_chk_line_t *);
  void (*print_done)(MDBX_chk_line_t *);
  void (*print_chars)(MDBX_chk_line_t *, const char *str, size_t len);
  void (*print_format)(MDBX_chk_line_t *, const char *fmt, va_list args);
  void (*print_size)(MDBX_chk_line_t *, const char *prefix, const uint64_t value, const char *suffix);
} MDBX_chk_callbacks_t;

/** \brief Checks the integrity of a database.
 * \ingroup c_extra
 *
 * \details Interaction with the application code is implemented through callback functions provided by the application
 * using the `cb` parameter. During such interaction, the application can monitor the verification process, including
 * skipping/filtering the processing of individual elements, as well as implement additional verification of the
 * structure and/or information, taking into account the purpose and semantic significance for an application. For
 * example, an application can check its own indexes and the correctness of database entries. It is for this purpose
 * that the integrity check functionality has been improved for intensive use of callbacks and moved from the `mdbx_chk`
 * utility to the main library.
 *
 * Verification is performed in several stages, starting with initialization and ending with finalization. For more
 * details, see \ref MDBX_chk_stage_t. The application code is notified about the beginning and end of each stage
 * through the corresponding callback functions. For more details, see \ref MDBX_chk_callbacks_t.
 *
 * \param [in] env        A pointer to an instance of environment.
 * \param [in] cb         A set of callback functions.
 * \param [in,out] ctx    The context of a database integrity check, where the results of the check will be generated.
 * \param [in] flags      Flags/options for checking database integrity.
 * \param [in] verbosity  The required level of detail of information about the progress and results of the checking.
 * \param [in] timeout_seconds_16dot16  The duration limit for performing the check in 1/65536 fractions of a second,
 *                                      either 0 means no limit.
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_chk(MDBX_env *env, const MDBX_chk_callbacks_t *cb, MDBX_chk_context_t *ctx,
                             const MDBX_chk_flags_t flags, MDBX_chk_severity_t verbosity,
                             unsigned timeout_seconds_16dot16);

/** \brief An auxiliary function to account issues detected by an application, including those coming to an application
 * through logging.
 * \ingroup c_extra
 *
 * \details An application should call this function to account for detected issues, or vice versa, do not make these
 * calls to ignore discovered issues.
 *
 * \see mdbx_env_chk()
 * \see MDBX_debug_func
 * \returns A non-zero error value on failure and 0 on success. */
LIBMDBX_API int mdbx_env_chk_encount_problem(MDBX_chk_context_t *ctx);

/** \brief An auxiliary function for converting fractions to string of decimal digits without using floating-point
 * operations.
 * \ingroup c_extra
 *
 * \note The accuracy of the conversion result is limited both by the simplicity of the algorithms and by 64-bit
 * arithmetic.
 *
 * \returns A pointer to the beginning of the string with the result of conversion.*/
LIBMDBX_API const char *mdbx_ratio2digits(uint64_t numerator, uint64_t denominator, int precision, char *buffer,
                                          size_t buffer_size);

/** \brief An auxiliary function for converting fractions to percentage string without using floating-point operations.
 * \ingroup c_extra
 *
 * \note The accuracy of the conversion result is limited both by the simplicity of the algorithms and by 64-bit
 * arithmetic.
 *
 * \returns A pointer to the beginning of the string with the result of conversion.*/
LIBMDBX_API const char *mdbx_ratio2percents(uint64_t value, uint64_t whole, char *buffer, size_t buffer_size);

/** end of chk @} */

/** \brief A callback function for iterating GC entries.
 * \ingroup c_statinfo
 * \see mdbx_gc_info()
 *
 * The callback function is called for each sequence of an adjacent pages inside GC, including single pages.
 * \note This API has not been frozen yet, and there may be improvements and changes in subsequent versions.
 *
 * \param [in] ctx                 A pointer to the context passed by a similar parameter in \ref mdbx_gc_info().
 * \param [in] txn                 A transaction handle returned by \ref mdbx_txn_begin().
 * \param [in] span_txnid          A transaction ID and the same as MVCC-snapshot number
 *                                 of which the span is associated in reclaiming order.
 * \param [in] span_pgno           The starting page number of a span.
 * \param [in] span_length         The number of pages in a span, it is 1 for a single pages.
 * \param [in] span_is_reclaimable A boolean flag indicates the span is reclaimable for now either no.
 *
 * \returns Zero if an enumeration step is successful and should be continues,
 * if another value is returned, it will be immediately returned to the caller without continuing an enumeration. */
typedef int (*MDBX_gc_iter_func)(void *ctx, const MDBX_txn *txn, uint64_t span_txnid, size_t span_pgno,
                                 size_t span_length, bool span_is_reclaimable) MDBX_CXX17_NOEXCEPT;

/** \brief Information about Garbage Collection and page usage.
 * \ingroup c_statinfo
 * \see mdbx_gc_info */
typedef struct MDBX_gc_info {
  size_t pages_total;     /**< Total number of pages in a database, i.e. the upper limit defined by geometry */
  size_t pages_backed;    /**< Number of pages currently backed by a database file */
  size_t pages_allocated; /**< Number of pages currently allocated */
  size_t pages_gc;        /**< Number of all pages within GC,
                           * includes a pages formes the B-tree structure of GC itself */
  struct {
    size_t pages; /**< Number of reclaimable pages, including pending reserve within a current write transaction */
    struct MDBX_chk_histogram
        span_histogram; /**< Histogram of the spans length of a sequence(s) adjacent reclaimable pages */
    struct MDBX_chk_histogram pgno_distribution; /**< Distribution of a reclaimable pages over the file of a database */
  } gc_reclaimable;
  size_t max_reader_lag;
  size_t max_retained_pages;
} MDBX_gc_info_t;

/** \brief Provides information of Garbage Collection and page usage.
 *
 * \details Scans the whole GC to summarise information of GC state and page page usage for given transaction.
 * During this optionaly iterages GC entries by calling a user-specified visitor function for each span of
 * pages inside GC, excepting a pages formes the B-tree structure of GC itself. Such iteration continues until the GC
 * items are exhausted, or until a result other than zero is returned from a user-defined callback function, which will
 * be returned immediately as a result.
 *
 * \note This API has not been frozen yet, and there may be improvements and changes in subsequent versions.
 *
 * \ingroup c_statinfo
 * \see MDBX_gc_info_t
 * \see MDBX_gc_iter_func
 *
 * \param [in] txn          A transaction started by \ref mdbx_txn_begin().
 *
 * \param [out] info        The address of a \ref MDBX_gc_info_t structure
 *                          where the information will be provided.
 *
 * \param [in] bytes        The actual size of \ref MDBX_gc_info_t,
 *                          this value is used to provide ABI compatibility.
 *
 * \param [in] iter_func    A custom callback function with the signature \ref MDBX_gc_iter_func,
 *                          which will be called for each span.
 *
 * \param [in] iter_ctx     A pointer to some context that will be passed to the `iter_func()` function as it is.
 *
 * \returns A non-zero error value on failure and 0 on success,
 *          some possible errors are:
 * \retval MDBX_EINVAL    An invalid parameter was specified.
 * \retval MDBX_NOTFOUND  The GC is empty for now. */
LIBMDBX_API int mdbx_gc_info(MDBX_txn *txn, MDBX_gc_info_t *info, size_t bytes, MDBX_gc_iter_func iter_func,
                             void *iter_ctx);

/** \brief The returned reasons for stopping database defragmentation.
 * \details Any number of individual values could be OR'ed together while while returning actual set of reasons.
 * \ingroup c_extra
 * \see MDBX_defrag_result_t
 * \see mdbx_env_defrag() */
typedef enum MDBX_defrag_stopping_reasons {
  MDBX_defrag_noobstacles = 0,
  MDBX_defrag_step_size = 1,         /**< Step transaction size limit reached */
  MDBX_defrag_large_chunk = 2,       /**< Preliminary movement is necessary to form
                                          a sufficient sequence of adjacent free pages in order to
                                          then move the revealed Large/Overflow page on a next cycle */
  MDBX_defrag_discontinued = 4,      /**< Discontinued by user */
  MDBX_defrag_laggard_reader = 8,    /**< At least one process performing a reading transaction
                                          prevents further defragmentation */
  MDBX_defrag_enough_threshold = 16, /**< The defragmentation goal set by the user has been achieved */
  MDBX_defrag_time_limit = 32,       /**< The specified limit on the duration of defragmentation has been reached */
  MDBX_defrag_aborted = 64,          /**< Aborted by user */
  MDBX_defrag_error = 128            /**< An error occurred during defragmentation */
} MDBX_defrag_stopping_reasons_t;
DEFINE_ENUM_FLAG_OPERATORS(MDBX_defrag_stopping_reasons)

/** \brief The numerical metrics of progress and result of database defragmentation.
 * \ingroup c_extra
 * \see mdbx_env_defrag()
 * \see MDBX_defrag_notify_func */
typedef struct MDBX_defrag_result {
  /** The number of pages that the database size was shrinked by. In the worst case, this value can be negative, for
   * instance -1, when defragmentation was stopped for some reason, or the database structure does not allow it to be
   * defragmented by moving individual pages. */
  intptr_t pages_shrinked;

  /** The total number of pages moved during defragmentation. */
  size_t pages_moved;

  /** The number of pages scheduled to be moved at the next stage of the current defragmentation cycle. */
  size_t pages_scheduled;

  /** The number of pages held by other processes via reading MVCC-snapshots that prevent
   *  reclaiming and defragmentation. */
  size_t pages_retained;

  /** The estimated remaining number of pages that are potentially defragmented. */
  size_t pages_left;

  /** The whole number of pages in the database. */
  size_t pages_whole;

  /** The number of the page where the defragmentation stumbled,
   *  according to the reasons given in the `stopping_reasons` field. */
  size_t obstructed_pgno;

  /** The length of the large/overflow-page span where the defragmentation stumbled,
   *  according to the reasons given in the `stopping_reasons` field. */
  size_t obstructed_span;

  /** The transaction number corresponds to the earliest/first MVCC-snapshot
   *  held by reader(s) and preventing defragmentation. */
  uint64_t obstructed_txnid;
  /** The system/native Thread ID of one of a readers holding the MVCC snapshot
   *  that prevents defragmentation. */
  mdbx_tid_t obstructor_tid;
  /** The system/native Process ID of one of a readers holding the MVCC snapshot
   *  that prevents defragmentation. */
  mdbx_pid_t obstructor_pid;

  /** Rough estimation a progress of the current defragmentation cycle
   * in permilles (the 1000 means 100%). */
  unsigned rough_estimation_cycle_progress_permille;

  /** The number of defragmentation cycles. */
  unsigned cycles;

  /** Obstacles and reasons for stopping defragmentation in the form of
   * a mask of OR'ed \ref MDBX_defrag_stopping_reasons_t bits. */
  unsigned stopping_reasons;

  /** The time elapsed since the beginning of defragmentation in a 1/65536 second fractions. */
  size_t spent_time_dot16;
} MDBX_defrag_result_t;

/** \brief A callback function to notify an application about the progress of defragmentation.
 * \ingroup c_extra
 * \see mdbx_env_defrag()
 * \details If provided such callback will be called time-to-time to notify about the progress of defragmentation.
 * The rate of such notification calls is not explicitly defined, but it is guaranteed that it will be called at the
 * beginning and end of each defragmentation cycle, as well as often will be enough to track progress in a percentages
 * sharp.
 *
 * \param [in] ctx       A pointer to the context passed by a similar parameter in \ref mdbx_env_defrag().
 *
 * \param [in] progress  A pointer to the \ref MDBX_defrag_result_t structure filled in to reflects
 *                       the current state of database defragmentation.
 *
 * \returns A signed integer value that allows you to control the continuation of defragmentation:
 * \retval    0  To continue defragmentation.
 * \retval   -1  To abort defragmentation immediately,
 *               see \ref MDBX_defrag_aborted.
 * \retval    1  To discontinue defragmentation with completion scheduled operations,
 *               see \ref MDBX_defrag_discontinued. */
typedef int (*MDBX_defrag_notify_func)(void *ctx, const MDBX_defrag_result_t *progress) MDBX_CXX17_NOEXCEPT;

/** \brief Performs database defragmentation.
 * \ingroup c_extra
 * \see MDBX_defrag_notify_func
 * \see MDBX_defrag_result_t
 *
 * \details Defragmentation is the transfer of data from pages located at the end of the database to free pages closer
 * to the beginning. After that, the pages that have become unused at the end of the database can be cut off while
 * reducing the size of the database file. This function performs all the described actions in comply with ACID, trying
 * to minimize the number of operations for moving data and writing to media.
 *
 * The function accepts an extended set of parameters that allow you to fully control the goals and progress of
 * defragmentation, including as to quickly get a minimum result by small steps and as to perform a fairly complete
 * defragmentation in the least number of large cycles.
 *
 * \note Any parallel reading transactions do not make defragmentation impossible, but ones limit it to single cycle
 * and will definitely disallow to be performed completely.
 *
 * During the movement of data in the b-tree structure, it is necessary to adjust the links in the parent pages to the
 * new moved child pages. According to the MVCC concept, this requires creating copies of altered parent pages along the
 * entire chain from leaves to the root of the b-tree, inclusive. In addition, to move large/overflow pages, it may be
 * necessary to form sequences of adjacent free pages, which will require moving other pages with data and correcting
 * links to ones. Defragmentation almost always cannot be completely completed in one pass, as there are always fewer
 * free pages than necessary to move due to the need to copy all the parent pages. In addition, if there are not enough
 * sequences to move large/overflow pages, an additional commit step is required and a new transaction is started to
 * continue.
 *
 * Thus, defragmentation is almost always performed in several cycles, each of which ends with the transaction being
 * committed and can be interrupted in any way while ensuring the durability of the database specified when it was
 * opened.
 *
 * \param [in] env                   A pointer to an instance of environment.
 *
 * \param [in] defrag_atleast        The required at least number of pages by which the database
 *                                   must be reduced as a result of defragmentation.
 *                                   Defragmentation will not be completed and its goals will not be
 *                                   considered achieved until the database is shrinked by the specified amount.
 *                                   Must be less or equal to `defrag_enough`.
 *                                   Specify zero if in doubt or not known, this will mean no lower bound.
 *
 * \param [in] time_atleast_dot16    The time by a wall clock in 1/65536 unit of second that should be spent to
 *                                   defragment more, even if the goals given via other parameters have already
 *                                   been reached. Must be less or equal to `time_limit_16dot16`.
 *                                   Specify zero if in doubt or not known, this will mean no lower bound.
 *
 * \param [in] defrag_enough         The number of pages by which it will be enough to shrink the database during
 *                                   a defragmentation process to finish it rather than dig more.
 *                                   Must be greater or equal to `defrag_atleast`.
 *                                   Specify zero if in doubt or not known, this will mean no limit.
 *
 * \param [in] time_limit_dot16      The time limit by a wall clock in 1/65536 unit of second that could be spent to
 *                                   defragment. When the specified limit is reached, defragmentation will not
 *                                   continue, but the stage of writing the moved pages that has already begun
 *                                   will be completed. Must be greater or equal to `time_atleast_dot16`.
 *                                   Specify zero if in doubt or not known, this will mean no limit.
 *
 * \param [in] acceptable_backlash   Defragmentation stops if a next cycle will unable to shrink database by the
 *                                   number of pages more than the specified value. This avoids the last few
 *                                   defragmentation cycles, which do not significantly reduce the size of the database.
 *                                   Specify `-1` if in doubt or not known, this will mean autopilot.
 *
 * \param [in] preferred_batch       The preferred maximum number of pages to be moved per defragmentation cycle.
 *                                   Small batches take less time, so if necessary, defragmentation could be stopped
 *                                   faster without losing the intermediate result. On the other hand, smaller batches
 *                                   will require more transaction commits and more page rewrites to achieve a similar
 *                                   result. Specify 0 if in doubt or not known, this will mean no limit.
 *
 * \param [in] progress_callback     An optional custom progress notification callback function with the signature
 *                                   \ref MDBX_defrag_notify_func, which will be called time-to-time to notify about
 *                                   the progress of defragmentation.
 *                                   The rate of calls to the provided function is not explicitly defined,
 *                                   but it is guaranteed that it will be called at the beginning and end of each
 *                                   defragmentation cycle, as well as often enough to track progress.
 *                                   Specify nullptr if in doubt or not known, this will mean unused.
 *
 * \param [in] ctx                   An optional pointer to some context that will be passed to the
 *                                   `progress_callback()` function as it is.
 *                                   Specify nullptr if in doubt or not known, this will mean unused.
 *
 * \param [out] result               An optional address of a \ref MDBX_defrag_result_t structure where the information
 *                                   of defragmentation results will be provided.
 *                                   Specify nullptr if in doubt or not known, this will mean a result is not needed.
 *
 * \returns A non-zero error value on failure and 0 on success, some possible errors are:
 * \retval MDBX_EINVAL          An invalid parameter was specified.
 * \retval MDBX_LAGGARD_READER  One or more readers use old MVCC-snapshots of data and thus
 *                              do not allow defragmentation to be completed.
 * \retval MDBX_RESULT_TRUE     It was not possible to complete defragmentation or achieve the goals
 *                              specified by the parameters due to the given limits or other obstacles,
 *                              that can be knew from the \ref MDBX_defrag_result_t structure. */
LIBMDBX_API int mdbx_env_defrag(MDBX_env *env, size_t defrag_atleast, size_t time_atleast_dot16, size_t defrag_enough,
                                size_t time_limit_dot16, intptr_t acceptable_backlash, intptr_t preferred_batch,
                                MDBX_defrag_notify_func progress_callback, void *ctx, MDBX_defrag_result_t *result);

/** end of c_api @} */

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* LIBMDBX_H */
