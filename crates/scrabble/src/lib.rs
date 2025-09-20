use std::{
    fmt::{Display, Formatter},
    ops::Neg,
};

use board::{CellValue, MoveCell};
use pos::Position;

use tiles::{Tile, TileBag, TileList, ALPHABET};
use utils::Timer;

use crate::word_list::is_word;
//use word_list::{is_word, LETTER_PREFIXES, LETTER_SUFFIXES};

pub mod board;
pub mod tiles;
pub mod word_list;

pub mod pos;
pub mod utils;

pub type TScore = i16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlayerType {
    Human,
    Computer,
}
#[derive(Debug, Clone, Copy)]
pub struct Player {
    pub player_type: PlayerType,
    pub rack: tiles::TileBag,
    pub score: TScore,
    pub passes: u8,
    pub exchanges: u8,
    pub timer: Timer,
    pub last_move: usize,
}

impl Player {
    pub fn new(player_type: PlayerType) -> Self {
        Self {
            player_type,
            rack: TileBag::new_empty(),
            score: 0,
            passes: 0,
            exchanges: 0,
            timer: Timer::new(false),
            last_move: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    Horizontal,
    Vertical,
}

impl TryFrom<char> for Direction {
    type Error = String;
    fn try_from(value: char) -> Result<Self, Self::Error> {
        match value {
            'h' => Ok(Direction::Horizontal),
            'v' => Ok(Direction::Vertical),
            _ => Err(format!("Invalid direction: {}", value)),
        }
    }
}

impl TryFrom<&str> for Direction {
    type Error = String;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "H" => Ok(Direction::Horizontal),
            "V" => Ok(Direction::Vertical),
            _ => Err(format!("Invalid direction: {}", value)),
        }
    }
}

impl Display for Direction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Horizontal => write!(f, "Horizontal"),
            Direction::Vertical => write!(f, "Vertical"),
        }
    }
}

impl Neg for Direction {
    type Output = Direction;
    fn neg(self) -> Self::Output {
        match self {
            Direction::Horizontal => Direction::Vertical,
            Direction::Vertical => Direction::Horizontal,
        }
    }
}

pub struct MovePositionMap {
    positions: Vec<Position>,
    position_types: Vec<MoveCell>,
}

impl MovePositionMap {
    pub fn add(&mut self, position: Position, position_type: MoveCell) {
        self.positions.push(position);
        self.position_types.push(position_type);
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GameMove {
    //    pub player: usize,
    pub starting_position: Position,
    pub direction: Direction,
    pub tiles: TileList,
    pub score: TScore,
}

impl GameMove {
    pub fn new(
        starting_position: Position,
        direction: Direction,
        tiles: TileList,
        score: TScore,
    ) -> Self {
        Self {
            starting_position,
            direction,
            tiles,
            score,
        }
    }
    pub fn get_main_word_start_pos(&self, board: &board::Board) -> Position {
        let mut current_position = self.starting_position;
        while let Some(next_position) = current_position.try_step_backward(self.direction) {
            if board.get_cell_pos(next_position).is_empty() {
                break;
            }
            current_position = next_position;
        }
        current_position
    }
}

#[derive(Debug, Clone)]
pub enum GameMoveRecordDetail {
    Move {
        starting_position: Position,
        direction: Direction,
        tiles: TileList,
        score: i16,
        word: String,
    },
    Exchange {
        tiles: TileList,
    },
    Pass,
}
#[derive(Debug, Clone)]
pub struct GameMoveRecord {
    pub player: usize,
    pub player_name: String,
    pub detail: GameMoveRecordDetail,
}

impl Display for GameMoveRecordDetail {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GameMoveRecordDetail::Move { score, word, .. } => {
                write!(f, "{:3} points {}", score, word)
            }
            GameMoveRecordDetail::Exchange { tiles } => {
                write!(f, "Exchange: tiles - {}", tiles)
            }
            GameMoveRecordDetail::Pass => write!(f, "Pass"),
        }
    }
}

impl Display for GameMoveRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail)
    }
}

