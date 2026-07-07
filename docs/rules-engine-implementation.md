# Rules Engine Implementation Plan

This document turns the rules-engine design into a concrete implementation plan.

It covers:

- crate and module layout
- core Rust types
- cache invalidation after `apply_move`

## Workspace Shape

The cleanest split is a small set of crates with one-way dependencies.

```text
rules-shared
  ├─ dictionary
  ├─ model
  ├─ board
  ├─ cache
  ├─ validate
  ├─ score
  └─ generate

engine-core
  ├─ evaluator
  ├─ search
  ├─ ordering
  └─ policy

server-game
  ├─ game_state
  ├─ game_service
  └─ persistence

clients/*
  └─ previews built on rules-shared
```

Dependency direction:

- `rules-shared` depends on no engine code
- `engine-core` depends on `rules-shared`
- `server-game` depends on `rules-shared` and optionally `engine-core`
- clients depend on `rules-shared` only for preview and validation feedback

## Suggested Modules In `rules-shared`

### `dictionary`

- lexicon loading
- word existence lookup
- prefix or bidirectional generation support
- language-specific assets such as SOWPODS

The first implementation can use the existing simple lookup model. Later, this module should be revisited to evaluate faster dictionary structures for generation-heavy workloads.

### `model`

- `Direction`
- `Position`
- `Letter`
- `Tile`
- `Rack`
- `Premium`
- `VariantRules`
- `MoveCandidate`
- `ValidatedMove`
- `MovePreview`
- `MoveScore`
- `MoveError`

### `board`

- canonical board cells
- filled and empty cell state
- board read and write helpers
- row and column stepping

### `cache`

- directional cross-check data
- anchor flags
- row and column extents
- incremental recomputation logic

### `validate`

- rack fit checks
- placement geometry
- main-word legality
- perpendicular legality

### `score`

- main-word score
- perpendicular score
- premium application
- bingo bonus

### `generate`

- legal move enumeration primitives
- lane traversal
- anchor traversal
- rack-aware legal expansion

## Core Rust Types

### Primitive Types

```rust
pub type LetterMask = u32;
pub type Score = i16;
```

`LetterMask` uses 26 bits, one per letter.

### Board Types

```rust
pub struct BoardState {
    pub cells: [BoardCell; 225],
}

pub enum BoardCell {
    Empty(EmptyCell),
    Filled(FilledCell),
}

pub struct FilledCell {
    pub letter: Letter,
    pub is_blank: bool,
}

pub struct EmptyCell {
    pub premium: Premium,
}
```

The canonical board should stay minimal. Derived legality data belongs in the cache, not in the canonical cell itself.

### Cache Types

```rust
pub struct RuleCache {
    pub cells: [CachedCell; 225],
    pub extents: LineExtents,
}

pub struct CachedCell {
    pub horizontal: CrossCheck,
    pub vertical: CrossCheck,
    pub anchor_flags: AnchorFlags,
}

pub enum CrossCheck {
    Unconstrained,
    Constrained(ConstrainedCrossCheck),
}

pub struct ConstrainedCrossCheck {
    pub allowed_mask: LetterMask,
    pub score_by_letter: [Score; 26],
}

pub struct AnchorFlags {
    pub horizontal_anchor: bool,
    pub vertical_anchor: bool,
}

pub struct LineExtents {
    pub row_left: [u8; 225],
    pub row_right: [u8; 225],
    pub col_top: [u8; 225],
    pub col_bottom: [u8; 225],
}
```

### Move Types

```rust
pub struct MoveCandidate {
    pub start: Position,
    pub direction: Direction,
    pub tiles: Vec<TilePlacement>,
}

pub struct TilePlacement {
    pub offset: u8,
    pub tile: Tile,
}

pub struct MovePreview {
    pub legal: bool,
    pub main_word: String,
    pub total_score: Score,
    pub cross_words: Vec<CrossWordPreview>,
    pub error: Option<MoveError>,
}

pub struct CrossWordPreview {
    pub pos: Position,
    pub word: String,
    pub score: Score,
}

pub struct MoveScore {
    pub total: Score,
    pub main_word_score: Score,
    pub cross_word_score: Score,
    pub bingo_bonus: Score,
}

pub struct ValidatedMove {
    pub candidate: MoveCandidate,
    pub preview: MovePreview,
    pub score: MoveScore,
}
```

