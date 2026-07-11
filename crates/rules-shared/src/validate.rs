use std::collections::BTreeMap;

use crate::board::{BoardCell, BoardState};
use crate::cache::RuleCache;
use crate::dictionary::Dictionary;
use crate::model::{
    CrossWordPreview, Direction, MoveCandidate, MoveError, MovePreview, MoveScore, Position, Rack,
    Score, Tile, TilePlacement, ValidatedMove, VariantRules,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameState {
    pub board: BoardState,
    pub cache: RuleCache,
}

impl Default for GameState {
    fn default() -> Self {
        Self {
            board: BoardState::default(),
            cache: RuleCache::default(),
        }
    }
}

impl GameState {
    pub fn new<D: Dictionary>(rules: &VariantRules, dictionary: &D) -> Self {
        Self::from_board(BoardState::new(rules), rules, dictionary)
    }

    pub fn from_board<D: Dictionary>(
        board: BoardState,
        rules: &VariantRules,
        dictionary: &D,
    ) -> Self {
        let mut cache = RuleCache::default();
        cache.recompute_all(&board, rules, dictionary);
        Self { board, cache }
    }

    pub fn position(&self) -> RulesPosition<'_> {
        RulesPosition {
            board: &self.board,
            cache: &self.cache,
            rack: None,
        }
    }

    pub fn position_with_rack<'a>(&'a self, rack: &'a Rack) -> RulesPosition<'a> {
        RulesPosition {
            board: &self.board,
            cache: &self.cache,
            rack: Some(rack),
        }
    }
}

pub struct RulesPosition<'a> {
    pub board: &'a BoardState,
    pub cache: &'a RuleCache,
    pub rack: Option<&'a Rack>,
}

pub struct RulesEngine<'a, D> {
    pub rules: &'a VariantRules,
    pub dictionary: &'a D,
}

pub trait MoveValidator<State> {
    fn preview_move(&self, state: &State, candidate: &MoveCandidate) -> MovePreview;

    fn validate_move(
        &self,
        state: &State,
        candidate: &MoveCandidate,
    ) -> Result<ValidatedMove, MoveError>;
}

