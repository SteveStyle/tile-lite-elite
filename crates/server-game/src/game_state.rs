use std::sync::Arc;
use std::time::Duration;

use api::{
    BoardCellDto, DirectionDto, EngineProfileDto, GameStateDto, GameStatus, MoveCandidateDto,
    MoveRecordDto, ParticipantDto, PositionDto, PremiumDto, RackDto, SeatKind, TileDto,
    TilePlacementDto,
};
use engine_core::{EngineAction, EngineMetadata, EngineRequest, GreedyEngine, ScrabbleEngine};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rules_shared::{
    BoardCell, BoardState, Direction, FilledCell, GameState, Letter, MoveCandidate, Rack,
    RulesEngine, SOWPODS, Tile, TilePlacement, VariantRules, format_move_error,
};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct EngineRegistry {
    pub engines: Vec<Arc<dyn ScrabbleEngine>>,
}

impl Default for EngineRegistry {
    fn default() -> Self {
        Self {
            engines: vec![Arc::new(GreedyEngine::new())],
        }
    }
}

impl EngineRegistry {
    pub fn metadata(&self) -> Vec<EngineProfileDto> {
        self.engines
            .iter()
            .map(|engine| engine_profile_from_metadata(engine.metadata()))
            .collect()
    }

    pub fn find(&self, id: &str) -> Option<Arc<dyn ScrabbleEngine>> {
        self.engines
            .iter()
            .find(|engine| engine.metadata().id == id)
            .cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveRecord {
    pub move_number: i64,
    pub seat_number: u8,
    pub move_type: String,
    pub main_word: Option<String>,
    pub score_delta: i32,
    pub description: String,
    /// Board squares this move placed a tile on — empty for anything but
    /// `"place"` (pass/exchange/resign/timeout touch no squares). `#[serde(default)]`
    /// so game snapshots persisted before this field existed still deserialize.
    #[serde(default)]
    pub positions: Vec<PositionDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantState {
    pub seat_number: u8,
    pub kind: SeatKind,
    pub display_name: String,
    pub player_id: Option<String>,
    pub engine_id: Option<String>,
    pub score: i32,
    pub rack: Rack,
    pub resigned: bool,
}

/// Number of consecutive scoreless plays (passes or exchanges), summed
/// across all seats, that ends the game with no one going out. This is the
/// standard tournament rule; in heads-up (2-player) play it amounts to each
/// player passing three times in a row.
const SCORELESS_TURN_LIMIT: u8 = 6;

/// Default per-move time limit: 72 hours, chosen for async play where
/// opponents aren't expected to be online at the same time.
pub const DEFAULT_MOVE_TIME_LIMIT_SECONDS: u64 = 72 * 60 * 60;

#[derive(Debug, Clone)]
pub struct GameSession {
    pub id: String,
    pub status: GameStatus,
    pub variant: String,
    pub language: String,
    pub board_layout: String,
    pub turn_number: i64,
    pub current_seat: u8,
    pub winner_seat: Option<u8>,
    /// Set only when someone went out (emptied their rack) — the standard
    /// end-of-game rack bonus, where every other seat's remaining rack
    /// value is deducted from them and handed to whoever went out. A
    /// scoreless-turn-limit ending or a resignation/timeout has no such
    /// transfer, so these stay `None` for those.
    pub final_bonus_seat: Option<u8>,
    pub final_bonus_points: Option<i32>,
    pub random_seed: u64,
    pub rules: VariantRules,
    pub state: GameState,
    pub bag: Vec<Tile>,
    pub participants: Vec<ParticipantState>,
    pub moves: Vec<MoveRecord>,
    pub consecutive_scoreless_turns: u8,
    pub move_time_limit_seconds: u64,
    /// Unix seconds (as a string, matching the rest of the codebase's
    /// timestamp convention) when `current_seat`'s turn began — reset every
    /// time the turn advances. Meaningless until the game is `Active`.
    pub turn_started_at: String,
}

impl GameSession {
    pub fn new(
        id: String,
        participants: Vec<ParticipantState>,
        random_seed: u64,
        rules: VariantRules,
        move_time_limit_seconds: u64,
    ) -> Self {
        let state = GameState::new(&rules, &*SOWPODS);
        let mut bag = build_bag(&rules);
        shuffle_bag(&mut bag, random_seed);

        Self {
            id,
            status: GameStatus::Waiting,
            variant: "official".to_string(),
            language: "sowpods".to_string(),
            board_layout: "official".to_string(),
            turn_number: 0,
            current_seat: 0,
            winner_seat: None,
            final_bonus_seat: None,
            final_bonus_points: None,
            random_seed,
            rules,
            state,
            bag,
            participants,
            moves: Vec::new(),
            consecutive_scoreless_turns: 0,
            move_time_limit_seconds,
            turn_started_at: now_unix_seconds().to_string(),
        }
    }

    pub fn start(&mut self) {
        if self.status != GameStatus::Waiting {
            return;
        }

        for participant in &mut self.participants {
            refill_rack(&mut participant.rack, &mut self.bag, self.rules.rack_size);
        }

        self.status = GameStatus::Active;
        self.turn_number = 1;
        self.current_seat = 0;
        self.turn_started_at = now_unix_seconds().to_string();
    }

    /// Auto-retires the current seat if it has sat on its turn past
    /// `move_time_limit_seconds`, exactly as if that player had resigned —
    /// same "the game ends the moment anyone leaves" rule `apply_resign`
    /// already applies, just triggered by a deadline instead of a manual
    /// action. Returns whether anything changed, so callers know whether to
    /// persist and broadcast. There's no background scheduler in this
    /// server, so this is checked lazily whenever a game is touched (see
    /// `expire_overdue_turns` in `app.rs`) rather than firing exactly on
    /// the deadline.
    pub fn apply_move_timeout(&mut self) -> bool {
        if self.status != GameStatus::Active {
            return false;
        }
        let Ok(started) = self.turn_started_at.parse::<u64>() else {
            return false;
        };
        if now_unix_seconds().saturating_sub(started) < self.move_time_limit_seconds {
            return false;
        }

        let seat = self.current_seat;
        let Some(participant) = self.participants.get_mut(seat as usize) else {
            return false;
        };
        participant.resigned = true;
        let display_name = participant.display_name.clone();
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number: seat,
            move_type: "timeout".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: format!("{display_name} was retired for exceeding the move time limit"),
        });
        self.winner_seat = self
            .participants
            .iter()
            .find(|other| !other.resigned)
            .map(|other| other.seat_number);
        self.status = GameStatus::Finished;
        true
    }

    /// A lightweight summary for games-list views. `last_activity_at` is
    /// supplied by the caller since move timestamps live in the persistence
    /// layer, not on `GameSession` itself.
    pub fn to_summary_dto(&self, last_activity_at: String) -> api::GameSummaryDto {
        api::GameSummaryDto {
            id: self.id.clone(),
            status: self.status.clone(),
            current_seat: self.current_seat,
            participants: self
                .participants
                .iter()
                .map(|participant| ParticipantDto {
                    seat_number: participant.seat_number,
                    kind: participant.kind.clone(),
                    display_name: participant.display_name.clone(),
                    player_id: participant.player_id.clone(),
                    engine_id: participant.engine_id.clone(),
                    score: participant.score,
                })
                .collect(),
            last_activity_at,
            move_time_limit_seconds: self.move_time_limit_seconds,
            turn_started_at: self.turn_started_at.clone(),
            // Caller-relative fields — `list_games` fills these in per
            // requester since "why does this game show up" depends on who's
            // asking, not on the game itself.
            relationship: api::GameRelationship::Participant,
            invitation_id: None,
        }
    }

    pub fn to_dto(&self) -> GameStateDto {
        GameStateDto {
            id: self.id.clone(),
            status: self.status.clone(),
            variant: self.variant.clone(),
            language: self.language.clone(),
            board_layout: self.board_layout.clone(),
            turn_number: self.turn_number,
            current_seat: self.current_seat,
            winner_seat: self.winner_seat,
            final_bonus_seat: self.final_bonus_seat,
            final_bonus_points: self.final_bonus_points,
            bag_count: self.bag.len(),
            move_time_limit_seconds: self.move_time_limit_seconds,
            turn_started_at: self.turn_started_at.clone(),
            participants: self
                .participants
                .iter()
                .map(|participant| ParticipantDto {
                    seat_number: participant.seat_number,
                    kind: participant.kind.clone(),
                    display_name: participant.display_name.clone(),
                    player_id: participant.player_id.clone(),
                    engine_id: participant.engine_id.clone(),
                    score: participant.score,
                })
                .collect(),
            board: board_to_dto(&self.state.board),
            racks: self
                .participants
                .iter()
                .map(|participant| RackDto {
                    counts: participant.rack.counts,
                    blanks: participant.rack.blanks,
                })
                .collect(),
            moves: self
                .moves
                .iter()
                .map(|record| MoveRecordDto {
                    move_number: record.move_number,
                    seat_number: record.seat_number,
                    move_type: record.move_type.clone(),
                    main_word: record.main_word.clone(),
                    score_delta: record.score_delta,
                    positions: record.positions.clone(),
                    description: record.description.clone(),
                })
                .collect(),
        }
    }

    pub fn apply_place_move(
        &mut self,
        seat_number: u8,
        candidate: MoveCandidate,
    ) -> Result<(), String> {
        ensure_active_turn(self, seat_number)?;
        let rules_engine = RulesEngine {
            rules: &self.rules,
            dictionary: &*SOWPODS,
        };
        let participant = self
            .participants
            .get_mut(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;
        let validated = rules_engine
            .validate_game_move(&self.state, Some(&participant.rack), &candidate)
            .map_err(|error| format_move_error(&error))?;

        for placement in &candidate.tiles {
            if !participant.rack.consume_tile(placement.tile) {
                return Err("Rack no longer matches move".to_string());
            }
        }

        rules_engine
            .apply_move_to_game(&mut self.state, &validated)
            .map_err(|error| format!("{error:?}"))?;

        participant.score += validated.score.total as i32;
        refill_rack(&mut participant.rack, &mut self.bag, self.rules.rack_size);
        let went_out = participant.rack.is_empty();
        let positions = candidate
            .tiles
            .iter()
            .map(|placement| match candidate.direction {
                Direction::Horizontal => PositionDto {
                    x: candidate.start.x + placement.offset,
                    y: candidate.start.y,
                },
                Direction::Vertical => PositionDto {
                    x: candidate.start.x,
                    y: candidate.start.y + placement.offset,
                },
            })
            .collect();
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number,
            move_type: "place".to_string(),
            main_word: Some(validated.preview.main_word.clone()),
            score_delta: validated.score.total as i32,
            positions,
            description: format!(
                "{} played {} for {}",
                participant.display_name, validated.preview.main_word, validated.score.total
            ),
        });
        self.consecutive_scoreless_turns = 0;
        if went_out {
            self.finish_game(Some(seat_number));
        } else {
            self.advance_turn();
        }
        Ok(())
    }

