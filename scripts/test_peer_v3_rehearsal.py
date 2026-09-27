#!/usr/bin/env python3
"""Focused tests for the manual rehearsal's evidence and process ownership."""

import importlib.util
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "peer_v3_rehearsal", Path(__file__).with_name("peer-v3-loopback-rehearsal.py"),
)
REHEARSAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REHEARSAL)


class TransactionEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.rehearsal = REHEARSAL.Rehearsal.__new__(REHEARSAL.Rehearsal)

    def test_wallet_address_handles_rendering_and_rejects_ambiguity(self):
        address = "1" * 55
        rendered = "Active master key is set to \x1b[32m" + address[:25] + "\n  " + address[25:] + "\x1b[0m."
        self.assertEqual(REHEARSAL.wallet_address(rendered), address)
        for output in ("", "Active master key is set to invalid0.", rendered + "\nActive master key is set to " + "2" * 55 + "."):
            with self.subTest(output=output), self.assertRaises(RuntimeError):
                REHEARSAL.wallet_address(output)

    def test_acceptance_requires_an_explicit_boolean(self):
        for accepted in (False, True):
            self.rehearsal.rpc = lambda *args: {"accepted": accepted}
            self.assertIs(self.rehearsal.tx_accepted("b", "id"), accepted)
        for response in ({}, {"accepted": 0}, {"accepted": "true"}, {"ack": {}}):
            self.rehearsal.rpc = lambda *args: response
            with self.subTest(response=response), self.assertRaises(RuntimeError):
                self.rehearsal.tx_accepted("b", "id")

    def test_pending_requires_the_requested_id_and_boolean(self):
        self.rehearsal.nodes = {"b": {"private": 12345}}
        for pending in (False, True):
            self.rehearsal.tx_tool = lambda *args: {"tx_id": "expected", "pending": pending}
            self.assertIs(self.rehearsal.tx_pending("b", "expected"), pending)
        for response in ({"tx_id": "other", "pending": True}, {"tx_id": "expected"}, {"tx_id": "expected", "pending": 1}):
            self.rehearsal.tx_tool = lambda *args: response
            with self.subTest(response=response), self.assertRaises(RuntimeError):
                self.rehearsal.tx_pending("b", "expected")

    def test_gossip_requires_new_exact_cause_and_configured_sender(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.rehearsal.nodes = {"b": {"data": directory}}
            self.rehearsal.peer_id = lambda name: "configured-a"
            cause = bytes.fromhex("12" * 32)
            tags = json.dumps(["gossip", "peer-id", "configured-a"])
            rows = [
                (1, "libp2p", 1, tags, cause),  # before the submission cursor
                (2, "libp2p", 1, tags, b"different transaction"),
                (3, "libp2p", 1, json.dumps(["gossip", "peer-id", "other"]), cause),
                (4, "grpc", 1, tags, cause),
                (5, "libp2p", 3, tags, cause),  # wire version is not peer generation
                (6, "libp2p", 1, json.dumps(["response", "peer-id", "configured-a"]), cause),
                (7, "libp2p", 1, tags, cause),
            ]
            with sqlite3.connect(directory / "event-log.sqlite3") as connection:
                connection.execute("CREATE TABLE events (event_num INTEGER, wire_source TEXT, wire_version INTEGER, wire_tags_json TEXT, cause_hash BLOB)")
                connection.executemany("INSERT INTO events VALUES (?, ?, ?, ?, ?)", rows)
            self.assertEqual(self.rehearsal.receiver_event_cursor(), 7)
            self.assertEqual(self.rehearsal.transaction_gossip_event(1, cause.hex()), 7)
            self.assertIsNone(self.rehearsal.transaction_gossip_event(7, cause.hex()))

    def test_inclusion_requires_exact_id_height_and_proof(self):
        identity = {f"belt{i}": {"value": str(i)} for i in range(1, 6)}
        detail = {"height": "9", "blockId": identity, "hasPow": True, "txIds": [{"hash": "expected"}]}
        self.rehearsal.rpc = lambda *args: {"details": detail}
        self.assertEqual(self.rehearsal.transaction_in_block("b", 9, "expected"), {"height": 9, "block_id": identity})
        self.assertIsNone(self.rehearsal.transaction_in_block("b", 9, "other"))
        with self.assertRaises(RuntimeError):
            self.rehearsal.transaction_in_block("b", 8, "expected")
        detail["hasPow"] = False
        with self.assertRaises(RuntimeError):
            self.rehearsal.transaction_in_block("b", 9, "expected")

    def test_cleanup_reaps_an_owned_wallet_process(self):
        with tempfile.TemporaryDirectory() as temporary:
            child = subprocess.Popen(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            try:
                self.rehearsal.work = Path(temporary)
                self.rehearsal.processes = {"wallet-miner": child}
                self.rehearsal.logs = {}
                self.rehearsal.report = {"status": "failed", "failed_stage": "wallet-miner-create-tx"}
                self.rehearsal.close()
                self.assertIsNotNone(child.poll())
                self.assertEqual(self.rehearsal.processes, {})
                report = json.loads((self.rehearsal.work / "report.json").read_text())
                self.assertNotIn("surviving_processes", report)
                self.assertNotIn("cleanup_errors", report)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
