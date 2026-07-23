use rules_shared::{
    BoardCell, BoardState, FilledCell, GameState, Letter, MoveGenerator, Rack, RulesEngine,
    SOWPODS, VariantRules,
};

fn main() {
    let rules = VariantRules::official();
    let engine = RulesEngine {
        rules: &rules,
        dictionary: &*SOWPODS,
    };

    // Reconstruct the exact board from the reported bug: "LEXICON" placed
    // horizontally at row 7, starting column 7.
    let mut board = BoardState::new(&rules);
    let word = "LEXICON";
    for (i, ch) in word.chars().enumerate() {
        let pos = rules_shared::Position::new(7 + i as u8, 7);
        board.set(
            pos,
            BoardCell::Filled(FilledCell {
                letter: Letter::from(ch),
                is_blank: false,
            }),
        );
    }

    let state = GameState::from_board(board, &rules, &*SOWPODS);

    // Seat 0's actual rack from the bug report: D, D, K, R, S, U, blank
    let mut rack0 = Rack::default();
    for ch in ['D', 'D', 'K', 'R', 'S', 'U'] {
        rack0.add_letter(Letter::from(ch));
    }
    rack0.blanks = 1;

    // Seat 1's actual rack: A, E, F, N, O, T, blank
    let mut rack1 = Rack::default();
    for ch in ['A', 'E', 'F', 'N', 'O', 'T'] {
        rack1.add_letter(Letter::from(ch));
    }
    rack1.blanks = 1;

    for (label, rack) in [
        ("seat0 (D,D,K,R,S,U,?)", rack0),
        ("seat1 (A,E,F,N,O,T,?)", rack1),
    ] {
        let t0 = std::time::Instant::now();
        let candidates: Vec<_> = engine.enumerate_legal_moves(&state, &rack).collect();
        let gen_elapsed = t0.elapsed();
        println!(
            "{label}: {} raw candidates from generator ({gen_elapsed:?})",
            candidates.len()
        );

        let t1 = std::time::Instant::now();
        let mut valid_count = 0;
        let mut best: Option<(String, i16)> = None;
        for candidate in &candidates {
            if let Ok(validated) = engine.validate_game_move(&state, Some(&rack), candidate) {
                valid_count += 1;
                let score = validated.score.total;
                if best.as_ref().is_none_or(|(_, s)| score > *s) {
                    best = Some((validated.preview.main_word.clone(), score));
                }
            }
        }
        let validate_elapsed = t1.elapsed();
        println!(
            "  {valid_count} validated as legal ({validate_elapsed:?}, {:?}/candidate)",
            validate_elapsed / candidates.len().max(1) as u32
        );
        if let Some((word, score)) = best {
            println!("  best: {word} for {score}");
        } else {
            println!("  NO LEGAL MOVE FOUND");
        }
    }
}