impl<D: Dictionary> RulesEngine<'_, D> {
    pub fn preview_game_move(
        &self,
        state: &GameState,
        rack: Option<&Rack>,
        candidate: &MoveCandidate,
    ) -> MovePreview {
        let position = RulesPosition {
            board: &state.board,
            cache: &state.cache,
            rack,
        };
        self.preview_move(&position, candidate)
    }

    pub fn validate_game_move(
        &self,
        state: &GameState,
        rack: Option<&Rack>,
        candidate: &MoveCandidate,
    ) -> Result<ValidatedMove, MoveError> {
        let position = RulesPosition {
            board: &state.board,
            cache: &state.cache,
            rack,
        };
        self.validate_move(&position, candidate)
    }

    pub fn preview_move(
        &self,
        state: &RulesPosition<'_>,
        candidate: &MoveCandidate,
    ) -> MovePreview {
        match self.validate_move(state, candidate) {
            Ok(validated) => validated.preview,
            Err(error) => MovePreview {
                legal: false,
                main_word: String::new(),
                total_score: 0,
                cross_words: Vec::new(),
                error: Some(error),
            },
        }
    }

    pub fn validate_move(
        &self,
        state: &RulesPosition<'_>,
        candidate: &MoveCandidate,
    ) -> Result<ValidatedMove, MoveError> {
        let offset_placements = normalize_placements(candidate)?;
        if offset_placements.is_empty() {
            return Err(MoveError::InvalidMove);
        }
        if offset_placements.len() > self.rules.rack_size as usize {
            return Err(MoveError::TilesDoNotFit);
        }

        if let Some(rack) = state.rack {
            validate_rack_usage(
                rack,
                offset_placements.values().map(|placement| placement.tile),
            )?;
        }

        let placements = candidate_placements(candidate, &offset_placements, self.rules)?;
        let span = main_word_span(candidate, &offset_placements, self.rules)?;
        let placed_positions: Vec<Position> = placements.keys().copied().collect();

        for pos in &placed_positions {
            if !matches!(state.board.get(*pos), Some(BoardCell::Empty(_))) {
                return Err(MoveError::InvalidPosition);
            }
        }

        let mut connects = false;
        for pos in &placed_positions {
            let cached = &state.cache.cells[pos.to_index(BoardState::WIDTH)];
            let anchor = match candidate.direction {
                Direction::Horizontal => cached.anchor_flags.horizontal_anchor,
                Direction::Vertical => cached.anchor_flags.vertical_anchor,
            };
            connects |= anchor;
        }
        if !connects {
            return Err(MoveError::TilesDoNotConnect);
        }

        validate_main_word_span(state.board, candidate, &placements, span, self.rules)?;

        let word_start = extend_start(state.board, placed_positions[0], candidate.direction);
        let word_end = extend_end(
            state.board,
            *placed_positions.last().ok_or(MoveError::InvalidMove)?,
            candidate.direction,
            self.rules,
        );

        let mut word = String::new();
        let mut cross_words = Vec::new();
        let mut invalid_words = Vec::new();
        let mut main_word_score: Score = 0;
        let mut cross_word_score: Score = 0;
        let mut word_multiplier: Score = 1;
        let mut saw_placement = false;

        let mut current = word_start;
        loop {
            match state.board.get(current) {
                Some(BoardCell::Filled(cell)) => {
                    word.push(cell.letter.as_char());
                    if !cell.is_blank {
                        main_word_score +=
                            self.rules.letter_values[cell.letter.as_usize()] as Score;
                    }
                }
                Some(BoardCell::Empty(empty_cell)) => {
                    let placement = placements.get(&current).ok_or(MoveError::InvalidMove)?;
                    let letter = placement.tile.letter().ok_or(MoveError::InvalidMove)?;
                    let cached = &state.cache.cells[current.to_index(BoardState::WIDTH)];
                    let cross_check = match candidate.direction {
                        Direction::Horizontal => cached.horizontal,
                        Direction::Vertical => cached.vertical,
                    };
                    // Don't bail out on the first bad cross word: keep
                    // walking so every simultaneously-invalid word (the
                    // main word and any other cross words) gets collected
                    // and named too, instead of only ever reporting
                    // whichever one happens to be checked first.
                    if !cross_check.allows(letter) {
                        invalid_words.push(build_cross_word(
                            state.board,
                            current,
                            candidate.direction,
                            letter,
                            self.rules,
                        ));
                    }

                    saw_placement = true;
                    word.push(letter.as_char());
                    let tile_score = if matches!(placement.tile, crate::model::Tile::Blank { .. }) {
                        0
                    } else {
                        self.rules.letter_values[letter.as_usize()] as Score
                    };
                    main_word_score += tile_score * empty_cell.premium.letter_multiplier() as Score;
                    word_multiplier *= empty_cell.premium.word_multiplier() as Score;

                    let perpendicular_score = cross_check.perpendicular_score(letter);
                    if perpendicular_score > 0 {
                        cross_word_score += perpendicular_score;
                        cross_words.push(CrossWordPreview {
                            pos: current,
                            word: build_cross_word(
                                state.board,
                                current,
                                candidate.direction,
                                letter,
                                self.rules,
                            ),
                            score: perpendicular_score,
                        });
                    }
                }
                None => return Err(MoveError::InvalidPosition),
            }

            if current == word_end {
                break;
            }

            current = current
                .try_step_forward(candidate.direction, self.rules.width, self.rules.height)
                .ok_or(MoveError::InvalidPosition)?;
        }

        if !saw_placement || !self.dictionary.is_word(&word) {
            invalid_words.insert(0, word.clone());
        }
        if !invalid_words.is_empty() {
            return Err(MoveError::InvalidWord(invalid_words));
        }

        let bingo_bonus = if placements.len() == self.rules.rack_size as usize {
            self.rules.bingo_bonus
        } else {
            0
        };
        let total = main_word_score * word_multiplier + cross_word_score + bingo_bonus;
        let preview = MovePreview {
            legal: true,
            main_word: word,
            total_score: total,
            cross_words,
            error: None,
        };
        let score = MoveScore {
            total,
            main_word_score: main_word_score * word_multiplier,
            cross_word_score,
            bingo_bonus,
        };

        Ok(ValidatedMove {
            candidate: candidate.clone(),
            preview,
            score,
        })
    }

    pub fn apply_move(
        &self,
        board: &mut BoardState,
        cache: &mut RuleCache,
        validated: &ValidatedMove,
    ) -> Result<(), MoveError> {
        let offset_placements = normalize_placements(&validated.candidate)?;
        let placements =
            candidate_placements(&validated.candidate, &offset_placements, self.rules)?;

        for (pos, placement) in placements {
            let letter = placement.tile.letter().ok_or(MoveError::InvalidMove)?;
            board.set(
                pos,
                BoardCell::Filled(crate::board::FilledCell {
                    letter,
                    is_blank: matches!(placement.tile, Tile::Blank { .. }),
                }),
            );
        }

        cache.recompute_extents(board, self.rules);
        cache.recompute_anchor_flags(board, self.rules);

        for y in 0..self.rules.height {
            for x in 0..self.rules.width {
                let pos = Position::new(x, y);
                if matches!(board.get(pos), Some(BoardCell::Empty(_))) {
                    cache.recompute_cross_check(
                        board,
                        pos,
                        Direction::Horizontal,
                        self.rules,
                        self.dictionary,
                    );
                    cache.recompute_cross_check(
                        board,
                        pos,
                        Direction::Vertical,
                        self.rules,
                        self.dictionary,
                    );
                }
            }
        }

        Ok(())
    }

    pub fn apply_move_to_game(
        &self,
        state: &mut GameState,
        validated: &ValidatedMove,
    ) -> Result<(), MoveError> {
        self.apply_move(&mut state.board, &mut state.cache, validated)
    }
}

