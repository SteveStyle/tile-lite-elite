use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;

use crate::player::Direction;
use crate::player::MoveError;
use crate::pos::Position;
use crate::tiles::ALPHABET;
use crate::tiles::Letter;

use crate::tiles::LetterSet;
use crate::tiles::Tile;
use crate::word_list::is_word;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CellValue {
    // The possible letters when playing horizontally or vertically, so that this cell can be filled and make a valid word
    // in the other direction
    Empty {
        horizontal_letters: LetterSet,
        vertical_letters: LetterSet,
    },
    Filled {
        letter: Letter,
        is_blank: bool,
        populated_last_move: bool,
    },
}

impl Display for CellValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty {
                horizontal_letters,
                vertical_letters,
            } => write!(
                f,
                "Empty {{ horizontal_letters: {}, vertical_letters: {} }}",
                horizontal_letters, vertical_letters
            ),
            Self::Filled { letter, .. } => write!(f, "Filled {{ letter: {} }}", letter),
        }
    }
}

impl CellValue {
    pub(crate) fn as_filled(&self) -> Option<Letter> {
        if let Self::Filled { letter, .. } = *self {
            Some(letter)
        } else {
            None
        }
    }

    pub fn is_vertically_connected(&self) -> bool {
        match self {
            Self::Empty {
                vertical_letters, ..
            } => !vertical_letters.is_empty(),
            Self::Filled { .. } => true,
        }
    }

    pub fn is_horizontally_connected(&self) -> bool {
        match self {
            Self::Empty {
                horizontal_letters, ..
            } => !horizontal_letters.is_empty(),
            Self::Filled { .. } => true,
        }
    }

