//! Pure state machine transitions and effect generation for project lifecycles.
//!
//! All transition logic is implemented as pure functions without side effects,
//! database access, or asynchronous code. The database layer executes the resulting
//! [`Effect`] items inside an atomic transaction.

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{ProjectId, UserId};
use crate::project::Phase;
use crate::round::{RoundId, RoundsConfig};

/// System actors or roles that can trigger a state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    /// Automatic transition triggered internally (e.g. within a page save transaction).
    Auto,
    /// Regular proofreader.
    Proofer,
    /// Project Manager.
    Manager,
    /// Post-Processor.
    Pp,
    /// Site Administrator.
    Admin,
}

impl ActorRole {
    /// Canonical display string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Proofer => "proofer",
            Self::Manager => "pm",
            Self::Pp => "pp",
            Self::Admin => "admin",
        }
    }
}

/// Actor specification for transitions: either a single role or a union of allowed roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// Exactly one actor role.
    Role(ActorRole),
    /// Allowed if the executing actor matches any role in the list.
    AnyOf(&'static [ActorRole]),
}

impl Actor {
    /// Convenient shorthand for auto transitions.
    pub const AUTO: Self = Self::Role(ActorRole::Auto);
    /// Convenient shorthand for Manager or Admin.
    pub const MANAGER_OR_ADMIN: Self = Self::AnyOf(&[ActorRole::Manager, ActorRole::Admin]);
    /// Convenient shorthand for Manager, Post-Processor, or Admin.
    pub const PP_OR_MANAGER_OR_ADMIN: Self =
        Self::AnyOf(&[ActorRole::Manager, ActorRole::Pp, ActorRole::Admin]);
    /// Convenient shorthand for Admin only.
    pub const ADMIN: Self = Self::Role(ActorRole::Admin);

    /// Return true if `role` is permitted by this actor constraint.
    #[must_use]
    pub fn permits(&self, role: ActorRole) -> bool {
        match self {
            Self::Role(r) => *r == role,
            Self::AnyOf(roles) => roles.contains(&role),
        }
    }
}

/// Pre-condition guards evaluated before a transition is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// No preconditions required.
    None,
    /// Project must contain at least 1 page.
    HasPages,
    /// Every page in the project must have an image associated with it.
    AllPagesHaveImages,
    /// Every page in the project must have OCR text.
    AllPagesHaveOcr,
    /// All prerequisite page readiness checks (HasPages + AllPagesHaveImages + AllPagesHaveOcr).
    PagesReady,
    /// All pages in the active round must be in 'done' state (0 incomplete pages).
    RoundComplete,
    /// Guard that always fails (disallows auto-transition).
    Never,
}

impl Guard {
    /// Evaluate the guard against the provided project context.
    ///
    /// # Errors
    /// Returns [`DomainError::TransitionDenied`] or a specific validation error if the guard is not satisfied.
    pub fn evaluate(&self, ctx: &ProjectContext) -> Result<(), DomainError> {
        match self {
            Self::None => Ok(()),
            Self::HasPages => {
                if ctx.total_pages > 0 {
                    Ok(())
                } else {
                    Err(DomainError::InvalidInput(
                        "dự án chưa có trang nào".to_string(),
                    ))
                }
            }
            Self::AllPagesHaveImages => {
                if ctx.total_pages > 0 && ctx.pages_with_image == ctx.total_pages {
                    Ok(())
                } else {
                    Err(DomainError::InvalidInput(
                        "tất cả các trang phải có tệp hình ảnh trước khi chuyển trạng thái"
                            .to_string(),
                    ))
                }
            }
            Self::AllPagesHaveOcr => {
                if ctx.total_pages > 0 && ctx.pages_with_ocr == ctx.total_pages {
                    Ok(())
                } else {
                    Err(DomainError::InvalidInput(
                        "tất cả các trang phải có văn bản OCR trước khi chuyển trạng thái"
                            .to_string(),
                    ))
                }
            }
            Self::PagesReady => {
                if ctx.total_pages == 0 {
                    return Err(DomainError::InvalidInput(
                        "dự án phải có ít nhất 1 trang".to_string(),
                    ));
                }
                if ctx.pages_with_image != ctx.total_pages {
                    return Err(DomainError::InvalidInput(
                        "tất cả các trang phải có hình ảnh".to_string(),
                    ));
                }
                if ctx.pages_with_ocr != ctx.total_pages {
                    return Err(DomainError::InvalidInput(
                        "tất cả các trang phải có văn bản OCR".to_string(),
                    ));
                }
                Ok(())
            }
            Self::RoundComplete => {
                if ctx.phase == Phase::InRound && ctx.round_incomplete_pages == 0 {
                    Ok(())
                } else {
                    let current = ctx
                        .current_round
                        .as_ref()
                        .map_or("unknown", RoundId::as_str);
                    Err(DomainError::RoundIncomplete {
                        round: current.to_string(),
                    })
                }
            }
            Self::Never => Err(DomainError::TransitionDenied {
                from: ctx.phase.to_string(),
                to: "auto".to_string(),
                actor: "auto".to_string(),
            }),
        }
    }
}

