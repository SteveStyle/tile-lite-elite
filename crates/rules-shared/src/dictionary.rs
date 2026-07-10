use std::collections::HashSet;
use std::sync::LazyLock;

const SOWPODS_WORD_FILE: &str = include_str!("sowpods.txt");

pub trait Dictionary {
    fn is_word(&self, word: &str) -> bool;
}

pub struct SowpodsDictionary {
    words: HashSet<&'static str>,
}

impl SowpodsDictionary {
    pub fn new() -> Self {
        Self {
            words: SOWPODS_WORD_FILE.split_whitespace().collect(),
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
}

pub static SOWPODS: LazyLock<SowpodsDictionary> = LazyLock::new(SowpodsDictionary::new);

pub fn is_word(word: &str) -> bool {
    SOWPODS.is_word(word)
}

#[cfg(test)]
mod tests {
    use super::is_word;

    #[test]
    fn sowpods_lookup_works() {
        assert!(is_word("ACE"));
        assert!(!is_word("NOTAWORD"));
    }
}
