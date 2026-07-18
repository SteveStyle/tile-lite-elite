# Authentication And Player Recognition

## Current Status (as of 2026-07-18)

What's actually implemented, vs. the aspirational design below:

- ✅ Player/session split, with bearer-token sessions (`Authorization: Bearer <token>`) — matches the model described here.
- ✅ Password hashing uses argon2 (not a lightweight PIN scheme) — despite this doc's "short passphrase" framing below, the actual `password` field is a real password with no length/strength cap in the API.
- ✅ `display_name` is enforced unique at the DB level (this doc doesn't mention that requirement explicitly, but it's load-bearing: without it, two players with the same name would collide unpredictably at login).
- ✅ Login/register UI exists (web + desktop), with "Remember me" (prefills display name only) and "Stay logged in" (persists the session token, *and* — since the request now carries the flag server-side too — determines the session's actual lifetime: 7 days if unchecked, no expiry if checked) checkboxes.
- ✅ **Sessions now actually expire** — previously `sessions.expires_at` was schema-only and never set, so every login left a permanent row regardless of "Stay logged in." `RegisterPlayerRequest`/`LoginPlayerRequest` gained a `stay_logged_in: bool` field so the server can tell "abandoned the moment this tab closes" apart from "meant to last"; expired rows are deleted lazily whenever `GET /games` runs, same pattern as the move-time-limit/finished-game cleanup (no background scheduler). See `authentication-and-invitations.md`'s login notes for the full reasoning.
- ✅ **Self-service account editing**: `POST /auth/update-details` (`UpdatePlayerDetailsRequest { display_name, email }`, both optional) changes display name and/or email for the caller — no current-password confirmation required (a valid session is enough), unlike an actual password change. Display name re-checks the same uniqueness constraint as registration; email has no uniqueness constraint, same as registration. UI: "Edit user details" in `AuthPanel` (replaced the old bare "Change password" trigger), with password-change kept as a nested sub-section that still requires the current password.
- ✅ **Seat ownership enforcement, on every action-capable endpoint** — `submit_action`, `start_game`, `preview_move`, `suggest_move`, seat reordering, and the roster-management endpoints below all reject a request for a seat the caller doesn't own (or, for an unclaimed seat, reject everyone). This closed out what used to be a `submit_action`-only gap.
- ⚠️ Email verification: schema exists (`email_verification_*` columns), nothing sends or checks a code yet.
- ✅ **Password-reset ("forgot password") flow**: `POST /auth/forgot-password` (`RequestPasswordResetRequest { email }`, always `204` — same non-enumeration principle as login's shared error message) issues a single-use, hour-lived token (`password_reset_tokens` table, hashed at rest like `sessions.token_hash`) and requesting again retires any earlier still-valid token for that account. `POST /auth/reset-password` (`ResetPasswordRequest { token, new_password }`) consumes it, sets the new password, and invalidates every session for that player, same as self-service change-password. UI: a "Forgot password?" toggle in `AuthPanel`'s login form, and a standalone `/reset-password?token=...` landing page (`crates/ui/src/views/reset_password.rs`) reached by clicking the emailed link — the app has no router, so this is a raw `window.location` path check in `RootApp`, not a route. Note that `players.email` has *deliberately* no uniqueness constraint (one person running several identities under the same email is an accepted use case — see `get_player_by_email`'s doc comment in `persistence.rs`), so `/auth/forgot-password` can only ever reach one arbitrary account for a given email, never all of them.
- ✅ **Transactional email, live in production**: `crates/server-game/src/email.rs` sends via Resend — welcome (on register), a named-invitation notification, a join-invitation email (for `SeatClaim::Email`, see below — a distinct "you don't have an account yet" message, not the named-invitation one), and the forgot-password link above. Content lives in `crates/server-game/emails/*.txt` (plain text, `{{placeholder}}` substitution, no templating engine) — currently placeholder wording, still to be replaced with the project owner's own copy. `mail.tileliteelite.com` is verified with Resend (DKIM/MX/SPF records added under that subdomain in GoDaddy DNS, deliberately not touching the root domain's own GoDaddy-mail records), the API key is configured on the production VM, and real emails have been confirmed delivered/round-tripped end-to-end. `RESEND_API_KEY` unset (the default in local dev) means every send just logs instead, so nothing here needs a live provider to work or to test locally.
- ✅ **Invite by email** (`SeatClaim::Email`) — a fourth way to fill a Human seat alongside Creator/Named/Open, targeting a raw email address with no account required to exist yet. See `authentication-and-invitations.md` for the full seat-claim model, the join-link landing page, and why an email invitation is deliberately excluded from the generic "open invitations" list every signed-in player otherwise sees.

The rest of this document is the original design reasoning and is still broadly accurate in spirit — read it as direction, not as a literal description of what exists.

## Goal

A player should be recognized each time they connect.

That means the system needs a persistent player identity that survives reconnects, separate from the browser, device, or current network session.

## Recommended Model

Use two distinct concepts:

- `player`: the persistent identity of the person using the system
- `session`: a temporary connection token that proves the current client is allowed to act as that player

For connecting from a different client, add a small password such as a PIN or short passphrase. The secret should be easy enough to enter without much friction, but strong enough to prevent accidental impersonation.

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
- If the token is missing or invalid, the client can restore identity by entering the same display name plus password, or become a new anonymous player if that flow is allowed.

## Recommended UX

Keep the first-run flow short:

- choose a display name
- create or accept a short password
- add an email address for recovery
- optionally trust this device so future connects are automatic

The server should send a verification email after the address is captured.

The email should include a short verification code.

On another client, the player can restore identity by entering:

- the display name, or a lookup handle if we later add one
- the password

This gives the player a low-friction way to identify themselves, without the heavier flows (email verification gates, password-strength rules, etc.) a general-purpose account system would need.

## What Should Persist

Persist:

- player identity
- email address for recovery
- email verification sent time
- email verification code hash or equivalent
- email verification status and verified time
- email may be unverified initially, especially for local play
- password hash or equivalent verifier
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
- one short password for cross-client restore
- reconnect support based on that token

This avoids the heavier flows (email verification gates, password-strength rules, etc.) a general-purpose account system would need, while still letting the server recognize a returning player.

## Security Notes

- Store only token hashes in SQLite.
- Store only password hashes in SQLite.
- Store verification-code hashes in SQLite.
- Defer email verification unless we later need account recovery or remote hosting.
- Send verification email without blocking normal play.
- Rotate or expire tokens if needed.
- Treat the token like a password-equivalent secret.
- Treat the password like a lightweight password-equivalent secret.
- Keep the token transport protected if the app is exposed beyond a local network.

## Interaction With Game Data

Player identity should be separate from game state.

A player can participate in many games, and a game can reference a player through the `game_participants` table.

The server should use the persistent player identity when restoring seats, reconnecting to a game, or attributing moves to the right person.
