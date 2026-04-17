# Interplanetary Asynchronous Ledger Protocol

IALP is a proof-bearing, delay-aware multi-domain ledger prototype built around
the idea that cross-domain coordination should survive real transport latency,
blackouts, retries, and asynchronous finality.

This repository contains the local chain runtime, transport services, shared
types, and scenario harness for a three-domain model:
- Earth
- Moon
- Mars

The current accepted baseline goes beyond simple transfer export/import. It now
includes:
- real local domain chains for Earth, Moon, and Mars
- on-chain epoch summaries
- certified summary packages with GRANDPA / finality-context certification
- summary-header storage proofs
- export inclusion proofs
- finalized import proofs for Phase 4 settlement
- real `export_root`, `import_root`, and `governance_root`
- relay transport with delay, blackout, retry, and replay-safe delivery
- importer-side verification and on-chain application
- full cross-domain settlement lifecycle
- Phase 5 governance transport with scheduled future activation of
  `protocol_version`

## What IALP Proves

IALP is modeling two things at once:

1. Delayed cross-domain value transfer
2. Delayed cross-domain coordination

The important architectural rule is that both flows use the same pattern:
- canonical on-chain source object
- committed epoch summary root
- proof-bearing certified package
- transport-only relay
- verifier/importer on the destination side
- canonical on-chain remote effect

Governance does not get a shortcut. It uses the same proof-bearing transport
architecture as transfers.

## Current Architecture

At a high level, the system looks like this:

```text
source chain
  -> epoch close commits summary roots
  -> exporter certifies summary and builds proof-bearing package
  -> relay stores, delays, retries, and delivers bytes
  -> importer verifies proofs and submits on-chain claims
  -> destination chain records canonical remote state
```

Transfer and governance packages both ride this path. The relay does not decide
economic correctness, governance correctness, or activation readiness.

## Implemented Today

### Transfers and settlement
- source-side held funds
- summary commitment of exports
- proof-bearing remote observation
- destination credit
- reverse completion package
- source-side hold resolution exactly once

### Governance
- real on-chain proposals
- token-weighted voting with quorum and simple majority
- deterministic `governance_root`
- proof-bearing governance inclusion proofs
- remote on-chain acknowledgment
- activation scheduled for a future epoch
- visible on-chain effect via `protocol_version`

## Repository Layout

```text
chain/
  node/                 Substrate node binary and chain spec wiring
  runtime/              Runtime composition
  pallets/
    domain/             Domain identity and chain identity state
    epochs/             Epoch accounting and summary creation
    transfers/          Cross-domain transfer and settlement state
    governance/         Governance proposals, votes, acks, activation

crates/
  common-config/        TOML config loading and validation
  common-types/         Shared hashes, proof types, package types, storage keys

services/
  exporter/             Builds certified summary packages
  relay/                Delay / blackout / retry transport
  importer/             Verifies packages and submits destination claims
  aggregator/           Reserved for later phases
  dashboard/            Reserved / auxiliary UI work

tests/
  smoke/                Basic local node smoke tests
  scenario-harness/     Canonical end-to-end scenario runner
  scenarios/            Convenience wrappers and scenario assets

config/
  domains/              Default Earth / Moon / Mars domain configs
  transport/            Local transport topology

docs/                   Detailed protocol and implementation spec pack
```

## Prerequisites

The repo is pinned in [rust-toolchain.toml](/Users/borisstoyanov/Desktop/Interplanetary-Asynchronous-Ledger-Protocol/rust-toolchain.toml):
- Rust `1.88.0`
- `rustfmt`
- `clippy`
- `rust-src`
- `wasm32-unknown-unknown`

Install the pinned toolchain and target with rustup:

```bash
rustup toolchain install 1.88.0 --component rustfmt --component clippy --component rust-src
rustup target add wasm32-unknown-unknown --toolchain 1.88.0
```

You will also want:
- `curl`
- a rustup-managed toolchain on your `PATH`

If Homebrew `cargo` or `rustc` shadow rustup on your machine, runtime WASM
builds can fail even when the target is installed. In that case, invoke the
rustup-managed binaries explicitly or prepend the rustup toolchain `bin/`
directory to `PATH`.