    pub fn apply_pass(&mut self, seat_number: u8) -> Result<(), String> {
        ensure_active_turn(self, seat_number)?;
        let participant = self
            .participants
            .get(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number,
            move_type: "pass".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: format!("{} passed", participant.display_name),
        });
        self.consecutive_scoreless_turns += 1;
        if self.consecutive_scoreless_turns >= SCORELESS_TURN_LIMIT {
            self.finish_game(None);
        } else {
            self.advance_turn();
        }
        Ok(())
    }

    pub fn apply_exchange(&mut self, seat_number: u8, tiles: Vec<Tile>) -> Result<(), String> {
        ensure_active_turn(self, seat_number)?;
        if self.bag.len() < tiles.len() {
            return Err("Not enough tiles left in bag to exchange".to_string());
        }
        let participant = self
            .participants
            .get_mut(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;

        for tile in &tiles {
            if !participant.rack.consume_tile(*tile) {
                return Err("Rack does not contain exchange tiles".to_string());
            }
        }

        self.bag.extend(tiles.iter().copied().map(reset_blank_tile));
        shuffle_bag(&mut self.bag, self.random_seed ^ self.turn_number as u64);
        refill_rack(&mut participant.rack, &mut self.bag, self.rules.rack_size);

        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number,
            move_type: "exchange".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: format!(
                "{} exchanged {} tiles",
                participant.display_name,
                tiles.len()
            ),
        });
        self.consecutive_scoreless_turns += 1;
        if self.consecutive_scoreless_turns >= SCORELESS_TURN_LIMIT {
            self.finish_game(None);
        } else {
            self.advance_turn();
        }
        Ok(())
    }

    pub fn apply_resign(&mut self, seat_number: u8) -> Result<(), String> {
        ensure_active_turn(self, seat_number)?;
        let participant = self
            .participants
            .get_mut(seat_number as usize)
            .ok_or_else(|| format!("Unknown seat {seat_number}"))?;
        participant.resigned = true;
        self.moves.push(MoveRecord {
            move_number: self.turn_number,
            seat_number,
            move_type: "resign".to_string(),
            main_word: None,
            score_delta: 0,
            positions: Vec::new(),
            description: format!("{} resigned", participant.display_name),
        });
        self.winner_seat = self
            .participants
            .iter()
            .find(|other| !other.resigned)
            .map(|other| other.seat_number);
        self.status = GameStatus::Finished;
        Ok(())
    }

    /// Runs the current seat's engine (if any) and applies its chosen action
    /// through exactly the same `apply_*` methods a human client's HTTP
    /// action would go through, so the server stays authoritative over
    /// engine-originated moves as much as human ones.
    ///
    /// The engine runs on a blocking-friendly thread pool (via
    /// `spawn_blocking`, since move search is CPU-bound, not I/O-bound) and
    /// is subject to `engine_timeout`. If the engine hasn't responded by
    /// then, the seat auto-passes rather than stalling the game.
    pub async fn maybe_run_engine_turn(
        &mut self,
        engines: &EngineRegistry,
        engine_timeout: Duration,
    ) -> Result<bool, String> {
        if self.status != GameStatus::Active {
            return Ok(false);
        }

        let current = self
            .participants
            .get(self.current_seat as usize)
            .ok_or_else(|| "Current seat missing".to_string())?;

        if current.kind != SeatKind::Engine {
            return Ok(false);
        }

        let engine_id = current
            .engine_id
            .clone()
            .ok_or_else(|| "Engine seat missing engine id".to_string())?;
        let engine = engines
            .find(&engine_id)
            .ok_or_else(|| format!("Unknown engine: {engine_id}"))?;
        let rack = current.rack;
        let seat_number = self.current_seat;
        let state_snapshot = self.state.clone();
        let time_budget_ms = engine_timeout.as_millis() as u64;

        let outcome = tokio::time::timeout(
            engine_timeout,
            tokio::task::spawn_blocking(move || {
                engine.choose_action(EngineRequest {
                    state: &state_snapshot,
                    seat_number,
                    rack: &rack,
                    time_budget_ms: Some(time_budget_ms),
                })
            }),
        )
        .await;

        let response = match outcome {
            Ok(Ok(response)) => response,
            Ok(Err(join_error)) => {
                return Err(format!(
                    "Engine '{engine_id}' panicked while choosing a move: {join_error}"
                ));
            }
            Err(_elapsed) => {
                tracing::warn!(
                    game_id = %self.id,
                    engine_id,
                    seat_number,
                    budget_ms = engine_timeout.as_millis() as u64,
                    "engine exceeded its move budget; auto-passing"
                );
                self.apply_pass(seat_number)?;
                return Ok(true);
            }
        };

        match response.action {
            EngineAction::Place(candidate) => {
                self.apply_place_move(self.current_seat, candidate)?
            }
            EngineAction::Pass => self.apply_pass(self.current_seat)?,
            EngineAction::Exchange(tiles) => self.apply_exchange(self.current_seat, tiles)?,
            EngineAction::Resign => self.apply_resign(self.current_seat)?,
        }

        Ok(true)
    }

    /// Ends the game via the standard endgame scoring adjustment: every
    /// participant's score is reduced by the value of the tiles left on
    /// their rack, and if `goer_out` went out (emptied their rack while
    /// tiles remained for everyone else), they additionally receive the sum
    /// of every other rack's value. `goer_out` is `None` when the game ends
    /// by a scoreless-turn streak instead, in which case only the
    /// deductions apply.
    fn finish_game(&mut self, goer_out: Option<u8>) {
        let letter_values = self.rules.letter_values;
        let rack_value = |rack: &Rack| -> i32 {
            rack.counts
                .iter()
                .zip(letter_values.iter())
                .map(|(&count, &value)| count as i32 * value as i32)
                .sum::<i32>()
        };

        let mut opponents_total = 0i32;
        for participant in &mut self.participants {
            let value = rack_value(&participant.rack);
            if Some(participant.seat_number) != goer_out {
                opponents_total += value;
            }
            participant.score -= value;
        }
        if let Some(seat) = goer_out {
            if let Some(participant) = self
                .participants
                .iter_mut()
                .find(|participant| participant.seat_number == seat)
            {
                participant.score += opponents_total;
            }
            self.final_bonus_seat = Some(seat);
            self.final_bonus_points = Some(opponents_total);
        }

        self.status = GameStatus::Finished;
        self.winner_seat = self.compute_winner_seat();
    }

    fn compute_winner_seat(&self) -> Option<u8> {
        let max_score = self.participants.iter().map(|p| p.score).max()?;
        let mut leaders = self
            .participants
            .iter()
            .filter(|participant| participant.score == max_score);
        let first = leaders.next()?;
        if leaders.next().is_some() {
            None
        } else {
            Some(first.seat_number)
        }
    }

    fn advance_turn(&mut self) {
        if self.status == GameStatus::Finished {
            return;
        }

        let next_seat = ((self.current_seat as usize + 1) % self.participants.len()) as u8;
        self.current_seat = next_seat;
        self.turn_number += 1;
        self.turn_started_at = now_unix_seconds().to_string();

        if self
            .participants
            .iter()
            .filter(|participant| !participant.resigned)
            .count()
            <= 1
        {
            self.status = GameStatus::Finished;
            self.winner_seat = self
                .participants
                .iter()
                .find(|participant| !participant.resigned)
                .map(|participant| participant.seat_number);
        }
    }
}

