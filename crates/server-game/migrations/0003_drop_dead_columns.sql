-- Drop columns that were only ever written with a placeholder value (or never
-- touched at all) and read by nothing anywhere in the codebase -- see
-- docs/4.2-database-schema.md's provenance notes, which had already flagged
-- each of these as dead:
--
--   games.notes                -- always inserted as NULL, never selected
--   game_participants.left_at  -- always inserted as NULL; exits are tracked
--                                 in the game snapshot (`resigned`) + `outcome`
--   game_moves.tiles_json      -- always inserted as NULL; the move's tiles
--                                 live inside payload_json's MoveRecord
--   game_moves.is_validated    -- always inserted as the literal 1
--   sessions.game_id           -- never referenced by any session query
--
-- Safe as a one-step drop despite migrations/README.md rule 3's two-step
-- caution: that rule guards against a running deployment still *reading* a
-- dropped column, but nothing reads these. This is also a single-container
-- deployment, so the same release that runs this migration at startup is the
-- one that stops binding these columns in persistence.rs -- there is never an
-- old+new overlap where live code writes a now-missing column.
--
-- (`last_seen_at` on players/sessions is deliberately NOT dropped here: it is
-- read/displayed and is slated to drive idle-session cleanup -- a dormant
-- feature to wire up, not dead schema.)

alter table games drop column notes;
alter table game_participants drop column left_at;
alter table game_moves drop column tiles_json;
alter table game_moves drop column is_validated;
alter table sessions drop column game_id;
