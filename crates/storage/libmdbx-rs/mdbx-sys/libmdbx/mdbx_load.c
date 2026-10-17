/* This file is part of the libmdbx amalgamated source code (v0.14.4-0-g716ce9d5 at 2026-09-18T22:53:42+03:00).
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

#if IS_WINDOWS

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

static char *prog;
static bool quiet = false;
static size_t lineno;
static void error(const char *func, int rc) {
  if (!quiet) {
    if (lineno)
      fprintf(stderr, "%s: at input line %" PRIiSIZE ": %s() error %d, %s\n", prog, lineno, func, rc,
              mdbx_strerror(rc));
    else
      fprintf(stderr, "%s: %s() error %d %s\n", prog, func, rc, mdbx_strerror(rc));
  }
}

static void logger(MDBX_log_level_t level, const char *function, int line, const char *fmt, va_list args) {
  static const char *const prefixes[] = {
      "!!!fatal: ", // 0 fatal
      " ! ",        // 1 error
      " ~ ",        // 2 warning
      "   ",        // 3 notice
      "   //",      // 4 verbose
  };
  if (level >= 0 && level < MDBX_LOG_DEBUG) {
    if (function && line && (size_t)level < ARRAY_LENGTH(prefixes))
      fprintf(stderr, "%s", prefixes[level]);
    vfprintf(stderr, fmt, args);
  }
}

static char *valstr(char *line, const char *item) {
  const size_t len = strlen(item);
  if (strncmp(line, item, len) != 0)
    return nullptr;
  if (line[len] != '=') {
    if (line[len] > ' ')
      return nullptr;
    if (!quiet)
      fprintf(stderr, "%s: line %" PRIiSIZE ": unexpected line format for '%s'\n", prog, lineno, item);
    exit(EXIT_FAILURE);
  }
  char *ptr = strchr(line, '\n');
  if (ptr)
    *ptr = '\0';
  return line + len + 1;
}

static bool valnum(char *line, const char *item, uint64_t *value) {
  char *str = valstr(line, item);
  if (!str)
    return false;

  char *end = nullptr;
  *value = strtoull(str, &end, 0);
  if (end && *end) {
    if (!quiet)
      fprintf(stderr, "%s: line %" PRIiSIZE ": unexpected number format for '%s'\n", prog, lineno, item);
    exit(EXIT_FAILURE);
  }
  return true;
}

static bool valbool(char *line, const char *item, bool *value) {
  uint64_t u64;
  if (!valnum(line, item, &u64))
    return false;

  if (u64 > 1) {
    if (!quiet)
      fprintf(stderr, "%s: line %" PRIiSIZE ": unexpected value for '%s'\n", prog, lineno, item);
    exit(EXIT_FAILURE);
  }
  *value = u64 != 0;
  return true;
}

/*----------------------------------------------------------------------------*/

static char *subname = nullptr;
static int dbi_flags;
static uint64_t sequence;
static MDBX_canary canary;
static MDBX_envinfo envinfo;

#define PLAINTEXT 1
#define NOHDR 2
#define GLOBAL 4
static int mode = GLOBAL;

static MDBX_val key_buf, data_buf;

#define STRLENOF(s) (sizeof(s) - 1)

typedef struct flagbit {
  unsigned bit;
  unsigned len;
  char *name;
} flagbit;

#define S(s) STRLENOF(s), s

static const flagbit dbflags[] = {{MDBX_REVERSEKEY, S("reversekey")}, {MDBX_DUPSORT, S("duplicates")},
                                  {MDBX_DUPSORT, S("dupsort")},       {MDBX_INTEGERKEY, S("integerkey")},
                                  {MDBX_DUPFIXED, S("dupfix")},       {MDBX_INTEGERDUP, S("integerdup")},
                                  {MDBX_REVERSEDUP, S("reversedup")}, {0, 0, nullptr}};

