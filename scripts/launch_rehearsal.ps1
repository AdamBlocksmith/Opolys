param(
    [string]$RunRoot = "launch-rehearsal-local",
    [int]$Port = 48170,
    [int]$RpcPort = 48171,
    [int]$RestartPort = 48172,
    [int]$RestartRpcPort = 48173,
    [string]$ApiKey = "rehearsal-local-api-key-2026",
    [int]$MineTimeoutSeconds = 2400,
    [int]$RestartTimeoutSeconds = 90
)

$ErrorActionPreference = "Stop"

function Invoke-Rpc {
    param(
        [string]$RpcUrl,
        [string]$Method,
        [object]$Params = $null,
        [int]$Id = 1
    )

    $body = @{
        jsonrpc = "2.0"
        method = $Method
        params = $Params
        id = $Id
    } | ConvertTo-Json -Depth 10 -Compress

    $response = Invoke-RestMethod -Uri "$RpcUrl/rpc" -Method Post -ContentType "application/json" -Body $body
    if ($null -ne $response.error) {
        throw "RPC $Method failed: $($response.error.message)"
    }
    $response
}

function Stop-RehearsalProcess {
    param([System.Diagnostics.Process]$Process)

    if ($null -ne $Process -and -not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force
        $Process.WaitForExit()
    }
}

function Convert-FlakesToOplString {
    param([uint64]$Flakes)

    $Whole = [math]::Floor($Flakes / 1000000)
    $Fraction = $Flakes % 1000000
    "$Whole.$($Fraction.ToString('D6'))"
}

