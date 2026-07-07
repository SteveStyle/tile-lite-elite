use rules_shared::{
    BoardCell, Direction, GameState, Letter, MoveCandidate, MoveGenerator, Rack, RulesEngine,
    SOWPODS, Tile, TilePlacement, VariantRules,
};

fn main() {
    let rules = VariantRules::official();
    let engine = RulesEngine {
        rules: &rules,
        dictionary: &*SOWPODS,
    };

    let mut state = GameState::new(&rules, &*SOWPODS);

    let mut rack = Rack::default();
    rack.add_letter(Letter::from('A'));
    rack.add_letter(Letter::from('T'));

    let candidate = MoveCandidate {
        start: rules_shared::Position::new(7, 7),
        direction: Direction::Horizontal,
        tiles: vec![
            TilePlacement {
                offset: 0,
                tile: Tile::Letter(Letter::from('A')),
            },
            TilePlacement {
                offset: 1,
                tile: Tile::Letter(Letter::from('T')),
            },
        ],
    };

    let preview = engine.preview_game_move(&state, Some(&rack), &candidate);
    println!("legal: {}", preview.legal);
    println!("main word: {}", preview.main_word);
    println!("score: {}", preview.total_score);

    let validated = engine
        .validate_game_move(&state, Some(&rack), &candidate)
        .expect("move should validate");
    engine
        .apply_move_to_game(&mut state, &validated)
        .expect("move should apply");

    println!(
        "center: {:?}",
        state.board.get(rules_shared::Position::new(7, 7))
    );
    println!(
        "next: {:?}",
        state.board.get(rules_shared::Position::new(8, 7))
    );

    let above_anchor = state.cache.cells
        [rules_shared::Position::new(7, 6).to_index(rules_shared::BoardState::WIDTH)]
    .anchor_flags;
    println!(
        "above anchor: horizontal={}, vertical={}",
        above_anchor.horizontal_anchor, above_anchor.vertical_anchor
    );

    let mut reply_rack = Rack::default();
    reply_rack.add_letter(Letter::from('C'));
    let generated = engine
        .enumerate_legal_moves(&state, &reply_rack)
        .collect::<Vec<_>>();
    println!("generated legal single-tile replies: {}", generated.len());

    match state.board.get(rules_shared::Position::new(7, 7)) {
        Some(BoardCell::Filled(_)) => println!("board updated"),
        _ => println!("board not updated"),
    }
}
