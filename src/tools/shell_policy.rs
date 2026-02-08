use std::collections::HashSet;

use glob::Pattern;

use crate::config::ShellPermissions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellRisk {
    Safe,
    Risky,
    Deny,
}

impl ShellRisk {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "safe" => Some(ShellRisk::Safe),
            "risky" => Some(ShellRisk::Risky),
            "deny" => Some(ShellRisk::Deny),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ShellRisk::Safe => "safe",
            ShellRisk::Risky => "risky",
            ShellRisk::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShellPolicyResult {
    pub risk: ShellRisk,
    pub reason: String,
    pub policy_key: String,
}

#[derive(Debug, Clone)]
pub struct ShellPolicy {
    deny_patterns: Vec<CommandPattern>,
    risky_patterns: Vec<CommandPattern>,
    safe_commands: HashSet<String>,
    default_risk: ShellRisk,
}

#[derive(Debug, Clone)]
struct CommandPattern {
    raw: String,
    tokens: Vec<Pattern>,
}

const DEFAULT_DENY_PATTERNS: &[&str] = &[
    "sudo *",
    "su *",
    "doas *",
    "chroot *",
    "rm -rf *",
    "rm -fr *",
    "rm --no-preserve-root *",
    "dd *",
    "wipefs *",
    "blkdiscard *",
    "mkfs* *",
    "fdisk *",
    "sfdisk *",
    "parted *",
    "diskutil *",
    "diskpart *",
    "format *",
    "cryptsetup *",
    "shutdown *",
    "reboot *",
    "halt *",
    "poweroff *",
    "init *",
    "systemctl *",
    "service *",
    "launchctl *",
    "sh *",
    "bash *",
    "zsh *",
    "fish *",
    "dash *",
    "ksh *",
    "csh *",
    "tcsh *",
    "cmd *",
    "python *",
    "python3 *",
    "node *",
    "ruby *",
    "perl *",
    "php *",
    "lua *",
    "rscript *",
    "R *",
    "deno *",
    "bun *",
    "pwsh *",
    "powershell *",
];

const DEFAULT_RISKY_PATTERNS: &[&str] = &[
    "rm *",
    "mv *",
    "cp *",
    "chmod *",
    "chown *",
    "chgrp *",
    "ln *",
    "mkdir *",
    "rmdir *",
    "touch *",
    "truncate *",
    "tee *",
    "sed *",
    "awk *",
    "tar *",
    "zip *",
    "unzip *",
    "7z *",
    "gzip *",
    "gunzip *",
    "bzip2 *",
    "xz *",
    "git *",
    "hg *",
    "svn *",
    "curl *",
    "wget *",
    "scp *",
    "rsync *",
    "ssh *",
    "sftp *",
    "ftp *",
    "nc *",
    "ncat *",
    "netcat *",
    "socat *",
    "openssl *",
    "mount *",
    "umount *",
    "iptables *",
    "ufw *",
    "pfctl *",
    "route *",
    "ifconfig *",
    "ip *",
    "docker *",
    "podman *",
    "kubectl *",
    "helm *",
    "terraform *",
    "ansible *",
    "make *",
    "cmake *",
    "npm *",
    "npx *",
    "pnpm *",
    "yarn *",
    "pip *",
    "pip3 *",
    "cargo *",
    "go *",
    "dotnet *",
    "java *",
    "javac *",
    "gradle *",
    "mvn *",
    "psql *",
    "mysql *",
    "sqlite3 *",
    "redis-cli *",
    "kill *",
    "killall *",
];

const DEFAULT_SAFE_COMMANDS: &[&str] = &[
    "ls",
    "pwd",
    "whoami",
    "date",
    "echo",
    "cat",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "fd",
    "find",
    "head",
    "tail",
    "wc",
    "uname",
    "which",
    "stat",
    "du",
    "df",
    "ps",
    "top",
    "uptime",
    "id",
    "groups",
    "hostname",
    "who",
    "w",
    "env",
    "printenv",
    "sort",
    "uniq",
    "cut",
    "tr",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "file",
    "lsblk",
    "free",
    "vm_stat",
    "ipconfig",
    "systeminfo",
    "tasklist",
];

impl Default for ShellPolicy {
    fn default() -> Self {
        Self {
            deny_patterns: build_patterns(&strings_from_defaults(DEFAULT_DENY_PATTERNS)),
            risky_patterns: build_patterns(&strings_from_defaults(DEFAULT_RISKY_PATTERNS)),
            safe_commands: DEFAULT_SAFE_COMMANDS
                .iter()
                .map(|cmd| (*cmd).to_string())
                .collect(),
            default_risk: ShellRisk::Safe,
        }
    }
}

impl ShellPolicy {
    pub fn from_config(shell: Option<&ShellPermissions>) -> Self {
        let config = shell.and_then(|shell| shell.policy.as_ref());
        let default_risk = config
            .and_then(|policy| policy.default_risk.as_deref())
            .and_then(ShellRisk::parse)
            .unwrap_or(ShellRisk::Safe);
        let deny_patterns = config
            .and_then(|policy| policy.deny_patterns.clone())
            .unwrap_or_else(|| strings_from_defaults(DEFAULT_DENY_PATTERNS));
        let risky_patterns = config
            .and_then(|policy| policy.risky_patterns.clone())
            .unwrap_or_else(|| strings_from_defaults(DEFAULT_RISKY_PATTERNS));
        let safe_commands = config
            .and_then(|policy| policy.safe_commands.clone())
            .unwrap_or_else(|| strings_from_defaults(DEFAULT_SAFE_COMMANDS));

        Self {
            deny_patterns: build_patterns(&deny_patterns),
            risky_patterns: build_patterns(&risky_patterns),
            safe_commands: safe_commands
                .into_iter()
                .map(|command| command.trim().to_string())
                .filter(|command| !command.is_empty())
                .collect(),
            default_risk,
        }
    }

