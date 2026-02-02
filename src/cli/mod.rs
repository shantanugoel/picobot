pub mod repl;
pub mod tui;

use crate::config::PermissionsConfig;

pub fn format_permissions(config: Option<&PermissionsConfig>) -> String {
    let mut lines = Vec::new();
    match config {
        Some(config) => {
            if let Some(filesystem) = &config.filesystem {
                lines.push(format!(
                    "filesystem.read_paths: {}",
                    format_list(&filesystem.read_paths)
                ));
                lines.push(format!(
                    "filesystem.write_paths: {}",
                    format_list(&filesystem.write_paths)
                ));
            } else {
                lines.push("filesystem: (none)".to_string());
            }

            if let Some(network) = &config.network {
                lines.push(format!(
                    "network.allowed_domains: {}",
                    format_list(&network.allowed_domains)
                ));
            } else {
                lines.push("network: (none)".to_string());
            }

            if let Some(shell) = &config.shell {
                lines.push(format!(
                    "shell.allowed_commands: {}",
                    format_list(&shell.allowed_commands)
                ));
                if let Some(working_directory) = &shell.working_directory {
                    lines.push(format!("shell.working_directory: {working_directory}"));
                }
            } else {
                lines.push("shell: (none)".to_string());
            }
        }
        None => {
            lines.push("Permissions: (none configured)".to_string());
        }
    }

    lines.join("\n")
}

fn format_list(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}
