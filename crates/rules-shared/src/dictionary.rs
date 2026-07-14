use std::collections::HashSet;
use std::sync::LazyLock;

use crate::model::{Alphabet, Letter};

const SOWPODS_WORD_FILE: &str = include_str!("sowpods.txt");

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
    /// the speedup. `SowpodsDictionary` overrides this with
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

pub struct SowpodsDictionary {
    words: HashSet<&'static str>,
    /// Tokenized into characters (not left as raw UTF-8 bytes) and sorted
    /// once at construction — the backing store for `SortedPrefixCursor`,
    /// which narrows a sub-slice one *character* at a time instead of
    /// re-searching the whole dictionary at every step of move generation.
    /// SOWPODS itself is plain ASCII, so this only matters once a
    /// dictionary actually contains multi-byte UTF-8 words — but the
    /// search structure has to be character-indexed regardless of which
    /// dictionary it's built for, since byte-indexing silently desyncs
    /// the moment any word has a multi-byte character earlier in it.
    sorted_words: Vec<Vec<char>>,
}

impl SowpodsDictionary {
    pub fn new() -> Self {
        let words: HashSet<&'static str> = SOWPODS_WORD_FILE.split_whitespace().collect();
        let mut sorted_words: Vec<Vec<char>> =
            words.iter().map(|word| word.chars().collect()).collect();
        sorted_words.sort_unstable();
        Self { words, sorted_words }
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

impl Default for SowpodsDictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl Dictionary for SowpodsDictionary {
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
    /// Narrows to words that also match `letter` at the next position, or
    /// `None` if nothing does — the prune signal: no word can possibly
    /// start with this prefix, so the caller doesn't need to keep
    /// building on top of it.
    fn advance(&self, letter: Letter, alphabet: &Alphabet) -> Option<SortedPrefixCursor<'a>> {
        let target = alphabet.to_char(letter);
        let lo = self
            .words
            .partition_point(|word| Self::char_at(word, self.depth) < target);
        let hi = lo
            + self.words[lo..].partition_point(|word| Self::char_at(word, self.depth) == target);
        if lo == hi {
            None
        } else {
            Some(SortedPrefixCursor {
                words: &self.words[lo..hi],
                depth: self.depth + 1,
            })
        }
    }
}

pub static SOWPODS: LazyLock<SowpodsDictionary> = LazyLock::new(SowpodsDictionary::new);

pub fn is_word(word: &str) -> bool {
    SOWPODS.is_word(word)
}

#[cfg(test)]
mod tests {
    use super::{PrefixCursor, SOWPODS, SortedPrefixCursor, is_word};
    use crate::model::{Alphabet, Letter};

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
            let letter = alphabet.to_letter(ch)?;
            cursor = cursor.advance(letter, alphabet)?;
        }
        Some(cursor)
    }
}
