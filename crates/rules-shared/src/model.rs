use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::ops::Neg;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Horizontal,
    Vertical,
}

impl Neg for Direction {
    type Output = Direction;

    fn neg(self) -> Self::Output {
        match self {
            Direction::Horizontal => Direction::Vertical,
            Direction::Vertical => Direction::Horizontal,
        }
    }
}

impl Display for Direction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Horizontal => write!(f, "Horizontal"),
            Direction::Vertical => write!(f, "Vertical"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub x: u8,
    pub y: u8,
}

impl Position {
    pub const fn new(x: u8, y: u8) -> Self {
        Self { x, y }
    }

    pub fn try_step_forward(&self, direction: Direction, width: u8, height: u8) -> Option<Self> {
        match direction {
            Direction::Horizontal if self.x + 1 < width => Some(Self::new(self.x + 1, self.y)),
            Direction::Vertical if self.y + 1 < height => Some(Self::new(self.x, self.y + 1)),
            _ => None,
        }
    }

    pub fn try_step_backward(&self, direction: Direction) -> Option<Self> {
        match direction {
            Direction::Horizontal if self.x > 0 => Some(Self::new(self.x - 1, self.y)),
            Direction::Vertical if self.y > 0 => Some(Self::new(self.x, self.y - 1)),
            _ => None,
        }
    }

    pub const fn to_index(self, width: usize) -> usize {
        (self.y as usize) * width + (self.x as usize)
    }
}

impl Display for Position {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", (self.x + b'A') as char, self.y + 1)
    }
}

impl FromStr for Position {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() < 2 {
            return Err(format!("Invalid position: {s}"));
        }

        let mut chars = s.chars();
        let column = chars
            .next()
            .ok_or_else(|| format!("Invalid position: {s}"))?;

        if !(('A'..='O').contains(&column)) {
            return Err(format!("Invalid position: {s}"));
        }

        let row_str: String = chars.collect();
        let row = row_str
            .parse::<u8>()
            .map_err(|_| format!("Invalid position: {s}"))?;

        if !(1..=15).contains(&row) {
            return Err(format!("Invalid position: {s}"));
        }

