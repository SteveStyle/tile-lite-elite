use crate::model::MoveScore;

pub trait MoveScorer {
    fn empty_score(&self) -> MoveScore {
        MoveScore {
            total: 0,
            main_word_score: 0,
            cross_word_score: 0,
            bingo_bonus: 0,
        }
    }
}
