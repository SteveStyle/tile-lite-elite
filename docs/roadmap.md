# Roadmap

## MVP

- Server-owned game state and rule enforcement ✅
- Human-vs-human, human-vs-engine, and engine-vs-engine games (engine-vs-engine untested, not unbuilt)
- One stable engine interface for plug-ins ✅
- Shared pure rules library used by server, clients, and engine proxies for legality and score previews ✅
- Game creation, join, start, resign, pass, exchange tiles, and move history ✅
- Server-side scoring and legality checks ✅
- Web client first ✅ (desktop client also built, ahead of this list)
- Basic reconnect support ✅ (GET /games/{id} reloads full state; session restore on client relaunch via "stay logged in")
- Deterministic tests for move legality, scoring, and endgame flow ✅ — 32 tests, see `IMPLEMENTATION_STATUS.md`

## v1

- Multiple engine implementations with selectable seat assignment (only GreedyEngine exists so far)
- Engine benchmarking and head-to-head runs
- CLI and desktop clients using the same API (desktop ✅, CLI not built)
- Player identity separate from client device/session identity ✅ — register/login/validate, seat-ownership enforcement on `submit_action`; see `authentication.md` for exactly what's still partial (email verification, forgot-password, ownership checks on other endpoints)
- Lobby flow for private and public games
- Save/load game state (save works; load requires knowing the game id, no discovery UI beyond the games list)
- Spectator mode
- Audit log of moves and server decisions
- Versioned engine contract (version strings exist, no migration path tested)

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
