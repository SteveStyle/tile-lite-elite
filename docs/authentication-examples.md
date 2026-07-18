# Authentication and Invitations - Quick Start Examples

This document shows practical examples of using the authentication and game invitation APIs.

## Setup

First, start the server:

```bash
./scripts/services.sh start
```

All examples use `curl` and assume the server is running on `http://127.0.0.1:3000`. Every endpoint below except registration, login, and forgot-password requires a `Authorization: Bearer <session_token>` header — omitting it (or sending an expired/unknown token) gets `401 Unauthorized`.

## Example 1: Alice Invites Bob to Play

### Step 1: Alice Registers

```bash
curl -X POST http://127.0.0.1:3000/auth/register \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Alice",
    "email": "alice@example.com",
    "password": "alice-secret-phrase-123",
    "stay_logged_in": false
  }'
```

**Response:**

```json
{
  "player_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_token": "660e8400-e29b-41d4-a716-446655440000",
  "display_name": "Alice",
  "email": "alice@example.com"
}
```

Save the `session_token` and `player_id` for later use.

### Step 2: Bob Registers

```bash
curl -X POST http://127.0.0.1:3000/auth/register \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Bob",
    "email": "bob@example.com",
    "password": "bob-secret-phrase-456",
    "stay_logged_in": false
  }'
```

**Response:**

```json
{
  "player_id": "770e8400-e29b-41d4-a716-446655440000",
  "session_token": "880e8400-e29b-41d4-a716-446655440000",
  "display_name": "Bob",
  "email": "bob@example.com"
}
```

Save Bob's IDs as well.

### Step 3: Alice Creates a Game, Claiming Her Own Seat and Naming Bob for the Second

Creating a game requires being signed in — every Human seat needs a `claim` (`creator`/`named`/`open`/`email`; see `authentication-and-invitations.md` for the full model), and there's no more "anonymous" game creation.

```bash
ALICE_TOKEN="660e8400-e29b-41d4-a716-446655440000"

curl -X POST http://127.0.0.1:3000/games \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $ALICE_TOKEN" \
  -d '{
    "seats": [
      {
        "kind": "human",
        "display_name": "Alice",
        "engine_id": null,
        "claim": { "type": "creator" }
      },
      {
        "kind": "human",
        "display_name": "Bob",
        "engine_id": null,
        "claim": { "type": "named", "display_name": "Bob" }
      }
    ],
    "seed": null,
    "variant": null,
    "language": null,
    "board_layout": null,
    "move_time_limit_seconds": null
  }'
```

**Response** (trimmed — `board`/`racks`/`moves`/`messages` omitted for brevity):

```json
{
  "id": "game-123-abc",
  "status": "waiting",
  "creator_player_id": "550e8400-e29b-41d4-a716-446655440000",
  "variant": "official",
  "language": "sowpods",
  "board_layout": "official",
  "turn_number": 0,
  "current_seat": 0,
  "winner_seat": null,
  "bag_count": 100,
  "move_time_limit_seconds": 259200,
  "turn_started_at": "1234567890",
  "participants": [
    {
      "seat_number": 0,
      "kind": "human",
      "display_name": "Alice",
      "player_id": "550e8400-e29b-41d4-a716-446655440000",
      "engine_id": null,
      "score": 0,
      "invitation_status": null,
      "invited_email": null
    },
    {
      "seat_number": 1,
      "kind": "human",
      "display_name": "Bob",
      "player_id": null,
      "engine_id": null,
      "score": 0,
      "invitation_status": "pending",
      "invited_email": null
    }
  ]
}
```

Bob's invitation was created — and, with a mail provider configured (see `authentication.md`'s status section), emailed to him — in this same request. Save the `id` (e.g. `game-123-abc`).

### Step 4: Bob Checks His Invitations

Bob's invitation already shows up in his own `GET /games` response too (tagged `invited_by_name`, with `invitation_id` attached) — this endpoint is specifically the "just invitations, any status" view.

```bash
BOB_TOKEN="880e8400-e29b-41d4-a716-446655440000"
BOB_PLAYER_ID="770e8400-e29b-41d4-a716-446655440000"

curl -X GET http://127.0.0.1:3000/players/$BOB_PLAYER_ID/invitations \
  -H "Authorization: Bearer $BOB_TOKEN"
```

