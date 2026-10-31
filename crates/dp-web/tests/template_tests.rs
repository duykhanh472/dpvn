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
            created_at_label => "01/01/2024",
            updated_at_label => "01/01/2024 00:00",
            image_source => "Scans",
            comments => "Ghi chú dự án",
        },
        page_count => 0i64,
        projects => Vec::<minijinja::Value>::new(),
        preview => minijinja::Value::UNDEFINED,
        my_projects => false,
        base_path => "/projects",
        round_options => vec![
            context! { value => "all", label => "Tất cả các vòng", selected => true },
        ],
        availability_options => vec![
            context! { value => "available", label => "Có trang chờ soát", selected => true },
        ],
        sort_options => vec![
            context! { value => "recent", label => "Cập nhật gần đây", selected => true },
        ],
        per_options => vec![
            context! { value => 25u32, label => "25 / trang", selected => true },
        ],
        filter_round => "all",
        filter_availability => "available",
        filter_sort => "recent",
        per_page => 25u32,
        total_pages_count => 1u32,
        has_previous => false,
        has_next => false,
        previous_url => "",
        next_url => "",
        page_links => Vec::<minijinja::Value>::new(),
        progress => context! {
            total_pages => 0i64,
            available_pages => 0i64,
            in_progress_pages => 0i64,
            done_pages => 0i64,
            percent => 0u32,
        },
        comments => Vec::<minijinja::Value>::new(),
        comment_count => 0i64,
        events => Vec::<minijinja::Value>::new(),
        can_view_events => true,
        can_manage => true,
        limit_reached => false,
        notice => "",
        // Proofing workspace fixture fields
        project_id => 1u64,
        project_name => "Truyện Kiều",
        author => "Nguyễn Du",
        page_number => 1i32,
        total_pages => 10i64,
        current_round => "P1",
        round_state => "in_progress",
        is_readonly => false,
        image_url => "/projects/1/pages/1/image",
        image_width => 1500u32,
        image_height => 2200u32,
        initial_text => "Văn bản mẫu",
        palette => vec!["đ", "…", "—"],
        daily_tally => 5u32,
        daily_limit => 100u32,
        issues => Vec::<minijinja::Value>::new(),
        confirmed_hash => "dummyhash",
        words => Vec::<minijinja::Value>::new(),
        total_count => 0i64,
        current_page => 1u32,
        total_pages => 1u32,
        filter_kind => "",
        filter_q => "",
        // Diff view fixture fields (§10)
        from_label => "OCR",
        to_label => "Vòng P1",
        from_value => "ocr",
        to_value => "P1",
        from_options => vec![
            context! { value => "ocr", label => "OCR", selected => true },
            context! { value => "P1", label => "Vòng P1", selected => false },
        ],
        to_options => vec![
            context! { value => "ocr", label => "OCR", selected => false },
            context! { value => "P1", label => "Vòng P1", selected => true },
        ],
        changes_only => false,
        whitespace => false,
        has_changes => true,
        rows => vec![
            context! {
                kind => "equal",
                old_line => 1u32,
                new_line => 1u32,
                old_html => "Dòng giữ nguyên",
                new_html => "Dòng giữ nguyên",
            },
            context! {
                kind => "replace",
                old_line => 2u32,
                new_line => 2u32,
                old_html => "Tôi <del aria-label=\"đã xoá\">nghỉ</del> học",
                new_html => "Tôi <ins aria-label=\"đã thêm\">nghĩ</ins> học",
            },
        ],
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
        // Skip partials / macros files which aren't standalone renderable pages
        let filename = name.split('/').next_back().unwrap_or(name);
        if filename.starts_with('_') || filename.ends_with("macros.html") {
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

#[test]
fn test_diff_template_snapshot() {
    let config = AppConfig::default();
    let manifest = Arc::new(AssetManifest::default());
    let env = create_template_env(&config, manifest).expect("failed to load template environment");
    let template = env
        .get_template("projects/diff.html")
        .expect("diff template must be embedded");

    let html = template
        .render(fixture_context())
        .expect("diff template must render");
    insta::assert_snapshot!("diff_template", html);
}