#[derive(Debug, Clone)]
pub struct Game {
    //  fixed part of the game
    scrabble_variant: &'static board::ScrabbleVariant,
    //  mutable part of the game
    pub number_of_players: usize,
    pub player: [Player; 4],
    pub player_name: Vec<String>,
    pub board: board::Board,
    pub bag: tiles::TileBag,
    pub current_player: usize, // index into `players`
    pub first_move: bool,
    pub is_over: bool,
    pub last_player_to_play: Option<usize>,
    pub winner: Option<usize>,
    pub non_scoring_plays: u8,
    //  history of moves
    pub moves: Vec<GameMoveRecord>,
    //local_word_list: HashSet<String>,
}

impl Display for Game {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut recent_words: Vec<String> = vec![];
        write!(f, "\x1B[2J\x1B[1;1H")?;
        for i in 0..self.number_of_players {
            writeln!(
                f,
                "{:10}: {:4} points {:7.2} seconds   {}",
                self.player_name[i],
                self.player[i].score,
                self.player[i].timer.elapsed().as_secs_f64(),
                if let Some(last_move) = self.moves.get(self.player[i].last_move) {
                    if let GameMoveRecordDetail::Move { word, .. } = &last_move.detail {
                        recent_words.push(word.clone());
                    };
                    format!("last move {}", last_move)
                } else {
                    "".to_string()
                },
            )?;
        }

        writeln!(f, "{}", self.board)?;
        writeln!(f, "Recent words:",)?;

        for word in recent_words.iter() {
            writeln!(
                f,
                "https://www.collinsdictionary.com/dictionary/english/{}",
                word
            )?;
        }

        writeln!(
            f,
            "Rack: {:7}    Bag: {} tiles",
            self.current_player().rack,
            self.bag.count()
        )?;
        Ok(())
    }
}

// implement an error type for my apply move function
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveError {
    InvalidMove,
    InvalidWord(String),
    InvalidPosition,
    InvalidDirection,
    TilesDonNotFit,
    TilesDoNotConnect,
    LetterNotAllowedInPosition,
    TilesNotInRack(Tile),
    BlankTileNotActingAsLetter,
    NoTilesPassed,
    NotEnoughSpaceForTiles(u8),
    BlockingEmptyCellFound,
    NotEnoughTilesInBag,
    InvalidTile(char),
}

impl Game {
    pub fn new(
        scrabble_variant: &'static board::ScrabbleVariant,
        number_of_players: usize,
        players: [Player; 4],
        player_name: Vec<String>,
    ) -> Self {
        is_word("the"); //  just to make sure the word list is loaded

        let bag = tiles::TileBag::new(scrabble_variant);

        let board = board::Board::new(scrabble_variant);
        let next_player = 0;
        let moves = Vec::new();
        //let local_word_list = word_list::generate_anagrams(&players[1].rack);
        let mut game = Self {
            scrabble_variant,
            number_of_players: number_of_players,
            player: players,
            player_name,
            board,
            bag,
            current_player: next_player,
            first_move: true,
            is_over: false,
            last_player_to_play: None,
            winner: None,
            non_scoring_plays: 0,
            moves,
            //  local_word_list,
        };

        for i in 0..number_of_players {
            game.player[i].rack.fill_rack(&mut game.bag);
        }

        game.current_player_mut().timer.start();
        game
    }

    pub fn restart(&mut self) {
        self.bag = tiles::TileBag::new(self.scrabble_variant);
        self.board = board::Board::new(self.scrabble_variant);
        self.first_move = true;
        self.is_over = false;
        self.last_player_to_play = None;
        self.winner = None;
        self.non_scoring_plays = 0;
        self.moves = Vec::new();
        for i in 0..self.number_of_players {
            self.player[i].score = 0;
            self.player[i].passes = 0;
            self.player[i].exchanges = 0;
            self.player[i].rack = TileBag::new_empty();
            self.player[i].rack.fill_rack(&mut self.bag);
            self.player[i].timer = Timer::new(false);
            self.player[i].last_move = 0;
        }
        self.current_player = 0;
        self.current_player_mut().timer.start();
    }

    pub fn current_player(&self) -> &Player {
        &(self.player[self.current_player])
    }

    pub fn current_player_mut(&mut self) -> &mut Player {
        &mut (self.player[self.current_player])
    }

