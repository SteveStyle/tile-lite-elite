use crate::board::BoardState;
use crate::dictionary::Dictionary;
use crate::model::{Direction, Letter, MoveCandidate, Rack, Tile, TilePlacement};
use crate::validate::{GameState, RulesEngine};

pub trait MoveGenerator<State, Rack> {
    type Iter: Iterator<Item = MoveCandidate>;

    fn enumerate_legal_moves(&self, state: &State, rack: &Rack) -> Self::Iter;
}

impl<D: Dictionary> RulesEngine<'_, D> {
    pub fn enumerate_legal_single_tile_moves(
        &self,
        state: &GameState,
        rack: &Rack,
    ) -> Vec<MoveCandidate> {
        let position = state.position_with_rack(rack);
        let mut moves = Vec::new();

        for y in 0..self.rules.height {
            for x in 0..self.rules.width {
                let start = crate::model::Position::new(x, y);
                if !matches!(
                    state.board.get(start),
                    Some(crate::board::BoardCell::Empty(_))
                ) {
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

                    for tile in unique_rack_tiles(rack) {
                        let candidate = MoveCandidate {
                            start,
                            direction,
                            tiles: vec![TilePlacement { offset: 0, tile }],
                        };

                        if self.validate_move(&position, &candidate).is_ok() {
                            moves.push(candidate);
                        }
                    }
                }
            }
        }

        moves
    }
}

impl<D: Dictionary> MoveGenerator<GameState, Rack> for RulesEngine<'_, D> {
    type Iter = std::vec::IntoIter<MoveCandidate>;

    fn enumerate_legal_moves(&self, state: &GameState, rack: &Rack) -> Self::Iter {
        self.enumerate_legal_single_tile_moves(state, rack)
            .into_iter()
    }
}

fn unique_rack_tiles(rack: &Rack) -> Vec<Tile> {
    let mut tiles = Vec::new();

    for i in 0..26 {
        if rack.counts[i] > 0 {
            tiles.push(Tile::Letter(Letter::from(i as u8)));
        }
    }

    if rack.blanks > 0 {
        for i in 0..26 {
            tiles.push(Tile::Blank {
                acting_as: Some(Letter::from(i as u8)),
            });
        }
    }

    tiles
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
}
