use lazy_static::lazy_static;

const WORD_FILE: &'static str = include_str!("sowpods.txt");

lazy_static! {
    pub static ref WORDS: std::collections::HashSet<&'static str> =
        WORD_FILE.split_whitespace().collect();
}

pub fn is_word(word: &str) -> bool {
    WORDS.contains(word)
}
