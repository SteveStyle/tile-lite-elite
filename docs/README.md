# Scrabble PX Docs

This folder collects the project plan and architecture notes for the final version of the Scrabble project.

## Documents

- [Operations Guide](operations.md)
- [Authentication and Game Invitations](authentication-and-invitations.md)
  - [Quick Start Examples](authentication-examples.md)
- [Project History](history.md)
- [Roadmap](roadmap.md)
- [Architecture](architecture.md)
- [Engine Interface](engine-interface.md)
- [Components And Interactions](components-and-interactions.md)
- [Technology Decisions](technology-decisions.md)
- [Persistence](persistence.md)
- [Authentication](authentication.md)
- [Schema](schema.md)
- [Rules Engine](rules-engine.md)
- [Rules Engine Implementation](rules-engine-implementation.md)

## Current Direction

The project is moving toward a client-server design where the server owns game state and rule enforcement, and clients are thin presentation layers for web, desktop, CLI, or mobile.

The engine system is designed so multiple computer engines can plug into the server and play against human or computer opponents.

The project is a hobby project, so the architecture should favor local-first development and hosting options that are free or nearly free to run.

Axum is the backend web server layer for the project; no separate web server is required unless deployment needs change later.
