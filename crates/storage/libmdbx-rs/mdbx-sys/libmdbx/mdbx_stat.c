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

#define xMDBX_TOOLS /* Avoid using internal ASSERT(), etc */
#include "mdbx-internals.h"

enum { MDBX_STAT_MAXDBS = 2 };

#if defined(_WIN32) || defined(_WIN64)

/* Bit of madness for Windows console */
#define mdbx_strerror mdbx_strerror_ANSI2OEM
#define mdbx_strerror_r mdbx_strerror_r_ANSI2OEM

#include "mdbx-wingetopt.h"

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

static void print_stat(const MDBX_stat *ms) {
  printf("  Pagesize: %u\n", ms->ms_psize);
  printf("  Tree depth: %u\n", ms->ms_depth);
  printf("  Branch pages: %" PRIu64 "\n", ms->ms_branch_pages);
  printf("  Leaf pages: %" PRIu64 "\n", ms->ms_leaf_pages);
  printf("  Large/Overflow pages: %" PRIu64 "\n", ms->ms_overflow_pages);
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
           "   #\tslot\t%10s %*s %20s %10s %13s %13s\n",
           "pid", (int)sizeof(size_t) * 2, "thread", "txnid", "lag", "used", "retained");

  char thread_buf[32], *thread_str;
  if (thread == (mdbx_tid_t)((uintptr_t)MDBX_TID_TXN_OUSTED))
    thread_str = "ousted";
  else if (thread == (mdbx_tid_t)((uintptr_t)MDBX_TID_TXN_PARKED))
    thread_str = "parked";
  else
    snprintf(thread_str = thread_buf, sizeof(thread_buf), "%" PRIxPTR, (uintptr_t)thread);

  printf(" %3d)\t[%d]\t%10" PRIdSIZE " %*s", num, slot, (size_t)pid, (int)sizeof(size_t) * 2, thread_str);

  if (txnid)
    printf(" %20" PRIu64 " %10" PRIu64 " %12.1fM %12.1fM\n", txnid, lag, bytes_used / 1048576.0,
           bytes_retained / 1048576.0);
  else
    printf(" %20s %10s %13s %13s\n", "-", "0", "0", "0");

  return user_break ? MDBX_RESULT_TRUE : MDBX_RESULT_FALSE;
}

static int table_enum_func(void *ctx, const MDBX_txn *txn, const MDBX_val *name, MDBX_db_flags_t db_flags,
                           const struct MDBX_stat *stat, MDBX_dbi dbi) {
  (void)ctx;
  (void)txn;
  (void)db_flags;
  (void)dbi;
  printf("Status of %.*s\n", (int)name->iov_len, (const char *)name->iov_base);
  print_stat(stat);
  return user_break ? MDBX_RESULT_TRUE : MDBX_RESULT_FALSE;
}

static int gc_list_func(void *ctx, const MDBX_txn *txn, uint64_t span_txnid, size_t span_pgno, size_t span_length,
                        bool span_is_reclaimable) {
  (void)ctx;
  (void)txn;
  (void)span_txnid;
  (void)span_pgno;
  (void)span_length;
  (void)span_is_reclaimable;
  return user_break ? MDBX_RESULT_TRUE : MDBX_RESULT_FALSE;
}

static const char *prog;
static bool quiet = false;
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

static void print_pages_percentage(const char *caption, size_t value, size_t backed, size_t total) {
  char buf[42];
  const char *suffix = " pages";
  printf("  %s: %" PRIuSIZE, caption, value);
  if (value && value < backed) {
    printf(", %s%% of %s", mdbx_ratio2percents(value, backed, buf, sizeof(buf)), "backed");
    suffix = "";
  }
  if (value && value < total) {
    printf(", %s%% of %s", mdbx_ratio2percents(value, total, buf, sizeof(buf)), "total");
    suffix = "";
  }
  puts(suffix);
}

