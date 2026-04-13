# Summary Importer

Phase 2B turns the importer into a real protocol participant.

The importer:
- exposes an HTTP ingest endpoint for relay delivery
- persists structurally valid inbound packages before verification
- verifies package hash integrity
- verifies the GRANDPA/finality-context certificate
- verifies the summary-header storage proof
- verifies one or more `ExportV1` proofs against `header.export_root`
- enforces target-domain matching and duplicate protection by `export_id`
- submits `observe_verified_import` to the destination chain
- persists package-level ack state plus per-export verification results

It records `remote_observed` only. Recipient credit and `remote_finalized`
remain deferred.