function Test-EconomicBooks {
    param(
        [string]$RpcUrl,
        [object]$MintLedger,
        [string]$OutputPath
    )

    $Supply = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getSupply" -Id 31
    $Ledger = $MintLedger.result
    [uint64]$Height = [uint64]$Ledger.height

    if ([uint64]$Supply.result.total_issued_flakes -ne [uint64]$Ledger.total_issued_flakes) {
        throw "Economic invariant failed: supply total_issued disagrees with Mint Ledger"
    }
    if ([uint64]$Supply.result.total_burned_flakes -ne [uint64]$Ledger.total_burned_flakes) {
        throw "Economic invariant failed: supply total_burned disagrees with Mint Ledger"
    }
    if ([uint64]$Supply.result.circulating_supply_flakes -ne [uint64]$Ledger.circulating_supply_flakes) {
        throw "Economic invariant failed: supply circulating disagrees with Mint Ledger"
    }

    [uint64]$ExpectedCirculating = [uint64]$Ledger.total_issued_flakes - [uint64]$Ledger.total_burned_flakes
    if ([uint64]$Ledger.circulating_supply_flakes -ne $ExpectedCirculating) {
        throw "Economic invariant failed: circulating supply is not issued minus burned"
    }

    [uint64]$LedgerIssued = [uint64]$Ledger.total_genesis_issued_flakes + [uint64]$Ledger.total_mined_gross_reward_flakes
    if ([uint64]$Ledger.total_issued_flakes -ne $LedgerIssued) {
        throw "Economic invariant failed: total issued is not genesis issued plus mined gross reward"
    }

    [uint64]$LedgerBurned = [uint64]$Ledger.total_mine_assay_burned_flakes +
        [uint64]$Ledger.total_ordinary_fees_burned_flakes +
        [uint64]$Ledger.total_bond_unbond_assay_burned_flakes +
        [uint64]$Ledger.total_slashed_stake_burned_flakes
    if ([uint64]$Ledger.total_burned_flakes -ne $LedgerBurned) {
        throw "Economic invariant failed: total burned is not the sum of burn categories"
    }

    [uint64]$ReceiptGross = 0
    [uint64]$ReceiptMineAssay = 0
    [uint64]$ReceiptFeesBurned = 0
    [uint64]$ReceiptBondAssay = 0
    [uint64]$ReceiptSlashed = 0
    [uint64]$ReceiptRefinerIncome = 0
    [uint64]$ReceiptMinedBlocks = 0
    [uint64]$ReceiptRefinedBlocks = 0
    [uint64]$ReceiptSuccessfulTransactions = 0

    for ([uint64]$H = 1; $H -le $Height; $H++) {
        $Receipt = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getBlockAssayCertificate" -Params @($H) -Id 32
        $R = $Receipt.result
        $ReceiptGross += [uint64]$R.gross_reward_flakes
        $ReceiptMineAssay += [uint64]$R.mine_assay_burned_flakes
        $ReceiptFeesBurned += [uint64]$R.ordinary_fees_burned_flakes
        $ReceiptBondAssay += [uint64]$R.bond_unbond_assay_burned_flakes
        $ReceiptSlashed += [uint64]$R.slashed_burned_flakes
        $ReceiptRefinerIncome += [uint64]$R.refiner_fee_income_flakes
        $ReceiptSuccessfulTransactions += [uint64]$R.successful_transaction_count
        if ($R.production_kind -eq "mined") {
            $ReceiptMinedBlocks += 1
        }
        if ($R.production_kind -eq "refined") {
            $ReceiptRefinedBlocks += 1
        }
    }

    if ($ReceiptGross -ne [uint64]$Ledger.total_mined_gross_reward_flakes) {
        throw "Economic invariant failed: receipt gross reward sum disagrees with Mint Ledger"
    }
    if ($ReceiptMineAssay -ne [uint64]$Ledger.total_mine_assay_burned_flakes) {
        throw "Economic invariant failed: receipt mine assay sum disagrees with Mint Ledger"
    }
    if ($ReceiptFeesBurned -ne [uint64]$Ledger.total_ordinary_fees_burned_flakes) {
        throw "Economic invariant failed: receipt burned fee sum disagrees with Mint Ledger"
    }
    if ($ReceiptBondAssay -ne [uint64]$Ledger.total_bond_unbond_assay_burned_flakes) {
        throw "Economic invariant failed: receipt bond/unbond assay sum disagrees with Mint Ledger"
    }
    if ($ReceiptSlashed -ne [uint64]$Ledger.total_slashed_stake_burned_flakes) {
        throw "Economic invariant failed: receipt slashed burn sum disagrees with Mint Ledger"
    }
    if ($ReceiptRefinerIncome -ne [uint64]$Ledger.total_refiner_fee_income_flakes) {
        throw "Economic invariant failed: receipt refiner income sum disagrees with Mint Ledger"
    }
    if ($ReceiptMinedBlocks -ne [uint64]$Ledger.total_mined_blocks) {
        throw "Economic invariant failed: receipt mined block count disagrees with Mint Ledger"
    }
    if ($ReceiptRefinedBlocks -ne [uint64]$Ledger.total_refined_blocks) {
        throw "Economic invariant failed: receipt refined block count disagrees with Mint Ledger"
    }
    if ($ReceiptSuccessfulTransactions -ne [uint64]$Ledger.total_successful_transactions) {
        throw "Economic invariant failed: receipt successful transaction count disagrees with Mint Ledger"
    }

    [pscustomobject]@{
        height = $Height
        total_issued_flakes = [uint64]$Ledger.total_issued_flakes
        total_burned_flakes = [uint64]$Ledger.total_burned_flakes
        circulating_supply_flakes = [uint64]$Ledger.circulating_supply_flakes
        receipt_mined_blocks = $ReceiptMinedBlocks
        receipt_refined_blocks = $ReceiptRefinedBlocks
        receipt_successful_transactions = $ReceiptSuccessfulTransactions
        result = "PASS"
    } | ConvertTo-Json -Depth 8 | Out-File $OutputPath
}

