# Opolys Release Packaging

This guide describes how to produce a local operator package from a clean Opolys checkout. The package is not a governance event and does not change consensus. It is a reproducible bundle of the binaries and launch documents an operator needs to run the node they built.

## What Gets Packaged

The release script builds with the committed `Cargo.lock` and copies these binaries:

- `opolys-node`
- `opl`
- `genesis-ceremony`
- `genesis-keys`

It also includes:

- `README.md`
- `LAUNCH_BINDER.md`
- `MAINNET_LAUNCH.md`
- `OPERATOR_CONFIG.md`
- `THREAT_MODEL.md`
- `RELEASE.md`
- `release-manifest.json`
- `SHA256SUMS.txt`

The manifest records the git commit, package version, Rust compiler version, Cargo version, host triple, and build timestamp. `SHA256SUMS.txt` records the SHA-256 hash of every file in the unpacked package.

## Build A Package

Run from the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build_release.ps1
```

The script requires a clean working tree by default. That protects operators from accidentally packaging local edits. For a private local experiment only, use:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build_release.ps1 -AllowDirty
```

Output is written under `dist/`:

```text
dist/
  opolys-<version>-<commit>-<host>/
    opolys-node.exe
    opl.exe
    genesis-ceremony.exe
    genesis-keys.exe
    release-manifest.json
    SHA256SUMS.txt
    README.md
    LAUNCH_BINDER.md
    MAINNET_LAUNCH.md
    OPERATOR_CONFIG.md
    THREAT_MODEL.md
    RELEASE.md
  opolys-<version>-<commit>-<host>.zip
  opolys-<version>-<commit>-<host>.zip.sha256
```

On Linux or macOS, the executable files do not use `.exe`.

## Verify A Package

First verify the archive hash:

```powershell
Get-FileHash -Algorithm SHA256 dist\opolys-<version>-<commit>-<host>.zip
Get-Content dist\opolys-<version>-<commit>-<host>.zip.sha256
```

The two hashes must match.

After extracting the archive, verify every file in the package:

```powershell
Get-ChildItem dist\opolys-<version>-<commit>-<host> -File |
  Where-Object { $_.Name -ne "SHA256SUMS.txt" } |
  Sort-Object Name |
  ForEach-Object {
      $hash = (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant()
      "$hash  $($_.Name)"
  }
```

The output must match the package's `SHA256SUMS.txt`, except that `SHA256SUMS.txt` itself is not included in the recomputed list.

## Smoke-Test A Package

After building the package, run the packaged binaries without `cargo run`:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release_smoke.ps1
```

The smoke script uses the latest package under `dist/` by default. It runs a
dry-run ceremony, verifies the attestation, exports a throwaway wallet key,
starts the packaged node with loopback RPC, checks RPC health, queries chain
state, runs packaged wallet read commands, and confirms the node printed the
`Launch configuration summary`.

Artifacts are written under `release-smoke-local/`, including:

- `release-smoke-report.md`
- ceremony and verification logs
- node stdout/stderr logs
- throwaway dry-run data

To test a specific package directory:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release_smoke.ps1 `
  -PackageDir dist\opolys-<version>-<commit>-<host>
```

## Operator Rules

- Build from a clean checkout.
- Build with `--locked`; the script enforces this.
- Record the git commit from `release-manifest.json`.
- Verify the package hash before copying binaries to another machine.
- Keep the genesis operator key separate from miner and refiner keys.
- Never run production data with `--allow-dry-run-genesis`.
- Never expose public write RPC without authentication or an authenticated reverse proxy.

For the end-to-end packaged-operator launch sequence, use
`docs/LAUNCH_BINDER.md`. For the source-tree launch runbook, use
`docs/MAINNET_LAUNCH.md`.

## Build Packages On GitHub

The `Release Artifacts` workflow builds downloadable packages on GitHub for:

- Windows x86_64
- Linux x86_64
- macOS arm64

The workflow runs automatically when a tag beginning with `v` is pushed, and it
can also be started manually from GitHub Actions with `workflow_dispatch`.

Each platform uploads an artifact named:

```text
opolys-<platform>
```

Each artifact contains the packaged directory, the `.zip` archive, and the
archive `.sha256` file produced by `scripts/build_release.ps1`.

The release workflow intentionally uses `actions/checkout@v5` and
`actions/upload-artifact@v7`, with
`FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true` as a guard. GitHub began warning that
Node.js 20 actions will be forced onto Node.js 24 starting June 2, 2026, so
release packaging stays on action versions that run on Node.js 24 directly.
