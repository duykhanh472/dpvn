//! Home page route handler.

use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use minijinja::context;

use crate::app::{AppError, AppState};
use crate::auth::OptionalAuthUser;

/// Landing and dashboard page handler.
pub async fn index(
    State(state): State<AppState>,
    OptionalAuthUser(auth): OptionalAuthUser,
) -> Result<Response, AppError> {
    let tmpl = state.templates.get_template("home.html")?;
    let csrf_token = auth.as_ref().map(|u| u.csrf_token.as_str()).unwrap_or("");
    let user_info = auth.as_ref().map(|u| {
        context! {
            username => u.username.as_str(),
            real_name => u.real_name.as_str(),
            roles => u.roles.clone(),
            is_admin => u.is_admin(),
            is_pm => u.is_pm(),
        }
    });

    let rendered = tmpl.render(context! {
        csrf_token => csrf_token,
        user => user_info,
    })?;
    Ok(Html(rendered).into_response())
}
