# IALP Detailed Agent Spec Pack

This pack is a **detailed implementation spec** for AI coding agents building the
Interplanetary Asynchronous Ledger Protocol (IALP) MVP.

Phase 0 now includes a working Rust bootstrap under `chain/` with real Aura +
Grandpa consensus, domain-aware chain specs, reserved epoch summary storage,
and typed Earth/Moon/Mars configs.

## Read order

1. `docs/00_START_HERE.md`
2. `docs/01_PRODUCT_AND_MVP_GOALS.md`
3. `docs/02_SYSTEM_MODEL_AND_ARCHITECTURE.md`
4. `docs/03_PROTOCOL_RULES_SOURCE_OF_TRUTH.md`
5. `docs/04_DATA_MODEL_AND_PERSISTENCE.md`
6. `docs/05_STATE_MACHINES_AND_INVARIANTS.md`
7. `docs/06_LOCAL_DOMAIN_CHAIN_SPEC.md`
8. `docs/07_INTERPLANETARY_TRANSPORT_SPEC.md`
9. `docs/08_GLOBAL_LEDGER_SPEC.md`
10. `docs/09_GOVERNANCE_SPEC.md`
11. `docs/10_TOKEN_AND_TRANSFER_SPEC.md`
12. `docs/11_NODE_ROLES_AND_OPERATOR_UX.md`
13. `docs/12_APIS_EVENTS_AND_SERVICE_BOUNDARIES.md`
14. `docs/13_REPOSITORY_AND_MODULE_LAYOUT.md`
15. `docs/14_PHASED_IMPLEMENTATION_PLAN.md`
16. `docs/15_TEST_PLAN_ACCEPTANCE_AND_SCENARIOS.md`
17. `docs/16_RISKS_OPEN_QUESTIONS_AND_DECISION_LOG.md`
18. `docs/17_PHASE0_BOOTSTRAP_DECISIONS.md`

Phase 0 provenance appendix:
- `docs/PROVENANCE_PHASE0.md`

## Priority rule

If files appear to conflict, use this precedence:

1. `03_PROTOCOL_RULES_SOURCE_OF_TRUTH.md`
2. `05_STATE_MACHINES_AND_INVARIANTS.md`
3. `04_DATA_MODEL_AND_PERSISTENCE.md`
4. all remaining files

## Purpose

This pack is intentionally long and repetitive. The repetition is deliberate.
An AI coding agent should be able to open a single file and still find enough
context to avoid inventing incompatible behavior.

## Phase 0 Quickstart

Prerequisites:
- Rust `1.88.0` with `rustfmt`, `clippy`, `rust-src`, and `wasm32-unknown-unknown`
- `curl`

Common commands:

```bash
cargo build -p ialp-node --locked
cargo run -p ialp-node -- --domain earth --tmp --validator
cargo run -p ialp-node -- build-spec --domain earth > /tmp/earth.json
bash tests/smoke/earth_two_node.sh
```

The Phase 0 bootstrap intentionally uses real consensus rather than manual seal
so later Earth/Moon/Mars multi-node work can reuse the same node architecture.
