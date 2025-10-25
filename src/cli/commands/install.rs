pub(crate) use install_command::cmd_install;

pub(crate) use fast::build_fast_instances;
pub(crate) use prune::{
    cleanup_empty_node_modules_dir, lockfile_has_no_packages, prune_removed_from_lock,
    prune_unreachable, remove_dirs,
};

mod download;
mod fast;
mod install_command;
mod manifest_updates;
mod node_modules;
mod platform;
mod progress;
mod prune;
mod util;
