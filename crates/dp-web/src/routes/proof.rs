//! Proofing interface routes, page claiming, saving, and protected image serving.
//!
//! Follows Section 6, 8, 9, 14, and 15 of the implementation plan:
//! - Section 14.3: Page claim and navigation flow
//! - Section 14.4 / Section 6.3: Critical section saving and confirmation
//! - Section 15.2: Authorized image delivery with X-Accel-Redirect
//! - Section 8.6: IME composition awareness

use axum::{
    Json,
    extract::{Form, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use dp_core::{
    CharsetCatalog, RoundId, RoundsConfig, ToneStyle, WordcheckContext, check_page_text,
    render_preview, viet,
    wordcheck::{WordIssue, validate_characters},
};
use dp_db::queries::{
    page_rounds::{
        SavePageInput, claim_next_available_page, claim_specific_page, get_proofing_page_data,
        get_user_active_claim, release_claim, save_page,
    },
    page_tallies::{check_daily_limit, current_vietnam_date, get_total_daily_tally},
    pages::count_pages_by_project_id,
    projects::get_project_by_id,
    words::get_all_project_words,
};
use minijinja::context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::instrument;

use crate::{
    app::{AppError, AppState},
    auth::{AuthUser, csrf::verify_csrf_token},
};

/// Payload sent when saving a proofed page.
#[derive(Debug, Deserialize)]
pub struct SavePagePayload {
    /// Proofed page text.
    pub text: String,
    /// Content hash computed by the client after reviewing advisory issues.
    pub confirmed_hash: Option<String>,
    /// CSRF token.
    pub csrf_token: Option<String>,
}

/// JSON response after saving.
#[derive(Debug, Serialize)]
pub struct SavePageResponse {
    pub ok: bool,
    pub next_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Realtime wordcheck check payload.
#[derive(Debug, Deserialize)]
pub struct WordcheckCheckPayload {
    pub text: String,
}

/// Voluntary release claim form.
#[derive(Debug, Deserialize)]
pub struct ReleaseClaimForm {
    pub csrf_token: String,
}

/// Realtime format-preview payload.
#[derive(Debug, Deserialize)]
pub struct FormatPreviewPayload {
    /// Draft page text from the editor.
    pub text: String,
    /// CSRF token (HTMX may send it as a header instead).
    pub csrf_token: Option<String>,
}

/// Calculate SHA-256 hex digest for proofing confirmation binding (§9.5).
fn compute_sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// `GET /projects/{id}/proof` — Find active claim or claim next available page, 303 to editor.
#[instrument(skip(state, auth))]
pub async fn proof_entry(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Response, AppError> {
    // 1. Verify project exists and is active in a round
    let project = get_project_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if project.phase != "in_round" || project.on_hold != 0 {
        return Err(AppError::BadRequest(
            "Dự án hiện không ở trạng thái nhận soát lỗi.".to_string(),
        ));
    }

    let current_round = project
        .current_round
        .ok_or_else(|| AppError::BadRequest("Dự án chưa có vòng hoạt động.".to_string()))?;

    // 2. Check daily limit
    check_daily_limit(&state.db, auth.user_id.0, state.config.limits.daily_pages).await?;

    // 3. If user already holds a page in this project, redirect directly to it
    if let Some(active) = get_user_active_claim(&state.db, auth.user_id.0).await? {
        if active.project_id == id {
            return Ok(
                Redirect::to(&format!("/projects/{id}/pages/{}", active.page_number))
                    .into_response(),
            );
        }
    }

    // 4. Resolve previous round for own-page exclusion
    let rounds = RoundsConfig::default_embedded();
    let current_rid =
        RoundId::new(&current_round).map_err(|e| AppError::BadRequest(e.to_string()))?;
    let prev_round_opt = rounds
        .get_round(&current_rid)
        .and_then(|r| rounds.prev_round(&r.id))
        .map(|r| r.id.as_str());

    // 5. Claim next available page
    let claimed = claim_next_available_page(
        &state.db,
        id,
        &current_round,
        prev_round_opt,
        auth.user_id.0,
        3,
    )
    .await?;

    match claimed {
        Some(page) => {
            Ok(Redirect::to(&format!("/projects/{id}/pages/{}", page.page_number)).into_response())
        }
        None => {
            // Pool empty for this user
            Ok(Redirect::to(&format!("/projects/{id}?empty=1")).into_response())
        }
    }
}

/// `GET /projects/{id}/pages/{n}` — Render proofing interface.
#[instrument(skip(state, auth))]
pub async fn proof_page(
    auth: AuthUser,
    State(state): State<AppState>,
    Path((id, page_number)): Path<(u64, i32)>,
) -> Result<Response, AppError> {
    // 1. Fetch comprehensive proofing data
    let data = get_proofing_page_data(&state.db, id, page_number)
        .await?
        .ok_or(AppError::NotFound)?;

    if data.phase != "in_round" {
        return Err(AppError::BadRequest(
            "Dự án không ở trạng thái soát lỗi.".to_string(),
        ));
    }

    let current_round = data
        .current_round
        .clone()
        .ok_or_else(|| AppError::BadRequest("Dự án chưa có vòng hoạt động.".to_string()))?;

    let is_manager = auth.has_role("admin") || auth.has_role("pm");
    let mut page_state = data.round_state.as_deref().unwrap_or("available");
    let is_mine = data.proofer_id == Some(auth.user_id.0);

    // 2. Handle claim transition if page is currently available
    if page_state == "available" {
        // Check daily limit before claiming
        check_daily_limit(&state.db, auth.user_id.0, state.config.limits.daily_pages).await?;

        // If user already has an active claim on a different page, redirect to that
        if let Some(active) = get_user_active_claim(&state.db, auth.user_id.0).await? {
            if active.project_id == id && active.page_number != page_number {
                return Ok(
                    Redirect::to(&format!("/projects/{id}/pages/{}", active.page_number))
                        .into_response(),
                );
            }
        }

        // Determine prev_round for own-page exclusion
        let rounds = RoundsConfig::default_embedded();
        let current_rid =
            RoundId::new(&current_round).map_err(|e| AppError::BadRequest(e.to_string()))?;
        let prev_round_opt = rounds
            .get_round(&current_rid)
            .and_then(|r| rounds.prev_round(&r.id))
            .map(|r| r.id.as_str());

        let claimed = claim_specific_page(
            &state.db,
            data.page_id,
            &current_round,
            prev_round_opt,
            auth.user_id.0,
        )
        .await?;

        if !claimed {
            return Err(AppError::Domain(dp_core::DomainError::PageUnavailable));
        }
        page_state = "in_progress";
    } else if page_state == "in_progress" && !is_mine && !is_manager {
        return Err(AppError::Domain(dp_core::DomainError::PageUnavailable));
    }

    let is_readonly = page_state == "done" || (!is_mine && is_manager);

    // 3. Resolve charset & curated palette
    let catalog = CharsetCatalog::default_embedded();
    let charset = catalog
        .get(&data.charset)
        .unwrap_or_else(|| catalog.default_vi());
    let palette_chars: Vec<String> = charset.palette.iter().map(|c| c.to_string()).collect();

    // 4. Fetch counts & tallies
    let total_pages = count_pages_by_project_id(&state.db, id).await?;
    let daily_tally =
        get_total_daily_tally(&state.db, auth.user_id.0, current_vietnam_date()).await?;

    let initial_text = data
        .text
        .as_deref()
        .or(data.ocr_text.as_deref())
        .unwrap_or("");

    let template = state.templates.get_template("proof/proof.html")?;
    let rendered = template.render(context! {
        project_id => id,
        project_name => data.project_name,
        author => data.author,
        page_number => page_number,
        total_pages => total_pages,
        current_round => current_round,
        round_state => page_state,
        is_readonly => is_readonly,
        image_url => format!("/projects/{id}/pages/{page_number}/image"),
        image_width => data.image_width,
        image_height => data.image_height,
        initial_text => initial_text,
        palette => palette_chars,
        daily_tally => daily_tally,
        daily_limit => state.config.limits.daily_pages,
        csrf_token => auth.csrf_token,
        user => minijinja::context! {
            username => auth.username.clone(),
            roles => auth.roles.clone(),
        },
    })?;

    Ok(Html(rendered).into_response())
}

/// `POST /projects/{id}/pages/{n}/save` — Save page in critical section (§6.3) with confirmation (§9.5).
#[instrument(skip(state, auth, headers, payload))]
pub async fn proof_save(
    auth: AuthUser,
    State(state): State<AppState>,
    Path((id, page_number)): Path<(u64, i32)>,
    headers: HeaderMap,
    Json(payload): Json<SavePagePayload>,
) -> Result<Response, AppError> {
    // 1. Verify CSRF
    let token = payload
        .csrf_token
        .as_deref()
        .or_else(|| headers.get("x-csrf-token").and_then(|v| v.to_str().ok()))
        .unwrap_or("");

    if !verify_csrf_token(token, &auth.csrf_token) {
        return Err(AppError::Forbidden);
    }

    // 2. Fetch page context
    let data = get_proofing_page_data(&state.db, id, page_number)
        .await?
        .ok_or(AppError::NotFound)?;

    let current_round = data
        .current_round
        .ok_or_else(|| AppError::BadRequest("Dự án chưa có vòng hoạt động.".to_string()))?;

    // Verify ownership
    if data.proofer_id != Some(auth.user_id.0) || data.round_state.as_deref() != Some("in_progress")
    {
        return Err(AppError::Db(dp_db::DbError::Conflict(
            "Bạn không phải người đang giữ trang này.".to_string(),
        )));
    }

    // 3. Normalize text (NFC + line endings) per Section 8.1
    let normalized = viet::normalize(&payload.text);

    // 4. Character validation per Section 9.5 (BLOCKING)
    let catalog = CharsetCatalog::default_embedded();
    let charset = catalog
        .get(&data.charset)
        .unwrap_or_else(|| catalog.default_vi());

    if let Err(block_err) = validate_characters(&normalized, charset) {
        return Err(AppError::BadRequest(block_err));
    }

    // 5. Wordcheck advisory issues & Content-bound confirmation (§9.5)
    let current_hash = compute_sha256_hex(&normalized);

    let (project_good, project_bad) = get_all_project_words(&state.db, id).await?;
    let site_words_guard = state.site_words.read().await;

    let tone_style = data.tone_style.parse::<ToneStyle>().unwrap_or_default();
    let wc_ctx = WordcheckContext {
        charset,
        tone_style,
        project_good: &project_good,
        project_bad: &project_bad,
        site_good: &site_words_guard.0,
        site_bad: &site_words_guard.1,
    };

    let issues = check_page_text(&normalized, &wc_ctx);

    let is_confirmed = payload
        .confirmed_hash
        .as_deref()
        .is_some_and(|h| h == current_hash);

    if !issues.is_empty() && !is_confirmed {
        // Render confirmation panel (HTMX swap per §9.5)
        let issues_val: Vec<_> = issues
            .iter()
            .map(|i| {
                context! {
                    word => i.word.clone(),
                    kind => i.kind.description(),
                    suggestion => i.suggestion.clone(),
                }
            })
            .collect();

        let tmpl = state.templates.get_template("proof/_confirm.html")?;
        let rendered = tmpl.render(context! {
            project_id => id,
            page_number => page_number,
            issues => issues_val,
            confirmed_hash => current_hash,
            csrf_token => auth.csrf_token,
        })?;

        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            rendered,
        )
            .into_response());
    }

    // 6. Execute critical section save (§6.3)
    let rounds_config = RoundsConfig::default_embedded();
    let outcome = save_page(
        &state.db,
        SavePageInput {
            project_id: id,
            page_id: data.page_id,
            round: &current_round,
            user_id: auth.user_id.0,
            text: &normalized,
        },
        &rounds_config,
    )
    .await?;

    let next_url = format!("/projects/{id}/proof");
    let mut msg = None;

    if let Some(ref adv_round) = outcome.advanced_round {
        msg = Some(format!(
            "Trang đã lưu thành công! Dự án đã chuyển sang vòng {adv_round}."
        ));
    } else if let Some(ref phase) = outcome.transitioned_phase {
        msg = Some(format!(
            "Dự án đã hoàn thành tất cả các vòng và chuyển sang giai đoạn {phase}!"
        ));
    }

    Ok(Json(SavePageResponse {
        ok: true,
        next_url,
        message: msg,
    })
    .into_response())
}

/// `POST /projects/{id}/pages/{n}/wordcheck` — Realtime wordcheck advisory feedback for CM6.
#[instrument(skip(state, _auth, payload))]
pub async fn proof_wordcheck(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path((id, page_number)): Path<(u64, i32)>,
    Json(payload): Json<WordcheckCheckPayload>,
) -> Result<Json<Vec<WordIssue>>, AppError> {
    let data = get_proofing_page_data(&state.db, id, page_number)
        .await?
        .ok_or(AppError::NotFound)?;

    let catalog = CharsetCatalog::default_embedded();
    let charset = catalog
        .get(&data.charset)
        .unwrap_or_else(|| catalog.default_vi());

    let normalized = viet::normalize(&payload.text);

    let (project_good, project_bad) = get_all_project_words(&state.db, id).await?;
    let site_words_guard = state.site_words.read().await;

    let tone_style = data.tone_style.parse::<ToneStyle>().unwrap_or_default();
    let wc_ctx = WordcheckContext {
        charset,
        tone_style,
        project_good: &project_good,
        project_bad: &project_bad,
        site_good: &site_words_guard.0,
        site_bad: &site_words_guard.1,
    };

    let issues = check_page_text(&normalized, &wc_ctx);
    Ok(Json(issues))
}

/// `POST /projects/{id}/pages/{n}/preview` — Render the format-preview fragment (§19 Phase 9).
///
/// The endpoint is read-only: it renders the submitted draft text with the
/// locked Appendix B markup and returns an HTMX fragment. CSRF is enforced even
/// though nothing is persisted, matching §12.4's rule for every non-GET request.
#[instrument(skip(state, auth, headers, payload))]
pub async fn proof_format_preview(
    auth: AuthUser,
    State(state): State<AppState>,
    Path((id, page_number)): Path<(u64, i32)>,
    headers: HeaderMap,
    Form(payload): Form<FormatPreviewPayload>,
) -> Result<Response, AppError> {
    let token = payload
        .csrf_token
        .as_deref()
        .or_else(|| {
            headers
                .get("x-csrf-token")
                .and_then(|value| value.to_str().ok())
        })
        .unwrap_or("");
    if !verify_csrf_token(token, &auth.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let data = get_proofing_page_data(&state.db, id, page_number)
        .await?
        .ok_or(AppError::NotFound)?;

    // Preview follows the same visibility rule as the scanned image: the proofer
    // holding the page and staff may always preview; an available or finished
    // page is visible to everyone who can reach the project.
    let is_mine = data.proofer_id == Some(auth.user_id.0);
    let is_staff = auth.has_role("admin") || auth.has_role("pm") || auth.has_role("pp");
    let claimed_by_other =
        data.round_state.as_deref() == Some("in_progress") && !is_mine && !is_staff;
    if claimed_by_other {
        return Err(AppError::Forbidden);
    }

    let normalized = viet::normalize(&payload.text);
    let preview_html = render_preview(&normalized);
    let is_empty = preview_html.is_empty();

    let template = state.templates.get_template("proof/_format_preview.html")?;
    let rendered = template.render(context! {
        preview_html => preview_html,
        is_empty => is_empty,
    })?;

    Ok(Html(rendered).into_response())
}

/// `POST /projects/{id}/pages/{n}/release` — Proofer voluntarily releases claim back to pool.
#[instrument(skip(state, auth, form))]
pub async fn proof_release(
    auth: AuthUser,
    State(state): State<AppState>,
    Path((id, page_number)): Path<(u64, i32)>,
    Form(form): Form<ReleaseClaimForm>,
) -> Result<Response, AppError> {
    if !verify_csrf_token(&form.csrf_token, &auth.csrf_token) {
        return Err(AppError::Forbidden);
    }

    let data = get_proofing_page_data(&state.db, id, page_number)
        .await?
        .ok_or(AppError::NotFound)?;

    let current_round = data
        .current_round
        .ok_or_else(|| AppError::BadRequest("Dự án chưa có vòng hoạt động.".to_string()))?;

    release_claim(&state.db, data.page_id, &current_round, auth.user_id.0).await?;

    Ok(Redirect::to(&format!("/projects/{id}")).into_response())
}

/// `GET /projects/{id}/pages/{n}/image` — Authorize proofer/manager and serve image scan (§15.2).
#[instrument(skip(state, auth))]
pub async fn proof_image(
    auth: AuthUser,
    State(state): State<AppState>,
    Path((id, page_number)): Path<(u64, i32)>,
) -> Result<Response, AppError> {
    let data = get_proofing_page_data(&state.db, id, page_number)
        .await?
        .ok_or(AppError::NotFound)?;

    // Authorization check (§15.2):
    // Proofer holding the page, or manager/admin/pp, or completed page
    let is_mine = data.proofer_id == Some(auth.user_id.0);
    let is_staff = auth.has_role("admin") || auth.has_role("pm") || auth.has_role("pp");
    let is_done = data.round_state.as_deref() == Some("done");

    if !is_mine && !is_staff && !is_done {
        return Err(AppError::Forbidden);
    }

    let accel_redirect = format!("/protected/{}", data.image_path);
    let content_type = if data.image_path.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };

    // In local dev/tests, stream file bytes directly if it exists on disk,
    // while always including the X-Accel-Redirect header required by §15.2
    let local_file = state.config.paths.data.join(&data.image_path);
    if local_file.exists() {
        let bytes = tokio::fs::read(&local_file)
            .await
            .map_err(|e| AppError::Internal(format!("failed to read image file: {e}")))?;

        return Ok((
            StatusCode::OK,
            [
                (
                    header::HeaderName::from_static("x-accel-redirect"),
                    HeaderValue::try_from(accel_redirect)
                        .map_err(|e| AppError::Internal(format!("invalid header value: {e}")))?,
                ),
                (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("private, max-age=86400"),
                ),
            ],
            bytes,
        )
            .into_response());
    }

    // Production behind nginx with X-Accel-Redirect
    Ok((
        StatusCode::OK,
        [
            (
                header::HeaderName::from_static("x-accel-redirect"),
                HeaderValue::try_from(accel_redirect)
                    .map_err(|e| AppError::Internal(format!("invalid header value: {e}")))?,
            ),
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=86400"),
            ),
        ],
    )
        .into_response())
}
