# nxm-mcp-session — how it works {#nxm-session}

nxm-mcp-session turns a long, noisy coding session into a **standard handoff document** and keeps a **session registry**, so a fresh agent resumes the work without losing quality — and it is a **thin binary over a shared, config-agnostic core**.

- A long session degrades the model; a structured, forward-looking handoff lets the next session restart cleanly.
- It is harness-agnostic: any MCP client (Kiro, Pi, Codex, Claude) drives it over JSON-RPC on stdio.
- All session logic lives in `nxm-session-core`; the binary only resolves config/env and wires logging.
- **14 typed tools**: pure read/state, three transport-driven (Sampling/Elicitation), lifecycle, archive and a cross-repo registry.

@answer scope="nxm-mcp-session v0.5.0 · public edition · MIT"
@meta locale=en

## What it is {#what}

A small, dependency-light MCP server. It does not watch your conversation and does not run in the background: it is **reactive**. A client calls a tool, the server acts and returns structured JSON. Context isolation is by design: the server never sees the conversation, so synthesis runs through the client's own model via MCP Sampling.

@facts id=what-facts
| Transport | JSON-RPC 2.0 over stdio |
| Edition | public, MIT licensed |
| Core | nxm-session-core (all logic) |
| Binary | main.rs + wiring.rs only |
| Handoff | Markdown + YAML frontmatter |

## How is the binary put together? {#architecture}

The binary owns config/env resolution and logging; it injects already-resolved values into the core, which reads no environment at all.

@architecture id=session-architecture title="Thin binary over a shared core"
| client | MCP Client | external |
| binary | nxm-mcp-session | backend |
| wiring | wiring.rs | backend |
| core | nxm-session-core | backend |
| local | sessions/ | database |
| global | ~/nxm/sessions | database |
| client > binary | stdio | emphasis |
| binary > wiring | resolve |
| wiring > core | inject structs | emphasis |
| core > local | handoffs |
| core > global | pointers |

@steps id=arch-steps
| client | MCP Client | Any harness drives the server over JSON-RPC on stdio. | session-architecture | client |
| binary | nxm-mcp-session | Captures HOME/PWD/session id, sets up logging via nxm-shared. | session-architecture | binary |
| wiring | wiring.rs | Reads the [session] config and env, builds the injected structs. | session-architecture | wiring |
| core | nxm-session-core | Receives final values — zero std::env — and runs all session logic. | session-architecture | core |

## What are the 14 tools? {#tools}

| Group | Tools | What they do |
| --- | --- | --- |
| Transport-driven | `generate_handoff`, `update_session`, `audit_last_task` | Distil / update a handoff and audit the last task, via Sampling + Elicitation. |
| Read / state | `list_sessions`, `get_session`, `find_resumable_session`, `mark_consumed`, `create_session`, `register_prompt` | Pure dispatch over the local registry — no transport. |
| Lifecycle | `set_status`, `reopen` | Move a session between states; archive or restore its files. |
| Archive | `archive_session`, `purge_archive` | Save / delete the raw session `.jsonl` (0600, gitignored). |
| Global | `list_global_sessions` | Cross-repo session pointers + a monotonic session number. |

## How does a session move between states? {#lifecycle}

Five states, driven by `set_status` and `reopen`. `done` and `superseded` physically archive the handoff; `done` is terminal, `superseded` can be reopened.

| State | Meaning | Reopen? |
| --- | --- | --- |
| Active | Live in the registry | — |
| On Hold | Paused in place | Yes, in place |
| Abandoned | Dormant, stays in place | Yes, in place |
| Superseded | Archived, replaced by a successor | Yes {good:restores files} |
| Done | Archived, terminal | No {bad:create a successor} |

## How does a fresh session resume? {#resume}

@steps id=resume-steps
| discover | find_resumable_session | At startup, ask for the best unconsumed handoff to resume. |
| read | get_session | Read the chosen handoff document in full before starting. |
| consume | mark_consumed | Flip it to consumed so no one resumes it twice. |
| handoff | generate_handoff | When context fills up again, distil the new session. |

## Running it {#run}

Register it as an MCP server pointing at the installed binary. The product config lives at `~/.config/nxm/nxm-session.toml` — the server reads it itself, whichever client launched it.

```json title="mcp.json"
{
  "mcpServers": {
    "nxm-session": {
      "command": "nxm-mcp-session",
      "args": ["--transport", "stdio"],
      "env": { "RUST_LOG": "info" }
    }
  }
}
```

- [ ] Install the binary with `scripts/release.sh` (keeps the code signature valid).
- [ ] Register it in your client's MCP config.
- [ ] Set `[session] no_elicit` in `~/.config/nxm/nxm-session.toml` if you want to skip the write gate.
- [ ] Reload MCP servers (or run `/mcp`) so the client picks up the current binary.
