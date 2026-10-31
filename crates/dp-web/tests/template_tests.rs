use dp_web::{AppConfig, AssetManifest, create_template_env};
use minijinja::context;
use std::sync::Arc;

fn fixture_context() -> minijinja::Value {
    context! {
        csrf_token => "test-csrf-token",
        status_code => 404,
        message => "Không tìm thấy trang yêu cầu.",
        user => context! {
            username => "testuser",
            roles => vec!["proofer", "pm"],
        },
        error => minijinja::Value::UNDEFINED,
        success => minijinja::Value::UNDEFINED,
        username => "testuser",
        email => "test@example.com",
        real_name => "Test User",
        // Project templates
        project => context! {
            id => 1u64,
            name => "Truyện Kiều",
            author => "Nguyễn Du",
            phase => "draft",
            current_round => minijinja::Value::UNDEFINED,
            on_hold => 0i8,
            tone_style => "either",
            charset => "vi",
            manager_id => 1u64,
            manager_username => "testuser",
            created_at => "2024-01-01T00:00:00+00:00",
            updated_at => "2024-01-01T00:00:00+00:00",
        },
        page_count => 0i64,
        projects => Vec::<minijinja::Value>::new(),
        preview => minijinja::Value::UNDEFINED,
        my_projects => minijinja::Value::UNDEFINED,
    }
}

#[test]
fn test_all_templates_render() {
    let config = AppConfig::default();
    let manifest = Arc::new(AssetManifest::default());
    let env = create_template_env(&config, manifest).expect("failed to load template environment");

    let ctx = fixture_context();

    let mut count = 0;
    for (name, tmpl) in env.templates() {
        // Skip macros files which aren't standalone renderable pages
        if name.ends_with("macros.html") || name.starts_with('_') {
            continue;
        }

        let rendered = tmpl
            .render(&ctx)
            .unwrap_or_else(|e| panic!("failed to render template {name}: {e}"));

        assert!(
            !rendered.is_empty(),
            "template {name} rendered to an empty string"
        );
        count += 1;
    }

    assert!(count > 0, "no templates were found to render");
}