/// Action to perform upon successful transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// No additional round action.
    None,
    /// Explicitly set active round to a specific ID (e.g. initial round P1).
    SetRound(RoundId),
    /// Advance to the next round configured in rounds.toml and seed page_rounds.
    SeedNextRound,
    /// Clear the active round (sets current_round = NULL).
    ClearRound,
}

/// Specification of a permissible state transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// Originating project phase.
    pub from: Phase,
    /// Target project phase.
    pub to: Phase,
    /// Actor allowed to perform this transition.
    pub actor: Actor,
    /// Precondition guard.
    pub guard: Guard,
    /// Associated round management action.
    pub action: Action,
}

/// Context snapshot of a project used to evaluate transitions and guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContext {
    /// Unique project ID.
    pub id: ProjectId,
    /// Current lifecycle phase.
    pub phase: Phase,
    /// Active round if in `InRound` phase.
    pub current_round: Option<RoundId>,
    /// Whether project is on hold.
    pub on_hold: bool,
    /// Total number of pages in project.
    pub total_pages: usize,
    /// Number of pages with an image path.
    pub pages_with_image: usize,
    /// Number of pages with non-empty OCR text.
    pub pages_with_ocr: usize,
    /// Number of pages in the current round that are NOT done.
    pub round_incomplete_pages: usize,
}

/// Database side-effect emitted by pure transition execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Update project phase, current round, and on_hold state.
    UpdateProjectPhase {
        /// Project ID.
        project_id: ProjectId,
        /// New lifecycle phase.
        phase: Phase,
        /// New active round (`Some` only if `phase == Phase::InRound`).
        current_round: Option<RoundId>,
    },
    /// Seed the next round from the previous round's finished text.
    SeedRound {
        /// Project ID.
        project_id: ProjectId,
        /// Previous round ID to copy text from.
        from_round: RoundId,
        /// Newly activated round ID.
        to_round: RoundId,
    },
    /// Record an entry in the project audit log (`project_events`).
    RecordEvent {
        /// Project ID.
        project_id: ProjectId,
        /// Previous phase.
        from_phase: Option<Phase>,
        /// Destination phase.
        to_phase: Option<Phase>,
        /// Previous round.
        from_round: Option<RoundId>,
        /// Destination round.
        to_round: Option<RoundId>,
        /// User who initiated the action, or None for Auto.
        actor_id: Option<UserId>,
        /// Descriptive note.
        note: String,
    },
    /// Toggle project hold flag.
    ToggleOnHold {
        /// Project ID.
        project_id: ProjectId,
        /// Target on_hold state.
        on_hold: bool,
    },
}

