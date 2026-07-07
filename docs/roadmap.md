# Roadmap

## MVP

- Server-owned game state and rule enforcement
- Human-vs-human, human-vs-engine, and engine-vs-engine games
- One stable engine interface for plug-ins
- Shared pure rules library used by server, clients, and engine proxies for legality and score previews
- Game creation, join, start, resign, pass, exchange tiles, and move history
- Server-side scoring and legality checks
- Web client first
- Basic reconnect support
- Deterministic tests for move legality, scoring, and endgame flow

## v1

- Multiple engine implementations with selectable seat assignment
- Engine benchmarking and head-to-head runs
- CLI and desktop clients using the same API
- Player identity separate from client device/session identity
- Lobby flow for private and public games
- Save/load game state
- Spectator mode
- Audit log of moves and server decisions
- Versioned engine contract

## Later

- Mobile client
- Ranked play and ratings
- Tournaments and ladders
- Chat and notifications
- AI analysis after games
- Accessibility and localization
- More board variants and rule presets
- Engine sandbox hardening and observability
- Self-hostable deployment path that can run on free or near-free infrastructure
