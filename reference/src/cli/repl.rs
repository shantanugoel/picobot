pub enum ReplCommand {
    Quit,
    Clear,
    Permissions,
    Raw(String),
}

pub fn parse_command(input: &str) -> ReplCommand {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return ReplCommand::Raw(input.to_string());
    }

    match trimmed {
        "/quit" | "/exit" => ReplCommand::Quit,
        "/clear" => ReplCommand::Clear,
        "/permissions" => ReplCommand::Permissions,
        _ => ReplCommand::Raw(input.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{ReplCommand, parse_command};

    #[test]
    fn parse_command_returns_raw_for_plain_text() {
        match parse_command("hello") {
            ReplCommand::Raw(value) => assert_eq!(value, "hello"),
            _ => panic!("expected raw command"),
        }
    }

    #[test]
    fn parse_command_maps_quit_aliases() {
        assert!(matches!(parse_command("/quit"), ReplCommand::Quit));
        assert!(matches!(parse_command("/exit"), ReplCommand::Quit));
    }
}
