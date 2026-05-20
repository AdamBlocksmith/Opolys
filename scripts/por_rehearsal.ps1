param(
    [string]$RunRoot = "por-rehearsal-local"
)

$ErrorActionPreference = "Stop"

function Invoke-CargoTest {
    param(
        [string]$Name,
        [string]$Package,
        [string]$Filter,
        [string]$OutputPath
    )

    Write-Host "STEP: $Name"
    $command = "cargo test -p $Package $Filter 2>&1"
    $output = & cmd.exe /d /c $command
    $exitCode = $LASTEXITCODE
    $output | Tee-Object -FilePath $OutputPath
    if ($exitCode -ne 0) {
        throw "$Name failed"
    }
}

$RunRootPath = Join-Path (Get-Location) $RunRoot
if (Test-Path $RunRootPath) {
    Remove-Item -Recurse -Force $RunRootPath
}
New-Item -ItemType Directory -Path $RunRootPath | Out-Null

$StartedAt = (Get-Date).ToUniversalTime().ToString("o")

Invoke-CargoTest `
    -Name "POR zero issuance and ordinary fee routing" `
    -Package "opolys-node" `
    -Filter "proof_of_refinement_block_pays_fees_not_issuance" `
    -OutputPath (Join-Path $RunRootPath "por-fee-routing.log")

Invoke-CargoTest `
    -Name "POR burns bond assay but pays ordinary fee" `
    -Package "opolys-node" `
    -Filter "proof_of_refinement_burns_assays_but_pays_ordinary_fee" `
    -OutputPath (Join-Path $RunRootPath "por-assay-routing.log")

Invoke-CargoTest `
    -Name "POR has no issuance reward" `
    -Package "opolys-node" `
    -Filter "proof_of_refinement_has_no_issuance_reward" `
    -OutputPath (Join-Path $RunRootPath "por-zero-reward.log")

Invoke-CargoTest `
    -Name "POR attestation behavior" `
    -Package "opolys-node" `
    -Filter "apply_block_counts_valid_included_attestation" `
    -OutputPath (Join-Path $RunRootPath "por-attestation.log")

Invoke-CargoTest `
    -Name "Mined blocks do not receive refiner attestations" `
    -Package "opolys-node" `
    -Filter "active_refiner_does_not_attest_mined_block" `
    -OutputPath (Join-Path $RunRootPath "mined-attestation-guard.log")

Invoke-CargoTest `
    -Name "Full supply accounting invariant" `
    -Package "opolys-node" `
    -Filter "supply_accounting_invariant" `
    -OutputPath (Join-Path $RunRootPath "supply-accounting.log")

$FinishedAt = (Get-Date).ToUniversalTime().ToString("o")
$Commit = (& git rev-parse --short HEAD).Trim()

$Report = @(
    "# Opolys Proof-of-Refinement Rehearsal Report",
    "",
    "Generated: $FinishedAt",
    "Started: $StartedAt",
    "Commit: $Commit",
    "Artifact directory: $RunRootPath",
    "",
    "## Result",
    "",
    "- POR rehearsal: PASS",
    "- Zero issuance for refined blocks: PASS",
    "- Refined-block ordinary fee split between burn and selected refiner: PASS",
    "- Bond assay burn inside refined block: PASS",
    "- Refiner attestation inclusion path: PASS",
    "- Mined-block attestation guard: PASS",
    "- Supply accounting invariant: PASS",
    "",
    "## Scope",
    "",
    "This rehearsal exercises the Proof-of-Refinement path through the node",
    "apply-block logic with an in-process active refiner. It does not bypass",
    "mainnet refiner activation rules in the production binary.",
    "",
    "A fully live node reaches active refiner status only after the normal one",
    "epoch bonding delay. That long-form epoch-maturity rehearsal remains a",
    "separate operator exercise.",
    "",
    "## Verified Invariants",
    "",
    "- Refined blocks mint zero OPL.",
    "- Refined blocks split ordinary user fees between POR fee burn and selected refiner income.",
    "- Bond and unbond assays burn even when included in a refined block.",
    "- Refiner blocks can carry valid attestations.",
    "- Mined blocks do not receive refiner attestations.",
    "- ``total_issued - total_burned`` remains equal to accounted balances, bonded stake, and pending unbonding stake."
)

$ReportPath = Join-Path $RunRootPath "por-rehearsal-report.md"
$Report | Set-Content -Path $ReportPath -Encoding UTF8

Write-Host "POR REHEARSAL PASS"
Write-Host "REPORT=$ReportPath"
