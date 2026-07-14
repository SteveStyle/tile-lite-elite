//! Converts between the wire DTOs (`api::*`) and the shared rules engine's
//! own types (`rules_shared::*`), so the client can run the exact same move
//! validation/scoring the server does — instantly, with no network
//! round-trip — for the live move preview. Nothing here ever calls into
//! `rules_shared::generate` (the recursive move generator the AI opponent
//! uses); only `RulesEngine::validate_game_move`, which is a plain,
//! non-recursive dictionary lookup per word formed.

use api::{BoardCellDto, DirectionDto, MoveCandidateDto, PremiumDto, TileDto};

pub fn to_rules_direction(direction: DirectionDto) -> rules_shared::Direction {
    match direction {
        DirectionDto::Horizontal => rules_shared::Direction::Horizontal,
        DirectionDto::Vertical => rules_shared::Direction::Vertical,
    }
}

pub fn to_rules_premium(premium: &PremiumDto) -> rules_shared::Premium {
    match premium {
        PremiumDto::Blank => rules_shared::Premium::Blank,
        PremiumDto::DoubleLetter => rules_shared::Premium::DoubleLetter,
        PremiumDto::TripleLetter => rules_shared::Premium::TripleLetter,
        PremiumDto::DoubleWord => rules_shared::Premium::DoubleWord,
        PremiumDto::TripleWord => rules_shared::Premium::TripleWord,
    }
}

pub fn to_rules_tile(tile: &TileDto) -> rules_shared::Tile {
    match tile {
        TileDto::Letter { letter } => rules_shared::Tile::Letter(rules_shared::Letter::from(*letter)),
        TileDto::Blank { acting_as } => rules_shared::Tile::Blank {
            acting_as: acting_as.map(rules_shared::Letter::from),
        },
    }
}

pub fn to_rules_rack(rack: &api::RackDto) -> rules_shared::Rack {
    // The wire format stays 26-wide for now (see rules_shared's
    // MAX_ALPHABET_SIZE doc comment) — pad out to the internal Rack's
    // width, leaving the rest at zero.
    let mut counts = [0u8; rules_shared::MAX_ALPHABET_SIZE];
    counts[..26].copy_from_slice(&rack.counts);
    rules_shared::Rack {
        counts,
        blanks: rack.blanks,
    }
}

pub fn to_rules_candidate(candidate: &MoveCandidateDto) -> rules_shared::MoveCandidate {
    rules_shared::MoveCandidate {
        start: rules_shared::Position::new(candidate.start.x, candidate.start.y),
        direction: to_rules_direction(candidate.direction),
        tiles: candidate
            .tiles
            .iter()
            .map(|placement| rules_shared::TilePlacement {
                offset: placement.offset,
                tile: to_rules_tile(&placement.tile),
            })
            .collect(),
    }
}

/// Board width/height are always 15 for every game this app creates (see
/// `BOARD_WIDTH`/`BOARD_HEIGHT` in `app.rs`) — same assumption
/// `rules_shared::BoardState` itself hardcodes.
pub fn to_rules_board_state(board: &[BoardCellDto]) -> rules_shared::BoardState {
    let mut state = rules_shared::BoardState::default();
    for (index, cell) in board.iter().enumerate() {
        let pos = rules_shared::Position::new(
            (index % rules_shared::BoardState::WIDTH) as u8,
            (index / rules_shared::BoardState::WIDTH) as u8,
        );
        let rules_cell = match cell.letter {
            Some(letter) => rules_shared::BoardCell::Filled(rules_shared::FilledCell {
                letter: rules_shared::Letter::from(letter),
                is_blank: cell.is_blank,
            }),
            None => rules_shared::BoardCell::Empty(rules_shared::EmptyCell {
                premium: to_rules_premium(&cell.premium),
            }),
        };
        state.set(pos, rules_cell);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::PositionDto;

    #[test]
    fn direction_maps_one_to_one() {
        assert_eq!(
            to_rules_direction(DirectionDto::Horizontal),
            rules_shared::Direction::Horizontal
        );
        assert_eq!(
            to_rules_direction(DirectionDto::Vertical),
            rules_shared::Direction::Vertical
        );
    }

    #[test]
    fn letter_tile_converts_by_char() {
        let tile = to_rules_tile(&TileDto::Letter { letter: 'Q' });
        assert_eq!(tile, rules_shared::Tile::Letter(rules_shared::Letter::from('Q')));
    }

    #[test]
    fn unresolved_blank_stays_unresolved() {
        let tile = to_rules_tile(&TileDto::Blank { acting_as: None });
        assert_eq!(tile, rules_shared::Tile::Blank { acting_as: None });
    }

    #[test]
    fn resolved_blank_carries_its_chosen_letter() {
        let tile = to_rules_tile(&TileDto::Blank {
            acting_as: Some('Z'),
        });
        assert_eq!(
            tile,
            rules_shared::Tile::Blank {
                acting_as: Some(rules_shared::Letter::from('Z'))
            }
        );
    }

    #[test]
    fn rack_counts_and_blanks_carry_over_unchanged() {
        let mut counts = [0u8; 26];
        counts[0] = 2;
        let rack = to_rules_rack(&api::RackDto { counts, blanks: 1 });
        assert_eq!(rack.counts[..26].to_vec(), counts.to_vec());
        assert!(rack.counts[26..].iter().all(|&count| count == 0));
        assert_eq!(rack.blanks, 1);
    }

    #[test]
    fn candidate_offsets_and_start_position_survive_conversion() {
        let candidate = MoveCandidateDto {
            start: PositionDto { x: 7, y: 7 },
            direction: DirectionDto::Horizontal,
            tiles: vec![
                api::TilePlacementDto {
                    offset: 0,
                    tile: TileDto::Letter { letter: 'A' },
                },
                api::TilePlacementDto {
                    offset: 1,
                    tile: TileDto::Letter { letter: 'T' },
                },
            ],
        };
        let rules_candidate = to_rules_candidate(&candidate);
        assert_eq!(rules_candidate.start, rules_shared::Position::new(7, 7));
        assert_eq!(rules_candidate.direction, rules_shared::Direction::Horizontal);
        assert_eq!(rules_candidate.tiles.len(), 2);
        assert_eq!(rules_candidate.tiles[1].offset, 1);
    }

    #[test]
    fn board_state_carries_letters_and_premiums_at_the_right_index() {
        let mut board = vec![
            BoardCellDto {
                premium: PremiumDto::Blank,
                letter: None,
                is_blank: false,
            };
            225
        ];
        board[0].premium = PremiumDto::TripleWord;
        board[112].letter = Some('A');
        board[112].is_blank = true;

        let state = to_rules_board_state(&board);
        assert_eq!(
            state.get(rules_shared::Position::new(0, 0)),
            Some(&rules_shared::BoardCell::Empty(rules_shared::EmptyCell {
                premium: rules_shared::Premium::TripleWord
            }))
        );
        assert_eq!(
            state.get(rules_shared::Position::new(7, 7)),
            Some(&rules_shared::BoardCell::Filled(rules_shared::FilledCell {
                letter: rules_shared::Letter::from('A'),
                is_blank: true,
            }))
        );
    }
}
