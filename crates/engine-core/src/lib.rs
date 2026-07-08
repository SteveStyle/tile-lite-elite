use rules_shared::{
    GameState, MoveCandidate, MoveGenerator, Rack, RulesEngine, SOWPODS, Score, VariantRules,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineCapabilities {
    pub supports_timed_play: bool,
    pub supports_analysis: bool,
    pub supports_ranking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub supported_variants: Vec<String>,
    pub capabilities: EngineCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineRequest<'a> {
    pub state: &'a GameState,
    pub seat_number: u8,
    pub rack: &'a Rack,
    pub time_budget_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct EngineResponse {
    pub action: EngineAction,
    pub diagnostics: EngineDiagnostics,
}

#[derive(Debug, Clone)]
pub enum EngineAction {
    Place(MoveCandidate),
    Pass,
    Exchange(Vec<rules_shared::Tile>),
    Resign,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineDiagnostics {
    pub explanation: Option<String>,
    pub candidate_count: usize,
    pub chosen_score: Option<Score>,
}

pub trait ScrabbleEngine: Send + Sync {
    fn metadata(&self) -> &EngineMetadata;

    fn choose_action(&self, request: EngineRequest<'_>) -> EngineResponse;
}

#[derive(Debug, Clone)]
pub struct GreedyEngine {
    metadata: EngineMetadata,
    rules: VariantRules,
}

impl GreedyEngine {
    pub fn new() -> Self {
        Self {
            metadata: EngineMetadata {
                id: "greedy-v1".to_string(),
                name: "Greedy".to_string(),
                version: "1".to_string(),
                author: Some("scrabble-px".to_string()),
                description: Some(
                    "Chooses the highest-scoring legal move from the shared move generator."
                        .to_string(),
                ),
                supported_variants: vec!["official".to_string()],
                capabilities: EngineCapabilities {
                    supports_timed_play: false,
                    supports_analysis: false,
                    supports_ranking: false,
                },
            },
            rules: VariantRules::official(),
        }
    }
}

impl Default for GreedyEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrabbleEngine for GreedyEngine {
    fn metadata(&self) -> &EngineMetadata {
        &self.metadata
    }

    fn choose_action(&self, request: EngineRequest<'_>) -> EngineResponse {
        let engine = RulesEngine {
            rules: &self.rules,
            dictionary: &*SOWPODS,
        };

        let mut best: Option<(MoveCandidate, Score)> = None;
        let mut candidate_count = 0;

        for candidate in engine.enumerate_legal_moves(request.state, request.rack) {
            candidate_count += 1;
            if let Ok(validated) =
                engine.validate_game_move(request.state, Some(request.rack), &candidate)
            {
                let score = validated.score.total;
                match &best {
                    Some((_, best_score)) if *best_score >= score => {}
                    _ => best = Some((candidate, score)),
                }
            }
        }

        match best {
            Some((candidate, score)) => EngineResponse {
                action: EngineAction::Place(candidate),
                diagnostics: EngineDiagnostics {
                    explanation: Some("selected best legal move by score".to_string()),
                    candidate_count,
                    chosen_score: Some(score),
                },
            },
            None => EngineResponse {
                action: EngineAction::Pass,
                diagnostics: EngineDiagnostics {
                    explanation: Some("no legal move available".to_string()),
                    candidate_count,
                    chosen_score: None,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EngineAction, EngineRequest, GreedyEngine, ScrabbleEngine};
    use rules_shared::{GameState, Letter, Rack, SOWPODS, VariantRules};

    #[test]
    fn greedy_engine_plays_opening_move_when_available() {
        let rules = VariantRules::official();
        let state = GameState::new(&rules, &*SOWPODS);
        let engine = GreedyEngine::new();
        let mut rack = Rack::default();
        rack.add_letter(Letter::from('A'));
        rack.add_letter(Letter::from('T'));

        let response = engine.choose_action(EngineRequest {
            state: &state,
            seat_number: 0,
            rack: &rack,
            time_budget_ms: None,
        });

        assert!(matches!(response.action, EngineAction::Place(_)));
    }
}
