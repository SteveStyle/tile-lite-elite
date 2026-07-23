use std::collections::HashSet;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::LazyLock;

use crate::model::{Alphabet, Letter};

/// Embedded at compile time for every non-wasm target (the server, and the
/// desktop client) — for those, this is free: no network cost, and disk
/// space isn't a constraint the way page-load size is. The wasm/web client
/// deliberately does *not* embed this (see `WordListDictionary::from_word_list`
/// and `sowpods_word_list()` below) — baking a ~2.7MB word list into every
/// page load doesn't scale once a second dictionary exists, and the server
/// already has this exact text in memory to serve on request instead.
#[cfg(not(target_arch = "wasm32"))]
const SOWPODS_WORD_FILE: &str = include_str!("sowpods.txt");

/// Same reasoning as `SOWPODS_WORD_FILE`, for the North American (ENABLE2K)
/// edition's dictionary.
#[cfg(not(target_arch = "wasm32"))]
const ENABLE2K_WORD_FILE: &str = include_str!("enable2k.txt");

/// Same reasoning as `SOWPODS_WORD_FILE`, for the German edition's
/// dictionary.
#[cfg(not(target_arch = "wasm32"))]
const GERMAN_WORD_FILE: &str = include_str!("german.txt");

/// Same reasoning as `SOWPODS_WORD_FILE`, for the Spanish edition's
/// dictionary. Plain, unannotated text — nothing here marks which "ch"/
/// "ll"/"rr" substrings are meant as a digraph tile, since the Spanish
/// edition deliberately doesn't require one (see `VariantRules::spanish`'s
/// doc comment).
#[cfg(not(target_arch = "wasm32"))]
const SPANISH_WORD_FILE: &str = include_str!("spanish.txt");

/// The raw SOWPODS word list text, for whoever serves it to clients that
/// fetch it at runtime instead of embedding it (see `crates/server-game`'s
/// `/dictionaries/:name` endpoint). Not meaningful on wasm, where nothing
/// has this text compiled in to begin with.
#[cfg(not(target_arch = "wasm32"))]
pub fn sowpods_word_list() -> &'static str {
    SOWPODS_WORD_FILE
}

/// Same as `sowpods_word_list`, for ENABLE2K.
#[cfg(not(target_arch = "wasm32"))]
pub fn enable2k_word_list() -> &'static str {
    ENABLE2K_WORD_FILE
}

/// Same as `sowpods_word_list`, for German.
#[cfg(not(target_arch = "wasm32"))]
pub fn german_word_list() -> &'static str {
    GERMAN_WORD_FILE
}

/// Same as `sowpods_word_list`, for Spanish.
#[cfg(not(target_arch = "wasm32"))]
pub fn spanish_word_list() -> &'static str {
    SPANISH_WORD_FILE
}

pub trait Dictionary {
    fn is_word(&self, word: &str) -> bool;

    /// A cursor type for incrementally searching this dictionary one
    /// letter at a time, used by move generation to prune a branch the
    /// moment the partial word it's building can't lead anywhere (see
    /// `expand_lane` in `generate.rs`) — without this, generation has to
    /// explore every rack-letter combination down to the full rack size
    /// before finding out most of them were dead ends, which is
    /// exponential in practice.
    ///
    /// The default `Cursor = ()` never prunes (`advance` always
    /// succeeds) — still correct for any `Dictionary` impl, just without
    /// the speedup. `WordListDictionary` overrides this with
    /// `SortedPrefixCursor`, a real implementation.
    type Cursor<'a>: PrefixCursor
    where
        Self: 'a;

    fn root_cursor(&self) -> Self::Cursor<'_>;
}

