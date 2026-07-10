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
    "recovery_secret": "alice-secret-phrase-123"
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
    "recovery_secret": "bob-secret-phrase-456"
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

Assume Alice is on a new laptop. She retrieves her account using her display name + recovery secret:

```bash
curl -X POST http://127.0.0.1:3000/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Alice",
    "recovery_secret": "alice-secret-phrase-123"
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

### Invalid Recovery Secret

If Alice tries to log in with the wrong recovery secret:

```bash
curl -X POST http://127.0.0.1:3000/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Alice",
    "recovery_secret": "wrong-secret"
  }'
```

**Response:** `400 Bad Request`
```json
{
  "message": "Invalid recovery secret"
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
    recovery_secret: 'alice-secret-phrase-123'
  })
});

const { session_token, player_id } = await response.json();

// Store for future use
localStorage.setItem('session_token', session_token);
localStorage.setItem('player_id', player_id);
```

### Desktop Client

Store the session token in the application's secure storage (platform-dependent):

- **Linux/macOS**: Consider `keychain` or encrypted file
- **Windows**: Consider `Credential Manager` or encrypted file
- For MVP: encrypted local file or secure directory

```rust
// Pseudocode
let response = register_player("Alice", "alice@example.com", "secret").await?;
secure_storage::write("session_token", &response.session_token)?;
secure_storage::write("player_id", &response.player_id)?;
```

## Next Steps

- Implement session validation endpoint (`POST /auth/validate`) for full reconnect support
- Add email verification flow with short codes
- Integrate authentication UI into web/desktop clients
- Implement automatic seat assignment when invitation is accepted
- Add player search / discovery endpoint
