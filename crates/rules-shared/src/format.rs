use crate::model::MoveError;

/// Short, player-facing text for a rejected move. Deliberately terse (one
/// line, no jargon) since the UI renders this in a fixed-height banner slot
/// that never displaces the rack. Shared by the server's real move-submission
/// path and the client's own local preview, so both give the same wording
/// for the same mistake.
pub fn format_move_error(error: &MoveError) -> String {
    match error {
        MoveError::InvalidWord(words) => {
            let verb = if words.len() == 1 { "is" } else { "are" };
            format!("{} {verb} not in the dictionary.", format_word_list(words))
        }
        MoveError::InvalidMove
        | MoveError::InvalidPosition
        | MoveError::InvalidDirection
        | MoveError::TilesDoNotFit
        | MoveError::TilesDoNotConnect => "Incorrect tile placement.".to_string(),
    }
}

/// English list join: "A" / "A and B" / "A, B and C".
fn format_word_list(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::format_move_error;
    use crate::model::MoveError;

    #[test]
    fn one_invalid_word_uses_singular_verb() {
        assert_eq!(
            format_move_error(&MoveError::InvalidWord(vec!["QX".to_string()])),
            "QX is not in the dictionary."
        );
    }

    #[test]
    fn two_invalid_words_are_joined_with_and() {
        assert_eq!(
            format_move_error(&MoveError::InvalidWord(vec![
                "QX".to_string(),
                "ZY".to_string()
            ])),
            "QX and ZY are not in the dictionary."
        );
    }

    #[test]
    fn three_or_more_invalid_words_use_a_comma_list() {
        assert_eq!(
            format_move_error(&MoveError::InvalidWord(vec![
                "QX".to_string(),
                "ZY".to_string(),
                "JJ".to_string()
            ])),
            "QX, ZY and JJ are not in the dictionary."
        );
    }

    #[test]
    fn structural_errors_all_read_as_incorrect_placement() {
        assert_eq!(
            format_move_error(&MoveError::TilesDoNotConnect),
            "Incorrect tile placement."
        );
        assert_eq!(
            format_move_error(&MoveError::InvalidPosition),
            "Incorrect tile placement."
        );
    }
}
