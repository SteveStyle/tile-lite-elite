# Authentication and Game Invitations

## Overview

The Scrabble PX server provides two complementary systems:

1. **Authentication** — Player identification and session management
2. **Game Invitations** — Invite players to join games without needing to know all participants upfront

## Authentication API

### Register a New Player

**Endpoint:** `POST /auth/register`

Create a new player account.

**Request:**
```json
{
  "display_name": "Alice",
  "email": "alice@example.com",
  "recovery_secret": "my-secret-phrase"
}
```

**Response:** `200 OK`
```json
{
  "player_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_token": "660e8400-e29b-41d4-a716-446655440000",
  "display_name": "Alice",
  "email": "alice@example.com"
}
```

**Errors:**
- `400 Bad Request` — Invalid input or player already exists

**Notes:**
- Store the `session_token` securely on the client (e.g., localStorage for web, secure storage on desktop)
- The `recovery_secret` is hashed server-side; use it to restore your account on another device
- Email is captured for future account recovery; no verification required in MVP

### Login with Recovery Secret

**Endpoint:** `POST /auth/login`

Restore an existing player account using display name + recovery secret.

**Request:**
```json
{
  "display_name": "Alice",
  "recovery_secret": "my-secret-phrase"
}
```

**Response:** `200 OK`
```json
{
  "player_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_token": "770e8400-e29b-41d4-a716-446655440000",
  "display_name": "Alice",
  "email": "alice@example.com"
}
```

**Errors:**
- `400 Bad Request` — Invalid recovery secret
- `404 Not Found` — Player not found

**Notes:**
- Use this endpoint when opening the app on a new device
- Each login generates a fresh session token
- No password; just the recovery secret and display name

### Validate Session

**Endpoint:** `POST /auth/validate`

Check if a session token is still valid (not yet implemented in MVP).

**Request:**
```json
{
  "session_token": "770e8400-e29b-41d4-a716-446655440000"
}
```

**Response:** `200 OK` (future implementation)
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "display_name": "Alice",
  "email": "alice@example.com",
  "created_at": "1234567890",
  "last_seen_at": "1234567890"
}
```

## Game Invitations API

### Invite a Player to a Game

**Endpoint:** `POST /games/{game_id}/invite`

Invite another player to join a specific game seat.

**Request:**
```json
{
  "invited_display_name": "Bob",
  "seat_number": 1
}
```

**Response:** `200 OK`
```json
{
  "id": "inv-550e8400-e29b-41d4-a716-446655440000",
  "game_id": "game-123",
  "invited_player_id": "player-bob-id",
  "inviting_player_id": "player-alice-id",
  "seat_number": 1,
  "status": "pending",
  "created_at": "1234567890",
  "responded_at": null,
  "inviting_player_display_name": "Alice"
}
```

**Errors:**
- `400 Bad Request` — Game not in waiting state, or invited player not found
- `404 Not Found` — Game not found

**Notes:**
- Only games in "waiting" status can receive invitations
- The inviting player is inferred from the first participant in the game
- The same display name must exist as a player in the system

### List Pending Invitations for a Player

**Endpoint:** `GET /players/{player_id}/invitations`

Retrieve all pending and responded invitations for a player.

**Response:** `200 OK`
```json
[
  {
    "id": "inv-550e8400-e29b-41d4-a716-446655440000",
    "game_id": "game-123",
    "invited_player_id": "player-bob-id",
    "inviting_player_id": "player-alice-id",
    "seat_number": 1,
    "status": "pending",
    "created_at": "1234567890",
    "responded_at": null,
    "inviting_player_display_name": "Alice"
  },
  {
    "id": "inv-660e8400-e29b-41d4-a716-446655440000",
    "game_id": "game-456",
    "invited_player_id": "player-bob-id",
    "inviting_player_id": "player-charlie-id",
    "seat_number": 2,
    "status": "rejected",
    "created_at": "1234567888",
    "responded_at": "1234567889",
    "inviting_player_display_name": "Charlie"
  }
]
```

**Status Values:**
- `pending` — Invitation awaiting response
- `accepted` — Player accepted
- `rejected` — Player declined
- `cancelled` — Inviter cancelled

### Accept an Invitation

**Endpoint:** `POST /invitations/{invitation_id}/accept`

Accept an invitation to join a game.

**Response:** `200 OK`
```json
{
  "status": "accepted"
}
```

**Errors:**
- `400 Bad Request` — Failed to update invitation
- `404 Not Found` — Invitation not found

**Notes:**
- Accepting an invitation updates the invitation status to "accepted"
- Server does not automatically add the player to the game seat; that's handled by game start logic
- A future version may automatically place the player in the seat

### Reject an Invitation

**Endpoint:** `POST /invitations/{invitation_id}/reject`

Decline an invitation to join a game.

**Response:** `200 OK`
```json
{
  "status": "rejected"
}
```

**Errors:**
- `400 Bad Request` — Failed to update invitation
- `404 Not Found` — Invitation not found

## Usage Flow

### Scenario: Alice Invites Bob to a Game

1. **Alice creates a game:**
   ```bash
   POST /games
   {
     "seats": [
       { "kind": "human", "display_name": "Alice" }
     ]
   }
   ```
   Response: Game ID is `game-123`

2. **Alice invites Bob:**
   ```bash
   POST /games/game-123/invite
   {
     "invited_display_name": "Bob",
     "seat_number": 1
   }
   ```
   Response: Invitation created

3. **Bob checks his invitations:**
   ```bash
   GET /players/{bob_id}/invitations
   ```
   Response: List of invitations including Alice's

4. **Bob accepts:**
   ```bash
   POST /invitations/{invitation_id}/accept
   ```
   Response: Invitation status = "accepted"

5. **Alice starts the game:**
   ```bash
   POST /games/game-123/start
   ```
   Both players are now in the game

## Database Schema

### `players` table
- `id` — Unique player identifier
- `display_name` — Player username
- `email` — Contact email (for recovery)
- `recovery_secret_hash` — Hashed recovery secret (not the raw secret)
- `created_at` — Registration timestamp
- `updated_at` — Last profile update
- `last_seen_at` — Last activity timestamp

### `sessions` table
- `id` — Unique session identifier
- `player_id` — Reference to `players.id`
- `token_hash` — Hashed session token (not the raw token)
- `created_at` — Session creation timestamp
- `last_seen_at` — Last activity timestamp
- `expires_at` — Optional expiration (not yet used in MVP)

### `game_invitations` table
- `id` — Unique invitation identifier
- `game_id` — Reference to `games.id`
- `invited_player_id` — Reference to `players.id`
- `inviting_player_id` — Reference to `players.id`
- `seat_number` — Which seat in the game is being offered
- `status` — "pending", "accepted", "rejected", or "cancelled"
- `created_at` — Invitation sent timestamp
- `responded_at` — Response timestamp (if responded)

Unique constraint: `(game_id, invited_player_id, seat_number)`

## Security Notes

- All secrets (recovery_secret, session_token) are hashed using a DefaultHasher before storage
- In production, use bcrypt or scrypt instead of DefaultHasher
- Session tokens are opaque UUIDs; clients must treat them like passwords
- Email is unverified in MVP; future versions can add verification flow
- Invitations carry no authentication; game-level access control is handled separately

## Future Enhancements

- Session expiration and refresh tokens
- Email verification with short codes
- Invitation timeout and auto-cancellation
- Player blocking / ignore list
- Invitation via email (send invitation link)
- Player search / directory
