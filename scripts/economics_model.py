#!/usr/bin/env python3
"""Deterministic Opolys economics planning model.

This script mirrors the documented economics formulas in docs/ECONOMICS.md.
It is not consensus code; it is a planning aid for comparing supply pressure
under different difficulty, fee, and POR-share assumptions.
"""

from __future__ import annotations

import math


BASE_REWARD_OPL = 332.0
BLOCKS_PER_YEAR = 350_640
EPOCH = 960
AVERAGE_VEIN_YIELD = 1.0 + math.sqrt(math.pi) / 2.0
CAPACITY_RATIO = 10
MIN_FEE_FLAKES = 1


def mined_block_row(difficulty: float) -> dict[str, float]:
    gross = BASE_REWARD_OPL / difficulty * AVERAGE_VEIN_YIELD
    mine_assay = gross * math.sqrt(difficulty) / EPOCH
    net = gross - mine_assay
    return {
        "difficulty": difficulty,
        "gross": gross,
        "assay": mine_assay,
        "assay_pct": mine_assay / gross * 100.0 if gross else 0.0,
        "net": net,
        "gross_year": gross * BLOCKS_PER_YEAR,
        "assay_year": mine_assay * BLOCKS_PER_YEAR,
        "net_year": net * BLOCKS_PER_YEAR,
    }


def scenario(
    name: str,
    difficulty: float,
    mined_share: float,
    tx_per_block: float,
    fee_opl: float,
) -> dict[str, float | str]:
    mined = mined_block_row(difficulty)
    mined_blocks = BLOCKS_PER_YEAR * mined_share
    refined_blocks = BLOCKS_PER_YEAR - mined_blocks

    gross_issue = mined["gross"] * mined_blocks
    mine_assay = mined["assay"] * mined_blocks
    mined_fee_burn = mined_blocks * tx_per_block * fee_opl
    refiner_fee_income = refined_blocks * tx_per_block * fee_opl
    total_burn = mine_assay + mined_fee_burn

    return {
        "name": name,
        "difficulty": difficulty,
        "mined_share": mined_share,
        "tx_per_block": tx_per_block,
        "fee_opl": fee_opl,
        "gross_issue": gross_issue,
        "mine_assay": mine_assay,
        "mined_fee_burn": mined_fee_burn,
        "refiner_fee_income": refiner_fee_income,
        "total_burn": total_burn,
        "circulating_delta": gross_issue - total_burn,
    }


def suggested_fee_sequence(
    start_fee_flakes: int,
    observed_average_fee_flakes: int,
    blocks: int,
) -> list[int]:
    fees = []
    suggested = start_fee_flakes
    for _ in range(blocks):
        suggested = max(
            MIN_FEE_FLAKES,
            (observed_average_fee_flakes + (CAPACITY_RATIO - 1) * suggested) // CAPACITY_RATIO,
        )
        fees.append(suggested)
    return fees


def print_table(headers: list[str], rows: list[list[str]]) -> None:
    widths = [len(h) for h in headers]
    for row in rows:
        for idx, value in enumerate(row):
            widths[idx] = max(widths[idx], len(value))

    def fmt(row: list[str]) -> str:
        return " | ".join(value.rjust(widths[idx]) for idx, value in enumerate(row))

    print(fmt(headers))
    print("-+-".join("-" * width for width in widths))
    for row in rows:
        print(fmt(row))


def main() -> None:
    print("Mine assay curve")
    difficulties = [7, 25, 100, 500, 1_000, 5_000, 10_000, 50_000]
    rows = []
    for diff in difficulties:
        row = mined_block_row(diff)
        rows.append(
            [
                f"{diff:,.0f}",
                f"{row['gross']:,.6f}",
                f"{row['assay']:,.6f}",
                f"{row['assay_pct']:,.3f}%",
                f"{row['net']:,.6f}",
                f"{row['gross_year']:,.0f}",
                f"{row['assay_year']:,.0f}",
            ]
        )
    print_table(
        [
            "Difficulty",
            "Gross/block",
            "Assay/block",
            "Assay %",
            "Miner net/block",
            "Gross/year",
            "Assay/year",
        ],
        rows,
    )

    print("\nBreak-even ordinary fees for mined blocks")
    rows = []
    for diff in difficulties:
        net = mined_block_row(diff)["net"]
        rows.append(
            [
                f"{diff:,.0f}",
                f"{net:,.6f}",
                f"{net / 100.0:,.6f}",
                f"{net / 1_000.0:,.6f}",
                f"{net / 10_000.0:,.6f}",
            ]
        )
    print_table(
        [
            "Difficulty",
            "Net issuance/block",
            "Fee @100 tx",
            "Fee @1,000 tx",
            "Fee @10,000 tx",
        ],
        rows,
    )

    print("\nYearly scenarios")
    scenarios = [
        scenario("Launch quiet", 7, 1.0, 5, 0.0001),
        scenario("Launch active", 7, 1.0, 100, 0.001),
        scenario("Mature busy PoW", 100, 1.0, 1_000, 0.001),
        scenario("High difficulty busy PoW", 1_000, 1.0, 1_000, 0.001),
        scenario("Half POR / half mined", 1_000, 0.5, 1_000, 0.001),
        scenario("Mostly POR", 1_000, 0.1, 1_000, 0.001),
    ]
    rows = []
    for item in scenarios:
        rows.append(
            [
                str(item["name"]),
                f"{item['difficulty']:,.0f}",
                f"{item['mined_share'] * 100:,.0f}%",
                f"{item['gross_issue']:,.0f}",
                f"{item['total_burn']:,.0f}",
                f"{item['circulating_delta']:,.0f}",
                f"{item['refiner_fee_income']:,.0f}",
            ]
        )
    print_table(
        [
            "Scenario",
            "Difficulty",
            "Mined share",
            "Gross/year",
            "Burn/year",
            "Supply delta/year",
            "Refiner fee/year",
        ],
        rows,
    )

    print("\nSuggested fee response")
    fee_scenarios = [
        ("1 -> 10,000 flakes", 1, 10_000),
        ("1,000 -> 10,000 flakes", 1_000, 10_000),
        ("10,000 -> 1 flake", 10_000, 1),
    ]
    rows = []
    for name, start, observed in fee_scenarios:
        fees = suggested_fee_sequence(start, observed, 20)
        rows.append(
            [
                name,
                f"{fees[0]:,}",
                f"{fees[4]:,}",
                f"{fees[9]:,}",
                f"{fees[19]:,}",
                f"{fees[19] * CAPACITY_RATIO:,}",
            ]
        )
    print_table(
        [
            "Observed avg",
            "After 1 block",
            "After 5 blocks",
            "After 10 blocks",
            "After 20 blocks",
            "Rush min after 20",
        ],
        rows,
    )


if __name__ == "__main__":
    main()
