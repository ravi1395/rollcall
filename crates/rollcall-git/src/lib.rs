use std::path::{Path, PathBuf};

use rollcall_core::{DiscoveredRepo, Discovery, GitOptions, RepoState, WalkOptions};

pub mod classify;
pub mod open;
pub mod root;
pub mod state;
#[cfg(test)]
pub mod testgit;
pub mod trust;
pub mod walk;

/// Outcome of the §4.3 refused-root guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootCheck {
    Allowed,
    /// Up to 5 `rollcall <dir>` suggestions.
    Refused {
        suggestions: Vec<PathBuf>,
    },
}

pub fn check_root(root: &Path, opts: &WalkOptions) -> RootCheck {
    root::check_root(root, opts)
}

// P21 owns this body.
pub fn discover(_root: &Path, _opts: &WalkOptions) -> Discovery {
    unimplemented!()
}

// P21 owns this body.
pub fn repo_state(_repo: &DiscoveredRepo, _opts: &GitOptions) -> RepoState {
    unimplemented!()
}

pub fn user_excludes_file(home: &Path, xdg_config_home: Option<&Path>) -> Option<PathBuf> {
    open::user_excludes_file(home, xdg_config_home)
}
