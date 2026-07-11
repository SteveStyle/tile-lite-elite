use std::collections::HashSet;

use crate::board::{BoardCell, BoardState};
use crate::cache::CrossCheck;
use crate::dictionary::{Dictionary, PrefixCursor};
use crate::model::{Direction, Letter, MoveCandidate, Position, Rack, Tile, TilePlacement};
use crate::validate::{GameState, RulesEngine};

pub trait MoveGenerator<State, Rack> {
    type Iter: Iterator<Item = MoveCandidate>;

    fn enumerate_legal_moves(&self, state: &State, rack: &Rack) -> Self::Iter;
}

impl<'a, D: Dictionary> RulesEngine<'a, D> {
    pub fn enumerate_legal_single_tile_moves(
        &self,
        state: &GameState,
        rack: &Rack,
    ) -> Vec<MoveCandidate> {
        self.enumerate_legal_moves_with_tile_limit(state, rack, 1)
    }

    pub fn enumerate_legal_multi_tile_moves(
        &self,
        state: &GameState,
        rack: &Rack,
    ) -> Vec<MoveCandidate> {
        self.enumerate_legal_moves_with_tile_limit(state, rack, self.rules.rack_size)
    }

    fn enumerate_legal_moves_with_tile_limit(
        &self,
        state: &GameState,
        rack: &Rack,
        max_tiles: u8,
    ) -> Vec<MoveCandidate> {
        let position = state.position_with_rack(rack);
        let mut moves = Vec::new();
        let mut seen = HashSet::new();
        let max_tiles = max_tiles.min(self.rules.rack_size) as usize;

        if max_tiles == 0 {
            return moves;
        }

        for y in 0..self.rules.height {
            for x in 0..self.rules.width {
                let start = Position::new(x, y);
                if !matches!(state.board.get(start), Some(BoardCell::Empty(_))) {
                    continue;
                }

                let cached = &state.cache.cells[start.to_index(BoardState::WIDTH)];
                for direction in [Direction::Horizontal, Direction::Vertical] {
                    let is_anchor = match direction {
                        Direction::Horizontal => cached.anchor_flags.horizontal_anchor,
                        Direction::Vertical => cached.anchor_flags.vertical_anchor,
                    };
                    if !is_anchor {
                        continue;
                    }

                    // Seed the prefix search with whatever's already on
                    // the board immediately before this anchor (if this
                    // lane is extending an existing word rather than
                    // starting fresh) — the cursor tracks the *whole*
                    // word being built, not just the newly placed part.
                    let Some(cursor) =
                        seed_cursor(self.dictionary.root_cursor(), &state.board, start, direction)
                    else {
                        continue;
                    };

                    let mut remaining = *rack;
                    let mut placements = Vec::new();
                    let next =
                        start.try_step_forward(direction, self.rules.width, self.rules.height);

                    self.expand_lane(
                        &position,
                        state,
                        start,
                        direction,
                        start,
                        next,
                        0,
                        &mut remaining,
                        &mut placements,
                        max_tiles,
                        &mut moves,
                        &mut seen,
                        cursor,
                    );
                }
            }
        }

        moves
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_lane(
        &self,
        position: &crate::validate::RulesPosition<'_>,
        state: &GameState,
        start: Position,
        direction: Direction,
        current: Position,
        next: Option<Position>,
        offset: u8,
        remaining: &mut Rack,
        placements: &mut Vec<TilePlacement>,
        max_tiles: usize,
        moves: &mut Vec<MoveCandidate>,
        seen: &mut HashSet<String>,
        cursor: D::Cursor<'a>,
    ) {
        if !placements.is_empty() {
            let candidate = MoveCandidate {
                start,
                direction,
                tiles: placements.clone(),
            };
            if self.validate_move(position, &candidate).is_ok() {
                let key = move_candidate_key(&candidate);
                if seen.insert(key) {
                    moves.push(candidate);
                }
            }
        }

        if placements.len() >= max_tiles {
            return;
        }

        let cell = state.board.get(current);
        match cell {
            Some(BoardCell::Filled(filled)) => {
                // Only one possible "choice" here (whatever letter is
                // already on the board) — just track it and keep going.
                // If it somehow doesn't extend any real word (shouldn't
                // happen: only fully-valid words ever get placed), that's
                // a legitimate dead end for this lane, not a bug.
                let Some(cursor) = cursor.advance(filled.letter) else {
                    return;
                };
                if let Some(next_pos) = next {
                    self.expand_lane(
                        position,
                        state,
                        start,
                        direction,
                        next_pos,
                        next_pos.try_step_forward(direction, self.rules.width, self.rules.height),
                        offset + 1,
                        remaining,
                        placements,
                        max_tiles,
                        moves,
                        seen,
                        cursor,
                    );
                }
            }
            Some(BoardCell::Empty(_)) => {
                let cached = &state.cache.cells[current.to_index(BoardState::WIDTH)];
                let cross_check = match direction {
                    Direction::Horizontal => cached.horizontal,
                    Direction::Vertical => cached.vertical,
                };

                for tile in available_tiles_for_crosscheck(remaining, cross_check) {
                    // The actual pruning win: skip a letter entirely (no
                    // recursion, no rack mutation) the moment it can't
                    // possibly continue toward any real word, rather than
                    // finding that out only after exploring everything
                    // beneath it.
                    let Some(letter) = tile.letter() else {
                        continue;
                    };
                    let Some(next_cursor) = cursor.advance(letter) else {
                        continue;
                    };
                    if !remaining.consume_tile(tile) {
                        continue;
                    }

                    placements.push(TilePlacement { tile, offset });
                    if let Some(next_pos) = next {
                        self.expand_lane(
                            position,
                            state,
                            start,
                            direction,
                            next_pos,
                            next_pos.try_step_forward(
                                direction,
                                self.rules.width,
                                self.rules.height,
                            ),
                            offset + 1,
                            remaining,
                            placements,
                            max_tiles,
                            moves,
                            seen,
                            next_cursor,
                        );
                    }
                    placements.pop();
                    put_tile_back(remaining, tile);
                }
            }
            None => {}
        }
    }
}

impl<D: Dictionary> MoveGenerator<GameState, Rack> for RulesEngine<'_, D> {
    type Iter = std::vec::IntoIter<MoveCandidate>;