function Write-LaunchReport {
    param(
        [string]$OutputPath,
        [object]$Info,
        [object]$Balance,
        [object]$MintLedger,
        [object]$EconomicInvariants,
        [object]$TipAssay,
        [object]$Hallmark,
        [string]$MinerAddress,
        [string]$RecipientAddress,
        [string]$ArtifactDir
    )

    $Ledger = $MintLedger.result
    $Assay = $TipAssay.result
    $Refiner = $Hallmark.result
    $Invariant = $EconomicInvariants
    $GeneratedAt = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    $TipBlockHash = if ($null -ne $Assay.block_hash) { $Assay.block_hash } else { "" }
    $RecentRefinedBlocks = if ($null -ne $Refiner.recent_refined_blocks_produced) { $Refiner.recent_refined_blocks_produced } else { 0 }
    $RecentAttestations = if ($null -ne $Refiner.recent_attestations_included) { $Refiner.recent_attestations_included } else { 0 }

    $Lines = @(
        "# Opolys Launch Rehearsal Report",
        "",
        "Generated: $GeneratedAt",
        "Artifact directory: $ArtifactDir",
        "",
        "## Result",
        "",
        "- Rehearsal: PASS",
        "- Economic books: $($Invariant.result)",
        "- Restart persistence: PASS",
        "",
        "## Chain",
        "",
        "- Height: $($Info.result.height)",
        "- Difficulty: $($Info.result.difficulty)",
        "- Finalized height: $($Info.result.finalized_height)",
        "- Tip block hash: $TipBlockHash",
        "- Miner/refiner address: $MinerAddress",
        "- Recipient address: $RecipientAddress",
        "- Recipient balance after restart: $($Balance.result.balance_opl)",
        "",
        "## Mint Ledger",
        "",
        "- Total issued: $($Ledger.total_issued_opl)",
        "- Total burned: $($Ledger.total_burned_opl)",
        "- Circulating supply: $($Ledger.circulating_supply_opl)",
        "- Mined gross reward: $($Ledger.total_mined_gross_reward_opl)",
        "- Mine assay burned: $($Ledger.total_mine_assay_burned_opl)",
        "- Ordinary fees burned: $($Ledger.total_ordinary_fees_burned_opl)",
        "- Bond/unbond assay burned: $($Ledger.total_bond_unbond_assay_burned_opl)",
        "- Slashed stake burned: $($Ledger.total_slashed_stake_burned_opl)",
        "- Refiner fee income: $($Ledger.total_refiner_fee_income_opl)",
        "- Mined blocks: $($Ledger.total_mined_blocks)",
        "- Refined blocks: $($Ledger.total_refined_blocks)",
        "- Successful transactions: $($Ledger.total_successful_transactions)",
        "",
        "## Tip Assay Certificate",
        "",
        "- Height: $($Assay.height)",
        "- Production kind: $($Assay.production_kind)",
        "- Gross reward: $($Assay.gross_reward_opl)",
        "- Mine assay burned: $($Assay.mine_assay_burned_opl)",
        "- Miner credit: $($Assay.miner_credit_opl)",
        "- Ordinary fees burned: $($Assay.ordinary_fees_burned_opl)",
        "- Refiner fee income: $($Assay.refiner_fee_income_opl)",
        "- Bond/unbond assay burned: $($Assay.bond_unbond_assay_burned_opl)",
        "",
        "## Refiner",
        "",
        "- Status: $($Refiner.status)",
        "- Total stake: $($Refiner.total_stake_opl)",
        "- Total weight: $($Refiner.total_weight_flakes) flakes",
        "- Recent refined blocks: $RecentRefinedBlocks",
        "- Recent attestations included: $RecentAttestations",
        "",
        "## Economic Invariants",
        "",
        "- Supply equals Mint Ledger totals: PASS",
        "- Circulating supply equals issued minus burned: PASS",
        "- Burn categories sum to total burned: PASS",
        "- Block assay receipts sum to Mint Ledger: PASS",
        "- Receipt mined blocks: $($Invariant.receipt_mined_blocks)",
        "- Receipt refined blocks: $($Invariant.receipt_refined_blocks)",
        "- Receipt successful transactions: $($Invariant.receipt_successful_transactions)"
    )

    $Lines | Out-File $OutputPath -Encoding utf8
}

$Root = (Resolve-Path ".").Path
$RunRootPath = Join-Path $Root $RunRoot
$GenesisDir = Join-Path $RunRootPath "genesis-dry-run"
$DataDir = Join-Path $RunRootPath "node-data"
$KeyPath = Join-Path $RunRootPath "miner.key"
$MiningLog = Join-Path $RunRootPath "node-mining.log"
$MiningErr = Join-Path $RunRootPath "node-mining.err.log"
$RestartLog = Join-Path $RunRootPath "node-restart.log"
$RestartErr = Join-Path $RunRootPath "node-restart.err.log"
$RpcUrl = "http://127.0.0.1:$RpcPort"
$RestartRpcUrl = "http://127.0.0.1:$RestartRpcPort"
$Mnemonic = (("abandon " * 23) + "art").Trim()

