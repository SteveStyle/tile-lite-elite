use dioxus::prelude::*;

const BOARD_SIZE: usize = 15; // Full 15x15 Scrabble board
const TILES: [char; 26] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

#[derive(Clone, Copy, PartialEq)]
enum SquareType {
    Normal,
    DoubleWord,
    TripleWord,
    DoubleLetter,
    TripleLetter,
    Center,
}

#[derive(Clone, Copy, PartialEq)]
enum BoardLayout {
    Traditional,
    Wordfeud,
}

fn get_square_type(row: usize, col: usize, layout: BoardLayout) -> SquareType {
    // Center square
    if row == 7 && col == 7 {
        return SquareType::Center;
    }

    match layout {
        BoardLayout::Traditional => get_traditional_square_type(row, col),
        BoardLayout::Wordfeud => get_wordfeud_square_type(row, col),
    }
}
fn get_traditional_square_type(row: usize, col: usize) -> SquareType {
    // Traditional Scrabble layout (US/UK/International standard)

    // Triple Word Score squares
    const TRIPLE_WORD: [(usize, usize); 8] = [
        (0, 0),
        (0, 7),
        (0, 14),
        (7, 0),
        (7, 14),
        (14, 0),
        (14, 7),
        (14, 14),
    ];
    if TRIPLE_WORD.contains(&(row, col)) {
        return SquareType::TripleWord;
    }

    // Double Word Score squares
    const DOUBLE_WORD: [(usize, usize); 16] = [
        (1, 1),
        (1, 13),
        (2, 2),
        (2, 12),
        (3, 3),
        (3, 11),
        (4, 4),
        (4, 10),
        (10, 4),
        (10, 10),
        (11, 3),
        (11, 11),
        (12, 2),
        (12, 12),
        (13, 1),
        (13, 13),
    ];
    if DOUBLE_WORD.contains(&(row, col)) {
        return SquareType::DoubleWord;
    }

    // Triple Letter Score squares
    const TRIPLE_LETTER: [(usize, usize); 12] = [
        (1, 5),
        (1, 9),
        (5, 1),
        (5, 5),
        (5, 9),
        (5, 13),
        (9, 1),
        (9, 5),
        (9, 9),
        (9, 13),
        (13, 5),
        (13, 9),
    ];
    if TRIPLE_LETTER.contains(&(row, col)) {
        return SquareType::TripleLetter;
    }

    // Double Letter Score squares
    const DOUBLE_LETTER: [(usize, usize); 24] = [
        (0, 3),
        (0, 11),
        (2, 6),
        (2, 8),
        (3, 0),
        (3, 7),
        (3, 14),
        (6, 2),
        (6, 6),
        (6, 8),
        (6, 12),
        (7, 3),
        (7, 11),
        (8, 2),
        (8, 6),
        (8, 8),
        (8, 12),
        (11, 0),
        (11, 7),
        (11, 14),
        (12, 6),
        (12, 8),
        (14, 3),
        (14, 11),
    ];
    if DOUBLE_LETTER.contains(&(row, col)) {
        return SquareType::DoubleLetter;
    }

    SquareType::Normal
}

