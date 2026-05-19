param(
    [string]$PackageDir,
    [string]$RunRoot = "release-smoke-local",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try {
        return $listener.LocalEndpoint.Port
    }
    finally {
        $listener.Stop()
    }
}

function Invoke-JsonRpc {
    param(
        [string]$RpcUrl,
        [string]$Method,
        [object[]]$Params = $null,
        [int]$Id = 1
    )

    $request = @{
        jsonrpc = "2.0"
        method = $Method
        params = $Params
        id = $Id
    }
    $body = $request | ConvertTo-Json -Depth 8
    $response = Invoke-RestMethod -Uri "$RpcUrl/rpc" -Method Post -ContentType "application/json" -Body $body
    if ($null -ne $response.error) {
        throw "RPC $Method failed: $($response.error | ConvertTo-Json -Compress)"
    }
    return $response
}

function Wait-ForRpc {
    param(
        [string]$RpcUrl,
        [int]$TimeoutSeconds = 30
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        try {
            $response = Invoke-WebRequest -Uri "$RpcUrl/health" -UseBasicParsing -TimeoutSec 2
            if ($response.StatusCode -eq 200) {
                return
            }
        }
        catch {
            Start-Sleep -Milliseconds 500
        }
    } while ((Get-Date) -lt $deadline)

    throw "Timed out waiting for RPC health at $RpcUrl"
}

function Wait-ForLogPattern {
    param(
        [string[]]$Paths,
        [string]$Pattern,
        [int]$TimeoutSeconds = 30
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $text = ""
        foreach ($path in $Paths) {
            if (Test-Path $path) {
                $text += "`n" + (Get-Content $path -Raw)
            }
        }
        if ($text -match $Pattern) {
            return $text
        }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)

    throw "Timed out waiting for log pattern '$Pattern'"
}

function Stop-SmokeProcess {
    param([System.Diagnostics.Process]$Process)

    if ($null -eq $Process -or $Process.HasExited) {
        return
    }

    $Process.Kill()
    $Process.WaitForExit()
}

function Remove-Ansi {
    param([string]$Text)
    return $Text -replace "$([char]27)\[[0-9;]*[A-Za-z]", ""
}

function Find-FirstMatch {
    param(
        [string]$Text,
        [string]$Pattern
    )
    if ($Text -match $Pattern) {
        return $Matches[1]
    }
    return ""
}

$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $Root

if (-not $PackageDir) {
    if (-not $SkipBuild) {
        & powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build_release.ps1 -SkipTests
        if ($LASTEXITCODE -ne 0) {
            throw "Release package build failed"
        }
    }
    $PackageDir = Get-ChildItem (Join-Path $Root "dist") -Directory |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}

if (-not $PackageDir -or -not (Test-Path $PackageDir)) {
    throw "Package directory not found. Build a package first or pass -PackageDir."
}

$PackageDir = (Resolve-Path $PackageDir).Path
$NodeBin = Join-Path $PackageDir "opolys-node.exe"
$WalletBin = Join-Path $PackageDir "opl.exe"
$CeremonyBin = Join-Path $PackageDir "genesis-ceremony.exe"

foreach ($Binary in @($NodeBin, $WalletBin, $CeremonyBin)) {
    if (-not (Test-Path $Binary)) {
        throw "Missing packaged binary: $Binary"
    }
}

$RunRootPath = Join-Path $Root $RunRoot
if (Test-Path $RunRootPath) {
    Remove-Item -Recurse -Force $RunRootPath
}
New-Item -ItemType Directory -Force -Path $RunRootPath | Out-Null

$GenesisDir = Join-Path $RunRootPath "genesis-dry-run"
$DataDir = Join-Path $RunRootPath "node-data"
$KeyFile = Join-Path $RunRootPath "producer.key"
$NodeOut = Join-Path $RunRootPath "node.stdout.log"
$NodeErr = Join-Path $RunRootPath "node.stderr.log"
$ReportPath = Join-Path $RunRootPath "release-smoke-report.md"
$Mnemonic = (("abandon " * 23) + "art").Trim()
$PreviousRustLog = $env:RUST_LOG
$env:OPOLYS_MNEMONIC = $Mnemonic
$env:RUST_LOG = "info"

