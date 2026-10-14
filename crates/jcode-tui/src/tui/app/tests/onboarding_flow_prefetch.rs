// Onboarding flow: recent-project prefetch.


#[test]
fn start_choice_prefetches_recent_project_so_enter_does_not_block() {
    let mut app = onboarding_test_app();
    assert!(
        app.onboarding_recent_project_prefetch.is_none(),
        "no prefetch before the start choice is shown"
    );

    app.onboarding_open_start_choice();

    let slot = app
        .onboarding_recent_project_prefetch
        .clone()
        .expect("opening the start choice should warm the recent-project lookup");

    // Wait briefly for the background scan; the action must not depend on it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if slot.lock().expect("prefetch slot").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        slot.lock().expect("prefetch slot").is_some(),
        "prefetch should resolve in the background"
    );

    // Opening the choice twice must not spawn a second scan.
    app.onboarding_open_start_choice();
    assert!(
        std::sync::Arc::ptr_eq(
            &slot,
            app.onboarding_recent_project_prefetch
                .as_ref()
                .expect("prefetch retained")
        ),
        "the warm prefetch should be reused"
    );

    // The resolved path is still the repository the session runs in.
    assert_eq!(
        app.onboarding_recent_project_path(),
        crate::import::repo_ranking::resolve_git_root(std::path::Path::new(
            app.session
                .working_dir
                .as_deref()
                .expect("test session working dir")
        ))
    );
}