**Response:**

```json
[
  {
    "id": "inv-550e8400-e29b-41d4-a716-446655440001",
    "game_id": "game-123-abc",
    "invited_player_id": "770e8400-e29b-41d4-a716-446655440000",
    "inviting_player_id": "550e8400-e29b-41d4-a716-446655440000",
    "seat_number": 1,
    "status": "pending",
    "created_at": "1234567890",
    "responded_at": null,
    "inviting_player_display_name": "Alice"
  }
]
```

### Step 5: Bob Accepts the Invitation

```bash
INVITATION_ID="inv-550e8400-e29b-41d4-a716-446655440001"

curl -X POST http://127.0.0.1:3000/invitations/$INVITATION_ID/accept \
  -H "Authorization: Bearer $BOB_TOKEN"
```

**Response:** the full, updated `GameStateDto` — seat 1's `player_id` is now Bob's, and `display_name` is his real name (not a placeholder). The client uses this response to drop Bob straight into the game rather than making a second `GET /games/{id}` call.

### Step 6: Alice Starts the Game

Only the creator can start a game, and only once every Human seat is claimed.

```bash
GAME_ID="game-123-abc"

curl -X POST http://127.0.0.1:3000/games/$GAME_ID/start \
  -H "Authorization: Bearer $ALICE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}'
```

Now both Alice and Bob are in the game and can play!

## Example 2: Alice Invites Someone by Email

Alice doesn't have to know whether the person she's inviting already has an account — an `email` claim sends a join link instead of requiring an existing `display_name`.

```bash
curl -X POST http://127.0.0.1:3000/games \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $ALICE_TOKEN" \
  -d '{
    "seats": [
      { "kind": "human", "display_name": "Alice", "engine_id": null, "claim": { "type": "creator" } },
      { "kind": "human", "display_name": "carol@example.com", "engine_id": null, "claim": { "type": "email", "email": "carol@example.com" } }
    ],
    "seed": null, "variant": null, "language": null, "board_layout": null, "move_time_limit_seconds": null
  }'
```

