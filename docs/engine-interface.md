# Engine Interface

## Purpose

The engine interface allows different computer opponents to plug into the server and participate in games.

The engine logic should remain separate from the shared rules logic. Engines may consume shared rules data and helper functions, but they should not own the canonical rules model.

This supports:

- human vs engine play
- engine vs engine play
- benchmarking engines against each other
- swapping strategies without changing the game server

## Requirements

An engine implementation should be:

- deterministic for a given input state when possible
- versioned
- observable through logs and metadata
- safe to run through a server proxy

## Core Contract

At minimum, an engine needs to accept a game snapshot and return an action.

Conceptually:

- input: current game state, player seat, legal move constraints, time budget, supported language word list
- output: chosen move, pass, exchange, or resign

## Suggested Interface Shape

An engine should expose:

- metadata: name, version, author, supported variants
- capability flags: supports timed play, supports analysis, supports ranking
- move generation: produce the next action from a game state
- optional diagnostics: explanation, candidate moves, search depth, timing

Engine implementations may also keep private state for search trees, opening books, evaluation caches, and tuning values that are not part of the shared rules layer.

## Server Proxy Responsibilities

The server proxy should:

- translate game state into the engine request format
- use the shared rules library to preview legality and score candidates
- enforce time limits
- validate the engine response
- reject illegal actions
- record the engine move in the authoritative game history
- isolate the engine from direct access to clients

This keeps the proxy responsive for engine authors while ensuring the server remains the final authority.

## Versioning

The engine interface must be versioned from the start.

That prevents old engines from breaking when the game state or rule model changes.

## Testing Strategy

Every engine should be testable through the same harness.

Useful tests:

- engine can produce a legal move from a standard opening state
- engine respects time budget
- engine-vs-engine match completes without server errors
- engine response is rejected when it becomes illegal
