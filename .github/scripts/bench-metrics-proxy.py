#!/usr/bin/env python3
"""
Prometheus metrics proxy that fetches from a local reth node and
re-exposes with additional benchmark labels.

Reads labels from a JSON file (updated by local-reth-bench.sh between runs)
and injects them into every Prometheus metric line.

Returns empty 200 when reth is not running (clean Grafana gaps).
"""
import argparse
import ipaddress
import json
import subprocess
import sys
import time
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.request import urlopen
from urllib.error import URLError


def read_labels(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def inject_labels(metrics_bytes, label_str, label_names):
    """Inject labels into Prometheus text format.

    Operates on bytes and uses simple string ops instead of regex
    for speed on large payloads (reth exposes thousands of metrics).

    Skips injecting into lines that already contain any of the label names
    to avoid duplicate labels (which Prometheus rejects).
    """
    if not label_str:
        return metrics_bytes

    label_bytes = label_str.encode("utf-8")
    # Pre-encode label names for fast duplicate detection
    label_name_bytes = [n.encode("utf-8") for n in label_names]
    out = []
    for line in metrics_bytes.split(b"\n"):
        # Skip comments and blank lines
        if line.startswith(b"#") or not line:
            out.append(line)
            continue

        brace = line.find(b"{")
        space = line.find(b" ")

        if space == -1:
            # Malformed, pass through
            out.append(line)
        elif brace != -1 and brace < space:
            # Has labels: metric{existing="val"} 123
            close = line.find(b"}", brace)
            if close == -1:
                out.append(line)
                continue

            # Filter out labels that already exist in this line
            existing = line[brace + 1:close]
            inject = label_bytes
            if existing:
                for name in label_name_bytes:
                    if name + b"=" in existing:
                        # Rebuild inject string excluding this label
                        inject = _remove_label(inject, name)
                if not inject:
                    out.append(line)
                    continue

            if close == brace + 1:
                # Empty braces: metric{} 123
                out.append(line[:close] + inject + line[close:])
            else:
                out.append(line[:close] + b"," + inject + line[close:])
        else:
            # No labels: metric 123
            out.append(line[:space] + b"{" + label_bytes + b"}" + line[space:])

    return b"\n".join(out)


def _remove_label(label_bytes, name):
    """Remove a single label (name=\"...\") from a comma-separated label string."""
    parts = []
    for part in label_bytes.split(b","):
        if not part.startswith(name + b"="):
            parts.append(part)
    return b",".join(parts)


def build_label_str(labels):
    """Pre-format the label injection string: key1="val1",key2="val2" """
    if not labels:
        return ""
    return ",".join(f'{k}="{v}"' for k, v in sorted(labels.items()))


def build_elapsed_gauge(labels):
    """Build a bench_elapsed_seconds gauge from run_start_epoch in labels."""
    start = labels.get("run_start_epoch")
    if not start:
        return b""
    try:
        elapsed = time.time() - float(start)
    except (ValueError, TypeError):
        return b""
    # Build labels excluding internal keys
    display = {k: v for k, v in labels.items()
               if k not in ("run_start_epoch", "reference_epoch")}
    lstr = build_label_str(display)
    return (
        f"# HELP bench_elapsed_seconds Seconds since benchmark run started\n"
        f"# TYPE bench_elapsed_seconds gauge\n"
        f"bench_elapsed_seconds{{{lstr}}} {elapsed:.1f}\n"
    ).encode("utf-8")


def compute_timestamp_ms(labels):
    """Compute a synthetic timestamp so all runs share a common time origin.

    Returns the timestamp in milliseconds, or None if not enough info.
    Uses: reference_epoch + (now - run_start_epoch) → all runs overlay at
    the same Grafana time range.
    """
    ref = labels.get("reference_epoch")
    start = labels.get("run_start_epoch")
    if not ref or not start:
        return None
    try:
        elapsed = time.time() - float(start)
        return int((float(ref) + elapsed) * 1000)
    except (ValueError, TypeError):
        return None


def inject_timestamps(metrics_bytes, timestamp_ms):
    """Append a Prometheus timestamp (ms) to every data line.

    Prometheus text format: metric{labels} value [timestamp_ms]
    Adding timestamps causes Prometheus to store all runs' samples
    at the same relative time, enabling natural overlay in Grafana.
    """
    if timestamp_ms is None:
        return metrics_bytes

    ts = str(timestamp_ms).encode("utf-8")
    out = []
    for line in metrics_bytes.split(b"\n"):
        if line.startswith(b"#") or not line:
            out.append(line)
        else:
            out.append(line + b" " + ts)
    return b"\n".join(out)


class MetricsHandler(BaseHTTPRequestHandler):
    # Use HTTP/1.1 so Content-Length is respected and Prometheus
    # doesn't have to rely on connection close to detect end of body.
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        src = self.client_address[0]
        try:
            resp = urlopen(self.server.upstream, timeout=2)
            metrics = resp.read()
        except (URLError, ConnectionError, OSError):
            # reth not running — return empty 200
            self._send(b"")
            #print(f"  scrape from {src}: empty (reth not running)", flush=True)
            return

        all_labels = read_labels(self.server.labels_file)
        # Internal keys — not injected as Prometheus labels
        internal = ("run_start_epoch", "reference_epoch")
        labels = {k: v for k, v in all_labels.items() if k not in internal}
        label_str = build_label_str(labels)
        label_names = sorted(labels.keys())

        t0 = time.monotonic()
        result = inject_labels(metrics, label_str, label_names)
        result += build_elapsed_gauge(all_labels)
        ts_ms = compute_timestamp_ms(all_labels)
        result = inject_timestamps(result, ts_ms)
        dt = time.monotonic() - t0

        self._send(result)
        print(f"  scrape from {src}: {len(metrics)} -> {len(result)} bytes, "
              f"inject {dt*1000:.1f}ms", flush=True)

    def _send(self, body):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; version=0.0.4")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        if body:
            self.wfile.write(body)

    def log_message(self, format, *args):
        pass  # suppress per-request logging


def resolve_bind_address(subnet_cidr):
    """Find the local IP address that belongs to the given subnet.

    Uses ``ip -j addr show`` to enumerate interfaces and returns the first
    address that falls within *subnet_cidr* (e.g. ``10.10.0.0/24``).
    """
    network = ipaddress.ip_network(subnet_cidr, strict=False)
    try:
        result = subprocess.run(
            ["ip", "-j", "addr", "show"],
            capture_output=True, text=True, check=True,
        )
        interfaces = json.loads(result.stdout)
    except (subprocess.CalledProcessError, FileNotFoundError, json.JSONDecodeError) as exc:
        print(f"Error: cannot enumerate interfaces: {exc}", file=sys.stderr)
        sys.exit(1)

    for iface in interfaces:
        for addr_info in iface.get("addr_info", []):
            try:
                addr = ipaddress.ip_address(addr_info["local"])
            except (KeyError, ValueError):
                continue
            if addr in network:
                return str(addr)

    print(f"Error: no interface address found in subnet {subnet_cidr}", file=sys.stderr)
    sys.exit(1)


def main():
    parser = argparse.ArgumentParser(description="Prometheus metrics proxy with label injection")
    parser.add_argument("--labels", default="/tmp/bench-metrics-labels.json",
                        help="Path to JSON file with labels to inject (default: /tmp/bench-metrics-labels.json)")
    parser.add_argument("--upstream", default="http://127.0.0.1:9100/",
                        help="Upstream reth metrics URL (default: http://127.0.0.1:9100/)")

    bind_group = parser.add_mutually_exclusive_group()
    bind_group.add_argument("--bind", default=None,
                            help="Address to bind the proxy (default: 0.0.0.0)")
    bind_group.add_argument("--subnet", default=None,
                            help="Auto-detect bind address from a local interface in this subnet (e.g. 10.10.0.0/24)")

    parser.add_argument("--port", type=int, default=9090,
                        help="Port to bind the proxy (default: 9090)")
    args = parser.parse_args()

    if args.subnet:
        bind_addr = resolve_bind_address(args.subnet)
    elif args.bind:
        bind_addr = args.bind
    else:
        bind_addr = "0.0.0.0"

    server = HTTPServer((bind_addr, args.port), MetricsHandler)
    server.upstream = args.upstream
    server.labels_file = args.labels

    print(f"bench-metrics-proxy listening on {bind_addr}:{args.port}")
    print(f"  upstream: {args.upstream}")
    print(f"  labels:   {args.labels}")
    sys.stdout.flush()
    server.serve_forever()


if __name__ == "__main__":
    main()