/// Canonical transition table per Section 5.3 of the specification.
pub static STANDARD_TRANSITIONS: &[Transition] = &[
    // 1. Draft -> Queued (Manager, Admin)
    Transition {
        from: Phase::Draft,
        to: Phase::Queued,
        actor: Actor::MANAGER_OR_ADMIN,
        guard: Guard::PagesReady,
        action: Action::None,
    },
    // 2. Queued -> InRound (Manager, Admin) — starts at first round (e.g. P1)
    Transition {
        from: Phase::Queued,
        to: Phase::InRound,
        actor: Actor::MANAGER_OR_ADMIN,
        guard: Guard::None,
        action: Action::SeedNextRound, // triggers setting initial round
    },
    // 3. Queued -> Draft (Manager, Admin)
    Transition {
        from: Phase::Queued,
        to: Phase::Draft,
        actor: Actor::MANAGER_OR_ADMIN,
        guard: Guard::None,
        action: Action::ClearRound,
    },
    // 4. InRound -> InRound (Auto) — next round
    Transition {
        from: Phase::InRound,
        to: Phase::InRound,
        actor: Actor::AUTO,
        guard: Guard::RoundComplete,
        action: Action::SeedNextRound,
    },
    // 5. InRound -> PostProcessing (Auto) — when last round finishes
    Transition {
        from: Phase::InRound,
        to: Phase::PostProcessing,
        actor: Actor::AUTO,
        guard: Guard::RoundComplete,
        action: Action::ClearRound,
    },
    // 6. PostProcessing -> Posted (Manager, PP, Admin)
    Transition {
        from: Phase::PostProcessing,
        to: Phase::Posted,
        actor: Actor::PP_OR_MANAGER_OR_ADMIN,
        guard: Guard::None,
        action: Action::None,
    },
    // 7. Posted -> Archived (Admin)
    Transition {
        from: Phase::Posted,
        to: Phase::Archived,
        actor: Actor::ADMIN,
        guard: Guard::None,
        action: Action::None,
    },
    // 8. InRound -> Draft (Admin) — destructive rewind
    Transition {
        from: Phase::InRound,
        to: Phase::Draft,
        actor: Actor::ADMIN,
        guard: Guard::None,
        action: Action::ClearRound,
    },
];

/// Execute a state transition on a project context using domain rules.
///
/// This pure function validates:
/// 1. The transition exists from `ctx.phase` to `target_phase`.
/// 2. The `actor_role` has permission to execute the transition.
/// 3. The associated guard condition is satisfied.
///
/// On success, returns an ordered list of [`Effect`] items that should be executed
/// atomically in the database.
///
/// # Errors
/// Returns [`DomainError::TransitionDenied`] if the transition is illegal or unauthorized,
/// or another [`DomainError`] if a guard fails.
pub fn apply(
    ctx: &ProjectContext,
    target_phase: Phase,
    actor_role: ActorRole,
    actor_id: Option<UserId>,
    rounds: &RoundsConfig,
) -> Result<Vec<Effect>, DomainError> {
    // Special case: InRound -> InRound or InRound -> PostProcessing auto-advance
    if ctx.phase == Phase::InRound && actor_role == ActorRole::Auto {
        return apply_auto_advance(ctx, rounds);
    }

    // Lookup transition in the standard transition table
    let transition = STANDARD_TRANSITIONS
        .iter()
        .find(|t| t.from == ctx.phase && t.to == target_phase)
        .ok_or_else(|| DomainError::TransitionDenied {
            from: ctx.phase.to_string(),
            to: target_phase.to_string(),
            actor: actor_role.as_str().to_string(),
        })?;

    // Check actor permission
    if !transition.actor.permits(actor_role) {
        return Err(DomainError::TransitionDenied {
            from: ctx.phase.to_string(),
            to: target_phase.to_string(),
            actor: actor_role.as_str().to_string(),
        });
    }

    // Evaluate precondition guard
    transition.guard.evaluate(ctx)?;

    // Compute effects based on action
    let mut effects = Vec::new();
    let mut next_round: Option<RoundId> = None;

    match &transition.action {
        Action::None => {
            // Keep existing current_round only if remaining in InRound, else None
            next_round = if target_phase == Phase::InRound {
                ctx.current_round.clone()
            } else {
                None
            };
        }
        Action::SetRound(r) => {
            next_round = Some(r.clone());
        }
        Action::SeedNextRound => {
            if ctx.phase == Phase::Queued && target_phase == Phase::InRound {
                // Starting initial round
                let first = rounds.first_round().ok_or_else(|| {
                    DomainError::Config("no rounds defined in rounds configuration".to_string())
                })?;
                next_round = Some(first.id.clone());
            }
        }
        Action::ClearRound => {
            next_round = None;
        }
    }

    effects.push(Effect::UpdateProjectPhase {
        project_id: ctx.id,
        phase: target_phase,
        current_round: next_round.clone(),
    });

    effects.push(Effect::RecordEvent {
        project_id: ctx.id,
        from_phase: Some(ctx.phase),
        to_phase: Some(target_phase),
        from_round: ctx.current_round.clone(),
        to_round: next_round,
        actor_id,
        note: format!("chuyển từ {} sang {}", ctx.phase, target_phase),
    });

    Ok(effects)
}

