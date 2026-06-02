from __future__ import annotations

import subprocess
import unittest
from pathlib import Path

from bench_pages.ghcr import derive_ghcr_tag, publish_docker_images
from bench_pages.loader import load_sweep


FIXTURE_DIR = Path(__file__).parent / "fixtures"


class TestGhcr(unittest.TestCase):
    def test_publish_docker_images_prefers_provenance_digest_identity(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "docker_minimal")
        commands: list[list[str]] = []

        def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
            commands.append(command)
            if command[:3] == ["docker", "manifest", "inspect"]:
                return subprocess.CompletedProcess(command, 1, stdout="", stderr="not found")
            if command[:3] == ["docker", "image", "inspect"]:
                raise AssertionError("docker image inspect should not be used when provenance digest exists")
            return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

        records = publish_docker_images(
            sweep=sweep,
            owner="username",
            repo="repository",
            ghcr_package="nockchain-bench",
            publish=False,
            runner=runner,
        )

        self.assertEqual(len(records), 1)
        record = records[0]
        self.assertEqual(record.canonical_identity, record.provenance_image_digest)
        self.assertEqual(record.ghcr_tag, derive_ghcr_tag(record.provenance_image_digest or ""))
        self.assertEqual(
            record.ghcr_package_url,
            "https://github.com/username/repository/pkgs/container/nockchain-bench",
        )
        self.assertIsNone(record.local_image_id)
        self.assertFalse(any(command[:3] == ["docker", "image", "inspect"] for command in commands))

    def test_publish_docker_images_skips_existing_remote_and_pushes_missing_remote(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "docker_minimal")
        existing_commands: list[list[str]] = []
        missing_commands: list[list[str]] = []

        def existing_runner(command: list[str]) -> subprocess.CompletedProcess[str]:
            existing_commands.append(command)
            if command[:3] == ["docker", "manifest", "inspect"]:
                return subprocess.CompletedProcess(command, 0, stdout="{}", stderr="")
            return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

        def missing_runner(command: list[str]) -> subprocess.CompletedProcess[str]:
            missing_commands.append(command)
            if command[:3] == ["docker", "manifest", "inspect"]:
                return subprocess.CompletedProcess(command, 1, stdout="", stderr="not found")
            return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

        existing = publish_docker_images(
            sweep=sweep,
            owner="username",
            repo="repository",
            ghcr_package="nockchain-bench",
            publish=True,
            runner=existing_runner,
        )
        missing = publish_docker_images(
            sweep=sweep,
            owner="username",
            repo="repository",
            ghcr_package="nockchain-bench",
            publish=True,
            runner=missing_runner,
        )

        self.assertEqual(existing[0].publish_status, "already-present")
        self.assertFalse(any(command[:2] == ["docker", "tag"] for command in existing_commands))
        self.assertFalse(any(command[:2] == ["docker", "push"] for command in existing_commands))

        self.assertEqual(missing[0].publish_status, "pushed")
        self.assertTrue(any(command[:2] == ["docker", "tag"] for command in missing_commands))
        self.assertTrue(any(command[:2] == ["docker", "push"] for command in missing_commands))


if __name__ == "__main__":
    unittest.main()
