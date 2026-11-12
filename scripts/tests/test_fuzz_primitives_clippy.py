#!/usr/bin/env python3
"""Real Clippy policy tests, isolated from the reth workspace build.

Run this file explicitly, or opt into unittest discovery with
RETH_RUN_CLIPPY_TESTS=1. Requires Cargo with Clippy and dependency access.
RETH_FUZZ_PRIMITIVES_TOOLCHAIN selects an optional rustup toolchain;
RETH_FUZZ_PRIMITIVES_TARGET_DIR selects a dedicated reusable build cache.
No fixture executable is run: these tests exercise the compiler/linter boundary.
"""

import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[2]
LINTS = tuple(f"clippy::disallowed_{kind}" for kind in ("types", "methods", "macros"))

# Each entry names the exact compiler diagnostic required, not a text grep.
CASES = {
    "local_macro": ("types", """
        macro_rules! make_native { () => { std::sync::Mutex::new(()) }; }

        fn main() { let _first = make_native!(); let _second = make_native!(); }
    """),
    "native_atomic": ("types", """
        fn main() { let _flag = std::sync::atomic::AtomicBool::new(false); }
    """),
    "std_alias": ("types", """
        use std::sync::Mutex as NativeMutex;

        fn main() { let _: Option<NativeMutex<()>> = None; }
    """),
    "std_type_alias": ("types", """
        type HiddenMutex = std::sync::Mutex<()>;

        fn main() { let _: Option<HiddenMutex> = None; }
    """),
    "parking_lot_alias": ("types", """
        use parking_lot::Mutex as NativeMutex;

        fn main() { let _: Option<NativeMutex<()>> = None; }
    """),
    "renamed_spawn": ("methods", """
        use std::thread::spawn as launch;

        fn main() { let _worker = launch(|| ()); }
    """),
    "renamed_mpsc": ("methods", """
        use std::sync::mpsc::channel as make_queue;

        fn main() { let (_sender, _receiver) = make_queue::<()>(); }
    """),
    "renamed_clock": ("methods", """
        use std::time::Instant as Clock;

        fn main() { let _now = Clock::now(); }
    """),
    "std_tls": ("macros", """
        std::thread_local! { static LOCAL: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }

        fn main() { LOCAL.with(|value| value.set(1)); }
    """),
    "external_tls": ("macros", """
        tls_macro::native_tls!();

        fn main() { LOCAL.with(|value| value.set(1)); }
    """),
    "crossbeam_select": ("macros", """
        fn main() { crossbeam_channel::select! { default => {} } }
    """),
    "crossbeam_select_biased": ("macros", """
        fn main() { crossbeam_channel::select_biased! { default => {} } }
    """),
    "renamed_crossbeam": ("methods", """
        use crossbeam_channel::bounded as make_queue;

        fn main() { let (_sender, _receiver) = make_queue::<()>(1); }
    """),
    "rayon_glob_par_iter": ("methods", """
        use rayon::prelude::*;

        fn main() { let values = [1, 2]; let _iter = values.par_iter(); }
    """),
    "rayon_glob_par_sort": ("methods", """
        use rayon::prelude::*;

        fn main() { let mut values = [2, 1]; values.par_sort(); }
    """),
    "rayon_glob_par_lines": ("methods", """
        use rayon::prelude::*;

        fn main() { let _lines = "one\\ntwo".par_lines(); }
    """),
    "rayon_ufcs": ("methods", """
        fn main() {
            let mut values = [2, 1];
            <[i32] as rayon::slice::ParallelSliceMut<i32>>::par_sort(&mut values);
        }
    """),
    "renamed_rayon": ("methods", """
        use rayon::join as run_pair;

        fn main() { let _pair = run_pair(|| 1, || 2); }
    """),
    "sleep_mutation": ("methods", """
        fn main() { std::thread::sleep(std::time::Duration::ZERO); }
    """),
}