static int readhdr(void) {
  /* reset parameters */
  if (subname) {
    free(subname);
    subname = nullptr;
  }
  dbi_flags = 0;
  sequence = 0;

  while (true) {
    errno = 0;
    if (fgets(data_buf.iov_base, (int)data_buf.iov_len, stdin) == nullptr)
      return errno ? errno : EOF;
    if (user_break)
      return MDBX_EINTR;

    lineno++;
    uint64_t u64;

    if (valnum(data_buf.iov_base, "VERSION", &u64)) {
      if (u64 != 3) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": unsupported value %" PRIu64 " for %s\n", prog, lineno, u64,
                  "VERSION");
        exit(EXIT_FAILURE);
      }
      continue;
    }

    if (valnum(data_buf.iov_base, "db_pagesize", &u64)) {
      if (!(mode & GLOBAL) && envinfo.mi_dxb_pagesize != u64) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": ignore value %" PRIu64 " for '%s' in non-global context\n", prog,
                  lineno, u64, "db_pagesize");
      } else if (u64 < MDBX_MIN_PAGESIZE || u64 > MDBX_MAX_PAGESIZE) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": ignore unsupported value %" PRIu64 " for %s\n", prog, lineno, u64,
                  "db_pagesize");
      } else
        envinfo.mi_dxb_pagesize = (uint32_t)u64;
      continue;
    }

    char *str = valstr(data_buf.iov_base, "format");
    if (str) {
      if (strcmp(str, "print") == 0) {
        mode |= PLAINTEXT;
        continue;
      }
      if (strcmp(str, "bytevalue") == 0) {
        mode &= ~PLAINTEXT;
        continue;
      }
      if (!quiet)
        fprintf(stderr, "%s: line %" PRIiSIZE ": unsupported value '%s' for %s\n", prog, lineno, str, "format");
      exit(EXIT_FAILURE);
    }

    str = valstr(data_buf.iov_base, "database");
    if (str) {
      if (*str) {
        free(subname);
        subname = osal_strdup(str);
        if (!subname) {
          if (!quiet)
            perror("strdup()");
          exit(EXIT_FAILURE);
        }
      }
      continue;
    }

    str = valstr(data_buf.iov_base, "type");
    if (str) {
      if (strcmp(str, "btree") != 0) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": unsupported value '%s' for %s\n", prog, lineno, str, "type");
        free(subname);
        exit(EXIT_FAILURE);
      }
      continue;
    }

    if (valnum(data_buf.iov_base, "mapaddr", &u64)) {
      if (u64) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": ignore unsupported value 0x%" PRIx64 " for %s\n", prog, lineno, u64,
                  "mapaddr");
      }
      continue;
    }

    if (valnum(data_buf.iov_base, "mapsize", &u64)) {
      if (!(mode & GLOBAL)) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": ignore value %" PRIu64 " for '%s' in non-global context\n", prog,
                  lineno, u64, "mapsize");
      } else if (u64 < MIN_MAPSIZE || u64 > MAX_MAPSIZE64) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": ignore unsupported value 0x%" PRIx64 " for %s\n", prog, lineno, u64,
                  "mapsize");
      } else
        envinfo.mi_mapsize = (size_t)u64;
      continue;
    }

    if (valnum(data_buf.iov_base, "maxreaders", &u64)) {
      if (!(mode & GLOBAL)) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": ignore value %" PRIu64 " for '%s' in non-global context\n", prog,
                  lineno, u64, "maxreaders");
      } else if (u64 < 1 || u64 > MDBX_READERS_LIMIT) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": ignore unsupported value 0x%" PRIx64 " for %s\n", prog, lineno, u64,
                  "maxreaders");
      } else
        envinfo.mi_maxreaders = (int)u64;
      continue;
    }

    if (valnum(data_buf.iov_base, "txnid", &u64)) {
      if (u64 < MIN_TXNID || u64 > MAX_TXNID) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": ignore unsupported value 0x%" PRIx64 " for %s\n", prog, lineno, u64,
                  "txnid");
      }
      continue;
    }

    if (valnum(data_buf.iov_base, "sequence", &u64)) {
      sequence = u64;
      continue;
    }

    str = valstr(data_buf.iov_base, "geometry");
    if (str) {
      if (!(mode & GLOBAL)) {
        if (!quiet)
          fprintf(stderr,
                  "%s: line %" PRIiSIZE ": ignore values %s"
                  " for '%s' in non-global context\n",
                  prog, lineno, str, "geometry");
      } else if (sscanf(str, "l%" PRIu64 ",u%" PRIu64 ",s%" PRIu64 ",g%" PRIu64, &envinfo.mi_geo.lower,
                        &envinfo.mi_geo.upper, &envinfo.mi_geo.shrink, &envinfo.mi_geo.grow) == 4) {
        envinfo.mi_geo.current = (uint64_t)INT64_C(-1);
      } else if (sscanf(str, "l%" PRIu64 ",c%" PRIu64 ",u%" PRIu64 ",s%" PRIu64 ",g%" PRIu64, &envinfo.mi_geo.lower,
                        &envinfo.mi_geo.current, &envinfo.mi_geo.upper, &envinfo.mi_geo.shrink,
                        &envinfo.mi_geo.grow) != 5) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": unexpected line format for '%s'\n", prog, lineno, "geometry");
        exit(EXIT_FAILURE);
      }
      continue;
    }

    str = valstr(data_buf.iov_base, "canary");
    if (str) {
      if (!(mode & GLOBAL)) {
        if (!quiet)
          fprintf(stderr,
                  "%s: line %" PRIiSIZE ": ignore values %s"
                  " for '%s' in non-global context\n",
                  prog, lineno, str, "canary");
      } else if (sscanf(str, "v%" PRIu64 ",x%" PRIu64 ",y%" PRIu64 ",z%" PRIu64, &canary.v, &canary.x, &canary.y,
                        &canary.z) != 4) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": unexpected line format for '%s'\n", prog, lineno, "canary");
        exit(EXIT_FAILURE);
      }
      continue;
    }

    for (int i = 0; dbflags[i].bit; i++) {
      bool value = false;
      if (valbool(data_buf.iov_base, dbflags[i].name, &value)) {
        if (value)
          dbi_flags |= dbflags[i].bit;
        else
          dbi_flags &= ~dbflags[i].bit;
        goto next;
      }
    }

    str = valstr(data_buf.iov_base, "HEADER");
    if (str) {
      if (strcmp(str, "END") == 0)
        return MDBX_SUCCESS;
    }

    if (!quiet)
      fprintf(stderr, "%s: line %" PRIiSIZE ": unrecognized keyword ignored: %s\n", prog, lineno,
              (char *)data_buf.iov_base);
  next:;
  }
  return EOF;
}

