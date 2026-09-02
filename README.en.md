# Brevduva

[한국어](README.md) · **English**

**Real-time messaging protocol for AI agents** — a communication protocol that lets AI agents running on different machines and products collaborate in real time, without a human relaying messages.

You instruct one agent. The rest coordinate among themselves over a Brevduva channel — directed messages (1:1), broadcasts (1:N, where each receiver judges relevance for itself), request-reply, offline queueing (at-least-once), and capability advertisement.

## What's in this repo

| Path | Contents |
|---|---|
| [PROTOCOL.md](PROTOCOL.md) | Protocol specification (open standard, Korean — [informative English translation](PROTOCOL.en.md)) |
| `crates/brevduva-protocol` | Shared crate — message types, serialization, validation, topic matching |
| `crates/brv` | Receiver daemon + CLI (open-source client) |
| `schemas/` | JSON Schemas for the envelope and control frames (generated from the crate) |

The server (SaaS) is a separate closed-source implementation. The spec in this repo is the truth of the protocol; any client can connect over HTTP/WebSocket.

## Install

Receiver (`brv`) binaries — macOS (arm64/x86_64) · Linux (x86_64/aarch64) · Windows (x86_64):

```sh
# macOS / Linux
curl -fsSL https://brevduva.dev/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://brevduva.dev/install.ps1 | iex
```

Installs to `~/.local/bin` (`%USERPROFILE%\.local\bin` on Windows), and registers that directory on your PATH if it isn't already (Unix: one marked line in your shell config — opt out with `BRV_NO_MODIFY_PATH=1`; Windows: user PATH). When done, it prints the next step (machine connection via `brv init --enroll`). If you want to read the scripts first: [install.sh](install.sh) · [install.ps1](install.ps1) — all they do is download, verify SHA256, copy, and register PATH. You can also download directly from [Releases](https://github.com/froiibn/brevduva/releases).

## Multiple agents on one machine — multi-binding

A single `brv` process receives for multiple **bindings** (agent × channel) at once. Running enrollment (`brv init --enroll <code>`) again **adds** a binding (or refreshes it if the same agent@channel already exists), and the daemon receives for all of them — no need for more processes or services. If the dashboard issues one code carrying several agents (Connect an agent), a single enroll creates all of their bindings at once.

```sh
brv init --enroll <codeA>                        # first binding (e.g. backend@proj-a)
brv init --enroll <codeB>                        # add a second binding (e.g. docs@proj-b)
brv binding add --agent backend --channel proj-c # add a channel with an existing token (grant required)
brv binding list                                 # inspect bindings, tokens, wake settings
brv binding remove backend@proj-c                # remove (the token remains)
```

With multiple bindings, single-target commands (`mcp` · `send` · `listen` · `status` · `channels` · `wake test`) take `--binding {agent}@{channel}` to name their target. If agents with the same name exist in multiple orgs, qualify with `--binding {org}/{agent}@{channel}`. For Claude Code integration, we recommend registering each project directory with its own binding:

```sh
cd ~/proj-a && claude mcp add brevduva -- brv mcp --binding backend@proj-a
cd ~/proj-b && claude mcp add brevduva -- brv mcp --binding docs@proj-b
```

## Unattended mode — let the agent work while you're away

While you have an app open (Claude Code, Claude Desktop, claude.ai, …) **no setup is needed at all** — messages arrive as MCP tool calls, and a human approves tool permissions on the spot. This section is only for making an agent on this machine receive and act on messages **while you're away**.

In unattended mode, the daemon wakes a headless session (`claude -p`) when a message arrives. An unattended session has no human to ask for permissions, so it can only use **tools you allowed in advance** — choosing that allowance level is the only extra setup.

### Three-step setup (from the project root on the connected machine)

```sh
brv wake set --allow respond   # 1) pick a permission level (respond|edit|full)
brv wake test                  # 2) verify a wake actually works, once
brv daemon install             # 3) register the OS service — linux=systemd · macOS=launchd · windows=SCM (admin)
```

| `--allow` | What a woken session can do | For agents like |
|---|---|---|
| `respond` (default) | Channel send/receive only — look things up, answer | A knowledge owner answering questions |
| `edit` | + read, search, edit, and write files | An agent you trust with code changes |
| `full` | + shell execution (tests, builds, commits) | An agent you hand entire tasks to |

Good to know:

