/* Experimental, process-local residency-cache diagnostics. The caller must use
 * the same writer serialization as the cache. No atomics, allocation, clocks,
 * or logging occur per probe. Window units are max(DB page size, OS page size).
 * This observes one writer environment; it does not simulate alternative LRU
 * caches or validate the freshness of cached residency answers. */
#ifndef RETH_MINCORE_DIAGNOSTICS_H
#define RETH_MINCORE_DIAGNOSTICS_H

typedef struct {
  uint64_t begin, used;
  unsigned units;
} mincore_diag_window_t;

typedef struct {
  uint64_t probes, hits[4], misses, errors, resident_hits, nonresident_hits;
  uint64_t resident_misses, nonresident_misses, repeated_hits, untracked_hits;
  uint64_t queried_os_pages, resident_os_pages, queried_units;
  uint64_t all_resident, none_resident, mixed, clipped_queries;
  uint64_t evictions, invalidated_windows, clears;
  uint64_t completed_windows, completed_units, used_units;
  uint64_t usage_hist[65], offset_hits[64], ghost_hits[16], ghost_misses;
  mincore_diag_window_t windows[4], ghosts[16];
  unsigned ghost_next;
} mincore_diagnostics_t;

static unsigned mincore_diag_popcount(uint64_t bits) {
  return (unsigned)__builtin_popcountll(bits);
}

static void mincore_diag_finish(mincore_diagnostics_t *d, size_t slot) {
  mincore_diag_window_t *w = &d->windows[slot];
  if (w->units) {
    const unsigned used = mincore_diag_popcount(w->used);
    ++d->completed_windows;
    d->completed_units += w->units;
    d->used_units += used;
    ++d->usage_hist[used];
    w->units = 0;
  }
}

static void mincore_diag_clear(mincore_diagnostics_t *d) {
  if (!d)
    return;
  ++d->clears;
  for (size_t i = 0; i < 4; ++i) {
    d->invalidated_windows += d->windows[i].units != 0;
    mincore_diag_finish(d, i);
  }
  /* Old residency answers cannot survive an explicit cache invalidation. */
  memset(d->ghosts, 0, sizeof(d->ghosts));
  d->ghost_next = 0;
}

static void mincore_diag_hit(mincore_diagnostics_t *d, size_t slot, uint64_t begin,
                             unsigned offset, bool resident) {
  ++d->hits[slot];
  ++d->offset_hits[offset];
  d->resident_hits += resident;
  d->nonresident_hits += !resident;
  mincore_diag_window_t *w = &d->windows[slot];
  if (w->units && w->begin == begin && offset < w->units) {
    const uint64_t bit = UINT64_C(1) << offset;
    d->repeated_hits += (w->used & bit) != 0;
    w->used |= bit;
  } else {
    ++d->untracked_hits;
  }
  const mincore_diag_window_t hit = *w;
  for (size_t i = slot; i > 0; --i)
    d->windows[i] = d->windows[i - 1];
  d->windows[0] = hit;
}

static void mincore_diag_miss(mincore_diagnostics_t *d, uint64_t begin) {
  ++d->misses;
  /* Search recently evicted query windows, newest first. A match indicates
   * potential capacity reuse, not the hit rate of a larger simulated cache. */
  for (unsigned age = 0; age < 16; ++age) {
    const mincore_diag_window_t *w = &d->ghosts[(d->ghost_next + 15 - age) % 16];
    if (w->units && begin >= w->begin && begin - w->begin < w->units) {
      ++d->ghost_hits[age];
      return;
    }
  }
  ++d->ghost_misses;
}

static void mincore_diag_fetch(mincore_diagnostics_t *d, uint64_t begin, size_t pages,
                               unsigned shift, const uint8_t *vector, uint64_t nonresident) {
  mincore_diag_window_t evicted = d->windows[3];
  if (evicted.units) {
    ++d->evictions;
    d->ghosts[d->ghost_next] = evicted;
    d->ghost_next = (d->ghost_next + 1) % 16;
  }
  mincore_diag_finish(d, 3);
  for (size_t i = 3; i > 0; --i)
    d->windows[i] = d->windows[i - 1];
  const unsigned units = (unsigned)((pages + ((size_t)1 << shift) - 1) >> shift);
  d->windows[0] = (mincore_diag_window_t){begin, 1, units};
  d->queried_os_pages += pages;
  d->queried_units += units;
  d->clipped_queries += units < 64;
  for (size_t i = 0; i < pages; ++i)
    d->resident_os_pages += vector[i] & 1;
  const unsigned absent = mincore_diag_popcount(nonresident);
  d->all_resident += absent == 0;
  d->none_resident += absent == units;
  d->mixed += absent != 0 && absent != units;
  d->resident_misses += (nonresident & 1) == 0;
  d->nonresident_misses += (nonresident & 1) != 0;
}

/* Cumulative snapshots include still-live windows, without evicting them.
 * Emit after each top-level write transaction so graceful env teardown is not
 * required. Consumers must difference snapshots, never sum cumulative rows. */
static void mincore_diag_report(const mincore_diagnostics_t *d, uint64_t txnid,
                                unsigned mode, unsigned db_page_size, unsigned os_page_size) {
  if (!d || !d->probes)
    return;
  mincore_diagnostics_t snapshot = *d;
  for (size_t i = 0; i < 4; ++i)
    mincore_diag_finish(&snapshot, i);
  char buffer[16384]; /* All fields and arrays fit even with 20-digit counters. */
  size_t used = 0;
#define MC_APPEND(...) used += (size_t)snprintf(buffer + used, sizeof(buffer) - used, __VA_ARGS__)
#define MC_FIELD(name) MC_APPEND(",\"" #name "\":%" PRIu64, snapshot.name)
#define MC_ARRAY(name)                                                                                                 \
  do {                                                                                                                \
    MC_APPEND(",\"" #name "\":[");                                                                                       \
    for (size_t i = 0; i < sizeof(snapshot.name) / sizeof(snapshot.name[0]); ++i)                                       \
      MC_APPEND("%s%" PRIu64, i ? "," : "", snapshot.name[i]);                                                         \
    MC_APPEND("]");                                                                                                   \
  } while (0)
  MC_APPEND("MDBX_MINCORE_DIAG {\"txnid\":%" PRIu64 ",\"end_mode\":%u,\"db_page_size\":%u,\"os_page_size\":%u",
            txnid, mode, db_page_size, os_page_size);
  MC_FIELD(probes); MC_ARRAY(hits); MC_FIELD(misses); MC_FIELD(errors);
  MC_FIELD(resident_hits); MC_FIELD(nonresident_hits); MC_FIELD(resident_misses); MC_FIELD(nonresident_misses);
  MC_FIELD(repeated_hits); MC_FIELD(untracked_hits);
  MC_FIELD(queried_os_pages); MC_FIELD(resident_os_pages); MC_FIELD(queried_units);
  MC_FIELD(all_resident); MC_FIELD(none_resident); MC_FIELD(mixed); MC_FIELD(clipped_queries);
  MC_FIELD(evictions); MC_FIELD(invalidated_windows); MC_FIELD(clears);
  MC_FIELD(completed_windows); MC_FIELD(completed_units); MC_FIELD(used_units);
  MC_ARRAY(usage_hist); MC_ARRAY(offset_hits); MC_ARRAY(ghost_hits); MC_FIELD(ghost_misses);
  MC_APPEND("}\n");
  assert(used < sizeof(buffer));
  fputs(buffer, stderr);
#undef MC_ARRAY
#undef MC_FIELD
#undef MC_APPEND
}
#endif