try {
    Write-Host "STEP 1: packaged dry-run genesis ceremony"
    & $CeremonyBin --dry-run --output-dir $GenesisDir | Tee-Object -FilePath (Join-Path $RunRootPath "genesis-ceremony.stdout.log")
    if ($LASTEXITCODE -ne 0) {
        throw "Packaged genesis ceremony failed"
    }

    $Attestation = Join-Path $GenesisDir "genesis_attestation.json"
    Write-Host "STEP 2: packaged genesis verification"
    $VerifyText = & $CeremonyBin verify --attestation $Attestation
    $VerifyText | Tee-Object -FilePath (Join-Path $RunRootPath "genesis-verify.stdout.log")
    if ($LASTEXITCODE -ne 0 -or ($VerifyText -join "`n") -notmatch "RESULT: PASS") {
        throw "Packaged genesis verification did not pass"
    }

    Write-Host "STEP 3: packaged wallet key export and address"
    $ProducerAddress = (& $WalletBin address --from-env).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $ProducerAddress) {
        throw "Packaged wallet address command failed"
    }
    $ExportedAddress = (& $WalletBin export-key-file --from-env $KeyFile).Trim()
    if ($LASTEXITCODE -ne 0 -or $ExportedAddress -ne $ProducerAddress) {
        throw "Packaged wallet export-key-file command failed or disagreed with address"
    }

    Write-Host "STEP 4: packaged node startup with RPC"
    $P2pPort = Get-FreeTcpPort
    $RpcPort = Get-FreeTcpPort
    $RpcUrl = "http://127.0.0.1:$RpcPort"
    $NodeArgs = @(
        "--genesis-params", $Attestation,
        "--data-dir", $DataDir,
        "--port", "$P2pPort",
        "--rpc-port", "$RpcPort",
        "--rpc-api-key", "release-smoke-api-key",
        "--key-file", $KeyFile,
        "--allow-dry-run-genesis",
        "--no-bootstrap",
        "--log-level", "info"
    )
    $NodeProc = Start-Process -FilePath $NodeBin -ArgumentList $NodeArgs -NoNewWindow -RedirectStandardOutput $NodeOut -RedirectStandardError $NodeErr -PassThru
    Wait-ForRpc -RpcUrl $RpcUrl -TimeoutSeconds 45
    $NodeLogText = Wait-ForLogPattern -Paths @($NodeErr, $NodeOut) -Pattern "Launch configuration summary" -TimeoutSeconds 30

    Write-Host "STEP 5: packaged RPC and wallet read checks"
    $ChainInfo = Invoke-JsonRpc -RpcUrl $RpcUrl -Method "opl_getChainInfo"
    $Supply = Invoke-JsonRpc -RpcUrl $RpcUrl -Method "opl_getSupply"
    $Ledger = & $WalletBin --rpc-url $RpcUrl ledger | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
        throw "Packaged wallet ledger command failed"
    }
    $BondMinimum = & $WalletBin --rpc-url $RpcUrl bond-minimum | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
        throw "Packaged wallet bond-minimum command failed"
    }
    $Balance = & $WalletBin --rpc-url $RpcUrl balance $ProducerAddress | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
        throw "Packaged wallet balance command failed"
    }

    if ($NodeLogText -notmatch "Dangerous flag active: --allow-dry-run-genesis") {
        throw "Packaged node logs did not warn about --allow-dry-run-genesis"
    }
    $PlainNodeLog = Remove-Ansi $NodeLogText
    $GenesisHash = Find-FirstMatch -Text $PlainNodeLog -Pattern "genesis_hash=([0-9a-f]{64})"
    $LatestHash = Find-FirstMatch -Text $PlainNodeLog -Pattern "latest_hash=([0-9a-f]{64})"

    $Lines = @(
        "# Opolys Release Smoke Report",
        "",
        "Generated: $((Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ"))",
        "Package: $PackageDir",
        "",
        "## Result",
        "",
        "- Release smoke: PASS",
        "- Packaged genesis ceremony: PASS",
        "- Packaged genesis verification: PASS",
        "- Packaged node startup: PASS",
        "- Packaged wallet reads: PASS",
        "- Launch configuration summary observed: PASS",
        "",
        "## Chain",
        "",
        "- Height: $($ChainInfo.result.height)",
        "- Difficulty: $($ChainInfo.result.difficulty)",
        "- Genesis hash: $GenesisHash",
        "- Latest hash: $LatestHash",
        "- Data dir: $DataDir",
        "",
        "## Wallet",
        "",
        "- Producer address: $ProducerAddress",
        "- Balance: $($Balance.balance_opl)",
        "- Minimum refiner bond: $($BondMinimum.minimum_refiner_bond_opl)",
        "",
        "## Supply",
        "",
        "- Total issued: $($Supply.result.total_issued_opl)",
        "- Total burned: $($Supply.result.total_burned_opl)",
        "- Circulating supply: $($Supply.result.circulating_supply_opl)",
        "",
        "## Mint Ledger",
        "",
        "- Total issued: $($Ledger.total_issued_opl)",
        "- Total burned: $($Ledger.total_burned_opl)",
        "- Mined blocks: $($Ledger.total_mined_blocks)",
        "- Refined blocks: $($Ledger.total_refined_blocks)"
    )
    $Lines | Out-File $ReportPath -Encoding utf8

    Write-Host "RELEASE SMOKE PASS"
    Write-Host "REPORT=$ReportPath"
}
finally {
    if ($NodeProc) {
        Stop-SmokeProcess -Process $NodeProc
    }
    Remove-Item Env:\OPOLYS_MNEMONIC -ErrorAction SilentlyContinue
    if ($null -eq $PreviousRustLog) {
        Remove-Item Env:\RUST_LOG -ErrorAction SilentlyContinue
    } else {
        $env:RUST_LOG = $PreviousRustLog
    }
}
