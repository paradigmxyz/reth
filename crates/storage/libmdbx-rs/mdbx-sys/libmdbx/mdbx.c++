/* This file is part of the libmdbx amalgamated source code (v0.14.2-8-gcfb319f8 at 2026-06-08T23:38:47+03:00).
 *
 * libmdbx (aka MDBX) is an extremely fast, compact, powerful, embeddedable, transactional key-value storage engine with
 * open-source code. MDBX has a specific set of properties and capabilities, focused on creating unique lightweight
 * solutions.  Please visit https://libmdbx.dqdkfa.ru for more information, changelog, documentation, C++ API description
 * and links to the original git repo with the source code.  Questions, feedback and suggestions are welcome to the
 * Telegram' group https://t.me/libmdbx, MAX' chat https://max.ru/join/dKckvyuARxp1vRK-wnPur8zYCEkbR3OUOmpPWkWxp78.
 *
 * The libmdbx code will forever remain open and with high-quality free support, as far as the life circumstances of the
 * project participants allow. Donations are welcome to ETH `0xD104d8f8B2dC312aaD74899F83EBf3EEBDC1EA3A`,
 * BTC `bc1qzvl9uegf2ea6cwlytnanrscyv8snwsvrc0xfsu`, SOL `FTCTgbHajoLVZGr8aEFWMzx3NDMyS5wXJgfeMTmJznRi`.
 * Всё будет хорошо!
 *
 * For ease of use and to eliminate potential limitations in both distribution and obstacles in technology development,
 * libmdbx is distributed as an amalgamated source code starting at the end of 2025.  The source code of the tests, as
 * well as the internal documentation, will be available only to the team directly involved in the development.
 *
 * Copyright 2015-2026 Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru>
 * SPDX-License-Identifier: Apache-2.0
 *
 * For notes about the license change, credits and acknowledgments, please refer to the COPYRIGHT file. */

/* clang-format off */

/*----------------------------------------------------------------------------*/

#include "mdbx-internals.h"

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
typedef struct page_cache_entry page_cache_entry_t;

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

typedef struct page_ref {
  page_t *page;
  page_cache_entry_t *cache;
  pgno_t pgno;
  size_t npages;
  unsigned flags;
} page_ref_t;

typedef struct page_get_result {
  page_t *page;
  int err;
  page_ref_t ref;
} pgr_t;

enum page_ref_flags {
  PAGE_REF_NONE = 0,
  PAGE_REF_TXN_DIRTY = 1u << 0,
  PAGE_REF_OWNED = 1u << 1,
  PAGE_REF_CACHE = 1u << 2
};

typedef struct page_cache {
  page_cache_entry_t *entries;
  size_t entries_count;
  size_t pages;
  size_t bytes;
  size_t pinned;
} page_cache_t;

struct page_cache_entry {
  page_cache_entry_t *next;
  page_cache_t *owner;
  struct dxb_storage *storage;
  page_t *page;
  txnid_t snapshot_txnid;
  pgno_t pgno;
  size_t npages;
  size_t bytes;
  size_t pins;
  uint8_t pagesize_ln;
  bool reusable;
};

typedef struct bind_reader_slot_result {
  int err;
  reader_slot_t *slot;
} bsr_t;

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

/* List of txnid */
typedef txnid_t *txl_t;
typedef const txnid_t *const_txl_t;

enum txl_rules {
  txl_granulate = 32,
  txl_initial = txl_granulate - 2 - MDBX_ASSUME_MALLOC_OVERHEAD / sizeof(txnid_t),
  txl_max = (1u << 26) - 2 - MDBX_ASSUME_MALLOC_OVERHEAD / sizeof(txnid_t)
};

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
  ASSERT((uintptr_t)ptr % expected_alignment == 0);
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
  ASSERT((uintptr_t)ptr % expected_alignment == 0);
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
  ASSERT((uintptr_t)ptr % expected_alignment == 0);
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
  ASSERT((uintptr_t)ptr % expected_alignment == 0);
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
  ASSERT((uintptr_t)ptr % expected_alignment == 0);
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
  ASSERT((uintptr_t)ptr % expected_alignment == 0);
  ASSERT(expected_alignment % sizeof(uint32_t) == 0);
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
  ASSERT((uintptr_t)ptr % expected_alignment == 0);
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
  pgno_t pgno;
#if MDBX_DPL_CACHE_NPAGES
  pgno_t npages;