/// An incremental, letter-at-a-time search position into some backing
/// word structure. Different `Dictionary` implementations can back this
/// with completely different structures (a sorted array, a trie, ...) —
/// this is deliberately the *only* thing move generation needs from
/// whichever one is in use.
pub trait PrefixCursor: Copy {
    /// Narrows to the search state after also matching `letter`, or
    /// `None` if no word can possibly continue with it — the prune
    /// signal. `alphabet` resolves `letter` to the actual character it
    /// means under the current ruleset (see `VariantRules::alphabet`) —
    /// a cursor has no ambient alphabet of its own to assume.
    fn advance(&self, letter: Letter, alphabet: &Alphabet) -> Option<Self>;
}

impl PrefixCursor for () {
    fn advance(&self, _letter: Letter, _alphabet: &Alphabet) -> Option<Self> {
        Some(())
    }
}

pub struct WordListDictionary {
    words: HashSet<&'static str>,
    /// Tokenized into characters (not left as raw UTF-8 bytes) and sorted
    /// once at construction — the backing store for `SortedPrefixCursor`,
    /// which narrows a sub-slice one *character* at a time instead of
    /// re-searching the whole dictionary at every step of move generation.
    /// SOWPODS/ENABLE2K are both plain ASCII, so this only matters once a
    /// dictionary actually contains multi-byte UTF-8 words — but the
    /// search structure has to be character-indexed regardless of which
    /// dictionary it's built for, since byte-indexing silently desyncs
    /// the moment any word has a multi-byte character earlier in it.
    sorted_words: Vec<Vec<char>>,
}

impl WordListDictionary {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Self::from_static_word_list(SOWPODS_WORD_FILE)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn new_enable2k() -> Self {
        Self::from_static_word_list(ENABLE2K_WORD_FILE)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn new_german() -> Self {
        Self::from_static_word_list(GERMAN_WORD_FILE)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn new_spanish() -> Self {
        Self::from_static_word_list(SPANISH_WORD_FILE)
    }

    /// Builds a dictionary from word-list text fetched at runtime (one
    /// word per whitespace-separated token, same format as `sowpods.txt`)
    /// rather than embedded at compile time — this is how the wasm/web
    /// client gets its dictionary (see `crates/ui/src/app.rs`'s
    /// `load_client_dictionary`), fetching it from the server's
    /// `/dictionaries/:name` endpoint instead of bundling it into the page.
    ///
    /// `text` is leaked into a `&'static str` — accepted here since this is
    /// only ever called once per dictionary actually in use, for the
    /// program's whole lifetime (a page load, a server process), the same
    /// trade-off `include_str!` makes implicitly for the non-wasm path.
    pub fn from_word_list(text: String) -> Self {
        Self::from_static_word_list(Box::leak(text.into_boxed_str()))
    }

    fn from_static_word_list(text: &'static str) -> Self {
        let words: HashSet<&'static str> = text.split_whitespace().collect();
        let mut sorted_words: Vec<Vec<char>> =
            words.iter().map(|word| word.chars().collect()).collect();
        sorted_words.sort_unstable();
        Self {
            words,
            sorted_words,
        }
    }

    /// The starting point for an incremental prefix search — advance it
    /// one letter at a time as a candidate word is built up.
    pub fn prefix_cursor(&self) -> SortedPrefixCursor<'_> {
        SortedPrefixCursor {
            words: &self.sorted_words,
            depth: 0,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for WordListDictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl Dictionary for WordListDictionary {
    fn is_word(&self, word: &str) -> bool {
        self.words.contains(word)
    }

    type Cursor<'a> = SortedPrefixCursor<'a>;

    fn root_cursor(&self) -> Self::Cursor<'_> {
        self.prefix_cursor()
    }
}

/// A position in an incremental search over the sorted word list: `words`
/// is the sub-slice of (still sorted) entries that share the prefix
/// matched so far, and `depth` is how many *characters* that prefix has
/// (not bytes — a word's `Vec<char>` length, so a multi-byte character
/// still only ever counts as one step, unlike indexing raw UTF-8 bytes).
///
/// Advancing by one more letter binary-searches *within* `words` for the
/// narrower sub-range that also matches at position `depth` — a shrinking
/// window, not a fresh search over the whole dictionary — and each
/// comparison only looks at the one new character rather than
/// re-comparing the whole accumulated prefix. That's what makes this
/// cheap to call once per letter during move generation, unlike a
/// one-shot `is_prefix(&str)` which redoes all the earlier work every
/// time.
#[derive(Debug, Clone, Copy)]
pub struct SortedPrefixCursor<'a> {
    words: &'a [Vec<char>],
    depth: usize,
}

impl<'a> SortedPrefixCursor<'a> {
    fn char_at(word: &[char], index: usize) -> Option<char> {
        word.get(index).copied()
    }

