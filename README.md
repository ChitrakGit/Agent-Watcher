# Agent Watcher

Agent Watcher records local agent commands on Windows and opens their history in a native Rust desktop viewer. The collector runs independently of the window. Each command keeps its agent name, submitted command, available output, and exit or capture status. Logs are ordinary UTF-8 JSON arrays.

**Delivery status.** This is source for version 0.1.0. The SQLite contract checks, JSON Schema checks, and PowerShell integration tests ran in the authoring environment. A Rust toolchain was unavailable there. The Windows build, Rust tests, service lifecycle, and desktop UI still need execution on a Rust-enabled Windows machine. The repository includes a Windows CI workflow and a disposable-VM smoke test.

## Build and try it

Use a current stable Rust MSVC toolchain on Windows x64, Visual Studio C++ Build Tools with the Windows SDK, and PowerShell. The first Cargo build needs access to crates.io or a populated Cargo cache. Node 24 and PowerShell 7.2+ run the supplementary tests. Start in the folder containing `Cargo.toml`.

```powershell
cargo build --release --bins
$watcher = (Resolve-Path .\target\release\agent-watcher.exe).Path
$env:AGENT_WATCHER_HOME = Join-Path $env:LOCALAPPDATA 'AgentWatcher-Demo'

& $watcher run --agent 'Demo agent' -- cmd.exe /d /c 'echo hello & echo diagnostic 1>&2 & exit /b 7'
# The wrapper returns the child exit code, 7 for this example.
& $watcher collect --once
& $watcher files
& $watcher search --agent 'Demo agent' --exit-code 7 --output 'diagnostic'
& .\target\release\agent-watcher-view.exe
```

Open the listed file and select the command to inspect both streams. The demo uses a separate data directory and requires no installation. Clear the override before using the installed service with `Remove-Item Env:AGENT_WATCHER_HOME`.

## Background service

Fastest setup: double-click [Install-AgentWatcher.cmd](Install-AgentWatcher.cmd) (or run `.\Install-AgentWatcher.cmd` / `.\scripts\Setup.ps1` from PowerShell). It builds the release binaries if missing, prompts once for Administrator (UAC) permission to install and start the `AgentWatcher` Windows service, configures Claude Code hooks for your user account, and verifies collector health. Pass `-Uninstall` to remove the service, binaries, and Claude hooks.

Manual steps:

1. Build the release binaries above. From an elevated PowerShell session for the Windows account being enrolled, run `.\scripts\Install.ps1`. The installer registers the service for automatic startup, runs it as LocalService, configures restart recovery, and adds **Agent Watcher** to the Start menu.
2. Enable an adapter from the table below. The service can run before sign-in. GUI agents and their hooks produce events while their own user sessions are running.
3. Open the Start menu shortcut to browse logs. Closing the viewer leaves collection running. The home screen lists the newest local date and numeric version first. **Sources** lists agents observed in retained history.
4. Use `Get-Service AgentWatcher` for SCM status. The installed CLI is `$env:ProgramFiles\AgentWatcher\agent-watcher.exe`. Its `status` command reports collector health and the last heartbeat.
5. For development, `agent-watcher collect` runs the same worker in the foreground. Point every command at the same `--home` or `AGENT_WATCHER_HOME`. One worker owns a given directory.

The default installation enrolls one owner. Logs live under `%ProgramData%\AgentWatcher\<owner-SID>\logs`. The installer grants the owner read access to history and write access to the inbox. Administrators and the collector manage the catalog. Source events identify apps through their adapter or process ancestry. A process running as the enrolled owner can submit a claimed app name.

## Connect agent apps

