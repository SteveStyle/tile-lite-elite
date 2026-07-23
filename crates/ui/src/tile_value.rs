//! Point values shown on tile faces (letter + value, like a physical
//! Scrabble tile) — sourced from the active game's own resolved
//! `VariantRules.letter_values`/`.alphabet` (passed in by the caller),
//! since different editions genuinely have different values (Wordfeud,
//! German, ...), not just one shared table.

fn value_for_letter(
    letter: &str,
    letter_values: &[u8; rules_shared::MAX_ALPHABET_SIZE],
    alphabet: &rules_shared::Alphabet,
) -> u8 {
    let upper = letter.to_uppercase();
    match alphabet.to_letter(&upper) {
        Some(letter) => letter_values[letter.as_usize()],
        None => 0,
    }
}

/// A blank scores 0 regardless of which letter it's standing in for —
/// true whether it's still unresolved or has already been assigned one.
pub fn tile_point_value(
    tile: &api::TileDto,
    letter_values: &[u8; rules_shared::MAX_ALPHABET_SIZE],
    alphabet: &rules_shared::Alphabet,
) -> u8 {
    match tile {
        api::TileDto::Letter { letter } => value_for_letter(letter, letter_values, alphabet),
        api::TileDto::Blank { .. } => 0,
    }
}

/// For a permanently placed board cell, which carries only the resolved
/// display letter plus an `is_blank` flag rather than a full `TileDto`.
pub fn board_cell_point_value(
    letter: &str,
    is_blank: bool,
    letter_values: &[u8; rules_shared::MAX_ALPHABET_SIZE],
    alphabet: &rules_shared::Alphabet,
) -> u8 {
    if is_blank {
        0
    } else {
        value_for_letter(letter, letter_values, alphabet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::TileDto;

    fn official_values() -> [u8; rules_shared::MAX_ALPHABET_SIZE] {
        rules_shared::VariantRules::official().letter_values
    }

    fn latin26() -> rules_shared::Alphabet {
        rules_shared::Alphabet::latin26()
    }

    #[test]
    fn common_letters_match_standard_scrabble_values() {
        let values = official_values();
        let alphabet = latin26();
        assert_eq!(value_for_letter("A", &values, &alphabet), 1);
        assert_eq!(value_for_letter("e", &values, &alphabet), 1);
        assert_eq!(value_for_letter("Q", &values, &alphabet), 10);
        assert_eq!(value_for_letter("Z", &values, &alphabet), 10);
    }

    #[test]
    fn letter_tile_uses_its_letter_value() {
        assert_eq!(
            tile_point_value(
                &TileDto::Letter {
                    letter: "F".to_string()
                },
                &official_values(),
                &latin26()
            ),
            4
        );
    }

    #[test]
    fn blank_tile_is_always_zero_even_once_resolved() {
        assert_eq!(
            tile_point_value(
                &TileDto::Blank { acting_as: None },
                &official_values(),
                &latin26()
            ),
            0
        );
        assert_eq!(
            tile_point_value(
                &TileDto::Blank {
                    acting_as: Some("Z".to_string())
                },
                &official_values(),
                &latin26()
            ),
            0
        );
    }

    #[test]
    fn board_cell_blank_is_zero_regardless_of_its_displayed_letter() {
        assert_eq!(
            board_cell_point_value("Z", true, &official_values(), &latin26()),
            0
        );
        assert_eq!(
            board_cell_point_value("Z", false, &official_values(), &latin26()),
            10
        );
    }

    #[test]
    fn german_umlaut_uses_german_values_not_zero() {
        let rules = rules_shared::VariantRules::german();
        assert_eq!(
            value_for_letter("Ö", &rules.letter_values, &rules.alphabet),
            8
        );
    }

    #[test]
    fn spanish_digraph_tile_uses_its_own_value_not_zero() {
        let rules = rules_shared::VariantRules::spanish();
        assert_eq!(
            value_for_letter("RR", &rules.letter_values, &rules.alphabet),
            8
        );
    }
}