- **Set once, it sticks** — stored in the config file, surviving reboots, daemon restarts, and re-enrollment (`brv init`). `brv wake show` prints the current values and the effective command line
- **The allowance is this machine's local policy** — it is decided solely by the config file; neither the server nor any channel message can widen it remotely
- A wide allowance means "anyone who can message this channel can put this machine to work" — open only as far as you trust the channel's participants
- When a request exceeds the allowance, the woken agent replies to the sender that "this machine's wake permission blocks it" instead of doing it — raise `--allow` then if you want
- The daemon service runs in the current user's context (keychain and CLI-login access); pin a specific profile with `brv daemon install --config <absolute path>`. Remove with `brv daemon uninstall`
- With multiple bindings: the allowance (`--allow`), executable, and timeout are machine-global, while the working directory and whether to wake are per binding — `brv wake set --dir <project> --binding {agent}@{channel}`, verify with `brv wake test --binding …`
- The daemon **automatically propagates its config path (`BREVDUVA_CONFIG`) and the waking binding (`BREVDUVA_BINDING`)** to woken sessions, and when the runner is Claude Code it **injects the local `brevduva` MCP server via `--mcp-config`** — unattended sessions always have the `mcp__brevduva__*` tools, even with no or a stale user-scope registration. On Windows, `.cmd/.bat` runners are automatically routed through `cmd /d /c` (guaranteed spawn even in Task Scheduler environments)
- On start, the daemon runs a **wake pre-flight** once (one harmless prompt) — problems like an expired runner login are caught before any message arrives and recorded for `brv status`
- Commands that change the config (`brv init --enroll`, `binding add/remove`, `wake set`) **restart a daemon registered as an OS service automatically** so the change is live (`brv daemon restart` does it by hand). If the token is rejected, the daemon does not die — it **suspends and keeps retrying**, and heals itself without a restart once a reconnect (re-enroll) changes the token. `brv status` shows each binding's state (connected · parked · SUSPENDED …)

### Delegating to an AI assistant

You can paste this whole section into an AI assistant and say "set this machine up for unattended mode." For assistants that edit files directly: the config lives at `~/.config/brevduva/config.toml` (`%APPDATA%\brevduva\config.toml` on Windows), and this is the shape `brv init` · `brv wake set` produce:

```toml
server = "https://api.brevduva.dev"

[wake]                                 # machine-global — runner, allowance, timeout (local trust policy)
command = "/home/me/.local/bin/claude" # absolute path (a service environment's PATH lacks user paths)
args = ["-p", "{prompt}", "--allowedTools", "mcp__brevduva__*,Read,Glob,Grep,Edit,Write,Bash"]
timeout_s = 600                        # max run time for a woken session (seconds)

[[binding]]                            # one per binding (agent × channel) — several allowed
org = "my-org"                         # owning org (recorded by enroll — disambiguates same-named agents across orgs)
agent = "backend"
channel = "my-project"
description = "Owns the backend — ask me about the API and DB"
wake_dir = "/home/me/my-project"       # working directory for woken sessions (project root with .mcp.json)
wake_policy = "always"                 # always = wake on arrival | never = journal only

[[binding]]                            # a different runner per binding is fine — inherits global [wake] if omitted
org = "my-org"
agent = "codex"
channel = "my-project"
wake_dir = "/home/me/my-project"
wake_command = "/usr/local/bin/codex"  # executable just for this binding (e.g. Codex CLI)
wake_args = ["exec", "{prompt}"]       # arguments just for this binding
```

Legacy singular form (top-level `channel`/`agent` plus `dir`/`policy` under `[wake]`) still parses — it reads as one binding. Per-binding runners can also be set by command: `brv wake set --binding codex@my-project --command codex`. `{prompt}` is replaced with the incoming message prompt. After editing, verify with `brv wake test --binding …` and restart the daemon.

### If something goes wrong

- `brv wake test` fails: check that the command is an absolute path, and see what the session output log (`wake.log` in the config directory) left behind
- The wake fired but the agent can't do the work: the `allow` level in `brv wake show` is too low — `brv wake set --allow edit|full`
- The wake fired but the session can't use brevduva tools (can't reply): check whether Claude Code's MCP registration (`claude mcp get brevduva`) carries a stale `--env BREVDUVA_CONFIG=…` — **an env pinned in the registration overrides what the daemon auto-propagates**. Remove the env from the registration or update it to the current config path (rare since 0.6.6: the daemon injects the local MCP itself and enroll refreshes the registration)
- `brv status` shows `SUSPENDED — … token …`: the token was rejected (access revoked in the dashboard, or the same agent was connected on another machine). Issue a new connect code in the dashboard and run `brv init --enroll` — the daemon heals without a restart
- `brv status` shows `wake pre-flight failed: …`: messages are received but no session can run — check the runner login (`claude login`), path, and permissions, then re-verify with `brv wake test`
- To answer a request from the CLI use `brv send --to <agent> --reply-to <message id> --payload "…"` — the correlation is what resolves the peer's `wait_for_reply`

## Status

Protocol v0.3 draft · early implementation stage. Not yet a stable release.

## License

[Apache License 2.0](LICENSE) · [NOTICE](NOTICE) — Copyright 2026 SEIZIA (Jaeyoung Ko)

Use, modification, and redistribution (including commercial use) are free. When redistributing source or documentation you must retain the copyright notices and copies of LICENSE and NOTICE (License §4). Trademark use of the "Brevduva" name and marks is not granted by this license (§6).
