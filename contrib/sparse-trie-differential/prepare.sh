#!/usr/bin/env bash
# Build the exact source tree supplied by the workload's pinned Git checkout.
set -euo pipefail
umask 022

if [[ $# != 2 || $1 != --workload-config ]]; then
    echo 'usage: prepare.sh --workload-config /absolute/config.json' >&2
    exit 2
fi
: "${RUN_DIR:?RUN_DIR must be the retained run inputs directory}"
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
root=$(cd -- "$script_dir/../.." && pwd -P)
mkdir -p -- "$RUN_DIR"
run_dir=$(cd -- "$RUN_DIR" && pwd -P)
case "$run_dir/" in "$root/"*) echo 'RUN_DIR must be outside the source checkout' >&2; exit 2;; esac
python3 - "$2" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    config = json.load(f)
if config != {}:
    raise SystemExit('workload_config must be {}')
PY
bundle="$run_dir/execution"
if [[ -e "$bundle" || -e "$run_dir/fuzz-session.json" ]]; then
    echo 'refusing to replace an existing prepared session' >&2
    exit 2
fi
mkdir -- "$bundle"
mkdir -- "$bundle/lib"

# Detect concurrent source changes rather than archiving a tree different from the build inputs.
source_digest() {
    python3 - "$root" <<'PYDIGEST'
import hashlib, pathlib, subprocess, sys
root = pathlib.Path(sys.argv[1])
names = subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z',
                                 '--cached', '--others', '--exclude-standard'])
digest = hashlib.sha256()
for name in sorted(set(names.decode().split('\0')) - {''}):
    path = root / name
    if path.is_symlink():
        data = str(path.readlink()).encode()
    elif path.is_file():
        data = path.read_bytes()
    else:
        continue
    digest.update(name.encode() + b'\0' + hashlib.sha256(data).digest())
print(digest.hexdigest())
PYDIGEST
}
source_sha256=$(source_digest)

# The independent crate resolves public path dependencies only. No private Git source is needed.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target/sparse-trie-differential-sancov}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
# Scope LLVM instrumentation to the target crate: never instrument the adapter/core driver.
# Keep this wrapper with the execution bundle so its precise build policy is reproducible.
if [[ -n ${RUSTC_WRAPPER:-} || -n ${RUSTC_WORKSPACE_WRAPPER:-} ]]; then
    echo 'prepare.sh requires control of RUSTC_WRAPPER and RUSTC_WORKSPACE_WRAPPER' >&2
    exit 2
fi
cat > "$bundle/rustc-wrapper.py" <<'PYWRAPPER'
#!/usr/bin/env python3
import hashlib
import os
import pathlib
import sys

rustc, *args = sys.argv[1:]
# Preserve every original argument, including separate/equal flag forms and the real rustc.
def values(flag):
    for index, arg in enumerate(args):
        if arg == flag and index + 1 < len(args):
            yield args[index + 1]
        elif arg.startswith(flag + '='):
            yield arg[len(flag) + 1:]

codegen = [arg for index, arg in enumerate(args)
           if arg.startswith('-C') or (index > 0 and args[index - 1] == '-C')]
if any('sanitizer-coverage' in arg or 'sancov' in arg for arg in codegen):
    raise SystemExit('external SanCov flags would instrument the adapter; remove them')
crate = next(values('--crate-name'), None)
if crate == 'reth_trie_sparse':
    args += ['-Cpasses=sancov-module', '-Cllvm-args=-sanitizer-coverage-level=3',
             '-Cllvm-args=-sanitizer-coverage-inline-8bit-counters']
elif crate == 'sparse_trie_differential_workload':
    target = next((value.split('=', 1)[1] for value in values('--extern')
                   if value.startswith('reth_trie_sparse=')), None)
    if target is None:
        raise SystemExit('adapter is missing the reth_trie_sparse dependency')
    os.environ['SPARSE_TRIE_NATIVE_COVERAGE_ID'] = hashlib.sha256(
        pathlib.Path(target).read_bytes()).hexdigest()