if (Test-Path $RunRootPath) {
    Remove-Item -LiteralPath $RunRootPath -Recurse -Force
}
New-Item -ItemType Directory -Path $RunRootPath | Out-Null

Write-Host "STEP 1: dry-run genesis ceremony"
cargo run -p genesis-ceremony -- --dry-run --output-dir $GenesisDir |
    Tee-Object -FilePath (Join-Path $RunRootPath "ceremony.out")

Write-Host "STEP 2: verify genesis ceremony"
cargo run -p genesis-ceremony -- verify --attestation (Join-Path $GenesisDir "genesis_attestation.json") |
    Tee-Object -FilePath (Join-Path $RunRootPath "verify.out")

Write-Host "STEP 3: derive miner key and recipient address"
$env:OPOLYS_MNEMONIC = $Mnemonic
$MinerAddress = (cargo run -p opolys-wallet -- export-key-file --from-env $KeyPath | Select-Object -Last 1).Trim()
$RecipientAddress = (cargo run -p opolys-wallet -- address --from-env --account 1 | Select-Object -Last 1).Trim()
Write-Host "MINER=$MinerAddress"
Write-Host "RECIPIENT=$RecipientAddress"

Write-Host "STEP 4: start mining node with RPC"
$MiningArgs = @(
    "run", "-p", "opolys-node", "--",
    "--genesis-params", (Join-Path $GenesisDir "genesis_attestation.json"),
    "--data-dir", $DataDir,
    "--port", "$Port",
    "--rpc-port", "$RpcPort",
    "--rpc-listen-addr", "127.0.0.1",
    "--rpc-api-key", $ApiKey,
    "--allow-dry-run-genesis",
    "--no-bootstrap",
    "--mine",
    "--allow-solo-mining",
    "--refine",
    "--key-file", $KeyPath,
    "--log-level", "info"
)
$MiningProc = Start-Process -FilePath "cargo" -ArgumentList $MiningArgs -WorkingDirectory $Root -RedirectStandardOutput $MiningLog -RedirectStandardError $MiningErr -PassThru -WindowStyle Hidden

