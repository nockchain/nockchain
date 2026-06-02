from __future__ import annotations

import copy
import json
import subprocess
import shutil
import tarfile
import tempfile
import unittest
from pathlib import Path

from bench_pages.gh_pages import bootstrap_pages_checkout, publish_sweep_to_pages
from bench_pages.loader import load_sweep
from bench_pages.manifest import build_manifest


FIXTURE_DIR = Path(__file__).parent / "fixtures"


class TestPages(unittest.TestCase):
    def _git(self, repo_root: Path, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", "-C", str(repo_root), *args],
            check=True,
            capture_output=True,
            text=True,
        )

    def test_bootstrap_pages_checkout_creates_fresh_orphan_layout(self) -> None:
        commands: list[list[str]] = []
        with tempfile.TemporaryDirectory() as temp_dir:
            repo_root = Path(temp_dir) / "repo"
            pages_root = Path(temp_dir) / "pages"
            repo_root.mkdir()

            def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
                commands.append(command)
                if command[:6] == [
                    "git",
                    "-C",
                    str(repo_root),
                    "show-ref",
                    "--verify",
                    "refs/heads/gh-pages",
                ]:
                    return subprocess.CompletedProcess(command, 1, stdout="", stderr="")
                if command[:8] == [
                    "git",
                    "-C",
                    str(repo_root),
                    "ls-remote",
                    "--exit-code",
                    "--heads",
                    "origin",
                    "gh-pages",
                ]:
                    return subprocess.CompletedProcess(command, 2, stdout="", stderr="")
                return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

            bootstrap_pages_checkout(
                repo_root=repo_root,
                pages_root=pages_root,
                branch="gh-pages",
                runner=runner,
            )

            self.assertTrue((pages_root / ".nojekyll").exists())
            self.assertTrue((pages_root / "index.json").exists())
            self.assertTrue((pages_root / "index.html").exists())
            self.assertIn(
                ["git", "-C", str(repo_root), "show-ref", "--verify", "refs/heads/gh-pages"],
                commands,
            )
            self.assertIn(
                ["git", "-C", str(repo_root), "worktree", "add", "--detach", str(pages_root)],
                commands,
            )
            self.assertIn(
                ["git", "-C", str(pages_root), "checkout", "--orphan", "gh-pages"],
                commands,
            )

    def test_bootstrap_pages_checkout_removes_inherited_files_from_orphan_branch(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo_root = Path(temp_dir) / "repo"
            pages_root = Path(temp_dir) / "pages"
            repo_root.mkdir()

            self._git(repo_root, "init")
            self._git(repo_root, "config", "user.name", "test")
            self._git(repo_root, "config", "user.email", "test@example.com")
            (repo_root / "tracked.txt").write_text("tracked\n")
            self._git(repo_root, "add", "tracked.txt")
            self._git(repo_root, "commit", "-m", "init")

            bootstrap_pages_checkout(
                repo_root=repo_root,
                pages_root=pages_root,
                branch="gh-pages",
            )

            self.assertFalse((pages_root / "tracked.txt").exists())
            self.assertTrue((pages_root / ".nojekyll").exists())
            self.assertEqual((pages_root / "index.json").read_text(), "[]\n")

    def test_bootstrap_pages_checkout_uses_existing_unfetched_remote_branch(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            origin_root = Path(temp_dir) / "origin.git"
            repo_root = Path(temp_dir) / "repo"
            publisher_root = Path(temp_dir) / "publisher"
            pages_root = Path(temp_dir) / "pages"

            subprocess.run(["git", "init", "--bare", str(origin_root)], check=True)
            subprocess.run(
                ["git", "clone", str(origin_root), str(repo_root)],
                check=True,
                capture_output=True,
                text=True,
            )
            self._git(repo_root, "config", "user.name", "test")
            self._git(repo_root, "config", "user.email", "test@example.com")

            (repo_root / "tracked.txt").write_text("tracked\n")
            self._git(repo_root, "add", "tracked.txt")
            self._git(repo_root, "commit", "-m", "init")
            self._git(repo_root, "push", "origin", "HEAD:main")

            subprocess.run(
                ["git", "clone", str(origin_root), str(publisher_root)],
                check=True,
                capture_output=True,
                text=True,
            )
            self._git(publisher_root, "config", "user.name", "test")
            self._git(publisher_root, "config", "user.email", "test@example.com")
            self._git(publisher_root, "checkout", "--orphan", "gh-pages")
            self._git(publisher_root, "rm", "-rf", "--ignore-unmatch", ".")
            (publisher_root / "index.json").write_text('[{"id":"existing"}]\n')
            (publisher_root / "existing.txt").write_text("from remote branch\n")
            self._git(publisher_root, "add", "index.json", "existing.txt")
            self._git(publisher_root, "commit", "-m", "publish")
            self._git(publisher_root, "push", "origin", "gh-pages")

            bootstrap_pages_checkout(
                repo_root=repo_root,
                pages_root=pages_root,
                branch="gh-pages",
            )

            self.assertTrue((pages_root / "existing.txt").exists())
            self.assertEqual(
                (pages_root / "existing.txt").read_text(),
                "from remote branch\n",
            )
            self.assertEqual(
                (pages_root / "index.json").read_text(),
                '[{"id":"existing"}]\n',
            )

    def test_publish_sweep_to_pages_writes_expected_layout(self) -> None:
        sweep_root = FIXTURE_DIR / "native_minimal"
        sweep = load_sweep(sweep_root)
        manifest = build_manifest(sweep)

        with tempfile.TemporaryDirectory() as temp_dir:
            pages_root = Path(temp_dir) / "pages"
            assets_root = Path(temp_dir) / "assets"
            assets_root.mkdir(parents=True)
            (assets_root / "site.css").write_text("body {}")
            (assets_root / "chart.umd.js").write_text("window.Chart = {};")

            publish_sweep_to_pages(
                pages_root=pages_root,
                sweep_root=sweep_root,
                manifest=manifest,
                sweep_html="<html><body>sweep</body></html>",
                index_html="<html><body>index</body></html>",
                assets_dir=assets_root,
            )

            sweep_id = manifest["sweep"]["id"]
            self.assertTrue((pages_root / "index.html").exists())
            self.assertTrue((pages_root / "index.json").exists())
            self.assertTrue((pages_root / "assets/site.css").exists())
            self.assertTrue((pages_root / "assets/chart.umd.js").exists())
            self.assertTrue((pages_root / f"sweeps/{sweep_id}/index.html").exists())
            self.assertTrue((pages_root / f"sweeps/{sweep_id}/manifest.json").exists())
            self.assertTrue(
                (
                    pages_root
                    / f"sweeps/{sweep_id}/artifacts/cases/case-000-threads_1/summary.json"
                ).exists()
            )

    def test_publish_sweep_to_pages_copies_profile_symbol_bundle(self) -> None:
        sweep_root = FIXTURE_DIR / "docker_minimal"
        sweep = load_sweep(sweep_root)
        manifest = build_manifest(sweep)

        with tempfile.TemporaryDirectory() as temp_dir:
            pages_root = Path(temp_dir) / "pages"
            assets_root = Path(temp_dir) / "assets"
            assets_root.mkdir(parents=True)
            (assets_root / "site.css").write_text("body {}")
            (assets_root / "chart.umd.js").write_text("window.Chart = {};")

            publish_sweep_to_pages(
                pages_root=pages_root,
                sweep_root=sweep_root,
                manifest=manifest,
                sweep_html="<html><body>sweep</body></html>",
                index_html="<html><body>index</body></html>",
                assets_dir=assets_root,
            )

            sweep_id = manifest["sweep"]["id"]
            self.assertTrue(
                (
                    pages_root
                    / f"sweeps/{sweep_id}/artifacts/cases/case-000-memory_limit_8g/symbols/nockchain-bench"
                ).exists()
            )

    def test_publish_sweep_to_pages_excludes_run_work_files_and_bundle_entries(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            sweep_root = Path(temp_dir) / "docker_pma_minimal"
            shutil.copytree(FIXTURE_DIR / "docker_pma_minimal", sweep_root)
            work_file = (
                sweep_root
                / "cases/case-000-memory_limit_8g/runs/run-0/work/replay-pma/0.pma"
            )
            work_file.parent.mkdir(parents=True)
            work_file.write_text("transient pma work file")

            sweep = load_sweep(sweep_root)
            manifest = build_manifest(sweep)
            pages_root = Path(temp_dir) / "pages"
            assets_root = Path(temp_dir) / "assets"
            assets_root.mkdir(parents=True)
            (assets_root / "site.css").write_text("body {}")
            (assets_root / "chart.umd.js").write_text("window.Chart = {};")

            publish_sweep_to_pages(
                pages_root=pages_root,
                sweep_root=sweep_root,
                manifest=manifest,
                sweep_html="<html><body>sweep</body></html>",
                index_html="<html><body>index</body></html>",
                assets_dir=assets_root,
            )

            sweep_id = manifest["sweep"]["id"]
            relative_path = "cases/case-000-memory_limit_8g/runs/run-0/work/replay-pma/0.pma"
            self.assertFalse(
                (pages_root / f"sweeps/{sweep_id}/artifacts/{relative_path}").exists()
            )

            bundle_path = pages_root / f"sweeps/{sweep_id}/{sweep_id}-artifacts.tar.gz"
            with tarfile.open(bundle_path, "r:gz") as bundle:
                self.assertNotIn(
                    f"{sweep_id}-artifacts/{relative_path}",
                    set(bundle.getnames()),
                )

    def test_publish_sweep_to_pages_keeps_single_index_entry_without_replace(self) -> None:
        sweep_root = FIXTURE_DIR / "native_minimal"
        sweep = load_sweep(sweep_root)
        manifest = build_manifest(sweep)

        with tempfile.TemporaryDirectory() as temp_dir:
            pages_root = Path(temp_dir) / "pages"
            assets_root = Path(temp_dir) / "assets"
            assets_root.mkdir(parents=True)
            (assets_root / "site.css").write_text("body {}")
            (assets_root / "chart.umd.js").write_text("window.Chart = {};")

            publish_sweep_to_pages(
                pages_root=pages_root,
                sweep_root=sweep_root,
                manifest=manifest,
                sweep_html="<html><body>first</body></html>",
                index_html="<html><body>index</body></html>",
                assets_dir=assets_root,
            )
            publish_sweep_to_pages(
                pages_root=pages_root,
                sweep_root=sweep_root,
                manifest=manifest,
                sweep_html="<html><body>second</body></html>",
                index_html="<html><body>index</body></html>",
                assets_dir=assets_root,
            )

            index_entries = json.loads((pages_root / "index.json").read_text())
            self.assertEqual(len(index_entries), 1)

            replaced_manifest = copy.deepcopy(manifest)
            replaced_manifest["sweep"]["fixture_identity"] = "replacement-fixture"
            publish_sweep_to_pages(
                pages_root=pages_root,
                sweep_root=sweep_root,
                manifest=replaced_manifest,
                sweep_html="<html><body>third</body></html>",
                index_html="<html><body>index</body></html>",
                assets_dir=assets_root,
                replace=True,
            )
            replaced_entries = json.loads((pages_root / "index.json").read_text())
            self.assertEqual(len(replaced_entries), 1)
            self.assertEqual(replaced_entries[0]["fixture_identity"], "replacement-fixture")

    def test_publish_sweep_to_pages_persists_context_summary_fields_in_index(self) -> None:
        sweep_root = FIXTURE_DIR / "native_fixture_axis_pma"
        manifest = build_manifest(load_sweep(sweep_root))

        with tempfile.TemporaryDirectory() as temp_dir:
            pages_root = Path(temp_dir) / "pages"
            assets_root = Path(temp_dir) / "assets"
            assets_root.mkdir(parents=True)
            (assets_root / "site.css").write_text("body {}")
            (assets_root / "chart.umd.js").write_text("window.Chart = {};")

            publish_sweep_to_pages(
                pages_root=pages_root,
                sweep_root=sweep_root,
                manifest=manifest,
                sweep_html="<html><body>sweep</body></html>",
                index_html="<html><body>index</body></html>",
                assets_dir=assets_root,
            )

            index_entries = json.loads((pages_root / "index.json").read_text())
            self.assertEqual(index_entries[0]["runtime_summary"], "pma")
            self.assertEqual(index_entries[0]["fixture_summary"], "2 fixtures")

    def test_publish_sweep_to_pages_copies_raw_tx_step_evidence(self) -> None:
        sweep_root = FIXTURE_DIR / "raw_tx_snapshot_minimal"
        manifest = build_manifest(load_sweep(sweep_root))

        with tempfile.TemporaryDirectory() as temp_dir:
            pages_root = Path(temp_dir) / "pages"
            assets_root = Path(temp_dir) / "assets"
            assets_root.mkdir(parents=True)
            (assets_root / "site.css").write_text("body {}")

            publish_sweep_to_pages(
                pages_root=pages_root,
                sweep_root=sweep_root,
                manifest=manifest,
                sweep_html="<html><body>raw tx</body></html>",
                index_html="<html><body>index</body></html>",
                assets_dir=assets_root,
            )

            sweep_id = manifest["sweep"]["id"]
            published_steps = (
                pages_root
                / f"sweeps/{sweep_id}/artifacts/cases/case-000-threads_1/runs/run-0/steps.ndjson"
            )
            bundle_path = pages_root / f"sweeps/{sweep_id}/{sweep_id}-artifacts.tar.gz"
            published_manifest = json.loads(
                (pages_root / f"sweeps/{sweep_id}/manifest.json").read_text()
            )

            self.assertTrue(published_steps.exists())
            self.assertIn(
                "cases/case-000-threads_1/runs/run-0/steps.ndjson",
                {
                    entry["relative_path"]
                    for entry in published_manifest["artifact_inventory"]
                },
            )
            with tarfile.open(bundle_path, "r:gz") as bundle:
                self.assertIn(
                    f"{sweep_id}-artifacts/cases/case-000-threads_1/runs/run-0/steps.ndjson",
                    set(bundle.getnames()),
                )
            self.assertEqual(
                published_manifest["cases"][0]["raw_tx_replay"]["raw_tx_pokes_completed"],
                3,
            )


if __name__ == "__main__":
    unittest.main()
