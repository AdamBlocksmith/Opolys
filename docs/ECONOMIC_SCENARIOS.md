# Opolys Economic Scenarios

This note models whether Opolys has enough deflationary pressure under several
network conditions. It is a planning document, not a consensus rule.

## Assumptions

The model uses the current documented economics:

```text
BASE_REWARD = 332 OPL
BLOCKS_PER_YEAR = 350,640
EPOCH = 960
average_vein_yield = 1 + sqrt(pi) / 2 = about 1.8862269x
mine_assay = gross_reward * sqrt(effective_difficulty) / EPOCH
```

The same model can be reproduced with:

```text
python scripts/economics_model.py
```

The average vein-yield estimate comes from the distribution of valid PoW
hashes. If `U` is uniformly distributed over `(0, 1]`, then
`-ln(U)` is exponentially distributed and:

```text
E[sqrt(-ln(U))] = sqrt(pi) / 2
average_vein_yield = 1 + sqrt(pi) / 2
```

That makes the average mined block:

```text
gross_reward_per_block = BASE_REWARD / difficulty * average_vein_yield
mine_assay_per_block = gross_reward_per_block * sqrt(difficulty) / EPOCH
miner_credit_per_block = gross_reward_per_block - mine_assay_per_block
```

Only mined blocks create new OPL. Refined blocks create no new OPL.

Ordinary fee routing:

```text
mined block: ordinary fees burn
refined block: ordinary fees go to the selected refiner
```

## Scenario Results

| Scenario | Difficulty | Mined Share | Tx / Block | Fee / Tx | Gross Issued / Year | Burned / Year | Circulating Change / Year |
|---|---:|---:|---:|---:|---:|---:|---:|
| Launch quiet, mostly mined | 7 | 100% | 5 | 0.0001 OPL | 31,368,622 OPL | 86,627 OPL | +31,281,995 OPL |
| Launch active, mostly mined | 7 | 100% | 100 | 0.001 OPL | 31,368,622 OPL | 121,516 OPL | +31,247,106 OPL |
| Mature busy PoW | 100 | 100% | 1,000 | 0.001 OPL | 2,195,804 OPL | 373,513 OPL | +1,822,291 OPL |
| High difficulty busy PoW | 1,000 | 100% | 1,000 | 0.001 OPL | 219,580 OPL | 357,873 OPL | -138,293 OPL |
| Miner stress, POR carries half | 1,000 | 50% | 1,000 | 0.001 OPL | 109,790 OPL | 178,937 OPL | -69,146 OPL |
| Mostly POR, high activity | 1,000 | 10% | 1,000 | 0.001 OPL | 21,958 OPL | 35,787 OPL | -13,829 OPL |

## Break-Even Fee Pressure

This table asks: how much ordinary fee pressure is needed in mined blocks to
offset the miner's net new issuance?

| Difficulty | Net Miner Issuance / Block | Break-Even Fee at 100 Tx | Break-Even Fee at 1,000 Tx |
|---:|---:|---:|---:|
| 7 | 89.214495 OPL | 0.892145 OPL | 0.089214 OPL |
| 25 | 24.918630 OPL | 0.249186 OPL | 0.024919 OPL |
| 100 | 6.197041 OPL | 0.061970 OPL | 0.006197 OPL |
| 500 | 1.223282 OPL | 0.012233 OPL | 0.001223 OPL |
| 1,000 | 0.605599 OPL | 0.006056 OPL | 0.000606 OPL |
| 5,000 | 0.116020 OPL | 0.001160 OPL | 0.000116 OPL |
| 10,000 | 0.056100 OPL | 0.000561 OPL | 0.000056 OPL |

## Mine Assay Curve

The mine assay is:

```text
mine_assay = gross_reward * sqrt(difficulty) / EPOCH
```

Because gross reward falls as `1 / difficulty`, while the assay fraction rises
as `sqrt(difficulty) / EPOCH`, the assay gets stronger as a percentage of each
block but smaller in absolute OPL at high difficulty.