try {
    $Deadline = (Get-Date).AddSeconds($MineTimeoutSeconds)
    $Height = -1
    do {
        Start-Sleep -Seconds 2
        if ($MiningProc.HasExited) {
            throw "Mining node exited early with code $($MiningProc.ExitCode). See $MiningErr"
        }
        try {
            $Info = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getChainInfo" -Id 1
            $Height = [int]$Info.result.height
            Write-Host "height=$Height"
        } catch {
            # RPC may not be ready yet.
        }
    } while ($Height -lt 1 -and (Get-Date) -lt $Deadline)

    if ($Height -lt 1) {
        throw "Node did not mine block 1 within $MineTimeoutSeconds seconds"
    }

    Write-Host "STEP 5: query block 1"
    $Block1 = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getBlockByHeight" -Params @(1) -Id 2
    $Block1.result.header | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "block1.header.json")
    $Block1Assay = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getBlockAssayCertificate" -Params @(1) -Id 20
    if ([uint64]$Block1Assay.result.height -ne 1) {
        throw "Block 1 assay certificate returned wrong height"
    }
    $WalletBlock1AssayJson = (cargo run -p opolys-wallet -- --rpc-url $RpcUrl assay 1 | Out-String).Trim()
    $WalletBlock1Assay = $WalletBlock1AssayJson | ConvertFrom-Json
    if ($WalletBlock1Assay.block_hash -ne $Block1Assay.result.block_hash) {
        throw "Wallet assay command disagrees with block 1 assay RPC"
    }
    $Block1Assay.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "block1.assay.json")
    $WalletBlock1Assay | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "wallet.block1.assay.json")

    Write-Host "STEP 6: create and send wallet transaction"
    $TxHex = (cargo run -p opolys-wallet -- --rpc-url $RpcUrl transfer --from-env $RecipientAddress 1 --fee 0.000001 | Select-Object -Last 1).Trim()
    cargo run -p opolys-wallet -- --rpc-url $RpcUrl --rpc-api-key $ApiKey send $TxHex |
        Tee-Object -FilePath (Join-Path $RunRootPath "send.out")

    Write-Host "STEP 7: wait for transaction inclusion"
    $Deadline = (Get-Date).AddSeconds($MineTimeoutSeconds)
    [uint64]$RecipientBalance = 0
    do {
        Start-Sleep -Seconds 2
        if ($MiningProc.HasExited) {
            throw "Mining node exited early with code $($MiningProc.ExitCode). See $MiningErr"
        }
        try {
            $Balance = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getBalance" -Params @($RecipientAddress) -Id 3
            if ($null -ne $Balance.result.balance_flakes) {
                $RecipientBalance = [uint64]$Balance.result.balance_flakes
            }
            $Info = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getChainInfo" -Id 4
            Write-Host "height=$($Info.result.height) recipient_flakes=$RecipientBalance"
        } catch {
            # Keep polling until the timeout.
        }
    } while ($RecipientBalance -lt 1000000 -and (Get-Date) -lt $Deadline)

    if ($RecipientBalance -lt 1000000) {
        throw "Recipient balance did not show the wallet transaction within $MineTimeoutSeconds seconds"
    }

    Write-Host "STEP 8: bond local refiner"
    $BondInfo = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getChainInfo" -Id 26
    $WalletBondMinimumJson = (cargo run -p opolys-wallet -- --rpc-url $RpcUrl bond-minimum | Out-String).Trim()
    $WalletBondMinimum = $WalletBondMinimumJson | ConvertFrom-Json
    $RefinerBondMinFlakes = [uint64]$BondInfo.result.minimum_refiner_bond_flakes
    if ([uint64]$WalletBondMinimum.minimum_refiner_bond_flakes -ne $RefinerBondMinFlakes) {
        throw "Wallet bond-minimum disagrees with opl_getChainInfo"
    }
    $WalletBondMinimum | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "wallet.bond-minimum.json")
    $RefinerBondFlakes = $RefinerBondMinFlakes + 10000000
    $RefinerBondOpl = Convert-FlakesToOplString -Flakes $RefinerBondFlakes
    Write-Host "minimum_refiner_bond=$($BondInfo.result.minimum_refiner_bond_opl); rehearsal_bond=$RefinerBondOpl OPL"
    $BondTxHex = (cargo run -p opolys-wallet -- --rpc-url $RpcUrl bond --from-env $RefinerBondOpl --fee 0.000001 | Select-Object -Last 1).Trim()
    cargo run -p opolys-wallet -- --rpc-url $RpcUrl --rpc-api-key $ApiKey send $BondTxHex |
        Tee-Object -FilePath (Join-Path $RunRootPath "bond-send.out")

    Write-Host "STEP 9: wait for refiner bond inclusion"
    $Deadline = (Get-Date).AddSeconds($MineTimeoutSeconds)
    $HallmarkBefore = $null
    do {
        Start-Sleep -Seconds 2
        if ($MiningProc.HasExited) {
            throw "Mining node exited early with code $($MiningProc.ExitCode). See $MiningErr"
        }
        try {
            $HallmarkBefore = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getRefinerHallmark" -Params @($MinerAddress) -Id 22
            $Info = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getChainInfo" -Id 23
            Write-Host "height=$($Info.result.height) refiner_status=$($HallmarkBefore.result.status) refiner_stake=$($HallmarkBefore.result.total_stake_flakes)"
        } catch {
            # Bond transaction may still be waiting for inclusion.
        }
    } while (($null -eq $HallmarkBefore -or [uint64]$HallmarkBefore.result.total_stake_flakes -lt $RefinerBondMinFlakes) -and (Get-Date) -lt $Deadline)

    if ($null -eq $HallmarkBefore) {
        throw "Refiner hallmark did not become available within $MineTimeoutSeconds seconds"
    }
    if ([uint64]$HallmarkBefore.result.total_stake_flakes -lt $RefinerBondMinFlakes) {
        throw "Refiner hallmark did not show the bonded stake"
    }
    if ($HallmarkBefore.result.status -ne "Bonding" -and $HallmarkBefore.result.status -ne "Waiting" -and $HallmarkBefore.result.status -ne "Active") {
        throw "Unexpected refiner status after bond: $($HallmarkBefore.result.status)"
    }

    $RefinersBefore = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getRefiners" -Id 24
    $WalletHallmarkBeforeJson = (cargo run -p opolys-wallet -- --rpc-url $RpcUrl refiner $MinerAddress | Out-String).Trim()
    $WalletHallmarkBefore = $WalletHallmarkBeforeJson | ConvertFrom-Json
    if ([uint64]$WalletHallmarkBefore.total_stake_flakes -lt $RefinerBondMinFlakes) {
        throw "Wallet refiner command did not show the bonded stake before restart"
    }
    if ($WalletHallmarkBefore.status -ne $HallmarkBefore.result.status) {
        throw "Wallet refiner status disagrees with Hallmark RPC before restart"
    }
    $HallmarkBefore.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "refiner.hallmark-before-restart.json")
    $WalletHallmarkBefore | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "wallet.refiner-before-restart.json")
    $RefinersBefore.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "refiners-before-restart.json")

    $InfoBefore = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getChainInfo" -Id 5
    $MintLedgerBefore = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getMintLedger" -Id 27
    $WalletMintLedgerBeforeJson = (cargo run -p opolys-wallet -- --rpc-url $RpcUrl ledger | Out-String).Trim()
    $WalletMintLedgerBefore = $WalletMintLedgerBeforeJson | ConvertFrom-Json
    if ([uint64]$WalletMintLedgerBefore.total_issued_flakes -ne [uint64]$MintLedgerBefore.result.total_issued_flakes) {
        throw "Wallet ledger command disagrees with Mint Ledger RPC before restart"
    }
    Test-EconomicBooks -RpcUrl $RpcUrl -MintLedger $MintLedgerBefore -OutputPath (Join-Path $RunRootPath "economic-invariants-before-restart.json")
    $InfoBefore.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "chain-before-restart.json")
    $MintLedgerBefore.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "mint-ledger-before-restart.json")
    $WalletMintLedgerBefore | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "wallet.mint-ledger-before-restart.json")
    Write-Host "BEFORE_RESTART_HEIGHT=$($InfoBefore.result.height)"
}
finally {
    Stop-RehearsalProcess -Process $MiningProc
}