    /// True if the prefix matched so far is itself a complete word. Since
    /// a strict prefix always sorts immediately before its extensions
    /// (`"A" < "AA" < "AAH"`), that's exactly the first entry of `words`,
    /// if it's the right length.
    pub fn is_word(&self) -> bool {
        self.words
            .first()
            .is_some_and(|word| word.len() == self.depth)
    }
}

impl<'a> PrefixCursor for SortedPrefixCursor<'a> {
    /// Narrows to words that also match `letter` at the next position(s),
    /// or `None` if nothing does — the prune signal: no word can possibly
    /// start with this prefix, so the caller doesn't need to keep
    /// building on top of it. A digraph `Letter` (e.g. Spanish's CH tile)
    /// resolves to two characters, so this narrows once per character in
    /// the grapheme — one placed tile can still consume more than one
    /// `depth` step, which is exactly what makes two different tilings of
    /// the same word (one tile spelling "ch", or two ordinary tiles
    /// spelling "c" then "h") both independently reach the same
    /// dictionary entry at the end.
    fn advance(&self, letter: Letter, alphabet: &Alphabet) -> Option<SortedPrefixCursor<'a>> {
        let grapheme = alphabet.to_grapheme(letter)?;
        let mut cursor = *self;
        for ch in grapheme.chars() {
            let target = Some(ch);
            let lo = cursor
                .words
                .partition_point(|word| Self::char_at(word, cursor.depth) < target);
            let hi = lo
                + cursor.words[lo..]
                    .partition_point(|word| Self::char_at(word, cursor.depth) == target);
            if lo == hi {
                return None;
            }
            cursor = SortedPrefixCursor {
                words: &cursor.words[lo..hi],
                depth: cursor.depth + 1,
            };
        }
        Some(cursor)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub static SOWPODS: LazyLock<WordListDictionary> = LazyLock::new(WordListDictionary::new);

#[cfg(not(target_arch = "wasm32"))]
pub static ENABLE2K: LazyLock<WordListDictionary> = LazyLock::new(WordListDictionary::new_enable2k);

#[cfg(not(target_arch = "wasm32"))]
pub static GERMAN: LazyLock<WordListDictionary> = LazyLock::new(WordListDictionary::new_german);

#[cfg(not(target_arch = "wasm32"))]
pub static SPANISH: LazyLock<WordListDictionary> = LazyLock::new(WordListDictionary::new_spanish);

#[cfg(not(target_arch = "wasm32"))]
pub fn is_word(word: &str) -> bool {
    SOWPODS.is_word(word)
}

/// Resolves a `VariantRules.language` value to its backing dictionary — the
/// one choke point every server/desktop/engine call site uses instead of
/// hardcoding `&*SOWPODS`, so a game's actual edition (not just whichever
/// dictionary happened to be wired in first) decides what it's validated
/// against.
#[cfg(not(target_arch = "wasm32"))]
pub fn dictionary_by_name(name: &str) -> Option<&'static WordListDictionary> {
    match name {
        "sowpods" => Some(&*SOWPODS),
        "enable2k" => Some(&*ENABLE2K),
        "german" => Some(&*GERMAN),
        "spanish" => Some(&*SPANISH),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Dictionary, ENABLE2K, GERMAN, PrefixCursor, SOWPODS, SPANISH, SortedPrefixCursor,
        dictionary_by_name, is_word,
    };
    use crate::model::{Alphabet, Letter, VariantRules};