The engine should consume `MoveCandidate` and `MovePreview`, not board internals.

## `LetterMask` Helpers

Use small helpers so both rules and engine code stay readable.

```rust
pub fn mask_contains(mask: LetterMask, letter: Letter) -> bool;
pub fn mask_insert(mask: &mut LetterMask, letter: Letter);
pub fn mask_is_empty(mask: LetterMask) -> bool;
pub fn mask_is_full(mask: LetterMask) -> bool;
pub fn mask_allows_rack(mask: LetterMask, rack: &Rack) -> bool;
```

This replaces the old `LetterSet` concept cleanly.

## Validation API

The rules API should be explicit about what is geometric validation, what is lexical validation, and what is scoring.

```rust
impl RulesEngine {
    pub fn preview_move(&self, state: &GameState, candidate: &MoveCandidate) -> MovePreview;

    pub fn validate_move(
        &self,
        state: &GameState,
        candidate: &MoveCandidate,
    ) -> Result<ValidatedMove, MoveError>;

    pub fn apply_move(
        &self,
        state: &mut GameState,
        validated: &ValidatedMove,
    );

    pub fn enumerate_legal_moves(
        &self,
        state: &GameState,
        rack: &Rack,
    ) -> impl Iterator<Item = MoveCandidate>;
}
```

## Cache Invalidation After `apply_move`

The first implementation should be correct before it is minimal.

### Inputs

After a validated move is applied, we know:

- placed positions
- direction of the main word
- affected row and column spans

### Conservative Update Plan

1. Write the placed tiles into `BoardState`.
2. Mark all placed positions as filled in the cache.
3. Recompute row extents for each affected row.
4. Recompute column extents for each affected column.
5. Recompute cross-checks for empty cells near the affected rows.
6. Recompute cross-checks for empty cells near the affected columns.
7. Recompute anchor flags around all changed cells.
8. Recompute any cached lane descriptors if we add them later.

This is likely more work than strictly necessary, but it is straightforward and safe.

### Suggested Changed Region

For each placed tile:

- the tile cell itself
- empty cells contiguous in the same row until a blocking empty region is reached
- empty cells contiguous in the same column until a blocking empty region is reached
- immediate neighbors in all four directions for anchor recalculation

If that still feels too subtle, recompute the full affected row and full affected column. On a 15x15 board, that is still cheap enough for a first correct implementation.

### Recompute Order

The order matters.

1. canonical board cells
2. row and column extents
3. directional cross-checks
4. anchor flags
5. optional generation helpers

Cross-check computation depends on the board state and sometimes on extents. Anchor flags depend on board connectivity and cross-check state.

## Cross-Check Computation

For an empty cell and a candidate placement direction:

1. inspect the perpendicular line
2. determine whether existing neighboring tiles create a perpendicular word pattern
3. if no perpendicular pattern exists, return `CrossCheck::Unconstrained`
4. otherwise, try all 26 letters
5. keep only letters that form a valid perpendicular word
6. store the resulting mask and perpendicular score contribution

That makes one cache responsible for both legality and perpendicular scoring.

## Anchor Computation

An anchor should mean an empty cell that is useful as part of a legal new move.

At minimum, an anchor cell is an empty cell that:

- touches existing tiles, or
- is the center starting square before the first move

Directional anchor flags can then refine whether the cell is useful for horizontal or vertical generation.

## Enumeration Boundary

The move generator in `rules-shared` should produce legal candidates, not strategic choices.

A good shape is:

- iterate by direction
- iterate by anchor or lane
- use cross-check masks to prune letters
- emit `MoveCandidate`

The engine can then rank, filter, and search over those emitted candidates.

## First Implementation Strategy

The fastest path to a working rewrite is:

1. replace `LetterSet` with `LetterMask`
2. move directional legality cache out of the canonical cell type into a `RuleCache`
3. keep whole-row and whole-column recomputation first
4. implement `preview_move` and `validate_move`
5. implement a basic legal-move enumerator
6. only then optimize cache locality or dictionary structure

That last step is important: dictionary restructuring should be done after the initial rules path works, so it is driven by measured generation and validation costs rather than guesswork.

## Recommendation

Build the first version around a clean `BoardState + RuleCache` split.

That gives you:

- a reusable rules engine for server, client, and engine proxy
- exact move preview support
- a solid basis for human validation and engine enumeration
- a clean boundary so engine logic does not leak into rules