    fn enumerate_legal_moves(&self, state: &GameState, rack: &Rack) -> Self::Iter {
        self.enumerate_legal_multi_tile_moves(state, rack)
            .into_iter()
    }
}

/// Walks backward from `pos` through any existing filled cells (i.e. this
/// lane is extending an already-placed word rather than starting fresh),
/// and replays those letters forward through `root` to get the correct
/// starting search position — the cursor needs to track the *whole* word,
/// not just the part still to be typed from `pos` onward.
fn seed_cursor<C: PrefixCursor>(
    root: C,
    board: &BoardState,
    pos: Position,
    direction: Direction,
) -> Option<C> {
    let mut letters = Vec::new();
    let mut current = pos;
    while let Some(prev) = current.try_step_backward(direction) {
        match board.filled_letter(prev) {
            Some((letter, _)) => {
                letters.push(letter);
                current = prev;
            }
            None => break,
        }
    }
    letters.reverse();

    let mut cursor = root;
    for letter in letters {
        cursor = cursor.advance(letter)?;
    }
    Some(cursor)
}

fn available_tiles_for_crosscheck(rack: &Rack, cross_check: CrossCheck) -> Vec<Tile> {
    let mut tiles = Vec::new();

    for i in 0..26 {
        let letter = Letter::from(i as u8);
        if rack.counts[i] > 0 && cross_check.allows(letter) {
            tiles.push(Tile::Letter(letter));
        }
    }

    if rack.blanks > 0 {
        for i in 0..26 {
            let letter = Letter::from(i as u8);
            if !cross_check.allows(letter) {
                continue;
            }
            tiles.push(Tile::Blank {
                acting_as: Some(letter),
            });
        }
    }

    tiles
}

