# Rules Engine Design

This document turns the `first-try` board model into a concrete rules-engine design.

The goal is to support:

- fast move validation
- fast move scoring
- fast legal move enumeration
- clean separation between rules logic and engine search logic

## Starting Point

In `first-try`, the board already stores directional letter sets on empty cells.

Conceptually, that old model was:

- horizontal letters allowed at an empty cell
- vertical letters allowed at an empty cell

That is the right direction, but it is too narrow.

The new rules engine should store richer, board-derived cache data so that legality and scoring use the same structure.

## Design Principle

The rules engine owns anything that is a deterministic function of:

- the placed tiles on the board
- the board premiums
- the language word list
- the variant rules

The engine layer owns search strategy and ranking.

That means:

- rules engine: legal move constraints, score calculation, move enumeration primitives
- engine layer: search, pruning strategy, candidate ranking, evaluation, transposition caches

## Core Split

There should be three related layers inside the rules subsystem.

### 1. Immutable Rules Data

This is loaded once per variant or language.

- board premium layout
- letter values
- tile distribution
- bingo bonus
- language word list
- dictionary index for word lookup and generation

### 2. Mutable Board State

This is the canonical board position.

- occupied cells
- tile identity, including blanks
- move count
- optional move history hooks

### 3. Derived Rule Cache

This is recomputed incrementally after each move.

- anchor information
- cross-check constraints per cell and direction
- perpendicular score contribution per cell and direction
- contiguous word extents in rows and columns
- optional lane metadata for move generation

## Legality Model

For each empty cell and direction, the rules engine should answer:

- if a tile is placed here as part of a main word in this direction, what letters are legal?
- if a perpendicular word is created, what score does that perpendicular word contribute?

Standard Scrabble semantics apply:

- if placing a tile creates a perpendicular word, that perpendicular word must be valid
- if no perpendicular word is created, there is no perpendicular validity restriction from that square

That means the cache is not just a set of allowed letters. It also needs to distinguish between:

- no perpendicular constraint
- constrained by a perpendicular word pattern

## Proposed Rust Shape

```rust
pub enum Direction {
    Horizontal,
    Vertical,
}

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
    pub horizontal: CrossCheck,
    pub vertical: CrossCheck,
    pub anchor_flags: AnchorFlags,
}

pub enum CrossCheck {
    Unconstrained,
    Constrained {
        allowed_mask: LetterMask,
        score_by_letter: [i16; 26],
    },
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

pub struct RuleCache {
    pub cells: [CachedCell; 225],
    pub extents: LineExtents,
}

pub struct CachedCell {
    pub horizontal: CrossCheck,
    pub vertical: CrossCheck,
    pub anchor_flags: AnchorFlags,
}
```

This keeps the board state explicit and allows the derived cache to be rebuilt or updated without mixing search state into the rules layer.

## Why `CrossCheck::Unconstrained` Matters

Do not encode every empty cell as a plain letter mask.

These are different cases:

- any letter is cross-legal because no perpendicular word is formed
- only some letters are legal because a perpendicular word must be valid

Both may look permissive in a simple mask representation, but they are not the same concept. The first case has no perpendicular score contribution. The second does.

That distinction matters for:

- scoring
- user preview
- legal move enumeration
- future debugging

## What The Cache Should Store

For each empty cell, for each direction:

- whether the cell is an anchor for that direction
- whether placement is unconstrained or perpendicular-constrained
- if constrained, a 26-letter allowed mask
- if constrained, perpendicular score contribution for each legal letter

This gives a single source of truth for both validation and scoring.

## What Belongs In Rules

The rules layer should provide:

- `validate_move(...) -> Result<MoveScore, MoveError>`
- `preview_move(...) -> MovePreview`
- `enumerate_legal_moves(rack, direction, lane_or_anchor)`
- `apply_move(...)`
- `rebuild_cache_after_move(...)`

It may also expose lower-level primitives such as:

- get cross-check for cell and direction
- get line extent for cell
- get anchors in a row or column

## What Does Not Belong In Rules

The following should stay in `engine/core`:

- best-move search
- move ordering
- heuristic evaluation
- rack leave values
- opening books
- simulation or rollout state
- transposition tables
- partial search trees

Those are engine concerns, even if they consume rule-derived caches.

## Legal Move Enumeration Boundary

There is one boundary that needs to stay clear.

The rules layer may enumerate legal moves.

That is not the same as choosing the best move.

So:

- generate legal placements: rules
- score a legal move exactly: rules
- rank or search among legal moves: engine

## Dictionary Structure

The current `first-try` word list is effectively a plain set lookup.

That is enough for `is_word(word)` but weak for high-performance move generation.

The rules engine should eventually use a dictionary structure that supports generation efficiently, such as:

- trie
- DAWG
- GADDAG

This still belongs in the rules layer because it is dictionary infrastructure, not search policy.

This should be treated as an intentional later optimization step. The current rules crate can begin with simple word-existence lookup, but the dictionary representation should be revisited once move generation and cross-check caching are in place and measurable.

## Incremental Cache Update

The cache should update after every applied move.

At minimum, the update needs to recompute:

- each newly filled cell
- nearby empty cells in the same row
- nearby empty cells in the same column
- word extents affected by the newly placed tiles
- anchor flags near the changed region

The first implementation can be conservative and recompute more than necessary. Correctness matters more than perfect locality at the start.

## Suggested API Direction

The clean long-term shape is:

```rust
pub struct RulesEngine {
    pub rules: Arc<RulesData>,
}

impl RulesEngine {
    pub fn validate_move(&self, state: &GameState, candidate: &MoveCandidate) -> Result<MoveScore, MoveError>;
    pub fn preview_move(&self, state: &GameState, candidate: &MoveCandidate) -> MovePreview;
    pub fn apply_move(&self, state: &mut GameState, candidate: &ValidatedMove);
    pub fn enumerate_legal_moves(&self, state: &GameState, rack: &Rack) -> LegalMoveIter;
}
```

The engine layer would depend on this API, not on board internals.

## Recommendation

Use the richer cross-check cache in the rules engine.

Do not move engine ranking or search into the rules engine.

The right split is:

- rules engine owns board-derived legality and score caches
- engine owns strategy and search

That gives fast previews, fast validation, and a reusable move-generation foundation without coupling the whole system to one engine design.