#endif /* MDBX_DPL_CACHE_NPAGES */
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

typedef struct search_foliage_result {
  node_t *node;
  bool exact;
} sfr_t;

typedef size_t (*MDBX_search_branch)(const MDBX_cursor *mc, const MDBX_val *key);
typedef sfr_t (*MDBX_search_foliage)(MDBX_cursor *mc, const MDBX_val *key);

/* Comparing/ordering and length constraints */
typedef struct clc {
  MDBX_cmp_func cmp; /* comparator */
  size_t lmin, lmax; /* min/max length constraints */
  MDBX_search_branch search_branch;
  MDBX_search_foliage search_foliage;
  void *reserve_node_add;
  void *reserve_node_del;
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
 *  - размер kvx_t становится равным 8 словам (16 словам после добавление функций поиска и т.п).
 *
 * Трюки и прочая экономия на спичках:
 *  - не храним dbi внутри курсора, вместо этого вычисляем его как разницу между
 *    dbi_state курсора и началом таблицы dbi_state в транзакции. Смысл тут в
 *    экономии кол-ва полей при инициализации курсора. Затрат это не создает,
 *    так как dbi требуется для последующего доступа к массивам в транзакции,
 *    т.е. при вычислении dbi разыменовывается тот-же указатель на txn
 *    и читается та же кэш-линия с указателями. */
typedef struct clc_couple {
  clc_t k; /* для ключей */
  clc_t v; /* для значений */
} clc_couple_t;

struct kvx {
  clc_couple_t clc;
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
  txn_ro_flat = MDBX_TXN_RDONLY,
  txn_ro_nested = UINT32_C(0x0800),
  txn_ro_both = txn_ro_flat | txn_ro_nested,
  txn_ro_begin_flags = MDBX_TXN_RDONLY | MDBX_TXN_RDONLY_PREPARE,
  txn_rw_begin_flags = MDBX_TXN_NOMETASYNC | MDBX_TXN_NOSYNC | MDBX_TXN_TRY,
  txn_rw_already_locked = MDBX_TXN_RDONLY_PREPARE & ~MDBX_TXN_RDONLY,
  txn_shrink_allowed = UINT32_C(0x40000000),
  txn_parked = MDBX_TXN_PARKED,
  txn_gc_drained = 0x100 /* GC was depleted up to oldest reader */,
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

#if MDBX_ENABLE_PGET_STAT
  /* Counter of page get operations */
  uint64_t ops_pget;
#endif /* MDBX_ENABLE_PGET_STAT */

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
      void *preserve_parent_userctx;
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
  clc_couple_t *clc;
  subcur_t *__restrict subcur;
  page_t *pg[CURSOR_STACK_SIZE]; /* stack of pushed pages */
  page_ref_t pgref[CURSOR_STACK_SIZE];
  page_ref_t value_ref;          /* page ref backing the latest returned non-stack value */
  indx_t ki[CURSOR_STACK_SIZE];  /* stack of page indices */
  MDBX_cursor *next;
  /* Состояние на момент старта вложенной транзакции */
  MDBX_cursor *backup;
#ifndef MDBX_DEBUG_SEARCH_DISPATCHING
#define MDBX_DEBUG_SEARCH_DISPATCHING MDBX_DEBUG
#endif /* MDBX_DEBUG_SEARCH_DISPATCHING */

#if MDBX_DEBUG_SEARCH_DISPATCHING
  unsigned search_step_counter;
#define MDBX_CURSOR_STC_INC(cursor)                                                                                    \
  do                                                                                                                   \
    ((MDBX_cursor *)(cursor))->search_step_counter += 1;                                                               \
  while (0)
#define MDBX_CURSOR_STC_GET(cursor) ((cursor)->search_step_counter)
#else
#define MDBX_CURSOR_STC_INC(cursor) __noop
#define MDBX_CURSOR_STC_GET(cursor) (0)
#endif /* MDBX_DEBUG_SEARCH_DISPATCHING */
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

typedef struct dxb_storage {
  mdbx_filehandle_t data_fd;
  mdbx_filehandle_t meta_fd;
  mdbx_filehandle_t dsync_fd;
  osal_ioring_t ioring;
  page_cache_t page_cache;
  size_t page_cache_limit;
  osal_fastmutex_t page_cache_lock;
  bool page_cache_lock_initialized;
  uint8_t pagesize_ln;
  uint64_t filesize;
  size_t current;
  size_t limit;
} dxb_storage_t;

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
  dxb_storage_t dxb_storage;
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
  uint8_t ps2ln;              /* log2 of DB page size */
  int8_t stuck_meta;          /* recovery-only: target meta page or less that zero */
  uint16_t merge_threshold;   /* pages emptier than this are candidates for merging */
  unsigned max_readers;       /* size of the reader table */
  MDBX_dbi max_dbi;           /* size of the DB table */
  mdbx_pid_t pid;             /* process ID of this env */
  osal_thread_key_t me_txkey; /* thread-key for readers */
  struct {                    /* path to the DB files */
    pathchar_t *lck, *dxb, *specified;
    void *buffer;
  } pathname;
  void *page_auxbuf;              /* scratch area for DUPSORT put() */
  MDBX_txn *basal_txn;            /* preallocated write transaction */
  void *meta_shadow;              /* explicit-I/O copy of the three meta pages */
  size_t meta_shadow_bytes;
  kvx_t *kvs;                     /* array of auxiliary key-value properties */
  uint8_t *__restrict dbs_flags;  /* array of flags from tree_t.flags */
  mdbx_atomic_uint32_t *dbi_seqs; /* array of dbi sequence numbers */
  unsigned maxgc_large1page;      /* Number of pgno_t fit in a single large page */
  unsigned maxgc_per_branch;
  mdbx_pid_t registered_reader_pid; /* have liveness lock in reader table */
  void *userctx;                    /* User-settable context */
  MDBX_hsr_func hsr_callback;       /* Callback for kicking laggard readers */
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
    uint16_t merge_threshold_dot16;
    uint16_t split_reserve_dot16;
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

#if defined(_WIN32) || defined(_WIN64)
  osal_srwlock_t remap_lock;
  /* Workaround for LockFileEx and WriteFile multithread bug */
  CRITICAL_SECTION lck_event_cs;
  CRITICAL_SECTION dxb_event_cs;
  char *pathname_char; /* cache of multi-byte representation of pathname
                             to the DB files */
#else
  osal_fastmutex_t remap_lock;
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
  STATIC_ASSERT(offsetof(lck_t, cached_oldest_txnid) % MDBX_CACHELINE_SIZE == 0);
  STATIC_ASSERT(offsetof(lck_t, rdt_length) % MDBX_CACHELINE_SIZE == 0);
#endif /* MDBX_LOCKING */
#if FLEXIBLE_ARRAY_MEMBERS
  STATIC_ASSERT(offsetof(lck_t, rdt) % MDBX_CACHELINE_SIZE == 0);
#endif /* FLEXIBLE_ARRAY_MEMBERS */

#if FLEXIBLE_ARRAY_MEMBERS
  STATIC_ASSERT(NODESIZE == offsetof(node_t, payload));
  STATIC_ASSERT(PAGEHDRSZ == offsetof(page_t, entries));
#endif /* FLEXIBLE_ARRAY_MEMBERS */
  STATIC_ASSERT(sizeof(clc_t) == 7 * sizeof(void *));
  STATIC_ASSERT(sizeof(kvx_t) == 16 * sizeof(void *));

#define KVX_SIZE_LN2 (MDBX_WORDBITS_LN2 + 1)
  STATIC_ASSERT(sizeof(kvx_t) == (1u << KVX_SIZE_LN2));
}
#endif /* Disabled for MSVC 19.0 (VisualStudio 2015) */

/******************************************************************************/

#ifdef _MSC_VER
#pragma warning(pop)
#endif /* MSVC */

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

#include "mdbx.h++"

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
  ASSERT(needed >= 0);
  std::string result;
  result.reserve(size_t(needed + 1));
  result.resize(size_t(needed), '\0');
  ASSERT(int(result.capacity()) > needed);
  int actual = vsnprintf(const_cast<char *>(result.data()), result.capacity(), fmt, ones);
  ASSERT(actual == needed);
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
DEFINE_EXCEPTION(laggard_reader)
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
    CASE_EXCEPTION(laggard_reader, MDBX_LAGGARD_READER);
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
    LS = 4,                // shift for UTF8 sequence length
    P_ = 1 << LS,          // printable ASCII flag
    X_ = 1 << (LS - 1),    // printable extended ASCII flag
    N_ = 0,                // non-printable ASCII
    r80_BF = 0,            // flag for UTF8 2nd byte range
    rA0_BF = 1,            // flag for UTF8 2nd byte range
    r80_9F = 2,            // flag for UTF8 2nd byte range
    r90_BF = 3,            // flag for UTF8 2nd byte range
    r80_8F = 4,            // flag for UTF8 2nd byte range
    second_range_mask = 7, // mask for range flag

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
    ASSERT(ptr <= dest + dest_size);
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

    if (MDBX_UNLIKELY(left < 2 || !isxdigit(src[0]) || !isxdigit(src[1])))
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
    ASSERT(ptr <= dest + dest_size);
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

    if (MDBX_UNLIKELY(left < 2 || !isxdigit(src[0]) || !isxdigit(src[1])))
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
      ASSERT(ptr > buf.area);
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
    ASSERT(chunk < modulo);
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
    ASSERT(static_cast<void *>(ptr) < static_cast<void *>(porous));
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
    ASSERT(wrapper.ptr <= dest + dest_size);
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
        ASSERT(ptr > buf.area);
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
    ASSERT(chunk <= (~b58_uint(0) >> CHAR_BIT));
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
      ASSERT(ptr <= dest + dest_size);
      continue;
    case 2:
      b64_3to4(src[0], src[1], 0, ptr);
      ptr[3] = '=';
      ASSERT(ptr + 4 <= dest + dest_size);
      return ptr + 4;
    case 1:
      b64_3to4(src[0], 0, 0, ptr);
      ptr[2] = ptr[3] = '=';
      ASSERT(ptr + 4 <= dest + dest_size);
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
          ASSERT(ptr + 2 <= dest + dest_size);
          return ptr + 2;
        }
        if (c == d) {
          ASSERT(ptr + 1 <= dest + dest_size);
          return ptr + 1;
        }
      }
      MDBX_CXX20_UNLIKELY goto bailout;
    }
    src += 4;
    left -= 4;
    ptr += 3;
    ASSERT(ptr <= dest + dest_size);
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
    if (options.nested_transactions)
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
      nested_transactions((flags & (MDBX_WRITEMAP | MDBX_RDONLY)) ? false : true),
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