    /// Returns `true` if the cell value is [`Empty`].
    ///
    /// [`Empty`]: CellValue::Empty
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        matches!(self, Self::Empty { .. })
    }

    pub(crate) fn set_letter_set(&mut self, direction: Direction, letter_set: LetterSet) {
        match self {
            Self::Empty {
                horizontal_letters,
                vertical_letters,
            } => match direction {
                Direction::Horizontal => *horizontal_letters = letter_set,
                Direction::Vertical => *vertical_letters = letter_set,
            },
            Self::Filled { .. } => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CellType {
    Blank,
    DoubleLetter,
    TripleLetter,
    DoubleWord,
    TripleWord,
}

impl From<char> for CellType {
    fn from(c: char) -> Self {
        match c {
            '2' => Self::DoubleLetter,
            '3' => Self::TripleLetter,
            'd' => Self::DoubleWord,
            't' => Self::TripleWord,
            _ => Self::Blank,
        }
    }
}

impl CellType {
    pub(crate) fn as_char(&self) -> char {
        match self {
            Self::Blank => ' ',
            Self::DoubleLetter => '2',
            Self::TripleLetter => '3',
            Self::DoubleWord => 'd',
            Self::TripleWord => 't',
        }
    }

    pub(crate) fn word_multiplier(&self) -> u8 {
        match self {
            Self::DoubleWord => 2,
            Self::TripleWord => 3,
            _ => 1,
        }
    }

    pub(crate) fn letter_multiplier(&self) -> u8 {
        match self {
            Self::DoubleLetter => 2,
            Self::TripleLetter => 3,
            _ => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub(crate) value: CellValue,
    pub(crate) cell_type: CellType,
}

impl Display for Cell {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.value {
            CellValue::Empty { .. } => write!(
                f,
                "{}{} {} {}",
                AnsiCodes::BOLDON,
                match self.cell_type {
                    CellType::DoubleWord | CellType::TripleWord => AnsiCodes::RED,
                    _ => TILECOLOUR,
                },
                self.cell_type.as_char(),
                AnsiCodes::BOLDOFF
            ),
            CellValue::Filled {
                letter,
                is_blank,
                populated_last_move,
            } => {
                write!(
                    f,
                    "{}{}{}{}{} {}",
                    if populated_last_move {
                        AnsiCodes::BOLDON
                    } else {
                        ""
                    },
                    if is_blank { BLANKCOLOUR } else { LETTERCOLOUR },
                    if is_blank { "*" } else { " " },
                    LETTERCOLOUR,
                    letter,
                    AnsiCodes::BOLDOFF
                )
            }
        }
    }
}

impl Debug for Cell {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.value {
            CellValue::Empty {
                horizontal_letters,
                vertical_letters,
            } => write!(
                f,
                "Horizontal Letters {}   Vertical Letters  {}",
                horizontal_letters, vertical_letters
            ),
            CellValue::Filled {
                letter, is_blank, ..
            } => {
                if is_blank {
                    write!(f, "{}{}", "*", letter)
                } else {
                    write!(
                        f,
                        "{} {}{} {}",
                        AnsiCodes::BOLDOFF,
                        LETTERCOLOUR,
                        letter,
                        AnsiCodes::BOLDOFF
                    )
                }
            }
        }
    }
}

impl Cell {
    pub(crate) fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub(crate) fn as_filled(&self) -> Option<Letter> {
        self.value.as_filled()
    }

    pub(crate) fn set_tile(&mut self, tile: Tile) {
        self.value = CellValue::Filled {
            letter: tile.letter().unwrap(),
            is_blank: tile.is_blank(),
            populated_last_move: true,
        };
    }
}

pub(crate) type BoardLayout = [(u8, u8, CellType); 18];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrabbleVariant {
    pub(crate) letter_values: [u8; 26],
    pub(crate) letter_distribution: [u8; 26],
    pub(crate) blanks: u8,
    pub(crate) bingo_bonus: u8,
    pub(crate) board_layout: BoardLayout,
    pub width: u8,
    pub height: u8,
}

pub const SCRABBLE_VARIANT_OFFICIAL: ScrabbleVariant = ScrabbleVariant {
    letter_values: [
        /*
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q , R, S, T, U, V, W, X, Y, Z, */
        1, 3, 3, 2, 1, 4, 2, 4, 1, 8, 5, 1, 3, 1, 1, 3, 10, 1, 1, 1, 1, 4, 4, 8, 4, 10,
    ],
    letter_distribution: [
        /*
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q , R, S, T, U, V, W, X, Y, Z, */
        9, 2, 2, 4, 12, 2, 3, 2, 9, 1, 1, 4, 2, 6, 8, 2, 1, 6, 4, 6, 4, 2, 2, 1, 2, 1,
    ],
    blanks: 2,
    bingo_bonus: 50,
    board_layout: [
        (0, 0, CellType::TripleWord),
        (3, 0, CellType::DoubleLetter),
        (7, 0, CellType::TripleWord),
        (1, 1, CellType::DoubleWord),
        (5, 1, CellType::TripleLetter),
        (2, 2, CellType::DoubleWord),
        (6, 2, CellType::DoubleLetter),
        (0, 3, CellType::DoubleLetter),
        (3, 3, CellType::DoubleWord),
        (7, 3, CellType::DoubleLetter),
        (4, 4, CellType::DoubleWord),
        (1, 5, CellType::TripleLetter),
        (5, 5, CellType::TripleLetter),
        (2, 6, CellType::DoubleLetter),
        (6, 6, CellType::DoubleLetter),
        (0, 7, CellType::TripleWord),
        (3, 7, CellType::DoubleLetter),
        (7, 7, CellType::DoubleWord),
    ],
    width: 15,
    height: 15,
};

pub const SCRABBLE_VARIANT_WORDFEUD: ScrabbleVariant = ScrabbleVariant {
    letter_values: [
        /*
        A, B, C, D, E, F, G, H, I, J , K, L, M, N, O, P, Q , R, S, T, U, V, W, X, Y,
        Z */
        1, 4, 4, 2, 1, 4, 3, 4, 1, 10, 5, 1, 3, 1, 1, 4, 10, 1, 1, 1, 2, 4, 4, 8, 4,
        10,
        //1, 3, 3, 2, 1, 4, 2, 4, 1, 8 , 5, 1, 3, 1, 1, 3, 10, 1, 1, 1, 1, 4, 4, 8, 4, 10,
    ],
    letter_distribution: [
        /*
        A , B, C, D, E , F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,  */
        10, 2, 2, 5, 12, 2, 3, 3, 9, 1, 1, 4, 2, 6, 7, 2, 1, 6, 5, 7, 4, 2, 2, 1, 2, 1,
    ],
    blanks: 2,
    bingo_bonus: 40,
    board_layout: [
        (0, 0, CellType::TripleLetter),
        (4, 0, CellType::TripleWord),
        (7, 0, CellType::DoubleLetter),
        (1, 1, CellType::DoubleLetter),
        (5, 1, CellType::TripleLetter),
        (2, 2, CellType::DoubleWord),
        (6, 2, CellType::DoubleLetter),
        (3, 3, CellType::TripleLetter),
        (7, 3, CellType::DoubleWord),
        (0, 4, CellType::TripleWord),
        (4, 4, CellType::DoubleWord),
        (6, 4, CellType::DoubleLetter),
        (1, 5, CellType::TripleLetter),
        (5, 5, CellType::TripleLetter),
        (2, 6, CellType::DoubleLetter),
        (4, 6, CellType::DoubleLetter),
        (0, 7, CellType::DoubleLetter),
        (3, 7, CellType::DoubleWord),
    ],
    width: 15,
    height: 15,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    pub(crate) scrabble_variant: &'static ScrabbleVariant,
    pub(crate) cells: [Cell; 225],
}

#[allow(unused_variables)]
//const VERTICAL_BORDER: char = '|';
const VERTICAL_BORDER: char = '\u{2502}';
const HEADINGS: &str = "    A   B   C   D   E   F   G   H   I   J   K   L   M   N   O\n";
const TOP_BORDER: &str = "  \u{250c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{252c}───\u{2510}\n";
const MIDDLE_BORDER: &str = "  \u{251c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{253c}───\u{2524}\n";
const BOTTOM_BORDER: &str = "  \u{2514}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2534}───\u{2518}\n";
#[deny(unused_variables)]

// TO DO: use unicode box drawing characters
// https://en.wikipedia.org/wiki/Box-drawing_character
// https://www.fileformat.info/info/unicode/block/box_drawing/list.htm

// impl Display as a grid with borders around each cell
impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = String::new();
        let mut first_line = true;
        s.push_str(&BACKGROUND);
        s.push_str(&GRIDCOLOUR);
        for y in 0..15 {
            if first_line {
                s.push_str(&HEADINGS);
                s.push_str(&TOP_BORDER);
                first_line = false;
            } else {
                s.push_str(&MIDDLE_BORDER);
            }
            s.push_str(format!("{:2}", y + 1).as_str());

            for x in 0..15 {
                s.push(VERTICAL_BORDER);
                s.push_str(&format!("{}{}", self.get_cell(x, y), GRIDCOLOUR));
            }
            s.push(VERTICAL_BORDER);
            s.push_str(format!("{:2}", y + 1).as_str());
            s.push('\n');
        }
        s.push_str(&BOTTOM_BORDER);
        s.push_str(&HEADINGS);
        write!(f, "{}{}", s, AnsiCodes::RESET)
    }
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WordPositions {
    pub start_pos: Position,
    pub end_pos: Position,
}
impl Board {
    pub(crate) fn get_cell(&self, x: u8, y: u8) -> &Cell {
        &self.cells[(y * 15 + x) as usize]
    }
    pub(crate) fn get_cell_mut<'a>(&'a mut self, x: u8, y: u8) -> &'a mut Cell {
        &mut self.cells[(y * 15 + x) as usize]
    }
    pub fn get_cell_pos(&self, pos: Position) -> &Cell {
        &self.cells[(pos.y * 15 + pos.x) as usize]
    }
    pub(crate) fn get_cell_pos_mut<'a>(&'a mut self, pos: Position) -> &'a mut Cell {
        &mut self.cells[(pos.y * 15 + pos.x) as usize]
    }
    pub fn is_valid_position(&self, pos: Position) -> bool {
        pos.x < self.scrabble_variant.width && pos.y < self.scrabble_variant.height
    }
    pub(crate) fn new(variant: &'static ScrabbleVariant) -> Board {
        let mut board = Board {
            scrabble_variant: variant,
            cells: [Cell {
                value: CellValue::Empty {
                    horizontal_letters: LetterSet::new_full(),
                    vertical_letters: LetterSet::new_full(),
                },
                cell_type: CellType::Blank,
            }; 225],
        };
        for (x, y, cell_type) in variant.board_layout.iter() {
            board.get_cell_mut(*x, *y).cell_type = *cell_type;
            board.get_cell_mut(14 - *x, *y).cell_type = *cell_type;
            board.get_cell_mut(*x, 14 - *y).cell_type = *cell_type;
            board.get_cell_mut(14 - *x, 14 - *y).cell_type = *cell_type;
        }
        board
    }

    pub fn is_enabler(&self, pos: Position) -> bool {
        if pos == (Position { x: 7, y: 7 }) {
            return true;
        }
        let cell = self.get_cell_pos(pos);
        cell.value.is_horizontally_connected() || cell.value.is_vertically_connected()
    }

    pub fn read_word(&self, wp: WordPositions, direction: Direction) -> String {
        // assume positions are aligned along direction
        // assume cells in range are filled with letters

        let mut pos = wp.start_pos;
        let mut word = String::from(self.get_cell_pos(pos).as_filled().unwrap().as_char());
        if pos != wp.end_pos {
            while let Some(pos_next) = pos.try_step_forward(direction) {
                word.push(self.get_cell_pos(pos_next).as_filled().unwrap().as_char());
                if pos_next == wp.end_pos {
                    break;
                }
                pos = pos_next;
            }
        }
        word
    }

    pub fn read_word_at_pos(&self, reference_position: Position, direction: Direction) -> String {
        let mut pos_iterator = self.start_of_word(reference_position, direction);
        let mut word = String::new();
        loop {
            match self.get_cell_pos(pos_iterator).value {
                CellValue::Empty { .. } => break,
                CellValue::Filled { letter, .. } => word.push(letter.as_char()),
            }
            if let Some(next_position) = pos_iterator.try_step_forward(direction) {
                pos_iterator = next_position;
            } else {
                break;
            }
        }
        word
    }

    /*
    For a given cell identify the word it is contained in, the central word.
    Also, if they exist, the previous word and the next word. A gap of only one cell is allowed.
    Update these gap cells with the letters that can be played in them.
    */
    pub fn update_word_gaps(&mut self, reference_position: Position, direction: Direction) {
        // assume central word is surrounded by empty cells or the board edge
        let mut previous_word: String = String::new();
        let central_word = self.read_word_at_pos(reference_position, direction);
        let mut next_word: String = String::new();

        let pos = self.start_of_word(reference_position, direction);
        if let Some(previous_gap) = pos.try_step_backward(direction) {
            // there is an empty cell, not the board edge
            if let Some(pos) = previous_gap.try_step_backward(direction) {
                match self.get_cell_pos(pos).value {
                    CellValue::Filled { .. } => {
                        // there is a previous word, not another empty cell or the board edge
                        previous_word = self.read_word_at_pos(pos, direction);
                    }
                    _ => {}
                }
            }
            let mut ls = LetterSet::new_empty();
            for l in ALPHABET {
                if is_word(format!("{}{}{}", previous_word, l, central_word).as_str()) {
                    ls.add(*l);
                }
            }
            self.get_cell_pos_mut(previous_gap)
                .value
                .set_letter_set(-direction, ls);
        }

        let pos = self.end_of_word(pos, direction);
        if let Some(next_gap) = pos.try_step_forward(direction) {
            // there is an empty cell, not the board edge
            if let Some(pos) = next_gap.try_step_forward(direction) {
                match self.get_cell_pos(pos).value {
                    CellValue::Filled { .. } => {
                        // there is a next word, not another empty cell or the board edge
                        next_word = self.read_word_at_pos(pos, direction);
                    }
                    _ => {}
                }
            }
            let mut ls = LetterSet::new_empty();
            for l in ALPHABET {
                if is_word(format!("{}{}{}", central_word, l, next_word).as_str()) {
                    ls.add(*l);
                }
            }
            self.get_cell_pos_mut(next_gap)
                .value
                .set_letter_set(-direction, ls);
        }
    }

    pub fn score_cross_word(
        &self,
        central_position: Position,
        direction: Direction,
        central_tile: Tile,
        central_letter: Letter,
    ) -> Result<i16, MoveError> {
        let mut cross_score = 0i16;
        let mut cross_pos = central_position;
        let central_cell = self.get_cell_pos(central_position);

        // get the letters and score of the preceding tiles
        let mut previous_letters: Vec<Letter> = vec![];
        while let Some(cross_pos_next) = cross_pos.try_step_backward(direction) {
            match self.get_cell_pos(cross_pos_next).value {
                CellValue::Filled {
                    letter, is_blank, ..
                } => {
                    previous_letters.push(letter);
                    cross_score += if is_blank {
                        0
                    } else {
                        self.scrabble_variant.letter_values[letter.as_usize()] as i16
                    };
                }
                CellValue::Empty { .. } => break,
            }
            cross_pos = cross_pos_next;
        }

        let mut cross_word = String::new();
        for letter in previous_letters.iter().rev() {
            cross_word.push(letter.as_char());
        }

        // add the letter and score of the central tile
        cross_word.push(central_letter.as_char());
        cross_score += (central_tile.score(self.scrabble_variant)
            * central_cell.cell_type.letter_multiplier()) as i16;

        // get the letters and score of the following tiles
        cross_pos = central_position;
        while let Some(cross_pos_next) = cross_pos.try_step_forward(direction) {
            match self.get_cell_pos(cross_pos_next).value {
                CellValue::Filled {
                    letter, is_blank, ..
                } => {
                    cross_word.push(letter.as_char());
                    cross_score += if is_blank {
                        0
                    } else {
                        self.scrabble_variant.letter_values[letter.as_usize()] as i16
                    };
                }
                CellValue::Empty { .. } => break,
            }
            cross_pos = cross_pos_next;
        }

        if !is_word(&cross_word) {
            return Err(MoveError::InvalidWord(cross_word));
        }
        Ok(cross_score * central_cell.cell_type.word_multiplier() as i16)
    }

    pub fn start_of_word(&self, reference_pos: Position, direction: Direction) -> Position {
        let mut pos_iterator = reference_pos;
        let mut start_pos = pos_iterator;
        while let Some(next_pos) = pos_iterator.try_step_backward(direction) {
            match self.get_cell_pos(next_pos).value {
                CellValue::Empty { .. } => break,
                CellValue::Filled { .. } => {
                    start_pos = next_pos;
                }
            }
            pos_iterator = next_pos;
        }
        start_pos
    }

    pub fn end_of_word(&self, reference_position: Position, direction: Direction) -> Position {
        let mut pos_iterator = reference_position;
        let mut end_pos = pos_iterator;
        while let Some(next_pos) = pos_iterator.try_step_forward(direction) {
            match self.get_cell_pos(next_pos).value {
                CellValue::Empty { .. } => break,
                CellValue::Filled { .. } => {
                    end_pos = next_pos;
                }
            }
            pos_iterator = next_pos;
        }
        end_pos
    }

    pub fn move_iterator(
        &'_ self,
        starting_position: Position,
        direction: Direction,
    ) -> MoveIterator<'_> {
        MoveIterator::new(
            self,
            self.start_of_word(starting_position, direction),
            direction,
        )
    }
    pub fn reset_last_move_flags(&mut self) {
        for cell in self.cells.iter_mut() {
            match cell.value {
                CellValue::Filled {
                    ref mut populated_last_move,
                    ..
                } => {
                    *populated_last_move = false;
                }
                _ => {}
            }
        }
    }
}

