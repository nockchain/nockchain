Why it was added

It's the write side of a "behavior event" stream for oracle-vs-native parity debugging. The pieces:

- log_hint_event tags every event with engine "oracle" — the same vocabulary as hoon/tests/parser-oracle/: in honk's parity work, the interpreted pipeline (nockvm running the Hoon compiler) is the reference oracle and
honk's native compiler is the thing being checked against it.
- The events it records — hint.push/hint.pop for %spot/%mean/%hand/%hunk/%lose, plus hint.slog — are exactly a source-level dynamic trace of the interpreted run: %spot push/pop gives you source spans entered and exited in
order, %mean gives crash-trace context, %slog gives printf output, all timestamped and sequenced (event_id, ts_ns) into nock-trace.parquet.
- The point of that: when honk's native compiler diverges from the interpreted one, the hardest problem is localizing the divergence. An ordered behavior stream from the oracle side, queryable offline (parquet →
DuckDB/pandas), lets you find the first source span where the two executions disagree. The PathPrefixFilter added in the same diff (scope tracing to a jet-path prefix) is part of the same campaign.
- The design clearly anticipated a second emitter: the schema has an engine column, and write_behavior_event takes location and trace_path parameters that the lone caller passes as None. A native-side ("honk") emitter would
have populated those — it never landed in this tree.

Is it still used?

No, on every axis I can check:

- Nothing turns it on. Emission requires --mode parquet on the nockapp trace CLI (TraceMode::CaptureParquet). No justfile recipe, test, script, or doc in the repo passes it. In a normal hoonc/honk/nockchain run, trace_info
is None and zero events are emitted.
- One emitter, zero readers. write_behavior_event_safe has exactly one caller in the repository (log_hint_event), the engine is always "oracle", and there is no consumer anywhere: honk-tools ships only
extract_hoonc_octs_type and jam_diff, neither reads parquet; no .py/.sql/.sh/doc references nock-trace.parquet, hint.push, or behavior events at all.
- Zero tests. parquet_backend.rs has no #[test] and nothing exercises write_behavior_event.
- The half of the comparison that would make the stream meaningful (the native-side emitter and a diffing tool) doesn't exist in-tree — whatever querying happened was ad hoc and out-of-tree, during a debugging session that
appears concluded.

The kicker: you pay for it even though it's off

The real offense isn't the dormant feature, it's that the cost is unconditional. write_behavior_event_safe does check context.trace_info and return immediately — but log_hint_event builds everything before that check:
fast_noun_space(), UTF-8 validation of the tag atom, a String from atom_text, and a format! allocation, on every transparent-hint push and pop and every %slog, in every run, tracing or not. Compiler code is saturated with
~|/~_ mean hints, so this is a steady allocator tax on hoonc for a stream nobody ever reads.

# Instructions

1. Drop the three log_hint_event call sites (interpreter.rs:2061,2077,2149) and the helper. This loses nothing any in-tree consumer depends on. Note this is separable from ParquetBackend itself — the backend also records the ordinary serf/nock trace rows (path_parts, chum, elapsed_us), which is an independently useful capture mode you can keep along with PathPrefixFilter. Remove `ParquetBackend` and any other functionality no longer needed as a result.