fn now_unix_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs()
}

fn ensure_active_turn(session: &GameSession, seat_number: u8) -> Result<(), String> {
    if session.status != GameStatus::Active {
        return Err("Game is not active".to_string());
    }
    if session.current_seat != seat_number {
        return Err(format!("It is not seat {seat_number}'s turn"));
    }
    Ok(())
}

fn build_bag(rules: &VariantRules) -> Vec<Tile> {
    let mut bag = Vec::new();
    for (index, count) in rules.tile_distribution.iter().copied().enumerate() {
        for _ in 0..count {
            bag.push(Tile::Letter(Letter::from(index as u8)));
        }
    }
    for _ in 0..rules.blank_tiles {
        bag.push(Tile::Blank { acting_as: None });
    }
    bag
}

fn shuffle_bag(bag: &mut [Tile], seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    for index in (1..bag.len()).rev() {
        let swap_index = rng.gen_range(0..=index);
        bag.swap(index, swap_index);
    }
}

fn refill_rack(rack: &mut Rack, bag: &mut Vec<Tile>, rack_size: u8) {
    while rack.count() < rack_size && !bag.is_empty() {
        let tile = bag.pop().expect("checked non-empty bag");
        match tile {
            Tile::Letter(letter) => rack.add_letter(letter),
            Tile::Blank { .. } => rack.blanks += 1,
        }
    }
}

