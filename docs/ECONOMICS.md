# Opolys Economics

This document is the current economic map of Opolys. It explains where OPL
comes from, where it goes, what burns, what pays refiners, and which equations
drive each mechanism. For read-only gold observability, see
[`GOLD_OBSERVABILITY.md`](GOLD_OBSERVABILITY.md). For the latest economics
review, see [`ECONOMIC_AUDIT.md`](ECONOMIC_AUDIT.md).

Opolys is designed as digital gold:

- New OPL enters circulation only through mined blocks.
- Proof of Refinement creates no new OPL.
- Ordinary fees are activity-based.
- Assays are burned as refining/vault friction.
- Refiner stake is slashable service collateral, not interest-bearing capital.
- There is no treasury, no premine, no governance emission, and no passive
  staking yield.

All consensus amounts are stored in Flakes:

```text
1 OPL = 1,000,000 Flakes
```

## Supply Accounting

The chain tracks three supply numbers:

```text
total_issued      = gross OPL ever mined
total_burned      = fees + assays + slashed stake + mine assay
circulating_supply = total_issued - total_burned
```

Gold analogy:

- `total_issued` is all ore ever extracted.
- `total_burned` is ore lost to assay, waste, fees, and bad refinery behavior.
- `circulating_supply` is refined gold still in the economy.

## Genesis Base Reward

The base reward is set once by the genesis ceremony from annual world gold
mine production.

Default fallback:

```text
annual_gold_oz = annual_production_tonnes * 32,150.7
blocks_per_year = 365.25 * 86,400 / 90 = 350,640
BASE_REWARD = floor(annual_gold_oz / blocks_per_year)
```

With the fallback numbers:

```text
annual_gold_oz = 3,630 * 32,150.7 = about 116,707,041 oz
BASE_REWARD = floor(116,707,041 / 350,640) = 332 OPL
```

Mainnet does not hardcode this fallback. Nodes read the ceremony-signed
`base_reward_flakes` from `genesis_attestation.json`.

Gold analogy:

The genesis ceremony sets the initial "world mine output per block."

## Difficulty

Difficulty has two parts:

```text
retarget_difficulty = current_difficulty * expected_epoch_time / actual_epoch_time
consensus_floor = total_issued / bonded_stake
effective_difficulty = max(retarget_difficulty, consensus_floor, MIN_DIFFICULTY)
```

Constants:

```text
EPOCH = 960 blocks
BLOCK_TARGET_TIME = 90 seconds
expected_epoch_time = 960 * 90 seconds = 24 hours
MIN_DIFFICULTY = 1
```

If blocks arrive too fast, retarget difficulty rises. If blocks arrive too
slowly, it falls. The consensus floor prevents difficulty from falling below
the ratio of issued supply to bonded security.

Mined and refined blocks both carry timestamps and both enter the same rolling
retarget window. That means Proof of Refinement can keep the chain moving while
miners are absent; if those blocks arrive slower than the 90-second target, the
next epoch lowers the retarget difficulty and mining can become viable again.

Example:

```text
current_difficulty = 1,000

If the next epoch takes 48 hours instead of 24:
retarget_difficulty = 1,000 * 24 / 48 = 500

If the consensus_floor is 400:
effective_difficulty = max(500, 400, 1) = 500

If the next epoch takes 96 hours instead of 24:
retarget_difficulty = 1,000 * 24 / 96 = 250
effective_difficulty = max(250, 400, 1) = 400
```

Gold analogy:

When lots of miners arrive, easy ore disappears faster and extraction gets
harder. When mining slows, refineries can still move existing gold, and the
next assay period can make extraction easier again. Bonded refiner security
also becomes part of the economic floor.

## Mined Block Reward

Only mined blocks issue new OPL.

```text
base_component = BASE_REWARD / effective_difficulty
vein_yield = 1 + sqrt(ln(target / hash_int))
gross_mined_reward = base_component * vein_yield
mine_assay = gross_mined_reward * sqrt(effective_difficulty) / EPOCH
miner_credit = gross_mined_reward - mine_assay
```

In implementation, `vein_yield` is stored in milli-units:

```text
1000 = 1.000x
1500 = 1.500x
2000 = 2.000x
```

Then:

```text
gross_mined_reward = (base_component * vein_yield_milli) / 1000
```

Gold analogy:

