# Changelog

## 0.1.0 · 2026-11-25

### Added

- Rust CLI, native desktop viewer, headless collector, and Windows service entry point.
- Configurable executor with separate stdout and stderr capture, argument-boundary preservation, exit codes, and streaming credential masking.
- Codex CLI structured-event adapter, Claude Code shell hooks, and an optional Windows 4688 direct-child process reader.
- SQLite-backed event journal, bounded local inbox, replay deduplication, crash-aware JSON publication, and opt-in sealed-log retention.
- Daily zero-based `YYYY_MM_DD_V.json` chunks with a hard limit of 50,000,000 serialized bytes.
- Active and sealed file browsing, paginated command output, source coverage, time and text filters, exit-code filtering, and cross-file command reconstruction.
- Legacy v1 log import, version 2 event schema, Windows install and uninstall scripts, and an idempotent Claude hook configurator.
- Rust integration tests, native-agent fixtures, SQLite contract checks, schema and PowerShell checks, a disposable Windows service test, and a Windows CI workflow.

### Verification

Eight independent SQLite, fixture, and file-format tests passed locally, including an exact 50,000,000-byte file and a one-byte-overflow case. The PowerShell checks passed for script syntax, valid and invalid JSON Schema examples, hook installation, repeat installation, and selective removal. Cargo and rustc were unavailable in the authoring environment. The Rust tests, native GUI, service lifecycle, real-agent verification, and performance targets await a Rust-enabled Windows run.