static int badend(void) {
  if (!quiet)
    fprintf(stderr, "%s: line %" PRIiSIZE ": unexpected end of input\n", prog, lineno);
  return errno ? errno : MDBX_ENODATA;
}

static inline uint8_t unhex(const char *hex) {
  int8_t hi = hex[0];
  hi = (hi | 0x20) - 'a';
  hi += 10 + ((hi >> 7) & 39);

  int8_t lo = hex[1];
  lo = (lo | 0x20) - 'a';
  lo += 10 + ((lo >> 7) & 39);

  return (uint8_t)(hi << 4 | lo);
}

__hot static int readline(MDBX_val *out, MDBX_val *buf) {
  if (user_break)
    return MDBX_EINTR;

  errno = 0;
  if (!(mode & NOHDR)) {
    int c = fgetc(stdin);
    if (c == EOF)
      return errno ? errno : EOF;
    if (c != ' ') {
      lineno++;
      if (fgets(buf->iov_base, (int)buf->iov_len, stdin)) {
        if (c == 'D' && !strncmp(buf->iov_base, "ATA=END", STRLENOF("ATA=END")))
          return EOF;
      }
      return badend();
    }
  }

  /* modern concise mode, where space in second position mean the same (previously) value */
  int c = fgetc(stdin);
  if (c == EOF)
    return errno ? errno : EOF;
  if (c == ' ')
    return (ungetc(c, stdin) == c) ? MDBX_SUCCESS : (errno ? errno : EOF);

  char *line = buf->iov_base;
  line[0] = (char)c;
  line[1] = 0;
  if (c != '\n' && fgets(line + 1, (int)buf->iov_len - 1, stdin) == nullptr)
    return errno ? errno : EOF;
  lineno++;

  size_t len = strlen(line);
  if (len > 0) {
    /* Is buffer too short? */
    while (line[len - 1] != '\n') {
      /* double buffer size */
      line = osal_realloc(buf->iov_base, buf->iov_len * 2);
      if (!line) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": out of memory, line too long\n", prog, lineno);
        return MDBX_ENOMEM;
      }
      buf->iov_base = line;
      buf->iov_len *= 2;

      /* continue read line */
      errno = 0;
      if (fgets(line + len, (int)(buf->iov_len - len), stdin) == nullptr)
        return errno ? errno : EOF;
      len += strlen(line + len);
    }
    /* strip '\n' at the end */
    line[--len] = '\0';
  }

  char *w = line, *r = line;
  char *const end = r + len;
  if (mode & PLAINTEXT) {
    while (r < end) {
      if (unlikely(r[0] == '\\')) {
        if (r[1] == '\\') {
          *w++ = '\\';
        } else {
          if (r + 3 > end || !isxdigit(r[1]) || !isxdigit(r[2]))
            return badend();
          *w++ = unhex(++r);
        }
        r += 2;
      } else {
        /* copies are redundant when no escapes were used */
        *w++ = *r++;
      }
    }
  } else {
    /* odd length not allowed */
    if (len & 1)
      return badend();
    while (r < end) {
      if (!isxdigit(r[0]) || !isxdigit(r[1]))
        return badend();
      *w++ = unhex(r);
      r += 2;
    }
  }

  out->iov_base = line;
  out->iov_len = w - line;
  return MDBX_SUCCESS;
}