    fn advance_all<'a>(
        mut cursor: SortedPrefixCursor<'a>,
        word: &str,
    ) -> Option<SortedPrefixCursor<'a>> {
        let alphabet = Alphabet::latin26();
        for ch in word.chars() {
            cursor = cursor.advance(Letter::from(ch), &alphabet)?;
        }
        Some(cursor)
    }

    #[test]
    fn sowpods_lookup_works() {
        assert!(is_word("ACE"));
        assert!(!is_word("NOTAWORD"));
    }

    #[test]
    fn dictionary_by_name_resolves_every_known_dictionary() {
        assert!(std::ptr::eq(
            dictionary_by_name("sowpods").unwrap(),
            &*SOWPODS
        ));
        assert!(std::ptr::eq(
            dictionary_by_name("enable2k").unwrap(),
            &*ENABLE2K
        ));
        assert!(std::ptr::eq(
            dictionary_by_name("german").unwrap(),
            &*GERMAN
        ));
        assert!(std::ptr::eq(
            dictionary_by_name("spanish").unwrap(),
            &*SPANISH
        ));
        assert!(dictionary_by_name("not-a-real-dictionary").is_none());
    }

    #[test]
    fn german_dictionary_recognizes_a_real_umlaut_word() {
        assert!(GERMAN.is_word("ÖL"));
        assert!(!GERMAN.is_word("NOTAWORD"));
    }

    #[test]
    fn german_prefix_cursor_finds_a_word_with_an_umlaut_letter() {
        let rules = VariantRules::german();
        let cursor = advance_all_with(GERMAN.prefix_cursor(), "ÖL", &rules.alphabet)
            .expect("ÖL should be reachable through the real German dictionary");
        assert!(cursor.is_word());
    }

    #[test]
    fn spanish_dictionary_recognizes_a_real_digraph_word() {
        assert!(SPANISH.is_word("CARRO"));
        assert!(SPANISH.is_word("CALLE"));
        assert!(SPANISH.is_word("CHICO"));
        assert!(!SPANISH.is_word("NOTAWORD"));
    }

    /// The core design decision this phase is built on: both tilings of
    /// the same word validate, since the dictionary is plain unannotated
    /// text and `advance` just narrows once per character in whichever
    /// grapheme was actually placed.
    #[test]
    fn spanish_prefix_cursor_accepts_both_tilings_of_the_same_word() {
        let rules = VariantRules::spanish();
        let rr = rules.alphabet.to_letter("RR").expect("RR is a real tile");
        let r = rules.alphabet.to_letter("R").expect("R is a real tile");

        let via_digraph_tile = advance_letters(
            SPANISH.prefix_cursor(),
            &[c(&rules, 'C'), c(&rules, 'A'), rr, c(&rules, 'O')],
        )
        .expect("CARRO should be reachable via the RR tile");
        assert!(via_digraph_tile.is_word());

        let via_two_ordinary_tiles = advance_letters(
            SPANISH.prefix_cursor(),
            &[c(&rules, 'C'), c(&rules, 'A'), r, r, c(&rules, 'O')],
        )
        .expect("CARRO should also be reachable via two separate R tiles");
        assert!(via_two_ordinary_tiles.is_word());
    }

    fn c(rules: &VariantRules, ch: char) -> Letter {
        rules
            .alphabet
            .to_letter(&ch.to_string())
            .expect("single ASCII letter should be a real tile")
    }

