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

#include <ctype.h>

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

#define PRINT 1
#define GLOBAL 2
#define CONCISE 4
static int mode = GLOBAL;

typedef struct flagbit {
  int bit;
  char *name;
} flagbit;

static const flagbit dbflags[] = {{MDBX_REVERSEKEY, "reversekey"},
                                  {MDBX_DUPSORT, "dupsort"},
                                  {MDBX_INTEGERKEY, "integerkey"},
                                  {MDBX_DUPFIXED, "dupfix"},
                                  {MDBX_INTEGERDUP, "integerdup"},
                                  {MDBX_REVERSEDUP, "reversedup"},
                                  {0, nullptr}};

static void dumpval(const MDBX_val *v) {
  static const char digits[] = "0123456789abcdef";
  putchar(' ');
  for (const unsigned char *c = v->iov_base, *end = c + v->iov_len; c < end; ++c) {
    if (mode & PRINT) {
      if (isprint(*c) && *c != '\\') {
        putchar(*c);
        continue;
      } else
        putchar('\\');
    }
    putchar(digits[*c >> 4]);
    putchar(digits[*c & 15]);
  }
  putchar('\n');
}

static bool quiet = false, rescue = false;
static const char *prog;
static void error(const char *func, int rc) {
  if (!quiet)
    fprintf(stderr, "%s: %s() error %d %s\n", prog, func, rc, mdbx_strerror(rc));
}

/* Dump in BDB-compatible format */
static int dump_tbl(MDBX_txn *txn, MDBX_dbi dbi, char *name) {
  unsigned flags;
  int rc = mdbx_dbi_flags(txn, dbi, &flags);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_dbi_flags", rc);
    return rc;
  }

  MDBX_stat ms;
  rc = mdbx_dbi_stat(txn, dbi, &ms, sizeof(ms));
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_dbi_stat", rc);
    return rc;
  }

  MDBX_envinfo info;
  rc = mdbx_env_info_ex(mdbx_txn_env(txn), txn, &info, sizeof(info));
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_env_info_ex", rc);
    return rc;
  }

  printf("VERSION=3\n");
  if (mode & GLOBAL) {
    mode -= GLOBAL;
    if (info.mi_geo.upper != info.mi_geo.lower)
      printf("geometry=l%" PRIu64 ",u%" PRIu64 ",s%" PRIu64 ",g%" PRIu64 "\n", info.mi_geo.lower, info.mi_geo.upper,
             info.mi_geo.shrink, info.mi_geo.grow);
    printf("mapsize=%" PRIu64 "\n", info.mi_geo.upper);
    /* printf("maxreaders=%u\n", info.mi_maxreaders); */

    MDBX_canary canary;
    rc = mdbx_canary_get(txn, &canary);
    if (unlikely(rc != MDBX_SUCCESS)) {
      error("mdbx_canary_get", rc);
      return rc;
    }
    if (canary.v)
      printf("canary=v%" PRIu64 ",x%" PRIu64 ",y%" PRIu64 ",z%" PRIu64 "\n", canary.v, canary.x, canary.y, canary.z);
  }
  printf("format=%s\n", mode & PRINT ? "print" : "bytevalue");
  if (name)
    printf("database=%s\n", name);
  printf("type=btree\n");
  printf("db_pagesize=%u\n", ms.ms_psize);
  /* if (ms.ms_mod_txnid)
    printf("txnid=%" PRIaTXN "\n", ms.ms_mod_txnid);
  else if (!name)
    printf("txnid=%" PRIaTXN "\n", mdbx_txn_id(txn)); */

  printf("duplicates=%d\n", (flags & (MDBX_DUPSORT | MDBX_DUPFIXED | MDBX_INTEGERDUP | MDBX_REVERSEDUP)) ? 1 : 0);
  for (int i = 0; dbflags[i].bit; i++)
    if (flags & dbflags[i].bit)
      printf("%s=1\n", dbflags[i].name);

  uint64_t sequence;
  rc = mdbx_dbi_sequence(txn, dbi, &sequence, 0);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_dbi_sequence", rc);
    return rc;
  }
  if (sequence)
    printf("sequence=%" PRIu64 "\n", sequence);

  printf("HEADER=END\n"); /*-------------------------------------------------*/

  MDBX_cursor *cursor;
  MDBX_val key, data;
  rc = mdbx_cursor_open(txn, dbi, &cursor);
  if (unlikely(rc != MDBX_SUCCESS)) {
    error("mdbx_cursor_open", rc);
    return rc;
  }
  if (rescue) {
    rc = mdbx_cursor_ignord(cursor);
    if (unlikely(rc != MDBX_SUCCESS))
      error("mdbx_cursor_ignord", rc);
  }

  while ((rc = mdbx_cursor_get(cursor, &key, &data, MDBX_NEXT)) == MDBX_SUCCESS) {
    if (user_break) {
      rc = MDBX_EINTR;
      break;
    }
    dumpval(&key);
    dumpval(&data);
    if ((flags & MDBX_DUPSORT) && (mode & CONCISE)) {
      while ((rc = mdbx_cursor_get(cursor, &key, &data, MDBX_NEXT_DUP)) == MDBX_SUCCESS) {
        if (user_break) {
          rc = MDBX_EINTR;
          break;
        }
        putchar(' ');
        dumpval(&data);
      }
      if (rc != MDBX_NOTFOUND)
        break;
    }
  }
  printf("DATA=END\n");
  if (rc == MDBX_NOTFOUND)
    rc = MDBX_SUCCESS;
  if (unlikely(rc != MDBX_SUCCESS))
    error("mdbx_cursor_get", rc);

  mdbx_cursor_close(cursor);
  return rc;
}

