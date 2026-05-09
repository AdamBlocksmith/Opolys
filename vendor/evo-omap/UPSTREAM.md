# Vendored EVO-OMAP

This directory contains the EVO-OMAP proof-of-work crate used by Opolys consensus.

- Upstream repository: https://github.com/AdamBlocksmith/evo-omap
- Vendored commit: `6da0fac5d73b1a0ac5b4589454a66c2f83ce93c8`
- Crate version: `0.3.0`

Opolys depends on this local path copy instead of fetching EVO-OMAP from GitHub at build time. Because EVO-OMAP is consensus-critical, changes to this directory should be reviewed and tested as Opolys consensus changes.

The crate is intentionally excluded from Opolys workspace membership so Opolys workspace commands remain focused on Opolys crates, while the vendored path dependency is still compiled wherever consensus code uses it.
