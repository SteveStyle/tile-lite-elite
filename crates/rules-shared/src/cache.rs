use crate::board::{BoardCell, BoardState};
use crate::dictionary::Dictionary;
use crate::model::{
    mask_contains, mask_insert, Direction, Letter, LetterMask, Position, Score, VariantRules,
    ALPHABET,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleCache {
    pub cells: [CachedCell; 225],
    pub extents: LineExtents,
}

impl Default for RuleCache {
    fn default() -> Self {
        Self {
            cells: [CachedCell::default(); 225],
            extents: LineExtents::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedCell {
    pub horizontal: CrossCheck,
    pub vertical: CrossCheck,
    pub anchor_flags: AnchorFlags,
}

impl Default for CachedCell {
    fn default() -> Self {
        Self {
            horizontal: CrossCheck::default(),
            vertical: CrossCheck::default(),
            anchor_flags: AnchorFlags::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossCheck {
    Unconstrained,
    Constrained(ConstrainedCrossCheck),
}

impl Default for CrossCheck {
    fn default() -> Self {
        Self::Unconstrained
    }
}

impl CrossCheck {
    pub fn allows(self, letter: Letter) -> bool {
        match self {
            CrossCheck::Unconstrained => true,
            CrossCheck::Constrained(check) => mask_contains(check.allowed_mask, letter),
        }
    }

    pub fn perpendicular_score(self, letter: Letter) -> Score {
        match self {
            CrossCheck::Unconstrained => 0,
            CrossCheck::Constrained(check) => check.score_by_letter[letter.as_usize()],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstrainedCrossCheck {
    pub allowed_mask: LetterMask,
    pub score_by_letter: [Score; 26],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnchorFlags {
    pub horizontal_anchor: bool,
    pub vertical_anchor: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineExtents {
    pub row_left: [u8; 225],
    pub row_right: [u8; 225],
    pub col_top: [u8; 225],
    pub col_bottom: [u8; 225],
}

impl Default for LineExtents {
    fn default() -> Self {
        Self {
            row_left: [0; 225],
            row_right: [0; 225],
            col_top: [0; 225],
            col_bottom: [0; 225],
        }
    }
}

impl RuleCache {
    pub fn recompute_all<D: Dictionary>(
        &mut self,
        board: &BoardState,
        rules: &VariantRules,
        dictionary: &D,
    ) {
        self.recompute_extents(board, rules);
        self.recompute_anchor_flags(board, rules);

        for y in 0..rules.height {
            for x in 0..rules.width {
                let pos = Position::new(x, y);
                if matches!(board.get(pos), Some(BoardCell::Empty(_))) {
                    self.recompute_cross_check(
                        board,
                        pos,
                        Direction::Horizontal,
                        rules,
                        dictionary,
                    );
                    self.recompute_cross_check(board, pos, Direction::Vertical, rules, dictionary);
                }
            }
        }
    }

    pub fn recompute_cross_check<D: Dictionary>(
        &mut self,
        board: &BoardState,
        pos: Position,
        placement_direction: Direction,
        rules: &VariantRules,
        dictionary: &D,
    ) {
        let cross_check = compute_cross_check(board, pos, placement_direction, rules, dictionary);
        let index = pos.to_index(BoardState::WIDTH);
        match placement_direction {
            Direction::Horizontal => self.cells[index].horizontal = cross_check,
            Direction::Vertical => self.cells[index].vertical = cross_check,
        }
    }

    pub fn recompute_extents(&mut self, board: &BoardState, rules: &VariantRules) {
        for y in 0..rules.height {
            for x in 0..rules.width {
                let pos = Position::new(x, y);
                let index = pos.to_index(BoardState::WIDTH);

                self.extents.row_left[index] = find_extent(board, pos, Direction::Horizontal, true);
                self.extents.row_right[index] =
                    find_extent(board, pos, Direction::Horizontal, false);
                self.extents.col_top[index] = find_extent(board, pos, Direction::Vertical, true);
                self.extents.col_bottom[index] =
                    find_extent(board, pos, Direction::Vertical, false);
            }
        }
    }

    pub fn recompute_anchor_flags(&mut self, board: &BoardState, rules: &VariantRules) {
        let has_any_tiles = board_has_any_tiles(board, rules);

        for y in 0..rules.height {
            for x in 0..rules.width {
                let pos = Position::new(x, y);
                let index = pos.to_index(BoardState::WIDTH);

                self.cells[index].anchor_flags =
                    if matches!(board.get(pos), Some(BoardCell::Empty(_))) {
                        if !has_any_tiles {
                            AnchorFlags {
                                horizontal_anchor: pos
                                    == Position::new(rules.width / 2, rules.height / 2),
                                vertical_anchor: pos
                                    == Position::new(rules.width / 2, rules.height / 2),
                            }
                        } else {
                            let touching = touches_filled_neighbor(board, pos, rules);
                            AnchorFlags {
                                horizontal_anchor: touching,
                                vertical_anchor: touching,
                            }
                        }
                    } else {
                        AnchorFlags::default()
                    };
            }
        }
    }
}

fn find_extent(board: &BoardState, pos: Position, direction: Direction, backward: bool) -> u8 {
    let mut current = pos;

    loop {
        let next = if backward {
            current.try_step_backward(direction)
        } else {
            current.try_step_forward(direction, BoardState::WIDTH as u8, BoardState::HEIGHT as u8)
        };

        let Some(next) = next else {
            break;
        };

        match board.get(next) {
            Some(BoardCell::Filled(_)) => current = next,
            _ => break,
        }
    }

    match direction {
        Direction::Horizontal => current.x,
        Direction::Vertical => current.y,
    }
}

fn board_has_any_tiles(board: &BoardState, rules: &VariantRules) -> bool {
    for y in 0..rules.height {
        for x in 0..rules.width {
            if matches!(board.get(Position::new(x, y)), Some(BoardCell::Filled(_))) {
                return true;
            }
        }
    }
    false
}

fn touches_filled_neighbor(board: &BoardState, pos: Position, rules: &VariantRules) -> bool {
    for direction in [Direction::Horizontal, Direction::Vertical] {
        if let Some(next) = pos.try_step_backward(direction) {
            if matches!(board.get(next), Some(BoardCell::Filled(_))) {
                return true;
            }
        }

        if let Some(next) = pos.try_step_forward(direction, rules.width, rules.height) {
            if matches!(board.get(next), Some(BoardCell::Filled(_))) {
                return true;
            }
        }
    }

    false
}

pub fn compute_cross_check<D: Dictionary>(
    board: &BoardState,
    pos: Position,
    placement_direction: Direction,
    rules: &VariantRules,
    dictionary: &D,
) -> CrossCheck {
    let Some(BoardCell::Empty(empty_cell)) = board.get(pos).copied() else {
        return CrossCheck::Unconstrained;
    };

    let perpendicular = -placement_direction;
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut surrounding_score: Score = 0;

    let mut current = pos;
    while let Some(next) = current.try_step_backward(perpendicular) {
        match board.filled_letter(next) {
            Some((letter, is_blank)) => {
                before.push(letter);
                if !is_blank {
                    surrounding_score += rules.letter_values[letter.as_usize()] as Score;
                }
                current = next;
            }
            None => break,
        }
    }

    current = pos;
    while let Some(next) = current.try_step_forward(perpendicular, rules.width, rules.height) {
        match board.filled_letter(next) {
            Some((letter, is_blank)) => {
                after.push(letter);
                if !is_blank {
                    surrounding_score += rules.letter_values[letter.as_usize()] as Score;
                }
                current = next;
            }
            None => break,
        }
    }

    if before.is_empty() && after.is_empty() {
        return CrossCheck::Unconstrained;
    }

    let mut allowed_mask = 0;
    let mut score_by_letter = [0; 26];

    for letter in ALPHABET {
        let mut word = String::with_capacity(before.len() + 1 + after.len());
        for existing in before.iter().rev() {
            word.push(existing.as_char());
        }
        word.push(letter.as_char());
        for existing in &after {
            word.push(existing.as_char());
        }

        if dictionary.is_word(&word) {
            mask_insert(&mut allowed_mask, letter);
            let central_score = (rules.letter_values[letter.as_usize()] as Score)
                * empty_cell.premium.letter_multiplier() as Score;
            score_by_letter[letter.as_usize()] =
                (surrounding_score + central_score) * empty_cell.premium.word_multiplier() as Score;
        }
    }

    CrossCheck::Constrained(ConstrainedCrossCheck {
        allowed_mask,
        score_by_letter,
    })
}

#[cfg(test)]
mod tests {
    use super::{compute_cross_check, CrossCheck, RuleCache};
    use crate::board::{BoardCell, BoardState, EmptyCell, FilledCell};
    use crate::dictionary::SowpodsDictionary;
    use crate::model::{Direction, Letter, Position, Premium, VariantRules};

    fn sample_rules() -> VariantRules {
        VariantRules::official()
    }

    #[test]
    fn unconstrained_when_no_perpendicular_neighbors() {
        let board = BoardState::default();
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
        let pos = Position::new(7, 7);

        let cross_check =
            compute_cross_check(&board, pos, Direction::Horizontal, &rules, &dictionary);

        assert!(matches!(cross_check, CrossCheck::Unconstrained));
    }

    #[test]
    fn constrained_when_perpendicular_word_must_be_valid() {
        let mut board = BoardState::default();
        let rules = sample_rules();
        let dictionary = SowpodsDictionary::new();
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

        let cross_check =
            compute_cross_check(&board, pos, Direction::Horizontal, &rules, &dictionary);

        match cross_check {
            CrossCheck::Constrained(check) => {
                assert!(super::CrossCheck::Constrained(check).allows(Letter::from('A')));
                assert!(!super::CrossCheck::Constrained(check).allows(Letter::from('Z')));
                assert_eq!(check.score_by_letter[Letter::from('A').as_usize()], 5);
            }
            CrossCheck::Unconstrained => panic!("expected constrained cross-check"),
        }
    }

    #[test]
    fn center_is_anchor_on_first_move() {
        let board = BoardState::default();
        let rules = sample_rules();
        let mut cache = RuleCache::default();

        cache.recompute_anchor_flags(&board, &rules);

        let center = cache.cells[Position::new(7, 7).to_index(BoardState::WIDTH)].anchor_flags;
        let corner = cache.cells[Position::new(0, 0).to_index(BoardState::WIDTH)].anchor_flags;

        assert!(center.horizontal_anchor);
        assert!(center.vertical_anchor);
        assert!(!corner.horizontal_anchor);
        assert!(!corner.vertical_anchor);
    }

    #[test]
    fn touching_cell_becomes_anchor_after_placement() {
        let mut board = BoardState::default();
        let rules = sample_rules();
        let mut cache = RuleCache::default();

        board.set(
            Position::new(7, 7),
            BoardCell::Filled(FilledCell {
                letter: Letter::from('A'),
                is_blank: false,
            }),
        );

        cache.recompute_anchor_flags(&board, &rules);

        let anchor = cache.cells[Position::new(7, 6).to_index(BoardState::WIDTH)].anchor_flags;
        assert!(anchor.horizontal_anchor);
        assert!(anchor.vertical_anchor);
    }
}