impl<D: Dictionary> MoveValidator<RulesPosition<'_>> for RulesEngine<'_, D> {
    fn preview_move(&self, state: &RulesPosition<'_>, candidate: &MoveCandidate) -> MovePreview {
        RulesEngine::preview_move(self, state, candidate)
    }

    fn validate_move(
        &self,
        state: &RulesPosition<'_>,
        candidate: &MoveCandidate,
    ) -> Result<ValidatedMove, MoveError> {
        RulesEngine::validate_move(self, state, candidate)
    }
}

fn validate_rack_usage(rack: &Rack, tiles: impl Iterator<Item = Tile>) -> Result<(), MoveError> {
    let mut remaining = *rack;
    for tile in tiles {
        if !remaining.consume_tile(tile) {
            return Err(MoveError::TilesDoNotFit);
        }
    }
    Ok(())
}

fn normalize_placements(
    candidate: &MoveCandidate,
) -> Result<BTreeMap<u8, TilePlacement>, MoveError> {
    let mut placements = BTreeMap::new();
    for placement in &candidate.tiles {
        if placements
            .insert(placement.offset, placement.clone())
            .is_some()
        {
            return Err(MoveError::InvalidMove);
        }
    }
    Ok(placements)
}

fn candidate_placements(
    candidate: &MoveCandidate,
    placements: &BTreeMap<u8, TilePlacement>,
    rules: &VariantRules,
) -> Result<BTreeMap<Position, TilePlacement>, MoveError> {
    let mut positioned = BTreeMap::new();
    for (offset, placement) in placements {
        let pos = offset_position(candidate.start, candidate.direction, *offset, rules)?;
        if positioned.insert(pos, placement.clone()).is_some() {
            return Err(MoveError::InvalidMove);
        }
    }
    Ok(positioned)
}

fn main_word_span(
    candidate: &MoveCandidate,
    placements: &BTreeMap<u8, TilePlacement>,
    rules: &VariantRules,
) -> Result<(u8, u8), MoveError> {
    let Some((&min_offset, _)) = placements.first_key_value() else {
        return Err(MoveError::InvalidMove);
    };
    let Some((&max_offset, _)) = placements.last_key_value() else {
        return Err(MoveError::InvalidMove);
    };

    offset_position(candidate.start, candidate.direction, min_offset, rules)?;
    offset_position(candidate.start, candidate.direction, max_offset, rules)?;

    Ok((min_offset, max_offset))
}

fn offset_position(
    start: Position,
    direction: Direction,
    offset: u8,
    rules: &VariantRules,
) -> Result<Position, MoveError> {
    match direction {
        Direction::Horizontal if start.x + offset < rules.width => {
            Ok(Position::new(start.x + offset, start.y))
        }
        Direction::Vertical if start.y + offset < rules.height => {
            Ok(Position::new(start.x, start.y + offset))
        }
        _ => Err(MoveError::InvalidPosition),
    }
}

