param(
    [string]$RunRoot = "launch-rehearsal-local",
    [int]$Port = 48170,
    [int]$RpcPort = 48171,
    [int]$RestartPort = 48172,
    [int]$RestartRpcPort = 48173,
    [string]$ApiKey = "rehearsal-local-api-key-2026",
    [int]$MineTimeoutSeconds = 900,
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
    $Block1Assay.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "block1.assay.json")

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
    $RefinerBondMinFlakes = [uint64]$BondInfo.result.minimum_refiner_bond_flakes
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
    $HallmarkBefore.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "refiner.hallmark-before-restart.json")
    $RefinersBefore.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "refiners-before-restart.json")

    $InfoBefore = Invoke-Rpc -RpcUrl $RpcUrl -Method "opl_getChainInfo" -Id 5
    $InfoBefore.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "chain-before-restart.json")
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
    $HallmarkAfter = Invoke-Rpc -RpcUrl $RestartRpcUrl -Method "opl_getRefinerHallmark" -Params @($MinerAddress) -Id 25
    if ([uint64]$TipAssayAfter.result.height -ne [uint64]$InfoAfter.result.height) {
        throw "Tip assay certificate returned wrong height after restart"
    }
    $InfoAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "chain-after-restart.json")
    $BalanceAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "recipient-after-restart.json")
    $TipAssayAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "tip.assay-after-restart.json")
    $HallmarkAfter.result | ConvertTo-Json -Depth 8 | Out-File (Join-Path $RunRootPath "refiner.hallmark-after-restart.json")
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
}
finally {
    Stop-RehearsalProcess -Process $RestartProc
}

Write-Host "REHEARSAL PASS"
Write-Host "ARTIFACT_DIR=$RunRootPath"
