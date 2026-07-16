# Technology Decisions

This document is a first-pass technology shortlist for the project, shaped by the current architecture and the constraint that the project should stay free or nearly free to host.

## Recommended Stack

| Area | Recommendation | Pros | Cons |
|---|---|---|---|
| Core language | Rust | Safe, fast, already fits the repo, good shared-core story | Slower to iterate than scripting languages |
| Client framework | Dioxus | One Rust stack for web and desktop, matches current UI work | Smaller ecosystem than mainstream web UI stacks |
| Server framework | Axum | Modern, async-first, good fit for typed APIs | You assemble more infrastructure yourself |
| API style | HTTP + WebSocket hybrid | Good fit for live game updates and client compatibility | More design work than a single protocol |
| Persistence | SQLite first | Simple, local-friendly, low ops burden, free to run | Less scalable than PostgreSQL |
| Engine boundary | In-process trait (`GameEngine`), compiled in | Type-safe, no IPC, engine output flows through the same validation path as a human client's actions; async-wrapped with a timeout so a slow engine can't stall the server | Engines aren't sandboxed from the server process; revisit if untrusted third-party engines are ever supported |
| Shared rules | Pure shared Rust crate | Same legality and scoring logic everywhere | Requires strict dependency discipline |
| Word lists | Embedded per language | Fast, deterministic, offline-friendly | Larger binaries and less flexible updates |
| Message format | Typed Rust model with serialized wire format | Strong contracts and maintainability | Needs versioned serialization from the start |
| Testing | Unit, integration, replay tests | Strong protection against rule regressions | More setup work up front |
| CLI client | ratatui or simple terminal UI | Useful for debugging and power users | Extra work if web is the main focus |
| Mobile | Defer initially | Keeps scope manageable | Delays a client type you eventually want |
| Auth/session | Lightweight sessions first | Faster to build and demo | Not enough for ratings or strong matchmaking |
| Deployment | Server plus thin clients | Matches the architecture cleanly | More moving parts than a monolith |

## Why This Stack Fits The Project

The project is a hobby project, so the stack should minimize cost and operational overhead. That makes Rust, SQLite, and a single-server deployment the safest defaults. Dioxus keeps the web and desktop client story inside the same language, which reduces duplication and makes the shared rules model easier to use.

Axum is the web server layer for the backend. The project does not need a separate web server by default. If we want to serve static client assets from the same process, Axum can handle that too. A separate reverse proxy or static-file server would only be needed for deployment-specific reasons such as TLS termination, caching, or splitting frontend and API hosting.

The engine proxy should stay separate from the shared rules crate. That lets clients and proxies preview legality and score without mixing engine search state into the canonical rules model.

## Practical First Pass

If we want the lowest-risk starting point, the first pass should be:

- Rust workspace for all code
- Dioxus for the client UI
- Axum for the server API
- SQLite for storage
- Shared pure rules crate with embedded word lists
- In-process engine trait, compiled in (not a separate process — see the Engine boundary row above)
- HTTP plus WebSocket transport
