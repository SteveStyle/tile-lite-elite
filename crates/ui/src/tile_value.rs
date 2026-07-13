//! Point values shown on tile faces (letter + value, like a physical
//! Scrabble tile). Duplicated from `rules_shared::VariantRules::official()`'s
//! `letter_values` rather than fetched from the server — there's currently
//! only one ruleset in play; if a second variant is ever added, this should
//! come from the server instead of staying hardcoded here.
const LETTER_VALUES: [u8; 26] = [
    1, 3, 3, 2, 1, 4, 2, 4, 1, 8, 5, 1, 3, 1, 1, 3, 10, 1, 1, 1, 1, 4, 4, 8, 4, 10,
];

fn value_for_letter(letter: char) -> u8 {
    let index = letter.to_ascii_uppercase() as i32 - 'A' as i32;
    if (0..26).contains(&index) {
        LETTER_VALUES[index as usize]
    } else {
        0
    }
}

/// A blank scores 0 regardless of which letter it's standing in for —
/// true whether it's still unresolved or has already been assigned one.
pub fn tile_point_value(tile: &api::TileDto) -> u8 {
    match tile {
        api::TileDto::Letter { letter } => value_for_letter(*letter),
        api::TileDto::Blank { .. } => 0,
    }
}

/// For a permanently placed board cell, which carries only the resolved
/// display letter plus an `is_blank` flag rather than a full `TileDto`.
pub fn board_cell_point_value(letter: char, is_blank: bool) -> u8 {
    if is_blank { 0 } else { value_for_letter(letter) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::TileDto;

    #[test]
    fn common_letters_match_standard_scrabble_values() {
        assert_eq!(value_for_letter('A'), 1);
        assert_eq!(value_for_letter('e'), 1);
        assert_eq!(value_for_letter('Q'), 10);
        assert_eq!(value_for_letter('Z'), 10);
    }

    #[test]
    fn letter_tile_uses_its_letter_value() {
        assert_eq!(tile_point_value(&TileDto::Letter { letter: 'F' }), 4);
    }

    #[test]
    fn blank_tile_is_always_zero_even_once_resolved() {
        assert_eq!(tile_point_value(&TileDto::Blank { acting_as: None }), 0);
        assert_eq!(
            tile_point_value(&TileDto::Blank { acting_as: Some('Z') }),
            0
        );
    }

    #[test]
    fn board_cell_blank_is_zero_regardless_of_its_displayed_letter() {
        assert_eq!(board_cell_point_value('Z', true), 0);
        assert_eq!(board_cell_point_value('Z', false), 10);
    }
}