fn put_tile_back(rack: &mut Rack, tile: Tile) {
    match tile {
        Tile::Letter(letter) => rack.add_letter(letter),
        Tile::Blank { acting_as: Some(_) } => rack.blanks += 1,
        Tile::Blank { acting_as: None } => {}
    }
}

fn move_candidate_key(candidate: &MoveCandidate) -> String {
    let dir = match candidate.direction {
        Direction::Horizontal => 'H',
        Direction::Vertical => 'V',
    };
    let mut key = format!("{}:{}:{}:", candidate.start.x, candidate.start.y, dir);

    for placement in &candidate.tiles {
        key.push_str(&placement.offset.to_string());
        key.push('=');
        match placement.tile {
            Tile::Letter(letter) => {
                key.push('L');
                key.push(letter.as_char());
            }
            Tile::Blank {
                acting_as: Some(letter),
            } => {
                key.push('B');
                key.push(letter.as_char());
            }
            Tile::Blank { acting_as: None } => key.push_str("B?"),
        }
        key.push(';');
    }

    key
}

#[cfg(test)]
mod tests {
    use super::MoveGenerator;
    use crate::dictionary::Dictionary;
    use crate::model::{Direction, Letter, Rack, Tile};
    use crate::validate::{GameState, RulesEngine};

    struct TinyDictionary {
        words: std::collections::HashSet<&'static str>,
    }

    impl TinyDictionary {
        fn new(words: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                words: words.into_iter().collect(),
            }
        }
    }

    impl Dictionary for TinyDictionary {
        fn is_word(&self, word: &str) -> bool {
            self.words.contains(word)
        }

        type Cursor<'a> = ();

        fn root_cursor(&self) -> Self::Cursor<'_> {}
    }

    fn sample_rules() -> crate::model::VariantRules {
        crate::model::VariantRules::official()
    }

    #[test]
    fn generator_emits_valid_single_tile_opening_moves() {
        let rules = sample_rules();
        let dictionary = TinyDictionary::new(["A"]);
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };

        let state = GameState::new(&rules, &dictionary);
        let mut rack = Rack::default();
        rack.add_letter(Letter::from('A'));

        let moves: Vec<_> = engine.enumerate_legal_moves(&state, &rack).collect();
        assert!(!moves.is_empty());

        for candidate in moves {
            assert!(
                engine
                    .validate_game_move(&state, Some(&rack), &candidate)
                    .is_ok()
            );
        }
    }

    #[test]
    fn generator_uses_blank_tiles_when_present() {
        let rules = sample_rules();
        let dictionary = TinyDictionary::new(["A"]);
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };

        let state = GameState::new(&rules, &dictionary);
        let rack = Rack {
            counts: [0; 26],
            blanks: 1,
        };

        let moves = engine.enumerate_legal_single_tile_moves(&state, &rack);
        assert!(moves.iter().any(|candidate| {
            candidate.direction == Direction::Horizontal
                && candidate.tiles.len() == 1
                && matches!(candidate.tiles[0].tile, Tile::Blank { acting_as: Some(letter) } if letter == Letter::from('A'))
        }));
    }

    #[test]
    fn generator_emits_multi_tile_opening_move() {
        let rules = sample_rules();
        let dictionary = TinyDictionary::new(["AT", "A", "T"]);
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };

        let state = GameState::new(&rules, &dictionary);
        let mut rack = Rack::default();
        rack.add_letter(Letter::from('A'));
        rack.add_letter(Letter::from('T'));

        let moves = engine.enumerate_legal_multi_tile_moves(&state, &rack);
        assert!(moves.iter().any(|candidate| {
            candidate.tiles.len() == 2
                && engine
                    .validate_game_move(&state, Some(&rack), candidate)
                    .map(|validated| validated.preview.main_word == "AT")
                    .unwrap_or(false)
        }));
    }
}