        Ok(Self::new(column as u8 - b'A', row - 1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Letter(pub u8);

impl Letter {
    pub const FIRST_ASCII: u8 = b'A';

    pub const fn as_byte(self) -> u8 {
        self.0
    }

    pub const fn as_char(self) -> char {
        (self.0 + Self::FIRST_ASCII) as char
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl From<u8> for Letter {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

impl From<char> for Letter {
    fn from(value: char) -> Self {
        Self(value as u8 - Self::FIRST_ASCII)
    }
}

impl Display for Letter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_char())
    }
}

/// The ordered set of characters a ruleset's `Letter`s actually mean —
/// `Letter` is just a compact index (0.., see `Letter::as_usize`), and
/// without an `Alphabet` to look it up in, that index has no inherent
/// meaning. Every ruleset in production today uses `Alphabet::latin26()`,
/// but nothing in `rules-shared`'s core logic (dictionary search,
/// cross-checks, scoring) hardcodes that assumption anymore — it always
/// goes through whichever alphabet the current `VariantRules` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alphabet {
    chars: Vec<char>,
}

impl Alphabet {
    /// The standard 26-letter Latin alphabet (A-Z) — every edition this
    /// project ships today uses this.
    pub fn latin26() -> Self {
        Self {
            chars: ('A'..='Z').collect(),
        }
    }

    /// Builds an alphabet from an explicit, ordered list of characters —
    /// for a language whose letters aren't a dense run starting at 'A'
    /// (accented Latin, a different script, ...). `Letter(i)` means
    /// `chars[i]`, for whatever order is given here.
    pub fn from_chars(chars: impl IntoIterator<Item = char>) -> Self {
        Self {
            chars: chars.into_iter().collect(),
        }
    }

    pub fn to_char(&self, letter: Letter) -> Option<char> {
        self.chars.get(letter.as_usize()).copied()
    }

    pub fn to_letter(&self, ch: char) -> Option<Letter> {
        self.chars
            .iter()
            .position(|&candidate| candidate == ch)
            .map(|index| Letter(index as u8))
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tile {
    Letter(Letter),
    Blank { acting_as: Option<Letter> },
}

impl Tile {
    pub fn letter(self) -> Option<Letter> {
        match self {
            Tile::Letter(letter) => Some(letter),
            Tile::Blank { acting_as } => acting_as,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Premium {
    Blank,
    DoubleLetter,
    TripleLetter,
    DoubleWord,
    TripleWord,
}

impl Premium {
    pub const fn letter_multiplier(self) -> u8 {
        match self {
            Premium::DoubleLetter => 2,
            Premium::TripleLetter => 3,
            _ => 1,
        }
    }

    pub const fn word_multiplier(self) -> u8 {
        match self {
            Premium::DoubleWord => 2,
            Premium::TripleWord => 3,
            _ => 1,
        }
    }
}

/// Widened from `u32`: still a plain bitmask (one bit per letter, cheap
/// branchless test/set), but `u32` capped every ruleset at 32 distinct
/// letters. `u64` comfortably covers any realistic single-codepoint
/// alphabet (Cyrillic ~33, for instance) without needing a dynamic bitset.
pub type LetterMask = u64;
pub type Score = i16;

/// Every `Rack`/`VariantRules` letter-indexed array is a fixed array sized
/// to this, rather than growing per-alphabet (`Vec`) — deliberately, to
/// keep `Rack` cheaply `Copy` (it's passed and stored by value throughout
/// move generation and game state) instead of a heap-allocated `Vec` no
/// alphabet in production actually needs yet. Matches `LetterMask`'s bit
/// width, since a letter index that didn't fit in the mask couldn't be
/// checked/pruned anyway.
pub const MAX_ALPHABET_SIZE: usize = 64;

pub const FULL_LETTER_MASK: LetterMask = LetterMask::MAX;

pub const fn mask_contains(mask: LetterMask, letter: Letter) -> bool {
    (mask & (1 << letter.as_usize())) != 0
}

pub fn mask_insert(mask: &mut LetterMask, letter: Letter) {
    *mask |= 1 << letter.as_usize();
}

pub fn mask_remove(mask: &mut LetterMask, letter: Letter) {
    *mask &= !(1 << letter.as_usize());
}

pub const fn mask_is_empty(mask: LetterMask) -> bool {
    mask == 0
}

pub const fn mask_is_full(mask: LetterMask) -> bool {
    mask == FULL_LETTER_MASK
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rack {
    // `std::array::from_fn`/serde's own array impls only go up to 32
    // elements natively — `Default` and `Serialize`/`Deserialize` are
    // written by hand below rather than pulling in a
    // fixed-size-array-serde crate for this one field.
    #[serde(
        serialize_with = "serialize_letter_array",
        deserialize_with = "deserialize_letter_array"
    )]
    pub counts: [u8; MAX_ALPHABET_SIZE],
    pub blanks: u8,
}

impl Default for Rack {
    fn default() -> Self {
        Self {
            counts: [0; MAX_ALPHABET_SIZE],
            blanks: 0,
        }
    }
}

fn serialize_letter_array<S: serde::Serializer>(
    counts: &[u8; MAX_ALPHABET_SIZE],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    counts.as_slice().serialize(serializer)
}

fn deserialize_letter_array<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<[u8; MAX_ALPHABET_SIZE], D::Error> {
    let values = Vec::<u8>::deserialize(deserializer)?;
    if values.len() > MAX_ALPHABET_SIZE {
        return Err(serde::de::Error::invalid_length(
            values.len(),
            &"at most MAX_ALPHABET_SIZE letter counts",
        ));
    }
    // Accepts anything up to MAX_ALPHABET_SIZE, zero-padding the rest —
    // not just an exact match — so a rack persisted before this array was
    // widened (26 counts) still deserializes correctly instead of every
    // pre-existing saved game failing to load.
    let mut counts = [0u8; MAX_ALPHABET_SIZE];
    counts[..values.len()].copy_from_slice(&values);
    Ok(counts)
}

impl Rack {
    pub fn count(self) -> u8 {
        self.counts.iter().sum::<u8>() + self.blanks
    }

    pub fn is_empty(self) -> bool {
        self.count() == 0
    }

    pub fn contains_letter(self, letter: Letter) -> bool {
        self.counts[letter.as_usize()] > 0
    }

    pub fn add_letter(&mut self, letter: Letter) {
        self.counts[letter.as_usize()] += 1;
    }

    pub fn remove_letter(&mut self, letter: Letter) -> bool {
        let count = &mut self.counts[letter.as_usize()];
        if *count > 0 {
            *count -= 1;
            true
        } else {
            false
        }
    }

    pub fn remove_blank(&mut self) -> bool {
        if self.blanks > 0 {
            self.blanks -= 1;
            true
        } else {
            false
        }
    }

    pub fn consume_tile(&mut self, tile: Tile) -> bool {
        match tile {
            Tile::Letter(letter) => self.remove_letter(letter),
            Tile::Blank { acting_as: Some(_) } => self.remove_blank(),
            Tile::Blank { acting_as: None } => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariantRules {
    /// The bundled edition name ("official", "wordfeud", ...) — board
    /// layout, letter values, tile distribution, and dictionary all travel
    /// together under this one name (real Scrabble editions don't mix and
    /// match these independently, so neither does this type).
    pub name: String,
    pub language: String,
    /// The characters this ruleset's `Letter`s mean — see `Alphabet`.
    /// Every edition today is `Alphabet::latin26()`, but nothing else in
    /// this type or in `rules-shared`'s search/scoring logic assumes that.
    pub alphabet: Alphabet,
    pub letter_values: [u8; MAX_ALPHABET_SIZE],
    pub tile_distribution: [u8; MAX_ALPHABET_SIZE],
    pub blank_tiles: u8,
    pub rack_size: u8,
    pub width: u8,
    pub height: u8,
    pub bingo_bonus: Score,
    pub premiums: [Premium; 225],
}

impl VariantRules {
    /// The actual character `letter` represents under this ruleset's
    /// alphabet — unlike `Letter::as_char()` (a fixed ASCII-offset guess
    /// that's only ever right for the default Latin alphabet), this is
    /// correct for whichever alphabet this specific ruleset actually uses.
    pub fn letter_char(&self, letter: Letter) -> char {
        self.alphabet
            .to_char(letter)
            .expect("Letter should be valid for this ruleset's alphabet")
    }

    /// Every letter this ruleset's alphabet defines, in index order —
    /// replaces the old global `ALPHABET` constant, which silently assumed
    /// every ruleset used the same 26-letter Latin alphabet.
    pub fn letters(&self) -> impl Iterator<Item = Letter> + '_ {
        (0..self.alphabet.len()).map(|index| Letter(index as u8))
    }

    pub fn official() -> Self {
        Self {
            name: "official".to_string(),
            language: "sowpods".to_string(),
            alphabet: Alphabet::latin26(),
            letter_values: pad_latin26([
                1, 3, 3, 2, 1, 4, 2, 4, 1, 8, 5, 1, 3, 1, 1, 3, 10, 1, 1, 1, 1, 4, 4, 8, 4, 10,
            ]),
            tile_distribution: pad_latin26([
                9, 2, 2, 4, 12, 2, 3, 2, 9, 1, 1, 4, 2, 6, 8, 2, 1, 6, 4, 6, 4, 2, 2, 1, 2, 1,
            ]),
            blank_tiles: 2,
            rack_size: 7,
            width: 15,
            height: 15,
            bingo_bonus: 50,
            premiums: mirrored_premiums(&[
                (0, 0, Premium::TripleWord),
                (3, 0, Premium::DoubleLetter),
                (7, 0, Premium::TripleWord),
                (1, 1, Premium::DoubleWord),
                (5, 1, Premium::TripleLetter),
                (2, 2, Premium::DoubleWord),
                (6, 2, Premium::DoubleLetter),
                (0, 3, Premium::DoubleLetter),
                (3, 3, Premium::DoubleWord),
                (7, 3, Premium::DoubleLetter),
                (4, 4, Premium::DoubleWord),
                (1, 5, Premium::TripleLetter),
                (5, 5, Premium::TripleLetter),
                (2, 6, Premium::DoubleLetter),
                (6, 6, Premium::DoubleLetter),
                (0, 7, Premium::TripleWord),
                (3, 7, Premium::DoubleLetter),
                (7, 7, Premium::DoubleWord),
            ]),
        }
    }

    /// Wordfeud's actual numbers (letter values, tile distribution, bingo
    /// bonus, premium layout all genuinely differ from official) — reused
    /// verbatim from `old-crates/*/src/board.rs`'s `SCRABBLE_VARIANT_WORDFEUD`,
    /// the project's own superseded-but-still-accurate prior art. Still
    /// English/ASCII and still 15×15, so this is proof of the edition
    /// registry, not of any board-size or alphabet generalization.
    pub fn wordfeud() -> Self {
        Self {
            name: "wordfeud".to_string(),
            language: "sowpods".to_string(),
            alphabet: Alphabet::latin26(),
            letter_values: pad_latin26([
                1, 4, 4, 2, 1, 4, 3, 4, 1, 10, 5, 1, 3, 1, 1, 4, 10, 1, 1, 1, 2, 4, 4, 8, 4, 10,
            ]),
            tile_distribution: pad_latin26([
                10, 2, 2, 5, 12, 2, 3, 3, 9, 1, 1, 4, 2, 6, 7, 2, 1, 6, 5, 7, 4, 2, 2, 1, 2, 1,
            ]),
            blank_tiles: 2,
            rack_size: 7,
            width: 15,
            height: 15,
            bingo_bonus: 40,
            premiums: mirrored_premiums(&[
                (0, 0, Premium::TripleLetter),
                (4, 0, Premium::TripleWord),
                (7, 0, Premium::DoubleLetter),
                (1, 1, Premium::DoubleLetter),
                (5, 1, Premium::TripleLetter),
                (2, 2, Premium::DoubleWord),
                (6, 2, Premium::DoubleLetter),
                (3, 3, Premium::TripleLetter),
                (7, 3, Premium::DoubleWord),
                (0, 4, Premium::TripleWord),
                (4, 4, Premium::DoubleWord),
                (6, 4, Premium::DoubleLetter),
                (1, 5, Premium::TripleLetter),
                (5, 5, Premium::TripleLetter),
                (2, 6, Premium::DoubleLetter),
                (4, 6, Premium::DoubleLetter),
                (0, 7, Premium::DoubleLetter),
                (3, 7, Premium::DoubleWord),
            ]),
        }
    }

    /// The edition registry — every bundled ruleset this server knows
    /// about, looked up by name. `None` for an unrecognized name (the
    /// caller decides whether that's a client error).
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "official" => Some(Self::official()),
            "wordfeud" => Some(Self::wordfeud()),
            _ => None,
        }
    }
}

/// Pads a 26-value Latin-alphabet table (letter values, tile distribution)
/// out to `MAX_ALPHABET_SIZE` — every edition's actual `Alphabet` (not this
/// array's length) is what bounds which slots are ever read, so the tail
/// stays zeroed and unused for any 26-letter edition.
fn pad_latin26(values: [u8; 26]) -> [u8; MAX_ALPHABET_SIZE] {
    let mut padded = [0u8; MAX_ALPHABET_SIZE];
    padded[..26].copy_from_slice(&values);
    padded
}

/// Expands 18 canonical premium-square positions (one symmetric quadrant)
/// into the full 225-cell board via 4-way mirroring — every edition's board
/// is symmetric, so this is shared regardless of which premiums it uses.
fn mirrored_premiums(canonical: &[(u8, u8, Premium)]) -> [Premium; 225] {
    let mut premiums = [Premium::Blank; 225];

    for &(x, y, premium) in canonical {
        for (mx, my) in mirror_positions(x, y) {
            premiums[(my as usize) * 15 + (mx as usize)] = premium;
        }
    }

    premiums
}

fn mirror_positions(x: u8, y: u8) -> [(u8, u8); 4] {
    let max = 14;
    [(x, y), (max - x, y), (x, max - y), (max - x, max - y)]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TilePlacement {
    pub offset: u8,
    pub tile: Tile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveCandidate {
    pub start: Position,
    pub direction: Direction,
    pub tiles: Vec<TilePlacement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossWordPreview {
    pub pos: Position,
    pub word: String,
    pub score: Score,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePreview {
    pub legal: bool,
    pub main_word: String,
    pub total_score: Score,
    pub cross_words: Vec<CrossWordPreview>,
    pub error: Option<MoveError>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MoveScore {
    pub total: Score,
    pub main_word_score: Score,
    pub cross_word_score: Score,
    pub bingo_bonus: Score,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedMove {
    pub candidate: MoveCandidate,
    pub preview: MovePreview,
    pub score: MoveScore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveError {
    InvalidMove,
    /// One or more words formed by this placement — the main word and/or
    /// any cross words — aren't in the dictionary. Always at least one
    /// entry; the main word (if invalid) comes first.
    InvalidWord(Vec<String>),
    InvalidPosition,
    InvalidDirection,
    TilesDoNotFit,
    TilesDoNotConnect,
}

#[cfg(test)]
mod tests {
    use super::{
        Direction, Letter, LetterMask, Position, Rack, Tile, VariantRules, mask_contains,
        mask_insert, mask_is_empty, mask_remove,
    };
    use std::str::FromStr;

    #[test]
    fn parse_position() {
        let pos = Position::from_str("H8").unwrap();
        assert_eq!(pos, Position::new(7, 7));
    }

    #[test]
    fn rack_deserializes_a_pre_widening_26_count_snapshot() {
        // Regression test: `Rack.counts` widened from [u8;26] to
        // [u8;MAX_ALPHABET_SIZE] — a rack persisted before that change is
        // exactly this 26-element JSON shape, and must still load instead
        // of every pre-existing saved game failing at startup.
        let json = r#"{"counts":[1,0,2,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"blanks":1}"#;
        let rack: Rack = serde_json::from_str(json).expect("old 26-count rack should still parse");
        assert_eq!(rack.counts[0], 1);
        assert_eq!(rack.counts[2], 2);
        assert_eq!(rack.blanks, 1);
        assert!(rack.counts[26..].iter().all(|&count| count == 0));
    }

    #[test]
    fn rack_round_trips_a_full_64_count_snapshot() {
        let mut rack = Rack::default();
        rack.add_letter(Letter::from(30u8));
        let json = serde_json::to_string(&rack).expect("rack should serialize");
        let restored: Rack = serde_json::from_str(&json).expect("rack should round-trip");
        assert_eq!(restored, rack);
    }

    #[test]
    fn step_position() {
        let pos = Position::new(7, 7);
        assert_eq!(
            pos.try_step_forward(Direction::Horizontal, 15, 15),
            Some(Position::new(8, 7))
        );
        assert_eq!(
            pos.try_step_backward(Direction::Vertical),
            Some(Position::new(7, 6))
        );
    }

    #[test]
    fn letter_to_char() {
        assert_eq!(Letter::from('A').as_char(), 'A');
        assert_eq!(Letter::from('Z').as_usize(), 25);
    }

    #[test]
    fn letter_mask_helpers() {
        let mut mask: LetterMask = 0;
        assert!(mask_is_empty(mask));
        mask_insert(&mut mask, Letter::from('C'));
        assert!(mask_contains(mask, Letter::from('C')));
        mask_remove(&mut mask, Letter::from('C'));
        assert!(mask_is_empty(mask));
    }

    #[test]
    fn rack_consumes_tiles() {
        let mut rack = Rack {
            blanks: 1,
            ..Rack::default()
        };
        rack.add_letter(Letter::from('A'));

        assert!(rack.consume_tile(Tile::Letter(Letter::from('A'))));
        assert!(rack.consume_tile(Tile::Blank {
            acting_as: Some(Letter::from('B')),
        }));
        assert!(!rack.consume_tile(Tile::Letter(Letter::from('Z'))));
    }

    #[test]
    fn wordfeud_bundles_its_own_letter_values_and_bingo_bonus_distinct_from_official() {
        let official = VariantRules::official();
        let wordfeud = VariantRules::wordfeud();
        assert_eq!(official.name, "official");
        assert_eq!(wordfeud.name, "wordfeud");
        assert_ne!(official.bingo_bonus, wordfeud.bingo_bonus);
        assert_ne!(official.letter_values, wordfeud.letter_values);
        assert_ne!(official.tile_distribution, wordfeud.tile_distribution);
        // Both editions are still 15x15/English at this stage of the
        // project — only the bundled economics/board layout differ.
        assert_eq!(official.width, wordfeud.width);
        assert_eq!(official.height, wordfeud.height);
        assert_eq!(official.language, wordfeud.language);
    }

    #[test]
    fn by_name_resolves_known_editions_and_rejects_unknown_ones() {
        assert_eq!(VariantRules::by_name("official").unwrap().name, "official");
        assert_eq!(VariantRules::by_name("wordfeud").unwrap().name, "wordfeud");
        assert!(VariantRules::by_name("not-a-real-edition").is_none());
    }

    #[test]
    fn every_editions_premiums_are_still_a_symmetric_15x15_board() {
        for rules in [VariantRules::official(), VariantRules::wordfeud()] {
            assert_eq!(rules.premiums.len(), 225);
            for y in 0..15u8 {
                for x in 0..15u8 {
                    let mirrored = rules.premiums[(y as usize) * 15 + (14 - x) as usize];
                    let original = rules.premiums[(y as usize) * 15 + x as usize];
                    assert_eq!(
                        mirrored, original,
                        "{}'s premiums should be left/right symmetric at ({x}, {y})",
                        rules.name
                    );
                }
            }
        }
    }
}