    // returns the positions of each tile and the positions of filled cells
    pub fn validate_position(
        &self,
        starting_position: Position,
        direction: Direction,
    ) -> Result<(u8, u8), MoveError> {
        // if valid returns the min and max number of tiles that can be placed
        let mut tiles_placed = 0u8;
        let mut min_tiles = 0u8;
        //let mut max_tiles = 0u8;
        /*  Loop through all the cells involved in the move
           End the loop when:
               - we reach the edge of the board or
               - we reach an empty cell but have already placed all the tiles
        */
        for (current_pos, move_cell) in self.board.move_iterator(starting_position, direction) {
            match move_cell {
                MoveCell::Open => {
                    // we can place a tile here
                    tiles_placed += 1;
                }
                MoveCell::Connecting { letter_set } => {
                    if letter_set.allows_rack(&self.player[self.current_player].rack) {
                        // we can place a tile here
                        tiles_placed += 1;
                        if min_tiles == 0 {
                            min_tiles = tiles_placed;
                        }
                    } else {
                        // we are blocked
                        break;
                    }
                }
                MoveCell::Filled { .. } => {
                    if min_tiles == 0 {
                        if tiles_placed == 0 {
                            min_tiles = 1;
                        } else {
                            min_tiles = tiles_placed;
                        }
                    }
                }
            }
            if min_tiles == 0 && current_pos == (Position { x: 7, y: 7 }) {
                min_tiles = tiles_placed;
            }
        }
        if min_tiles == 0 {
            return Err(MoveError::TilesDoNotConnect);
        }
        Ok((min_tiles, tiles_placed))
    }

    pub fn validate_move(
        &self,
        starting_position: Position,
        direction: Direction,
        tiles: &TileList,
    ) -> Result<i16, MoveError> {
        // returns the score of the move
        // check that the tiles are in the rack
        self.player[self.current_player]
            .rack
            .confirm_contains_tile_list(tiles)?;

        let cross_direction = -direction;

        // apply tiles to the position map
        let mut word = String::new();
        let mut main_word_score: i16 = 0;
        let mut connecting_word_scores = 0i16;
        let mut word_multiplier = 1u8;
        let mut tile_idx = 0;
        let number_of_tiles = tiles.0.len();

        // move iterators goes back to the start of contiguous filled cells
        for (current_pos, move_cell) in self.board.move_iterator(starting_position, direction) {
            match move_cell {
                MoveCell::Open | MoveCell::Connecting { .. } => {
                    // if we are out of tiles then finish
                    if tile_idx >= number_of_tiles {
                        break;
                    }
                    let tile = tiles.0[tile_idx];
                    tile_idx += 1;
                    let letter = tile.letter().unwrap();
                    word.push(letter.as_char());
                    let cell = self.board.get_cell_pos(current_pos);
                    let letter_multiplier = cell.cell_type.letter_multiplier();
                    main_word_score +=
                        (tile.score(self.scrabble_variant) * letter_multiplier) as TScore;
                    word_multiplier *= cell.cell_type.word_multiplier();

                    if let MoveCell::Connecting { letter_set } = move_cell {
                        if !letter_set.contains(letter) {
                            return Err(MoveError::LetterNotAllowedInPosition);
                        }
                        connecting_word_scores += self.board.score_cross_word(
                            current_pos,
                            cross_direction,
                            tile,
                            letter,
                        )?;
                    }
                }

                MoveCell::Filled { letter, score } => {
                    word.push(letter.clone().as_char());
                    main_word_score += score;
                }
            }
        }
        // check that the word is in the word list
        if !is_word(&word) {
            return Err(MoveError::InvalidWord(word));
        }
        let score = main_word_score * word_multiplier as TScore
            + connecting_word_scores
            + if tile_idx == 7 {
                self.scrabble_variant.bingo_bonus as TScore
            } else {
                0
            };
        Ok(score)
    }