fn validate_main_word_span(
    board: &BoardState,
    candidate: &MoveCandidate,
    placements: &BTreeMap<Position, TilePlacement>,
    span: (u8, u8),
    rules: &VariantRules,
) -> Result<(), MoveError> {
    for offset in span.0..=span.1 {
        let pos = offset_position(candidate.start, candidate.direction, offset, rules)?;
        if placements.contains_key(&pos) {
            continue;
        }

        if !matches!(board.get(pos), Some(BoardCell::Filled(_))) {
            return Err(MoveError::InvalidMove);
        }
    }

    Ok(())
}

fn extend_start(board: &BoardState, pos: Position, direction: Direction) -> Position {
    let mut current = pos;
    while let Some(next) = current.try_step_backward(direction) {
        if matches!(board.get(next), Some(BoardCell::Filled(_))) {
            current = next;
        } else {
            break;
        }
    }
    current
}

fn extend_end(
    board: &BoardState,
    pos: Position,
    direction: Direction,
    rules: &VariantRules,
) -> Position {
    let mut current = pos;
    while let Some(next) = current.try_step_forward(direction, rules.width, rules.height) {
        if matches!(board.get(next), Some(BoardCell::Filled(_))) {
            current = next;
        } else {
            break;
        }
    }
    current
}

fn build_cross_word(
    board: &BoardState,
    pos: Position,
    main_direction: Direction,
    placed_letter: crate::model::Letter,
    rules: &VariantRules,
) -> String {
    let perpendicular = -main_direction;
    let mut before = Vec::new();
    let mut current = pos;
    while let Some(next) = current.try_step_backward(perpendicular) {
        match board.filled_letter(next) {
            Some((letter, _)) => {
                before.push(letter);
                current = next;
            }
            None => break,
        }
    }

    let mut word = String::with_capacity(8);
    for letter in before.iter().rev() {
        word.push(letter.as_char());
    }
    word.push(placed_letter.as_char());

    current = pos;
    while let Some(next) = current.try_step_forward(perpendicular, rules.width, rules.height) {
        match board.filled_letter(next) {
            Some((letter, _)) => {
                word.push(letter.as_char());
                current = next;
            }
            None => break,
        }
    }
    word
}

#[cfg(test)]
mod tests {
    use super::{GameState, RulesEngine, RulesPosition};
    use crate::board::{BoardCell, BoardState, EmptyCell, FilledCell};
    use crate::cache::RuleCache;
    use crate::dictionary::SowpodsDictionary;
    use crate::model::{
        Direction, Letter, MoveCandidate, MoveError, Position, Premium, Rack, Tile,
        TilePlacement, VariantRules,
    };

    fn sample_rules() -> VariantRules {
        VariantRules::official()
    }

    #[test]
    fn preview_validates_simple_anchor_play() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let board = BoardState::default();
        let mut cache = RuleCache::default();
        cache.recompute_anchor_flags(&board, &rules);

        let state = RulesPosition {
            board: &board,
            cache: &cache,
            rack: None,
        };
        let candidate = MoveCandidate {
            start: Position::new(7, 7),
            direction: Direction::Horizontal,
            tiles: vec![
                TilePlacement {
                    offset: 0,
                    tile: Tile::Letter(Letter::from('A')),
                },
                TilePlacement {
                    offset: 1,
                    tile: Tile::Letter(Letter::from('T')),
                },
            ],
        };

