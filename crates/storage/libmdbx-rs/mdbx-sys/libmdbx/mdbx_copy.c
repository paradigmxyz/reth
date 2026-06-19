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

static void usage(const char *prog) {
  fprintf(stderr,
          "usage: %s [-V] [-q] [-c] [-d] [-p] [-u|U] src_path [dest_path]\n"
          "  -V\t\tprint version and exit\n"
          "  -q\t\tbe quiet\n"
          "  -c\t\tenable compactification (skip unused pages)\n"
          "  -f\t\tforce copying even the target file exists\n"
          "  -d\t\tenforce copy to be a dynamic size DB\n"
          "  -p\t\tusing transaction parking/ousting during copying MVCC-snapshot\n"
          "    \t\tto avoid stopping recycling and overflowing the DB\n"
          "  -u\t\twarmup database before copying\n"
          "  -U\t\twarmup and try lock database pages in memory before copying\n"
          "  src_path\tsource database\n"
          "  dest_path\tdestination (stdout if not specified)\n",
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

int main(int argc, char *argv[]) {
  int rc;
  MDBX_env *env = nullptr;
  const char *progname = argv[0], *act;
  unsigned flags = MDBX_RDONLY;
  unsigned cpflags = 0;
  bool quiet = false;
  bool warmup = false;
  MDBX_warmup_flags_t warmup_flags = MDBX_warmup_default;

  for (; argc > 1 && argv[1][0] == '-'; argc--, argv++) {
    if (argv[1][1] == 'n' && argv[1][2] == '\0')
      flags |= MDBX_NOSUBDIR;
    else if (argv[1][1] == 'c' && argv[1][2] == '\0')
      cpflags |= MDBX_CP_COMPACT;
    else if (argv[1][1] == 'd' && argv[1][2] == '\0')
      cpflags |= MDBX_CP_FORCE_DYNAMIC_SIZE;
    else if (argv[1][1] == 'p' && argv[1][2] == '\0')
      cpflags |= MDBX_CP_THROTTLE_MVCC;
    else if (argv[1][1] == 'f' && argv[1][2] == '\0')
      cpflags |= MDBX_CP_OVERWRITE;
    else if (argv[1][1] == 'q' && argv[1][2] == '\0')
      quiet = true;
    else if (argv[1][1] == 'u' && argv[1][2] == '\0')
      warmup = true;
    else if (argv[1][1] == 'U' && argv[1][2] == '\0') {
      warmup = true;
      warmup_flags = MDBX_warmup_force | MDBX_warmup_touchlimit | MDBX_warmup_lock;
    } else if ((argv[1][1] == 'h' && argv[1][2] == '\0') || strcmp(argv[1], "--help") == 0)
      usage(progname);
    else if (argv[1][1] == 'V' && argv[1][2] == '\0') {
      printf("mdbx_copy version %d.%d.%d.%d\n"
             " - source: %s %s, commit %s, tree %s\n"
             " - anchor: %s\n"
             " - build: %s for %s by %s\n"
             " - flags: %s\n"
             " - options: %s\n",
             mdbx_version.major, mdbx_version.minor, mdbx_version.patch, mdbx_version.tweak, mdbx_version.git.describe,
             mdbx_version.git.datetime, mdbx_version.git.commit, mdbx_version.git.tree, mdbx_sourcery_anchor,
             mdbx_build.datetime, mdbx_build.target, mdbx_build.compiler, mdbx_build.flags, mdbx_build.options);
      return EXIT_SUCCESS;
    } else
      argc = 0;
  }

  if (argc < 2 || argc > 3)
    usage(progname);

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

  if (!quiet) {
    fprintf((argc == 2) ? stderr : stdout, "mdbx_copy %s (%s, T-%s)\nRunning for copy %s to %s...\n",
            mdbx_version.git.describe, mdbx_version.git.datetime, mdbx_version.git.tree, argv[1],
            (argc == 2) ? "stdout" : argv[2]);
    fflush(nullptr);
    mdbx_setup_debug(MDBX_LOG_NOTICE, MDBX_DBG_DONTCHANGE, logger);
  }

  act = "opening environment";
  rc = mdbx_env_create(&env);
  if (rc == MDBX_SUCCESS)
    rc = mdbx_env_open(env, argv[1], flags, 0);

  if (rc == MDBX_SUCCESS && warmup) {
    act = "warming up";
    rc = mdbx_env_warmup(env, nullptr, warmup_flags, 3600 * 65536);
  }

  if (!MDBX_IS_ERROR(rc)) {
    act = "copying";
    if (argc == 2) {
      mdbx_filehandle_t fd;
#if defined(_WIN32) || defined(_WIN64)
      fd = GetStdHandle(STD_OUTPUT_HANDLE);
#else
      fd = fileno(stdout);
#endif
      rc = mdbx_env_copy2fd(env, fd, cpflags);
    } else
      rc = mdbx_env_copy(env, argv[2], cpflags);
  }
  if (rc)
    fprintf(stderr, "%s: %s failed, error %d (%s)\n", progname, act, rc, mdbx_strerror(rc));
  mdbx_env_close(env);

  return rc ? EXIT_FAILURE : EXIT_SUCCESS;
}
