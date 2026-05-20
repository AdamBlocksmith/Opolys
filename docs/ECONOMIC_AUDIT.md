# Economic Audit Pass

This pass checks the current Opolys economics after the Proof-of-Refinement,
fee-routing, assay, and dynamic-fee changes.

## Result

No consensus rule change is required from this pass. The pass strengthened
economic invariant test coverage.

The current model is internally consistent with the latest direction:

- mined blocks are the only source of new OPL
- refined blocks issue no new OPL
- refiners earn only explicit user-paid ordinary fees
- no yearly stake decay remains
- no passive refiner yield remains
- no finality fee or attestation fee exists
- bond and unbond assays are burned, not paid
- normal fees burn in mined blocks and pay the selected refiner in refined blocks
- difficulty can recover downward when blocks are slow
- consensus floor prevents difficulty from falling below bonded-security pressure

## Supply

Equation:

```text
circulating_supply = total_issued - total_burned
```

Interpretation:

- `total_issued` grows only from mined blocks.
- `total_burned` grows from fees, assays, mine assay, and slashed stake.
- Refined blocks move existing OPL; they do not mint.

Status: clean.

## Economic Invariant Coverage

The node-level invariant test `supply_accounting_invariant` now walks through:

- genesis accounting
- mined issuance and mine-assay burn
- mined bond transaction fee and bond-assay burn
- bonded stake remaining part of accounted supply
- Proof-of-Refinement fee routing with zero issuance
- failed transaction handling with no fee or assay burn

Invariant:

```text
total_issued - total_burned
    = account_balances + bonded_stake + pending_unbonding_stake
```

Interpretation:

- Only mined blocks mint new OPL.
- Refined blocks move existing OPL only.
- Mined-block ordinary fees are burned.
- Refined-block ordinary fees split between POR fee burn and selected refiner
  income.
- Failed transactions do not pay fees or assays.

Status: covered by tests.

## Mined Issuance

Equation:

```text
base_component = BASE_REWARD / effective_difficulty
vein_yield = 1 + sqrt(ln(target / hash_int))
gross_mined_reward = base_component * vein_yield
mine_assay = gross_mined_reward * sqrt(effective_difficulty) / EPOCH
miner_credit = gross_mined_reward - mine_assay
```

Interpretation:

- Harder extraction lowers the base component.
- Better hash luck models richer ore.
- Mine assay rises with difficulty pressure.

Status: clean, but vein yield should stay documented as ore-grade variance, not
as gambling language.

## Proof Of Refinement

Equation:

```text
refined_block_issuance = 0
active_refiner_limit = EPOCH + sqrt(total_issued_opl)
por_fee_burn = ordinary_fees * active_refiners / (active_refiner_limit + active_refiners)
refiner_fee_income = ordinary_fees - por_fee_burn
```

Interpretation:

- Refiners provide service only when miners are absent or slow.
- Refiners do not earn from time, bonding alone, or attestations alone.
- POR fees burn a system-derived assay portion before paying the selected
  refiner producer.

Status: clean.

## Fee Routing

Equation:

```text
if block is mined:
    ordinary_fees are burned

if block is refined:
    ordinary_fees split between POR fee burn and selected refiner income
```

Interpretation:

- Activity pays for inclusion.
- The producer type decides whether the fee is market attrition or service
  income.
- Refined-block fee burn rises from the active refiner market itself, not from
  a hand-picked percentage.

Status: clean.

## Suggested Fee

Equation:

```text
current_average_fee = previous_block_fee_signal / successful_transaction_count
suggested_fee = max(
    MIN_FEE,
    (current_average_fee + (CAPACITY_RATIO - 1) * previous_suggested_fee) / CAPACITY_RATIO
)
```

Interpretation:

- Suggested fee responds gradually to recent user-paid fees.
- Empty blocks decay the quote toward `MIN_FEE`.

Status: clean. The smoothing memory is derived from `CAPACITY_RATIO`, so the
fee quote reacts across the same block-capacity window that bounds the mempool.

## Queue-Depth Fee Floor

Equation:

```text
pending_blocks = ceil(mempool_bytes / MAX_BLOCK_SIZE_BYTES)
fee_multiplier = clamp(pending_blocks, 1, CAPACITY_RATIO)
effective_min_fee = suggested_fee * fee_multiplier
CAPACITY_RATIO = ceil(MEMPOOL_MAX_SIZE_BYTES / MAX_BLOCK_SIZE_BYTES)
```

Interpretation:

- More pending block-sized work raises the minimum fee gradually.
- The maximum multiplier is derived from mempool capacity.

Status: clean.

## Bond Minimum

Equation:

```text
minimum_new_bond_entry = max(1 OPL, sqrt(total_issued_opl)) OPL
```

Interpretation:

- Early participation remains accessible.
- The required posted metal grows with the economy.
- Old residual entries from FIFO unbonding are not forcibly kicked.

Status: clean.

## Active Refiner Limit

Equation:

```text
active_refiner_limit = EPOCH + sqrt(total_issued_opl)
```

Interpretation:

- The active set grows organically with issued supply.
- Excess eligible refiners wait instead of being rejected.

Status: clean.

## Bond And Unbond Assays

Shared equation:

```text
dynamic_assay(amount, pressure_numerator, pressure_denominator, active_limit)
```

Bond:

```text
baseline = minimum_new_bond_entry * active_refiner_limit
bonded_after = total_bonded_stake + bond_amount
bond_assay = dynamic_assay(bond_amount, bonded_after, baseline, active_refiner_limit)
```

Unbond:

```text
baseline = minimum_new_bond_entry * active_refiner_limit
total_bonded = total_bonded_stake
unbond_assay = dynamic_assay(unbond_amount, baseline, total_bonded, active_refiner_limit)
```

Interpretation:

- Bonding assay rises when incoming bonded metal crowds the vault.
- Unbonding assay rises when bonded security is thin.
- Both burn to nobody.

Status: clean.

## Slashing

Current slash reason:

```text
double-signing by a refiner
```

Interpretation:

- Slashing is tied to objective cryptographic evidence.
- Slashed stake burns; no treasury or accuser receives it.

Status: clean. Future double-attestation slashing is possible, but should only
be added if it remains objective and compactly provable.

## Difficulty Recovery

Equation:

```text
retarget_difficulty = current_difficulty * expected_epoch_time / actual_epoch_time
consensus_floor = total_issued / bonded_stake
effective_difficulty = max(retarget_difficulty, consensus_floor, MIN_DIFFICULTY)
```

Interpretation:

- Slow blocks lower retarget difficulty.
- Proof-of-Refinement can keep the chain moving while mining becomes viable
  again.
- The floor prevents difficulty from dropping below bonded-security pressure.

Status: acceptable. Keep monitoring the mine/refine cycling incentive, but do
not split POR out of retarget until a simulation shows actual abuse.

## Remaining Watch Items

These are not blockers, but they should stay visible:

1. Suggested fee smoothing is derived from `CAPACITY_RATIO`; keep watching
   whether the capacity-derived memory feels right under real activity.
2. Vein yield should be communicated carefully so it reads as ore variance.
3. Difficulty recovery via POR should be modeled under adversarial cycling.
4. Hallmarks, Assay Certificates, and Mint Ledger should start as computed
   observability views before adding persisted indexes.
5. Do not add extra analogies that change money mechanics without a separate
   economic review.

## Recommendation

Move forward with launch-rehearsal preparation after the observability docs are
merged. The economics are coherent enough for the next launch rehearsal, and
the remaining questions are modeling and UX/spec work rather than urgent
consensus fixes.
