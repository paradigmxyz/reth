#include "mdbx_pageviz.h"

#if defined(MDBX_PAGEVIZ) && MDBX_PAGEVIZ

/* ── Global state ─────────────────────────────────────────────────────── */

mdbx_pageviz_state_t *mdbx_pageviz_global = NULL;

/* ── Thread-local ring index ──────────────────────────────────────────── */

MDBX_PAGEVIZ_TLS uint32_t mdbx_pageviz_tls_ring = MDBX_PAGEVIZ_TLS_UNREGISTERED;

/* ── Lifecycle ────────────────────────────────────────────────────────── */

mdbx_pageviz_state_t *mdbx_pageviz_create(void) {
  mdbx_pageviz_state_t *state = calloc(1, sizeof(mdbx_pageviz_state_t));
  if (state)
    mdbx_pageviz_global = state;
  return state;
}

void mdbx_pageviz_destroy(mdbx_pageviz_state_t *state) {
  if (!state)
    return;
  if (mdbx_pageviz_global == state)
    mdbx_pageviz_global = NULL;
  free(state);
}

/* ── Enable / Disable ─────────────────────────────────────────────────── */

void mdbx_pageviz_enable(mdbx_pageviz_state_t *state) {
  atomic_store_explicit(&state->enabled, 1, memory_order_release);
}

void mdbx_pageviz_disable(mdbx_pageviz_state_t *state) {
  atomic_store_explicit(&state->enabled, 0, memory_order_release);
}

/* ── Drain ────────────────────────────────────────────────────────────── */

uint32_t mdbx_pageviz_drain(mdbx_pageviz_state_t *state, uint32_t ring_idx,
                            uint64_t *out_buf, uint32_t max_count) {
  if (!state || ring_idx >= MDBX_PAGEVIZ_MAX_RINGS || !out_buf || max_count == 0)
    return 0;

  mdbx_pageviz_ring_t *ring = &state->rings[ring_idx];
  uint32_t head = atomic_load_explicit(&ring->published_head,
                                       memory_order_acquire);
  uint32_t tail = atomic_load_explicit(&ring->consumer_tail,
                                       memory_order_relaxed);

  if (head == tail)
    return 0;

  uint32_t avail = head - tail;
  uint32_t count = avail < max_count ? avail : max_count;
  for (uint32_t i = 0; i < count; i++) {
    uint32_t slot = (tail + i) & MDBX_PAGEVIZ_RING_MASK;
    out_buf[i] = ring->events[slot];
  }

  atomic_store_explicit(&ring->consumer_tail, tail + count,
                        memory_order_release);
  return count;
}

/* ── Queries ──────────────────────────────────────────────────────────── */

uint32_t mdbx_pageviz_ring_count(mdbx_pageviz_state_t *state) {
  return atomic_load_explicit(&state->ring_count, memory_order_relaxed);
}

uint64_t mdbx_pageviz_dropped(mdbx_pageviz_state_t *state, uint32_t ring_idx) {
  return atomic_load_explicit(&state->rings[ring_idx].dropped,
                               memory_order_relaxed);
}

/* ── Mapping info ────────────────────────────────────────────────────── */

void mdbx_pageviz_set_mapping(void *base, size_t len, uint32_t mdbx_page_size) {
  mdbx_pageviz_state_t *state = mdbx_pageviz_global;
  if (!state)
    return;
  state->mapping.mdbx_page_size = mdbx_page_size;
  state->mapping.sys_page_size = (uint32_t)sysconf(_SC_PAGESIZE);
  state->mapping.len = len;
  /* base written last so consumers see consistent len+page_size first */
  __atomic_store_n((void *volatile *)&state->mapping.base, base, __ATOMIC_RELEASE);
}

int mdbx_pageviz_get_mapping(void **out_base, size_t *out_len,
                              uint32_t *out_mdbx_ps, uint32_t *out_sys_ps) {
  mdbx_pageviz_state_t *state = mdbx_pageviz_global;
  if (!state)
    return 0;
  void *b = __atomic_load_n((void *volatile *)&state->mapping.base, __ATOMIC_ACQUIRE);
  if (!b)
    return 0;
  *out_base = b;
  *out_len = state->mapping.len;
  *out_mdbx_ps = state->mapping.mdbx_page_size;
  *out_sys_ps = state->mapping.sys_page_size;
  return 1;
}

/* ── Block marker emit (non-inline wrapper for Rust FFI) ──────────────── */

void mdbx_pageviz_emit_block_marker(uint8_t op, uint32_t block_number,
                                     uint16_t tx_count, uint8_t duration_encoded,
                                     uint8_t gas_encoded) {
  mdbx_pageviz_emit(op, ((uint32_t)gas_encoded << 24) | ((uint32_t)duration_encoded << 16) | (uint32_t)tx_count, block_number);
}

#else /* !MDBX_PAGEVIZ */

typedef int mdbx_pageviz_empty_tu_;

#endif /* MDBX_PAGEVIZ */