    fn advance_letters<'a>(
        mut cursor: SortedPrefixCursor<'a>,
        letters: &[Letter],
    ) -> Option<SortedPrefixCursor<'a>> {
        let alphabet = &VariantRules::spanish().alphabet;
        for &letter in letters {
            cursor = cursor.advance(letter, alphabet)?;
        }
        Some(cursor)
    }

    #[test]
    fn prefix_cursor_finds_real_words_letter_by_letter() {
        let cursor =
            advance_all(SOWPODS.prefix_cursor(), "LEXICON").expect("LEXICON should be reachable");
        assert!(cursor.is_word());
    }

    #[test]
    fn prefix_cursor_recognizes_a_prefix_that_is_not_yet_a_word() {
        // "LEXIC" isn't a word on its own, but it is a valid prefix
        // (LEXICA, LEXICON, ...) — the cursor should still exist, just
        // not report is_word.
        let cursor =
            advance_all(SOWPODS.prefix_cursor(), "LEXIC").expect("LEXIC should be a live prefix");
        assert!(!cursor.is_word());
    }

    #[test]
    fn prefix_cursor_prunes_a_dead_end() {
        assert!(advance_all(SOWPODS.prefix_cursor(), "ZZZZZ").is_none());
    }

    #[test]
    fn prefix_cursor_finds_short_words() {
        let cursor = advance_all(SOWPODS.prefix_cursor(), "ZA").expect("ZA should be reachable");
        assert!(cursor.is_word());
    }

    /// A synthetic, non-ASCII, >26-letter toy alphabet — not a real shipped
    /// language, just enough to prove the cursor is genuinely
    /// character-indexed rather than byte-indexed. `É` is two UTF-8 bytes;
    /// the old byte-indexed cursor would desync `depth` from "letters
    /// matched" the moment it appeared anywhere but the very end of a word,
    /// silently corrupting every comparison after it.
    fn toy_alphabet() -> Alphabet {
        Alphabet::from_chars(('A'..='Z').chain(['É', 'Ñ']))
    }

    fn toy_words(words: &[&str]) -> Vec<Vec<char>> {
        let mut sorted: Vec<Vec<char>> = words.iter().map(|w| w.chars().collect()).collect();
        sorted.sort_unstable();
        sorted
    }

    #[test]
    fn prefix_cursor_finds_a_word_with_a_multi_byte_letter_in_the_middle() {
        let sorted = toy_words(&["ÉCLAT", "ÉCLATS", "CAT"]);
        let alphabet = toy_alphabet();
        let root = SortedPrefixCursor {
            words: &sorted,
            depth: 0,
        };
        let cursor = advance_all_with(root, "ÉCLAT", &alphabet)
            .expect("ÉCLAT should be reachable despite É being multi-byte");
        assert!(cursor.is_word());
    }

    #[test]
    fn prefix_cursor_still_recognizes_a_live_prefix_past_a_multi_byte_letter() {
        let sorted = toy_words(&["ÉCLAT", "ÉCLATS", "CAT"]);
        let alphabet = toy_alphabet();
        let root = SortedPrefixCursor {
            words: &sorted,
            depth: 0,
        };
        // "ÉCLAT" is itself a complete word, but also a live prefix of
        // "ÉCLATS" — both should be true, same as the plain-ASCII
        // `prefix_cursor_recognizes_a_prefix_that_is_not_yet_a_word` case.
        let cursor = advance_all_with(root, "ÉCLAT", &alphabet).expect("should be reachable");
        assert!(cursor.is_word());
        let extended =
            advance_all_with(root, "ÉCLATS", &alphabet).expect("ÉCLATS should be reachable too");
        assert!(extended.is_word());
    }

    #[test]
    fn prefix_cursor_prunes_a_dead_end_with_a_multi_byte_letter() {
        let sorted = toy_words(&["ÉCLAT", "CAT"]);
        let alphabet = toy_alphabet();
        let root = SortedPrefixCursor {
            words: &sorted,
            depth: 0,
        };
        assert!(advance_all_with(root, "ÑOPE", &alphabet).is_none());
    }

    fn advance_all_with<'a>(
        mut cursor: SortedPrefixCursor<'a>,
        word: &str,
        alphabet: &Alphabet,
    ) -> Option<SortedPrefixCursor<'a>> {
        for ch in word.chars() {
            let letter = alphabet.to_letter(&ch.to_string())?;
            cursor = cursor.advance(letter, alphabet)?;
        }
        Some(cursor)
    }
}
