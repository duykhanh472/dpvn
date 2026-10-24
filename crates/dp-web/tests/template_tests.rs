use dp_web::{AppConfig, AssetManifest, create_template_env};
use minijinja::context;
use std::sync::Arc;

#[test]
fn test_all_templates_render() {
    let config = AppConfig::default();
    let manifest = Arc::new(AssetManifest::default());
    let env = create_template_env(&config, manifest).expect("failed to load template environment");

    let fixture_context = context! {
        csrf_token => "test-csrf-token",
        status_code => 404,
        message => "Không tìm thấy trang yêu cầu.",
    };

    let mut count = 0;
    for (name, tmpl) in env.templates() {
        // Skip macros files which aren't standalone renderable pages
        if name.ends_with("macros.html") || name.starts_with('_') {
            continue;
        }

        let rendered = tmpl
            .render(&fixture_context)
            .unwrap_or_else(|e| panic!("failed to render template {name}: {e}"));

        assert!(
            !rendered.is_empty(),
            "template {name} rendered to an empty string"
        );
        count += 1;
    }

    assert!(count > 0, "no templates were found to render");
}