## Quick Start

### Build the workspace

```bash
cargo build --locked
```

### Run a local node

```bash
cargo run -p ialp-node -- --domain earth --tmp --validator
```

### Build a chain spec

```bash
cargo run -p ialp-node -- build-spec --domain earth > /tmp/earth.json
```

### Run a smoke test

```bash
bash tests/smoke/earth_two_node.sh
```

## End-to-End Scenarios

The canonical integration entrypoint is the scenario harness:

```bash
target/debug/ialp-scenario-harness run --scenario earth-to-moon-success --json
```

Available scenarios:
- `earth-to-moon-success`
- `earth-to-mars-delay`
- `earth-to-mars-blackout`
- `earth-to-moon-relay-restart`
- `earth-to-moon-governance-activation`

Run the full suite:

```bash
target/debug/ialp-scenario-harness run-all --json
```

Scenario artifacts are written under `target/scenarios/...` and include:
- generated configs
- node data directories
- exporter / relay / importer stores
- logs
- `summary.json`

The governance scenario is the current showpiece for Phase 5. It proves:
- Earth authors and approves a proposal on-chain
- the proposal is committed into `governance_root`
- a certified governance package is relayed to Moon
- Moon verifies and acknowledges it on-chain
- Moon exports an ack package back to Earth
- both domains activate the change at a future epoch, not immediately
- duplicate replay does not double-activate

## Useful Development Commands

Fast proof/service test sweep:

```bash
SKIP_WASM_BUILD=1 cargo test \
  -p ialp-common-types \
  -p pallet-ialp-governance \
  -p ialp-summary-exporter \
  -p ialp-summary-importer \
  -p ialp-summary-relay
```

Build the main runtime/service path:

```bash
cargo build \
  -p ialp-node \
  -p ialp-summary-exporter \
  -p ialp-summary-relay \
  -p ialp-summary-importer \
  -p ialp-scenario-harness
```

## Documentation Map

The detailed design lives in `docs/`. If you are new to the repo, start here:

1. `docs/00_START_HERE.md`
2. `docs/01_PRODUCT_AND_MVP_GOALS.md`
3. `docs/02_SYSTEM_MODEL_AND_ARCHITECTURE.md`
4. `docs/03_PROTOCOL_RULES_SOURCE_OF_TRUTH.md`
5. `docs/05_STATE_MACHINES_AND_INVARIANTS.md`
6. `docs/18_EPOCH_SUMMARY_SPEC.md`
7. `docs/19_SUMMARY_CERTIFICATION_AND_EXPORT_SPEC.md`
8. `docs/20_SUMMARY_STORAGE_PROOF_SPEC.md`
9. `docs/21_EXPORT_INCLUSION_PROOF_SPEC.md`
10. `docs/22_RELAY_TRANSPORT_AND_DELIVERY_SPEC.md`
11. `docs/23_END_TO_END_SCENARIO_HARNESS_SPEC.md`
12. `docs/25_GOVERNANCE_TRANSPORT_AND_ACTIVATION_SPEC.md`

If documents appear to conflict, use this precedence:

1. `docs/03_PROTOCOL_RULES_SOURCE_OF_TRUTH.md`
2. `docs/05_STATE_MACHINES_AND_INVARIANTS.md`
3. `docs/04_DATA_MODEL_AND_PERSISTENCE.md`
4. then the remaining documents

## Design Principles

- Keep canonical truth on-chain.
- Keep transport proof-bearing.
- Keep relay transport-only.
- Prefer deterministic, replay-safe state transitions.
- Do not bypass accepted proof/package contracts just because a new feature is
  being added.

## Status

The repo is no longer just a design pack. It is an executable prototype with:
- real Substrate-based domain runtimes
- real exporter / relay / importer services
- real end-to-end transfer settlement
- real end-to-end governance transport with scheduled activation

Later phases are still open for:
- richer governance payloads
- runtime-upgrade artifact handling
- aggregator/global ledger completion
- production hardening and benchmarking
