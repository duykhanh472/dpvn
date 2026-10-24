//! Home page route handler.

use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use minijinja::context;

use crate::app::{AppError, AppState};

/// Landing page handler.
pub async fn index(State(state): State<AppState>) -> Result<Response, AppError> {
    let tmpl = state.templates.get_template("home.html")?;
    let rendered = tmpl.render(context! {
        csrf_token => "",
    })?;
    Ok(Html(rendered).into_response())
}
