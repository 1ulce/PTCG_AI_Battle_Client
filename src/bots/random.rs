//! `RandomPolicy` — 合法手・選択肢から一様ランダムに選ぶベースライン bot。
//!
//! self-play の determinism はシード固定の `ChaCha20Rng` で担保される。他の bot が
//! 未実装の判断を委譲するフォールバック先でもある。

use crate::transport::TransportError;
use crate::wire::action::ActionDto;
use crate::wire::protocol::{PromptDto, PromptMsg, RequestMsg};

use rand::Rng;
use rand_chacha::ChaCha20Rng;

use super::{pick_random_subset, BotPolicy, PromptChoice};

/// 一様ランダム方策。
pub struct RandomPolicy;

impl BotPolicy for RandomPolicy {
    fn choose_action(
        &mut self,
        req: &RequestMsg,
        rng: &mut ChaCha20Rng,
    ) -> Result<ActionDto, TransportError> {
        random_action(req, rng)
    }

    fn choose_prompt(&mut self, p: &PromptMsg, rng: &mut ChaCha20Rng) -> PromptChoice {
        random_prompt_choice(p, rng)
    }
}

/// 合法手から一様ランダムに 1 つ選ぶ。
///
/// # Errors
/// 合法手が空のとき [`TransportError::Unexpected`]。
pub fn random_action(req: &RequestMsg, rng: &mut ChaCha20Rng) -> Result<ActionDto, TransportError> {
    if req.legal_actions.is_empty() {
        return Err(TransportError::Unexpected("no legal actions".to_string()));
    }
    let idx = rng.gen_range(0..req.legal_actions.len());
    Ok(req.legal_actions[idx].clone())
}

/// prompt の種別ごとにランダムな応答素材を生成する。
#[allow(clippy::too_many_lines)]
pub fn random_prompt_choice(p: &PromptMsg, rng: &mut ChaCha20Rng) -> PromptChoice {
    let selected = match &p.kind {
        PromptDto::ChooseFromZone { options, .. } => {
            let ids: Vec<u32> = options.iter().map(|o| o.entity_id).collect();
            pick_random_subset(rng, &ids, p.min, p.max)
        }
        PromptDto::ChooseTargetPokemon { targets } => {
            if targets.is_empty() {
                vec![]
            } else {
                vec![targets[rng.gen_range(0..targets.len())]]
            }
        }
        PromptDto::ChooseInitialActive { eligible } => {
            if eligible.is_empty() {
                vec![]
            } else {
                vec![eligible[rng.gen_range(0..eligible.len())]]
            }
        }
        PromptDto::PlaceInitialBench {
            eligible,
            bench_max,
        } => {
            let count = rng.gen_range(0..=usize::from(*bench_max).min(eligible.len()));
            let mut pool: Vec<u32> = eligible.clone();
            pool.truncate(count);
            pool
        }
        PromptDto::ReplaceActiveAfterKo { bench_options } => {
            if bench_options.is_empty() {
                vec![]
            } else {
                vec![bench_options[rng.gen_range(0..bench_options.len())]]
            }
        }
        PromptDto::DistributeDamage {
            eligible, total, ..
        } => {
            let count = eligible.len().min(usize::from(*total));
            eligible.iter().take(count).copied().collect()
        }
        PromptDto::AttachEnergyTo {
            energy_options,
            pokemon_eligible,
        } => {
            let mut v = Vec::new();
            if !energy_options.is_empty() {
                v.push(energy_options[0]);
            }
            if !pokemon_eligible.is_empty() {
                v.push(pokemon_eligible[0]);
            }
            v
        }
        PromptDto::DiscardFromAttached { eligible, .. } => {
            if eligible.is_empty() {
                vec![]
            } else {
                vec![eligible[0]]
            }
        }
        PromptDto::ReorderCards { cards, .. } => cards.clone(),
        PromptDto::SelectAbilityOrder { entries } => entries.clone(),
        PromptDto::PickAttackToCopy { candidates } => {
            if candidates.is_empty() {
                vec![]
            } else {
                let (e, _) = &candidates[rng.gen_range(0..candidates.len())];
                vec![*e]
            }
        }
        PromptDto::ChooseYesNo { .. }
        | PromptDto::ChooseFirstOrSecond
        | PromptDto::ChooseOneBranch { .. }
        | PromptDto::ChooseOpponentAttack { .. }
        | PromptDto::PickAmountFromEach { .. }
        | PromptDto::ChooseStatusToRemove { .. } => vec![],
        PromptDto::PeekAndReorder { peeked, .. } => peeked.clone(),
        PromptDto::AssignEnergyToTargets { energies, .. } => energies.clone(),
        PromptDto::PrizeHandSwapChoice {
            prize_options,
            hand_options,
        } => {
            if prize_options.is_empty() || hand_options.is_empty() {
                vec![]
            } else {
                vec![prize_options[0], hand_options[0]]
            }
        }
    };

    let yes = if matches!(
        p.kind,
        PromptDto::ChooseYesNo { .. } | PromptDto::ChooseFirstOrSecond
    ) {
        Some(rng.gen_bool(0.5))
    } else {
        None
    };

    let counts = if let PromptDto::DistributeDamage { total, .. } = &p.kind {
        let mut counts = vec![0u8; selected.len()];
        if !counts.is_empty() {
            counts[0] = *total;
        }
        counts
    } else {
        vec![]
    };

    let branch_index = match &p.kind {
        PromptDto::ChooseOneBranch { branch_count, .. }
        | PromptDto::ChooseOpponentAttack {
            attack_count: branch_count,
            ..
        } => Some(rng.gen_range(0..*branch_count)),
        PromptDto::ChooseStatusToRemove { statuses, .. } => {
            let n = u8::try_from(statuses.len()).unwrap_or(u8::MAX).max(1);
            Some(rng.gen_range(0..n))
        }
        PromptDto::PickAttackToCopy { candidates } => selected.first().and_then(|entity| {
            candidates
                .iter()
                .find(|(e, _)| e == entity)
                .map(|(_, names)| {
                    let n = u8::try_from(names.len()).unwrap_or(u8::MAX).max(1);
                    rng.gen_range(0..n)
                })
        }),
        _ => None,
    };

    // AssignEnergyToTargets — counts[i] = pokemon_eligible index 0 に集中
    let counts = if let PromptDto::AssignEnergyToTargets {
        pokemon_eligible, ..
    } = &p.kind
    {
        if pokemon_eligible.is_empty() {
            vec![]
        } else {
            vec![0u8; selected.len()]
        }
    } else {
        counts
    };

    PromptChoice {
        selected,
        counts,
        yes,
        branch_index,
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{dummy_request, rng};
    use super::*;

    #[test]
    fn random_action_picks_a_legal_action() {
        let legal = vec![ActionDto::EndTurn, ActionDto::Concede];
        let mut r = rng();
        let a = random_action(&dummy_request(legal.clone()), &mut r).expect("legal");
        assert!(legal.contains(&a));
    }

    #[test]
    fn random_action_errors_on_empty_legal_set() {
        let mut r = rng();
        let err = random_action(&dummy_request(vec![]), &mut r).unwrap_err();
        assert!(matches!(err, TransportError::Unexpected(_)));
    }
}