The base component is expected ore. Vein yield is ore richness: sometimes a
miner finds a richer vein. The mine assay is extraction/refining waste that
rises with difficulty pressure.

Example at ceremony fallback reward:

```text
BASE_REWARD = 332 OPL
difficulty = 7
base_component = 332 / 7 = 47.428571 OPL

If vein_yield = 1.500x:
gross_mined_reward = 47.428571 * 1.5 = 71.142856 OPL
mine_assay = 71.142856 * sqrt(7) / 960
           = 71.142856 * 2.64575 / 960
           = about 0.196000 OPL burned
miner_credit = about 70.946856 OPL
```

Difficulty-pressure examples with `vein_yield = 1.000x`:

| Difficulty | Gross Reward | Mine Assay Burn | Assay Rate | Miner Credit |
|---:|---:|---:|---:|---:|
| 7 | 47.428571 OPL | 0.148214 OPL | 0.312% | 47.280357 OPL |
| 25 | 13.280000 OPL | 0.069167 OPL | 0.521% | 13.210833 OPL |
| 100 | 3.320000 OPL | 0.034583 OPL | 1.042% | 3.285417 OPL |
| 1,000 | 0.332000 OPL | 0.011067 OPL | 3.333% | 0.320933 OPL |
| 5,000 | 0.066400 OPL | 0.004911 OPL | 7.396% | 0.061489 OPL |

At difficulty `7`, richer veins scale both gross reward and assay burn:

| Vein Yield | Gross Reward | Mine Assay Burn | Miner Credit |
|---:|---:|---:|---:|
| 1.000x | 47.428571 OPL | 0.148214 OPL | 47.280357 OPL |
| 1.500x | 71.142857 OPL | 0.222321 OPL | 70.920536 OPL |
| 2.000x | 94.857143 OPL | 0.296429 OPL | 94.560714 OPL |
| 5.000x | 237.142857 OPL | 0.741071 OPL | 236.401786 OPL |

## Proof Of Refinement Blocks

Proof of Refinement is a stalled-chain service path. It does not issue new OPL.

```text
refined_block_issuance = 0
refined_block_mine_assay = 0
refiner_fee_income = ordinary_fees_in_that_refined_block
```

Gold analogy:

A refiner does not discover new gold. A refiner earns only when it provides
actual service by moving user activity during miner silence.

## Fee Routing

Every successful transaction has an explicit ordinary fee:

```text
tx.fee >= MIN_FEE
MIN_FEE = 1 Flake
```

Where that ordinary fee goes depends on block kind:

```text
if block is mined:
    ordinary_fees are burned

if block is refined:
    ordinary_fees are paid to selected refiner producer
```

Gold analogy:

In mined blocks, fees are market attrition. In refined blocks, fees are payment
for a real vault/assay service that kept the chain moving.

## Suggested Fee

The protocol suggests a fee from recent activity. It does not force a fixed fee
schedule beyond the one-Flake minimum.

```text
current_average_fee = previous_block_fee_signal / successful_transaction_count
CAPACITY_RATIO = ceil(MEMPOOL_MAX_SIZE_BYTES / MAX_BLOCK_SIZE_BYTES)
window = CAPACITY_RATIO
suggested_fee = max(
    MIN_FEE,
    (current_average_fee + (CAPACITY_RATIO - 1) * previous_suggested_fee) / CAPACITY_RATIO
)
```

For mempool admission, the effective minimum scales with the actual pending
queue depth:

```text
pending_blocks = ceil(mempool_bytes / MAX_BLOCK_SIZE_BYTES)
fee_multiplier = clamp(pending_blocks, 1, CAPACITY_RATIO)
effective_min_fee = suggested_fee * fee_multiplier
```

If the previous block was empty:

```text
current_average_fee = MIN_FEE
```

Gold analogy:

The suggested fee is a market quote, not a tax. It moves slowly like a posted
assay/shipping quote responding to recent demand.

Example:

```text
previous_suggested_fee = 1,000 Flakes
previous block had 1 tx with fee = 10,000 Flakes

suggested_fee = (10,000 + (CAPACITY_RATIO - 1) * 1,000) / CAPACITY_RATIO
              = 19,000 / 10       // with today's CAPACITY_RATIO = 10
              = 1,900 Flakes
```

If 100 transactions paid 100,000 total Flakes:

