//! Project comment submission (§13.1, Phase 8).
//!
//! Comments are normalized, rendered and sanitized on the server, then stored in
//! both Markdown and HTML form. HTMX submissions receive the new comment as a
//! fragment for `beforeend` insertion; plain form submissions redirect back to
//! the project page.

use axum::{
    extract::{Form, Path, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Redirect, Response},
};
use chrono::Utc;
use dp_core::{render_markdown, viet};
use dp_db::queries::{comments::insert_project_comment, projects::get_project_by_id};
use minijinja::context;
use serde::Deserialize;
use tracing::instrument;

use crate::{
    app::{AppError, AppState, format_vietnam_time},
    auth::{AuthUser, csrf::verify_csrf_token},
};

/// Maximum comment length in characters (not bytes).
const MAX_COMMENT_CHARS: usize = 10_000;

/// Comment submission form.
#[derive(Debug, Deserialize)]
pub struct CommentForm {
    /// Raw Markdown supplied by the author.
    pub body_md: String,
    /// Session CSRF token.
    pub csrf_token: String,
}

/// `POST /projects/{id}/comments` — append a comment to a project.
///
/// Any authenticated user may comment (authorization matrix §12.6). The handler
/// checks CSRF, validates length, renders Markdown with `pulldown-cmark`, and
/// sanitizes the result with `ammonia` before storage.
#[instrument(skip(state, auth, headers, form), fields(project_id = id))]
pub async fn project_comment_submit(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<CommentForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &auth.csrf_token) {
        return Err(AppError::Forbidden);
    }

    // Project must exist; comments on archived projects remain readable.
    get_project_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    let body_md = viet::normalize(&form.body_md);
    let body_md = body_md.trim();
    if body_md.is_empty() {
        return Err(AppError::BadRequest(
            "Bình luận không được để trống.".to_string(),
        ));
    }
    if body_md.chars().count() > MAX_COMMENT_CHARS {
        return Err(AppError::BadRequest(format!(
            "Bình luận không được vượt quá {MAX_COMMENT_CHARS} ký tự."
        )));
    }

    let body_html = render_markdown(body_md);
    let comment_id =
        insert_project_comment(&state.db, id, auth.user_id.0, body_md, &body_html).await?;

    let is_htmx = headers
        .get("HX-Request")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));

    if is_htmx {
        let tmpl = state.templates.get_template("projects/_comment.html")?;
        let rendered = tmpl.render(context! {
            project_id => id,
            comment => context! {
                id => comment_id,
                body_html => body_html,
                author_username => auth.username.clone(),
                created_at_label => format_vietnam_time(Utc::now(), "%d/%m/%Y %H:%M"),
            },
            user => context! {
                username => auth.username,
                roles => auth.roles,
            },
        })?;
        return Ok(Html(rendered).into_response());
    }

    // Keep the total count fresh for any subsequent fragment render.
    Ok(Redirect::to(&format!("/projects/{id}#comments")).into_response())
}
