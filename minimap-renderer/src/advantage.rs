/// Team advantage calculation for the minimap renderer.
///
/// Evaluates which team has a stronger position based on capture points,
/// score trajectory, HP advantage, and fleet composition.

/// Per-team snapshot of game state for a single frame.
#[derive(Debug, Clone)]
pub struct TeamState {
    pub score: i64,
    /// Number of uncontested caps owned by this team
    pub uncontested_caps: usize,
    pub total_hp: f32,
    pub max_hp: f32,
    pub ships_alive: usize,
    /// Total number of players on this team (from arena state)
    pub ships_total: usize,
    /// Number of ships with known entity data (EntityCreate received)
    pub ships_known: usize,
}

/// How strong the advantage is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvantageLevel {
    Absolute,
    Strong,
    Moderate,
    Weak,
}

impl AdvantageLevel {
    pub fn label(&self) -> &'static str {
        match self {
            AdvantageLevel::Absolute => "Absolute",
            AdvantageLevel::Strong => "Strong",
            AdvantageLevel::Moderate => "Moderate",
            AdvantageLevel::Weak => "Weak",
        }
    }
}

/// Which team has the advantage, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamAdvantage {
    /// Team 0 has the advantage at the given level
    Team0(AdvantageLevel),
    /// Team 1 has the advantage at the given level
    Team1(AdvantageLevel),
    /// No clear advantage
    Even,
}

impl TeamAdvantage {
    fn for_team(team: usize, level: AdvantageLevel) -> Self {
        if team == 0 {
            TeamAdvantage::Team0(level)
        } else {
            TeamAdvantage::Team1(level)
        }
    }
}

/// Scoring rules from the replay's BattleLogic.
#[derive(Debug, Clone)]
pub struct ScoringParams {
    pub team_win_score: i64,
    pub hold_reward: i64,
    pub hold_period: f32,
}

/// Breakdown of individual factors contributing to the advantage verdict.
/// All contribution values are signed: positive = favors team 0, negative = favors team 1.
#[derive(Debug, Clone, Default)]
pub struct AdvantageBreakdown {
    /// Contribution from time-to-win projection (who reaches win score first)
    pub time_to_win: f64,
    /// Contribution from current score gap
    pub score_gap: f64,
    /// Contribution from projected final score gap
    pub projection: f64,
    /// Contribution from cap count advantage (time-weighted)
    pub cap_control: f64,
    /// Contribution from HP ratio difference (0 if data incomplete)
    pub hp: f64,
    /// Contribution from ship count difference (0 if data incomplete)
    pub ship_count: f64,
    /// Total advantage score (sum of all contributions)
    pub total: f64,
    /// Whether HP/ship data was complete enough to factor in
    pub hp_data_reliable: bool,
    /// Special case: a team was fully eliminated
    pub team_eliminated: bool,

    // Raw values for tooltip display
    /// Points per second from caps for team 0
    pub team0_pps: f64,
    /// Points per second from caps for team 1
    pub team1_pps: f64,
    /// Projected final score for team 0 (capped at win score)
    pub team0_projected: f64,
    /// Projected final score for team 1 (capped at win score)
    pub team1_projected: f64,
    /// HP ratio for team 0 (0..1)
    pub team0_hp_ratio: f32,
    /// HP ratio for team 1 (0..1)
    pub team1_hp_ratio: f32,
}

/// Result of advantage calculation: the verdict plus the breakdown of why.
#[derive(Debug, Clone)]
pub struct AdvantageResult {
    pub advantage: TeamAdvantage,
    pub breakdown: AdvantageBreakdown,
}

impl AdvantageResult {
    fn even() -> Self {
        AdvantageResult {
            advantage: TeamAdvantage::Even,
            breakdown: AdvantageBreakdown::default(),
        }
    }
}