```text
current_average_fee = 100,000 / 100 = 1,000 Flakes
suggested_fee = (1,000 + (CAPACITY_RATIO - 1) * 1,000) / CAPACITY_RATIO = 1,000 Flakes
```

Queue-depth examples with `suggested_fee = 1,000 Flakes`:

| Pending Block-Sized Work | Effective Minimum Fee |
|---:|---:|
| 0.0 blocks | 1,000 Flakes |
| 0.5 blocks | 1,000 Flakes |
| 1.0 blocks | 1,000 Flakes |
| 1.1 blocks | 2,000 Flakes |
| 2.0 blocks | 2,000 Flakes |
| 5.0 blocks | 5,000 Flakes |
| 10.0 blocks | 10,000 Flakes |

## Bonding

Bonding locks OPL as refiner collateral.

```text
minimum_new_bond_entry = max(1 OPL, sqrt(total_issued_opl)) OPL
bond_cost = stake + ordinary_fee + bond_assay
```

The bond remains owned by the refiner, but it is locked and slashable.

New bond entries must meet the dynamic minimum. Residual entries created by
FIFO unbond splitting are not forced to top up.

Operators can read the current requirement from `opl_getChainInfo`:

```text
minimum_refiner_bond_flakes
minimum_refiner_bond_opl
```

Gold analogy:

Bonding is placing good-delivery collateral in the vault. As the economy gets
larger, a credible refinery needs more posted metal.

Examples:

```text
total_issued = 0 OPL
minimum_new_bond_entry = 1 OPL

total_issued = 1,000,000 OPL
minimum_new_bond_entry = sqrt(1,000,000) = 1,000 OPL

total_issued = 25,000,000 OPL
minimum_new_bond_entry = sqrt(25,000,000) = 5,000 OPL
```

## Bond And Unbond Assays

Bond and unbond assays are system-derived burns. They are not paid to anyone.

Shared helper:

```text
dynamic_assay(amount, pressure_numerator, pressure_denominator, active_limit)
    = amount * sqrt(pressure_numerator / pressure_denominator)
      / (DECIMAL_PLACES * sqrt(active_limit))
```

Because `DECIMAL_PLACES = 6`, the denominator includes:

```text
6 * sqrt(active_limit)
```

Bond assay:

```text
active_limit = EPOCH + sqrt(total_issued_opl)
baseline = minimum_new_bond_entry * active_limit
bonded_after = total_bonded_stake + bond_amount

bond_assay = dynamic_assay(
    bond_amount,
    bonded_after,
    baseline,
    active_limit
)
```

Unbond assay:

```text
active_limit = EPOCH + sqrt(total_issued_opl)
baseline = minimum_new_bond_entry * active_limit
total_bonded = total_bonded_stake

unbond_assay = dynamic_assay(
    unbond_amount,
    baseline,
    total_bonded,
    active_limit
)
```

Gold analogy:

Bonding assay rises when the vault is crowded with incoming bars. Unbonding
assay rises when bonded security is thin and withdrawals are more costly to
the system.

Example at launch:

```text
total_issued = 0
active_limit = 960
minimum_new_bond_entry = 1 OPL
baseline = 960 OPL
```

If total bonded after a 100 OPL bond is 100 OPL:

```text
bond_assay = 100 * sqrt(100 / 960) / (6 * sqrt(960))
           = about 0.1736 OPL burned
```

If total bonded is 10,000 OPL and someone unbonds 100 OPL:

```text
unbond_assay = 100 * sqrt(960 / 10,000) / (6 * sqrt(960))
             = about 0.1667 OPL burned
```

System-pressure examples:

| Scenario | Issued Supply | Minimum Bond | Active Limit | Total Bonded | Action | Assay Burn | Assay Rate |
|---|---:|---:|---:|---:|---:|---:|---:|
| Launch first refiner | 0 OPL | 1 OPL | 960 | 0 OPL | Bond 1 OPL | 0.000174 OPL | 0.0174% |
| Launch crowded vault | 0 OPL | 1 OPL | 960 | 1,000 OPL | Bond 1 OPL | 0.005493 OPL | 0.5493% |
| Launch thin withdrawal | 0 OPL | 1 OPL | 960 | 1,000 OPL | Unbond 1 OPL | 0.005270 OPL | 0.5270% |
| Mature baseline | 25,000,000 OPL | 5,000 OPL | 5,960 | 29,800,000 OPL | Bond 5,000 OPL | 10.795234 OPL | 0.2159% |
| Mature baseline | 25,000,000 OPL | 5,000 OPL | 5,960 | 29,800,000 OPL | Unbond 5,000 OPL | 10.794328 OPL | 0.2159% |
| Mature thin security | 25,000,000 OPL | 5,000 OPL | 5,960 | 5,000,000 OPL | Bond 5,000 OPL | 4.423739 OPL | 0.0885% |
| Mature thin security | 25,000,000 OPL | 5,000 OPL | 5,960 | 5,000,000 OPL | Unbond 5,000 OPL | 26.352314 OPL | 0.5270% |

