/* mdbx_pageviz.h — Standalone page-access visualization ring buffer for MDBX.
 *
 * Self-contained: no dependencies on MDBX internals.
 * Gate everything behind MDBX_PAGEVIZ; when not defined, all macros expand to
 * nothing (zero overhead).
 *
 * Compatible with C11 and C17. */

#ifndef MDBX_PAGEVIZ_H
#define MDBX_PAGEVIZ_H

/* ── Disabled stub macros ─────────────────────────────────────────────── */

#if !defined(MDBX_PAGEVIZ) || !(MDBX_PAGEVIZ)

#define MDBX_PAGEVIZ_READ(mc, pgno)  ((void)0)
#define MDBX_PAGEVIZ_WRITE(mc, pgno) ((void)0)
#define MDBX_PAGEVIZ_FREE(mc, pgno)  ((void)0)

#else /* MDBX_PAGEVIZ enabled ───────────────────────────────────────────── */

#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* ── Tunables ─────────────────────────────────────────────────────────── */

#define MDBX_PAGEVIZ_RING_CAPACITY   65536u  /* must be power-of-2 */
#define MDBX_PAGEVIZ_MAX_RINGS       128u
#define MDBX_PAGEVIZ_PUBLISH_INTERVAL 32u

/* ── Event encoding ───────────────────────────────────────────────────── */

/* Layout of a 64-bit event word:
 *   bits 63..56  op   (1=READ, 2=WRITE, 3=FREE)
 *   bits 55..32  dbi  (lower 24 bits of a uint32_t)
 *   bits 31..0   pgno (uint32_t page number) */

#define MDBX_PAGEVIZ_OP_READ        1
#define MDBX_PAGEVIZ_OP_WRITE       2
#define MDBX_PAGEVIZ_OP_FREE        3
#define MDBX_PAGEVIZ_OP_BLOCK_START 4
#define MDBX_PAGEVIZ_OP_BLOCK_END   5

#define MDBX_PAGEVIZ_ENCODE(op, dbi, pgno)                                    \
  (((uint64_t)(op) << 56) | ((uint64_t)((dbi) & 0x00FFFFFFu) << 32) |         \
   (uint64_t)(uint32_t)(pgno))

#define MDBX_PAGEVIZ_DECODE_OP(ev)   ((uint8_t)((ev) >> 56))
#define MDBX_PAGEVIZ_DECODE_DBI(ev)  ((uint32_t)(((ev) >> 32) & 0x00FFFFFFu))
#define MDBX_PAGEVIZ_DECODE_PGNO(ev) ((uint32_t)(ev))

/* ── Ring-buffer mask (power-of-2 capacity) ───────────────────────────── */

#define MDBX_PAGEVIZ_RING_MASK (MDBX_PAGEVIZ_RING_CAPACITY - 1u)

/* ── Platform TLS ─────────────────────────────────────────────────────── */

#if defined(_MSC_VER)
#  define MDBX_PAGEVIZ_TLS __declspec(thread)
#elif defined(__GNUC__) || defined(__clang__)
#  define MDBX_PAGEVIZ_TLS __thread
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
#  define MDBX_PAGEVIZ_TLS _Thread_local
#else
#  error "No thread-local storage support detected"
#endif

/* ── Structures ───────────────────────────────────────────────────────── */

typedef struct mdbx_pageviz_ring {
  uint64_t events[MDBX_PAGEVIZ_RING_CAPACITY];
  _Atomic uint32_t published_head; /* consumer reads (acquire) */
  uint32_t local_head;             /* producer-only, no atomic needed */
  _Atomic uint32_t consumer_tail;  /* consumer updates after draining */
  _Atomic uint64_t dropped;        /* unused, kept for ABI compat */
  uint32_t _pad[10];               /* pad to separate cache lines */
} mdbx_pageviz_ring_t;

typedef struct mdbx_pageviz_mapping {
  volatile void *base;
  volatile size_t len;
  volatile uint32_t mdbx_page_size;
  volatile uint32_t sys_page_size;
} mdbx_pageviz_mapping_t;

typedef struct mdbx_pageviz_state {
  mdbx_pageviz_ring_t rings[MDBX_PAGEVIZ_MAX_RINGS];
  _Atomic uint32_t ring_count; /* how many rings registered */
  _Atomic uint32_t enabled;    /* runtime enable/disable */
  mdbx_pageviz_mapping_t mapping;
} mdbx_pageviz_state_t;

/* ── Global state (defined in .c) ─────────────────────────────────────── */

extern mdbx_pageviz_state_t *mdbx_pageviz_global;

/* ── Thread-local ring index (defined in .c) ──────────────────────────── */

#define MDBX_PAGEVIZ_TLS_UNREGISTERED UINT32_MAX

extern MDBX_PAGEVIZ_TLS uint32_t mdbx_pageviz_tls_ring;

/* ── Inline helpers (private) ─────────────────────────────────────────── */

