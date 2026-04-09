# Summary Exporter

Phase 2A owns summary export certification plus summary-header storage proof
packaging.

The exporter:
- reads staged epoch summaries from on-chain storage
- asks the node for proof-aware certification readiness
- persists export-certified `CertifiedSummaryPackage` artifacts locally
- exposes operator inspection commands

It does not yet implement relay transport, importer logic, or aggregator
behavior.
