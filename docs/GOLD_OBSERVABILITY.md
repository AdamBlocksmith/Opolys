# Gold Observability Layer

Opolys should use gold language where it clarifies the system, not where it
adds gimmicks or hidden monetary rules. This document defines three read-only
concepts that strengthen the digital-gold analogy without changing consensus
economics.

These are observability features:

- no new issuance
- no new burn
- no new passive yield
- no new slashing reason
- no change to fungibility

## Hallmarks

In physical gold markets, a refiner hallmark identifies who refined or
certified a bar. The hallmark is provenance, not ownership of future yield.

In Opolys:

```text
hallmark = public refiner service history
```

A refiner hallmark view should summarize:

```text
refiner_id
current_status
current_bonded_stake
refined_blocks_produced
ordinary_fees_earned_from_refined_blocks
valid_attestations_included_on_chain
refiner_blocks_finalized_with_help_from_this_refiner
double_sign_slash_events
last_signed_height
consecutive_correct_attestations
```

RPC:

```text
opl_getRefinerHallmark([refiner_id_hex])
opl_getRefinerHallmark([refiner_id_hex, lookback_blocks])
```

The first implementation is intentionally bounded: it scans at most one epoch
of recent blocks, even if a larger lookback is requested. That keeps public RPC
safe from deep-history scan abuse. Lifetime hallmark totals can be added later
as a persisted cumulative index without changing consensus.

Rules:

- Hallmarks do not affect producer selection.
- Hallmarks do not affect finality weight.
- Hallmarks do not create fee shares or passive income.
- Missed attestations should be reputation data, not a slash reason.
- Slashing remains limited to objective cryptographic evidence.

Gold analogy:

```text
a refinery builds reputation by stamping valid bars over time
```

## Assay Certificates

In physical gold, an assay certificate records weight, purity, refiner, date,
and test result. It explains what happened to the metal.

In Opolys:

```text
assay_certificate = per-block economic receipt
```

Every block can be explained with a certificate containing:

```text
height
block_hash
production_kind = genesis | mined | refined
producer
difficulty
vein_yield_milli
gross_mined_reward
mine_assay_burned
miner_credit
successful_transaction_count
ordinary_fees
ordinary_fees_burned
ordinary_fees_paid_to_refiner
bond_unbond_assay_burned
slashed_stake_burned
total_burned
```

RPC:

```text
opl_getBlockAssayCertificate([height])
opl_getBlockAssayCertificate([block_hash_hex])
```

The certificate is persisted as a non-consensus sidecar when a block is applied.
It does not change the block hash, PoW input, state root, or validity rules.
Persisting it avoids guessing dynamic bond/unbond assay burns later, because
those burns depend on live refiner state at block-application time.

Mined block example:

```text
gross_mined_reward = 71.142856 OPL
mine_assay_burned = 0.196021 OPL
ordinary_fees_burned = 0.040000 OPL
net_miner_credit = 70.946835 OPL
ordinary_fees_paid_to_refiner = 0
```

Refined block example:

```text
gross_mined_reward = 0
mine_assay_burned = 0
ordinary_fees_burned = 0
ordinary_fees_paid_to_refiner = 0.040000 OPL
net_new_issuance = 0
```

Rules:

- An assay certificate is a view of existing consensus data.
- It must not become a separate signed object required for block validity.
- It is persisted beside the block as exact observability data.
- It should make fee routing and burns visible.

Gold analogy:

```text
the block comes with a transparent assay receipt
```

## Mint Ledger

Serious gold markets track how metal enters, leaves, and changes custody. A
mint ledger is the aggregate accounting view.

In Opolys:

```text
mint_ledger = chain-wide supply and burn accounting
```

The ledger should expose:

```text
total_issued
total_burned
circulating_supply = total_issued - total_burned
total_mined_gross_reward
total_mine_assay_burned
total_ordinary_fees_burned
total_bond_unbond_assay_burned
total_slashed_stake_burned
total_refiner_fee_income
total_refined_blocks
total_mined_blocks
```

Rules:

- Mint Ledger is accounting, not governance.
- No treasury exists.
- Burns are not collected by anyone.
- Refiner fee income is explicit user-paid activity, not interest.

Gold analogy:

```text
the mint book proves what was mined, assayed, burned, and paid for service
```

## Suggested RPC Direction

These names are intentionally descriptive. They can be implemented after the
core launch flow is stable:

```text
opl_getRefinerHallmark(refiner_id)
opl_getBlockAssayCertificate(height_or_hash)
opl_getMintLedger()
```

The Mint Ledger is maintained as an aggregate in persisted chain state so the
RPC can answer in constant time instead of scanning every historical block.
Block Assay Certificates remain the per-block receipts that explain each ledger
increment.

## Non-Goals

Do not use these features to introduce:

- refiner bonuses
- holder penalties
- dormant-account burns
- fee shares for attestations
- finality fees
- treasury fees
- non-fungible OPL labels
- governance weights
- extra slashing reasons based on judgment calls

The gold analogy should make Opolys easier to understand, not harder to trust.