    pub fn classify(&self, command: &str, args: &[String]) -> ShellPolicyResult {
        let policy_key = command_line_key(command, args);
        if let Some(pattern) = self
            .deny_patterns
            .iter()
            .find(|pattern| pattern.matches(command, args))
        {
            return ShellPolicyResult {
                risk: ShellRisk::Deny,
                reason: format!("matched deny pattern '{}'", pattern.raw),
                policy_key,
            };
        }
        if let Some(pattern) = self
            .risky_patterns
            .iter()
            .find(|pattern| pattern.matches(command, args))
        {
            return ShellPolicyResult {
                risk: ShellRisk::Risky,
                reason: format!("matched risky pattern '{}'", pattern.raw),
                policy_key,
            };
        }
        if self.safe_commands.contains(command) {
            return ShellPolicyResult {
                risk: ShellRisk::Safe,
                reason: "command marked safe".to_string(),
                policy_key,
            };
        }
        ShellPolicyResult {
            risk: self.default_risk,
            reason: format!("default risk '{}'", self.default_risk.label()),
            policy_key,
        }
    }
}

impl CommandPattern {
    fn new(raw: &str) -> Result<Self, glob::PatternError> {
        let tokens = raw
            .split_whitespace()
            .map(Pattern::new)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            raw: raw.to_string(),
            tokens,
        })
    }

    fn matches(&self, command: &str, args: &[String]) -> bool {
        if self.tokens.is_empty() {
            return false;
        }
        let mut parts = Vec::with_capacity(args.len() + 1);
        parts.push(command);
        for arg in args {
            parts.push(arg.as_str());
        }
        if self.tokens.len() > parts.len() {
            return false;
        }
        self.tokens
            .iter()
            .zip(parts.iter())
            .all(|(pattern, value)| pattern.matches(value))
    }
}

fn command_line_key(command: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(command.to_string());
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

fn strings_from_defaults(defaults: &[&str]) -> Vec<String> {
    defaults.iter().map(|value| (*value).to_string()).collect()
}

fn build_patterns(patterns: &[String]) -> Vec<CommandPattern> {
    patterns
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty())
        .filter_map(|pattern| CommandPattern::new(pattern).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{build_patterns, ShellPolicy, ShellPolicyResult, ShellRisk};

    fn policy_result(policy: &ShellPolicy, command: &str, args: &[&str]) -> ShellPolicyResult {
        let args = args
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        policy.classify(command, &args)
    }

    #[test]
    fn classify_prefers_deny_over_risky() {
        let policy = ShellPolicy {
            deny_patterns: build_patterns(&vec!["rm -rf *".to_string()]),
            risky_patterns: build_patterns(&vec!["rm *".to_string()]),
            safe_commands: ["ls".to_string()].into_iter().collect(),
            default_risk: ShellRisk::Risky,
        };
        let result = policy_result(&policy, "rm", &["-rf", "/"]);
        assert_eq!(result.risk, ShellRisk::Deny);
    }

    #[test]
    fn classify_marks_safe_commands() {
        let policy = ShellPolicy {
            deny_patterns: build_patterns(&vec![]),
            risky_patterns: build_patterns(&vec!["rm *".to_string()]),
            safe_commands: ["ls".to_string()].into_iter().collect(),
            default_risk: ShellRisk::Risky,
        };
        let result = policy_result(&policy, "ls", &[]);
        assert_eq!(result.risk, ShellRisk::Safe);
    }

    #[test]
    fn classify_uses_default_risk_when_unmatched() {
        let policy = ShellPolicy {
            deny_patterns: build_patterns(&vec![]),
            risky_patterns: build_patterns(&vec![]),
            safe_commands: std::collections::HashSet::new(),
            default_risk: ShellRisk::Risky,
        };
        let result = policy_result(&policy, "cat", &["/tmp/example.txt"]);
        assert_eq!(result.risk, ShellRisk::Risky);
    }
}