fn get_wordfeud_square_type(row: usize, col: usize) -> SquareType {
    // Triple Word Score squares - Wordfeud pattern
    const TRIPLE_WORD: [(usize, usize); 8] = [
        (0, 0),
        (0, 7),
        (0, 14),
        (7, 0),
        (7, 14),
        (14, 0),
        (14, 7),
        (14, 14),
    ];
    if TRIPLE_WORD.contains(&(row, col)) {
        return SquareType::TripleWord;
    }

    // Double Word Score squares - Wordfeud has different pattern
    const DOUBLE_WORD: [(usize, usize); 17] = [
        (1, 1),
        (1, 13),
        (2, 2),
        (2, 12),
        (3, 3),
        (3, 11),
        (4, 4),
        (4, 10),
        (7, 7),
        (10, 4),
        (10, 10),
        (11, 3),
        (11, 11),
        (12, 2),
        (12, 12),
        (13, 1),
        (13, 13),
    ];
    if DOUBLE_WORD.contains(&(row, col)) {
        return SquareType::DoubleWord;
    }

    // Triple Letter Score squares - Wordfeud pattern
    const TRIPLE_LETTER: [(usize, usize); 12] = [
        (1, 5),
        (1, 9),
        (5, 1),
        (5, 5),
        (5, 9),
        (5, 13),
        (9, 1),
        (9, 5),
        (9, 9),
        (9, 13),
        (13, 5),
        (13, 9),
    ];
    if TRIPLE_LETTER.contains(&(row, col)) {
        return SquareType::TripleLetter;
    }

    // Double Letter Score squares - Wordfeud has more double letter squares
    const DOUBLE_LETTER: [(usize, usize); 32] = [
        (0, 3),
        (0, 11),
        (2, 6),
        (2, 8),
        (3, 0),
        (3, 7),
        (3, 14),
        (6, 2),
        (6, 6),
        (6, 8),
        (6, 12),
        (7, 3),
        (7, 11),
        (8, 2),
        (8, 6),
        (8, 8),
        (8, 12),
        (11, 0),
        (11, 7),
        (11, 14),
        (12, 6),
        (12, 8),
        (14, 3),
        (14, 11),
        // Additional Wordfeud double letter squares
        (4, 6),
        (4, 8),
        (6, 4),
        (6, 10),
        (8, 4),
        (8, 10),
        (10, 6),
        (10, 8),
    ];
    if DOUBLE_LETTER.contains(&(row, col)) {
        return SquareType::DoubleLetter;
    }

    SquareType::Normal
}
fn get_square_style(square_type: SquareType) -> &'static str {
    match square_type {
        SquareType::Normal => {
            "width:30px;height:30px;border:1px solid #666;text-align:center;font-size:14px;cursor:pointer;background:#f5f5dc;"
        }
        SquareType::DoubleWord => {
            "width:30px;height:30px;border:1px solid #666;text-align:center;font-size:14px;cursor:pointer;background:#ffb6c1;color:#8b0000;font-weight:bold;"
        }
        SquareType::TripleWord => {
            "width:30px;height:30px;border:1px solid #666;text-align:center;font-size:14px;cursor:pointer;background:#ff4500;color:white;font-weight:bold;"
        }
        SquareType::DoubleLetter => {
            "width:30px;height:30px;border:1px solid #666;text-align:center;font-size:14px;cursor:pointer;background:#87ceeb;color:#000080;font-weight:bold;"
        }
        SquareType::TripleLetter => {
            "width:30px;height:30px;border:1px solid #666;text-align:center;font-size:14px;cursor:pointer;background:#0000ff;color:white;font-weight:bold;"
        }
        SquareType::Center => {
            "width:30px;height:30px;border:1px solid #666;text-align:center;font-size:14px;cursor:pointer;background:#ffd700;color:#8b0000;font-weight:bold;"
        }
    }
}

fn get_square_label(square_type: SquareType) -> &'static str {
    match square_type {
        SquareType::Normal => "",
        SquareType::DoubleWord => "2W",
        SquareType::TripleWord => "3W",
        SquareType::DoubleLetter => "2L",
        SquareType::TripleLetter => "3L",
        SquareType::Center => "★",
    }
}