int main(int argc, char *argv[]) {
  int opt, rc;
  MDBX_env *env;
  MDBX_txn *txn = nullptr;
  MDBX_dbi dbi;
  MDBX_envinfo mei;
  prog = argv[0];
  char *envname;
  char *table = nullptr;
  bool alltbl = false, show_env_info = false, show_page_ops = false;
  short show_gc = 0, show_readers = 0;

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
      show_page_ops = true;
      break;
    case 'a':
      if (table)
        usage(prog);
      alltbl = true;
      break;
    case 'e':
      show_env_info = true;
      break;
    case 'f':
      show_gc += 1;
      break;
    case 'n':
      break;
    case 'r':
      show_readers += 1;
      break;
    case 's':
      if (alltbl)
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

  if (alltbl || table) {
    rc = mdbx_env_set_maxdbs(env, MDBX_STAT_MAXDBS);
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

  if (show_env_info || show_gc || show_page_ops) {
    rc = mdbx_env_info_ex(env, txn, &mei, sizeof(mei));
    if (unlikely(rc != MDBX_SUCCESS)) {
      error("mdbx_env_info_ex", rc);
      goto txn_abort;
    }
  } else {
    /* LY: zap warnings from gcc */
    memset(&mei, 0, sizeof(mei));
  }

  if (show_page_ops) {
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

  if (show_env_info) {
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
               "may be large than the database itself,\n                    "
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

  if (show_readers) {
    rc = mdbx_reader_list(env, reader_list_func, nullptr);
    if (MDBX_IS_ERROR(rc)) {
      error("mdbx_reader_list", rc);
      goto txn_abort;
    }
    if (rc == MDBX_RESULT_TRUE)
      printf("Reader Table is absent\n");
    else if (rc == MDBX_SUCCESS && show_readers > 1) {
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
    if (!(table || alltbl || show_gc))
      goto txn_abort;
  }

  if (show_gc) {
    printf("Page Usage & Garbage Collection%s\n",
           (show_gc > 1) ? " (please use `mdbx_chk` tool for detailed GC information instead)" : "");
    MDBX_gc_info_t info;
    rc = mdbx_gc_info(txn, &info, sizeof(info), gc_list_func, nullptr);

    switch (rc) {
    case MDBX_SUCCESS:
      break;
    case MDBX_EINTR:
      if (!quiet)
        fprintf(stderr, "Interrupted by signal/user\n");
      goto txn_abort;
    default:
      error("mdbx_gc_info", rc);
      goto txn_abort;
    }

    const size_t remained_pages = info.pages_total - info.pages_allocated;
    const size_t used_pages = info.pages_allocated - info.pages_gc;
    const size_t gc_retained = info.pages_gc - info.gc_reclaimable.pages;
    const size_t available_pages = info.gc_reclaimable.pages + remained_pages;
    print_pages_percentage("Total", info.pages_total, info.pages_backed, info.pages_total);
    print_pages_percentage("Backed", info.pages_backed, info.pages_backed, info.pages_total);
    print_pages_percentage("Allocated", info.pages_allocated, info.pages_backed, info.pages_total);
    print_pages_percentage("Remained", remained_pages, info.pages_backed, info.pages_total);
    print_pages_percentage("Used", used_pages, info.pages_backed, info.pages_total);
    print_pages_percentage("GC|whole", info.pages_gc, info.pages_backed, info.pages_total);
    print_pages_percentage("GC|reclaimable", info.gc_reclaimable.pages, info.pages_backed, info.pages_total);
    if (show_gc > 1) {
      printf("  %s: ", "GC|reclaimable span-length distribution");
      const char *suffix = "empty";
      if (info.gc_reclaimable.span_histogram.amount) {
        printf("single %zu", info.gc_reclaimable.span_histogram.le1_count);
        for (size_t i = 0; i < ARRAY_LENGTH(info.gc_reclaimable.span_histogram.ranges); ++i) {
          if (info.gc_reclaimable.span_histogram.ranges[i].count) {
            printf(", %zu", info.gc_reclaimable.span_histogram.ranges[i].begin);
            if (info.gc_reclaimable.span_histogram.ranges[i].end !=
                info.gc_reclaimable.span_histogram.ranges[i].begin + 1)
              printf("-%zu", info.gc_reclaimable.span_histogram.ranges[i].end);
            printf("x%zu", info.gc_reclaimable.span_histogram.ranges[i].count);
          }
        }
        suffix = " pages";
      }
      puts(suffix);
    }
    print_pages_percentage("GC|retained", gc_retained, info.pages_backed, info.pages_total);
    print_pages_percentage("Available", available_pages, info.pages_backed, info.pages_total);
    if (info.max_retained_pages || info.max_reader_lag) {
      printf("  max reader lag %zu\n", info.max_reader_lag);
      printf("  max retained pages %zu\n", info.max_retained_pages);
    }
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
  mdbx_dbi_close(env, dbi);

  if (alltbl) {
    rc = mdbx_enumerate_tables(txn, table_enum_func, nullptr);
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
        error("mdbx_enumerate_tables", rc);
    }
  }

txn_abort:
  mdbx_txn_abort(txn);
env_close:
  mdbx_env_close(env);

  return rc ? EXIT_FAILURE : EXIT_SUCCESS;
}