static void usage(void) {
  fprintf(stderr,
          "usage: %s "
          "[-V] [-q] [-a] [-f file] [-s name] [-N] [-p] [-T] [-r] [-n] dbpath\n"
          "  -V\t\tprint version and exit.\n"
          "  -q\t\tbe quiet.\n"
          "  -a\t\tappend records in input order (required for custom comparators).\n"
          "  -b number\tinsertion batch size as number of items (100K by default).\n"
          "  -L megabytes\tlimits the amount of transactions in megabytes.\n"
          "  -d percent\tdesired pages filling density in percent between 50 and 100 (100 by default).\n"
          "  -G geometry\toverride database geometry in the form of five numbers L:U:G:S:P delimited by a colon,\n"
          "\t\twhere:\n"
          "\t\t  L - lower/minimal database size in bytes;\n"
          "\t\t  U - upper/maximal database size in bytes;\n"
          "\t\t  G - growth step in bytes;\n"
          "\t\t  S - shrink threshold in bytes;\n"
          "\t\t  P - page size in bytes;\n"
          "\t\tsee description of mdbx_env_set_geometry() for more information.\n"
          "  -f file\tread from file instead of stdin.\n"
          "  -s name\tload into specified named table.\n"
          "  -N\t\tdon't overwrite existing records when loading, just skip ones.\n"
          "  -p\t\tpurge target table(s) before loading.\n"
          "  -T\t\tread plaintext.\n"
          "  -r\t\trescue mode (ignore errors to load corrupted DB dump).\n"
          "  -n\t\tdon't use subdirectory for newly created database (MDBX_NOSUBDIR).\n",
          prog);
  exit(EXIT_FAILURE);
}

static int equal_or_greater(const MDBX_val *a, const MDBX_val *b) {
  return (a->iov_len == b->iov_len && memcmp(a->iov_base, b->iov_base, a->iov_len) == 0) ? 0 : 1;
}

