# Relay Service

Phase 3A turns relay into a real transport service.

It owns:
- durable package queueing
- directed-link scheduling
- blackout deferral
- delivery retries
- importer ack polling

It does not own:
- GRANDPA verification
- summary-header storage-proof verification
- export Merkle-proof verification
- destination-chain submission logic

HTTP surface:
- `POST /api/v1/packages` accepts `SCALE(RelayPackageEnvelopeV1)`

CLI:
- `ialp-summary-relay run --transport-config <path>`
- `ialp-summary-relay status ...`
- `ialp-summary-relay show ...`