pub enum MoveCell {
    Open,
    Connecting { letter_set: LetterSet },
    Filled { letter: Letter, score: i16 },
}

pub struct MoveIterator<'a> {
    board: &'a Board,
    direction: Direction,

    pos: Position,
    tiles_placed: u8,
    reached_end: bool,
}

impl<'a> MoveIterator<'a> {
    pub fn new(
        board: &'a Board,
        starting_position: Position,
        direction: Direction,
    ) -> MoveIterator<'a> {
        MoveIterator {
            board,
            direction,
            pos: starting_position,
            tiles_placed: 0,
            reached_end: false,
        }
    }
}

impl Iterator for MoveIterator<'_> {
    type Item = (Position, MoveCell);

    fn next(&mut self) -> Option<Self::Item> {
        if self.reached_end {
            return None;
        }
        let cell = self.board.get_cell_pos(self.pos);
        let result = match cell.value {
            CellValue::Empty {
                horizontal_letters,
                vertical_letters,
            } => {
                let letter_set = match self.direction {
                    Direction::Horizontal => horizontal_letters,
                    Direction::Vertical => vertical_letters,
                };
                if letter_set.is_empty() {
                    None
                } else {
                    if self.tiles_placed == 7 {
                        None
                    } else {
                        self.tiles_placed += 1;
                        if letter_set.is_full() {
                            Some((self.pos, MoveCell::Open))
                        } else {
                            Some((self.pos, MoveCell::Connecting { letter_set }))
                        }
                    }
                }
            }
            CellValue::Filled {
                letter, is_blank, ..
            } => {
                let score = if is_blank {
                    0
                } else {
                    self.board.scrabble_variant.letter_values[letter.as_usize()] as i16
                };
                Some((self.pos, MoveCell::Filled { letter, score }))
            }
        };
        if let Some(pos) = self.pos.try_step_forward(self.direction) {
            self.pos = pos;
        } else {
            self.reached_end = true;
        };
        result
    }
}
pub struct AnsiCodes;
#[allow(unused, dead_code)]
impl AnsiCodes {
    const CLS: &str = "\x1B[2J";
    const HOME: &str = "\x1B[H";
    const RED: &str = "\x1B[31m";
    const GREEN: &str = "\x1B[32m";
    const BLUE: &str = "\x1B[34m";
    const YELLOW: &str = "\x1B[33m";
    const MAGENTA: &str = "\x1B[35m";
    const CYAN: &str = "\x1B[36m";
    const WHITE: &str = "\x1B[37m";
    const BLACK: &str = "\x1B[30m";
    const RESET: &str = "\x1B[0m";
    const BOLDON: &str = "\x1B[1m";
    const BOLDOFF: &str = "\x1B[22m";
    const BACKGROUNDWHITE: &str = "\x1B[47m";
    const BACKGROUNDRED: &str = "\x1B[41m";
    const BACKGROUNDGREEN: &str = "\x1B[42m";
    const BACKGROUNDYELLOW: &str = "\x1B[43m";
    const BACKGROUNDBLUE: &str = "\x1B[44m";
    const BACKGROUNDMAGENTA: &str = "\x1B[45m";
    const BACKGROUNDCYAN: &str = "\x1B[46m";
    const BACKGROUNDRESET: &str = "\x1B[49m";
    const UNDERLINEON: &str = "\x1B[4m";
    const UNDERLINEOFF: &str = "\x1B[24m";
    const BACKGROUNDBLACK: &str = "\x1B[40m";
}

const GRIDCOLOUR: &str = AnsiCodes::WHITE;
const TILECOLOUR: &str = AnsiCodes::BLUE;
const BLANKCOLOUR: &str = AnsiCodes::MAGENTA;
const LETTERCOLOUR: &str = AnsiCodes::WHITE;
const BACKGROUND: &str = AnsiCodes::BACKGROUNDBLACK;
