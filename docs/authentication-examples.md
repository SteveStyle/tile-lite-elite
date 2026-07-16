# Authentication and Invitations - Quick Start Examples

This document shows practical examples of using the authentication and game invitation APIs.

## Setup

First, start the server:

```bash
./scripts/services.sh start
```

All examples use `curl` and assume the server is running on `http://127.0.0.1:3000`.

## Example 1: Alice Invites Bob to Play

### Step 1: Alice Registers

```bash
curl -X POST http://127.0.0.1:3000/auth/register \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Alice",
    "email": "alice@example.com",
    "password": "alice-secret-phrase-123"
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
    "password": "bob-secret-phrase-456"
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

### Step 3: Alice Creates a Game

```bash
curl -X POST http://127.0.0.1:3000/games \
  -H "Content-Type: application/json" \
  -d '{
    "seats": [
      {
        "kind": "human",
        "display_name": "Alice"
      }
    ]
  }'
```

**Response:**
```json
{
  "id": "game-123-abc",
  "status": "waiting",
  "variant": "standard",
  "language": "english",
  "board_layout": "standard",
  "turn_number": 0,
  "current_seat": 0,
  "winner_seat": null,
  "bag_count": 100,
  "participants": [
    {
      "seat_number": 0,
      "kind": "human",
      "display_name": "Alice",
      "player_id": null,
      "engine_id": null,
      "score": 0
    }
  ],
  "board": [...],
  "racks": [...],
  "moves": []
}
```

Save the `game_id` (e.g., `game-123-abc`).

### Step 4: Alice Invites Bob to Seat 1

```bash
ALICE_PLAYER_ID="550e8400-e29b-41d4-a716-446655440000"
GAME_ID="game-123-abc"

curl -X POST http://127.0.0.1:3000/games/$GAME_ID/invite \
  -H "Content-Type: application/json" \
  -d '{
    "invited_display_name": "Bob",
    "seat_number": 1
  }'
```

**Response:**
```json
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
```

Save the `invitation_id` (e.g., `inv-550e8400-e29b-41d4-a716-446655440001`).

### Step 5: Bob Checks His Invitations

```bash
BOB_PLAYER_ID="770e8400-e29b-41d4-a716-446655440000"

curl -X GET http://127.0.0.1:3000/players/$BOB_PLAYER_ID/invitations
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

### Step 6: Bob Accepts the Invitation

```bash
INVITATION_ID="inv-550e8400-e29b-41d4-a716-446655440001"

curl -X POST http://127.0.0.1:3000/invitations/$INVITATION_ID/accept
```

**Response:**
```json
{
  "status": "accepted"
}
```

### Step 7: Alice Starts the Game

```bash
GAME_ID="game-123-abc"

curl -X POST http://127.0.0.1:3000/games/$GAME_ID/start \
  -H "Content-Type: application/json" \
  -d '{}'
```

Now both Alice and Bob are in the game and can play!

## Example 2: Alice Logs in on a New Device

### Step 1: Alice Logs in

Assume Alice is on a new laptop. She retrieves her account using her display name + password:

```bash
curl -X POST http://127.0.0.1:3000/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Alice",
    "password": "alice-secret-phrase-123"
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

Note: The `player_id` is the same, but the `session_token` is new. Store the new token securely.

## Example 3: Bob Rejects an Invitation

If Bob doesn't want to play:

```bash
INVITATION_ID="inv-550e8400-e29b-41d4-a716-446655440001"

curl -X POST http://127.0.0.1:3000/invitations/$INVITATION_ID/reject
```

**Response:**
```json
{
  "status": "rejected"
}
```

## Error Scenarios

### Player Not Found

If you invite a player with a display name that doesn't exist:

```bash
curl -X POST http://127.0.0.1:3000/games/game-123-abc/invite \
  -H "Content-Type: application/json" \
  -d '{
    "invited_display_name": "NonExistentPlayer",
    "seat_number": 1
  }'
```

**Response:** `404 Not Found`
```json
{
  "message": "Invited player not found"
}
```

### Game Not in Waiting State

If you try to invite after the game has started:

```bash
curl -X POST http://127.0.0.1:3000/games/game-123-abc/invite \
  -H "Content-Type: application/json" \
  -d '{
    "invited_display_name": "Bob",
    "seat_number": 1
  }'
```

**Response:** `400 Bad Request`
```json
{
  "message": "Game must be in waiting state to invite players"
}
```

### Invalid Password

If Alice tries to log in with the wrong password:

```bash
curl -X POST http://127.0.0.1:3000/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Alice",
    "password": "wrong-secret"
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

- ~~Implement session validation endpoint (`POST /auth/validate`)~~ — done.
- ~~Integrate authentication UI into web/desktop clients~~ — done (Login/Register tabs, Remember me / Stay logged in checkboxes).
- Add email verification flow with short codes.
- Build the "forgot password" flow (`/auth/forgot-password`, `/auth/reset-password`) — currently nothing sends a reset link at all.
- Implement automatic seat assignment when invitation is accepted (see `authentication-and-invitations.md`).
- Extend seat-ownership checks beyond `submit_action` to `start_game`, `preview_move`, `suggest_move`, and the WebSocket events endpoint.
- Add player search / discovery endpoint.