SELECTED = """
    use reth_tasks::executor::{
        channel, rayon::{self, prelude::*}, sync::{self, parking_lot}, thread, time, Runtime,
    };

    reth_tasks::executor::thread_local! {
        static LOCAL: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    }

    fn main() {
        Runtime::new().run(|| {
            let flag = sync::atomic::AtomicBool::new(false);
            flag.store(true, sync::atomic::Ordering::SeqCst);
            assert!(flag.load(sync::atomic::Ordering::SeqCst));
            let mutex = sync::Mutex::new(1);
            assert_eq!(*mutex.lock().unwrap(), 1);
            let lock = parking_lot::Mutex::new(2);
            assert_eq!(*lock.lock(), 2);
            let rwlock = parking_lot::RwLock::new(3);
            assert_eq!(*rwlock.read(), 3);
            LOCAL.with(|value| value.set(1));
            let (sender, receiver) = channel::bounded(1);
            sender.send(3).unwrap();
            channel::select! {
                recv(receiver) -> result => assert_eq!(result.unwrap(), 3),
                default => panic!("ready channel was skipped"),
            }
            let _now = time::Instant::now();
            time::sleep(time::Duration::ZERO);
            thread::spawn(|| ()).join().unwrap();
            let mut values = [2, 1];
            values.par_sort_by_key(|value| *value);
            assert_eq!(values.par_iter().map(|value| *value).sum::<i32>(), 3);
            assert_eq!(rayon::join(|| 1, || 2), (1, 2));
        });
    }
"""