/// Helper for automatic round progression when the active round completes.
fn apply_auto_advance(
    ctx: &ProjectContext,
    rounds: &RoundsConfig,
) -> Result<Vec<Effect>, DomainError> {
    if ctx.phase != Phase::InRound {
        return Err(DomainError::InvalidInput(
            "chỉ có thể tự động chuyển vòng khi dự án đang ở in_round".to_string(),
        ));
    }

    if ctx.round_incomplete_pages > 0 {
        let current = ctx
            .current_round
            .as_ref()
            .map_or("unknown", RoundId::as_str);
        return Err(DomainError::RoundIncomplete {
            round: current.to_string(),
        });
    }

    let current = ctx.current_round.as_ref().ok_or_else(|| {
        DomainError::InvalidInput("dự án in_round nhưng không có current_round".to_string())
    })?;

    let mut effects = Vec::new();

    if rounds.is_last_round(current) {
        // Final round completed: transition to PostProcessing
        effects.push(Effect::UpdateProjectPhase {
            project_id: ctx.id,
            phase: Phase::PostProcessing,
            current_round: None,
        });

        effects.push(Effect::RecordEvent {
            project_id: ctx.id,
            from_phase: Some(Phase::InRound),
            to_phase: Some(Phase::PostProcessing),
            from_round: Some(current.clone()),
            to_round: None,
            actor_id: None,
            note: format!("hoàn thành vòng cuối {}, chuyển sang hậu kỳ", current),
        });
    } else {
        // Advance to next round
        let next = rounds.next_round(current).ok_or_else(|| {
            DomainError::Config(format!("không tìm thấy vòng tiếp theo sau '{current}'"))
        })?;

        effects.push(Effect::UpdateProjectPhase {
            project_id: ctx.id,
            phase: Phase::InRound,
            current_round: Some(next.id.clone()),
        });

        effects.push(Effect::SeedRound {
            project_id: ctx.id,
            from_round: current.clone(),
            to_round: next.id.clone(),
        });

        effects.push(Effect::RecordEvent {
            project_id: ctx.id,
            from_phase: Some(Phase::InRound),
            to_phase: Some(Phase::InRound),
            from_round: Some(current.clone()),
            to_round: Some(next.id.clone()),
            actor_id: None,
            note: format!("hoàn thành vòng {}, chuyển sang vòng {}", current, next.id),
        });
    }

    Ok(effects)
}

