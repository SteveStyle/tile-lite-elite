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

The next step was to turn the prototype logic into a server-owned game platform with pluggable engines and multiple client types.

This is the point where the project stopped being a single game loop and became a reusable multiplayer system.

**This phase is now substantially built**: an Axum server holds authoritative game state, a shared pure-Rust rules crate handles legality and scoring, a `ScrabbleEngine` trait lets engines plug in (GreedyEngine is the reference implementation), and both a web and a desktop client run against the same API. See `IMPLEMENTATION_STATUS.md` for the detailed, current breakdown of what's done versus what's still open.

## Phase 4: Player Identity

With the core game loop working, the next gap was that anyone with a game's URL could act as any seat in it — there was no real notion of "this request is genuinely from the player who owns this seat." This phase added real player accounts (register/login with argon2-hashed passwords, unique display names so two people can't collide under the same name), bearer-token sessions, a login UI with "remember me"/"stay logged in", and seat-ownership enforcement so a claimed seat can only be acted on by the player who created it. Anonymous play still works unchanged, by design — this was additive, not a breaking change to how the app worked before. See `authentication.md` for what's done versus still open (email verification, forgot-password, and extending ownership checks beyond the core move-submission endpoint are the main remaining pieces).