        let preview = engine.preview_move(&state, &candidate);
        assert!(preview.legal);
        assert_eq!(preview.main_word, "AT");
    }

    #[test]
    fn preview_rejects_disallowed_cross_letter() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let mut board = BoardState::default();
        let mut cache = RuleCache::default();
        let pos = Position::new(7, 7);

        board.set(
            Position::new(7, 6),
            BoardCell::Filled(FilledCell {
                letter: Letter::from('C'),
                is_blank: false,
            }),
        );
        board.set(
            Position::new(7, 8),
            BoardCell::Filled(FilledCell {
                letter: Letter::from('T'),
                is_blank: false,
            }),
        );
        board.set(
            pos,
            BoardCell::Empty(EmptyCell {
                premium: Premium::Blank,
            }),
        );
        cache.recompute_anchor_flags(&board, &rules);
        cache.recompute_cross_check(&board, pos, Direction::Horizontal, &rules, &dictionary);

        let state = RulesPosition {
            board: &board,
            cache: &cache,
            rack: None,
        };
        let candidate = MoveCandidate {
            start: pos,
            direction: Direction::Horizontal,
            tiles: vec![TilePlacement {
                offset: 0,
                tile: Tile::Letter(Letter::from('Z')),
            }],
        };

        let preview = engine.preview_move(&state, &candidate);
        assert!(!preview.legal);
    }

    /// Same board as `preview_rejects_disallowed_cross_letter`, but this
    /// placement is a *single* tile with no horizontal neighbors, so its
    /// own "main word" is just "Z" — also not a dictionary word. Validation
    /// used to short-circuit on the first bad word it found (the cross
    /// word "CZT"), silently never checking or reporting that the main
    /// word was independently invalid too. It should now name both.
    #[test]
    fn validate_move_names_every_simultaneously_invalid_word_not_just_the_first() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let mut board = BoardState::default();
        let mut cache = RuleCache::default();
        let pos = Position::new(7, 7);

        board.set(
            Position::new(7, 6),
            BoardCell::Filled(FilledCell {
                letter: Letter::from('C'),
                is_blank: false,
            }),
        );
        board.set(
            Position::new(7, 8),
            BoardCell::Filled(FilledCell {
                letter: Letter::from('T'),
                is_blank: false,
            }),
        );
        board.set(
            pos,
            BoardCell::Empty(EmptyCell {
                premium: Premium::Blank,
            }),
        );
        cache.recompute_anchor_flags(&board, &rules);
        cache.recompute_cross_check(&board, pos, Direction::Horizontal, &rules, &dictionary);

        let state = RulesPosition {
            board: &board,
            cache: &cache,
            rack: None,
        };
        let candidate = MoveCandidate {
            start: pos,
            direction: Direction::Horizontal,
            tiles: vec![TilePlacement {
                offset: 0,
                tile: Tile::Letter(Letter::from('Z')),
            }],
        };

        let error = engine.validate_move(&state, &candidate).unwrap_err();
        assert_eq!(
            error,
            MoveError::InvalidWord(vec!["Z".to_string(), "CZT".to_string()])
        );
    }

    #[test]
    fn validate_move_names_the_main_word_when_it_is_the_only_problem() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let board = BoardState::default();
        let mut cache = RuleCache::default();
        cache.recompute_anchor_flags(&board, &rules);

        let state = RulesPosition {
            board: &board,
            cache: &cache,
            rack: None,
        };
        let candidate = MoveCandidate {
            start: Position::new(7, 7),
            direction: Direction::Horizontal,
            tiles: vec![
                TilePlacement {
                    offset: 0,
                    tile: Tile::Letter(Letter::from('Q')),
                },
                TilePlacement {
                    offset: 1,
                    tile: Tile::Letter(Letter::from('X')),
                },
            ],
        };

        let error = engine.validate_move(&state, &candidate).unwrap_err();
        assert_eq!(error, MoveError::InvalidWord(vec!["QX".to_string()]));
    }

    #[test]
    fn preview_rejects_tiles_not_in_rack() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let board = BoardState::default();
        let mut cache = RuleCache::default();
        cache.recompute_anchor_flags(&board, &rules);
        let rack = Rack::default();

        let state = RulesPosition {
            board: &board,
            cache: &cache,
            rack: Some(&rack),
        };
        let candidate = MoveCandidate {
            start: Position::new(7, 7),
            direction: Direction::Horizontal,
            tiles: vec![TilePlacement {
                offset: 0,
                tile: Tile::Letter(Letter::from('A')),
            }],
        };

        let preview = engine.preview_move(&state, &candidate);
        assert!(!preview.legal);
    }

    #[test]
    fn preview_awards_bingo_bonus_when_using_full_rack() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let board = BoardState::default();
        let mut cache = RuleCache::default();
        cache.recompute_anchor_flags(&board, &rules);

        let mut rack = Rack::default();
        for ch in ['S', 'E', 'A', 'T', 'I', 'N', 'G'] {
            rack.add_letter(Letter::from(ch));
        }

        let state = RulesPosition {
            board: &board,
            cache: &cache,
            rack: Some(&rack),
        };
        let candidate = MoveCandidate {
            start: Position::new(7, 7),
            direction: Direction::Horizontal,
            tiles: vec![
                TilePlacement {
                    offset: 0,
                    tile: Tile::Letter(Letter::from('S')),
                },
                TilePlacement {
                    offset: 1,
                    tile: Tile::Letter(Letter::from('E')),
                },
                TilePlacement {
                    offset: 2,
                    tile: Tile::Letter(Letter::from('A')),
                },
                TilePlacement {
                    offset: 3,
                    tile: Tile::Letter(Letter::from('T')),
                },
                TilePlacement {
                    offset: 4,
                    tile: Tile::Letter(Letter::from('I')),
                },
                TilePlacement {
                    offset: 5,
                    tile: Tile::Letter(Letter::from('N')),
                },
                TilePlacement {
                    offset: 6,
                    tile: Tile::Letter(Letter::from('G')),
                },
            ],
        };

        let preview = engine.preview_move(&state, &candidate);
        assert!(preview.legal);
        let validated = engine.validate_move(&state, &candidate).unwrap();
        assert_eq!(validated.score.bingo_bonus, 50);
    }

    #[test]
    fn apply_move_updates_board_and_cache() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let mut board = BoardState::default();
        let mut cache = RuleCache::default();
        cache.recompute_extents(&board, &rules);
        cache.recompute_anchor_flags(&board, &rules);

        let state = RulesPosition {
            board: &board,
            cache: &cache,
            rack: None,
        };
        let candidate = MoveCandidate {
            start: Position::new(7, 7),
            direction: Direction::Horizontal,
            tiles: vec![
                TilePlacement {
                    offset: 0,
                    tile: Tile::Letter(Letter::from('A')),
                },
                TilePlacement {
                    offset: 1,
                    tile: Tile::Letter(Letter::from('T')),
                },
            ],
        };

        let validated = engine.validate_move(&state, &candidate).unwrap();
        engine
            .apply_move(&mut board, &mut cache, &validated)
            .unwrap();

        assert!(matches!(
            board.get(Position::new(7, 7)),
            Some(BoardCell::Filled(_))
        ));
        assert!(matches!(
            board.get(Position::new(8, 7)),
            Some(BoardCell::Filled(_))
        ));

        let above_anchor =
            cache.cells[Position::new(7, 6).to_index(BoardState::WIDTH)].anchor_flags;
        assert!(above_anchor.horizontal_anchor);
        assert!(above_anchor.vertical_anchor);
    }

    #[test]
    fn game_state_initializes_cache_and_applies_move() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let mut state = GameState::new(&rules, &dictionary);

        let mut rack = Rack::default();
        rack.add_letter(Letter::from('A'));
        rack.add_letter(Letter::from('T'));

        let candidate = MoveCandidate {
            start: Position::new(7, 7),
            direction: Direction::Horizontal,
            tiles: vec![
                TilePlacement {
                    offset: 0,
                    tile: Tile::Letter(Letter::from('A')),
                },
                TilePlacement {
                    offset: 1,
                    tile: Tile::Letter(Letter::from('T')),
                },
            ],
        };

        let preview = engine.preview_game_move(&state, Some(&rack), &candidate);
        assert!(preview.legal);

        let validated = engine
            .validate_game_move(&state, Some(&rack), &candidate)
            .unwrap();
        engine.apply_move_to_game(&mut state, &validated).unwrap();

        assert!(matches!(
            state.board.get(Position::new(7, 7)),
            Some(BoardCell::Filled(_))
        ));
        let anchor =
            state.cache.cells[Position::new(7, 6).to_index(BoardState::WIDTH)].anchor_flags;
        assert!(anchor.horizontal_anchor);
    }

    #[test]
    fn preview_rejects_gap_without_existing_bridge() {
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let engine = RulesEngine {
            rules: &rules,
            dictionary: &dictionary,
        };
        let state = GameState::new(&rules, &dictionary);

        let mut rack = Rack::default();
        rack.add_letter(Letter::from('A'));
        rack.add_letter(Letter::from('T'));

        let candidate = MoveCandidate {
            start: Position::new(7, 7),
            direction: Direction::Horizontal,
            tiles: vec![
                TilePlacement {
                    offset: 0,
                    tile: Tile::Letter(Letter::from('A')),
                },
                TilePlacement {
                    offset: 2,
                    tile: Tile::Letter(Letter::from('T')),
                },
            ],
        };

        let preview = engine.preview_game_move(&state, Some(&rack), &candidate);
        assert!(!preview.legal);
        assert!(matches!(
            preview.error,
            Some(crate::model::MoveError::InvalidMove)
        ));
    }
}