    // returns true if the game is over
    pub fn apply_move(
        &mut self,
        starting_position: Position,
        direction: Direction,
        tile_list: &TileList,
        score: i16,
    ) -> Result<(), MoveError> {
        self.board.reset_last_move_flags();

        let player = &mut self.player[self.current_player];
        //let player = self.current_player();
        let cross_direction = -direction;

        let mut tile_vec = tile_list.0.clone();
        tile_vec.reverse();

        let mut current_pos = starting_position;
        loop {
            let cell = self.board.get_cell_pos_mut(current_pos);
            match cell.value {
                CellValue::Empty { .. } => {
                    if let Some(played_tile) = tile_vec.pop() {
                        cell.set_tile(played_tile);
                        player.rack.remove_tile(played_tile);

                        self.board.update_word_gaps(current_pos, cross_direction);
                    } else {
                        break;
                    }
                }
                _ => {}
            }
            if let Some(pos) = current_pos.try_step_forward(direction) {
                current_pos = pos;
            } else {
                break;
            }
        }

        self.board.update_word_gaps(starting_position, direction);

        self.moves.push(GameMoveRecord {
            player: self.current_player,
            player_name: self.player_name[self.current_player].clone(),
            detail: GameMoveRecordDetail::Move {
                starting_position,
                direction,
                tiles: tile_list.clone(),
                score,
                word: self.board.read_word_at_pos(starting_position, direction),
            },
        });

        player.last_move = self.moves.len() - 1;

        player.rack.fill_rack(&mut self.bag);
        player.score += score;

        player.timer.stop();

        if player.rack.is_empty() {
            self.end_game();
        } else {
            self.reset_current_player_stats();
            self.current_player = (self.current_player + 1) % self.number_of_players;
            self.player[self.current_player].timer.start();
            self.first_move = false;
        }

        Ok(())
    }

    pub fn exchange_tiles(&mut self, tiles: &TileList) -> Result<(), MoveError> {
        if self.bag.count() < 7 {
            return Err(MoveError::NotEnoughTilesInBag);
        }

        let player_rack = &mut self.player[self.current_player].rack;
        player_rack.confirm_contains_tile_list(tiles)?;
        player_rack.remove_tile_list(tiles);
        self.bag.add_tile_list(tiles);
        player_rack.fill_rack(&mut self.bag);
        self.player[self.current_player].exchanges += 1;
        self.non_scoring_plays += 1;

        self.moves.push(GameMoveRecord {
            player: self.current_player,
            player_name: self.player_name[self.current_player].clone(),
            detail: GameMoveRecordDetail::Exchange {
                tiles: tiles.clone(),
            },
        });

        self.current_player_mut().last_move = self.moves.len() - 1;

        self.player[self.current_player].timer.stop();

        if self.non_scoring_plays >= 6 {
            self.end_game();
        } else {
            self.current_player = (self.current_player + 1) % self.number_of_players;
            self.player[self.current_player].timer.start();
        }

        Ok(())
    }

    pub fn pass(&mut self) {
        self.moves.push(GameMoveRecord {
            player: self.current_player,
            player_name: self.player_name[self.current_player].clone(),
            detail: GameMoveRecordDetail::Pass,
        });

        self.current_player_mut().last_move = self.moves.len() - 1;

        self.player[self.current_player].timer.stop();

        self.player[self.current_player].passes += 1;
        self.non_scoring_plays += 1;
        if self.non_scoring_plays >= 6 {
            self.end_game();
        } else {
            self.current_player = (self.current_player + 1) % self.number_of_players;
            self.player[self.current_player].timer.start();
        }
    }

    pub fn quit(&mut self) {
        self.end_game();
    }

    pub fn reset_current_player_stats(&mut self) {
        for player in self.player.iter_mut() {
            player.exchanges = 0;
            player.passes = 0;
        }
    }

    pub fn human_move(
        &mut self,
        starting_position: Position,
        direction: Direction,
        tiles: &TileList,
    ) -> Result<(), MoveError> {
        let player = self.current_player();
        player.rack.confirm_contains_tile_list(&tiles)?;

        let (min_tiles, max_tiles) = self.validate_position(starting_position, direction)?;

        if tiles.len() < min_tiles as usize || tiles.len() > max_tiles as usize {
            return Err(MoveError::TilesDonNotFit);
        }
        let score = self.validate_move(starting_position, direction, &tiles)?;
        self.apply_move(starting_position, direction, &tiles, score)?;

        self.last_player_to_play = Some(self.current_player);
        self.reset_current_player_stats();
        Ok(())
    }

