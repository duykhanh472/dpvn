//! Text comparison routes for OCR baselines and round revisions.
//!
//! Implements Section 10 of the implementation plan:
//! - Section 10.1: OCR ↔ P1, adjacent rounds, and OCR ↔ current views.
//! - Section 10.3: Two-column rendering, changes-only filter, whitespace toggle.
//!
//! Diffs are computed per request and never stored.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Response},
};
use dp_core::{DiffOptions, RoundId, RoundsConfig, diff::diff_rows};
use dp_db::queries::{
    page_rounds::get_page_diff_data, pages::count_pages_by_project_id, projects::get_project_detail,
};
use minijinja::context;
use serde::Deserialize;
use tracing::instrument;

use crate::{AppError, AppState, auth::AuthUser};

/// Query parameters accepted by the diff view.
#[derive(Debug, Default, Deserialize)]
pub struct DiffQuery {
    /// 1-based page number. Defaults to the first page.
    pub page: Option<i32>,
    /// Source side: `ocr` or a round identifier such as `P1`.
    pub from: Option<String>,
    /// Target side: `ocr` or a round identifier such as `P2`.
    pub to: Option<String>,
    /// Hide unchanged lines when set to `1`, `true`, `on`, or `yes`.
    pub changes_only: Option<String>,
    /// Render spaces, tabs, and newlines visibly when set.
    pub whitespace: Option<String>,
}

/// One selectable text source in the diff view.
struct DiffSource {
    value: String,
    label: String,
    text: String,
}

/// `GET /projects/{id}/diff` — Compare OCR and round texts for one page.
#[instrument(skip(state, auth), fields(project_id = id, page))]
pub async fn diff_page(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<DiffQuery>,
) -> Result<Response, AppError> {
    // Authorization: every authenticated user who can view the project detail
    // page may compare its stored texts.
    let project = get_project_detail(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    let page_number = query.page.unwrap_or(1);
    if page_number < 1 {
        return Err(AppError::BadRequest(
            "Số trang phải là số nguyên dương.".to_string(),
        ));
    }

    let data = get_page_diff_data(&state.db, id, page_number)
        .await?
        .ok_or(AppError::NotFound)?;

    let total_pages = count_pages_by_project_id(&state.db, id).await?;
    if total_pages > 0 && i64::from(page_number) > total_pages {
        return Err(AppError::NotFound);
    }

    let rounds = RoundsConfig::default_embedded();
    let mut revision_text: HashMap<&str, &str> = HashMap::with_capacity(data.revisions.len());
    for revision in &data.revisions {
        revision_text.insert(
            revision.round.as_str(),
            revision.text.as_deref().unwrap_or(""),
        );
    }

    // Source list in round-config order, with the immutable OCR baseline first.
    let mut sources = Vec::with_capacity(data.revisions.len() + 1);
    sources.push(DiffSource {
        value: "ocr".to_owned(),
        label: "OCR".to_owned(),
        text: data.ocr_text.clone().unwrap_or_default(),
    });
    for round in &rounds.rounds {
        if let Some(text) = revision_text.get(round.id.as_str()) {
            sources.push(DiffSource {
                value: round.id.as_str().to_owned(),
                label: format!("Vòng {}", round.id),
                text: (*text).to_owned(),
            });
        }
    }

    let has_source = |value: &str| sources.iter().any(|source| source.value == value);

    let default_to = project
        .current_round
        .as_deref()
        .filter(|round| has_source(round))
        .map(str::to_owned)
        .or_else(|| sources.last().map(|source| source.value.clone()))
        .unwrap_or_else(|| "ocr".to_owned());

    let default_from = if default_to == "ocr" {
        "ocr".to_owned()
    } else {
        RoundId::new(&default_to)
            .ok()
            .and_then(|rid| rounds.prev_round(&rid))
            .map(|round| round.id.as_str().to_owned())
            .filter(|round| has_source(round))
            .unwrap_or_else(|| "ocr".to_owned())
    };

    let from = query
        .from
        .filter(|value| has_source(value))
        .unwrap_or(default_from);
    let to = query
        .to
        .filter(|value| has_source(value))
        .unwrap_or(default_to);

    let from_source = sources
        .iter()
        .find(|source| source.value == from)
        .ok_or(AppError::NotFound)?;
    let to_source = sources
        .iter()
        .find(|source| source.value == to)
        .ok_or(AppError::NotFound)?;

    let changes_only = parse_flag(query.changes_only.as_deref());
    let show_whitespace = parse_flag(query.whitespace.as_deref());

    let rows = diff_rows(
        &from_source.text,
        &to_source.text,
        DiffOptions {
            show_whitespace,
            changes_only,
        },
    );

    let rows_context: Vec<_> = rows
        .iter()
        .map(|row| {
            context! {
                kind => row.kind.as_str(),
                old_line => row.old_line_number,
                new_line => row.new_line_number,
                old_html => row.old_html.clone(),
                new_html => row.new_html.clone(),
            }
        })
        .collect();

    let from_options: Vec<_> = sources
        .iter()
        .map(|source| {
            context! {
                value => source.value.clone(),
                label => source.label.clone(),
                selected => source.value == from,
            }
        })
        .collect();
    let to_options: Vec<_> = sources
        .iter()
        .map(|source| {
            context! {
                value => source.value.clone(),
                label => source.label.clone(),
                selected => source.value == to,
            }
        })
        .collect();

    let template = state.templates.get_template("projects/diff.html")?;
    let rendered = template.render(context! {
        user => context! {
            username => auth.username.clone(),
            roles => auth.roles.clone(),
        },
        csrf_token => auth.csrf_token.clone(),
        project => context! {
            id => project.id,
            name => project.name.clone(),
            author => project.author.clone(),
            phase => project.phase.clone(),
            current_round => project.current_round.clone(),
        },
        page_number,
        total_pages,
        from_label => from_source.label.clone(),
        to_label => to_source.label.clone(),
        from_value => from.clone(),
        to_value => to.clone(),
        from_options,
        to_options,
        changes_only,
        whitespace => show_whitespace,
        has_changes => from_source.text != to_source.text,
        rows => rows_context,
    })?;

    Ok(Html(rendered).into_response())
}

fn parse_flag(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        )
    })
}