The email sent links to `{base_url}/invite?id=<invitation_id>` — a landing page that shows who invited Carol, handles her registering or logging in (or skips straight past that if she's already signed in with "stay logged in"), and always ends on an explicit "Accept invitation from Alice?" confirmation before she actually joins. See `authentication-and-invitations.md`'s "Invite by Email" section for the full flow, including `GET /invitations/{id}/preview` (the unauthenticated call the landing page uses to learn the inviter's name before anything else has happened).

## Example 3: Alice Logs in on a New Device

### Step 1: Alice Logs in

Assume Alice is on a new laptop. She retrieves her account using her display name + password:

```bash
curl -X POST http://127.0.0.1:3000/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Alice",
    "password": "alice-secret-phrase-123",
    "stay_logged_in": true
  }'
```

**Response:**

```json
{
  "player_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_token": "990e8400-e29b-41d4-a716-446655440000",
  "display_name": "Alice",
  "email": "alice@example.com"
}
```

Note: The `player_id` is the same, but the `session_token` is new — and the old one is *not* invalidated by logging in elsewhere (there's no "log out other devices" concept beyond a self-service password change, which invalidates all of them). This example sets `stay_logged_in: true`, so the new session never expires; the earlier registration examples set it `false`, so those sessions expire after 7 days and get cleaned up automatically — see `authentication-and-invitations.md`'s login notes for the full reasoning. Store the new token securely.

## Example 4: Bob Rejects an Invitation

If Bob doesn't want to play:

```bash
curl -X POST http://127.0.0.1:3000/invitations/$INVITATION_ID/reject \
  -H "Authorization: Bearer $BOB_TOKEN"
```

**Response:**

```json
{
  "status": "rejected"
}
```

Note: `reject` only works for a `Named` invitation addressed to the caller — there's no single invitee to reject on behalf of for an `Open` or `Email` invitation (simply not accepting is equivalent).

## Error Scenarios

### Player Not Found

If you invite a player with a display name that doesn't exist:

```bash
curl -X POST http://127.0.0.1:3000/games \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $ALICE_TOKEN" \
  -d '{
    "seats": [
      { "kind": "human", "display_name": "Alice", "engine_id": null, "claim": { "type": "creator" } },
      { "kind": "human", "display_name": "NonExistentPlayer", "engine_id": null, "claim": { "type": "named", "display_name": "NonExistentPlayer" } }
    ],
    "seed": null, "variant": null, "language": null, "board_layout": null, "move_time_limit_seconds": null
  }'
```

**Response:** `404 Not Found`

```json
{
  "message": "No player named 'NonExistentPlayer'"
}
```

Every named invitee is resolved up front, before anything is created — a typo fails the whole request cleanly rather than leaving a half-built game with an unresolvable seat behind.

### Game Not in Waiting State

If you try to add or invite a seat after the game has started:

```bash
curl -X POST http://127.0.0.1:3000/games/$GAME_ID/invite \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $ALICE_TOKEN" \
  -d '{
    "seat_number": 1,
    "invited_display_name": "Bob",
    "invited_email": null
  }'
```

**Response:** `400 Bad Request`

```json
{
  "message": "Game must be in waiting state to invite players"
}
```

### Not the Game's Creator

Roster-management calls (add/remove/invite/reorder/start) are creator-only:

```bash
curl -X POST http://127.0.0.1:3000/games/$GAME_ID/invite \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  -d '{ "seat_number": 1, "invited_display_name": null, "invited_email": null }'
```

**Response:** `401 Unauthorized`

```json
{
  "message": "Only the game's creator can invite players"
}
```

### Invalid Password

If Alice tries to log in with the wrong password:

```bash
curl -X POST http://127.0.0.1:3000/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Alice",
    "password": "wrong-secret",
    "stay_logged_in": false
  }'
```

**Response:** `400 Bad Request`

```json
{
  "message": "Incorrect name or password"
}
```

## Integration with Web/Desktop Clients

### Web Client

Store the session token in localStorage:

```javascript
// After registration or login
const response = await fetch('http://127.0.0.1:3000/auth/register', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    display_name: 'Alice',
    email: 'alice@example.com',
    password: 'alice-secret-phrase-123'
  })
});

const { session_token, player_id } = await response.json();

// Store for future use
localStorage.setItem('session_token', session_token);
localStorage.setItem('player_id', player_id);
```

Every subsequent authenticated call sends `Authorization: Bearer ${session_token}`.

### Desktop Client

**As actually implemented**: a plain JSON file under the OS config directory (via the `dirs` crate — e.g. `~/.config/tile-lite-elite/auth.json` on Linux), holding the remembered display name and, if "Stay logged in" was checked, the raw session token. This is **not** encrypted or OS-keychain-backed — anyone with filesystem access to that account can read a logged-in session token straight out of the file. Fine for a hobby project on a personal machine; revisit before this is ever exposed to a shared or untrusted machine.

```rust
// crates/ui/src/local_storage.rs, roughly:
let response = register_player(server_url, "Alice", "alice@example.com", "secret").await?;
local_storage::save(&StoredAuth {
    remembered_name: Some("Alice".to_string()),
    session_token: Some(response.session_token),
});
```

The web client's equivalent uses real browser `localStorage` (via `gloo-storage`), which has the same "not really a secret store" caveat — anything in `localStorage` is readable by any script with page access.

## Next Steps

Everything this document used to list under "Next Steps" as unbuilt is now done: session validation, auth UI, automatic seat-assignment on accept, seat-ownership checks on every action-capable endpoint (not just `submit_action`), player search/discovery (`GET /players/search?q=`), and an email provider wired up for `/auth/forgot-password` and invitations alike (see `authentication.md`'s status section). What's actually still open:

- Email verification flow with short codes (addresses are captured but never confirmed).
- Refresh tokens / rotating long-lived credentials (session expiration itself is now handled — see `POST /auth/login`'s notes above — but there's no concept of refreshing a session as it nears expiry, or of a "stay logged in" token ever needing to rotate).
- Invitation timeout / auto-cancellation (a `pending` invitation sits forever until the creator resends, removes the seat, or the invitee responds).