/// Pure helper to generate effects for toggling a project's `on_hold` flag.
///
/// # Errors
/// Returns [`DomainError::TransitionDenied`] if the actor is not a Manager or Admin.
pub fn toggle_on_hold(
    ctx: &ProjectContext,
    actor_role: ActorRole,
    actor_id: Option<UserId>,
) -> Result<Vec<Effect>, DomainError> {
    if !Actor::MANAGER_OR_ADMIN.permits(actor_role) {
        return Err(DomainError::TransitionDenied {
            from: ctx.phase.to_string(),
            to: ctx.phase.to_string(),
            actor: actor_role.as_str().to_string(),
        });
    }

    let target_hold = !ctx.on_hold;
    let note = if target_hold {
        "tạm dừng dự án (on_hold = 1)".to_string()
    } else {
        "kích hoạt lại dự án (on_hold = 0)".to_string()
    };

    Ok(vec![
        Effect::ToggleOnHold {
            project_id: ctx.id,
            on_hold: target_hold,
        },
        Effect::RecordEvent {
            project_id: ctx.id,
            from_phase: Some(ctx.phase),
            to_phase: Some(ctx.phase),
            from_round: ctx.current_round.clone(),
            to_round: ctx.current_round.clone(),
            actor_id,
            note,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_context(phase: Phase, current_round: Option<&str>) -> ProjectContext {
        ProjectContext {
            id: ProjectId::new(1),
            phase,
            current_round: current_round.map(|s| RoundId::new(s).unwrap()),
            on_hold: false,
            total_pages: 10,
            pages_with_image: 10,
            pages_with_ocr: 10,
            round_incomplete_pages: 0,
        }
    }

    #[test]
    fn test_draft_to_queued_success() {
        let ctx = dummy_context(Phase::Draft, None);
        let rounds = RoundsConfig::default_embedded();

        let effects = apply(
            &ctx,
            Phase::Queued,
            ActorRole::Manager,
            Some(UserId::new(42)),
            &rounds,
        )
        .unwrap();

        assert_eq!(effects.len(), 2);
        assert_eq!(
            effects[0],
            Effect::UpdateProjectPhase {
                project_id: ProjectId::new(1),
                phase: Phase::Queued,
                current_round: None,
            }
        );
    }

    #[test]
    fn test_draft_to_queued_denied_without_pages() {
        let mut ctx = dummy_context(Phase::Draft, None);
        ctx.total_pages = 0;
        let rounds = RoundsConfig::default_embedded();

        let res = apply(
            &ctx,
            Phase::Queued,
            ActorRole::Manager,
            Some(UserId::new(42)),
            &rounds,
        );
        assert!(res.is_err());
    }

    #[test]
    fn test_draft_to_queued_denied_for_proofer() {
        let ctx = dummy_context(Phase::Draft, None);
        let rounds = RoundsConfig::default_embedded();

        let res = apply(
            &ctx,
            Phase::Queued,
            ActorRole::Proofer,
            Some(UserId::new(42)),
            &rounds,
        );
        assert!(matches!(res, Err(DomainError::TransitionDenied { .. })));
    }

    #[test]
    fn test_queued_to_in_round_seeds_first_round() {
        let ctx = dummy_context(Phase::Queued, None);
        let rounds = RoundsConfig::default_embedded();

        let effects = apply(
            &ctx,
            Phase::InRound,
            ActorRole::Manager,
            Some(UserId::new(42)),
            &rounds,
        )
        .unwrap();

        assert_eq!(
            effects[0],
            Effect::UpdateProjectPhase {
                project_id: ProjectId::new(1),
                phase: Phase::InRound,
                current_round: Some(RoundId::new("P1").unwrap()),
            }
        );
    }

    #[test]
    fn test_auto_advance_from_p1_to_p2() {
        let ctx = dummy_context(Phase::InRound, Some("P1"));
        let rounds = RoundsConfig::default_embedded();

        let effects = apply(&ctx, Phase::InRound, ActorRole::Auto, None, &rounds).unwrap();

        assert_eq!(effects.len(), 3);
        assert_eq!(
            effects[0],
            Effect::UpdateProjectPhase {
                project_id: ProjectId::new(1),
                phase: Phase::InRound,
                current_round: Some(RoundId::new("P2").unwrap()),
            }
        );
        assert_eq!(
            effects[1],
            Effect::SeedRound {
                project_id: ProjectId::new(1),
                from_round: RoundId::new("P1").unwrap(),
                to_round: RoundId::new("P2").unwrap(),
            }
        );
    }

    #[test]
    fn test_auto_advance_blocked_if_pages_incomplete() {
        let mut ctx = dummy_context(Phase::InRound, Some("P1"));
        ctx.round_incomplete_pages = 1;
        let rounds = RoundsConfig::default_embedded();

        let res = apply(&ctx, Phase::InRound, ActorRole::Auto, None, &rounds);
        assert!(matches!(res, Err(DomainError::RoundIncomplete { .. })));
    }

    #[test]
    fn test_auto_advance_last_round_to_post_processing() {
        let ctx = dummy_context(Phase::InRound, Some("F2"));
        let rounds = RoundsConfig::default_embedded();

        let effects = apply(&ctx, Phase::PostProcessing, ActorRole::Auto, None, &rounds).unwrap();

        assert_eq!(
            effects[0],
            Effect::UpdateProjectPhase {
                project_id: ProjectId::new(1),
                phase: Phase::PostProcessing,
                current_round: None,
            }
        );
    }

    #[test]
    fn test_toggle_on_hold() {
        let ctx = dummy_context(Phase::InRound, Some("P1"));
        let effects = toggle_on_hold(&ctx, ActorRole::Manager, Some(UserId::new(10))).unwrap();

        assert_eq!(
            effects[0],
            Effect::ToggleOnHold {
                project_id: ProjectId::new(1),
                on_hold: true,
            }
        );
    }
}
