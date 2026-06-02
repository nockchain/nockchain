from __future__ import annotations

import json
import shutil
import tempfile
import unittest
from pathlib import Path

from bench_pages.errors import ValidationError
from bench_pages.loader import load_sweep
from bench_pages.raw_tx_replay import RAW_TX_METRIC_SPECS
try:
    from .support import create_partial_sweep_fixture
except ImportError:  # pragma: no cover - unittest discover imports as top-level modules.
    from support import create_partial_sweep_fixture


FIXTURE_DIR = Path(__file__).parent / "fixtures"


class TestLoadSweep(unittest.TestCase):
    def test_load_sweep_normalizes_native_execution_mode(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "native_minimal")

        self.assertEqual(sweep.execution_mode, "native")
        self.assertEqual(len(sweep.cases), 1)
        self.assertEqual(sweep.cases[0].execution_mode, "native")

    def test_load_sweep_normalizes_docker_execution_mode(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "docker_minimal")

        self.assertEqual(sweep.execution_mode, "docker")
        self.assertEqual(len(sweep.cases), 1)
        self.assertEqual(sweep.cases[0].execution_mode, "docker")

    def test_load_sweep_records_every_artifact_in_tree_walk_inventory(self) -> None:
        root = FIXTURE_DIR / "docker_minimal"
        sweep = load_sweep(root)

        expected = sorted(
            str(path.relative_to(root))
            for path in root.rglob("*")
            if path.is_file()
        )
        actual = sorted(record.relative_path for record in sweep.artifact_inventory)
        self.assertEqual(actual, expected)

    def test_load_sweep_excludes_run_work_files_from_artifact_inventory(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            copied_root = Path(temp_dir) / "docker_pma_minimal"
            shutil.copytree(FIXTURE_DIR / "docker_pma_minimal", copied_root)
            work_file = (
                copied_root
                / "cases/case-000-memory_limit_8g/runs/run-0/work/replay-pma/0.pma"
            )
            work_file.parent.mkdir(parents=True)
            work_file.write_text("transient pma work file")

            sweep = load_sweep(copied_root)

        relative_path = "cases/case-000-memory_limit_8g/runs/run-0/work/replay-pma/0.pma"
        self.assertNotIn(
            relative_path,
            {record.relative_path for record in sweep.artifact_inventory},
        )
        self.assertNotIn(
            relative_path,
            {record.relative_path for record in sweep.cases[0].runs[0].artifacts},
        )

    def test_load_sweep_reads_case_cpu_profile_metadata_when_present(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "docker_minimal")

        cpu_profile = sweep.cases[0].cpu_profile
        self.assertIsNotNone(cpu_profile)
        assert cpu_profile is not None
        self.assertEqual(cpu_profile["profiler_kind"], "samply")
        self.assertEqual(
            cpu_profile["output_relative_path"],
            "profiles/samply-profile.json.gz",
        )
        self.assertEqual(
            cpu_profile["symbol_dir_relative_path"],
            "symbols",
        )
        self.assertEqual(
            cpu_profile["symbol_binary_relative_path"],
            "symbols/nockchain-bench",
        )

    def test_load_sweep_rejects_missing_required_matrix_file(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            copied_root = Path(temp_dir) / "docker_minimal"
            shutil.copytree(FIXTURE_DIR / "docker_minimal", copied_root)
            (copied_root / "matrix.json").unlink()

            with self.assertRaises(ValidationError):
                load_sweep(copied_root)

    def test_load_sweep_accepts_native_pma_fixture(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "native_pma_minimal")

        self.assertEqual(sweep.execution_mode, "native")
        self.assertEqual(len(sweep.cases), 1)
        self.assertEqual(sweep.cases[0].execution_mode, "native")
        self.assertEqual(sweep.cases[0].provenance["runtime_flavor"], "pma")
        self.assertEqual(sweep.cases[0].provenance["boot_source"], "checkpoint")
        self.assertEqual(sweep.cases[0].provenance["boot_event_num"], 42)

    def test_load_sweep_accepts_fixture_axis_pma_fixture(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "native_fixture_axis_pma")

        self.assertEqual(sweep.execution_mode, "native")
        self.assertEqual(len(sweep.cases), 2)
        fixture_identities = {
            case.resolved_case["fixture_sha256_hex"]
            for case in sweep.cases
        }
        boot_events = {case.provenance["boot_event_num"] for case in sweep.cases}

        self.assertEqual(len(fixture_identities), 2)
        self.assertEqual(boot_events, {42, 84})

    def test_load_sweep_reads_raw_tx_step_rows_when_present(self) -> None:
        sweep = load_sweep(FIXTURE_DIR / "raw_tx_snapshot_minimal")
        run = sweep.cases[0].runs[0]

        self.assertFalse(hasattr(run, "steps"))
        self.assertTrue(run.raw_tx_replay["active"])
        self.assertEqual(run.raw_tx_replay["step_count"], 3)
        self.assertEqual(run.raw_tx_replay["raw_tx_pokes_completed"], 3)
        self.assertEqual(
            run.raw_tx_replay["error_rows"][0]["raw_tx_pokes_completed"],
            0,
        )
        self.assertIn(
            "cases/case-000-threads_1/runs/run-0/steps.ndjson",
            {record.relative_path for record in sweep.artifact_inventory},
        )

    def test_load_sweep_rejects_malformed_present_steps_ndjson(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            copied_root = Path(temp_dir) / "raw_tx_snapshot_minimal"
            shutil.copytree(FIXTURE_DIR / "raw_tx_snapshot_minimal", copied_root)
            steps_path = (
                copied_root
                / "cases/case-000-threads_1/runs/run-0/steps.ndjson"
            )
            steps_path.write_text('{"ok": true}\nnot-json\n')

            with self.assertRaises(ValidationError) as context:
                load_sweep(copied_root)

        self.assertIn("steps.ndjson:2", str(context.exception))

    def test_load_sweep_rejects_non_object_steps_ndjson_row(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            copied_root = Path(temp_dir) / "raw_tx_snapshot_minimal"
            shutil.copytree(FIXTURE_DIR / "raw_tx_snapshot_minimal", copied_root)
            steps_path = (
                copied_root
                / "cases/case-000-threads_1/runs/run-0/steps.ndjson"
            )
            steps_path.write_text('{"ok": true}\n[]\n')

            with self.assertRaises(ValidationError) as context:
                load_sweep(copied_root)

        self.assertIn("steps.ndjson:2", str(context.exception))
        self.assertIn("expected object", str(context.exception))

    def test_load_sweep_rejects_malformed_raw_tx_metric_fields(self) -> None:
        cases: list[tuple[str, object]] = []
        for field, spec in RAW_TX_METRIC_SPECS.items():
            if spec.value_type == "integer":
                cases.extend(
                    [
                        (field, True),
                        (field, "2"),
                        (field, 2.5),
                    ]
                )
            elif spec.value_type == "number":
                cases.extend(
                    [
                        (field, True),
                        (field, "1.0"),
                        (field, float("nan")),
                        (field, float("inf")),
                    ]
                )
            else:  # pragma: no cover - guards future metric spec mistakes.
                self.fail(f"unknown raw-tx metric value type: {spec.value_type}")

        for field, value in cases:
            with self.subTest(field=field, value=repr(value)):
                with tempfile.TemporaryDirectory() as temp_dir:
                    copied_root = Path(temp_dir) / "raw_tx_snapshot_minimal"
                    shutil.copytree(FIXTURE_DIR / "raw_tx_snapshot_minimal", copied_root)
                    steps_path = (
                        copied_root
                        / "cases/case-000-threads_1/runs/run-0/steps.ndjson"
                    )
                    row = {
                        "step_index": 0,
                        "type": "poke_archive_block",
                        "outcome": "ok",
                        field: value,
                    }
                    steps_path.write_text(json.dumps(row) + "\n")

                    with self.assertRaises(ValidationError) as context:
                        load_sweep(copied_root)

                message = str(context.exception)
                self.assertIn("steps.ndjson:1", message)
                self.assertIn(field, message)

    def test_load_sweep_accepts_partial_sweep_and_tracks_missing_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            partial_root = create_partial_sweep_fixture(Path(temp_dir))

            sweep = load_sweep(partial_root)

        self.assertEqual(sweep.completion_state, "incomplete")
        self.assertEqual(
            sweep.missing_top_level_artifacts,
            ["comparison.json", "verdict.json"],
        )
        self.assertEqual(len(sweep.cases), 3)
        self.assertEqual(
            [case.case_id for case in sweep.cases],
            [
                "case-000-memory_limit_8g",
                "case-001-memory_limit_4g",
                "case-002-memory_limit_2g",
            ],
        )
        self.assertEqual(sweep.cases[0].completion_state, "complete")
        self.assertEqual(sweep.cases[1].completion_state, "partial")
        self.assertEqual(
            sweep.cases[1].missing_artifacts,
            ["summary.json", "verdict.json"],
        )
        self.assertEqual(sweep.cases[2].completion_state, "missing")
        self.assertEqual(
            sweep.cases[2].missing_artifacts,
            [
                "provenance.json",
                "requested_case.json",
                "resolved_case.json",
                "summary.json",
                "verdict.json",
            ],
        )
        self.assertIsNotNone(sweep.cases[2].requested_case)
        self.assertEqual(
            sweep.cases[2].requested_case["execution"]["Docker"]["memory_limit"],
            "2g",
        )


if __name__ == "__main__":
    unittest.main()