| Source | How to enable it | What appears |
| --- | --- | --- |
| Configurable local executor | Set the agent’s command runner to `agent-watcher run --agent "App name" -- PROGRAM ARG...`. For shell syntax, pass the actual shell and its normal arguments. | Exact argv represented as a JSON array string, relayed stdout/stderr, child exit code, and launch failures. App attribution is **reported**. Output is captured from redirected streams. Interactive terminal programs can behave differently under redirection. |
| Codex CLI | Use `agent-watcher codex "your task"` in place of `codex exec --json "your task"`. Set `AGENT_WATCHER_CODEX_EXE` to the native executable if your installation normally uses a shell shim. | Command items, agent-reported combined output, and exit codes when present. The adapter relays the original JSONL to stdout. Chat items are excluded from stored logs. Upstream truncation or an unknown completeness guarantee appears in the capture status. |
| Claude Code | Run `pwsh -File .\scripts\ClaudeHooks.ps1` as the enrolled user, then reload the Claude Code session. For a custom binary or settings file use `-Executable` or `-Settings`. Use `-Remove` with the same arguments to remove this registration. | Shell tool requests, completions, denials, interruptions, and available result streams. A request alone stays **requested**. Hooks preserve unrelated settings and allow the tool to continue on capture errors. |
| Windows process audit | In an authorized account with Security log read access, run `.\scripts\Watch-Audit.ps1 -AgentExecutable 'C:\path\agent.exe' -AgentName 'My agent'`. Keep this optional reader running for continuous audit ingestion. It checkpoints records in the current user’s LocalAppData. | Direct child process creations whose parent path and owner SID match. Windows must already emit event 4688. Command arguments depend on audit policy. Attribution is **inferred**, with process metadata and unavailable output. |
| Other agents, GUI-only modes, WSL, containers, and remote execution | Configure a supported command runner at the actual execution boundary, or add a native adapter under `src/adapters`. | Coverage depends on integration. Installing the service alone does not capture every arbitrary agent, its shell built-ins, or its output. |

