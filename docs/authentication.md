# Authentication And Player Recognition

## Goal

A player should be recognized each time they connect.

That means the system needs a persistent player identity that survives reconnects, separate from the browser, device, or current network session.

## Recommended Model

Use two distinct concepts:

- `player`: the persistent identity of the person using the system
- `session`: a temporary connection token that proves the current client is allowed to act as that player

For connecting from a different client, add a small recovery secret such as a PIN or short passphrase. The secret should be easy enough to enter without much friction, but strong enough to prevent accidental impersonation.

Email should always be captured. It is part of the recovery model, and it gives the player a stable way to restore identity on a different client. The MVP does not need to verify it immediately.

The system should send a verification email after capture, but verification should not block play. The player can continue using the app while the email is pending.

The verification email should use a short verification code that the player enters in the app. That reduces the risk of accidentally verifying from an old or forwarded email and makes the confirmation step explicit.

The server should recognize a returning player by validating their session token and mapping it back to the same player record.

## Practical Behavior

- The first time a player connects, the server creates a player record.
- The server issues a session token and stores a hashed copy.
- The client stores the opaque token locally.
- On later connections, the client presents the token.
- If the token is valid, the server recognizes the same player.
- If the token is missing or invalid, the client can restore identity by entering the same display name plus recovery secret, or become a new anonymous player if that flow is allowed.

## Recommended UX

Keep the first-run flow short:

- choose a display name
- create or accept a short recovery secret
- add an email address for recovery
- optionally trust this device so future connects are automatic

The server should send a verification email after the address is captured.

The email should include a short verification code.

On another client, the player can restore identity by entering:

- the display name, or a lookup handle if we later add one
- the recovery secret

This gives the player a low-friction way to identify themselves without a full account/password system.

## What Should Persist

Persist:

- player identity
- email address for recovery
- email verification sent time
- email verification code hash or equivalent
- email verification status and verified time
- email may be unverified initially, especially for local play
- recovery secret hash or equivalent verifier
- authentication/session tokens as hashes
- last seen time
- any stable display name or preferred name
- optional player preferences

Do not persist:

- raw tokens
- transient UI state
- temporary preview state
- engine search state

## Recommended MVP Approach

For a hobby project, keep this simple:

- anonymous or lightweight players first
- one persistent player record per identity
- one email address per player for recovery
- email can remain unverified in the MVP
- one opaque session token per client/device identity
- one short recovery secret for cross-client restore
- reconnect support based on that token

This avoids a full account system while still letting the server recognize a returning player.

## Security Notes

- Store only token hashes in SQLite.
- Store only recovery-secret hashes in SQLite.
- Store verification-code hashes in SQLite.
- Defer email verification unless we later need account recovery or remote hosting.
- Send verification email without blocking normal play.
- Rotate or expire tokens if needed.
- Treat the token like a password-equivalent secret.
- Treat the recovery secret like a lightweight password-equivalent secret.
- Keep the token transport protected if the app is exposed beyond a local network.

## Interaction With Game Data

Player identity should be separate from game state.

A player can participate in many games, and a game can reference a player through the `game_participants` table.

The server should use the persistent player identity when restoring seats, reconnecting to a game, or attributing moves to the right person.