fn reset_blank_tile(tile: Tile) -> Tile {
    match tile {
        Tile::Blank { .. } => Tile::Blank { acting_as: None },
        Tile::Letter(letter) => Tile::Letter(letter),
    }
}

fn board_to_dto(board: &BoardState) -> Vec<BoardCellDto> {
    board
        .cells
        .iter()
        .map(|cell| match cell {
            BoardCell::Empty(empty) => BoardCellDto {
                premium: premium_to_dto(empty.premium),
                letter: None,
                is_blank: false,
            },
            BoardCell::Filled(FilledCell { letter, is_blank }) => BoardCellDto {
                premium: PremiumDto::Blank,
                letter: Some(letter.as_char()),
                is_blank: *is_blank,
            },
        })
        .collect()
}

pub fn board_from_dto(cells: &[BoardCellDto]) -> Result<BoardState, String> {
    if cells.len() != BoardState::WIDTH * BoardState::HEIGHT {
        return Err(format!(
            "Expected {} board cells, got {}",
            BoardState::WIDTH * BoardState::HEIGHT,
            cells.len()
        ));
    }

    let mut board = BoardState::default();
    for (index, cell) in cells.iter().enumerate() {
        let x = (index % BoardState::WIDTH) as u8;
        let y = (index / BoardState::WIDTH) as u8;
        let pos = rules_shared::Position::new(x, y);
        let board_cell = match cell.letter {
            Some(letter) => BoardCell::Filled(FilledCell {
                letter: Letter::from(letter),
                is_blank: cell.is_blank,
            }),
            None => BoardCell::Empty(rules_shared::EmptyCell {
                premium: premium_from_dto(cell.premium.clone()),
            }),
        };
        board.set(pos, board_cell);
    }

    Ok(board)
}

