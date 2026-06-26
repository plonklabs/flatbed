# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project adheres to semver — except that `0.0.x` releases are treated
as breaking by Cargo's compatibility rules, so every `0.0.x` bump may
contain breaking changes during the pre-1.0 stabilization window.

## [Unreleased]

### Changed

- **Breaking:** the telemetry config now reads `FLATBED_SERVICE_NAME`,
  `FLATBED_SERVICE_ADDRESS`, and `FLATBED_TELEMETRY_PORT` environment
  variables, renamed from their `PLONK_*` predecessors. Update any
  deployment that set the old names.

## [0.0.1] — 2026-06-26

### Added

- Initial extraction from `plonklabs/plonk` as a standalone repository
  on crates.io. Carries the framework's existing surface: the `#[route]`
  / `#[worker]` macros, the FlatBuffer codegen helper, the Hyper-backed
  server, and the optional `openapi` / `telemetry` / `nats` / `k8s`
  feature gates.

[Unreleased]: https://github.com/plonklabs/flatbed/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/plonklabs/flatbed/releases/tag/v0.0.1