static void usage(void) {
  fprintf(stderr,
          "usage: %s "
          "[-V] [-q] [-c] [-f file] [-l] [-p] [-r] [-a|-s table] [-u|U] "
          "dbpath\n"
          "  -V\t\tprint version and exit\n"
          "  -q\t\tbe quiet\n"
          "  -c\t\tconcise mode without repeating keys,\n"
          "  \t\tbut incompatible with Berkeley DB and LMDB\n"
          "  -f\t\twrite to file instead of stdout\n"
          "  -l\t\tlist tables and exit\n"
          "  -p\t\tuse printable characters\n"
          "  -r\t\trescue mode (ignore errors to dump corrupted DB)\n"
          "  -a\t\tdump main DB and all tables\n"
          "  -s name\tdump only the specified named table\n"
          "  -u\t\twarmup database before dumping\n"
          "  -U\t\twarmup and try lock database pages in memory before dumping\n"
          "  \t\tby default dump only the main DB\n",
          prog);
  exit(EXIT_FAILURE);
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

static int equal_or_greater(const MDBX_val *a, const MDBX_val *b) {
  return (a->iov_len == b->iov_len && memcmp(a->iov_base, b->iov_base, a->iov_len) == 0) ? 0 : 1;
}

int main(int argc, char *argv[]) {
  int i, err;
  MDBX_env *env;
  MDBX_txn *txn;
  MDBX_dbi dbi;
  prog = argv[0];
  char *envname;
  char *subname = nullptr, *buf4free = nullptr;
  unsigned envflags = 0;
  bool alldbs = false, list = false;
  bool warmup = false;
  MDBX_warmup_flags_t warmup_flags = MDBX_warmup_default;

  if (argc < 2)
    usage();

  while ((i = getopt(argc, argv,
                     "uU"
                     "a"
                     "f:"
                     "l"
                     "n"
                     "p"
                     "s:"
                     "V"
                     "r"
                     "c"
                     "q")) != EOF) {
    switch (i) {
    case 'V':
      printf("mdbx_dump version %d.%d.%d.%d\n"
             " - source: %s %s, commit %s, tree %s\n"
             " - anchor: %s\n"
             " - build: %s for %s by %s\n"
             " - flags: %s\n"
             " - options: %s\n",
             mdbx_version.major, mdbx_version.minor, mdbx_version.patch, mdbx_version.tweak, mdbx_version.git.describe,
             mdbx_version.git.datetime, mdbx_version.git.commit, mdbx_version.git.tree, mdbx_sourcery_anchor,
             mdbx_build.datetime, mdbx_build.target, mdbx_build.compiler, mdbx_build.flags, mdbx_build.options);
      return EXIT_SUCCESS;
    case 'l':
      list = true;
      /*FALLTHROUGH*/;
      __fallthrough;
    case 'a':
      if (subname)
        usage();
      alldbs = true;
      break;
    case 'f':
      if (freopen(optarg, "w", stdout) == nullptr) {
        fprintf(stderr, "%s: %s: reopen: %s\n", prog, optarg, mdbx_strerror(errno));
        exit(EXIT_FAILURE);
      }
      break;
    case 'n':
      break;
    case 'c':
      mode |= CONCISE;
      break;
    case 'p':
      mode |= PRINT;
      break;
    case 's':
      if (alldbs)
        usage();
      subname = optarg;
      break;
    case 'q':
      quiet = true;
      break;
    case 'r':
      rescue = true;
      break;
    case 'u':
      warmup = true;
      break;
    case 'U':
      warmup = true;
      warmup_flags = MDBX_warmup_force | MDBX_warmup_touchlimit | MDBX_warmup_lock;
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
    fprintf(stderr, "mdbx_dump %s (%s, T-%s)\nRunning for %s...\n", mdbx_version.git.describe,
            mdbx_version.git.datetime, mdbx_version.git.tree, envname);
    fflush(nullptr);
    mdbx_setup_debug(MDBX_LOG_NOTICE, MDBX_DBG_DONTCHANGE, logger);
  }

  err = mdbx_env_create(&env);
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_env_create", err);
    return EXIT_FAILURE;
  }

  if (alldbs || subname) {
    err = mdbx_env_set_maxdbs(env, 2);
    if (unlikely(err != MDBX_SUCCESS)) {
      error("mdbx_env_set_maxdbs", err);
      goto env_close;
    }
  }

  err = mdbx_env_open(env, envname, envflags | (rescue ? MDBX_RDONLY | MDBX_EXCLUSIVE | MDBX_VALIDATION : MDBX_RDONLY),
                      0);
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_env_open", err);
    goto env_close;
  }

  if (warmup) {
    err = mdbx_env_warmup(env, nullptr, warmup_flags, 3600 * 65536);
    if (MDBX_IS_ERROR(err)) {
      error("mdbx_env_warmup", err);
      goto env_close;
    }
  }

  err = mdbx_txn_begin(env, nullptr, MDBX_TXN_RDONLY, &txn);
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_txn_begin", err);
    goto env_close;
  }

  err = mdbx_dbi_open(txn, subname, MDBX_DB_ACCEDE, &dbi);
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_dbi_open", err);
    goto txn_abort;
  }

  if (alldbs) {
    ASSERT(dbi == MAIN_DBI);

    MDBX_cursor *cursor;
    err = mdbx_cursor_open(txn, MAIN_DBI, &cursor);
    if (unlikely(err != MDBX_SUCCESS)) {
      error("mdbx_cursor_open", err);
      goto txn_abort;
    }
    if (rescue) {
      err = mdbx_cursor_ignord(cursor);
      if (unlikely(err != MDBX_SUCCESS))
        error("mdbx_cursor_ignord", err);
    }

    bool have_raw = false;
    int count = 0;
    MDBX_val key;
    while (MDBX_SUCCESS == (err = mdbx_cursor_get(cursor, &key, nullptr, MDBX_NEXT_NODUP))) {
      if (user_break) {
        err = MDBX_EINTR;
        break;
      }

      if (memchr(key.iov_base, '\0', key.iov_len))
        continue;
      subname = osal_realloc(buf4free, key.iov_len + 1);
      if (!subname) {
        err = MDBX_ENOMEM;
        break;
      }

      buf4free = subname;
      memcpy(subname, key.iov_base, key.iov_len);
      subname[key.iov_len] = '\0';

      MDBX_dbi sub_dbi;
      err = mdbx_dbi_open_ex(txn, subname, MDBX_DB_ACCEDE, &sub_dbi, rescue ? equal_or_greater : nullptr,
                             rescue ? equal_or_greater : nullptr);
      if (unlikely(err != MDBX_SUCCESS)) {
        if (err == MDBX_INCOMPATIBLE) {
          have_raw = true;
          continue;
        }
        error("mdbx_dbi_open", err);
        if (!rescue)
          break;
      } else {
        count++;
        if (list) {
          printf("%s\n", subname);
        } else {
          err = dump_tbl(txn, sub_dbi, subname);
          if (unlikely(err != MDBX_SUCCESS)) {
            if (!rescue)
              break;
            if (!quiet)
              fprintf(stderr, "%s: %s: ignore %s for `%s` and continue\n", prog, envname, mdbx_strerror(err), subname);
            /* Here is a hack for rescue mode, don't do that:
             *  - we should restart transaction in case error due
             *    database corruption;
             *  - but we won't close cursor, reopen and re-positioning it
             *    for new a transaction;
             *  - this is possible since DB is opened in read-only exclusive
             *    mode and transaction is the same, i.e. has the same address
             *    and so on. */
            err = mdbx_txn_reset(txn);
            if (unlikely(err != MDBX_SUCCESS)) {
              error("mdbx_txn_reset", err);
              goto env_close;
            }
            err = mdbx_txn_renew(txn);
            if (unlikely(err != MDBX_SUCCESS)) {
              error("mdbx_txn_renew", err);
              goto env_close;
            }
          }
        }
        err = mdbx_dbi_close(env, sub_dbi);
        if (unlikely(err != MDBX_SUCCESS)) {
          error("mdbx_dbi_close", err);
          break;
        }
      }
    }
    mdbx_cursor_close(cursor);
    cursor = nullptr;

    if (have_raw && (!count /* || rescue */))
      err = dump_tbl(txn, MAIN_DBI, nullptr);
    else if (!count) {
      if (!quiet)
        fprintf(stderr, "%s: %s does not contain multiple databases\n", prog, envname);
      err = MDBX_NOTFOUND;
    }
  } else {
    err = dump_tbl(txn, dbi, subname);
  }

  switch (err) {
  case MDBX_NOTFOUND:
    err = MDBX_SUCCESS;
  case MDBX_SUCCESS:
    break;
  case MDBX_EINTR:
    if (!quiet)
      fprintf(stderr, "Interrupted by signal/user\n");
    break;
  default:
    if (unlikely(err != MDBX_SUCCESS))
      error("mdbx_cursor_get", err);
  }

  mdbx_dbi_close(env, dbi);
txn_abort:
  mdbx_txn_abort(txn);
env_close:
  mdbx_env_close(env);
  free(buf4free);

  return err ? EXIT_FAILURE : EXIT_SUCCESS;
}