    // recursive function to find the best move for a given position
    fn computer_move_position(
        &mut self,
        best_move: &mut GameMove, // the best move found so far, the contents will be updated if a better move is found
        starting_position: Position,
        direction: Direction,
        min_tiles: u8,
        max_tiles: u8,
        current_tile_list: TileList,
        current_rack: TileBag,
    ) {
        if current_tile_list.len() as u8 >= max_tiles {
            return;
        }
        if (current_tile_list.len() as u8) >= min_tiles {
            //let try_move = &mut best_move.clone();
            if let Ok(score) = self.validate_move(starting_position, direction, &current_tile_list)
            {
                if score > best_move.score {
                    *best_move = GameMove::new(
                        starting_position,
                        direction,
                        current_tile_list.clone(),
                        score,
                    );
                }
            }
        }

        if (current_tile_list.len() as u8) < max_tiles && !current_rack.is_empty() {
            for &letter in ALPHABET {
                if current_rack.contains(letter) {
                    let mut new_tile_list = current_tile_list.clone();
                    let tile = Tile::Letter(letter);
                    new_tile_list.0.push(tile);
                    let mut new_rack = current_rack.clone();
                    new_rack.remove_letter(letter);
                    self.computer_move_position(
                        best_move,
                        starting_position,
                        direction,
                        min_tiles,
                        max_tiles,
                        new_tile_list,
                        new_rack,
                    );
                }
                if current_rack.blanks > 0 {
                    let mut new_tile_list = current_tile_list.clone();
                    let tile = Tile::Blank {
                        acting_as_letter: Some(letter),
                    };
                    new_tile_list.0.push(tile);
                    let mut new_rack = current_rack.clone();
                    new_rack.remove_blank();
                    self.computer_move_position(
                        best_move,
                        starting_position,
                        direction,
                        min_tiles,
                        max_tiles,
                        new_tile_list,
                        new_rack,
                    );
                }
            }
        }
    }

    pub fn computer_move(&mut self) {
        let start_pos = Position::new(7, 7);
        let direction = Direction::Horizontal;
        let best_move = &mut GameMove::new(start_pos, direction, TileList::new(), 0);

        for &direction in [Direction::Vertical, Direction::Horizontal].iter() {
            for y in 0..15 {
                for x in 0..15 {
                    let start_pos = Position::new(x, y);
                    if let Ok((min_tiles, max_tiles)) = self.validate_position(start_pos, direction)
                    {
                        self.computer_move_position(
                            best_move,
                            start_pos,
                            direction,
                            min_tiles,
                            max_tiles,
                            TileList::new(),
                            self.player[self.current_player].rack.clone(),
                        );
                    }
                }
            }
        }

        if best_move.score == 0 {
            self.computer_no_move().unwrap();
        } else {
            self.apply_move(
                best_move.starting_position,
                best_move.direction,
                &best_move.tiles,
                best_move.score,
            )
            .unwrap();
        }
    }

    fn computer_no_move(&mut self) -> Result<(), MoveError> {
        // if best move has score 0, then we haven't found a move, so we either pass or exchange tiles
        // if we were the last player to play, we pass
        // we can assume that someone can play, it is not possible that no-one can get a playable rack given we can exchange tiles

        if self.bag.count() >= 7 {
            let tiles = self.player[self.current_player].rack.into();
            self.exchange_tiles(&tiles)?;
            return Ok(());
        } else {
            return Ok(());
        }
    }

    fn end_game(&mut self) {
        self.is_over = true;
        let mut draw = false;
        // if the current player has no tiles left, they get the sum of all the tiles in the other players racks
        // otherwise, each player has the sum of their rack deducted from their score
        let mut racks_total: TScore = 0;
        for player in self.player.iter() {
            racks_total += player.rack.sum_tile_values(self.scrabble_variant);
        }
        let current_player = &self.player[self.current_player];
        if current_player.rack.is_empty() {
            self.current_player_mut().score += racks_total;
        }
        for (i, player) in self.player.iter_mut().enumerate() {
            if i != self.current_player {
                player.score -= player.rack.sum_tile_values(self.scrabble_variant);
            }
        }

        let mut max_score = 0;
        let mut winner = 0;
        for (i, player) in self.player.iter().enumerate() {
            if player.score == max_score {
                draw = true;
            } else if player.score > max_score {
                max_score = player.score;
                winner = i;
                draw = false;
            }
        }
        self.is_over = true;
        self.winner = if draw { None } else { Some(winner) };
    }

    pub fn last_move(&self) -> Option<&GameMoveRecord> {
        self.moves.last()
    }
}