static inline uint32_t mdbx_pageviz_register_ring_(mdbx_pageviz_state_t *s) {
  uint32_t idx =
      atomic_fetch_add_explicit(&s->ring_count, 1, memory_order_relaxed);
  if (idx >= MDBX_PAGEVIZ_MAX_RINGS) {
    atomic_fetch_sub_explicit(&s->ring_count, 1, memory_order_relaxed);
    return MDBX_PAGEVIZ_TLS_UNREGISTERED;
  }
  mdbx_pageviz_ring_t *r = &s->rings[idx];
  atomic_store_explicit(&r->published_head, 0, memory_order_relaxed);
  r->local_head = 0;
  atomic_store_explicit(&r->consumer_tail, 0, memory_order_relaxed);
  atomic_store_explicit(&r->dropped, 0, memory_order_relaxed);
  return idx;
}

/* ── Hot-path emit (inlined) ──────────────────────────────────────────── */

static inline void mdbx_pageviz_emit(uint8_t op, uint32_t dbi,
                                      uint32_t pgno) {
  mdbx_pageviz_state_t *state = mdbx_pageviz_global;
  if (__builtin_expect(
          state == NULL ||
              !atomic_load_explicit(&state->enabled, memory_order_relaxed),
          1))
    return;

  uint32_t idx = mdbx_pageviz_tls_ring;
  if (__builtin_expect(idx == MDBX_PAGEVIZ_TLS_UNREGISTERED, 0)) {
    idx = mdbx_pageviz_register_ring_(state);
    if (__builtin_expect(idx == MDBX_PAGEVIZ_TLS_UNREGISTERED, 0))
      return;
    mdbx_pageviz_tls_ring = idx;
  }

  mdbx_pageviz_ring_t *r = &state->rings[idx];

  /* Block (spin) until the consumer drains enough space. */
  while (__builtin_expect(
      (uint32_t)(r->local_head -
                 atomic_load_explicit(&r->consumer_tail, memory_order_acquire))
          >= MDBX_PAGEVIZ_RING_CAPACITY, 0)) {
    /* Flush our writes so the consumer can make progress. */
    atomic_store_explicit(&r->published_head, r->local_head,
                          memory_order_release);
#if defined(__x86_64__) || defined(_M_X64) || defined(__i386__) || defined(_M_IX86)
    __builtin_ia32_pause();
#elif defined(__aarch64__) || defined(_M_ARM64)
    __asm__ volatile("yield");
#endif
  }

  r->events[r->local_head & MDBX_PAGEVIZ_RING_MASK] =
      MDBX_PAGEVIZ_ENCODE(op, dbi, pgno);
  r->local_head++;

  if ((r->local_head & (MDBX_PAGEVIZ_PUBLISH_INTERVAL - 1u)) == 0)
    atomic_store_explicit(&r->published_head, r->local_head,
                          memory_order_release);
}

/* ── Macro hooks (inserted into mdbx.c) ──────────────────────────────── */

#define MDBX_PAGEVIZ_READ(mc, pgno)                                            \
  mdbx_pageviz_emit(MDBX_PAGEVIZ_OP_READ, (uint32_t)cursor_dbi(mc), (pgno))
#define MDBX_PAGEVIZ_WRITE(mc, pgno)                                           \
  mdbx_pageviz_emit(MDBX_PAGEVIZ_OP_WRITE, (uint32_t)cursor_dbi(mc), (pgno))
#define MDBX_PAGEVIZ_FREE(mc, pgno)                                            \
  mdbx_pageviz_emit(MDBX_PAGEVIZ_OP_FREE, (uint32_t)cursor_dbi(mc), (pgno))

/* ── Public API (defined in .c) ───────────────────────────────────────── */

mdbx_pageviz_state_t *mdbx_pageviz_create(void);
void mdbx_pageviz_destroy(mdbx_pageviz_state_t *state);
void mdbx_pageviz_enable(mdbx_pageviz_state_t *state);
void mdbx_pageviz_disable(mdbx_pageviz_state_t *state);
uint32_t mdbx_pageviz_drain(mdbx_pageviz_state_t *state, uint32_t ring_idx,
                             uint64_t *out_buf, uint32_t max_count);
uint32_t mdbx_pageviz_ring_count(mdbx_pageviz_state_t *state);
uint64_t mdbx_pageviz_dropped(mdbx_pageviz_state_t *state, uint32_t ring_idx);
void mdbx_pageviz_set_mapping(void *base, size_t len, uint32_t mdbx_page_size);
int mdbx_pageviz_get_mapping(void **out_base, size_t *out_len,
                              uint32_t *out_mdbx_ps, uint32_t *out_sys_ps);
void mdbx_pageviz_emit_block_marker(uint8_t op, uint32_t block_number,
                                     uint16_t tx_count);

#endif /* MDBX_PAGEVIZ */
#endif /* MDBX_PAGEVIZ_H */