os.execvp(rustc, [rustc, *args])
PYWRAPPER
chmod +x "$bundle/rustc-wrapper.py"
export RUSTC_WRAPPER="$bundle/rustc-wrapper.py"
# This is native compilation on the execution machine, not a prebuilt registry upload.
# Bind local functions so a fuzz host's own SanCov callback cannot steal our counter spans.
# Resolve the active toolchain directly when rustup is available. Host Cargo dispatch shims
# can replace RUSTC_WRAPPER, which would silently remove the target-only instrumentation.
if command -v rustup >/dev/null 2>&1; then
    cargo_bin=$(rustup which cargo)
    export RUSTC="${RUSTC:-$(rustup which rustc)}"
else
    cargo_bin=$(command -v cargo)
fi
"$cargo_bin" rustc --locked --release --manifest-path "$script_dir/Cargo.toml" --lib -- \
    -C 'link-arg=-Wl,-rpath,$ORIGIN/lib' -C link-arg=-Wl,-Bsymbolic-functions
if [[ $(source_digest) != "$source_sha256" ]]; then
    echo 'source changed during compilation; prepare a fresh session from a stable checkout' >&2
    exit 1
fi
cp -- "$CARGO_TARGET_DIR/release/libsparse_trie_differential_workload.so" "$bundle/libworkload.so"

# Retain the native dependencies and exact source needed to inspect or rebuild this execution.
python3 - "$bundle" "$root" "$source_sha256" <<'PY'
import hashlib, json, pathlib, shutil, subprocess, sys, tarfile
bundle, root = map(pathlib.Path, sys.argv[1:3])
linked = subprocess.check_output(['ldd', str(bundle / 'libworkload.so')], text=True)
if 'not found' in linked:
    raise SystemExit('unresolved native dependency:\n' + linked)
for line in linked.splitlines():
    fields = line.strip().split()
    path = fields[2] if len(fields) >= 3 and fields[1] == '=>' else (fields[0] if fields else '')
    if path.startswith('/'):
        shutil.copy2(path, bundle / 'lib' / pathlib.Path(path).name, follow_symlinks=True)
(bundle / 'native-dependencies.txt').write_text(linked)
# Never include Git credentials/configuration or build caches in retained source.
def retain(info):
    if any(part in {'.git', 'target', '__pycache__'} for part in pathlib.PurePosixPath(info.name).parts):
        return None
    if info.issym() or info.islnk():
        relative = pathlib.PurePosixPath(info.name).relative_to('source')
        if not (root / relative).resolve().is_relative_to(root):
            raise SystemExit('source symlink escapes checkout: ' + info.name)
    return info
with tarfile.open(bundle / 'source.tar.gz', 'w:gz') as archive:
    names = subprocess.check_output(
        ['git', '-C', str(root), 'ls-files', '-z', '--cached', '--others', '--exclude-standard'])
    for name in sorted(set(names.decode().split('\0')) - {''}):
        path = root / name
        if path.exists() or path.is_symlink():
            archive.add(path, arcname='source/' + name, recursive=False, filter=retain)
commit = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
status = subprocess.check_output(['git', '-C', str(root), 'status', '--porcelain'], text=True)
files = {str(p.relative_to(bundle)): hashlib.sha256(p.read_bytes()).hexdigest()
         for p in bundle.rglob('*') if p.is_file()}
provenance = {'source_commit': commit, 'source_status': status,
              'source_tree_sha256': sys.argv[3],
              'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
              'cargo': subprocess.check_output(['cargo', '--version'], text=True).strip(),
              'coverage': {'kind': 'llvm-sancov-inline-8bit-counters',
                           'crate': 'reth_trie_sparse', 'adapter_instrumented': False},
              'sha256': files}
(bundle / 'build-info.json').write_text(json.dumps(provenance, indent=2) + '\n')
session = {'version': 2, 'library': './libworkload.so', 'args': {}, 'workers': 1,
           'search': {'max_seeds': 0, 'finalize_every': 1, 'corpus_injection_every': 100}}
(bundle / 'fuzz-session.json').write_text(json.dumps(session, indent=2) + '\n')
PY
python3 "$script_dir/smoke.py" "$bundle/libworkload.so"
printf '%s\n' '{"version":2,"execution_bundle":"execution"}' > "$run_dir/fuzz-session.json"
echo "Prepared native session: $run_dir/fuzz-session.json"