#[component]
pub fn scrabble_board() -> Element {
    let mut board = use_signal(|| vec![vec![' '; BOARD_SIZE]; BOARD_SIZE]);
    let mut selected_tile = use_signal(|| None::<char>);
    let mut current_layout = use_signal(|| BoardLayout::Traditional);

    let board_state = board.read();

    rsx! {
        div { class: "scrabble-container",
            h2 { "Full Size Scrabble Board (15x15)" }
            // Layout selector
            div {
                class: "layout-selector",
                style: "margin:20px auto;text-align:center;",
                h3 { "Choose Board Layout:" }
                div { style: "display:flex;justify-content:center;gap:10px;margin:10px;",
                    button {
                        onclick: move |_| current_layout.set(BoardLayout::Traditional),
                        style: if *current_layout.read() == BoardLayout::Traditional { "padding:10px 20px;font-size:16px;background:#4CAF50;color:white;border:none;border-radius:5px;cursor:pointer;" } else { "padding:10px 20px;font-size:16px;background:#f0f0f0;color:#333;border:1px solid #ccc;border-radius:5px;cursor:pointer;" },
                        "Traditional"
                    }
                    button {
                        onclick: move |_| current_layout.set(BoardLayout::Wordfeud),
                        style: if *current_layout.read() == BoardLayout::Wordfeud { "padding:10px 20px;font-size:16px;background:#4CAF50;color:white;border:none;border-radius:5px;cursor:pointer;" } else { "padding:10px 20px;font-size:16px;background:#f0f0f0;color:#333;border:1px solid #ccc;border-radius:5px;cursor:pointer;" },
                        "Wordfeud"
                    }
                }
            }
            div { class: "scrabble-board",
                table { style: "border-collapse:collapse;margin:20px auto;",
                    tbody {
                        {
                            board_state
                                .iter()
                                .enumerate()
                                .map(|(row_idx, row)| {
                                    rsx! {
                                        tr {
                                            {
                                                row.iter()
                                                    .enumerate()
                                                    .map(|(col_idx, &cell)| {
                                                        let layout = *current_layout.read();
                                                        let square_type = get_square_type(row_idx, col_idx, layout);
                                                        let square_style = get_square_style(square_type);
                                                        let square_label = get_square_label(square_type);
                                                        rsx! {
                                                            td {
                                                                onclick: move |_| {
                                                                    let tile_opt = selected_tile.read().clone();
                                                                    if let Some(tile) = tile_opt {
                                                                        let mut new_board = board.read().clone();
                                                                        new_board[row_idx][col_idx] = tile;
                                                                        board.set(new_board);
                                                                        selected_tile.set(None);
                                                                    }
                                                                },
                                                                style: "{square_style}",
                                                                if cell != ' ' {
                                                                    div { style: "background:#f4e4bc;border:1px solid #8b4513;border-radius:3px;width:100%;height:100%;display:flex;align-items:center;justify-content:center;font-weight:bold;color:#2f4f4f;",
                                                                        "{cell}"
                                                                    }
                                                                } else {
                                                                    div { style: "font-size:10px;line-height:1;", "{square_label}" }
                                                                }
                                                            }
                                                        }
                                                    })
                                            }
                                        }
                                    }
                                })
                        }
                    }
                }
            }

            div {
                class: "tile-rack",
                style: "margin:20px auto;text-align:center;max-width:600px;",
                h3 { "Letter Tiles" }
                div { style: "display:flex;flex-wrap:wrap;justify-content:center;gap:5px;",
                    {
                        TILES
                            .iter()
                            .map(|&tile| {
                                let is_selected = *selected_tile.read() == Some(tile);
                                rsx! {
                                    button {
                                        onclick: move |_| selected_tile.set(Some(tile)),
                                        style: "margin:2px;padding:8px 12px;font-size:16px;font-weight:bold;background:#f4e4bc;border:2px solid #8b4513;border-radius:5px;cursor:pointer;color:#2f4f4f;min-width:40px;",
                                        class: if is_selected { "selected-tile" } else { "" },
                                        "{tile}"
                                    }
                                }
                            })
                    }
                }
            }

            div { style: "margin:20px auto;text-align:center;font-size:18px;",
                "Selected tile: "
                {
                    let tile = selected_tile.read();
                    match *tile {
                        Some(t) => format!("{}", t),
                        None => "None".to_string(),
                    }
                }
            }
            div { style: "margin:20px auto;text-align:center;",
                button {
                    onclick: move |_| {
                        board.set(vec![vec![' '; BOARD_SIZE]; BOARD_SIZE]);
                        selected_tile.set(None);
                    },
                    style: "padding:10px 20px;font-size:16px;background:#ff6b6b;color:white;border:none;border-radius:5px;cursor:pointer;margin:10px;",
                    "Clear Board"
                }
            }
            div { style: "margin:20px auto;text-align:center;font-style:italic;color:#666;",
                "Current layout: "
                {
                    match *current_layout.read() {
                        BoardLayout::Traditional => "Traditional Scrabble (US/UK/International)",
                        BoardLayout::Wordfeud => "Wordfeud (More double letter squares)",
                    }
                }
            }

            div {
                class: "legend",
                style: "margin:20px auto;max-width:600px;text-align:center;",
                h3 { "Premium Squares Legend" }
                div { style: "display:flex;flex-wrap:wrap;justify-content:center;gap:10px;font-size:12px;",
                    div { style: "background:#ffb6c1;padding:5px;border:1px solid #666;",
                        "2W - Double Word"
                    }
                    div { style: "background:#ff4500;color:white;padding:5px;border:1px solid #666;",
                        "3W - Triple Word"
                    }
                    div { style: "background:#87ceeb;padding:5px;border:1px solid #666;",
                        "2L - Double Letter"
                    }
                    div { style: "background:#0000ff;color:white;padding:5px;border:1px solid #666;",
                        "3L - Triple Letter"
                    }
                    div { style: "background:#ffd700;padding:5px;border:1px solid #666;",
                        "★ - Center Star"
                    }
                }
            }
        }
    }
}
