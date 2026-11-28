use super::*;

fn context(windows: bool) -> PlanEdits {
    PlanEdits {
        windows: Some(windows),
        cwd: Some(
            if windows {
                r"C:\workspace"
            } else {
                "/workspace"
            }
            .into(),
        ),
        home: Some(
            if windows {
                r"C:\Users\test"
            } else {
                "/home/test"
            }
            .into(),
        ),
    }
}

#[test]
fn markdown_types_are_not_limited_to_the_plan_folder() {
    for suffix in ["md", "markdown", "mdown", "mkd", "mkdn", "mdx"] {
        for plan in [context(false), context(true)] {
            assert!(plan.validate(&format!("notes.{suffix}")).is_ok());
            assert!(plan
                .validate(&format!("notes.{}", suffix.to_uppercase()))
                .is_ok());
        }
    }
    for path in [
        "source.rs",
        "notes.md.exe",
        "notes.md/../source.rs",
        "notes.md/..",
    ] {
        assert!(context(false).validate(path).is_err(), "{path}");
    }
}

#[test]
fn canvas_policy_checks_the_resolved_direct_child() {
    for prefix in ["/workspace", "C:/workspace", "//server/share"] {
        let plan = context(prefix != "/workspace");
        let canvas = format!("{prefix}/.cursor/projects/project/canvases");
        for leaf in [
            "board.canvas.tsx",
            "nested/../board.canvas.tsx",
            "BOARD.CANVAS.TSX",
        ] {
            assert!(
                plan.validate(&format!("{canvas}/{leaf}")).is_ok(),
                "{prefix}/{leaf}"
            );
        }
        for leaf in [
            "nested/board.canvas.tsx",
            "../board.canvas.tsx",
            "board.tsx",
            "board.canvas.tsx/../../src.tsx",
        ] {
            assert!(
                plan.validate(&format!("{canvas}/{leaf}")).is_err(),
                "{prefix}/{leaf}"
            );
        }
    }
}

#[test]
fn windows_paths_do_not_use_the_server_platform() {
    for path in [
        r"D:\docs\plan.md",
        r"\\server\share\plan.md",
        r"C:\workspace\.cursor\projects\p\canvases\board.canvas.tsx",
    ] {
        assert!(PlanEdits::default().validate(path).is_ok(), "{path}");
    }
    let plan = context(true);
    assert_eq!(plan.resolve(r"\docs\plan.md").unwrap(), r"C:\docs\plan.md");
    assert!(plan
        .validate(r"C:\workspace\.cursor\projects\p\canvases\..\source.canvas.tsx")
        .is_err());
}

#[test]
fn file_uris_check_the_actual_path_not_the_query_or_fragment() {
    for (plan, root) in [
        (context(false), "file:///workspace"),
        (context(true), "file:///C:/workspace"),
    ] {
        assert!(plan
            .validate(&format!("{root}/my%20plan.md?ignored=.rs#section"))
            .is_ok());
        assert!(plan.validate(&format!("{root}/main.rs?fake=.md")).is_err());
        assert!(plan.validate(&format!("{root}/main.rs#fake.md")).is_err());
        assert!(plan.validate(&format!("{root}/%FF.md")).is_err());
        assert!(plan.validate(&format!("{root}/invalid%.md")).is_err());
        assert!(plan.validate(&format!("{root}/%00.md")).is_err());
        assert!(plan.validate(&format!("{root}/%2fnotes.md")).is_err());
    }
    assert!(context(true)
        .validate("file://server/share/docs/plan.md")
        .is_ok());
    assert!(context(false)
        .validate("file://server/share/docs/plan.md")
        .is_err());
    assert!(PlanEdits::default()
        .validate("file:///C:/docs/plan.md")
        .is_ok());
}

#[test]
fn relative_and_home_paths_require_client_context() {
    let plan = context(false);
    assert_eq!(
        plan.resolve("../plans/notes.md").unwrap(),
        "/plans/notes.md"
    );
    assert_eq!(
        plan.resolve("~/plans/notes.md").unwrap(),
        "/home/test/plans/notes.md"
    );
    let plan = PlanEdits {
        cwd: Some("/remote/.cursor/projects/p/canvases".into()),
        ..context(false)
    };
    assert!(plan.validate("board.canvas.tsx").is_ok());
    assert!(plan.validate("../board.canvas.tsx").is_err());
    for path in ["notes.md", "~/notes.md", r"D:notes.md"] {
        assert!(PlanEdits::default().validate(path).is_err(), "{path}");
    }
    assert!(context(true).validate(r"D:notes.md").is_err());
}

#[test]
fn home_is_only_known_when_the_client_identifies_its_cwd_as_home() {
    let mut env = pb::RequestContextEnv {
        os_version: "linux 6.8".into(),
        process_working_directory: Some("/remote/user".into()),
        workspace_paths: vec!["/misleading/workspace".into()],
        ..Default::default()
    };
    assert!(PlanEdits::new(Some(&env)).validate("~/notes.md").is_err());
    env.is_working_dir_home_dir = Some(true);
    assert_eq!(
        PlanEdits::new(Some(&env)).resolve("~/notes.md").unwrap(),
        "/remote/user/notes.md"
    );
}

#[test]
fn path_suffix_uses_ecmascript_whitespace() {
    for c in ['\u{a0}', '\u{feff}', '\u{2028}', '\u{2029}', '\u{2009}'] {
        assert!(context(false)
            .validate(&format!("/workspace/notes.md{c}"))
            .is_ok());
    }
    for c in ['\u{85}', '\u{200b}', '\u{180e}'] {
        assert!(context(false)
            .validate(&format!("/workspace/notes.md{c}"))
            .is_err());
    }
}

#[test]
fn matches_native_path_policy_for_valid_paths_and_file_uris() {
    // Cursor 3.31.15's isolated tie/kLe probe, synthetic cwd/home as above.
    // Malformed file URIs are rejected, rather than copied as literal paths.
    // Source and extraction hashes: docs/history/issues/2026-11-28-issue139-plan-contract.md.
    let cases: Vec<(bool, String, bool)> =
        serde_json::from_str(include_str!("native-path-cases.json")).unwrap();
    let mut mismatches = Vec::new();
    for (windows, path, allowed) in cases {
        let actual = context(windows).validate(&path);
        if actual.is_ok() != allowed {
            mismatches.push(format!(
                "windows={windows} path={path:?} expected={allowed} actual={actual:?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