The Codex and Claude adapters have synthetic protocol fixtures. Compatibility with the installed versions on your PC still requires a live command comparison. Native API references are the [Codex CLI event stream](https://developers.openai.com/codex/noninteractive/), [Claude Code hooks](https://code.claude.com/docs/en/hooks), and [Windows event 4688](https://learn.microsoft.com/windows/security/threat-protection/auditing/event-4688).

## Browse and search

The **Files** screen opens active or sealed chunks. Choose **Search all files** to search the full history. Combine date/time, agent, submitted command, output text, execution outcome, exit code, and capture status. Separate multiple agent names with commas in the viewer. Use a timestamp with a UTC offset for each date boundary. The start is inclusive and the end is exclusive. Text matching is case-insensitive and literal. Output matches can cross parts of the same stream and related files.

| Command | Purpose |
| --- | --- |
| `agent-watcher` or `agent-watcher view` | Open the desktop viewer |
| `agent-watcher files` | List active and sealed chunks as JSON |
| `agent-watcher search --file 2026_11_24_0.json --output timeout --outcome failed` | Search commands represented in a particular chunk |
| `agent-watcher search --from 2026-11-24T00:00:00+05:30 --until 2026-11-25T00:00:00+05:30` | Search a local calendar day using an explicit offset |
| `agent-watcher show COMMAND_ID --page 0` | Read one page of output with its related filenames |
| `agent-watcher collect --once` | Drain and seal an isolated store, then exit |
| `agent-watcher restore` | Import missing sealed-file entries into the catalog with the collector stopped |

Outcome examples include `requested`, `running`, `succeeded`, `failed`, `cancelled`, `denied`, `launchFailed`, `observed`, and `unknown`. Output states include `complete`, `empty`, `partial`, `unavailable`, and `unknown`. A confirmed silent command and a process with unobservable output have different states.

## Log files and recovery

- Each sealed `YYYY_MM_DD_V.json` file is capped at **50,000,000 bytes**. The counter starts at 0 for each PC-local date. File accounting includes UTF-8 bytes, JSON escaping, commas, and array brackets. A long command can continue across chunks. Stable command IDs and sequence numbers let the viewer join the pieces.
- The collector commits incoming records to SQLite WAL transactions. An active chunk is immediately available through the catalog. It becomes an ordinary JSON file on rotation, date change, clean shutdown, or about 60 seconds of inactivity. A collector restart reconciles interrupted publications and checks conflicting file contents before reclaiming a filename. Version numbers remain reserved after retention cleanup.
- Adapters publish bounded event files by atomic rename. The worker removes an event after durable ingestion. Replay uses the original event ID. The shared inbox has a 32 MB event backlog limit. The worker reports malformed entries, unreadable entries, and queue-loss markers in `status`. A failure that also prevents writing a loss marker can leave an unquantified gap.
- Retention defaults to unlimited. To enable a sealed-JSON budget, use the elevated installer with, for example, `-RetentionBytes 5000000000`. This authorizes deletion of the oldest sealed files as the budget is exceeded. Pass zero to disable future cleanup. The budget applies to sealed JSON. The catalog also uses disk space for queryable events and deduplication metadata.
- The viewer normally reads the live catalog in read-only mode. For offline recovery, copy sealed chunks into a fresh directory’s `logs` subfolder, point `--home` there, and open the viewer or run `restore`. Archives with the original plan’s v1 sample format are readable. Keep a stopped service’s complete data directory if active events or unpublished chunks must also be preserved.
- To upgrade, close the viewer and rerun the installer with a newly built binary directory under the same enrolled account. To remove the product, remove Claude hooks first, stop any optional audit reader, then run `.\scripts\Uninstall.ps1` elevated. Uninstallation preserves log history.

## Project layout

| Path | Responsibility |
| --- | --- |
| `src/main.rs`, `src/bin/agent-watcher-view.rs` | CLI and windowed desktop entry points |
| `src/adapters/` | Native agent event mapping, child-process relay, streaming UTF-8 capture |
| `src/model.rs`, `src/redact.rs`, `schemas/` | Event model, streaming credential masking, versioned JSON Schema |
| `src/collector.rs`, `src/spool.rs`, `src/service.rs` | Worker loop, durable inbox handoff, Windows SCM integration |
| `src/storage/` | SQLite schema, rotation, publication, retention, and offline import |
| `src/query.rs`, `src/viewer.rs` | File membership, cross-part filtering, pagination, and native UI |
| `scripts/` | Service lifecycle, Claude hook configuration, audit reader, and Windows tests |
| `tests/`, `tests/fixtures/` | Rust integration tests, independent SQL checks, and synthetic recordings |
| `.github/workflows/ci.yml` | Windows build, test, service smoke test, and binary artifacts |

## Run the test suite

```powershell
cargo test --all-targets
cargo test --test storage large_output -- --ignored
node --test tests/contracts.test.cjs
pwsh -File .\scripts\Test-IntegrationScripts.ps1
```

The Rust suite exercises exact-fit and one-byte-overflow rotation, 120 MB of output over real 50 MB chunks, Unicode, midnight and clock rollback, restart recovery, replay, concurrent producers, failed launch, known exit codes, retention, cross-file and cross-fragment filters, v1 recovery, redaction, and the CLI-to-JSON path. The 120 MB test is opt-in because it writes both catalog and JSON data. The PowerShell tests use disposable settings fixtures.

In a **clean disposable Windows VM**, build the release binaries, then run `.\scripts\Test-WindowsService.ps1 -DisposableMachine` elevated. This installs the service, submits a fixture with the viewer closed, reads it back through the CLI, stops the service to verify sealed files, restarts it, and uninstalls it. It changes the machine only when explicitly run. CI includes this test on its Windows runner.

## Release qualification

This implementation uses a Windows ACL-protected file inbox and a read-only SQLite query path. These replace the plan’s proposed pipes. Search uses indexed command/file membership and a streaming literal output scan. Performance targets at 1 GB of history, a signed installer, forced-power-loss durability, interactive-terminal capture, and a Narrator usability pass still require qualification or further work. The first connected Cargo build generates Cargo.lock. CI uses the generated lockfile for its tests and release build and includes it with the resulting binaries.

Known credential labels and bearer tokens are masked before persistence. The rule set cannot identify every sensitive value in free text. The executor relays the child’s original bytes to its caller. Stored text uses UTF-8, with a capture gap for undecodable bytes. Native agent hooks can supply combined, truncated, or missing output. A native event over 8 MiB exceeds the parser limit. Very long native command descriptions carry a truncation marker.

See [CHANGELOG.md](CHANGELOG.md) for the release history.
