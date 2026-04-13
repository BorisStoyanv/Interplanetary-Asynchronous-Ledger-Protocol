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
- exposes operator inspection commands

It does not yet implement relay transport, importer logic, or aggregator
behavior.