__cold const mdbx::path_char *env::get_path() const {
#if defined(_WIN32) || defined(_WIN64)
  const wchar_t *c_wstr = nullptr;
  error::success_or_throw(::mdbx_env_get_pathW(handle_, &c_wstr));
  static_assert(sizeof(path::value_type) == sizeof(wchar_t), "Oops");
  return c_wstr;
#else
  const char *c_str = nullptr;
  error::success_or_throw(::mdbx_env_get_path(handle_, &c_str));
  static_assert(sizeof(path::value_type) == sizeof(char), "Oops");
  return c_str;
#endif /* Windows */
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
  ASSERT(ptr != nullptr);
  return ptr;
}

env_managed &env_managed::operator=(env_managed &&other) {
  if (this != &other) {
    if (MDBX_UNLIKELY(handle_))
      MDBX_CXX20_UNLIKELY {
        assert(handle_ != other.handle_);
        close();
      }
    inherited::operator=(std::move(other));
  }
  return *this;
}

__cold env_managed::~env_managed() {
  if (MDBX_UNLIKELY(handle_))
    /* coverity[UNCAUGHT_EXCEPT] */
    MDBX_CXX20_UNLIKELY error::success_or_throw(::mdbx_env_close(handle_));
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

  if (op.options.nested_transactions && !get_options().nested_transactions)
    MDBX_CXX20_UNLIKELY error::throw_exception(MDBX_INCOMPATIBLE);
}

