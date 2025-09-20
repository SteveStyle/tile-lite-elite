use std::io;
use std::str::FromStr;

use itertools::Itertools;
use scrabble::board::SCRABBLE_VARIANT_OFFICIAL;
use scrabble::board::SCRABBLE_VARIANT_WORDFEUD;
use scrabble::*;

use scrabble::board::ScrabbleVariant;
use scrabble::pos::Position;
use scrabble::tiles::TileList;
use scrabble::word_list::is_word;

//TO DO : use terminal escape codes to clear the screen
//https://stackoverflow.com/questions/2979383/c-clear-the-console
//https://stackoverflow.com/questions/4842424/list-of-ansi-color-escape-sequences
//https://stackoverflow.com/questions/2616906/how-do-i-output-coloured-text-to-a-linux-terminal

fn main() {
    menu_top();
}

fn menu_top() {
    let mut game: Option<Game> = None;
    loop {
        // clear the screen
        print!("\x1B[2J\x1B[1;1H");
        println!("1) Computer vs Computer");
        println!("2) Human vs Computer");
        println!("3) Human vs Human");
        println!("4) Ad hoc game");
        if game.is_some() {
            println!("5) Restart game");
        }
        println!("9) Look up word");
        println!("0) Exit");

        // read a char from stdin and compare to  1 to 4
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        match input.trim() {
            "1" => {
                if let Ok(g) = computer_vs_computer() {
                    game = Some(g);
                }
            }
            "2" => {
                if let Ok(g) = human_vs_computer() {
                    game = Some(g);
                }
            }
            "3" => {
                if let Ok(g) = human_vs_human() {
                    game = Some(g);
                }
            }
            "4" => {
                if let Ok(g) = ad_hoc_game() {
                    game = Some(g);
                }
            }
            "5" => {
                if let Some(g) = game.as_mut() {
                    g.restart();
                }
            }

            "9" => look_up_word(),
            "0" => break,
            _ => {
                println!("Invalid input");
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum UserCancelError {
    UserCancelled,
}

fn play_game(game: &mut Game) -> Result<(), UserCancelError> {
    while !game.is_over {
        println!("{}", game);
        match game.current_player().player_type {
            PlayerType::Human => {
                human_move(game)?;
            }
            PlayerType::Computer => {
                game.computer_move();
            }
        }
    }

    println!("{}", game);
    println!("GAME OVER!!!\n\n\n");
    if let Some(winner) = game.winner {
        println!("{} has won!\n\n", game.player_name[winner]);
    } else {
        println!("It's a draw!\n\n");
    }

    println!("The scores are:");
    (0..game.number_of_players)
        .map(|i| (game.player[i].score, game.player_name[i].clone()))
        .sorted()
        .rev()
        .for_each(|(score, name)| println!("{} scored {}", name, score));

    get_user_input_string_uppercase("Press enter to continue...", "", false)?;

    Ok(())
}

fn ad_hoc_game() -> Result<Game, UserCancelError> {
    let scrabble_variant = get_user_input_scrabble_variant()?;
    let number_of_players =
        get_user_input_integer("Enter number of players", "2", 2, 4, true)? as usize;
    let mut players = [Player::new(PlayerType::Human); 4];
    let mut player_name: Vec<String> = vec![];

    for i in 0..number_of_players {
        let player_type = get_user_input_player_type()?;
        let name = get_user_player_name(i)?;

        players[i as usize] = Player::new(player_type);
        player_name.push(name);
    }

    let mut game = Game::new(&scrabble_variant, number_of_players, players, player_name);
    play_game(&mut game)?;
    Ok(game)
}

fn computer_vs_computer() -> Result<Game, UserCancelError> {
    let scrabble_variant = get_user_input_scrabble_variant()?;
    let number_of_players = 2;
    let mut players = [Player::new(PlayerType::Human); 4];
    let mut player_name: Vec<String> = vec![];

    for i in 0..number_of_players {
        let player_type = PlayerType::Computer;
        let name = format!("Player {}", i + 1);

        players[i as usize] = Player::new(player_type);
        player_name.push(name);
    }

    let mut game = Game::new(&scrabble_variant, number_of_players, players, player_name);

    play_game(&mut game)?;
    Ok(game)
}

fn human_vs_computer() -> Result<Game, UserCancelError> {
    let scrabble_variant = get_user_input_scrabble_variant()?;
    let human_name = get_user_input_string("What is your name?", "Human", true)?;
    let human_first = get_user_input_bool(
        &format!("Do you want to go first {}?", &human_name),
        "Y",
        true,
    )?;
    let number_of_players = 2;
    let human_player = Player::new(PlayerType::Human);
    let computer_player = Player::new(PlayerType::Computer);
    let player_name = if human_first {
        vec![human_name, "Computer".to_string()]
    } else {
        vec!["Computer".to_string(), human_name]
    };

    let players = if human_first {
        [
            human_player,
            computer_player,
            computer_player,
            computer_player,
        ]
    } else {
        [
            computer_player,
            human_player,
            computer_player,
            computer_player,
        ]
    };

    let mut game = Game::new(&scrabble_variant, number_of_players, players, player_name);

    play_game(&mut game)?;
    Ok(game)
}

fn human_move(game: &mut Game) -> Result<(), UserCancelError> {
    if game.is_over {
        return Ok(());
    }
    let mut debug_info = String::new();

    println!("{}", game);
    println!("{}", debug_info);
    println!("What would you like to do?");
    println!("1) Play a word");
    println!("2) Pass");
    println!("3) Exchange tiles");
    println!("4) Show cell info");
    println!("5) Computer suggestion");
    println!("9) Resign Game");
    println!("0) Quit Program");

    // read a char from stdin and compare to  1 to 4
    let menu_choice = get_user_input_string("Enter choice: ", "0", true).unwrap_or("0".to_string());
    match menu_choice.trim() {
        "1" => {
            play_word(game).unwrap();
        }
        "2" => {
            game.pass();
        }
        "3" => {
            if let Ok(tile_list) = get_user_input_tile_list("Enter tiles to exchange") {
                if let Err(e) = game.exchange_tiles(&tile_list) {
                    println!("Error: {:?}", e);
                }
            }
        }

        "4" => {
            //take a list of cells and show the info for each
            if let Ok(position) = get_user_input_position("Enter cell", "") {
                let cell = game.board.get_cell_pos(position);
                debug_info.push_str(format!("Cell {} {:?}", position.to_string(), cell).as_str());
                debug_info.push('\n');
            }
        }
        "5" => {
            computer_suggestion(game).unwrap();
        }
        "0" => {
            if get_user_input_bool(
                "Are you sure you want to quit the program? (Y or N)",
                "N",
                true,
            )
            .unwrap_or(false)
            {
                panic!("User quit");
            }
        }
        "9" => {
            if get_user_input_bool(
                "Are you sure you want to resign the game? (Y or N)",
                "N",
                true,
            )
            .unwrap_or(false)
            {
                game.quit();
            }
        }
        _ => println!("Invalid input"),
    }

    Ok(())
}

fn computer_suggestion(game: &mut Game) -> Result<(), UserCancelError> {
    let mut game_copy = game.clone();
    game_copy.computer_move();
    println!("{}", game_copy);
    let last_move = game_copy.last_move().unwrap().clone();
    match last_move.detail.clone() {
        GameMoveRecordDetail::Move { score, word, .. } => {
            println!(
                "Suggested move: you could play {} for {} points.",
                word, score
            );
        }
        GameMoveRecordDetail::Exchange { tiles } => {
            println!("Suggestion: you could exchange these tiles, {}", tiles);
        }
        GameMoveRecordDetail::Pass => {
            println!("Suggestion: you could pass...");
        }
    }

    if get_user_input_bool("Use this move?", "Y", true)? {
        *game = game_copy;
    }
    Ok(())
}

fn play_word(game: &mut Game) -> Result<(), UserCancelError> {
    // capture the starting cell

    loop {
        let starting_position = get_user_input_position("Enter starting position", "H8")?;
        let direction = get_user_input_direction("Which direction? ('H' or 'V')")?;
        let tiles = get_user_input_tile_list("Enter tiles to play:  ")?;

        match game.human_move(starting_position, direction, &tiles) {
            Ok(_) => {
                println!("{}", game);
                break;
            }
            Err(e) => {
                println!("Move rejected: {:?}", e);
            }
        }
    }

    Ok(())
}

fn human_vs_human() -> Result<Game, UserCancelError> {
    let mut game = Game::new(
        &SCRABBLE_VARIANT_OFFICIAL,
        2,
        [Player::new(PlayerType::Human); 4],
        vec![
            "Player 1".to_string(),
            "Player 2".to_string(),
            "Player 3".to_string(),
            "Player 4".to_string(),
        ],
    );

    play_game(&mut game).unwrap();

    Ok(game)
}

fn look_up_word() {
    if let Ok(word) = get_user_input_string_uppercase("Enter a word:  ", "0", true) {
        let result = is_word(&word);
        match result {
        true => println!("{word} is a word.  Follow the link for the definition:  https://www.collinsdictionary.com/dictionary/english/{word}"),
        false => println!("{} is not a word", word),
    }
    }
}

fn get_user_input_string(
    caption: &str,
    default: &str,
    show_default: bool,
) -> Result<String, UserCancelError> {
    println!(
        "{}\n(0 to cancel{}{})",
        caption,
        if show_default { ", return for " } else { "" },
        if show_default { default } else { "" },
    );
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input = input.trim().to_string();
    if input == "" {
        input = default.to_string();
    }
    match input.as_str() {
        "0" => Err(UserCancelError::UserCancelled),
        _ => Ok(input),
    }
}

fn get_user_player_name(player_number: usize) -> Result<String, UserCancelError> {
    let default = format!("Player {}", player_number);
    get_user_input_string(
        &format!("Enter name for player {}", player_number),
        &default,
        true,
    )
}

fn get_user_input_string_uppercase(
    caption: &str,
    default: &str,
    show_default: bool,
) -> Result<String, UserCancelError> {
    println!(
        "{}\n(0 to cancel{}{})",
        caption,
        if show_default { ", return for " } else { "" },
        if show_default { default } else { "" },
    );
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input = input.trim().to_uppercase();
    if input == "" {
        input = default.to_string();
    }
    match input.as_str() {
        "0" => Err(UserCancelError::UserCancelled),
        _ => Ok(input),
    }
}

fn get_user_input_position(caption: &str, default: &str) -> Result<Position, UserCancelError> {
    loop {
        let input = get_user_input_string_uppercase(caption, default, true)?;
        match Position::from_str(&input) {
            Ok(p) => return Ok(p),
            Err(e) => println!("{}", e),
        }
    }
}

fn get_user_input_tile_list(caption: &str) -> Result<TileList, UserCancelError> {
    loop {
        let input = get_user_input_string_uppercase(caption, "0", false)?;
        match TileList::try_from(input.as_str()) {
            Ok(t) => return Ok(t),
            Err(e) => println!("{:?}", e),
        }
    }
}

fn get_user_input_integer(
    caption: &str,
    default: &str,
    min: i32,
    max: i32,
    show_default: bool,
) -> Result<i32, UserCancelError> {
    loop {
        let input = get_user_input_string_uppercase(caption, default, show_default)?;
        match input.parse::<i32>() {
            Ok(i) => {
                if i >= min && i <= max {
                    return Ok(i);
                } else {
                    println!("Please enter a number between {} and {}", min, max);
                }
            }
            Err(e) => println!("{}", e),
        }
    }
}

fn get_user_input_direction(caption: &str) -> Result<Direction, UserCancelError> {
    loop {
        let input = get_user_input_string_uppercase(caption, "H", true)?;
        match Direction::try_from(input.as_str()) {
            Ok(d) => return Ok(d),
            Err(e) => println!("{}", e),
        }
    }
}

fn get_user_input_bool(
    caption: &str,
    default: &str,
    show_default: bool,
) -> Result<bool, UserCancelError> {
    loop {
        let input = get_user_input_string_uppercase(caption, default, show_default)?;
        match input.as_str() {
            "Y" => return Ok(true),
            "N" => return Ok(false),
            _ => println!("Please enter 'Y' or 'N'"),
        }
    }
}

fn get_user_input_scrabble_variant() -> Result<&'static ScrabbleVariant, UserCancelError> {
    loop {
        let input = get_user_input_string_uppercase(
            "Which scrabble ruleset should we use?\n1) Official\n2) Wordfeud",
            "1",
            true,
        )?;
        match input.as_str() {
            "1" => return Ok(&SCRABBLE_VARIANT_OFFICIAL),
            "2" => return Ok(&SCRABBLE_VARIANT_WORDFEUD),
            _ => println!("Please enter '1' or '2'"),
        }
    }
}

fn get_user_input_player_type() -> Result<PlayerType, UserCancelError> {
    loop {
        let input = get_user_input_string_uppercase(
            "Which player type should we use?\n1) Human\n2) Computer",
            "1",
            true,
        )?;
        match input.as_str() {
            "1" => return Ok(PlayerType::Human),
            "2" => return Ok(PlayerType::Computer),
            _ => println!("Please enter '1' or '2'"),
        }
    }
}

/*
fn test_anagram_version() {
    let timer = Timer::new("show_totals()");
    let mut letters = [0; 26];
    letters[0] = 2;
    letters[1] = 1;
    letters[2] = 1;
    letters[3] = 1;
    letters[4] = 2;

    let hs = word_list::generate_anagrams(&tiles::TileBag { letters, blanks: 0 });
    println!("hs: {:?}", hs);
    let result = hs.contains("CEDE");
    println!("result: {}", result);
    let timer = Timer::new("test");
    let mut result = hs.contains("BADE")
        && hs.contains("ABED")
        && hs.contains("BEAD")
        && hs.contains("CADEE")
        && hs.contains("ACE")
        && hs.contains("ECAD");
    for i in 0..10000000 {
        result = hs.contains("BADE")
            && hs.contains("ABED")
            && hs.contains("BEAD")
            && hs.contains("CADEE")
            && hs.contains("ACE")
            && hs.contains("ECAD");
    }
    let elapsed = timer.elapsed();
    println!(
        "Time taken anagram version: {} seconds",
        elapsed.as_secs_f64()
    );
    println!("result: {}", result);
}

fn test_full_version() {
    let timer = Timer::new("show_totals()");

    let result = word_list::is_word("CEDE");
    println!("result: {}", result);
    let timer = Timer::new("test full version");
    let mut result = word_list::is_word("BADE")
        && word_list::is_word("ABED")
        && word_list::is_word("BEAD")
        && word_list::is_word("CADEE")
        && word_list::is_word("ACE")
        && word_list::is_word("ECAD");
    for i in 0..10000000 {
        result = word_list::is_word("BADE")
            && word_list::is_word("ABED")
            && word_list::is_word("BEAD")
            && word_list::is_word("CADEE")
            && word_list::is_word("ACE")
            && word_list::is_word("ECAD");
    }
    let elapsed = timer.elapsed();
    println!("Time taken full version: {} seconds", elapsed.as_secs_f64());
    println!("result: {}", result);
}
*/