int main(int argc, char *argv[]) {
  int i, err;
  MDBX_env *env = nullptr;
  MDBX_txn *txn = nullptr;
  MDBX_cursor *mc = nullptr;
  MDBX_dbi dbi;
  char *envname = nullptr;
  int envflags = MDBX_SAFE_NOSYNC | MDBX_ACCEDE, putflags = MDBX_UPSERT;
  bool rescue = false;
  bool purge = false;
  unsigned density_percent = 100;
  bool override_geometry = false;
  intptr_t geometry_pagesize = -1;
  intptr_t geometry_lower = -1;
  intptr_t geometry_upper = -1;
  intptr_t geometry_growth = -1;
  intptr_t geometry_shrink = -1;
  size_t batch_items = 100000;
  size_t dirty_limit = 0;

  prog = argv[0];
  if (argc < 2)
    usage();

  while ((i = getopt(argc, argv,
                     "a"
                     "b:"
                     "L:"
                     "d:"
                     "G:"
                     "f:"
                     "n"
                     "s:"
                     "N"
                     "p"
                     "T"
                     "V"
                     "r"
                     "q")) != EOF) {
    switch (i) {
    case 'V':
      printf("mdbx_load version %d.%d.%d.%d\n"
             " - source: %s %s, commit %s, tree %s\n"
             " - anchor: %s\n"
             " - build: %s for %s by %s\n"
             " - flags: %s\n"
             " - options: %s\n",
             mdbx_version.major, mdbx_version.minor, mdbx_version.patch, mdbx_version.tweak, mdbx_version.git.describe,
             mdbx_version.git.datetime, mdbx_version.git.commit, mdbx_version.git.tree, mdbx_sourcery_anchor,
             mdbx_build.datetime, mdbx_build.target, mdbx_build.compiler, mdbx_build.flags, mdbx_build.options);
      return EXIT_SUCCESS;
    case 'a':
      putflags |= MDBX_APPEND;
      break;
    case 'f':
      if (freopen(optarg, "r", stdin) == nullptr) {
        if (!quiet)
          fprintf(stderr, "%s: %s: open: %s\n", prog, optarg, mdbx_strerror(errno));
        return EXIT_FAILURE;
      }
      break;
    case 'n':
      envflags |= MDBX_NOSUBDIR;
      break;
    case 's':
      subname = osal_strdup(optarg);
      break;
    case 'N':
      putflags |= MDBX_NOOVERWRITE | MDBX_NODUPDATA;
      break;
    case 'p':
      purge = true;
      break;
    case 'T':
      mode |= NOHDR | PLAINTEXT;
      break;
    case 'q':
      quiet = true;
      break;
    case 'r':
      rescue = true;
      break;
    case 'b':
      if (sscanf(optarg, "%zu", &batch_items) != 1) {
        if (!quiet)
          fprintf(stderr, "%s: %s option: expecting %s, but got '%s'\n", prog, "-b", "unsigned integer value", optarg);
        return EXIT_FAILURE;
      }
      break;
    case 'L':
      if (sscanf(optarg, "%zu", &dirty_limit) != 1) {
        if (!quiet)
          fprintf(stderr, "%s: %s option: expecting %s, but got '%s'\n", prog, "-L", "unsigned integer value", optarg);
        return EXIT_FAILURE;
      }
      break;
    case 'd':
      if (sscanf(optarg, "%u", &density_percent) != 1 || density_percent < 50 || density_percent > 100) {
        if (!quiet)
          fprintf(stderr, "%s: %s option: expecting %s, but got '%s'\n", prog, "-d",
                  "unsigned integer value in range between 50 and 100", optarg);
        return EXIT_FAILURE;
      }
      break;
    case 'G':
      if (sscanf(optarg, "%zi:%zi:%zi:%zi:%zi", &geometry_lower, &geometry_upper, &geometry_growth, &geometry_shrink,
                 &geometry_pagesize) != 5) {
        if (!quiet)
          fprintf(stderr, "%s: %s option: expecting %s, but got '%s'\n", prog, "-G",
                  "five numbers delimited by a colon", optarg);
        return EXIT_FAILURE;
      }
      override_geometry = true;
      break;
    default:
      usage();
    }
  }

  if (optind != argc - 1)
    usage();

#if IS_WINDOWS
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
    printf("mdbx_load %s (%s, T-%s)\nRunning for %s...\n", mdbx_version.git.describe, mdbx_version.git.datetime,
           mdbx_version.git.tree, envname);
    fflush(nullptr);
    mdbx_setup_debug(MDBX_LOG_NOTICE, MDBX_DBG_DONTCHANGE, logger);
  }

  data_buf.iov_len = 4096;
  data_buf.iov_base = osal_malloc(data_buf.iov_len);
  if (!data_buf.iov_base) {
    err = MDBX_ENOMEM;
    error("value-buffer", err);
    goto bailout;
  }

  /* read first header for mapsize= */
  if (!(mode & NOHDR)) {
    err = readhdr();
    if (unlikely(err != MDBX_SUCCESS)) {
      if (err == EOF)
        err = MDBX_ENODATA;
      error("readheader", err);
      goto bailout;
    }
  }

  err = mdbx_env_create(&env);
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_env_create", err);
    goto bailout;
  }

  err = mdbx_env_set_maxdbs(env, 2);
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_env_set_maxdbs", err);
    goto bailout;
  }

  if (envinfo.mi_maxreaders) {
    err = mdbx_env_set_maxreaders(env, envinfo.mi_maxreaders);
    if (unlikely(err != MDBX_SUCCESS)) {
      error("mdbx_env_set_maxreaders", err);
      goto bailout;
    }
  }

  if (override_geometry) {
    err = mdbx_env_set_geometry(env, geometry_lower, -1, geometry_upper, geometry_growth, geometry_shrink,
                                geometry_pagesize);
  } else {
    err = MDBX_SUCCESS;
    if (envinfo.mi_geo.lower | envinfo.mi_geo.upper | envinfo.mi_geo.shrink | envinfo.mi_geo.grow) {
      err = mdbx_env_set_geometry(env, (intptr_t)envinfo.mi_geo.lower, (intptr_t)envinfo.mi_geo.current,
                                  (intptr_t)envinfo.mi_geo.upper, (intptr_t)envinfo.mi_geo.grow,
                                  (intptr_t)envinfo.mi_geo.shrink,
                                  envinfo.mi_dxb_pagesize ? (intptr_t)envinfo.mi_dxb_pagesize : -1);
    } else if (envinfo.mi_mapsize) {
      const size_t mmap_limit =
          mdbx_limits_dbsize_max(envinfo.mi_dxb_pagesize ? (intptr_t)envinfo.mi_dxb_pagesize : -1);
      if (envinfo.mi_mapsize > mmap_limit) {
        err = MDBX_TOO_LARGE;
        if (!quiet)
          fprintf(stderr,
                  "Database size is too large for current system (mapsize=%" PRIu64
                  " is greater than current system-limit %zu)\n",
                  envinfo.mi_mapsize, mmap_limit);
        goto bailout;
      }
      err = mdbx_env_set_geometry(env, (intptr_t)envinfo.mi_mapsize, (intptr_t)envinfo.mi_mapsize,
                                  (intptr_t)envinfo.mi_mapsize, 0, 0,
                                  envinfo.mi_dxb_pagesize ? (intptr_t)envinfo.mi_dxb_pagesize : -1);
    }
  }
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_env_set_geometry", err);
    goto bailout;
  }

  err = mdbx_env_open(env, envname, envflags, 0664);
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_env_open", err);
    goto bailout;
  }

  err = mdbx_env_set_option(env, MDBX_opt_split_reserve, 65536u * (100u - density_percent) / 100u);
  if (unlikely(err != MDBX_SUCCESS)) {
    error("mdbx_env_set_option.split_reserve", err);
    goto bailout;
  }

  key_buf.iov_len = mdbx_env_get_maxkeysize_ex(env, 0) + (size_t)1;
  if (key_buf.iov_len >= INTPTR_MAX / 2) {
    err = MDBX_PROBLEM;
    if (!quiet)
      fprintf(stderr, "mdbx_env_get_maxkeysize_ex() failed, returns %zu\n", key_buf.iov_len);
    goto bailout;
  }

  key_buf.iov_base = malloc(key_buf.iov_len);
  if (!key_buf.iov_base) {
    err = MDBX_ENOMEM;
    error("key-buffer", err);
    goto bailout;
  }

  dirty_limit = (dirty_limit > SIZE_MAX >> 20) ? /* unlimited */ 0 : dirty_limit << /* megabytes to bytes */ 20;
  while (err == MDBX_SUCCESS) {
    if (user_break) {
      err = MDBX_EINTR;
      break;
    }

    err = mdbx_txn_begin(env, nullptr, 0, &txn);
    if (unlikely(err != MDBX_SUCCESS)) {
      error("mdbx_txn_begin", err);
      goto bailout;
    }

    if (mode & GLOBAL) {
      mode -= GLOBAL;
      if (canary.v | canary.x | canary.y | canary.z) {
        err = mdbx_canary_put(txn, &canary);
        if (unlikely(err != MDBX_SUCCESS)) {
          error("mdbx_canary_put", err);
          goto bailout;
        }
      }
    }

    const char *const dbi_name = subname ? subname : "@MAIN";
    err = mdbx_dbi_open_ex(txn, subname, dbi_flags | MDBX_CREATE, &dbi,
                           (putflags & MDBX_APPEND) ? equal_or_greater : nullptr,
                           (putflags & MDBX_APPEND) ? equal_or_greater : nullptr);
    if (unlikely(err != MDBX_SUCCESS)) {
      error("mdbx_dbi_open_ex", err);
      goto bailout;
    }

    uint64_t present_sequence;
    err = mdbx_dbi_sequence(txn, dbi, &present_sequence, 0);
    if (unlikely(err != MDBX_SUCCESS)) {
      error("mdbx_dbi_sequence", err);
      goto bailout;
    }
    if (present_sequence > sequence) {
      if (!quiet)
        fprintf(stderr, "present sequence for '%s' value (%" PRIu64 ") is greater than loaded (%" PRIu64 ")\n",
                dbi_name, present_sequence, sequence);
      err = MDBX_RESULT_TRUE;
      goto bailout;
    }
    if (present_sequence < sequence) {
      err = mdbx_dbi_sequence(txn, dbi, nullptr, sequence - present_sequence);
      if (unlikely(err != MDBX_SUCCESS)) {
        error("mdbx_dbi_sequence", err);
        goto bailout;
      }
    }

    if (purge) {
      err = mdbx_drop(txn, dbi, false);
      if (unlikely(err != MDBX_SUCCESS)) {
        error("mdbx_drop", err);
        goto bailout;
      }
    }

    if (putflags & MDBX_APPEND)
      putflags = (dbi_flags & MDBX_DUPSORT) ? putflags | MDBX_APPENDDUP : putflags & ~MDBX_APPENDDUP;

    err = mdbx_cursor_open(txn, dbi, &mc);
    if (unlikely(err != MDBX_SUCCESS)) {
      error("mdbx_cursor_open", err);
      goto bailout;
    }

    size_t count = 0;
    MDBX_val key = {.iov_base = nullptr, .iov_len = 0}, data = {.iov_base = nullptr, .iov_len = 0};
    while (err == MDBX_SUCCESS) {
      err = readline(&key, &key_buf);
      if (err == EOF)
        break;
      if (err) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": failed to read %s\n", prog, lineno, "key");
        goto bailout;
      }

      err = readline(&data, &data_buf);
      if (err) {
        if (!quiet)
          fprintf(stderr, "%s: line %" PRIiSIZE ": failed to read %s\n", prog, lineno, "value");
        goto bailout;
      }

      err = mdbx_cursor_put(mc, &key, &data, putflags);
      if (err == MDBX_KEYEXIST && putflags)
        continue;
      if (err == MDBX_BAD_VALSIZE && rescue) {
        if (!quiet)
          fprintf(stderr, "%s: skip line %" PRIiSIZE ": due to %s\n", prog, lineno, mdbx_strerror(err));
        continue;
      }
      if (unlikely(err != MDBX_SUCCESS)) {
        error("mdbx_cursor_put", err);
        goto bailout;
      }
      count++;

      bool should_checkpoint = batch_items && count >= batch_items;
      if (!should_checkpoint && (dirty_limit || (count % 256) == 0)) {
        MDBX_txn_info txn_info;
        err = mdbx_txn_info(txn, &txn_info, false);
        if (unlikely(err != MDBX_SUCCESS)) {
          error("mdbx_txn_info", err);
          goto bailout;
        }
        should_checkpoint =
            (dirty_limit && txn_info.txn_space_dirty >= dirty_limit) || txn_info.txn_space_leftover < 42 * MEGABYTE;
      }

      if (should_checkpoint) {
        err = mdbx_txn_checkpoint(txn, MDBX_TXN_NOMETASYNC, nullptr);
        if (unlikely(err != MDBX_SUCCESS)) {
          error("mdbx_txn_checkpoint", err);
          goto bailout;
        }
        count = 0;

        err = mdbx_cursor_bind(txn, mc, dbi);
        if (unlikely(err != MDBX_SUCCESS)) {
          error("mdbx_cursor_bind", err);
          goto bailout;
        }
      }
    }

    mdbx_cursor_close(mc);
    mc = nullptr;
    err = mdbx_txn_commit(txn);
    txn = nullptr;
    if (unlikely(err != MDBX_SUCCESS)) {
      error("mdbx_txn_commit", err);
      goto bailout;
    }
    if (subname) {
      ASSERT(dbi != MAIN_DBI);
      err = mdbx_dbi_close(env, dbi);
      if (unlikely(err != MDBX_SUCCESS)) {
        error("mdbx_dbi_close", err);
        goto bailout;
      }
    } else {
      ASSERT(dbi == MAIN_DBI);
    }

    /* try read next header */
    if (!(mode & NOHDR))
      err = readhdr();
    else if (ferror(stdin) || feof(stdin))
      break;
  }

  switch (err) {
  case EOF:
    err = MDBX_SUCCESS;
    /* FALLTHROUGH */
  case MDBX_SUCCESS:
    break;
  case MDBX_EINTR:
    if (!quiet)
      fprintf(stderr, "Interrupted by signal/user\n");
    break;
  default:
    error("readline", err);
  }

bailout:
  if (mc)
    mdbx_cursor_close(mc);
  if (txn)
    mdbx_txn_abort(txn);
  if (env)
    mdbx_env_close(env);
  free(key_buf.iov_base);
  free(data_buf.iov_base);

  return err ? EXIT_FAILURE : EXIT_SUCCESS;
}