__cold env_managed::env_managed(const char *pathname, const env_managed::create_parameters &cp,
                                const env::operate_parameters &op, bool accede)
    : env_managed(create_env()) {
  setup(op.max_maps, op.max_readers);
  set_geometry(cp.geometry);
  error::success_or_throw(
      ::mdbx_env_open(handle_, pathname, op.make_flags(accede, cp.use_subdirectory), cp.file_mode_bits));

  if (op.options.nested_transactions && !get_options().nested_transactions)
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

  if (op.options.nested_transactions && !get_options().nested_transactions)
    MDBX_CXX20_UNLIKELY error::throw_exception(MDBX_INCOMPATIBLE);
}

__cold env_managed::env_managed(const wchar_t *pathname, const env_managed::create_parameters &cp,
                                const env::operate_parameters &op, bool accede)
    : env_managed(create_env()) {
  setup(op.max_maps, op.max_readers);
  set_geometry(cp.geometry);
  error::success_or_throw(
      ::mdbx_env_openW(handle_, pathname, op.make_flags(accede, cp.use_subdirectory), cp.file_mode_bits));

  if (op.options.nested_transactions && !get_options().nested_transactions)
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

void cursor_managed::close() {
  error::success_or_throw(::mdbx_cursor_close2(handle_));
  handle_ = nullptr;
}

cursor_managed cursor::clone(void *your_context) const {
  cursor_managed clone(your_context);
  clone.assign(*this);
  return clone;
}

cursor_managed &cursor_managed::operator=(cursor_managed &&other) {
  if (this != &other) {
    if (MDBX_UNLIKELY(handle_))
      MDBX_CXX20_UNLIKELY {
        assert(handle_ != other.handle_);
        close();
      }
    inherited::operator=(std::move(other));
  }
  return *this;
}

cursor_managed::~cursor_managed() {
  if (handle_)
    /* coverity[UNCAUGHT_EXCEPT] */
    error::success_or_throw(::mdbx_cursor_close2(handle_));
}

//------------------------------------------------------------------------------

txn_managed txn::start_nested() { return start_nested(false); }

txn_managed txn::start_nested(bool readonly) {
  MDBX_txn *nested;
  error::throw_on_nullptr(handle_, MDBX_BAD_TXN);
  error::success_or_throw(
      ::mdbx_txn_begin(mdbx_txn_env(handle_), handle_, readonly ? MDBX_TXN_RDONLY : MDBX_TXN_READWRITE, &nested));
  ASSERT(nested != nullptr);
  return txn_managed(nested);
}

txn_managed &txn_managed::operator=(txn_managed &&other) {
  if (this != &other) {
    if (MDBX_UNLIKELY(handle_))
      MDBX_CXX20_UNLIKELY {
        assert(handle_ != other.handle_);
        abort();
      }
    inherited::operator=(std::move(other));
  }
  return *this;
}

txn_managed::~txn_managed() {
  if (MDBX_UNLIKELY(handle_))
    /* coverity[UNCAUGHT_EXCEPT] */
    MDBX_CXX20_UNLIKELY error::success_or_throw(::mdbx_txn_abort(handle_));
}

void txn_managed::abort() { abort(nullptr); }

void txn_managed::commit() { commit(nullptr); }

bool txn_managed::checkpoint() { return checkpoint(nullptr); }

void txn_managed::commit_embark_read() { commit_embark_read(nullptr); }

void txn_managed::abort(finalization_latency *latency) {
  const error err = static_cast<MDBX_error_t>(::mdbx_txn_abort_ex(handle_, latency));
  if (MDBX_LIKELY(err.code() != MDBX_THREAD_MISMATCH))
    MDBX_CXX20_LIKELY handle_ = nullptr;
  if (MDBX_UNLIKELY(err.code() != MDBX_SUCCESS))
    MDBX_CXX20_UNLIKELY err.throw_exception();
}

void txn_managed::commit(finalization_latency *latency) {
  const error err = static_cast<MDBX_error_t>(::mdbx_txn_commit_ex(handle_, latency));
  if (MDBX_LIKELY(err.code() != MDBX_THREAD_MISMATCH))
    MDBX_CXX20_LIKELY handle_ = nullptr;
  if (MDBX_UNLIKELY(err.code() != MDBX_SUCCESS))
    MDBX_CXX20_UNLIKELY err.throw_exception();
}

bool txn_managed::checkpoint(finalization_latency *latency) {
  const error err = static_cast<MDBX_error_t>(::mdbx_txn_checkpoint(handle_, MDBX_TXN_NOWEAKING, latency));
  if (MDBX_UNLIKELY(err.is_failure())) {
    if (err.code() != MDBX_THREAD_MISMATCH && err.code() != MDBX_EINVAL)
      handle_ = nullptr;
    MDBX_CXX20_UNLIKELY err.throw_exception();
  }
  return err.is_result_true();
}

void txn_managed::commit_embark_read(finalization_latency *latency) {
  error::success_or_throw(::mdbx_txn_commit_embark_read(&handle_, latency));
}

bool txn_managed::amend(bool dont_wait) {
  return !error::boolean_or_throw(::mdbx_txn_amend(
      handle_, &handle_, dont_wait ? MDBX_TXN_READWRITE | MDBX_TXN_TRY : MDBX_TXN_READWRITE, handle_->userctx));
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

  ASSERT(false);
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
  if (it.nested_transactions) {
    out << delimiter << "nested_transactions";
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