/// Calculate which team has the advantage.
///
/// Contested capture points (has_invaders == true) are excluded from both
/// teams' uncontested_caps counts before calling this function.
pub fn calculate_advantage(
    team0: &TeamState,
    team1: &TeamState,
    scoring: &ScoringParams,
    time_left: Option<i64>,
) -> AdvantageResult {
    // Not enough data yet (e.g. match start before enemy entities are created).
    if team0.ships_total == 0 || team1.ships_total == 0 {
        return AdvantageResult::even();
    }

    // We only have HP/alive data for ships whose entities have been created.
    // `ships_known` tracks how many ships we actually have entity data for.
    // If either team has incomplete data, we can't reliably compare HP or
    // ship counts — only score and cap data are trustworthy.
    let hp_data_reliable =
        team0.ships_known == team0.ships_total && team1.ships_known == team1.ships_total;

    // 1. Team eliminated -> Absolute (only when we have full entity data)
    if hp_data_reliable {
        if team0.ships_alive == 0 && team1.ships_alive > 0 {
            return AdvantageResult {
                advantage: TeamAdvantage::Team1(AdvantageLevel::Absolute),
                breakdown: AdvantageBreakdown {
                    team_eliminated: true,
                    hp_data_reliable: true,
                    ..Default::default()
                },
            };
        }
        if team1.ships_alive == 0 && team0.ships_alive > 0 {
            return AdvantageResult {
                advantage: TeamAdvantage::Team0(AdvantageLevel::Absolute),
                breakdown: AdvantageBreakdown {
                    team_eliminated: true,
                    hp_data_reliable: true,
                    ..Default::default()
                },
            };
        }
        if team0.ships_alive == 0 && team1.ships_alive == 0 {
            return AdvantageResult::even();
        }
    }

    let score_gap = team0.score - team1.score; // positive = team0 ahead

    // Points per second from uncontested caps
    let pps0 = if scoring.hold_period > 0.0 {
        team0.uncontested_caps as f64 * scoring.hold_reward as f64 / scoring.hold_period as f64
    } else {
        0.0
    };
    let pps1 = if scoring.hold_period > 0.0 {
        team1.uncontested_caps as f64 * scoring.hold_reward as f64 / scoring.hold_period as f64
    } else {
        0.0
    };

    // Project final scores
    let seconds_left = time_left.unwrap_or(0).max(0) as f64;
    let projected0 = team0.score as f64 + pps0 * seconds_left;
    let projected1 = team1.score as f64 + pps1 * seconds_left;

    // Cap both projections at win score
    let win = scoring.team_win_score as f64;
    let proj0 = projected0.min(win);
    let proj1 = projected1.min(win);

    // Time to reach win score for each team (None = can't reach)
    let time_to_win_fn = |score: i64, pps: f64| -> Option<f64> {
        let remaining = win - score as f64;
        if remaining <= 0.0 {
            Some(0.0)
        } else if pps > 0.0 {
            Some(remaining / pps)
        } else {
            None
        }
    };

    let ttw0 = time_to_win_fn(team0.score, pps0);
    let ttw1 = time_to_win_fn(team1.score, pps1);

    // Score projection advantage: who reaches win score first?
    let projection_gap = proj0 - proj1;

    // HP ratios
    let hp_ratio0 = if team0.max_hp > 0.0 {
        team0.total_hp / team0.max_hp
    } else {
        0.0
    };
    let hp_ratio1 = if team1.max_hp > 0.0 {
        team1.total_hp / team1.max_hp
    } else {
        0.0
    };
    let hp_advantage = hp_ratio0 - hp_ratio1; // positive = team0 healthier

    // Ship count ratio
    let total_alive = (team0.ships_alive + team1.ships_alive) as f64;
    let ship_advantage = if total_alive > 0.0 {
        (team0.ships_alive as f64 - team1.ships_alive as f64) / total_alive
    } else {
        0.0
    };

    // Cap advantage
    let cap_advantage = team0.uncontested_caps as i64 - team1.uncontested_caps as i64; // positive = team0

    // --- Determine advantage level ---
    let mut bd = AdvantageBreakdown {
        hp_data_reliable,
        team0_pps: pps0,
        team1_pps: pps1,
        team0_projected: proj0,
        team1_projected: proj1,
        team0_hp_ratio: hp_ratio0,
        team1_hp_ratio: hp_ratio1,
        ..Default::default()
    };

    // Score projection is the primary factor
    // One team wins by score before time runs out
    match (ttw0, ttw1) {
        (Some(t0), Some(t1)) if t0 < seconds_left && t1 < seconds_left => {
            // Both can reach win score — whoever gets there first
            let time_diff = t1 - t0; // positive = team0 wins first
            if time_diff.abs() > 30.0 {
                bd.time_to_win = time_diff.signum() * 3.0;
            } else if time_diff.abs() > 10.0 {
                bd.time_to_win = time_diff.signum() * 2.0;
            }
        }
        (Some(t0), _) if t0 < seconds_left => {
            // Only team0 can reach win score
            bd.time_to_win = 3.0;
        }
        (_, Some(t1)) if t1 < seconds_left => {
            // Only team1 can reach win score
            bd.time_to_win = -3.0;
        }
        _ => {}
    }

    // Score gap factor
    let abs_gap = score_gap.unsigned_abs();
    if abs_gap >= 400 {
        bd.score_gap = score_gap.signum() as f64 * 3.0;
    } else if abs_gap >= 200 {
        bd.score_gap = score_gap.signum() as f64 * 2.0;
    } else if abs_gap >= 100 {
        bd.score_gap = score_gap.signum() as f64 * 1.0;
    }

    // Projected final score gap
    if projection_gap.abs() >= 300.0 {
        bd.projection = projection_gap.signum() * 2.0;
    } else if projection_gap.abs() >= 150.0 {
        bd.projection = projection_gap.signum() * 1.0;
    }

    // Cap advantage (secondary) — weighted by time remaining so caps matter
    // less when there's little time to score from them.
    let time_weight = (seconds_left / 120.0).clamp(0.0, 1.0); // full weight at 2+ minutes
    if cap_advantage.abs() >= 2 {
        bd.cap_control = cap_advantage.signum() as f64 * 1.5 * time_weight;
    } else if cap_advantage.abs() >= 1 {
        bd.cap_control = cap_advantage.signum() as f64 * 0.5 * time_weight;
    }

    // HP and ship count factors only when we have complete entity data
    if hp_data_reliable {
        // HP advantage (tiebreaker): 25%+ difference
        if hp_advantage.abs() >= 0.25 {
            bd.hp = hp_advantage.signum() as f64 * 1.0;
        } else if hp_advantage.abs() >= 0.15 {
            bd.hp = hp_advantage.signum() as f64 * 0.5;
        }

        // Ship count advantage (tiebreaker): 20%+ fewer ships
        if ship_advantage.abs() >= 0.20 {
            bd.ship_count = ship_advantage.signum() * 1.0;
        }
    }

    bd.total =
        bd.time_to_win + bd.score_gap + bd.projection + bd.cap_control + bd.hp + bd.ship_count;

    // Map total to AdvantageLevel
    // Thresholds chosen so that Absolute requires multiple dominant factors
    // (e.g. only team can win by score AND 400+ point lead, or team eliminated).
    // A single factor category never exceeds 3.0, so Absolute (>= 7.0) needs
    // at least three strong signals aligned.
    let abs_score = bd.total.abs();
    let team = if bd.total > 0.0 { 0 } else { 1 };

    let advantage = if abs_score >= 7.0 {
        TeamAdvantage::for_team(team, AdvantageLevel::Absolute)
    } else if abs_score >= 4.0 {
        TeamAdvantage::for_team(team, AdvantageLevel::Strong)
    } else if abs_score >= 2.0 {
        TeamAdvantage::for_team(team, AdvantageLevel::Moderate)
    } else if abs_score >= 0.5 {
        TeamAdvantage::for_team(team, AdvantageLevel::Weak)
    } else {
        TeamAdvantage::Even
    };

    AdvantageResult {
        advantage,
        breakdown: bd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_scoring() -> ScoringParams {
        ScoringParams {
            team_win_score: 1000,
            hold_reward: 3,
            hold_period: 5.0,
        }
    }

    fn even_team(score: i64, caps: usize) -> TeamState {
        TeamState {
            score,
            uncontested_caps: caps,
            total_hp: 100000.0,
            max_hp: 100000.0,
            ships_alive: 12,
            ships_total: 12,
            ships_known: 12,
        }
    }

    #[test]
    fn even_game_start() {
        let t0 = even_team(0, 0);
        let t1 = even_team(0, 0);
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(1200));
        assert_eq!(r.advantage, TeamAdvantage::Even);
    }

    #[test]
    fn team_eliminated() {
        let t0 = TeamState {
            ships_alive: 8,
            ..even_team(500, 2)
        };
        let t1 = TeamState {
            ships_alive: 0,
            total_hp: 0.0,
            ..even_team(300, 0)
        };
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(600));
        assert_eq!(r.advantage, TeamAdvantage::Team0(AdvantageLevel::Absolute));
        assert!(r.breakdown.team_eliminated);
    }

    #[test]
    fn team_eliminated_other() {
        let t0 = TeamState {
            ships_alive: 0,
            total_hp: 0.0,
            ..even_team(300, 0)
        };
        let t1 = TeamState {
            ships_alive: 5,
            ..even_team(400, 3)
        };
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(600));
        assert_eq!(r.advantage, TeamAdvantage::Team1(AdvantageLevel::Absolute));
        assert!(r.breakdown.team_eliminated);
    }

    #[test]
    fn score_gap_400_plus() {
        let t0 = even_team(700, 2);
        let t1 = even_team(250, 1);
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(300));
        assert!(matches!(
            r.advantage,
            TeamAdvantage::Team0(AdvantageLevel::Absolute | AdvantageLevel::Strong)
        ));
        assert!(r.breakdown.score_gap > 0.0);
    }

    #[test]
    fn score_gap_200_plus() {
        let t0 = even_team(500, 2);
        let t1 = even_team(280, 2);
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(300));
        assert!(matches!(
            r.advantage,
            TeamAdvantage::Team0(
                AdvantageLevel::Moderate | AdvantageLevel::Strong | AdvantageLevel::Absolute
            )
        ));
    }

    #[test]
    fn cap_advantage_projects_win() {
        let t0 = even_team(0, 3);
        let t1 = even_team(0, 0);
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(1200));
        assert!(matches!(
            r.advantage,
            TeamAdvantage::Team0(
                AdvantageLevel::Strong | AdvantageLevel::Absolute | AdvantageLevel::Moderate
            )
        ));
        assert!(r.breakdown.time_to_win > 0.0);
        assert!(r.breakdown.cap_control > 0.0);
    }

    #[test]
    fn hp_advantage_25_percent() {
        let t0 = TeamState {
            total_hp: 80000.0,
            ..even_team(400, 1)
        };
        let t1 = TeamState {
            total_hp: 50000.0,
            ..even_team(400, 1)
        };
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(600));
        assert!(matches!(
            r.advantage,
            TeamAdvantage::Team0(
                AdvantageLevel::Weak
                    | AdvantageLevel::Moderate
                    | AdvantageLevel::Strong
                    | AdvantageLevel::Absolute
            )
        ));
        assert!(r.breakdown.hp > 0.0);
    }

    #[test]
    fn ship_count_20_percent_deficit() {
        // 10 vs 6 alive: ship_advantage = 4/16 = 0.25 > 0.20 threshold
        let t0 = TeamState {
            ships_alive: 10,
            ..even_team(400, 2)
        };
        let t1 = TeamState {
            ships_alive: 6,
            total_hp: 60000.0,
            ..even_team(400, 2)
        };
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(600));
        assert!(matches!(
            r.advantage,
            TeamAdvantage::Team0(
                AdvantageLevel::Weak
                    | AdvantageLevel::Moderate
                    | AdvantageLevel::Strong
                    | AdvantageLevel::Absolute
            )
        ));
        assert!(r.breakdown.ship_count > 0.0);
    }

    #[test]
    fn cap_advantage_but_trailing_score() {
        let t0 = even_team(600, 0);
        let t1 = even_team(400, 3);
        let scoring = default_scoring();
        let r = calculate_advantage(&t0, &t1, &scoring, Some(1000));
        assert!(matches!(r.advantage, TeamAdvantage::Team1(_)));
        // time_to_win should favor team1 (negative = team1)
        assert!(r.breakdown.time_to_win < 0.0);
    }

    #[test]
    fn close_to_win_threshold() {
        let t0 = even_team(950, 1);
        let t1 = even_team(900, 0);
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(300));
        // Team0 reaches 1000 in ~83s, team1 can't score: time_to_win=3.0, cap_control=0.5 -> 3.5
        assert!(matches!(
            r.advantage,
            TeamAdvantage::Team0(
                AdvantageLevel::Moderate | AdvantageLevel::Strong | AdvantageLevel::Absolute
            )
        ));
    }

    #[test]
    fn no_time_left_limits_cap_advantage() {
        let t0 = even_team(800, 0);
        let t1 = even_team(700, 4);
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(5));
        assert!(matches!(r.advantage, TeamAdvantage::Team0(_)));
        // Cap control should be near zero due to time weighting
        assert!(r.breakdown.cap_control.abs() < 0.1);
    }

    #[test]
    fn contested_caps_no_income() {
        let t0 = even_team(500, 0);
        let t1 = even_team(500, 0);
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(600));
        assert_eq!(r.advantage, TeamAdvantage::Even);
    }

    #[test]
    fn incomplete_entity_data_ignores_hp_and_ships() {
        let t0 = even_team(0, 0);
        let t1 = TeamState {
            ships_known: 1,
            ships_alive: 1,
            total_hp: 8000.0,
            max_hp: 8000.0,
            ..even_team(0, 0)
        };
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(1200));
        assert_eq!(r.advantage, TeamAdvantage::Even);
        assert!(!r.breakdown.hp_data_reliable);
        assert_eq!(r.breakdown.hp, 0.0);
        assert_eq!(r.breakdown.ship_count, 0.0);
    }

    #[test]
    fn breakdown_has_raw_values() {
        let t0 = even_team(500, 2);
        let t1 = even_team(300, 1);
        let r = calculate_advantage(&t0, &t1, &default_scoring(), Some(600));
        // team0 has 2 caps at 3pts/5s = 1.2 pps, team1 has 1 cap = 0.6 pps
        assert!((r.breakdown.team0_pps - 1.2).abs() < 0.01);
        assert!((r.breakdown.team1_pps - 0.6).abs() < 0.01);
        // Projected scores should be above current
        assert!(r.breakdown.team0_projected > 500.0);
        assert!(r.breakdown.team1_projected > 300.0);
        // HP ratios both 100%
        assert!((r.breakdown.team0_hp_ratio - 1.0).abs() < 0.01);
        assert!((r.breakdown.team1_hp_ratio - 1.0).abs() < 0.01);
    }
}
