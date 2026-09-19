# Production release contract

OpenForge releases are cut only from a commit for which CI, Security, market-readiness evaluations, desktop packaging, SBOM generation and build provenance have completed successfully.

## Signing material

The release environment owns the Tauri updater key and platform signing credentials. `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` sign updater artifacts. Apple distribution uses `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID`. Windows code signing is supplied through the organization runner or certificate provider configured for the release environment; unsigned Windows artifacts are never promoted to a stable release.

Private keys are never committed, copied into application configuration, exposed to plugins or passed to agent sandboxes.

## Mandatory gates

A stable tag is eligible for publication only when all of these pass:

1. Rust formatting, workspace compilation, Clippy with warnings denied and the complete Rust test suite on Rust 1.90.
2. Frozen-lockfile TypeScript installation, typecheck and production builds.
3. Ruff, mypy and Python tests.
4. JetBrains plugin build and schema/manifest validation.
5. Security workflow, dependency policy and secret scanning.
6. The executable `openforge-evals` market-readiness suite without a failed mandatory threshold.
7. Linux x64, macOS ARM64, macOS x64 and Windows x64 desktop bundles from the same tag.
8. macOS signing/notarization, cryptographically signed updater artifacts and Authenticode-signed stable Windows artifacts.
9. SPDX SBOM and build-provenance attestations attached to the release.
10. Installation, upgrade from the previous stable release, configuration/database migration, rollback and uninstall smoke tests.

A failed mandatory gate leaves the GitHub release in draft state. Release creation is deliberately separated from human promotion authorization.

## Rollback and migrations

Stable releases retain the previous installer and updater manifest. Before a state-schema migration the daemon creates a versioned backup of the OpenForge state directory and records the source and target schema versions. Destructive migrations are prohibited unless a tested reverse transformation or backup restoration path exists. Worker leases are allowed to expire before daemon rollback so another worker can safely resume from the latest durable checkpoint.
