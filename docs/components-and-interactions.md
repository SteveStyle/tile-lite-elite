# Components And Interactions

This document sketches the first-pass component model and the main interaction flows for the final Scrabble system.

## Component Diagram

```mermaid
flowchart LR
  subgraph Clients
    Web[Web Client]
    Desktop[Desktop Client]
    Cli[CLI Client]
    Mobile[Mobile Client]
  end

  subgraph Server
    Api[Transport API]
    Game[Game Service]
    Rules[Authoritative Rules Engine]
    Proxy[Engine Proxy]
    Registry[Engine Registry]
    Store[Game Store]
  end

  subgraph Shared Logic
    SharedRules[Shared Pure Rules Library]
  end

  subgraph Engines
    EngA[Engine A]
    EngB[Engine B]
  end

  Web --> Api
  Desktop --> Api
  Cli --> Api
  Mobile --> Api

  Api --> Game
  Game --> Store
  Game --> Rules
  Game --> Proxy
  Proxy --> Registry
  Registry --> EngA
  Registry --> EngB
  Proxy --> SharedRules
  Web --> SharedRules
  Desktop --> SharedRules
  Cli --> SharedRules
  Mobile --> SharedRules
  Rules --> SharedRules
```

## Interaction Diagram: Playing A Move

```mermaid
sequenceDiagram
  participant P as Player Client
  participant S as Server API
  participant G as Game Service
  participant R as Shared Rules Library
  participant X as Engine Proxy
  participant E as Engine

  P->>R: preview move / score locally
  P->>S: submit move
  S->>G: forward request
  G->>R: revalidate move and score
  R-->>G: legal / score result
  alt human player
    G->>G: apply move to authoritative state
  else engine-controlled seat
    G->>X: request engine move
    X->>R: preview candidate move
    X->>E: request best move
    E-->>X: engine action
    X-->>G: validated engine action
    G->>G: apply move to authoritative state
  end
  G-->>S: updated game state
  S-->>P: updated state and score
```

## Interaction Notes

- The shared rules library is compiled into the client, the server, and the engine proxy.
- The shared rules library includes the per-language word list; current support is SOWPODS.
- The rules layer and the engine layer are separate concerns; engines can use rules data as input, but engine search state is not part of the shared rules model.
- The server remains authoritative for the final legality and scoring decision.
- Clients and proxies use shared rules only for prediction, feedback, and move evaluation before submission.
- Engine-vs-engine games use the same proxy path as human-vs-engine games.