@unittest.skipUnless(
    __name__ == "__main__" or os.environ.get("RETH_RUN_CLIPPY_TESTS") == "1",
    "real Clippy tests: invoke this file or set RETH_RUN_CLIPPY_TESTS=1",
)
class ClippyPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="reth-fuzz-primitives-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.fixture = Path(cls.temporary.name)
        cls.bins = cls.fixture / "src/bin"
        cls.bins.mkdir(parents=True)
        cls.sources = {name: textwrap.dedent(source).strip() + "\n"
                       for name, (_, source) in CASES.items()}
        cls.sources["selected"] = textwrap.dedent(SELECTED).strip() + "\n"
        for name, source in cls.sources.items():
            (cls.bins / f"{name}.rs").write_text(source)
        helper = cls.fixture / "tls-macro"
        (helper / "src").mkdir(parents=True)
        (helper / "Cargo.toml").write_text(textwrap.dedent('''\
            [package]
            name = "tls-macro"
            version = "0.0.0"
            edition = "2021"
        '''))
        (helper / "src/lib.rs").write_text(textwrap.dedent('''\
            #[macro_export]
            macro_rules! native_tls {
                () => {
                    ::std::thread_local! {
                        static LOCAL: ::std::cell::Cell<u8> = const { ::std::cell::Cell::new(0) };
                    }
                };
            }
        '''))
        # An explicit workspace boundary prevents a root-workspace build.
        (cls.fixture / "Cargo.toml").write_text(textwrap.dedent(f'''\
            [package]
            name = "fuzz-primitives-policy-fixture"
            version = "0.0.0"
            edition = "2021"
            publish = false

            [workspace]

            [dependencies]
            reth-tasks = {{ path = {json.dumps(str(ROOT / "crates/tasks"))}, features = ["fuzz", "rayon"] }}
            rayon = "1.7"
            parking_lot = "0.12"
            crossbeam-channel = "0.5.13"
            tls-macro = {{ path = "tls-macro" }}
        '''))
        cls.policy = cls.fixture / "policy"
        cls.normal = cls.fixture / "normal"
        cls.mutated = cls.fixture / "mutated"
        for directory in (cls.policy, cls.normal, cls.mutated):
            directory.mkdir()
        policy = (ROOT / ".config/fuzz-primitives/clippy.toml").read_text()
        (cls.policy / "clippy.toml").write_text(policy)
        shutil.copyfile(ROOT / "clippy.toml", cls.normal / "clippy.toml")
        # Delete one entry only; never weaken a whole lint or replace the policy.
        lines = policy.splitlines(keepends=True)
        removed = [line for line in lines if re.search(r'\bpath\s*=\s*"std::thread::sleep"', line)]
        if len(removed) != 1 or not removed[0].lstrip().startswith("{ path ="):
            raise AssertionError("mutation requires exactly one std::thread::sleep policy entry")
        (cls.mutated / "clippy.toml").write_text(policy.replace(removed[0], "", 1))
        cls.target = Path(os.environ.get(
            "RETH_FUZZ_PRIMITIVES_TARGET_DIR", str(cls.fixture / "target")
        )).resolve()
        cls.generation = 0

    def clippy(self, config, names):
        type(self).generation += 1
        # Cargo may replay cached diagnostics after a config-only change. Change
        # source bytes so every control/mutation really invokes the compiler.
        for name in names:
            (self.bins / f"{name}.rs").write_text(
                self.sources[name] + "\n" * self.generation
            )
        command = [os.environ.get("CARGO", "cargo")]
        if toolchain := os.environ.get("RETH_FUZZ_PRIMITIVES_TOOLCHAIN"):
            command.append("+" + toolchain.lstrip("+"))
        command += ["clippy", "--manifest-path", str(self.fixture / "Cargo.toml"),
                    "--keep-going", "--message-format=json"]
        for name in names:
            command += ["--bin", name]
        command += ["--", "-A", "warnings"]
        for lint in LINTS:
            command += ["-D", lint]
        result = subprocess.run(
            command, cwd=self.fixture, env={**os.environ,
                "CLIPPY_CONF_DIR": str(config), "CARGO_TARGET_DIR": str(self.target)},
            capture_output=True, text=True, timeout=900,
        )
        context = f"{' '.join(command)}\n{result.stderr}\n{result.stdout}"
        messages = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        finished = [item["success"] for item in messages if item["reason"] == "build-finished"]
        self.assertEqual(finished, [result.returncode == 0], context)
        diagnostics = [(item["target"]["name"], item["message"])
                       for item in messages if item["reason"] == "compiler-message"]
        # A syntax, name-resolution, or unrelated lint error is never evidence
        # that the forbidden primitive detector worked.
        unrelated = [(name, message) for name, message in diagnostics
                     if message["level"] == "error"
                     and (message.get("code") or {}).get("code") not in LINTS]
        self.assertEqual(unrelated, [], context)
        return result.returncode, diagnostics, context

    def assert_clean(self, config, names):
        code, diagnostics, context = self.clippy(config, names)
        self.assertEqual(code, 0, context)
        targeted = [message for _, message in diagnostics
                    if (message.get("code") or {}).get("code") in LINTS]
        self.assertEqual(targeted, [], context)

    def test_native_primitives_compile_under_normal_policy(self):
        self.assert_clean(self.normal, list(CASES))

    def test_selected_official_wrappers_compile_under_fuzz_policy(self):
        self.assert_clean(self.policy, ["selected"])

    def test_native_primitives_rejected_by_exact_lint(self):
        code, diagnostics, context = self.clippy(self.policy, list(CASES))
        self.assertNotEqual(code, 0, context)
        for name, (kind, _) in CASES.items():
            with self.subTest(case=name):
                matches = [message for target, message in diagnostics
                           if target == name and message["level"] == "error"
                           and (message.get("code") or {}).get("code") == f"clippy::disallowed_{kind}"]
                self.assertTrue(matches, f"missing clippy::disallowed_{kind} for {name}\n{context}")

    def test_local_macro_calls_retain_distinct_guard_locations(self):
        _, diagnostics, context = self.clippy(self.policy, ["local_macro"])
        spec = importlib.util.spec_from_file_location("guard", ROOT / "scripts/lint-fuzz-primitives.py")
        guard = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(guard)
        messages = [{"reason": "compiler-message", "message": message}
                    for _, message in diagnostics]
        messages.append({"reason": "build-finished", "success": False})
        counts, _, _ = guard.read_diagnostics((json.dumps(item) for item in messages), self.fixture)
        native_types = [count for (_, lint, source, message), count in counts.items()
                        if lint == "clippy::disallowed_types" and "make_native!" in source]
        self.assertTrue(native_types, context)
        self.assertTrue(any(count == 2 for count in native_types), (counts, context))

    def test_removing_one_policy_entry_kills_only_its_detection(self):
        code, diagnostics, context = self.clippy(self.policy, ["sleep_mutation"])
        self.assertNotEqual(code, 0, context)
        codes = {(message.get("code") or {}).get("code") for _, message in diagnostics
                 if message["level"] == "error"}
        self.assertEqual(codes, {"clippy::disallowed_methods"}, context)
        self.assert_clean(self.mutated, ["sleep_mutation"])
        # The adjacent renamed-spawn detector still fires with that same config.
        code, diagnostics, context = self.clippy(self.mutated, ["renamed_spawn"])
        self.assertNotEqual(code, 0, context)
        self.assertTrue(any((message.get("code") or {}).get("code") == "clippy::disallowed_methods"
                            for _, message in diagnostics), context)


if __name__ == "__main__":
    unittest.main()