| Difficulty | Gross / Block | Mine Assay / Block | Assay % | Miner Net / Block | Assay / Year |
|---:|---:|---:|---:|---:|---:|
| 7 | 89.461048 OPL | 0.246554 OPL | 0.276% | 89.214495 OPL | 86,452 OPL |
| 25 | 25.049094 OPL | 0.130464 OPL | 0.521% | 24.918630 OPL | 45,746 OPL |
| 100 | 6.262273 OPL | 0.065232 OPL | 1.042% | 6.197041 OPL | 22,873 OPL |
| 500 | 1.252455 OPL | 0.029173 OPL | 2.329% | 1.223282 OPL | 10,229 OPL |
| 1,000 | 0.626227 OPL | 0.020628 OPL | 3.294% | 0.605599 OPL | 7,233 OPL |
| 5,000 | 0.125245 OPL | 0.009225 OPL | 7.366% | 0.116020 OPL | 3,235 OPL |
| 10,000 | 0.062623 OPL | 0.006523 OPL | 10.417% | 0.056100 OPL | 2,287 OPL |
| 50,000 | 0.012525 OPL | 0.002917 OPL | 23.292% | 0.009607 OPL | 1,023 OPL |

Reading:

- At launch difficulty, mine assay is intentionally light.
- At high difficulty, the miner gives up a larger percentage of each block.
- Net issuance still falls mostly because the gross reward shrinks with
  difficulty.
- High-difficulty deflation comes from low issuance plus fee burns, not mine
  assay alone.

## Reading

At launch difficulty, Opolys is inflationary unless fees are very high. That is
expected: new digital gold is still easy to mine.

As difficulty rises, mined issuance falls quickly:

```text
gross_reward_per_block = BASE_REWARD / difficulty * average_vein_yield
```

Mine assay rises as a fraction of gross reward:

```text
mine_assay_fraction = sqrt(difficulty) / EPOCH
```

POR also adds deflationary pressure indirectly because refined blocks do not
create new OPL. If miners are absent and refiners move activity, supply growth
pauses while user fees pay refiners.

## Conclusion

Opolys has meaningful deflationary pressure, but it is not constant. It is
weakest at launch and strongest when one or more of these are true:

- mining difficulty is high
- transaction activity is high
- a larger share of blocks are refined instead of mined
- bonding/unbonding churn adds assay burns
- slashing removes bad collateral

The current model is therefore activity-sensitive rather than holder-punitive:
it does not burn dormant accounts or charge passive vault rent, but it can move
toward net deflation as the network matures.

## Fee Response

The suggested fee is an exponential moving average:

```text
suggested_fee = max(MIN_FEE, (observed_average_fee + 9 * previous_suggested_fee) / 10)
```

Admission fees scale with the actual pending queue depth:

```text
pending_blocks = ceil(mempool_bytes / MAX_BLOCK_SIZE_BYTES)
fee_multiplier = clamp(pending_blocks, 1, CAPACITY_RATIO)
effective_min_fee = suggested_fee * fee_multiplier
CAPACITY_RATIO = ceil(MEMPOOL_MAX_SIZE_BYTES / MAX_BLOCK_SIZE_BYTES)
```

Example response over 20 blocks:

| Observed Average Fee | After 1 Block | After 5 Blocks | After 10 Blocks | After 20 Blocks | Full-Queue Minimum After 20 |
|---|---:|---:|---:|---:|---:|
| 1 -> 10,000 flakes | 1,000 | 4,095 | 6,511 | 8,781 | 87,810 |
| 1,000 -> 10,000 flakes | 1,900 | 4,685 | 6,859 | 8,902 | 89,020 |
| 10,000 -> 1 flake | 9,000 | 5,905 | 3,484 | 1,213 | 12,130 |

Reading:

- Suggested fee moves gradually, so one strange block does not permanently
  distort the market quote.
- When demand stays high, suggested fee converges toward the observed average.
- When demand collapses, suggested fee decays back toward `MIN_FEE`.
- The admission floor rises gradually with actual queue depth instead of
  jumping straight to the maximum multiplier.

Example with `suggested_fee = 1,000 flakes`:

| Pending Blocks | Multiplier | Effective Minimum |
|---:|---:|---:|
| 0 | 1x | 1,000 flakes |
| 0.5 | 1x | 1,000 flakes |
| 1.0 | 1x | 1,000 flakes |
| 1.1 | 2x | 2,000 flakes |
| 2.4 | 3x | 3,000 flakes |
| 5.0 | 5x | 5,000 flakes |
| 9.5 | 10x | 10,000 flakes |
