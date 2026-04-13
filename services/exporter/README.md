# Summary Exporter

Phase 2B owns summary export certification, summary-header storage proof
packaging, and per-export inclusion proof packaging.

The exporter:
- reads staged epoch summaries from on-chain storage
- asks the node for proof-aware certification readiness
- reads canonical epoch export records from normal storage
- recomputes `header.export_root` locally before packaging
- emits one deterministic package per `(epoch_id, target_domain)`
- persists export-certified `CertifiedSummaryPackage` artifacts locally
- submits certified packages to relay over HTTP using `RelayPackageEnvelopeV1`
- tracks relay handoff state in schema-4 local storage
- exposes operator inspection commands

It still does not implement relay scheduling, importer verification, or
aggregator behavior.
