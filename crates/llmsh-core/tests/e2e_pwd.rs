//! Verify shared cwd plumbing: try_chdir updates both the shared lock and
//! the process cwd, so a downstream consumer (executor or sysctx) sees the
//! new value via either source.

use llmsh_core::cwd;
use std::path::PathBuf;

#[test]
fn try_chdir_updates_shared_lock_and_process_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("sub");
    std::fs::create_dir(&sub).unwrap();

    // Save the current process cwd so concurrent tests don't drift.
    let saved = std::env::current_dir().ok();

    let shared = cwd::new_shared(tmp.path().to_path_buf());
    let new = cwd::try_chdir(&shared, &sub).unwrap();

    let canonical_sub = std::fs::canonicalize(&sub).unwrap();
    assert_eq!(new, canonical_sub);
    assert_eq!(cwd::snapshot(&shared), canonical_sub);
    let live = std::env::current_dir().unwrap();
    let canonical_live = std::fs::canonicalize(&live).unwrap_or(live);
    assert_eq!(canonical_live, canonical_sub);

    if let Some(p) = saved {
        let _ = std::env::set_current_dir(p);
    }
}

#[test]
fn try_chdir_rejects_nonexistent() {
    let shared = cwd::new_shared(PathBuf::from("/"));
    let err = cwd::try_chdir(&shared, &PathBuf::from("/nope_does_not_exist_xyz_99")).unwrap_err();
    assert!(matches!(err, cwd::ChdirError::NotFound));
}

#[test]
fn resolve_cd_target_handles_dash_tilde_and_relative() {
    let home = PathBuf::from("/home/u");
    // empty → home
    assert_eq!(
        cwd::resolve_cd_target(None, &PathBuf::from("/x"), Some(&home), None).unwrap(),
        home
    );
    // - → oldpwd
    let old = PathBuf::from("/old");
    assert_eq!(
        cwd::resolve_cd_target(Some("-"), &PathBuf::from("/x"), Some(&home), Some(&old)).unwrap(),
        old
    );
    // ~/foo
    assert_eq!(
        cwd::resolve_cd_target(Some("~/foo"), &PathBuf::from("/x"), Some(&home), None).unwrap(),
        home.join("foo")
    );
    // relative
    assert_eq!(
        cwd::resolve_cd_target(Some("foo"), &PathBuf::from("/base"), None, None).unwrap(),
        PathBuf::from("/base/foo")
    );
}
