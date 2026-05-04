use std::path::PathBuf;

use assert_cmd::assert::Assert;

use crate::task_flow_helpers::parse_value;

pub(crate) fn stdout_text(assert: &Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

pub(crate) fn path_from_stdout(stdout: &str, key: &str) -> PathBuf {
    PathBuf::from(parse_value(key, stdout).unwrap())
}

pub(crate) fn path_from_assert(assert: &Assert, key: &str) -> PathBuf {
    path_from_stdout(&stdout_text(assert), key)
}

pub(crate) fn task_worktree_from_assert(assert: &Assert) -> PathBuf {
    path_from_assert(assert, "TASK_WORKTREE")
}
