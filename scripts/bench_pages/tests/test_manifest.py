from __future__ import annotations

import copy
import json
import shutil
import tempfile
import unittest
from pathlib import Path

from bench_pages.artifacts import is_publish_artifact_path
from bench_pages.loader import load_sweep
from bench_pages.manifest import build_manifest, build_sweep_id
from bench_pages.raw_tx_replay import RAW_TX_METRIC_SPECS
try:
    from .support import create_partial_sweep_fixture
except ImportError:  # pragma: no cover - unittest discover imports as top-level modules.
    from support import create_partial_sweep_fixture


FIXTURE_DIR = Path(__file__).parent / "fixtures"


class TestManifest(unittest.TestCase):
    def test_build_sweep_id_is_stable_for_same_fixture(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "docker_minimal")

        first_id = build_sweep_id(sweep)
        second_id = build_sweep_id(sweep)

        self.assertEqual(first_id, second_id)

    def test_build_manifest_preserves_all_source_fields_and_artifacts(self) -> None:
        root = FIXTURE_DIR / "docker_minimal"
        sweep = load_sweep(root)
        manifest = build_manifest(sweep)

        case_manifest = manifest["cases"][0]
        summary_keys = set(
            json.loads(
                (root / "cases/case-000-memory_limit_8g/summary.json").read_text()
            ).keys()
        )
        provenance_keys = set(
            json.loads(
                (root / "cases/case-000-memory_limit_8g/provenance.json").read_text()
            ).keys()
        )
        result_keys = set(
            json.loads(
                (root / "cases/case-000-memory_limit_8g/runs/run-0/result.json").read_text()
            ).keys()
        )
        inventory_paths = {
            str(path.relative_to(root))
            for path in root.rglob("*")
            if path.is_file()
            and is_publish_artifact_path(path.relative_to(root))
        }

        self.assertTrue(summary_keys.issubset(case_manifest["summary"].keys()))
        self.assertTrue(provenance_keys.issubset(case_manifest["provenance"].keys()))
        self.assertTrue(result_keys.issubset(case_manifest["runs"][0]["result"].keys()))
        self.assertEqual(
            case_manifest["cpu_profile"]["profile_artifact"]["relative_path"],
            "cases/case-000-memory_limit_8g/profiles/samply-profile.json.gz",
        )
        self.assertEqual(
            case_manifest["cpu_profile"]["symbol_dir"]["relative_path"],
            "cases/case-000-memory_limit_8g/symbols",
        )
        self.assertEqual(
            case_manifest["cpu_profile"]["symbol_binary"]["relative_path"],
            "cases/case-000-memory_limit_8g/symbols/nockchain-bench",
        )
        self.assertEqual(
            case_manifest["cpu_profile"]["load_command"],
            "samply load --symbol-dir artifacts/cases/case-000-memory_limit_8g/symbols artifacts/cases/case-000-memory_limit_8g/profiles/samply-profile.json.gz",
        )
        self.assertEqual(
            {entry["relative_path"] for entry in manifest["artifact_inventory"]},
            inventory_paths,
        )

    def test_build_manifest_excludes_run_work_files(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            copied_root = Path(temp_dir) / "docker_pma_minimal"
            shutil.copytree(FIXTURE_DIR / "docker_pma_minimal", copied_root)
            work_file = (
                copied_root
                / "cases/case-000-memory_limit_8g/runs/run-0/work/replay-pma/0.pma"
            )
            work_file.parent.mkdir(parents=True)
            work_file.write_text("transient pma work file")

            manifest = build_manifest(load_sweep(copied_root))

        self.assertNotIn(
            "cases/case-000-memory_limit_8g/runs/run-0/work/replay-pma/0.pma",
            {entry["relative_path"] for entry in manifest["artifact_inventory"]},
        )

    def test_build_manifest_uses_provenance_digest_as_primary_docker_identity(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "docker_minimal")

        manifest = build_manifest(sweep)

        docker_image = manifest["docker_images"][0]
        self.assertEqual(
            docker_image["canonical_identity"],
            docker_image["provenance_image_digest"],
        )
        self.assertIsNone(docker_image["local_image_id"])

    def test_build_manifest_surfaces_generic_pma_context(self) -> None:
        pma_manifest = build_manifest(load_sweep(FIXTURE_DIR / "docker_pma_minimal"))
        mixed_manifest = build_manifest(load_sweep(FIXTURE_DIR / "native_fixture_axis_pma"))

        self.assertEqual(pma_manifest["cases"][0]["runtime_flavor"], "pma")
        self.assertEqual(pma_manifest["cases"][0]["boot_source"], "checkpoint")
        self.assertEqual(pma_manifest["cases"][0]["boot_event_num"], 42)
        self.assertEqual(pma_manifest["cases"][0]["pma_work_dir_mode"], "docker_tmpfs")
        self.assertEqual(pma_manifest["sweep"]["runtime_summary"], "pma")
        self.assertEqual(pma_manifest["sweep"]["boot_source_summary"], "checkpoint")
        self.assertEqual(pma_manifest["sweep"]["pma_work_dir_summary"], "docker_tmpfs")
        self.assertEqual(mixed_manifest["sweep"]["runtime_summary"], "pma")
        self.assertEqual(mixed_manifest["sweep"]["fixture_summary"], "2 fixtures")

    def test_unknown_additive_fields_survive_manifest_roundtrip(self) -> None:
        manifest = build_manifest(load_sweep(FIXTURE_DIR / "native_pma_minimal"))

        self.assertIn("new_metric_total", manifest["cases"][0]["summary"])
        self.assertIn("future_probe_status", manifest["cases"][0]["provenance"])

    def test_build_manifest_derives_raw_tx_replay_summary(self) -> None:
        manifest = build_manifest(load_sweep(FIXTURE_DIR / "raw_tx_snapshot_minimal"))
        case = manifest["cases"][0]
        run = case["runs"][0]

        self.assertNotIn("steps", run)
        self.assertTrue(case["raw_tx_replay"]["active"])
        self.assertEqual(case["raw_tx_replay"]["step_count"], 3)
        self.assertEqual(case["raw_tx_replay"]["raw_tx_pokes_completed"], 3)
        self.assertEqual(case["raw_tx_replay"]["raw_tx_slabs_prebuilt"], 5)
        self.assertEqual(
            case["raw_tx_replay"]["raw_tx_payload_bytes_prebuilt"],
            96132,
        )
        self.assertEqual(
            case["raw_tx_replay"]["known_zero_raw_tx_poke_steps"],
            1,
        )
        self.assertEqual(case["raw_tx_replay"]["error_rows_omitted"], 0)
        self.assertEqual(
            case["raw_tx_replay"]["slab_prebuild_peak_rss_bytes"]["max"],
            69206016,
        )
        self.assertEqual(
            case["raw_tx_replay"]["slab_prebuild_start_rss_bytes"],
            {"min": 52428800, "max": 55574528},
        )
        self.assertEqual(
            case["raw_tx_replay"]["slab_prebuild_peak_rss_bytes"],
            {"min": 67108864, "max": 69206016},
        )
        self.assertEqual(
            case["raw_tx_replay"]["error_rows"][0]["run_id"],
            "run-0",
        )
        self.assertEqual(
            case["raw_tx_replay"]["error_rows"][0]["raw_tx_pokes_completed"],
            0,
        )
        self.assertEqual(
            run["raw_tx_replay"]["raw_tx_payload_bytes_prebuilt"],
            96132,
        )
        self.assertNotIn("rows", case["raw_tx_replay"])

    def test_build_manifest_bounds_multi_run_raw_tx_failures(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            copied_root = Path(temp_dir) / "raw_tx_snapshot_minimal"
            shutil.copytree(FIXTURE_DIR / "raw_tx_snapshot_minimal", copied_root)
            run0 = copied_root / "cases/case-000-threads_1/runs/run-0"
            run1 = copied_root / "cases/case-000-threads_1/runs/run-1"
            shutil.copytree(run0, run1)

            non_raw_rows = [
                {"step_index": index, "type": "peek", "outcome": "ok"}
                for index in range(50)
            ]
            failure_rows = [
                {
                    "step_index": 100 + index,
                    "label": "repeat-failure",
                    "type": "poke_archive_block",
                    "height": 10022,
                    "outcome": "error",
                    "error": f"raw tx failure {index}",
                    "raw_tx_pokes_completed": 1,
                    "raw_tx_slabs_prebuilt": 1,
                    "raw_tx_payload_bytes_prebuilt": 10,
                    "slab_prebuild_duration_ms": 1.0,
                    "slab_prebuild_peak_rss_bytes": 1000 + index,
                }
                for index in range(25)
            ]
            missing_outcome_failure = {
                "step_index": 200,
                "label": "repeat-failure",
                "type": "poke_archive_block",
                "height": 10022,
                "error": "raw tx failure without outcome",
                "raw_tx_pokes_completed": 1,
                "raw_tx_slabs_prebuilt": 1,
                "raw_tx_payload_bytes_prebuilt": 10,
                "slab_prebuild_duration_ms": 1.0,
                "slab_prebuild_peak_rss_bytes": 2000,
            }
            (run0 / "steps.ndjson").write_text(
                "\n".join(json.dumps(row) for row in non_raw_rows + failure_rows)
                + "\n"
            )
            (run1 / "steps.ndjson").write_text(
                json.dumps(missing_outcome_failure) + "\n"
            )

            manifest = build_manifest(load_sweep(copied_root))

        case_summary = manifest["cases"][0]["raw_tx_replay"]
        run0_summary = manifest["cases"][0]["runs"][0]["raw_tx_replay"]
        run1_summary = manifest["cases"][0]["runs"][1]["raw_tx_replay"]

        self.assertEqual(run0_summary["step_count"], 25)
        self.assertEqual(run0_summary["error_step_count"], 25)
        self.assertEqual(len(run0_summary["error_rows"]), 20)
        self.assertEqual(run0_summary["error_rows_omitted"], 5)
        self.assertEqual(
            [row["error"] for row in run0_summary["error_rows"]],
            [f"raw tx failure {index}" for index in range(10)]
            + [f"raw tx failure {index}" for index in range(15, 25)],
        )
        self.assertEqual(run1_summary["step_count"], 1)
        self.assertEqual(run1_summary["error_step_count"], 1)
        self.assertEqual(case_summary["step_count"], 26)
        self.assertEqual(case_summary["raw_tx_pokes_completed"], 26)
        self.assertEqual(case_summary["error_step_count"], 26)
        self.assertEqual(len(case_summary["error_rows"]), 20)
        self.assertEqual(case_summary["error_rows_omitted"], 6)
        self.assertEqual(
            [row["error"] for row in case_summary["error_rows"]],
            [f"raw tx failure {index}" for index in range(10)]
            + [f"raw tx failure {index}" for index in range(16, 25)]
            + ["raw tx failure without outcome"],
        )
        self.assertEqual(
            [
                (row["run_id"], row["label"], row["height"])
                for row in case_summary["error_rows"]
            ],
            [("run-0", "repeat-failure", 10022) for _ in range(19)]
            + [("run-1", "repeat-failure", 10022)],
        )
        self.assertIn(
            ("run-0", "repeat-failure", 10022),
            {
                (row["run_id"], row["label"], row["height"])
                for row in case_summary["error_rows"]
            },
        )
        self.assertIn(
            ("run-1", "repeat-failure", 10022),
            {
                (row["run_id"], row["label"], row["height"])
                for row in case_summary["error_rows"]
            },
        )
        self.assertEqual(
            run1_summary["error_rows"][0]["error"],
            "raw tx failure without outcome",
        )

    def test_build_manifest_ignores_null_raw_tx_step_fields(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            copied_root = Path(temp_dir) / "raw_tx_snapshot_minimal"
            shutil.copytree(FIXTURE_DIR / "raw_tx_snapshot_minimal", copied_root)
            steps_path = (
                copied_root
                / "cases/case-000-threads_1/runs/run-0/steps.ndjson"
            )
            row = {"step_index": 0, "type": "poke_archive_block"}
            row.update({field: None for field in RAW_TX_METRIC_SPECS})
            steps_path.write_text(json.dumps(row) + "\n")

            manifest = build_manifest(load_sweep(copied_root))

        self.assertFalse(manifest["cases"][0]["raw_tx_replay"]["active"])

    def test_build_sweep_id_distinguishes_fixture_identity(self) -> None:
        mixed = load_sweep(FIXTURE_DIR / "native_fixture_axis_pma")
        mixed_single_fixture = copy.deepcopy(mixed)

        mixed_single_fixture.cases[1].resolved_case["fixture_sha256_hex"] = (
            mixed_single_fixture.cases[0].resolved_case["fixture_sha256_hex"]
        )
        mixed_single_fixture.cases[1].provenance["fixture_sha256_hex"] = (
            mixed_single_fixture.cases[0].provenance["fixture_sha256_hex"]
        )

        self.assertNotEqual(build_sweep_id(mixed), build_sweep_id(mixed_single_fixture))

    def test_build_manifest_preserves_partial_sweep_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            partial_root = create_partial_sweep_fixture(Path(temp_dir))
            manifest = build_manifest(load_sweep(partial_root))

        self.assertEqual(manifest["sweep"]["completion_state"], "incomplete")
        self.assertEqual(
            manifest["sweep"]["missing_top_level_artifacts"],
            ["comparison.json", "verdict.json"],
        )
        self.assertEqual(manifest["sweep"]["scheduled_case_count"], 3)
        self.assertEqual(manifest["sweep"]["materialized_case_count"], 2)
        self.assertEqual(manifest["sweep"]["complete_case_count"], 1)
        self.assertEqual(manifest["sweep"]["partial_case_count"], 1)
        self.assertEqual(manifest["sweep"]["missing_case_count"], 1)
        self.assertIsNone(manifest["source_artifacts"]["comparison"])
        self.assertIsNone(manifest["source_artifacts"]["verdict"])
        self.assertEqual(manifest["cases"][1]["completion_state"], "partial")
        self.assertEqual(
            manifest["cases"][1]["missing_artifacts"],
            ["summary.json", "verdict.json"],
        )
        self.assertEqual(manifest["cases"][2]["completion_state"], "missing")
        self.assertEqual(
            manifest["cases"][2]["requested_case"]["execution"]["Docker"]["memory_limit"],
            "2g",
        )


if __name__ == "__main__":
    unittest.main()