## Refiner Active Set

The number of Active refiners grows with issued supply:

```text
active_refiner_limit = EPOCH + sqrt(total_issued_opl)
```

At epoch boundaries:

```text
eligible refiners are sorted by total stake descending
top active_refiner_limit become Active
others remain Waiting
```

Examples:

```text
total_issued = 0 OPL
active_refiner_limit = 960

total_issued = 1,000,000 OPL
active_refiner_limit = 960 + 1,000 = 1,960

total_issued = 25,000,000 OPL
active_refiner_limit = 960 + 5,000 = 5,960
```

Gold analogy:

The refining network expands as the monetary base expands.

## Refiner Producer Selection

If miners are silent, one Active refiner may produce a Proof-of-Refinement
block. Selection is weighted only by active bonded stake:

```text
chance_to_produce = refiner_total_active_stake / total_active_stake
```

The selection ticket is sampled with deterministic rejection sampling, not
`seed % total_stake`, so tiny modulo bias does not tilt consensus selection.

Gold analogy:

A larger posted vault bond gives more service responsibility, but there is no
age-based privilege and no passive income.

## Finality

Only refiner-produced blocks use refiner attestation finality. Mined blocks are
secured by EVO-OMAP PoW.

```text
finality_threshold = 667 / 1000 of active refiner weight
```

Attestation weight is stake weight:

```text
attestation_weight = refiner_total_stake
```

Gold analogy:

Refiners can hallmark refined blocks after the fact. Their hallmark weight
comes from posted collateral, not from time or interest.

## Unbonding

Unbonding uses FIFO:

```text
oldest bond entries are consumed first
unbonded stake enters queue
matures_at = current_height + EPOCH
```

During the delay:

```text
stake no longer counts for producer selection
stake no longer counts for finality weight
stake remains slashable
```

After maturity, the stake is credited back to the owner.

Gold analogy:

Withdrawing bars from the vault has a settlement delay. During that delay, the
operator is still responsible for misconduct already committed.

## Slashing

Slashing is only for double-signing.

```text
slash_burn = all bonded stake + matching pending unbonding stake
```

Slashed stake is burned, not paid to another participant.

Gold analogy:

A refiner caught stamping two conflicting bars loses its posted good-delivery
collateral. Nobody receives a bounty from it; the bad collateral is removed.

## Current Economic Flow Table

| Event | Sender Pays | Minted | Burned | Paid To Miner | Paid To Refiner |
|---|---:|---:|---:|---:|---:|
| Mined empty block | none | gross mined reward | mine assay | gross reward - mine assay | 0 |
| Mined transfer block | amount + fee | gross mined reward | tx fees + mine assay | gross reward - mine assay | 0 |
| Refined transfer block | amount + fee | 0 | 0 ordinary fee burn | 0 | tx fees |
| Bond in mined block | stake + fee + bond assay | mined reward | fee + bond assay + mine assay | mined net reward | 0 |
| Bond in refined block | stake + fee + bond assay | 0 | bond assay | 0 | fee |
| Unbond in mined block | fee + unbond assay | mined reward | fee + unbond assay + mine assay | mined net reward | 0 |
| Unbond in refined block | fee + unbond assay | 0 | unbond assay | 0 | fee |
| Double-sign slash | stake | 0 | slashed stake | 0 | 0 |

## What Is Intentionally Not In The Model

Opolys currently has no:

- passive staking yield
- yearly stake decay
- refiner block subsidy
- finality fee
- attestation fee
- dormant-account burn
- treasury fee
- governance reward switch
- premine or founder allocation

This keeps the halal/anti-riba direction clean: refiners earn for explicit
service, not for the mere passage of time.
