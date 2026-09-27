#!/usr/bin/env python3
"""Rehearse v3 between two isolated, real fakenet nodes and one ZK miner.

Build current dumb/miner Hoon assets and both binaries before running. The
script never builds, contacts an external host, or reuses consensus state.
All listeners and explicit peers use loopback, bootstrap peers are disabled,
and child environments omit inherited networking/telemetry configuration.

This exercises block synchronization, authenticated block gossip, persistent
node restart, and catch-up. It does not claim wallet/transaction propagation,
mixed-version interoperability, or production deployment readiness.
Gossip receipt and accepted head agreement are checked independently; event
counts do not identify which block arrived through gossip.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time


PKH = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV"
BLOCK_SERVICE = "nockchain.public.v2.NockchainBlockService/"
METRICS_SERVICE = "nockchain.public.v2.NockchainMetricsService/"
VERIFIER_SETUP_SEED_FILE = "verifier-setup-seeds-v2.bin"


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def free_port(kind, assigned):
    for _ in range(100):
        with socket.socket(socket.AF_INET, kind) as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        if port not in assigned:
            assigned.add(port)
            return port
    raise RuntimeError("could not allocate a distinct loopback port")


def source_metadata():
    repository = Path(__file__).resolve().parent.parent
    revision = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        capture_output=True, text=True, timeout=10, check=True,
    ).stdout.strip()
    sources = hashlib.sha256()
    for path in sorted((repository / "hoon").rglob("*.hoon")):
        sources.update(str(path.relative_to(repository)).encode() + b"\0")
        sources.update(bytes.fromhex(file_hash(path)))
    return {
        "source_revision": revision,
        "hoon_source_tree_sha256": sources.hexdigest(),
        "kernel_assets_sha256": {
            name: file_hash(repository / "assets" / name)
            for name in ("dumb.jam", "miner.jam")
        },
    }


class Rehearsal:
    def __init__(self, args):
        self.args = args
        self.work = args.work_dir.resolve() if args.work_dir else Path(
            tempfile.mkdtemp(prefix="nockchain-peer-v3-loopback-")
        )
        if args.work_dir:
            self.work.mkdir(parents=True, exist_ok=False)
        self.processes = {}
        self.logs = {}
        self.log_offsets = {}
        self.born = set()
        self.nodes = {}
        assigned_ports = set()
        for name in ("a", "b"):
            work = self.work / name
            work.mkdir()
            self.nodes[name] = {
                "work": work,
                "data": work / "data",
                "identity": work / "identity",
                "public": free_port(socket.SOCK_STREAM, assigned_ports),
                "private": free_port(socket.SOCK_STREAM, assigned_ports),
                "p2p": free_port(socket.SOCK_DGRAM, assigned_ports),
            }
        # Keep OS process basics, but discard NOCKCHAIN_*, miner, proxy,
        # telemetry and other inherited application settings.
        self.env = {
            key: os.environ[key]
            for key in ("PATH", "HOME", "TMPDIR", "LANG")
            if key in os.environ
        }
        self.env.update({
            "NOCKAPP_DISABLE_METRICS": "1",
            "GNORT_DISABLE": "1",
            "RUST_LOG": "info",
            "RAYON_NUM_THREADS": "2",
            "NOCKCHAIN_LIBP2P_MIN_PEERS": "1",
            "NOCKCHAIN_LIBP2P_FORCE_PEER_DIAL_INTERVAL_SECS": "2",
        })
        self.report = {
            "status": "running",
            "active_stage": "initialization",
            "node_binary_sha256": file_hash(args.node_bin),
            "miner_binary_sha256": file_hash(args.miner_bin),
            "isolation": {
                "network": "127.0.0.1 only",
                "default_peers": False,
                "fresh_consensus_state": True,
                "inherited_application_environment": False,
            },
            "stages": [],
            **source_metadata(),
        }

    def spawn(self, name, command, cwd):
        if name in self.processes:
            raise RuntimeError(f"process {name} already running")
        log = (self.work / f"{name}.log").open("ab")
        self.logs[name] = log
        self.log_offsets[name] = log.tell()
        self.born.discard(name)
        self.processes[name] = subprocess.Popen(
            command, cwd=cwd, env=self.env, stdout=log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )

    def stop(self, name):
        process = self.processes.get(name)
        try:
            if process is not None and process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGINT)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    self.kill(name)
        finally:
            if process is not None and process.poll() is not None:
                self.processes.pop(name, None)
            log = self.logs.get(name)
            if log is not None:
                try:
                    log.close()
                finally:
                    if log.closed:
                        self.logs.pop(name, None)

    def kill(self, name):
        process = self.processes.get(name)
        if process is not None:
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
            if process.poll() is not None:
                self.processes.pop(name, None)

    def healthy(self):
        for name, process in self.processes.items():
            if process.poll() is not None:
                raise RuntimeError(f"{name} exited with status {process.returncode}")

    def wait(self, label, predicate, timeout):
        self.report["active_stage"] = label
        print(f"[{label}] waiting up to {timeout}s", flush=True)
        deadline = time.monotonic() + timeout
        last_error = None
        while time.monotonic() < deadline:
            self.healthy()
            try:
                value = predicate()
                if value:
                    return value
            except (RuntimeError, OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as error:
                last_error = str(error)
            time.sleep(2)
        raise RuntimeError(f"{label} timed out; last observation: {last_error}")

    def rpc(self, node, service, request):
        port = self.nodes[node]["public"]
        completed = subprocess.run(
            [str(self.args.grpcurl), "-plaintext", "-max-time", "8",
             "-max-msg-sz", "16777216", "-d", "@", f"127.0.0.1:{port}", service],
            input=json.dumps(request).encode(), capture_output=True,
            timeout=12, env=self.env, check=True,
        )
        result = json.loads(completed.stdout)
        if "error" in result:
            raise RuntimeError(f"gRPC {service} returned an application error")
        return result

    def head(self, name):
        result = self.rpc(name, BLOCK_SERVICE + "GetBlocks", {
            "page": {"clientPageItemsLimit": "1"},
        })["blocks"]
        height = int(result.get("currentHeight", 0))
        entries = result.get("blocks", [])
        if not entries or int(entries[0].get("height", 0)) != height:
            raise RuntimeError("head not present in explorer cache")
        return {"height": height, "block_id": entries[0]["blockId"]}

    def ready(self, name):
        # A fresh fakenet has only genesis; the explorer deliberately does not
        # seed a block list at height zero. Wait for this boot's actual born
        # command and a serving metrics RPC instead of requiring a cached head.
        process = f"node-{name}"
        marker = b"handle-command: born"
        if process not in self.born:
            with (self.work / f"{process}.log").open("rb") as log:
                log.seek(self.log_offsets[process])
                overlap = b""
                while chunk := log.read(65536):
                    if marker in overlap + chunk:
                        self.born.add(process)
                        break
                    overlap = chunk[-len(marker):]
                self.log_offsets[process] = max(0, log.tell() - len(marker))
            if process not in self.born:
                return False
        return "metrics" in self.rpc(name, METRICS_SERVICE + "GetExplorerMetrics", {})

    def peer_id(self, name):
        return self.nodes[name]["identity"].with_suffix(".peerid").read_text().strip()

    def stats(self, name, other):
        result = self.rpc(name, METRICS_SERVICE + "GetPeerStats", {})
        peers = result.get("stats", {}).get("peers", [])
        expected = self.peer_id(other)
        if any(peer.get("peerId") != expected for peer in peers):
            raise RuntimeError("unexpected peer in isolated rehearsal")
        if len(peers) != 1:
            raise RuntimeError("expected exactly one peer")
        peer = peers[0]
        if peer.get("protocolGeneration") != "PEER_REQ_RES_GENERATION_GEN3":
            raise RuntimeError("peer telemetry does not report generation 3")
        if int(peer.get("bytesReceived", 0)) == 0:
            raise RuntimeError("peer has not received v3 traffic")
        return {key: value for key, value in peer.items() if key != "peerId"}

    def gossip_count(self):
        database = self.nodes["b"]["data"] / "event-log.sqlite3"
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=1) as connection:
            return connection.execute(
                "SELECT count(*) FROM events WHERE wire_source = 'libp2p' "
                "AND json_extract(wire_tags_json, '$[0]') = 'gossip'"
            ).fetchone()[0]

    def start_node(self, name, fresh):
        self.report["active_stage"] = f"node-{name}-{'boot' if fresh else 'restart'}"
        node = self.nodes[name]
        args = [
            str(self.args.node_bin), "--fakenet", "--no-default-peers",
            "--data-dir", str(node["data"]),
            "--identity-path", str(node["identity"]),
            "--bind", f"/ip4/127.0.0.1/udp/{node['p2p']}/quic-v1",
            "--bind-private-grpc-addr", f"127.0.0.1:{node['private']}",
            "--bind-public-grpc-addr", f"127.0.0.1:{node['public']}",
            "--fakenet-pow-len", "2", "--fakenet-log-difficulty", "1",
            "--fakenet-v1-phase", "1", "--fakenet-bythos-phase", "1",
            "--stack-size", "normal", "--pma-initial-size", "512MiB",
            "--ai-pow-verifier-cache-cap", "1",
        ]
        if fresh:
            args.append("--new")
        if name == "b":
            a = self.nodes["a"]
            args.extend([
                "--force-peer",
                f"/ip4/127.0.0.1/udp/{a['p2p']}/quic-v1/p2p/{self.peer_id('a')}",
            ])
        self.spawn(f"node-{name}", args, node["work"])

    def start_miner(self, stage):
        self.report["active_stage"] = stage
        self.spawn("miner", [
            str(self.args.miner_bin),
            "--node-addr", f"http://127.0.0.1:{self.nodes['a']['private']}",
            "--mining-pkh", PKH, "--num-threads", "1",
            "--worker-shutdown-timeout-ms", "5000",
        ], self.work)

    def mined(self, height):
        head = self.head("a")
        return head if head["height"] >= height else None

    def equal_heads(self, minimum):
        left, right = self.head("a"), self.head("b")
        if left == right and left["height"] >= minimum:
            return left
        return None

    def agreement(self, name, minimum):
        head = self.wait(name, lambda: self.equal_heads(minimum), self.args.stage_timeout)
        detail = self.rpc("b", BLOCK_SERVICE + "GetBlockDetails", {
            "height": str(head["height"]),
        })["details"]
        if detail.get("blockId") != head["block_id"] or not detail.get("hasPow"):
            raise RuntimeError("receiving node did not expose the agreed proof-bearing block")
        stage = {"name": name, "head": head, "receiver_peer_stats": self.stats("b", "a")}
        self.report["stages"].append(stage)
        print(f"[{name}] agreed at height {head['height']}", flush=True)
        return head["height"]

    def run(self):
        print(f"Artifacts: {self.work}", flush=True)
        if self.args.setup_cache:
            self.report["active_stage"] = "prepare-verifier-cache"
            destination = self.nodes["a"]["data"] / "ai-pow"
            destination.mkdir(parents=True)
            shutil.copyfile(
                self.args.setup_cache / VERIFIER_SETUP_SEED_FILE,
                destination / VERIFIER_SETUP_SEED_FILE,
            )
        self.start_node("a", fresh=True)
        self.wait("node-a-boot", lambda: self.ready("a"), self.args.boot_timeout)
        # Reuse only proof-independent verifier setup, never consensus state.
        self.report["active_stage"] = "copy-verifier-cache"
        shutil.copytree(self.nodes["a"]["data"] / "ai-pow", self.nodes["b"]["data"] / "ai-pow")
        self.start_miner("mine-before-sync")
        self.wait("mine-before-sync", lambda: self.mined(3), self.args.stage_timeout)
        self.stop("miner")
        initial_height = self.head("a")["height"]
        self.start_node("b", fresh=True)
        self.wait("node-b-boot", lambda: self.ready("b"), self.args.boot_timeout)
        synced = self.agreement("initial-sync", initial_height)

        self.report["active_stage"] = "connected-block-gossip"
        before = self.gossip_count()
        self.start_miner("mine-connected")
        self.wait("mine-connected", lambda: self.mined(synced + 1), self.args.stage_timeout)
        self.stop("miner")
        gossiped = self.agreement("connected-block-gossip", synced + 1)
        self.wait("gossip-event", lambda: self.gossip_count() > before, self.args.stage_timeout)
        self.report["stages"][-1]["new_receiver_gossip_events"] = self.gossip_count() - before
        self.report["stages"][-1]["gossip_evidence"] = "block gossip receipt, without block-id correlation"

        self.report["active_stage"] = "disconnect-receiver"
        identity_before = self.peer_id("b")
        self.stop("node-b")
        self.start_miner("mine-during-disconnect")
        self.wait("mine-during-disconnect", lambda: self.mined(gossiped + 2), self.args.stage_timeout)
        self.stop("miner")
        target = self.head("a")["height"]
        self.start_node("b", fresh=False)
        self.wait("node-b-restart", lambda: self.ready("b"), self.args.boot_timeout)
        self.agreement("restart-and-catch-up", target)
        if self.peer_id("b") != identity_before:
            raise RuntimeError("receiver identity changed across restart")
        self.report["stages"][-1]["identity_preserved"] = True
        self.report["status"] = "passed"
        self.report["active_stage"] = None

    def close(self):
        # An additional interrupt must not skip remaining children or the report.
        handlers = {sig: signal.signal(sig, signal.SIG_IGN) for sig in (signal.SIGINT, signal.SIGTERM)}
        errors = []
        try:
            for name in ("miner", "node-b", "node-a"):
                try:
                    self.stop(name)
                except Exception as error:
                    errors.append(f"{name}: {type(error).__name__}: {error}")
                    try:
                        self.kill(name)
                    except Exception as kill_error:
                        errors.append(f"{name} forced shutdown: {type(kill_error).__name__}: {kill_error}")
            for name, log in list(self.logs.items()):
                try:
                    log.close()
                except Exception as error:
                    errors.append(f"{name} log close: {type(error).__name__}: {error}")
                finally:
                    self.logs.pop(name, None)
        finally:
            if errors:
                self.report["status"] = "failed"
                self.report.setdefault("failed_stage", "cleanup")
                self.report["cleanup_errors"] = errors
            remaining = {name: process.pid for name, process in self.processes.items() if process.poll() is None}
            if remaining:
                self.report["status"] = "failed"
                self.report.setdefault("failed_stage", "cleanup")
                self.report["surviving_processes"] = remaining
            try:
                (self.work / "report.json").write_text(json.dumps(self.report, indent=2) + "\n")
            finally:
                for sig, handler in handlers.items():
                    signal.signal(sig, handler)
        if errors or remaining:
            raise RuntimeError("rehearsal cleanup failed; see report.json")


def main():
    def interrupted(signum, _frame):
        raise KeyboardInterrupt(f"signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node-bin", required=True, type=Path)
    parser.add_argument("--miner-bin", required=True, type=Path)
    parser.add_argument("--grpcurl", default=shutil.which("grpcurl"), type=Path)
    parser.add_argument("--work-dir", type=Path, help="new directory; existing paths are refused")
    parser.add_argument(
        "--setup-cache", type=Path,
        help=f"optional directory containing {VERIFIER_SETUP_SEED_FILE}; only this seed file is imported",
    )
    parser.add_argument("--boot-timeout", type=int, default=3600)
    parser.add_argument("--stage-timeout", type=int, default=180)
    args = parser.parse_args()
    for name in ("node_bin", "miner_bin", "grpcurl"):
        path = getattr(args, name)
        if path is None or not path.is_file():
            parser.error(f"{name} must name an existing executable")
        setattr(args, name, path.resolve())
    if args.setup_cache is not None:
        if not (args.setup_cache / VERIFIER_SETUP_SEED_FILE).is_file():
            parser.error(f"setup_cache must contain {VERIFIER_SETUP_SEED_FILE}")
        args.setup_cache = args.setup_cache.resolve()
    if args.boot_timeout <= 0 or args.stage_timeout <= 0:
        parser.error("timeouts must be positive")
    rehearsal = Rehearsal(args)
    try:
        rehearsal.run()
    except (Exception, KeyboardInterrupt) as error:
        rehearsal.report["status"] = "failed"
        rehearsal.report["failed_stage"] = rehearsal.report["active_stage"]
        rehearsal.report["error"] = str(error)
        raise
    finally:
        rehearsal.close()
        print(f"Report: {rehearsal.work / 'report.json'}", flush=True)


if __name__ == "__main__":
    main()
