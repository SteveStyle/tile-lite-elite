# Tile Lite Elite Docs

This folder collects the design notes and operating guides for Tile Lite Elite, organized into four numbered groups. Within a group, the number order is the reading order; across groups, roughly: understand the system (1.x) → understand a specific domain (2.x) → run it through its lifecycle (3.x) → look something up (4.x).

## 1.x — Meta: overview, functionality, architecture

- [1.1 Architecture](1.1-architecture.md) — system overview, deployment topology, guiding principles and roles
- [1.2 Components and Interactions](1.2-components-and-interactions.md) — component diagram, move/turn sequence diagrams
- [1.3 Technology Decisions](1.3-technology-decisions.md) — why Axum/SQLite/Dioxus/etc.
- [1.4 Roadmap](1.4-roadmap.md) — CLI prototype → UI direction → MVP → v1 → Later

## 2.x — Design & Domain

- [2.1 Rules Engine](2.1-rules-engine.md)
- [2.2 Rules Engine Implementation](2.2-rules-engine-implementation.md)
- [2.3 Engine Interface](2.3-engine-interface.md)
- [2.4 Persistence](2.4-persistence.md) — original persistence design principles (see [4.2](4.2-database-schema.md) for the as-built schema)
- [2.5 Authentication](2.5-authentication.md)
- [2.6 Authentication Examples](2.6-authentication-examples.md) — worked request/response walkthroughs
- [2.7 Authentication and Invitations](2.7-authentication-and-invitations.md)

## 3.x — Lifecycle

Run in this order for a normal change: Setup once, then Development → Testing, CI & Release → Deployment → Production Support & Maintenance repeatedly.

- [3.0 Tools](3.0-tools.md) — every script, linking to where it's explained
- [3.1 Setup](3.1-setup.md) — one-time: dev machine, Oracle VM, HTTPS, troubleshooting
- [3.2 Development](3.2-development.md) — running services locally, building, resetting local state
- [3.3 Testing, CI & Release](3.3-testing-ci-and-release.md) — `cargo test`, GitHub Actions CI, the local staging environment, the end-to-end release runbook, and how `deploy.sh` ships an image
- [3.4 Production Environment & Operations](3.4-production-environment.md) — the running system: container topology, secrets, admin CLI, inspecting the database, logging, backups, wiping production

## 4.x — Reference

Facts you look up rather than read start to end.

- [4.1 Configuration](4.1-configuration.md) — environments, environment variables, versioning scheme
- [4.2 Database Schema](4.2-database-schema.md)
- [4.3 API Schema](4.3-api-schema.md) — every HTTP/WebSocket endpoint and DTO
- [4.4 snapshot_json Schema](4.4-snapshot-json-schema.md) — the authoritative game-state JSON blob's shape
- [4.5 Data Dictionary](4.5-data-dictionary.md) — where each game field lives across snapshot/DB/DTO, and its kind
- [4.6 Client-Local Storage](4.6-client-local-storage.md) — StoredAuth / chat watermarks kept on the device

## Current Direction

The project is moving toward a client-server design where the server owns game state and rule enforcement, and clients are thin presentation layers for web, desktop, CLI, or mobile.

The engine system is designed so multiple computer engines can plug into the server and play against human or computer opponents.

The project is a hobby project, so the architecture should favor local-first development and hosting options that are free or nearly free to run.

Axum is the backend web server layer for the project; no separate web server is required unless deployment needs change later.