fn premium_to_dto(premium: rules_shared::Premium) -> PremiumDto {
    match premium {
        rules_shared::Premium::Blank => PremiumDto::Blank,
        rules_shared::Premium::DoubleLetter => PremiumDto::DoubleLetter,
        rules_shared::Premium::TripleLetter => PremiumDto::TripleLetter,
        rules_shared::Premium::DoubleWord => PremiumDto::DoubleWord,
        rules_shared::Premium::TripleWord => PremiumDto::TripleWord,
    }
}

fn premium_from_dto(premium: PremiumDto) -> rules_shared::Premium {
    match premium {
        PremiumDto::Blank => rules_shared::Premium::Blank,
        PremiumDto::DoubleLetter => rules_shared::Premium::DoubleLetter,
        PremiumDto::TripleLetter => rules_shared::Premium::TripleLetter,
        PremiumDto::DoubleWord => rules_shared::Premium::DoubleWord,
        PremiumDto::TripleWord => rules_shared::Premium::TripleWord,
    }
}

pub fn move_candidate_from_dto(candidate: MoveCandidateDto) -> MoveCandidate {
    MoveCandidate {
        start: PositionDtoToRules::convert(candidate.start),
        direction: match candidate.direction {
            DirectionDto::Horizontal => Direction::Horizontal,
            DirectionDto::Vertical => Direction::Vertical,
        },
        tiles: candidate
            .tiles
            .into_iter()
            .map(|placement| TilePlacement {
                offset: placement.offset,
                tile: tile_from_dto(placement.tile),
            })
            .collect(),
    }
}