Write-Host "STEP 10: restart node against same data directory"
$RestartArgs = @(
    "run", "-p", "opolys-node", "--",
    "--genesis-params", (Join-Path $GenesisDir "genesis_attestation.json"),
    "--data-dir", $DataDir,
    "--port", "$RestartPort",
    "--rpc-port", "$RestartRpcPort",
    "--rpc-listen-addr", "127.0.0.1",
    "--rpc-api-key", $ApiKey,
    "--allow-dry-run-genesis",
    "--no-bootstrap",
    "--log-level", "info"
)
$RestartProc = Start-Process -FilePath "cargo" -ArgumentList $RestartArgs -WorkingDirectory $Root -RedirectStandardOutput $RestartLog -RedirectStandardError $RestartErr -PassThru -WindowStyle Hidden

try {
    $Deadline = (Get-Date).AddSeconds($RestartTimeoutSeconds)
    $InfoAfter = $null
    do {
        Start-Sleep -Seconds 2
        if ($RestartProc.HasExited) {
            throw "Restarted node exited early with code $($RestartProc.ExitCode). See $RestartErr"
        }
        try {
            $InfoAfter = Invoke-Rpc -RpcUrl $RestartRpcUrl -Method "opl_getChainInfo" -Id 6
        } catch {
            # RPC may not be ready yet.
        }
    } while ($null -eq $InfoAfter -and (Get-Date) -lt $Deadline)

    if ($null -eq $InfoAfter) {
        throw "Restarted node RPC did not become available within $RestartTimeoutSeconds seconds"
    }

    $BalanceAfter = Invoke-Rpc -RpcUrl $RestartRpcUrl -Method "opl_getBalance" -Params @($RecipientAddress) -Id 7
    $TipAssayAfter = Invoke-Rpc -RpcUrl $RestartRpcUrl -Method "opl_getBlockAssayCertificate" -Params @([uint64]$InfoAfter.result.height) -Id 21
    $WalletTipAssayAfterJson = (cargo run -p opolys-wallet -- --rpc-url $RestartRpcUrl assay "$($InfoAfter.result.height)" | Out-String).Trim()
    $WalletTipAssayAfter = $WalletTipAssayAfterJson | ConvertFrom-Json
    $MintLedgerAfter = Invoke-Rpc -RpcUrl $RestartRpcUrl -Method "opl_getMintLedger" -Id 28
    $WalletMintLedgerAfterJson = (cargo run -p opolys-wallet -- --rpc-url $RestartRpcUrl ledger | Out-String).Trim()
    $WalletMintLedgerAfter = $WalletMintLedgerAfterJson | ConvertFrom-Json
    $HallmarkAfter = Invoke-Rpc -RpcUrl $RestartRpcUrl -Method "opl_getRefinerHallmark" -Params @($MinerAddress) -Id 25
    $WalletHallmarkAfterJson = (cargo run -p opolys-wallet -- --rpc-url $RestartRpcUrl refiner $MinerAddress | Out-String).Trim()
    $WalletHallmarkAfter = $WalletHallmarkAfterJson | ConvertFrom-Json
    if ([uint64]$TipAssayAfter.result.height -ne [uint64]$InfoAfter.result.height) {
        throw "Tip assay certificate returned wrong height after restart"
    }
    if ($WalletTipAssayAfter.block_hash -ne $TipAssayAfter.result.block_hash) {
        throw "Wallet assay command disagrees with tip assay RPC after restart"
    }
    if ([uint64]$WalletMintLedgerAfter.total_issued_flakes -ne [uint64]$MintLedgerAfter.result.total_issued_flakes) {
        throw "Wallet ledger command disagrees with Mint Ledger RPC after restart"
    }
    $EconomicInvariantsAfterPath = Join-Path $RunRootPath "economic-invariants-after-restart.json"
    Test-EconomicBooks -RpcUrl $RestartRpcUrl -MintLedger $MintLedgerAfter -OutputPath $EconomicInvariantsAfterPath
    $EconomicInvariantsAfter = Get-Content $EconomicInvariantsAfterPath | ConvertFrom-Json
    $InfoAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "chain-after-restart.json")
    $BalanceAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "recipient-after-restart.json")
    $TipAssayAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "tip.assay-after-restart.json")
    $WalletTipAssayAfter | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "wallet.tip.assay-after-restart.json")
    $MintLedgerAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "mint-ledger-after-restart.json")
    $WalletMintLedgerAfter | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "wallet.mint-ledger-after-restart.json")
    $HallmarkAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "refiner.hallmark-after-restart.json")
    $WalletHallmarkAfter | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "wallet.refiner-after-restart.json")
    Write-Host "AFTER_RESTART_HEIGHT=$($InfoAfter.result.height)"
    Write-Host "AFTER_RESTART_RECIPIENT_FLAKES=$($BalanceAfter.result.balance_flakes)"
    Write-Host "AFTER_RESTART_REFINER_STATUS=$($HallmarkAfter.result.status)"
    Write-Host "AFTER_RESTART_REFINER_STAKE=$($HallmarkAfter.result.total_stake_flakes)"

    if ([uint64]$BalanceAfter.result.balance_flakes -lt 1000000) {
        throw "Recipient balance missing after restart"
    }
    if ([uint64]$HallmarkAfter.result.total_stake_flakes -lt $RefinerBondMinFlakes) {
        throw "Refiner bond missing after restart"
    }
    if ([uint64]$WalletHallmarkAfter.total_stake_flakes -lt $RefinerBondMinFlakes) {
        throw "Wallet refiner command did not show the bonded stake after restart"
    }
    if ($WalletHallmarkAfter.status -ne $HallmarkAfter.result.status) {
        throw "Wallet refiner status disagrees with Hallmark RPC after restart"
    }

    Write-LaunchReport `
        -OutputPath (Join-Path $RunRootPath "launch-report.md") `
        -Info $InfoAfter `
        -Balance $BalanceAfter `
        -MintLedger $MintLedgerAfter `
        -EconomicInvariants $EconomicInvariantsAfter `
        -TipAssay $TipAssayAfter `
        -Hallmark $HallmarkAfter `
        -MinerAddress $MinerAddress `
        -RecipientAddress $RecipientAddress `
        -ArtifactDir $RunRootPath
}
finally {
    Stop-RehearsalProcess -Process $RestartProc
}

Write-Host "REHEARSAL PASS"
Write-Host "ARTIFACT_DIR=$RunRootPath"
