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
