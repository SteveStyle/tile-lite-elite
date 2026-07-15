pub mod board;
pub mod cache;
pub mod dictionary;
pub mod format;
pub mod generate;
pub mod model;
pub mod score;
pub mod validate;

pub use board::{BoardCell, BoardState, EmptyCell, FilledCell};
pub use cache::{
    AnchorFlags, CachedCell, ConstrainedCrossCheck, CrossCheck, LineExtents, RuleCache,
};
pub use dictionary::{Dictionary, WordListDictionary};
#[cfg(not(target_arch = "wasm32"))]
pub use dictionary::{
    ENABLE2K, GERMAN, SOWPODS, SPANISH, dictionary_by_name, enable2k_word_list, german_word_list,
    is_word, sowpods_word_list, spanish_word_list,
};
pub use format::format_move_error;
pub use generate::MoveGenerator;
pub use model::{
    Alphabet, CrossWordPreview, Direction, Grapheme, Letter, LetterMask, MAX_ALPHABET_SIZE,
    MoveCandidate, MoveError, MovePreview, MoveScore, Position, Premium, Rack, Score, Tile,
    TilePlacement, ValidatedMove, VariantRules,
};
pub use validate::{GameState, MoveValidator, RulesEngine, RulesPosition};
