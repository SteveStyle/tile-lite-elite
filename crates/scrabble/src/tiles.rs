use std::fmt::{Display, Formatter};

use rand::Rng;

use crate::{board::ScrabbleVariant, MoveError, TScore};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct LetterSet {
    pub values: u32,
}

// defines a letter with a value between 0 and 25, where 'A' is 0 and 'Z' is 25
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Letter(pub u8);

impl From<u8> for Letter {
    fn from(n: u8) -> Letter {
        Letter(n)
    }
}

impl From<char> for Letter {
    fn from(c: char) -> Letter {
        Letter(c as u8 - FIRST_LETTER_ASCII_VALUE)
    }
}

impl Letter {
    pub fn as_byte(&self) -> u8 {
        self.0
    }
    pub fn as_char(&self) -> char {
        (self.0 + FIRST_LETTER_ASCII_VALUE) as char
    }
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

impl Display for Letter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_char())
    }
}

const FIRST_LETTER_ASCII_VALUE: u8 = 'A' as u8;

pub const ALPHABET: &'static [Letter] = &[
    Letter('A' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('B' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('C' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('D' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('E' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('F' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('G' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('H' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('I' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('J' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('K' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('L' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('M' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('N' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('O' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('P' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('Q' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('R' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('S' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('T' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('U' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('V' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('W' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('X' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('Y' as u8 - FIRST_LETTER_ASCII_VALUE),
    Letter('Z' as u8 - FIRST_LETTER_ASCII_VALUE),
];

const ALL_LETTERS_VALUE: u32 = 0b11111111111111111111111111;
const ALL_LETTERS: LetterSet = LetterSet {
    values: ALL_LETTERS_VALUE,
};

pub struct LetterSetIterator {
    values: u32,
    index: u8,
}

impl Iterator for LetterSetIterator {
    type Item = Letter;

    fn next(&mut self) -> Option<Letter> {
        if self.values == 0 {
            None
        } else {
            while self.index < 26 && self.values & (1 << self.index) == 0 {
                self.index += 1;
            }
            self.values &= !(1 << self.index);

            Some(Letter(self.index as u8))
        }
    }
}

impl IntoIterator for LetterSet {
    type Item = Letter;
    type IntoIter = LetterSetIterator;

    fn into_iter(self) -> Self::IntoIter {
        LetterSetIterator {
            values: self.values,
            index: 0,
        }
    }
}

impl Display for LetterSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for letter in self.into_iter() {
            write!(f, "{}", letter)?;
        }
        Ok(())
    }
}

impl From<Letter> for LetterSet {
    fn from(letter: Letter) -> Self {
        let mut set = LetterSet::new_empty();
        set.add(letter);
        set
    }
}

impl LetterSet {
    pub fn new_full() -> LetterSet {
        ALL_LETTERS
    }

    pub fn new_empty() -> LetterSet {
        LetterSet { values: 0 }
    }

    fn set_bit(&mut self, index: u8) {
        self.values |= 1 << index;
    }

    fn unset_bit(&mut self, index: u8) {
        self.values &= !(1 << index);
    }

    fn bit(&self, index: u8) -> bool {
        self.values & (1 << index) != 0
    }

    pub fn add(&mut self, letter: Letter) {
        self.set_bit(letter.0);
    }

    pub fn add_all(&mut self) {
        self.values = ALL_LETTERS_VALUE;
    }

    pub fn remove(&mut self, letter: Letter) {
        self.unset_bit(letter.0);
    }

    pub fn contains(&self, letter: Letter) -> bool {
        self.bit(letter.0)
    }

    pub fn remove_all(&mut self) {
        self.values = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.values == 0
    }

    pub fn is_full(&self) -> bool {
        self.values == ALL_LETTERS_VALUE
    }

    pub fn allows_rack(&self, rack: &TileBag) -> bool {
        for letter in self.into_iter() {
            if rack.contains(letter) {
                return true;
            }
        }
        false
    }
}
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tile {
    Letter(Letter),
    Blank { acting_as_letter: Option<Letter> },
}

impl Display for Tile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Tile::Letter(letter) => write!(f, "{}", letter),
            Tile::Blank {
                acting_as_letter: None,
            } => write!(f, "*"),
            Tile::Blank {
                acting_as_letter: Some(letter),
            } => write!(f, "{letter}"),
        }
    }
}

impl From<&str> for Tile {
    fn from(s: &str) -> Self {
        let s: Vec<char> = s.chars().collect();
        let first = s[0];
        let second = s[1];
        if first == '*' {
            Tile::Blank {
                acting_as_letter: Some(second.into()),
            }
        } else {
            Tile::Letter(Letter::from(first))
        }
    }
}
impl Tile {
    pub fn letter(&self) -> Option<Letter> {
        match self {
            Tile::Letter(letter) => Some(*letter),
            Tile::Blank {
                acting_as_letter: Some(letter),
            } => Some(*letter),
            Tile::Blank {
                acting_as_letter: None,
            } => None,
        }
    }
    pub fn is_blank(&self) -> bool {
        match self {
            Tile::Letter(_) => false,
            Tile::Blank { .. } => true,
        }
    }
    pub fn try_letter(&self) -> Result<Letter, crate::MoveError> {
        match self {
            Tile::Letter(letter) => Ok(*letter),
            Tile::Blank {
                acting_as_letter: Some(letter),
            } => Ok(*letter),
            Tile::Blank {
                acting_as_letter: None,
            } => Err(crate::MoveError::BlankTileNotActingAsLetter),
        }
    }

    pub fn score(&self, scrabble_variant: &ScrabbleVariant) -> u8 {
        match self {
            Tile::Letter(letter) => scrabble_variant.letter_values[letter.as_usize()],
            Tile::Blank { .. } => 0,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileBag {
    pub letters: [u8; 26],
    pub blanks: u8,
}

impl Display for TileBag {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for (letter, count) in self.letters.iter().enumerate() {
            for _ in 0..*count {
                write!(f, "{}", Letter(letter as u8))?;
            }
        }
        for _ in 0..self.blanks {
            write!(f, "*")?;
        }
        Ok(())
    }
}

impl TileBag {
    pub(crate) fn new(scrabble_variant: &'static ScrabbleVariant) -> TileBag {
        let letters = scrabble_variant.letter_distribution;
        let blanks = scrabble_variant.blanks;
        TileBag { letters, blanks }
    }
    pub fn new_empty() -> TileBag {
        TileBag {
            letters: [0; 26],
            blanks: 0,
        }
    }
    fn add_letter(&mut self, letter: Letter) {
        self.letters[letter.as_usize()] += 1;
    }
    fn add_blank(&mut self) {
        self.blanks += 1;
    }
    pub(crate) fn remove_letter(&mut self, letter: Letter) {
        self.letters[letter.as_usize()] -= 1;
    }
    pub(crate) fn remove_blank(&mut self) {
        self.blanks -= 1;
    }
    pub fn contains(&self, letter: Letter) -> bool {
        self.letters[letter.as_usize()] > 0
    }
    pub(crate) fn count(&self) -> u8 {
        self.letters.iter().sum::<u8>() + self.blanks
    }
    fn count_tile(&self, tile: Tile) -> u8 {
        match tile {
            Tile::Letter(letter) => self.letters[letter.as_usize()],
            Tile::Blank {
                acting_as_letter: _,
            } => self.blanks,
        }
    }
    pub(crate) fn sum_tile_values(&self, scrabble_variant: &ScrabbleVariant) -> TScore {
        let mut sum = 0;
        for (letter, letter_count) in self.letters.iter().enumerate() {
            sum += letter_count * scrabble_variant.letter_values[letter];
        }
        sum as TScore
    }

    fn random_tile(&self) -> Tile {
        let mut rng = rand::thread_rng();
        let count = self.count();
        let random = rng.gen_range(0..count);
        if random < self.blanks {
            Tile::Blank {
                acting_as_letter: None,
            }
        } else {
            let mut sum = self.blanks;
            for (letter, letter_count) in self.letters.iter().enumerate() {
                sum += letter_count;
                if random < sum {
                    return Tile::Letter(Letter(letter as u8));
                }
            }
            panic!("random_tile failed");
        }
    }

    fn add_tile(&mut self, tile: Tile) {
        match tile {
            Tile::Letter(letter) => self.add_letter(letter),
            Tile::Blank {
                acting_as_letter: _,
            } => self.add_blank(),
        }
    }
    pub fn remove_tile(&mut self, tile: Tile) {
        match tile {
            Tile::Letter(letter) => self.remove_letter(letter),
            Tile::Blank {
                acting_as_letter: _,
            } => self.remove_blank(),
        }
    }
    fn try_remove_tile(&mut self, tile: Tile) -> Result<(), crate::MoveError> {
        if self.count_tile(tile) > 0 {
            self.remove_tile(tile);
            Ok(())
        } else {
            Err(crate::MoveError::TilesNotInRack(tile.clone()))
        }
    }
    pub fn confirm_contains_tile_list(&self, other: &TileList) -> Result<(), MoveError> {
        let mut this_bag = self.clone();
        for tile in other.0.iter() {
            this_bag.try_remove_tile(*tile)?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }
    pub fn fill_rack(&mut self, bag: &mut TileBag) {
        while !bag.is_empty() && self.count() < 7 {
            let tile = bag.random_tile();
            self.add_tile(tile);
            bag.remove_tile(tile);
        }
    }

    pub(crate) fn remove_tile_list(&mut self, other: &TileList) {
        for tile in other.0.iter() {
            self.remove_tile(*tile);
        }
    }

    pub(crate) fn add_tile_list(&mut self, other: &TileList) {
        for tile in other.0.iter() {
            self.add_tile(*tile);
        }
    }
    pub fn to_vec(&self) -> Vec<char> {
        let mut vec = Vec::new();
        for (letter, count) in self.letters.iter().enumerate() {
            for _ in 0..*count {
                vec.push(Letter(letter as u8).as_char());
            }
        }

        vec
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileList(pub Vec<Tile>);
impl TileList {
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Display for TileList {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        for tile in self.0.iter() {
            s.push_str(&tile.to_string());
        }
        write!(f, "{}", s)
    }
}

impl From<Vec<Tile>> for TileList {
    fn from(vec: Vec<Tile>) -> Self {
        TileList(vec)
    }
}

impl From<TileList> for Vec<Tile> {
    fn from(list: TileList) -> Self {
        list.0
    }
}

impl From<TileList> for TileBag {
    fn from(list: TileList) -> Self {
        let mut bag = TileBag::new_empty();
        for tile in list.0 {
            bag.add_tile(tile);
        }
        bag
    }
}

impl TryFrom<&str> for TileList {
    type Error = crate::MoveError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let mut vec = Vec::new();
        let mut index = 0;
        let s = s.trim().to_uppercase();
        while index < s.len() {
            let c = s.chars().nth(index).unwrap();
            if c == '*' {
                if let Some(next) = s.chars().nth(index + 1) {
                    if next.is_ascii_uppercase() {
                        vec.push(Tile::Blank {
                            acting_as_letter: Some(next.into()),
                        });
                        index += 1;
                    } else {
                        return Err(crate::MoveError::InvalidTile(c));
                    }
                } else {
                    return Err(crate::MoveError::InvalidTile(c));
                }
            } else if c.is_ascii_uppercase() {
                vec.push(Tile::Letter(c.into()));
            } else {
                return Err(crate::MoveError::InvalidTile(c));
            }
            index += 1;
        }
        Ok(TileList(vec))
    }
}

impl From<TileBag> for TileList {
    fn from(bag: TileBag) -> Self {
        let mut vec = Vec::new();
        for (letter, count) in bag.letters.iter().enumerate() {
            for _ in 0..*count {
                vec.push(Tile::Letter(Letter(letter as u8)));
            }
        }
        for _ in 0..bag.blanks {
            vec.push(Tile::Blank {
                acting_as_letter: None,
            });
        }
        TileList(vec)
    }
}

impl TileList {
    pub fn new() -> Self {
        TileList(Vec::new())
    }
}

// test BitArray
#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_bit_array() {
        let mut bit_array = LetterSet::new_full();
        assert_eq!(bit_array.bit(0), true);
        bit_array.unset_bit(0);
        assert_eq!(bit_array.bit(0), false);
        bit_array.set_bit(0);
        assert_eq!(bit_array.bit(0), true);

        bit_array.remove_all();
        bit_array.set_bit(1);
        bit_array.set_bit(15);
        println!("{:b}", bit_array.values);
        println!("{}", bit_array.values);
    }

    #[test]
    fn test_bit_array_add() {
        let mut bit_array = LetterSet::new_empty();
        bit_array.add(Letter::from('A'));
        assert_eq!(bit_array.bit(0), true);
        bit_array.add(Letter::from('B'));
        assert_eq!(bit_array.bit(1), true);
        bit_array.add(Letter::from('Z'));
        assert_eq!(bit_array.bit(25), true);

        for i in 0..26 {
            assert_eq!(bit_array.bit(i), i == 0 || i == 1 || i == 25);
        }

        for c in ALPHABET {
            if bit_array.contains(*c) {
                println!("contains {}", c.as_char());
            }
        }
    }

    #[test]
    fn test_temp() {
        let letter: Letter = 'A'.into();
        println!("'A'.into::<Letter>() = {}", letter.as_byte());
        println!("'A'.into::<Letter>() = {}", letter.as_char());
    }

    // test letterset iterator
    #[test]
    fn test_letter_set_iterator() {
        let mut letter_set = LetterSet::new_full();
        for l in letter_set {
            print!("{}", l.as_char());
        }
        println!();
        letter_set.remove(Letter::from('A'));
        letter_set.remove(Letter::from('B'));
        letter_set.remove(Letter::from('Z'));
        for l in letter_set {
            print!("{}", l.as_char());
        }
        println!();
        let mut iter = letter_set.into_iter();
        assert_eq!(iter.next(), Some(Letter::from('C')));
        assert_eq!(iter.next(), Some(Letter::from('D')));
        assert_eq!(iter.next(), Some(Letter::from('E')));

        println!("letter set is: {}", letter_set);
        //assert_eq!(iter.next(), None);
    }
}