pub fn move_candidate_to_dto(candidate: &MoveCandidate) -> MoveCandidateDto {
    MoveCandidateDto {
        start: PositionDto {
            x: candidate.start.x,
            y: candidate.start.y,
        },
        direction: match candidate.direction {
            Direction::Horizontal => DirectionDto::Horizontal,
            Direction::Vertical => DirectionDto::Vertical,
        },
        tiles: candidate
            .tiles
            .iter()
            .map(|placement| TilePlacementDto {
                offset: placement.offset,
                tile: tile_to_dto(placement.tile),
            })
            .collect(),
    }
}

pub fn tile_from_dto(tile: TileDto) -> Tile {
    match tile {
        TileDto::Letter { letter } => Tile::Letter(Letter::from(letter)),
        TileDto::Blank { acting_as } => Tile::Blank {
            acting_as: acting_as.map(Letter::from),
        },
    }
}

pub fn tile_to_dto(tile: Tile) -> TileDto {
    match tile {
        Tile::Letter(letter) => TileDto::Letter {
            letter: letter.as_char(),
        },
        Tile::Blank { acting_as } => TileDto::Blank {
            acting_as: acting_as.map(|letter| letter.as_char()),
        },
    }
}

fn engine_profile_from_metadata(metadata: &EngineMetadata) -> EngineProfileDto {
    EngineProfileDto {
        id: metadata.id.clone(),
        name: metadata.name.clone(),
        version: metadata.version.clone(),
        author: metadata.author.clone(),
        description: metadata.description.clone(),
        supports_timed_play: metadata.capabilities.supports_timed_play,
        supports_analysis: metadata.capabilities.supports_analysis,
        supports_ranking: metadata.capabilities.supports_ranking,
    }
}

struct PositionDtoToRules;

impl PositionDtoToRules {
    fn convert(position: PositionDto) -> rules_shared::Position {
        rules_shared::Position::new(position.x, position.y)
    }
}
