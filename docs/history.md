# Project History

## Phase 1: Command-Line Prototypes

The project began with two Rust prototype crates, `first-try` and `second-try`, both centered on a terminal-based Scrabble game loop.

These prototypes established the core gameplay concepts:

- board representation
- tile bag and rack management
- player turns
- word lookup and validation
- basic game flow for human and computer players

The two crates reflect iteration rather than separate products. They share most of the same domain logic and differ mainly in how the prototype was organized over time.

## Phase 2: UI Direction

The `ui` crate introduced a Dioxus-based application, shifting the project toward a web and desktop presentation layer.

This phase clarified the intended product shape:

- a browser-first experience
- a desktop launch path
- a reusable UI surface for different clients
- a clearer separation between UI and game logic

## Phase 3: Productization

The next step is to turn the prototype logic into a server-owned game platform with pluggable engines and multiple client types.

This is the point where the project stops being a single game loop and becomes a reusable multiplayer system.
